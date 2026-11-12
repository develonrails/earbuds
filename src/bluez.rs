//! BlueZ over D-Bus: which JBL headset is connected, and where its control
//! service advertises.
//!
//! The buds have two identities. The paired headset (public address) carries
//! the audio; the control service lives on a separate LE advertiser named
//! "<name>-LE" at a random address that rotates. That one is found by a short
//! scan whenever the control link has to be opened.

use std::{collections::HashMap, time::Duration};

use futures_util::StreamExt;
use zbus::{
    Connection, MatchRule, Message, MessageStream,
    fdo::ObjectManagerProxy,
    message::Type,
    zvariant::{Dict, ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

use crate::protocol::{HARMAN_SERVICE, JBL_VENDOR_ID};

const BLUEZ: &str = "org.bluez";
const DEVICE: &str = "org.bluez.Device1";

type Props = HashMap<String, OwnedValue>;

#[zbus::proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez")]
trait Adapter {
    fn set_discovery_filter(&self, filter: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn start_discovery(&self) -> zbus::Result<()>;
    fn stop_discovery(&self) -> zbus::Result<()>;
    fn remove_device(&self, device: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez")]
trait Device1 {
    fn disconnect(&self) -> zbus::Result<()>;
}

#[derive(Clone, Debug, Default)]
pub struct Device {
    pub path: String,
    pub address: String,
    pub name: String,
    random: bool,
    connected: bool,
    paired: bool,
    modalias: String,
    seen_now: bool,
    harman_data: bool,
}

fn text(props: &Props, key: &str) -> String {
    props
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or_default()
        .to_owned()
}

fn flag(props: &Props, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
}

impl Device {
    fn new(path: &str, props: &Props) -> Device {
        let name = Some(text(props, "Name")).filter(|n| !n.is_empty());
        let harman_data = props
            .get("ServiceData")
            .and_then(|v| <&Dict>::try_from(v).ok())
            .is_some_and(|d| {
                d.iter().any(|(k, _)| {
                    <&str>::try_from(k).is_ok_and(|k| k.eq_ignore_ascii_case(HARMAN_SERVICE))
                })
            });
        Device {
            path: path.to_owned(),
            address: text(props, "Address"),
            name: name.unwrap_or_else(|| text(props, "Alias")),
            random: text(props, "AddressType") == "random",
            connected: flag(props, "Connected"),
            paired: flag(props, "Paired"),
            modalias: text(props, "Modalias").to_ascii_lowercase(),
            seen_now: props.contains_key("RSSI"),
            harman_data,
        }
    }

    /// A paired JBL headset: its classic identity, not the "-LE" advertiser.
    fn is_jbl_headset(&self) -> bool {
        !self.random
            && !self.name.ends_with("-LE")
            && (self
                .modalias
                .starts_with(&format!("bluetooth:v{JBL_VENDOR_ID:04x}"))
                || self.name.to_ascii_uppercase().starts_with("JBL"))
    }

    /// The buds' control side: a random-address advertiser named "<name>-LE"
    /// (Wave Buds 2, Tune 230NC), or one carrying Harman's service data.
    fn is_control_advertiser(&self, headset: &str) -> bool {
        self.random
            && (self.name == format!("{headset}-LE")
                || (self.harman_data && self.name.to_ascii_uppercase().starts_with("JBL")))
    }

    fn adapter(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map_or("/org/bluez/hci0", |(a, _)| a)
    }
}

pub struct Bluez {
    conn: Connection,
}

impl Bluez {
    pub async fn connect() -> zbus::Result<Bluez> {
        let conn = Connection::system().await?;
        let bluez = Bluez { conn };
        bluez.devices().await?; // fails here, not later, if BlueZ is not there
        Ok(bluez)
    }

    async fn devices(&self) -> zbus::Result<Vec<Device>> {
        let om = ObjectManagerProxy::builder(&self.conn)
            .destination(BLUEZ)?
            .path("/")?
            .build()
            .await?;
        Ok(om
            .get_managed_objects()
            .await?
            .iter()
            .filter_map(|(path, ifaces)| {
                ifaces
                    .iter()
                    .find(|(i, _)| i.as_str() == DEVICE)
                    .map(|(_, p)| Device::new(path, p))
            })
            .collect())
    }

    /// The connected JBL headset, if there is one.
    pub async fn headset(&self) -> zbus::Result<Option<Device>> {
        Ok(self
            .devices()
            .await?
            .into_iter()
            .find(|d| d.connected && d.is_jbl_headset()))
    }

    fn signals(&self) -> MatchRule<'static> {
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(BLUEZ)
            .expect("bus name")
            .build()
    }

    /// Every signal BlueZ sends; see [`is_connection_change`] to pick out
    /// the ones that matter while waiting.
    pub async fn changes(&self) -> zbus::Result<MessageStream> {
        MessageStream::for_match_rule(self.signals(), &self.conn, Some(64)).await
    }

    async fn props(&self, path: &str) -> zbus::Result<Props> {
        let reply = self
            .conn
            .call_method(
                Some(BLUEZ),
                path,
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &(DEVICE,),
            )
            .await?;
        reply.body().deserialize()
    }

    async fn adapter(&self, path: &str) -> zbus::Result<AdapterProxy<'static>> {
        AdapterProxy::builder(&self.conn)
            .path(path.to_owned())?
            .build()
            .await
    }

    /// Scans briefly for the headset's control advertiser and returns its
    /// address. Only an advertiser seen during this scan counts: an address
    /// BlueZ remembers from earlier has most likely rotated away.
    pub async fn find_advertiser(
        &self,
        headset: &Device,
        wait: Duration,
    ) -> zbus::Result<Option<String>> {
        let mut stream =
            MessageStream::for_match_rule(self.signals(), &self.conn, Some(256)).await?;
        let adapter = self.adapter(headset.adapter()).await?;
        let filter = HashMap::from([
            ("Transport", Value::from("le")),
            ("DuplicateData", Value::from(true)),
        ]);
        adapter.set_discovery_filter(filter).await?;
        adapter.start_discovery().await?;
        let search = async {
            while let Some(Ok(msg)) = stream.next().await {
                if let Some(device) = self.advertised(&msg).await
                    && device.is_control_advertiser(&headset.name)
                {
                    return Some(device.address);
                }
            }
            None
        };
        let found = tokio::time::timeout(wait, search).await.ok().flatten();
        let _ = adapter.stop_discovery().await;
        let _ = adapter.set_discovery_filter(HashMap::new()).await;
        Ok(found)
    }

    /// The device a discovery signal is about, if it was seen just now.
    async fn advertised(&self, msg: &Message) -> Option<Device> {
        let header = msg.header();
        match header.member()?.as_str() {
            "InterfacesAdded" => {
                let (path, ifaces): (OwnedObjectPath, HashMap<String, Props>) =
                    msg.body().deserialize().ok()?;
                let device = Device::new(&path, ifaces.get(DEVICE)?);
                device.seen_now.then_some(device)
            }
            "PropertiesChanged" => {
                let (iface, changed, _): (String, Props, Vec<String>) =
                    msg.body().deserialize().ok()?;
                if iface != DEVICE
                    || !(changed.contains_key("RSSI") || changed.contains_key("ServiceData"))
                {
                    return None;
                }
                let path = header.path()?.to_string();
                let props = self.props(&path).await.ok()?;
                Some(Device::new(&path, &props))
            }
            _ => None,
        }
    }

    /// Disconnects and forgets BlueZ's object for an advertiser address, so
    /// bluetoothd does not take the ATT bearer from our socket. Only ever
    /// touches unpaired random-address objects, never the headset itself.
    pub async fn release(&self, le_address: &str) -> bool {
        let Ok(devices) = self.devices().await else {
            return false;
        };
        let Some(device) = devices
            .into_iter()
            .find(|d| d.address.eq_ignore_ascii_case(le_address))
        else {
            return false;
        };
        if !device.random || device.paired {
            return false;
        }
        if device.connected
            && let Ok(builder) = Device1Proxy::builder(&self.conn).path(device.path.clone())
            && let Ok(proxy) = builder.build().await
        {
            let _ = proxy.disconnect().await;
        }
        let Ok(adapter) = self.adapter(device.adapter()).await else {
            return false;
        };
        let Ok(path) = ObjectPath::try_from(device.path.as_str()) else {
            return false;
        };
        adapter.remove_device(&path).await.is_ok()
    }
}

/// Whether a BlueZ signal may mean a headset came or went: a device's
/// Connected changed, or a device or adapter appeared or disappeared. The
/// stream also carries every RSSI update of any scan, which do not.
pub fn is_connection_change(msg: &Message) -> bool {
    let header = msg.header();
    match header.member().map(|m| m.as_str()) {
        Some("InterfacesAdded" | "InterfacesRemoved") => true,
        Some("PropertiesChanged") => msg
            .body()
            .deserialize::<(String, Props, Vec<String>)>()
            .is_ok_and(|(iface, changed, _)| {
                (iface == DEVICE && changed.contains_key("Connected"))
                    || (iface == "org.bluez.Adapter1" && changed.contains_key("Powered"))
            }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(entries: &[(&str, Value<'_>)]) -> Props {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v.clone()).unwrap()))
            .collect()
    }

    #[test]
    fn the_paired_wave_buds_2_is_a_headset() {
        let d = Device::new(
            "/org/bluez/hci0/dev_74_68_59_4C_B7_58",
            &props(&[
                ("Address", Value::from("74:68:59:4C:B7:58")),
                ("AddressType", Value::from("public")),
                ("Name", Value::from("JBL Wave Buds 2")),
                ("Modalias", Value::from("bluetooth:v0ECBp2100d001F")),
                ("Connected", Value::from(true)),
            ]),
        );
        assert!(d.is_jbl_headset());
        assert_eq!(d.adapter(), "/org/bluez/hci0");
    }

    #[test]
    fn its_le_twin_is_the_control_advertiser() {
        let d = Device::new(
            "/org/bluez/hci0/dev_5A_11_22_33_44_55",
            &props(&[
                ("Address", Value::from("5A:11:22:33:44:55")),
                ("AddressType", Value::from("random")),
                ("Name", Value::from("JBL Wave Buds 2-LE")),
                ("RSSI", Value::from(-60i16)),
            ]),
        );
        assert!(!d.is_jbl_headset());
        assert!(d.seen_now);
        assert!(d.is_control_advertiser("JBL Wave Buds 2"));
        assert!(!d.is_control_advertiser("JBL Tune Beam 2"));
    }
}
