//! What the window shows: the link, and what the buds have said about
//! themselves. A setting the buds never answered for stays `None`, and the
//! window leaves it out: that is the whole capability model.

use crate::protocol::{AutoOff, Eq, Fact, Level, Mode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// BlueZ is not there (no adapter, or the service is down).
    NoBluetooth,
    /// No JBL headset connected.
    #[default]
    NoDevice,
    /// Connected as a headset; looking for its control advertiser.
    Searching,
    Connecting,
    Ready,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Battery {
    pub left: Option<Level>,
    pub right: Option<Level>,
    pub case: Option<Level>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub link: Link,
    pub name: Option<String>,
    pub firmware: Option<String>,
    pub battery: Battery,
    /// Off included; empty until the buds answer.
    pub modes: Vec<Mode>,
    pub mode: Option<Mode>,
    pub eq: Option<Eq>,
    pub voice_aware: Option<u8>,
    /// The second VoiceAware byte, sent back as reported.
    pub voice_aware_extra: u8,
    pub voice_prompts: Option<bool>,
    pub safe_sound: Option<bool>,
    pub auto_off: Option<AutoOff>,
    pub find: Option<(bool, bool)>,
    /// A message for a toast, numbered so the window shows each one once.
    pub notice: Option<(u32, String)>,
}

impl State {
    pub fn apply(&mut self, fact: Fact) {
        match fact {
            Fact::Name(name) => self.name = Some(name),
            Fact::Firmware(v) => self.firmware = Some(v),
            Fact::BatteryLeft(l) => self.battery.left = l,
            Fact::BatteryRight(l) => self.battery.right = l,
            Fact::BatteryCase(l) => self.battery.case = l,
            Fact::AutoOff(a) => self.auto_off = Some(a),
            Fact::SafeSound(on) => self.safe_sound = Some(on),
            Fact::Mode { current, available } => {
                self.mode = Some(current);
                self.modes = available;
            }
            Fact::Eq(eq) => self.eq = Some(eq),
            Fact::VoicePrompts(on) => self.voice_prompts = Some(on),
            Fact::VoiceAware { level, extra } => {
                self.voice_aware = Some(level);
                if let Some(extra) = extra {
                    self.voice_aware_extra = extra;
                }
            }
            Fact::Finding { left, right } => self.find = Some((left, right)),
        }
    }

    /// Forget the device, keep the notice counter.
    pub fn reset(&mut self, link: Link, name: Option<String>) {
        let notice = self.notice.take();
        *self = State {
            link,
            name,
            notice,
            ..State::default()
        };
    }

    pub fn notify(&mut self, message: impl Into<String>) {
        let n = self.notice.as_ref().map_or(0, |(n, _)| n + 1);
        self.notice = Some((n, message.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{EqPreset, FrameReader, decode};

    #[test]
    fn the_capture_fills_in_the_wave_buds_2() {
        let mut state = State::default();
        let mut reader = FrameReader::default();
        for (tx, bytes) in crate::protocol::tests::capture() {
            if !tx {
                for frame in reader.feed(&bytes, true) {
                    decode(&frame).into_iter().for_each(|f| state.apply(f));
                }
            }
        }
        assert_eq!(state.name.as_deref(), Some("JBL Wave Buds 2"));
        assert_eq!(state.firmware.as_deref(), Some("2.7.0"));
        assert_eq!(state.mode, Some(Mode::Ambient));
        assert_eq!(state.modes.len(), 4);
        assert_eq!(state.eq.map(|e| e.preset), Some(EqPreset::Off));
        assert_eq!(state.voice_aware, Some(2));
        assert_eq!(state.voice_prompts, Some(true));
        assert_eq!(state.safe_sound, Some(false));
        assert_eq!(state.find, Some((false, false)));
        assert_eq!(state.battery.left.map(|l| l.percent), Some(100));
        assert_eq!(state.battery.right, None);
    }

    #[test]
    fn notices_are_numbered_across_resets() {
        let mut state = State::default();
        state.notify("one");
        state.reset(Link::NoDevice, None);
        state.notify("two");
        assert_eq!(state.notice, Some((1, "two".into())));
    }
}
