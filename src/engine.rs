//! The Bluetooth side, on a thread of its own: one current-thread tokio
//! runtime, asleep unless BlueZ or the window has something to say.
//!
//! It waits for a JBL headset to connect, finds its control advertiser,
//! opens the ATT link, reads every setting (the reads double as the probe
//! for what the model has), and then serves the window's commands. Writes
//! are paced: frames sent back to back get dropped by the firmware.

use std::{mem::discriminant, time::Duration};

use futures_util::StreamExt;
use tokio::{
    sync::mpsc,
    time::{Instant, sleep, sleep_until, timeout},
};
use zbus::MessageStream;

use crate::{
    att::{self, Att, Notifications},
    bluez::{self, Bluez},
    protocol::{self as p, AutoOff, Eq, EqPreset, Frame, FrameReader, Mode},
    state::{Link, State},
};

const PACE: Duration = Duration::from_millis(450);
const REPLY: Duration = Duration::from_millis(1600);
const SCAN: Duration = Duration::from_secs(12);
/// The buds push mode changes, but not every battery change.
const BATTERY_EVERY: Duration = Duration::from_secs(120);

/// `EARBUDS_DEBUG=1`: every frame to and from the buds on stderr.
fn debug() -> bool {
    static DEBUG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEBUG.get_or_init(|| std::env::var_os("EARBUDS_DEBUG").is_some())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug)]
pub enum EqChoice {
    Off,
    Preset(u8),
    Custom([f32; 10]),
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Mode(Mode),
    Eq(EqChoice),
    VoiceAware(u8),
    VoicePrompts(bool),
    SafeSound(bool),
    AutoOff(bool),
    /// `None` leaves that side as it is.
    Find(Option<bool>, Option<bool>),
}

#[derive(Clone)]
pub struct Engine {
    commands: mpsc::UnboundedSender<Command>,
}

impl Engine {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// Starts the Bluetooth thread. States come back on the receiver, one per
/// change; a reader that falls behind only needs the last.
pub fn start() -> (Engine, async_channel::Receiver<State>) {
    let (commands, rx) = mpsc::unbounded_channel();
    let (out, states) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("earbuds-bt".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(run(Ctx {
                state: State::default(),
                out,
                commands: rx,
            }));
        })
        .expect("bluetooth thread");
    (Engine { commands }, states)
}

struct Ctx {
    state: State,
    out: async_channel::Sender<State>,
    commands: mpsc::UnboundedReceiver<Command>,
}

impl Ctx {
    fn publish(&self) {
        let _ = self.out.try_send(self.state.clone());
    }

    fn set_link(&mut self, link: Link) {
        if self.state.link != link {
            self.state.link = link;
            self.publish();
        }
    }
}

async fn run(mut ctx: Ctx) {
    loop {
        match Bluez::connect().await {
            Ok(bluez) => {
                if let Err(e) = serve(&mut ctx, &bluez).await {
                    eprintln!("earbuds: bluez: {e}");
                }
            }
            Err(e) => eprintln!("earbuds: no bluez: {e}"),
        }
        if ctx.out.is_closed() {
            return;
        }
        ctx.state.reset(Link::NoBluetooth, None);
        ctx.publish();
        sleep(Duration::from_secs(10)).await;
    }
}

/// Waits until BlueZ reports a connection change, or `limit` passes.
/// Commands that arrive meanwhile have nothing to go to and are dropped.
async fn wait_for_change(ctx: &mut Ctx, changes: &mut MessageStream, limit: Option<Duration>) {
    let deadline = limit.map(|l| Instant::now() + l);
    loop {
        tokio::select! {
            msg = changes.next() => match msg {
                Some(Ok(msg)) if bluez::is_connection_change(&msg) => return,
                Some(_) => {}
                None => return,
            },
            cmd = ctx.commands.recv() => if cmd.is_none() { std::future::pending::<()>().await },
            _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => return,
        }
    }
}

async fn serve(ctx: &mut Ctx, bluez: &Bluez) -> zbus::Result<()> {
    let mut changes = bluez.changes().await?;
    let mut failures = 0u32;
    loop {
        let Some(headset) = bluez.headset().await? else {
            failures = 0;
            ctx.state.reset(Link::NoDevice, None);
            ctx.publish();
            wait_for_change(ctx, &mut changes, None).await;
            continue;
        };
        if ctx.state.link == Link::NoDevice || ctx.state.link == Link::NoBluetooth {
            ctx.state.reset(Link::Searching, Some(headset.name.clone()));
            ctx.publish();
        }
        ctx.set_link(Link::Searching);
        if debug() {
            eprintln!("headset {} ({}), scanning", headset.name, headset.address);
        }
        let Some(le) = bluez.find_advertiser(&headset, SCAN).await? else {
            if debug() {
                eprintln!("no control advertiser seen");
            }
            failures += 1;
            wait_for_change(ctx, &mut changes, Some(backoff(failures))).await;
            continue;
        };
        if bluez.release(&le).await {
            sleep(Duration::from_secs(1)).await; // let the old connection drop
        }
        ctx.set_link(Link::Connecting);
        if debug() {
            eprintln!("control advertiser {le}, connecting");
        }
        match session(ctx, bluez, &le, &mut changes).await {
            Ok(()) => failures = 0,
            Err(e) => {
                eprintln!("earbuds: control link {le}: {e}");
                failures += 1;
            }
        }
        ctx.set_link(Link::Searching);
        wait_for_change(ctx, &mut changes, Some(backoff(failures))).await;
    }
}

fn backoff(failures: u32) -> Duration {
    Duration::from_secs(u64::from((3 * failures).clamp(1, 60)))
}

struct Session {
    att: Att,
    notes: Notifications,
    write: u16,
    notify: u16,
    reader: FrameReader,
    last_write: Instant,
}

async fn session(
    ctx: &mut Ctx,
    bluez: &Bluez,
    le: &str,
    changes: &mut MessageStream,
) -> att::Result<()> {
    let address = le
        .parse()
        .map_err(|_| att::Error::Missing("a valid address"))?;
    let (mut att, notes) = Att::connect(address, Duration::from_secs(5)).await?;
    att.exchange_mtu().await?;
    let (write, notify) = att
        .open_control(p::SERVICE_UUID, p::WRITE_UUID, p::NOTIFY_UUID)
        .await?;
    let mut s = Session {
        att,
        notes,
        write,
        notify,
        reader: FrameReader::default(),
        last_write: Instant::now() - PACE,
    };

    s.send(&p::notify_on()).await?;
    sleep(Duration::from_millis(300)).await;
    for get in [
        p::mode_get(),
        p::info_probe(),
        p::eq_get(),
        p::voice_prompts_get(),
        p::voice_aware_get(),
        p::find_get(),
    ] {
        for _attempt in 0..2 {
            s.send(&get).await?;
            if s.reply(ctx, |f| p::answers(&get, f)).await?.is_some() {
                break;
            }
        }
    }
    ctx.set_link(Link::Ready);

    let mut battery = tokio::time::interval_at(Instant::now() + BATTERY_EVERY, BATTERY_EVERY);
    loop {
        tokio::select! {
            note = s.notes.recv() => match note {
                Some((handle, value)) => { s.take(ctx, handle, &value); }
                None => return Ok(()),
            },
            cmd = ctx.commands.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                for cmd in coalesce(cmd, &mut ctx.commands) {
                    s.execute(ctx, cmd).await?;
                }
            }
            _ = battery.tick() => s.send(&p::battery_get()).await?,
            msg = changes.next() => {
                if let Some(Ok(msg)) = msg
                    && bluez::is_connection_change(&msg)
                    && bluez.headset().await.ok().flatten().is_none()
                {
                    return Ok(());
                }
            }
        }
    }
}

/// The commands queued behind `first`, with only the newest of each slider
/// kept: dragging the equalizer should not replay every step it passed.
fn coalesce(first: Command, rx: &mut mpsc::UnboundedReceiver<Command>) -> Vec<Command> {
    let mut all = vec![first];
    while let Ok(cmd) = rx.try_recv() {
        all.push(cmd);
    }
    let slider = |c: &Command| matches!(c, Command::Eq(_) | Command::VoiceAware(_));
    let mut keep = Vec::with_capacity(all.len());
    for (i, cmd) in all.iter().enumerate() {
        let superseded = slider(cmd)
            && all[i + 1..]
                .iter()
                .any(|later| discriminant(later) == discriminant(cmd));
        if !superseded {
            keep.push(*cmd);
        }
    }
    keep
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "Off",
        Mode::Anc => "Noise Cancelling",
        Mode::Ambient => "Ambient Aware",
        Mode::TalkThru => "TalkThru",
    }
}

impl Session {
    async fn send(&mut self, frame: &[u8]) -> att::Result<()> {
        sleep_until(self.last_write + PACE).await;
        if debug() {
            eprintln!("tx {}", hex(frame));
        }
        self.att.write(self.write, frame).await?;
        self.last_write = Instant::now();
        Ok(())
    }

    /// Decodes a notification into the state. Returns the frames in it.
    fn take(&mut self, ctx: &mut Ctx, handle: u16, value: &[u8]) -> Vec<Frame> {
        if handle != self.notify {
            return vec![];
        }
        if debug() {
            eprintln!("rx {}", hex(value));
        }
        let last = value.len() + 3 < usize::from(self.att.mtu);
        let frames = self.reader.feed(value, last);
        self.apply(ctx, &frames);
        frames
    }

    fn apply(&self, ctx: &mut Ctx, frames: &[Frame]) {
        let before = ctx.state.clone();
        for frame in frames {
            p::decode(frame)
                .into_iter()
                .for_each(|fact| ctx.state.apply(fact));
        }
        if ctx.state != before {
            ctx.publish();
        }
    }

    /// Waits for a frame that satisfies `wanted`, taking in everything else
    /// that arrives meanwhile.
    async fn reply(
        &mut self,
        ctx: &mut Ctx,
        wanted: impl Fn(&Frame) -> bool,
    ) -> att::Result<Option<Frame>> {
        let deadline = Instant::now() + REPLY;
        loop {
            match timeout(deadline - Instant::now(), self.notes.recv()).await {
                Ok(Some((handle, value))) => {
                    if let Some(f) = self.take(ctx, handle, &value).into_iter().find(&wanted) {
                        return Ok(Some(f));
                    }
                }
                Ok(None) => return Err(att::Error::Closed),
                Err(_) => {
                    let rest: Vec<Frame> = self.reader.flush().into_iter().collect();
                    self.apply(ctx, &rest);
                    return Ok(rest.into_iter().find(wanted));
                }
            }
        }
    }

    async fn execute(&mut self, ctx: &mut Ctx, cmd: Command) -> att::Result<()> {
        let state = &mut ctx.state;
        let (frame, expect): (Vec<u8>, Option<u8>) = match cmd {
            Command::Mode(mode) => (p::mode_set(mode), Some(p::CMD_ANC)),
            Command::Eq(choice) => {
                let (category, preset, gains) = match choice {
                    EqChoice::Off => (p::EQ_OFF, EqPreset::Off, [0.0; 10]),
                    EqChoice::Preset(c) => (
                        c,
                        EqPreset::Preset(c),
                        p::preset_gains(c).unwrap_or_default(),
                    ),
                    EqChoice::Custom(g) => (
                        p::EQ_CUSTOM,
                        EqPreset::Custom,
                        g.map(|v| v.clamp(-p::EQ_LIMIT, p::EQ_LIMIT)),
                    ),
                };
                state.eq = Some(Eq { preset, gains });
                (p::eq_set(category, &gains), None)
            }
            Command::VoiceAware(level) => {
                state.voice_aware = Some(level);
                (
                    p::voice_aware_set(level, state.voice_aware_extra),
                    Some(p::CMD_VOICE_AWARE),
                )
            }
            Command::VoicePrompts(on) => {
                state.voice_prompts = Some(on);
                (p::voice_prompts_set(on), None)
            }
            Command::SafeSound(on) => {
                state.safe_sound = Some(on);
                (p::safe_sound_set(on), None)
            }
            Command::AutoOff(enabled) => {
                let minutes = state.auto_off.map_or(30, |a| a.minutes);
                state.auto_off = Some(AutoOff { enabled, minutes });
                (p::auto_off_set(enabled, minutes), None)
            }
            Command::Find(left, right) => {
                let (l, r) = state.find.unwrap_or((false, false));
                state.find = Some((left.unwrap_or(l), right.unwrap_or(r)));
                (p::find_set(left, right), None)
            }
        };
        ctx.publish();
        self.send(&frame).await?;
        if let Some(cmd_byte) = expect {
            let mode_reply = |f: &Frame| {
                f.cmd == cmd_byte && (cmd_byte != p::CMD_ANC || f.payload.first() == Some(&0x12))
            };
            let reply = self.reply(ctx, mode_reply).await?;
            // The buds decline some changes without an error: they report the
            // old value back. Say so instead of showing what was asked for.
            if let (Command::Mode(asked), Some(_)) = (cmd, reply)
                && let Some(actual) = ctx.state.mode.filter(|m| *m != asked)
            {
                ctx.state.notify(format!(
                    "Still on {}: {} needs both earbuds out of the case",
                    mode_label(actual),
                    mode_label(asked)
                ));
                ctx.publish();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_a_slider_sends_only_where_it_stopped() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for c in [
            Command::Eq(EqChoice::Custom([1.0; 10])),
            Command::Find(Some(true), None),
            Command::Eq(EqChoice::Custom([2.0; 10])),
            Command::Find(None, Some(true)),
        ] {
            tx.send(c).unwrap();
        }
        let first = rx.try_recv().unwrap();
        let kept = coalesce(first, &mut rx);
        assert_eq!(kept.len(), 3);
        assert!(
            matches!(kept[2], Command::Eq(EqChoice::Custom(g)) if g[0] == 2.0)
                || matches!(kept[1], Command::Eq(EqChoice::Custom(g)) if g[0] == 2.0)
        );
        assert_eq!(
            kept.iter()
                .filter(|c| matches!(c, Command::Find(..)))
                .count(),
            2
        );
    }

    #[test]
    fn backoff_grows_and_stops_at_a_minute() {
        assert_eq!(backoff(0), Duration::from_secs(1));
        assert_eq!(backoff(2), Duration::from_secs(6));
        assert_eq!(backoff(100), Duration::from_secs(60));
    }
}
