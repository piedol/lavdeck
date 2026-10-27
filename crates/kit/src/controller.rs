//! Talks to a DJI Mic Mobile RX over USB: keeps a live [`MicStatus`] from the receiver's pushes
//! and sends allowlisted commands. Reconnects automatically when the receiver is replugged.
//!
//! Commands block the calling thread until the receiver answers (at most a few seconds), so call
//! them off the main thread. Callbacks run on the link's reader or watchdog thread.

use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::commands::{MicCapabilities, MicCommand};
use crate::duml::{DumlFrame, DumlStreamParser};
use crate::status::decoder::{self, ascii};
use crate::status::{ChannelMode, MicStatus};
use crate::usb::UsbLink;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicError {
    NotConnected,
    NotAllowed,
    Timeout,
    Rejected { code: u8 },
    NotApplied,
    Transport(String),
}

impl fmt::Display for MicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => f.write_str("Receiver is not connected"),
            Self::NotAllowed => f.write_str("Command is outside the verified allowlist"),
            Self::Timeout => f.write_str("Receiver did not answer"),
            Self::Rejected { code } => write!(f, "Receiver rejected the command (0x{code:02x})"),
            Self::NotApplied => {
                f.write_str("Receiver accepted the command but its status did not change")
            }
            Self::Transport(message) => write!(f, "USB error: {message}"),
        }
    }
}

impl std::error::Error for MicError {}

/// Why a [`Link`] couldn't open or write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    NotFound,
    Open(String),
    NotOpen,
    Write(String),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("notFound"),
            Self::Open(message) => write!(f, "open({message})"),
            Self::NotOpen => f.write_str("notOpen"),
            Self::Write(message) => write!(f, "write({message})"),
        }
    }
}

/// What a [`Link`] reports while open, from its own thread.
pub trait LinkEvents: Send + Sync {
    fn bytes(&self, bytes: &[u8]);
    fn disconnected(&self);
}

/// The byte pipe to a receiver. The real one (USB interface 6) is private to this crate; this
/// trait exists so tests and demos can stand in a fake receiver.
pub trait Link: Send + Sync {
    fn is_present(&self) -> bool;
    fn is_open(&self) -> bool;
    fn open(self: Arc<Self>, events: Arc<dyn LinkEvents>) -> Result<(), LinkError>;
    fn close(&self);
    fn write(&self, bytes: &[u8]) -> Result<(), LinkError>;
}

type StatusCallback = Arc<dyn Fn(MicStatus) + Send + Sync>;
type ConnectionCallback = Arc<dyn Fn(bool) + Send + Sync>;

struct State {
    status: MicStatus,
    parser: DumlStreamParser,
    pending: HashMap<u16, SyncSender<Result<DumlFrame, MicError>>>,
    sequence: u16,
    on_status: Option<StatusCallback>,
    on_connection_change: Option<ConnectionCallback>,
    watchdog: Option<Arc<Watchdog>>,
}

#[derive(Default)]
struct Watchdog {
    stopped: Mutex<bool>,
    wake: Condvar,
}

struct Inner {
    link: Arc<dyn Link>,
    state: Mutex<State>,
}

/// A handle to the receiver. Clones share one connection; it closes when the last is dropped.
#[derive(Clone)]
pub struct MicController {
    inner: Arc<Inner>,
}

impl Default for MicController {
    fn default() -> Self {
        Self::new()
    }
}

impl MicController {
    /// A controller for the receiver on USB.
    pub fn new() -> Self {
        Self::with_link(Arc::new(UsbLink::new()))
    }

    /// A controller over another link, such as a fake receiver in tests.
    pub fn with_link(link: Arc<dyn Link>) -> Self {
        let sequence = 0x100 + (random_u32() % (0x7fff - 0x100 + 1)) as u16;
        Self {
            inner: Arc::new(Inner {
                link,
                state: Mutex::new(State {
                    status: MicStatus::new(),
                    parser: DumlStreamParser::new(),
                    pending: HashMap::new(),
                    sequence,
                    on_status: None,
                    on_connection_change: None,
                    watchdog: None,
                }),
            }),
        }
    }

    /// Called on an arbitrary thread whenever a status push changes the model.
    pub fn set_on_status(&self, callback: impl Fn(MicStatus) + Send + Sync + 'static) {
        self.inner.state.lock().unwrap().on_status = Some(Arc::new(callback));
    }

    /// Called on an arbitrary thread when the USB link opens (true) or closes (false).
    pub fn set_on_connection_change(&self, callback: impl Fn(bool) + Send + Sync + 'static) {
        self.inner.state.lock().unwrap().on_connection_change = Some(Arc::new(callback));
    }

    pub fn status(&self) -> MicStatus {
        self.inner.state.lock().unwrap().status.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.inner.link.is_open()
    }

    /// Opens the receiver if present and keeps retrying every second while it is absent.
    pub fn start(&self) {
        let watchdog = {
            let mut state = self.inner.state.lock().unwrap();
            if state.watchdog.is_some() {
                return;
            }
            let watchdog = Arc::new(Watchdog::default());
            state.watchdog = Some(watchdog.clone());
            watchdog
        };
        let weak = Arc::downgrade(&self.inner);
        thread::Builder::new()
            .name("lavdeck-kit.watchdog".into())
            .spawn(move || {
                loop {
                    let Some(inner) = weak.upgrade() else { return };
                    if *watchdog.stopped.lock().unwrap() {
                        return;
                    }
                    if !inner.link.is_open() && inner.link.is_present() {
                        let events: Arc<dyn LinkEvents> = Arc::new(Events(Arc::downgrade(&inner)));
                        if inner.link.clone().open(events).is_ok() {
                            let callback = inner.state.lock().unwrap().on_connection_change.clone();
                            if let Some(callback) = callback {
                                callback(true);
                            }
                        }
                    }
                    drop(inner);
                    let stopped = watchdog.stopped.lock().unwrap();
                    let (stopped, _) = watchdog
                        .wake
                        .wait_timeout(stopped, Duration::from_secs(1))
                        .unwrap();
                    if *stopped {
                        return;
                    }
                }
            })
            .expect("spawn watchdog thread");
    }

    pub fn stop(&self) {
        self.inner.stop();
    }

    // MARK: Commands

    /// The receiver's hardware name, e.g. `DJI MIC MINI RX`.
    pub fn version(&self) -> Result<String, MicError> {
        let response = self.send(MicCommand::GetVersion, Duration::from_secs(2))?;
        // Payload: result, then a NUL-padded hardware name, then firmware bytes.
        let name: Vec<u8> = response
            .payload
            .iter()
            .skip(2)
            .take_while(|&&b| b != 0)
            .copied()
            .collect();
        Ok(ascii(&name))
    }

    /// Capabilities: `id` 0 for all, or [`MicCapabilities::GAIN_ID`] / [`MicCapabilities::CHANNEL_MODES_ID`].
    pub fn capabilities(&self, id: u16) -> Result<MicCapabilities, MicError> {
        let response = self.send(MicCommand::GetCapability { id }, Duration::from_secs(2))?;
        MicCapabilities::parse(&response.payload)
            .ok_or_else(|| MicError::Transport("bad capability payload".into()))
    }

    /// Sets mono/stereo and waits until the receiver's status reports the new mode.
    pub fn set_channel(&self, mode: ChannelMode) -> Result<(), MicError> {
        self.send_write(MicCommand::SetChannel(mode))?;
        self.wait_for_status(Duration::from_secs(3), |s| {
            s.receiver.channel_mode == Some(mode)
        })
    }

    /// Sets receiver gain (dB) and waits until the receiver's status reports it.
    pub fn set_receiver_gain(&self, db: i32) -> Result<(), MicError> {
        self.send_write(MicCommand::SetReceiverGain { db })?;
        self.wait_for_status(Duration::from_secs(3), |s| s.receiver.gain_db == Some(db))
    }

    fn send_write(&self, command: MicCommand) -> Result<(), MicError> {
        let response = self.send(command, Duration::from_secs(2))?;
        match response.payload.first() {
            None => Err(MicError::Transport("empty ack".into())),
            Some(0) => Ok(()),
            Some(&code) => Err(MicError::Rejected { code }),
        }
    }

    fn wait_for_status(
        &self,
        timeout: Duration,
        predicate: impl Fn(&MicStatus) -> bool,
    ) -> Result<(), MicError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate(&self.inner.state.lock().unwrap().status) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(MicError::NotApplied)
    }

    fn send(&self, command: MicCommand, timeout: Duration) -> Result<DumlFrame, MicError> {
        if !self.inner.link.is_open() {
            return Err(MicError::NotConnected);
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let seq = {
            let mut state = self.inner.state.lock().unwrap();
            state.sequence = state.sequence.wrapping_add(1);
            if state.sequence < 0x100 {
                state.sequence = 0x100;
            }
            let seq = state.sequence;
            let Some(frame) = command.frame(seq) else {
                return Err(MicError::NotAllowed);
            };
            state.pending.insert(seq, sender);
            drop(state);
            if let Err(error) = self.inner.link.write(&frame.encoded()) {
                self.inner.state.lock().unwrap().pending.remove(&seq);
                return Err(MicError::Transport(error.to_string()));
            }
            seq
        };
        match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                self.inner.state.lock().unwrap().pending.remove(&seq);
                Err(MicError::Timeout)
            }
        }
    }
}

impl Inner {
    fn stop(&self) {
        let watchdog = self.state.lock().unwrap().watchdog.take();
        if let Some(watchdog) = watchdog {
            *watchdog.stopped.lock().unwrap() = true;
            watchdog.wake.notify_all();
        }
        self.link.close();
        self.fail_all_pending(MicError::NotConnected);
    }

    fn handle(&self, bytes: &[u8]) {
        let mut changed = None;
        let frames = self.state.lock().unwrap().parser.append(bytes);
        for frame in frames {
            if frame.is_response() {
                let sender = self.state.lock().unwrap().pending.remove(&frame.sequence);
                if let Some(sender) = sender {
                    let _ = sender.try_send(Ok(frame));
                }
                continue;
            }
            let mut state = self.state.lock().unwrap();
            let mut copy = state.status.clone();
            if !decoder::apply(&frame, &mut copy) {
                continue;
            }
            let differs = copy.receiver != state.status.receiver
                || copy.transmitters != state.status.transmitters;
            state.status = copy;
            if differs {
                changed = Some(state.status.clone());
            }
        }
        if changed.is_some() {
            let (callback, status) = {
                let state = self.state.lock().unwrap();
                (state.on_status.clone(), state.status.clone())
            };
            if let Some(callback) = callback {
                callback(status);
            }
        }
    }

    fn handle_disconnect(&self) {
        let (on_connection, on_status, status) = {
            let mut state = self.state.lock().unwrap();
            state.status = MicStatus::new();
            state.parser = DumlStreamParser::new();
            (
                state.on_connection_change.clone(),
                state.on_status.clone(),
                state.status.clone(),
            )
        };
        self.fail_all_pending(MicError::NotConnected);
        if let Some(callback) = on_connection {
            callback(false);
        }
        if let Some(callback) = on_status {
            callback(status);
        }
    }

    fn fail_all_pending(&self, error: MicError) {
        let all: Vec<_> = self
            .state
            .lock()
            .unwrap()
            .pending
            .drain()
            .map(|(_, sender)| sender)
            .collect();
        for sender in all {
            let _ = sender.try_send(Err(error.clone()));
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Forwards link events to the controller without keeping it alive.
struct Events(Weak<Inner>);

impl LinkEvents for Events {
    fn bytes(&self, bytes: &[u8]) {
        if let Some(inner) = self.0.upgrade() {
            inner.handle(bytes);
        }
    }

    fn disconnected(&self) {
        if let Some(inner) = self.0.upgrade() {
            inner.handle_disconnect();
        }
    }
}

fn random_u32() -> u32 {
    // SAFETY: arc4random has no preconditions.
    unsafe { libc::arc4random() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duml::tests::{identity_push, rx_tx_status_push};
    use crate::duml::{ADDRESS_APP, ADDRESS_RECEIVER};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A receiver in software: acks requests and pushes status the way the Mobile RX does.
    #[derive(Default)]
    struct FakeReceiver {
        open: AtomicBool,
        events: Mutex<Option<Arc<dyn LinkEvents>>>,
        written: Mutex<Vec<Vec<u8>>>,
        /// Ack code for writes; `None` never answers.
        ack: Mutex<Option<u8>>,
        /// Whether an accepted write shows up in the next status push.
        applies: AtomicBool,
    }

    impl FakeReceiver {
        fn new(ack: Option<u8>, applies: bool) -> Arc<Self> {
            let fake = Arc::new(Self::default());
            *fake.ack.lock().unwrap() = ack;
            fake.applies.store(applies, Ordering::SeqCst);
            fake
        }

        fn push(&self, bytes: &[u8]) {
            let events = self.events.lock().unwrap().clone();
            if let Some(events) = events {
                events.bytes(bytes);
            }
        }

        /// The captured receiver push with its status word's mode bits and gain byte replaced.
        fn status_push(mode: u8, gain: i8) -> Vec<u8> {
            let mut frame = DumlFrame::decode(&rx_tx_status_push()).unwrap();
            // Record 0x03 starts at payload offset 3; h is at r+6, gain at r+8.
            let h = frame.payload[9] as u16 | (frame.payload[10] as u16) << 8;
            let h = (h & !(7 << 9)) | (mode as u16) << 9;
            frame.payload[9] = (h & 0xff) as u8;
            frame.payload[10] = (h >> 8) as u8;
            frame.payload[11] = gain as u8;
            frame.encoded()
        }
    }

    impl Link for FakeReceiver {
        fn is_present(&self) -> bool {
            true
        }
        fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }
        fn open(self: Arc<Self>, events: Arc<dyn LinkEvents>) -> Result<(), LinkError> {
            *self.events.lock().unwrap() = Some(events);
            self.open.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn close(&self) {
            self.open.store(false, Ordering::SeqCst);
        }
        fn write(&self, bytes: &[u8]) -> Result<(), LinkError> {
            self.written.lock().unwrap().push(bytes.to_vec());
            let request = DumlFrame::decode(bytes).expect("a well-formed request");
            let Some(code) = *self.ack.lock().unwrap() else {
                return Ok(());
            };
            let ack = DumlFrame {
                sender: ADDRESS_RECEIVER,
                receiver: ADDRESS_APP,
                sequence: request.sequence,
                command_type: 0x80,
                command_set: request.command_set,
                command_id: request.command_id,
                payload: vec![code],
            };
            self.push(&ack.encoded());
            if code == 0 && self.applies.load(Ordering::SeqCst) && request.command_id == 0x01 {
                let p = &request.payload;
                let (id, value) = (p[5] as u16 | (p[6] as u16) << 8, p[8]);
                let push = match id {
                    0x0008 => Self::status_push(value, 0),
                    _ => Self::status_push(0, value as i8),
                };
                self.push(&push);
            }
            Ok(())
        }
    }

    fn connected(fake: &Arc<FakeReceiver>) -> MicController {
        let controller = MicController::with_link(fake.clone());
        controller.start();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !controller.is_connected() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        fake.push(&identity_push());
        fake.push(&rx_tx_status_push());
        controller
    }

    #[test]
    fn set_channel_sends_the_verified_bytes_and_waits_for_status() {
        let fake = FakeReceiver::new(Some(0), true);
        let controller = connected(&fake);
        assert_eq!(
            controller.status().receiver.channel_mode,
            Some(ChannelMode::Mono)
        );
        controller.set_channel(ChannelMode::Stereo).unwrap();
        assert_eq!(
            controller.status().receiver.channel_mode,
            Some(ChannelMode::Stereo)
        );
        let written = fake.written.lock().unwrap().last().cloned().unwrap();
        let sent = DumlFrame::decode(&written).unwrap();
        // Same frame as Sources/LavdeckKit builds for this sequence.
        assert_eq!(
            written,
            MicCommand::SetChannel(ChannelMode::Stereo)
                .frame(sent.sequence)
                .unwrap()
                .encoded()
        );
        assert_eq!(sent.payload, vec![0x02, 0, 0, 0, 0, 0x08, 0x00, 0x01, 0x02]);
        assert!(sent.sequence >= 0x100);
    }

    #[test]
    fn set_gain_round_trips_and_status_callback_fires() {
        let fake = FakeReceiver::new(Some(0), true);
        let controller = connected(&fake);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        controller.set_on_status(move |status| sink.lock().unwrap().push(status.receiver.gain_db));
        controller.set_receiver_gain(-6).unwrap();
        assert_eq!(controller.status().receiver.gain_db, Some(-6));
        assert!(seen.lock().unwrap().contains(&Some(-6)));
    }

    #[test]
    fn writes_report_rejection_timeouts_and_unapplied_changes() {
        let rejecting = FakeReceiver::new(Some(0xff), true);
        assert_eq!(
            connected(&rejecting).set_channel(ChannelMode::Stereo),
            Err(MicError::Rejected { code: 0xff })
        );
        let silent = FakeReceiver::new(None, true);
        let started = Instant::now();
        assert_eq!(
            connected(&silent).set_receiver_gain(6),
            Err(MicError::Timeout)
        );
        assert!(started.elapsed() >= Duration::from_secs(2));
        let stubborn = FakeReceiver::new(Some(0), false);
        assert_eq!(
            connected(&stubborn).set_channel(ChannelMode::Stereo),
            Err(MicError::NotApplied)
        );
    }

    #[test]
    fn commands_outside_the_allowlist_never_reach_the_link() {
        let fake = FakeReceiver::new(Some(0), true);
        let controller = connected(&fake);
        let before = fake.written.lock().unwrap().len();
        assert_eq!(
            controller.set_channel(ChannelMode::Other(1)),
            Err(MicError::NotAllowed)
        );
        assert_eq!(controller.set_receiver_gain(13), Err(MicError::NotAllowed));
        assert_eq!(fake.written.lock().unwrap().len(), before);
    }

    #[test]
    fn disconnect_resets_status_and_fails_pending() {
        let fake = FakeReceiver::new(None, true);
        let controller = connected(&fake);
        let changes = Arc::new(Mutex::new(Vec::new()));
        let sink = changes.clone();
        controller.set_on_connection_change(move |up| sink.lock().unwrap().push(up));
        let waiting = controller.clone();
        let pending = thread::spawn(move || waiting.version());
        thread::sleep(Duration::from_millis(100));
        fake.close();
        let events = fake.events.lock().unwrap().clone().unwrap();
        events.disconnected();
        assert_eq!(pending.join().unwrap(), Err(MicError::NotConnected));
        assert_eq!(controller.status(), MicStatus::new());
        assert_eq!(*changes.lock().unwrap(), vec![false]);
    }

    #[test]
    fn not_connected_without_a_link() {
        let fake = FakeReceiver::new(Some(0), true);
        let controller = MicController::with_link(fake);
        assert_eq!(controller.version(), Err(MicError::NotConnected));
    }
}
