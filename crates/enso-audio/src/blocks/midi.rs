//! MIDI into the audio workstream: [`MidiSource`] and [`MidiFilter`].

use enso_block::{Block, In, Inbox, Io, Out, Ports};
use enso_core::{Channel, MidiEvent, PortId};

use crate::{AudioContext, BlockEvent, MidiEvents, midi_events};

/// Messages waiting for their time to come.
const PENDING: usize = 4096;

/// Where MIDI enters an audio workstream. MIDI threads send timestamped
/// events to its `midi` inbox (through a `MailSender` from the engine; it
/// stays valid across rebuilds); each step, the events falling in the
/// step's time window come out of `out`, placed at their exact frames.
/// Events stamped in the future wait, so they can be scheduled ahead.
#[derive(Ports)]
pub struct MidiSource {
    #[inbox(name = "midi", capacity = 1024)]
    pub input: Inbox<MidiEvent>,
    #[output(initial = midi_events())]
    pub out: Out<MidiEvents>,
    /// Received, not yet due; sorted by time.
    #[state]
    pending: Vec<MidiEvent>,
    /// Events dropped because `pending` was full.
    #[state]
    dropped: u64,
}

impl MidiSource {
    pub fn new() -> Self {
        Self { input: Inbox::default(), out: Out::default(), pending: Vec::with_capacity(PENDING), dropped: 0 }
    }
}

impl Default for MidiSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Block for MidiSource {
    fn process(&mut self, io: &Io) {
        let Some(ctx) = io.context::<AudioContext>() else { return };
        for event in io.receive(self.input) {
            if self.pending.len() == self.pending.capacity() {
                self.dropped += 1;
                continue;
            }
            // Usually the newest: scan from the end. Within capacity: no allocation.
            let at = self.pending.iter().rposition(|e| e.time <= event.time).map_or(0, |i| i + 1);
            self.pending.insert(at, event);
        }

        let mut out = io.write(self.out);
        out.clear();
        let mut due = 0;
        for event in &self.pending {
            let Some(frame) = ctx.frame_of(event.time) else { break }; // sorted: the rest are later
            out.push(BlockEvent { time: event.time, offset: frame as u32, port: event.port, message: event.message });
            due += 1;
        }
        self.pending.drain(..due);
    }
}

/// Passes on only MIDI from one input device and/or one channel.
#[derive(Ports)]
pub struct MidiFilter {
    /// Only this device (`None`: any).
    pub port: Option<PortId>,
    /// Only this channel (`None`: any).
    pub channel: Option<Channel>,
    #[input(name = "in", default = midi_events(), merge = Append)]
    pub input: In<MidiEvents>,
    #[output(initial = midi_events())]
    pub out: Out<MidiEvents>,
}

impl MidiFilter {
    pub fn new(port: Option<PortId>, channel: Option<Channel>) -> Self {
        Self { port, channel, input: In::default(), out: Out::default() }
    }

    fn passes(&self, event: &BlockEvent) -> bool {
        self.port.is_none_or(|p| p == event.port) && self.channel.is_none_or(|c| c == event.message.channel())
    }
}

impl Block for MidiFilter {
    fn process(&mut self, io: &Io) {
        let input = io.read(self.input);
        let mut out = io.write(self.out);
        out.clear();
        for event in input.as_slice().iter().filter(|e| self.passes(e)) {
            out.push(*event);
        }
    }
}
