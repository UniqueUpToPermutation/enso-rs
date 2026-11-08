//! MIDI devices, on top of [`midir`].
//!
//! ```no_run
//! use enso_io::{enso_core::Clock, midi::MidiInputs};
//!
//! let clock = Clock::new(); // share this clock with everything else
//! let (sender, receiver) = std::sync::mpsc::channel();
//! let mut inputs = MidiInputs::new(clock, sender);
//! inputs.refresh(); // connect to every input; call again now and then for hot-plugging
//! for event in receiver {
//!     println!("{} {}", event.time, event.message);
//! }
//! ```
//!
//! ## Threads and timestamps
//!
//! midir calls us on its own MIDI thread, once per message. We parse the
//! bytes there, stamp the event with the shared [`Clock`] and pass it to a
//! [`MidiSink`], all without locking. Events are stamped on *arrival*:
//! midir's own timestamps count from an arbitrary per-port origin that we
//! can't line up with our clock.
//!
//! ## Windows
//!
//! With the classic Windows MIDI API, an input can be open in only **one
//! program at a time**. If a DAW holds the keyboard, opening it here fails
//! (logged once; [`MidiInputs::refresh`] keeps retrying quietly).

use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
};

use enso_core::{Clock, MidiEvent, MidiMessage, PortId};

/// The client name other MIDI software sees for us.
const CLIENT_NAME: &str = "enso";

#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("MIDI is unavailable: {0}")]
    Init(#[from] midir::InitError),
    #[error("no MIDI {direction} matches {pattern:?} (available: {available:?})")]
    NoSuchPort { direction: &'static str, pattern: String, available: Vec<String> },
    #[error("failed to open MIDI port {port:?}: {reason}")]
    Connect { port: String, reason: String },
    #[error("failed to send MIDI: {0}")]
    Send(#[from] midir::SendError),
}

// ---- where events go ----------------------------------------------------------------

/// Receives events on the MIDI thread.
///
/// `send` runs once per message, on midir's thread, so it must be quick and
/// must never block. Implemented for:
/// * `std::sync::mpsc::Sender<MidiEvent>`: to the simulation (or any thread);
/// * `rtrb::Producer<MidiEvent>`: a lock-free ring buffer, for the audio
///   thread (which may never wait). Events are dropped if it's full;
/// * `(A, B)`: both, e.g. audio *and* visuals;
/// * [`FnSink`]: any closure.
pub trait MidiSink: Send + 'static {
    fn send(&mut self, event: MidiEvent);
}

impl MidiSink for mpsc::Sender<MidiEvent> {
    fn send(&mut self, event: MidiEvent) {
        // An error only means the receiver is gone, i.e. nobody's listening.
        let _ = mpsc::Sender::send(self, event);
    }
}

impl MidiSink for rtrb::Producer<MidiEvent> {
    fn send(&mut self, event: MidiEvent) {
        if self.push(event).is_err() {
            log::warn!("MIDI ring buffer full: dropped {}", event.message);
        }
    }
}

impl<A: MidiSink, B: MidiSink> MidiSink for (A, B) {
    fn send(&mut self, event: MidiEvent) {
        self.0.send(event);
        self.1.send(event);
    }
}

/// Wraps a closure as a [`MidiSink`]: `FnSink(|event| println!("{event:?}"))`.
///
/// (Why not implement `MidiSink` for closures directly? Rust would then
/// reject the impl for `rtrb::Producer`: the compiler can't rule out that a
/// future rtrb makes `Producer` callable, which would give it two impls.)
#[derive(Debug, Clone)]
pub struct FnSink<F>(pub F);

impl<F: FnMut(MidiEvent) + Send + 'static> MidiSink for FnSink<F> {
    fn send(&mut self, event: MidiEvent) {
        (self.0)(event)
    }
}

// ---- listing ports ------------------------------------------------------------------

/// Names of the MIDI inputs currently available.
pub fn input_names() -> Result<Vec<String>, MidiError> {
    let input = midir::MidiInput::new(CLIENT_NAME)?;
    Ok(port_names(&input))
}

/// Names of the MIDI outputs currently available (including virtual ports,
/// e.g. loopMIDI, that lead to other programs).
pub fn output_names() -> Result<Vec<String>, MidiError> {
    let output = midir::MidiOutput::new(CLIENT_NAME)?;
    Ok(port_names(&output))
}

fn port_names<IO: midir::MidiIO>(io: &IO) -> Vec<String> {
    // A port that vanishes between listing and naming is simply skipped.
    io.ports().iter().filter_map(|port| io.port_name(port).ok()).collect()
}

/// The port whose name contains `pattern` (case-insensitive), with its name.
fn find_port<IO: midir::MidiIO>(
    io: &IO,
    pattern: &str,
    direction: &'static str,
) -> Result<(IO::Port, String), MidiError> {
    let needle = pattern.to_lowercase();
    let ports = io.ports();
    let named: Vec<_> = ports.iter().filter_map(|p| Some((p, io.port_name(p).ok()?))).collect();
    named
        .iter()
        .find(|(_, name)| name.to_lowercase().contains(&needle))
        .map(|(port, name)| ((*port).clone(), name.clone()))
        .ok_or_else(|| MidiError::NoSuchPort {
            direction,
            pattern: pattern.to_owned(),
            available: named.into_iter().map(|(_, name)| name).collect(),
        })
}

// ---- one input ----------------------------------------------------------------------

/// An open MIDI input. Events flow into its sink until it is dropped.
pub struct MidiIn {
    name: String,
    port: PortId,
    _connection: midir::MidiInputConnection<()>,
}

impl MidiIn {
    /// Open the first input whose name contains `pattern` (case-insensitive).
    /// Its events are tagged with `port` and stamped with `clock`.
    pub fn open(pattern: &str, port: PortId, clock: Clock, sink: impl MidiSink) -> Result<Self, MidiError> {
        let input = midir::MidiInput::new(CLIENT_NAME)?;
        let (midir_port, name) = find_port(&input, pattern, "input")?;
        Self::connect(input, &midir_port, name, port, clock, sink)
    }

    fn connect(
        mut input: midir::MidiInput,
        midir_port: &midir::MidiInputPort,
        name: String,
        port: PortId,
        clock: Clock,
        mut sink: impl MidiSink,
    ) -> Result<Self, MidiError> {
        // Drop SysEx, MIDI clock and active sensing before they reach us;
        // we only model channel messages so far.
        input.ignore(midir::Ignore::All);
        let connection = input
            .connect(
                midir_port,
                CLIENT_NAME,
                move |_midir_time, bytes, _| {
                    let time = clock.now(); // first, before any other work
                    if let Some(message) = MidiMessage::parse(bytes) {
                        sink.send(MidiEvent { time, port, message });
                    }
                },
                (),
            )
            .map_err(|err| MidiError::Connect { port: name.clone(), reason: err.to_string() })?;
        Ok(Self { name, port, _connection: connection })
    }

    /// The device's name as the OS reports it.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn port(&self) -> PortId {
        self.port
    }
}

// ---- every input, hot-pluggable ------------------------------------------------------

/// Keeps every MIDI input connected, feeding one sink.
///
/// [`refresh`](Self::refresh) connects inputs that appeared and drops those
/// that vanished; call it at startup and then every second or so (it lists
/// the system's ports, which takes a moment, so not every frame). A device
/// keeps its [`PortId`] across unplugging and replugging, so routing set up
/// for "the keyboard" survives a loose cable.
pub struct MidiInputs<S> {
    clock: Clock,
    sink: S,
    /// Only inputs whose names contain one of these (lowercase); empty = all.
    filters: Vec<String>,
    open: Vec<MidiIn>,
    /// Every device name seen so far, with its permanent ID.
    ids: HashMap<String, PortId>,
    /// Inputs that failed to open; logged once, retried quietly.
    failed: HashSet<String>,
}

impl<S: MidiSink + Clone> MidiInputs<S> {
    /// Nothing is connected until the first [`refresh`](Self::refresh).
    /// Each input gets its own clone of `sink`.
    pub fn new(clock: Clock, sink: S) -> Self {
        Self {
            clock,
            sink,
            filters: Vec::new(),
            open: Vec::new(),
            ids: HashMap::new(),
            failed: HashSet::new(),
        }
    }

    /// Only connect to inputs whose names contain `pattern`
    /// (case-insensitive). Call repeatedly to allow several.
    pub fn only(mut self, pattern: &str) -> Self {
        self.filters.push(pattern.to_lowercase());
        self
    }

    /// The inputs currently open.
    pub fn connected(&self) -> &[MidiIn] {
        &self.open
    }

    /// The permanent ID of the device called `name`, if it has been seen.
    pub fn id_of(&self, name: &str) -> Option<PortId> {
        self.ids.get(name).copied()
    }

    /// Connect new inputs, drop vanished ones. Never fails: problems are
    /// logged, and the next refresh tries again.
    pub fn refresh(&mut self) {
        let names = match input_names() {
            Ok(names) => names,
            Err(err) => {
                log::error!("{err}");
                return;
            }
        };
        let wanted =
            |name: &str| self.filters.is_empty() || self.filters.iter().any(|f| name.to_lowercase().contains(f));

        // Unplugged devices: close their connections.
        self.open.retain(|input| {
            let present = names.iter().any(|n| n == input.name());
            if !present {
                log::info!("MIDI input disconnected: {}", input.name());
            }
            present
        });
        self.failed.retain(|name| names.contains(name));

        for name in names.iter().filter(|n| wanted(n)) {
            if self.open.iter().any(|input| input.name() == name) {
                continue;
            }
            let next_id = PortId(self.ids.len() as u16);
            let id = *self.ids.entry(name.clone()).or_insert(next_id);
            match self.connect(name, id) {
                Ok(input) => {
                    log::info!("MIDI input connected: {name}");
                    self.failed.remove(name);
                    self.open.push(input);
                }
                Err(err) => {
                    if self.failed.insert(name.clone()) {
                        log::warn!("{err} (is another program using it? retrying quietly)");
                    }
                }
            }
        }
    }

    fn connect(&self, name: &str, id: PortId) -> Result<MidiIn, MidiError> {
        let input = midir::MidiInput::new(CLIENT_NAME)?;
        // Match the exact name: `find_port` would also accept "Keyboard 2"
        // for "Keyboard".
        let port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).is_ok_and(|n| n == name))
            .ok_or_else(|| MidiError::Connect { port: name.to_owned(), reason: "it vanished".into() })?;
        MidiIn::connect(input, &port, name.to_owned(), id, self.clock, self.sink.clone())
    }
}

// ---- output -------------------------------------------------------------------------

/// An open MIDI output, e.g. a hardware synth or a virtual port into a DAW.
pub struct MidiOut {
    name: String,
    connection: midir::MidiOutputConnection,
}

impl MidiOut {
    /// Open the first output whose name contains `pattern` (case-insensitive).
    pub fn open(pattern: &str) -> Result<Self, MidiError> {
        let output = midir::MidiOutput::new(CLIENT_NAME)?;
        let (port, name) = find_port(&output, pattern, "output")?;
        let connection = output
            .connect(&port, CLIENT_NAME)
            .map_err(|err| MidiError::Connect { port: name.clone(), reason: err.to_string() })?;
        Ok(Self { name, connection })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Send `message` now.
    pub fn send(&mut self, message: MidiMessage) -> Result<(), MidiError> {
        Ok(self.connection.send(&message.encode())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enso_core::{Channel, Timestamp};

    fn event(key: u8) -> MidiEvent {
        MidiEvent {
            time: Timestamp::from_nanos(u64::from(key)),
            port: PortId(0),
            message: MidiMessage::NoteOn { channel: Channel::new(0).unwrap(), key, velocity: 100 },
        }
    }

    #[test]
    fn sinks_deliver() {
        let (tx, rx) = mpsc::channel();
        let (producer, mut consumer) = rtrb::RingBuffer::new(1);
        let mut sink = (tx, producer);
        sink.send(event(1));
        sink.send(event(2)); // ring buffer full: dropped there, not in the channel
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![event(1), event(2)]);
        assert_eq!(consumer.pop(), Ok(event(1)));
        assert!(consumer.pop().is_err());
    }

    #[test]
    fn closures_are_sinks() {
        let (tx, rx) = mpsc::channel();
        let mut sink = FnSink(move |e: MidiEvent| tx.send(e.time).unwrap());
        sink.send(event(7));
        assert_eq!(rx.recv(), Ok(Timestamp::from_nanos(7)));
    }

    /// Talks to the real MIDI system; fine even with no devices attached.
    #[test]
    fn listing_ports_works_without_devices() {
        assert!(input_names().is_ok());
        assert!(output_names().is_ok());
        let mut inputs = MidiInputs::new(Clock::new(), mpsc::channel().0).only("no device is called this");
        inputs.refresh();
        assert!(inputs.connected().is_empty());
        assert!(matches!(
            MidiOut::open("no device is called this"),
            Err(MidiError::NoSuchPort { direction: "output", .. })
        ));
    }
}
