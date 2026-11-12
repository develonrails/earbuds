//! A minimal ATT (GATT client) over an LE L2CAP socket.
//!
//! The fixed ATT channel (CID 4) is opened straight to the buds' random LE
//! address, the way btgatt-client does, without a BlueZ device object: the
//! buds reject BlueZ's own GATT client (ATT error 0x0e), and bluetoothd would
//! take the bearer if it had an object for the address (see bluez::release).
//!
//! Only what the control link needs: MTU exchange, finding one service, its
//! characteristics and a CCCD, Write Request, and notifications.

use std::{fmt, io, sync::Arc, time::Duration};

use bluer::{
    Address, AddressType,
    l2cap::{SeqPacket, Socket, SocketAddr},
};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

const ATT_CID: u16 = 4;
const GATT_PRIMARY: u16 = 0x2800;
const GATT_CHARACTERISTIC: u16 = 0x2803;
const GATT_CCCD: u16 = 0x2902;

const OP_ERROR: u8 = 0x01;
const OP_MTU_REQ: u8 = 0x02;
const OP_MTU_RSP: u8 = 0x03;
const OP_FIND_INFO_REQ: u8 = 0x04;
const OP_FIND_INFO_RSP: u8 = 0x05;
const OP_FIND_BY_TYPE_REQ: u8 = 0x06;
const OP_FIND_BY_TYPE_RSP: u8 = 0x07;
const OP_READ_BY_TYPE_REQ: u8 = 0x08;
const OP_READ_BY_TYPE_RSP: u8 = 0x09;
const OP_WRITE_REQ: u8 = 0x12;
const OP_WRITE_RSP: u8 = 0x13;
const OP_NOTIFY: u8 = 0x1B;
const OP_INDICATE: u8 = 0x1D;
const OP_CONFIRM: u8 = 0x1E;
const COMMAND_FLAG: u8 = 0x40;

const ERR_REQ_NOT_SUPPORTED: u8 = 0x06;
const ERR_ATTR_NOT_FOUND: u8 = 0x0A;

const WANTED_MTU: u16 = 517;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// An ATT error response, with its code.
    Att(u8),
    Timeout,
    Closed,
    Missing(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Att(code) => write!(f, "ATT error 0x{code:02x}"),
            Error::Timeout => write!(f, "no answer"),
            Error::Closed => write!(f, "link closed"),
            Error::Missing(what) => write!(f, "{what} not found"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Notification values from the buds; the channel closes with the link.
pub type Notifications = mpsc::UnboundedReceiver<(u16, Vec<u8>)>;

pub struct Att {
    sock: Arc<SeqPacket>,
    responses: mpsc::UnboundedReceiver<Vec<u8>>,
    reader: JoinHandle<()>,
    timeout: Duration,
    pub mtu: u16,
}

impl Drop for Att {
    fn drop(&mut self) {
        self.reader.abort();
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

fn uuid_le(u: u128) -> [u8; 16] {
    u.to_le_bytes()
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

impl Att {
    pub async fn connect(address: Address, wait: Duration) -> Result<(Att, Notifications)> {
        let socket = Socket::<SeqPacket>::new_seq_packet()?;
        socket.bind(SocketAddr {
            addr: Address::any(),
            addr_type: AddressType::LePublic,
            psm: 0,
            cid: ATT_CID,
        })?;
        let target = SocketAddr {
            addr: address,
            addr_type: AddressType::LeRandom,
            psm: 0,
            cid: ATT_CID,
        };
        let sock = timeout(wait, socket.connect(target))
            .await
            .map_err(|_| Error::Timeout)??;
        let sock = Arc::new(sock);
        let (rsp_tx, responses) = mpsc::unbounded_channel();
        let (note_tx, notes) = mpsc::unbounded_channel();
        let reader = tokio::spawn(read_loop(sock.clone(), rsp_tx, note_tx));
        Ok((
            Att {
                sock,
                responses,
                reader,
                timeout: Duration::from_secs(5),
                mtu: 23,
            },
            notes,
        ))
    }

    async fn request(&mut self, pdu: &[u8], expect: u8) -> Result<Vec<u8>> {
        while self.responses.try_recv().is_ok() {}
        self.sock.send(pdu).await?;
        let rsp = timeout(self.timeout, self.responses.recv())
            .await
            .map_err(|_| Error::Timeout)?
            .ok_or(Error::Closed)?;
        match rsp.first() {
            Some(&OP_ERROR) => Err(Error::Att(rsp.get(4).copied().unwrap_or(0))),
            Some(&op) if op == expect => Ok(rsp),
            _ => Err(Error::Missing("the expected response")),
        }
    }

    pub async fn exchange_mtu(&mut self) -> Result<u16> {
        let mut pdu = vec![OP_MTU_REQ];
        pdu.extend_from_slice(&WANTED_MTU.to_le_bytes());
        let rsp = self.request(&pdu, OP_MTU_RSP).await?;
        if rsp.len() >= 3 {
            self.mtu = u16_at(&rsp, 1).clamp(23, WANTED_MTU);
        }
        Ok(self.mtu)
    }

    /// The (start, end) handles of a primary service.
    async fn find_service(&mut self, uuid: u128) -> Result<(u16, u16)> {
        let mut pdu = vec![OP_FIND_BY_TYPE_REQ];
        for v in [0x0001, 0xFFFF, GATT_PRIMARY] {
            pdu.extend_from_slice(&u16::to_le_bytes(v));
        }
        pdu.extend_from_slice(&uuid_le(uuid));
        match self.request(&pdu, OP_FIND_BY_TYPE_RSP).await {
            Ok(rsp) if rsp.len() >= 5 => Ok((u16_at(&rsp, 1), u16_at(&rsp, 3))),
            Ok(_) | Err(Error::Att(ERR_ATTR_NOT_FOUND)) => Err(Error::Missing("control service")),
            Err(e) => Err(e),
        }
    }

    /// (declaration handle, value handle, uuid) of each characteristic in a range.
    async fn characteristics(&mut self, start: u16, end: u16) -> Result<Vec<(u16, u16, u128)>> {
        let mut out = Vec::new();
        let mut cur = start;
        while cur <= end {
            let mut pdu = vec![OP_READ_BY_TYPE_REQ];
            for v in [cur, end, GATT_CHARACTERISTIC] {
                pdu.extend_from_slice(&v.to_le_bytes());
            }
            let rsp = match self.request(&pdu, OP_READ_BY_TYPE_RSP).await {
                Ok(rsp) => rsp,
                Err(Error::Att(ERR_ATTR_NOT_FOUND)) => break,
                Err(e) => return Err(e),
            };
            let item = usize::from(*rsp.get(1).unwrap_or(&0));
            if item < 7 {
                break;
            }
            let mut last = None;
            for entry in rsp[2..].chunks_exact(item) {
                let decl = u16_at(entry, 0);
                let value = u16_at(entry, 3);
                let uuid = match &entry[5..] {
                    [a, b] => {
                        let short = u128::from(u16::from_le_bytes([*a, *b]));
                        (short << 96) | 0x0000_0000_0000_1000_8000_0080_5f9b_34fb
                    }
                    long if long.len() == 16 => u128::from_le_bytes(long.try_into().unwrap()),
                    _ => 0,
                };
                out.push((decl, value, uuid));
                last = Some(decl);
            }
            match last {
                Some(decl) if decl < end => cur = decl + 1,
                _ => break,
            }
        }
        Ok(out)
    }

    async fn find_cccd(&mut self, start: u16, end: u16) -> Result<u16> {
        let mut cur = start;
        while cur <= end {
            let mut pdu = vec![OP_FIND_INFO_REQ];
            pdu.extend_from_slice(&cur.to_le_bytes());
            pdu.extend_from_slice(&end.to_le_bytes());
            let rsp = match self.request(&pdu, OP_FIND_INFO_RSP).await {
                Ok(rsp) => rsp,
                Err(Error::Att(ERR_ATTR_NOT_FOUND)) => break,
                Err(e) => return Err(e),
            };
            let step = if rsp.get(1) == Some(&1) { 4 } else { 18 };
            let mut last = None;
            for entry in rsp[2..].chunks_exact(step) {
                let handle = u16_at(entry, 0);
                if step == 4 && u16_at(entry, 2) == GATT_CCCD {
                    return Ok(handle);
                }
                last = Some(handle);
            }
            match last {
                Some(h) if h < end => cur = h + 1,
                _ => break,
            }
        }
        Err(Error::Missing("notify descriptor"))
    }

    pub async fn write(&mut self, handle: u16, value: &[u8]) -> Result<()> {
        let mut pdu = vec![OP_WRITE_REQ];
        pdu.extend_from_slice(&handle.to_le_bytes());
        pdu.extend_from_slice(value);
        self.request(&pdu, OP_WRITE_RSP).await.map(drop)
    }

    /// Finds a write/notify characteristic pair in a service and turns its
    /// notifications on. Returns (write handle, notify handle).
    pub async fn open_control(
        &mut self,
        service: u128,
        write: u128,
        notify: u128,
    ) -> Result<(u16, u16)> {
        let (start, end) = self.find_service(service).await?;
        let chars = self.characteristics(start, end).await?;
        let find = |u| chars.iter().find(|c| c.2 == u).copied();
        let (Some(w), Some(n)) = (find(write), find(notify)) else {
            return Err(Error::Missing("control characteristics"));
        };
        let next = chars.iter().map(|c| c.0).filter(|&d| d > n.0).min();
        let cccd = self.find_cccd(n.1 + 1, next.map_or(end, |d| d - 1)).await?;
        self.write(cccd, &[0x01, 0x00]).await?;
        Ok((w.1, n.1))
    }
}

async fn read_loop(
    sock: Arc<SeqPacket>,
    responses: mpsc::UnboundedSender<Vec<u8>>,
    notes: mpsc::UnboundedSender<(u16, Vec<u8>)>,
) {
    let mut buf = vec![0u8; 1024];
    loop {
        let n = match sock.recv(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        let pdu = &buf[..n];
        let op = pdu[0];
        match op {
            OP_NOTIFY | OP_INDICATE if pdu.len() >= 3 => {
                if op == OP_INDICATE {
                    let _ = sock.send(&[OP_CONFIRM]).await;
                }
                if notes.send((u16_at(pdu, 1), pdu[3..].to_vec())).is_err() {
                    return;
                }
            }
            // The buds may negotiate too; answer so they do not stall.
            OP_MTU_REQ => {
                let mut rsp = vec![OP_MTU_RSP];
                rsp.extend_from_slice(&WANTED_MTU.to_le_bytes());
                let _ = sock.send(&rsp).await;
            }
            // A command to our GATT server, which we do not have.
            _ if op & COMMAND_FLAG != 0 => {}
            // A request to it: say so, or the buds wait for an answer.
            _ if op.is_multiple_of(2) && op != OP_CONFIRM => {
                let _ = sock
                    .send(&[OP_ERROR, op, 0, 0, ERR_REQ_NOT_SUPPORTED])
                    .await;
            }
            _ => {
                let _ = responses.send(pdu.to_vec());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_go_on_the_wire_little_endian() {
        let wire = uuid_le(crate::protocol::SERVICE_UUID);
        assert_eq!(wire[0], 0x00);
        assert_eq!(wire[15], 0x65); // 'e' of "excelpoint.com"
    }
}
