//! JBL "Protocol 3": the AA-framed control protocol JBL earbuds speak over
//! their BLE control service. Pure functions only: no sockets, no D-Bus.
//!
//! ```text
//! AA <cmd> <len> <payload...>          no sequence byte, no checksum
//! AA <cmd> <len lo> <len hi> <...>     for the long-length commands
//! ```
//!
//! Ported from jbl-control (Ahmad Elyan, MIT), whose frames were captured from
//! a Wave Buds 2; tests/fixtures/wave-buds-2.txt is that capture.

/// The control service and its two characteristics ("excelpoint.com" in ASCII).
pub const SERVICE_UUID: u128 = 0x6578_6365_6c70_6f69_6e74_2e63_6f6d_0000;
pub const NOTIFY_UUID: u128 = 0x6578_6365_6c70_6f69_6e74_2e63_6f6d_0001;
pub const WRITE_UUID: u128 = 0x6578_6365_6c70_6f69_6e74_2e63_6f6d_0002;

/// Harman's Bluetooth SIG company id, the "v0ECB" in a paired headset's modalias.
pub const JBL_VENDOR_ID: u16 = 0x0ECB;
/// 16-bit service data in the buds' "<name>-LE" advertisement.
pub const HARMAN_SERVICE: &str = "0000fddf-0000-1000-8000-00805f9b34fb";

const HEADER: u8 = 0xAA;

pub const CMD_INFO: u8 = 0x06;
pub const CMD_NAME: u8 = 0x12;
pub const CMD_FIND: u8 = 0x23;
pub const CMD_ANC: u8 = 0x91;
pub const CMD_VOICE_PROMPT: u8 = 0x93;
pub const CMD_VOICE_AWARE: u8 = 0x98;
pub const CMD_NOTIFY_ON: u8 = 0x9B;
pub const CMD_EQ: u8 = 0xA2;

/// Commands whose frames from the buds carry a 2-byte length.
fn long_len_in(cmd: u8) -> bool {
    matches!(cmd, CMD_INFO | CMD_EQ)
}

// Device-info feature ids in the 0x06 TLV frames.
pub const FID_NAME: u8 = 0x01;
pub const FID_FIRMWARE: u8 = 0x05;
pub const FID_AUTO_OFF: u8 = 0x0B;
pub const FID_BATT_LEFT: u8 = 0x1A;
pub const FID_BATT_RIGHT: u8 = 0x1B;
pub const FID_BATT_CASE: u8 = 0x1C;
pub const FID_SAFE_SOUND: u8 = 0x20;

/// What the first read asks for. The same list as jbl-control, so the frame
/// matches the capture byte for byte.
const INFO_PROBE: [u8; 10] = [0x01, 0x05, 0x06, 0x0B, 0x0C, 0x0D, 0x1A, 0x1B, 0x1C, 0x20];
/// The periodic battery read.
const INFO_BATTERY: [u8; 3] = [FID_BATT_LEFT, FID_BATT_RIGHT, FID_BATT_CASE];

pub const EQ_BANDS: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
pub const EQ_OFF: u8 = 0;
pub const EQ_CUSTOM: u8 = 230;
/// The firmware takes gains in this range; more is clipped.
pub const EQ_LIMIT: f32 = 6.0;

/// JBL's presets: category id, label, gains (dB) for [`EQ_BANDS`].
pub const EQ_PRESETS: [(u8, &str, [f32; 10]); 6] = [
    (
        1,
        "Jazz",
        [4.0, 3.0, 1.0, 2.5, -1.5, -1.5, 0.0, 1.0, 3.0, 4.0],
    ),
    (
        2,
        "Vocal",
        [-1.5, -3.0, -3.0, 1.0, 3.5, 3.5, 3.0, 1.5, 0.0, -1.5],
    ),
    (
        3,
        "Bass",
        [4.0, 4.0, 3.0, 2.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        6,
        "Club",
        [2.0, 1.5, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 2.0],
    ),
    (
        7,
        "Studio",
        [-2.0, -1.0, -1.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        9,
        "Extreme Bass",
        [4.0, 4.5, 4.5, 3.5, -1.0, 0.0, 0.0, 2.0, 2.0, 0.0],
    ),
];

pub fn preset_gains(category: u8) -> Option<[f32; 10]> {
    EQ_PRESETS.iter().find(|p| p.0 == category).map(|p| p.2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Anc,
    Ambient,
    TalkThru,
}

impl Mode {
    /// The slot a mode has in the 0x91 frames; off has none.
    fn from_slot(slot: u8) -> Option<Mode> {
        match slot {
            1 => Some(Mode::Anc),
            2 => Some(Mode::Ambient),
            3 => Some(Mode::TalkThru),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- encoding

fn short(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let len = u8::try_from(payload.len()).expect("payload fits a 1-byte length");
    let mut out = vec![HEADER, cmd, len];
    out.extend_from_slice(payload);
    out
}

fn long(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(payload.len()).expect("payload fits a 2-byte length");
    let mut out = vec![HEADER, cmd];
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Asks the buds to push changes (mode switched by touch, battery) unasked.
pub fn notify_on() -> Vec<u8> {
    short(CMD_NOTIFY_ON, &[0x01, 0x01])
}

pub fn mode_get() -> Vec<u8> {
    short(CMD_ANC, &[0x11])
}

pub fn mode_set(mode: Mode) -> Vec<u8> {
    let on = |m: Mode| u8::from(mode == m);
    short(
        CMD_ANC,
        &[
            0x10,
            1,
            on(Mode::Anc),
            2,
            on(Mode::Ambient),
            3,
            on(Mode::TalkThru),
        ],
    )
}

fn info_get(fids: &[u8]) -> Vec<u8> {
    let mut body = vec![0x01];
    for &fid in fids {
        body.extend_from_slice(&[0x00, fid]);
    }
    long(CMD_INFO, &body)
}

pub fn info_probe() -> Vec<u8> {
    info_get(&INFO_PROBE)
}

pub fn battery_get() -> Vec<u8> {
    info_get(&INFO_BATTERY)
}

fn info_set(fid: u8, value: &[u8]) -> Vec<u8> {
    let mut body = vec![0x00, 0x00, fid, value.len() as u8];
    body.extend_from_slice(value);
    long(CMD_INFO, &body)
}

pub fn auto_off_set(enabled: bool, minutes: u16) -> Vec<u8> {
    let mut value = vec![u8::from(enabled)];
    value.extend_from_slice(&minutes.to_le_bytes());
    info_set(FID_AUTO_OFF, &value)
}

pub fn safe_sound_set(enabled: bool) -> Vec<u8> {
    info_set(FID_SAFE_SOUND, &[u8::from(enabled)])
}

/// The short GET has a 1-byte length; the SET below has a 2-byte one.
pub fn eq_get() -> Vec<u8> {
    short(CMD_EQ, &[0x01, 0xFF])
}

pub fn eq_set(category: u8, gains: &[f32; 10]) -> Vec<u8> {
    let mut body = vec![0x03, category];
    body.extend_from_slice(&(-1.0f32).to_le_bytes()); // calibration
    body.push(48); // sample rate, kHz
    body.extend_from_slice(&0.0f32.to_le_bytes()); // left gain
    body.extend_from_slice(&0.0f32.to_le_bytes()); // right gain
    body.push(gains.len() as u8);
    for (gain, freq) in gains.iter().zip(EQ_BANDS) {
        body.push(0x01); // peak filter
        body.extend_from_slice(&gain.clamp(-EQ_LIMIT, EQ_LIMIT).to_le_bytes());
        body.extend_from_slice(&freq.to_le_bytes());
        body.push(10); // Q = 1.0
    }
    long(CMD_EQ, &body)
}

pub fn voice_prompts_get() -> Vec<u8> {
    short(CMD_VOICE_PROMPT, &[0x04])
}

pub fn voice_prompts_set(enabled: bool) -> Vec<u8> {
    short(CMD_VOICE_PROMPT, &[0x03, u8::from(enabled)])
}

pub fn voice_aware_get() -> Vec<u8> {
    short(CMD_VOICE_AWARE, &[0x01])
}

/// VoiceAware takes `<level> <extra>`. Left out, a Wave Buds 2 fills the
/// second byte with 0xf8, so the one it reported is always sent back.
pub fn voice_aware_set(level: u8, extra: u8) -> Vec<u8> {
    short(CMD_VOICE_AWARE, &[0x00, level, extra])
}

pub fn find_get() -> Vec<u8> {
    short(CMD_FIND, &[0x01])
}

/// `None` leaves that side as it is.
pub fn find_set(left: Option<bool>, right: Option<bool>) -> Vec<u8> {
    let side = |v: Option<bool>| v.map_or(0xFF, u8::from);
    short(CMD_FIND, &[0x00, side(left), side(right)])
}

// ---------------------------------------------------------------- framing

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub cmd: u8,
    pub payload: Vec<u8>,
    /// Cut short by a small MTU; the fields that arrived are still usable.
    pub truncated: bool,
}

/// Reassembles AA frames from GATT notifications.
///
/// A frame normally arrives in one notification once the MTU is raised. A
/// notification that starts a new frame while an older one is incomplete
/// means the older one was truncated: it is returned as it is.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    fn need(buf: &[u8]) -> Option<usize> {
        if buf.len() < 3 {
            return None;
        }
        if long_len_in(buf[1]) {
            if buf.len() < 4 {
                return None;
            }
            return Some(4 + usize::from(u16::from_le_bytes([buf[2], buf[3]])));
        }
        Some(3 + usize::from(buf[2]))
    }

    fn make(raw: &[u8], truncated: bool) -> Frame {
        let cmd = raw.get(1).copied().unwrap_or(0);
        let offset = if long_len_in(cmd) { 4 } else { 3 };
        Frame {
            cmd,
            payload: raw.get(offset..).unwrap_or_default().to_vec(),
            truncated,
        }
    }

    /// `last`: the notification was shorter than the MTU allows, so a frame
    /// still incomplete after it will not be continued.
    pub fn feed(&mut self, data: &[u8], last: bool) -> Vec<Frame> {
        let mut frames = Vec::new();
        if !self.buf.is_empty()
            && data.first() == Some(&HEADER)
            && Self::need(&self.buf).is_some_and(|need| self.buf.len() < need)
        {
            frames.push(Self::make(&self.buf, true));
            self.buf.clear();
        }
        self.buf.extend_from_slice(data);
        loop {
            let Some(start) = self.buf.iter().position(|&b| b == HEADER) else {
                self.buf.clear();
                break;
            };
            self.buf.drain(..start);
            match Self::need(&self.buf) {
                Some(need) if self.buf.len() >= need => {
                    frames.push(Self::make(&self.buf[..need], false));
                    self.buf.drain(..need);
                }
                _ => break,
            }
        }
        if last {
            frames.extend(self.flush());
        }
        frames
    }

    pub fn flush(&mut self) -> Option<Frame> {
        if self.buf.is_empty() {
            return None;
        }
        let frame = Self::make(&self.buf, true);
        self.buf.clear();
        Some(frame)
    }
}

// ---------------------------------------------------------------- decoding

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub percent: u8,
    pub charging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoOff {
    pub enabled: bool,
    pub minutes: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EqPreset {
    Off,
    Preset(u8),
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eq {
    pub preset: EqPreset,
    pub gains: [f32; 10],
}

/// One thing a frame from the buds says.
#[derive(Clone, Debug, PartialEq)]
pub enum Fact {
    Name(String),
    Firmware(String),
    /// `None`: no reading, as for a bud in the case.
    BatteryLeft(Option<Level>),
    BatteryRight(Option<Level>),
    BatteryCase(Option<Level>),
    AutoOff(AutoOff),
    SafeSound(bool),
    /// The current mode, and every mode the buds have (off included).
    Mode {
        current: Mode,
        available: Vec<Mode>,
    },
    Eq(Eq),
    VoicePrompts(bool),
    VoiceAware {
        level: u8,
        extra: Option<u8>,
    },
    Finding {
        left: bool,
        right: bool,
    },
}

fn level(value: &[u8]) -> Option<Level> {
    match value {
        [charging, percent, ..] if *percent <= 100 => Some(Level {
            percent: *percent,
            charging: *charging != 0,
        }),
        _ => None,
    }
}

/// 0x06 body: `<marker>` then entries of `<fid hi> <fid lo> <len> <value>`.
/// A truncated last entry keeps whatever bytes of its value arrived.
fn tlv(payload: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut i = 1;
    while i + 3 <= payload.len() {
        let fid = u16::from_be_bytes([payload[i], payload[i + 1]]);
        let len = usize::from(payload[i + 2]);
        let end = (i + 3 + len).min(payload.len());
        out.push((fid, &payload[i + 3..end]));
        i += 3 + len;
    }
    out
}

fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

fn decode_info(payload: &[u8]) -> Vec<Fact> {
    let mut facts = Vec::new();
    for (fid, v) in tlv(payload) {
        let Ok(fid) = u8::try_from(fid) else { continue };
        match fid {
            FID_NAME if !text(v).is_empty() => facts.push(Fact::Name(text(v))),
            FID_FIRMWARE if v.len() >= 3 => {
                facts.push(Fact::Firmware(format!("{}.{}.{}", v[0], v[1], v[2])));
            }
            FID_BATT_LEFT if v.len() >= 2 => facts.push(Fact::BatteryLeft(level(v))),
            FID_BATT_RIGHT if v.len() >= 2 => facts.push(Fact::BatteryRight(level(v))),
            FID_BATT_CASE if v.len() >= 2 => facts.push(Fact::BatteryCase(level(v))),
            FID_AUTO_OFF if v.len() >= 3 => facts.push(Fact::AutoOff(AutoOff {
                enabled: v[0] != 0,
                minutes: u16::from_le_bytes([v[1], v[2]]),
            })),
            FID_SAFE_SOUND if !v.is_empty() && v[0] <= 1 => facts.push(Fact::SafeSound(v[0] == 1)),
            _ => {}
        }
    }
    facts
}

fn decode_mode(payload: &[u8]) -> Vec<Fact> {
    // 0x12 answers a GET, 0x10 is the buds' own report (a touch on a bud).
    let Some((&sub, pairs)) = payload.split_first() else {
        return vec![];
    };
    if !matches!(sub, 0x10 | 0x12) || pairs.len() < 2 {
        return vec![];
    }
    let mut current = Mode::Off;
    let mut available = vec![Mode::Off];
    for pair in pairs.as_chunks::<2>().0 {
        if let Some(mode) = Mode::from_slot(pair[0]) {
            available.push(mode);
            if pair[1] != 0 && current == Mode::Off {
                current = mode;
            }
        }
    }
    vec![Fact::Mode { current, available }]
}

fn decode_eq(payload: &[u8]) -> Vec<Fact> {
    if payload.len() < 16 {
        return vec![];
    }
    let category = payload[1];
    let count = usize::from(payload[15]);
    let mut gains = [0.0f32; 10];
    let mut i = 16;
    for gain in gains.iter_mut().take(count) {
        // A Wave Buds 2 cuts this reply 4 bytes short; the gains still fit.
        if i + 5 > payload.len() {
            break;
        }
        let g = f32::from_le_bytes([
            payload[i + 1],
            payload[i + 2],
            payload[i + 3],
            payload[i + 4],
        ]);
        *gain = (g * 10.0).round() / 10.0;
        i += 10;
    }
    let preset = match category {
        // A Wave Buds 2 with EQ off answers category 0 and no bands.
        EQ_OFF => EqPreset::Off,
        c if preset_gains(c).is_some() => EqPreset::Preset(c),
        _ => EqPreset::Custom,
    };
    vec![Fact::Eq(Eq { preset, gains })]
}

pub fn decode(frame: &Frame) -> Vec<Fact> {
    let p = frame.payload.as_slice();
    match frame.cmd {
        CMD_INFO => decode_info(p),
        CMD_ANC => decode_mode(p),
        CMD_EQ => decode_eq(p),
        CMD_NAME if !text(p).is_empty() => vec![Fact::Name(text(p))],
        CMD_VOICE_PROMPT if p.len() >= 2 && p[0] == 0x05 => vec![Fact::VoicePrompts(p[1] != 0)],
        // <dir> <level> <extra>, 02 02 00 on a Wave Buds 2.
        CMD_VOICE_AWARE if p.len() >= 2 && matches!(p[0], 0x00 | 0x02) => {
            vec![Fact::VoiceAware {
                level: p[1],
                extra: p.get(2).copied(),
            }]
        }
        CMD_FIND if p.len() >= 3 => vec![Fact::Finding {
            left: p[1] == 1,
            right: p[2] == 1,
        }],
        _ => vec![],
    }
}

// ---------------------------------------------------------------- replies

/// Whether a frame answers a given GET: the reads double as the probe for
/// which features a model has, so an answer has to be recognised.
pub fn answers(get: &[u8], frame: &Frame) -> bool {
    let p = &frame.payload;
    match (get.get(1).copied(), frame.cmd) {
        (Some(CMD_ANC), CMD_ANC) => p.first() == Some(&0x12),
        (Some(CMD_INFO), CMD_INFO) => p.first() == Some(&0x02),
        (Some(CMD_EQ), CMD_EQ) => p.len() >= 16,
        (Some(CMD_VOICE_PROMPT), CMD_VOICE_PROMPT) => p.first() == Some(&0x05),
        (Some(CMD_VOICE_AWARE), CMD_VOICE_AWARE) => p.len() >= 2,
        (Some(CMD_FIND), CMD_FIND) => p.len() >= 3,
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The capture as (is_tx, bytes) lines.
    pub(crate) fn capture() -> Vec<(bool, Vec<u8>)> {
        include_str!("../tests/fixtures/wave-buds-2.txt")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| {
                let (dir, hex) = l.split_once(' ').unwrap();
                let bytes = hex
                    .split_whitespace()
                    .map(|b| u8::from_str_radix(b, 16).unwrap())
                    .collect();
                (dir == "tx", bytes)
            })
            .collect()
    }

    fn sent() -> Vec<Vec<u8>> {
        capture()
            .into_iter()
            .filter(|(tx, _)| *tx)
            .map(|(_, b)| b)
            .collect()
    }

    fn frame(bytes: &[u8]) -> Frame {
        let mut frames = FrameReader::default().feed(bytes, true);
        assert_eq!(frames.len(), 1, "one frame in {bytes:02x?}");
        frames.remove(0)
    }

    fn received(cmd: u8) -> Vec<Frame> {
        capture()
            .into_iter()
            .filter(|(tx, b)| !tx && b[1] == cmd)
            .map(|(_, b)| frame(&b))
            .collect()
    }

    #[test]
    fn requests_match_the_jbl_app_byte_for_byte() {
        let sent = sent();
        for ours in [
            notify_on(),
            mode_get(),
            info_probe(),
            eq_get(),
            voice_prompts_get(),
            voice_aware_get(),
            find_get(),
        ] {
            assert!(sent.contains(&ours), "{ours:02x?} is not in the capture");
        }
    }

    #[test]
    fn mode_set_flags_one_slot() {
        assert_eq!(
            mode_set(Mode::Ambient),
            [0xAA, 0x91, 7, 0x10, 1, 0, 2, 1, 3, 0]
        );
        assert_eq!(mode_set(Mode::Off), [0xAA, 0x91, 7, 0x10, 1, 0, 2, 0, 3, 0]);
    }

    #[test]
    fn eq_set_is_long_framed_and_clamped() {
        let f = eq_set(EQ_CUSTOM, &[9.0; 10]);
        assert_eq!(&f[..2], &[0xAA, 0xA2]);
        let len = usize::from(u16::from_le_bytes([f[2], f[3]]));
        assert_eq!(f.len(), 4 + len);
        assert_eq!(len, 2 + 4 + 1 + 8 + 1 + 10 * 10);
        // first band: 01, gain, freq, Q
        let gain = f32::from_le_bytes(f[4 + 16 + 1..4 + 16 + 5].try_into().unwrap());
        assert_eq!(gain, EQ_LIMIT);
    }

    #[test]
    fn the_buds_report_their_name_and_info() {
        let facts: Vec<Fact> = received(CMD_INFO).iter().flat_map(decode).collect();
        assert!(facts.contains(&Fact::Name("JBL Wave Buds 2".into())));
        assert!(facts.contains(&Fact::Firmware("2.7.0".into())));
        assert!(facts.contains(&Fact::BatteryLeft(Some(Level {
            percent: 100,
            charging: false
        }))));
        // The right bud was in the case.
        assert!(facts.contains(&Fact::BatteryRight(None)));
        assert!(facts.contains(&Fact::BatteryCase(Some(Level {
            percent: 100,
            charging: false
        }))));
        assert!(facts.contains(&Fact::AutoOff(AutoOff {
            enabled: false,
            minutes: 0
        })));
        assert!(facts.contains(&Fact::SafeSound(false)));
    }

    #[test]
    fn the_name_frame_ends_at_its_nul() {
        assert_eq!(
            decode(&received(CMD_NAME)[0]),
            [Fact::Name("JBL Wave Buds 2".into())]
        );
    }

    #[test]
    fn mode_reply_lists_the_modes() {
        let reply = &received(CMD_ANC)[0];
        let get = mode_get();
        assert!(answers(&get, reply));
        assert_eq!(
            decode(reply),
            [Fact::Mode {
                current: Mode::Ambient,
                available: vec![Mode::Off, Mode::Anc, Mode::Ambient, Mode::TalkThru],
            }]
        );
    }

    #[test]
    fn eq_off_has_a_flat_curve() {
        let reply = &received(CMD_EQ)[0];
        assert!(answers(&eq_get(), reply));
        assert_eq!(
            decode(reply),
            [Fact::Eq(Eq {
                preset: EqPreset::Off,
                gains: [0.0; 10]
            })]
        );
    }

    #[test]
    fn an_eq_set_reads_back() {
        let gains = preset_gains(3).unwrap();
        let mut f = eq_set(3, &gains);
        f[4] = 0x02; // the buds answer with the "return" direction
        assert_eq!(
            decode(&frame(&f)),
            [Fact::Eq(Eq {
                preset: EqPreset::Preset(3),
                gains
            })]
        );
    }

    #[test]
    fn voice_settings_and_find() {
        assert_eq!(
            decode(&received(CMD_VOICE_PROMPT)[0]),
            [Fact::VoicePrompts(true)]
        );
        assert_eq!(
            decode(&received(CMD_VOICE_AWARE)[0]),
            [Fact::VoiceAware {
                level: 2,
                extra: Some(0)
            }]
        );
        assert_eq!(
            decode(&received(CMD_FIND)[0]),
            [Fact::Finding {
                left: false,
                right: false
            }]
        );
    }

    #[test]
    fn every_probe_in_the_capture_was_answered() {
        let lines = capture();
        for get in [
            mode_get(),
            info_probe(),
            eq_get(),
            voice_prompts_get(),
            voice_aware_get(),
            find_get(),
        ] {
            let at = lines.iter().position(|(tx, b)| *tx && *b == get).unwrap();
            let reply = lines[at + 1..]
                .iter()
                .take_while(|(tx, _)| !tx)
                .any(|(_, b)| answers(&get, &frame(b)));
            assert!(reply, "no answer to {get:02x?}");
        }
    }

    #[test]
    fn split_frames_are_reassembled() {
        let whole = &received(CMD_INFO)[1];
        let raw = capture()
            .into_iter()
            .filter(|(tx, b)| !tx && b[1] == CMD_INFO)
            .nth(1)
            .unwrap()
            .1;
        let mut reader = FrameReader::default();
        assert!(reader.feed(&raw[..20], false).is_empty());
        assert_eq!(reader.feed(&raw[20..], false), std::slice::from_ref(whole));
    }

    #[test]
    fn a_new_frame_surfaces_a_truncated_one() {
        let mut reader = FrameReader::default();
        assert!(reader.feed(&[0xAA, 0x98, 0x03, 0x02], false).is_empty());
        let frames = reader.feed(&[0xAA, 0x93, 0x02, 0x05, 0x01], false);
        assert_eq!(frames.len(), 2);
        assert!(frames[0].truncated);
        assert_eq!(frames[1].payload, [0x05, 0x01]);
    }

    #[test]
    fn find_leaves_the_unnamed_side_alone() {
        assert_eq!(find_set(Some(true), None), [0xAA, 0x23, 3, 0, 1, 0xFF]);
    }

    #[test]
    fn auto_off_minutes_are_little_endian() {
        assert_eq!(
            auto_off_set(true, 30),
            [0xAA, 0x06, 7, 0, 0, 0, 0x0B, 3, 1, 30, 0]
        );
    }
}
