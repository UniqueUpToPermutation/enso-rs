//! MIDI 1.0 channel messages as plain Rust values.
//!
//! Devices speak raw bytes (`[0x90, 60, 100]` = "note on, channel 1, middle
//! C, velocity 100"). [`MidiMessage::parse`] turns them into an enum once, at
//! the edge, so the rest of enso matches on meaning instead of bit patterns;
//! [`MidiMessage::encode`] goes back to bytes for MIDI output.
//!
//! Only channel messages are modelled (notes, controllers, pressure, pitch
//! bend, program change). System messages (SysEx, MIDI clock, active
//! sensing) parse to `None` and are dropped for now.

use std::fmt;

use crate::Timestamp;

/// A MIDI channel. Stored as 0..=15; musicians (and [`Display`](fmt::Display))
/// count 1..=16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Channel(u8);

impl Channel {
    /// `index` in 0..=15, or `None`.
    pub const fn new(index: u8) -> Option<Self> {
        if index < 16 { Some(Self(index)) } else { None }
    }

    /// 0..=15, as on the wire.
    pub const fn index(self) -> u8 {
        self.0
    }

    /// 1..=16, as printed on hardware.
    pub const fn number(self) -> u8 {
        self.0 + 1
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ch{}", self.number())
    }
}

/// Which input a [`MidiEvent`] came from. `enso-io` assigns one per device
/// name and keeps it if the device is unplugged and plugged back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PortId(pub u16);

/// A MIDI channel message. Data values (`key`, `velocity`, ...) are 0..=127.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MidiMessage {
    /// A key went down. `velocity` is 1..=127: a note-on with velocity 0
    /// means note-off on the wire, and [`parse`](Self::parse) returns
    /// [`NoteOff`](Self::NoteOff) for it.
    NoteOn { channel: Channel, key: u8, velocity: u8 },
    /// A key was released. `velocity` is release velocity (64 if the
    /// keyboard doesn't measure it).
    NoteOff { channel: Channel, key: u8, velocity: u8 },
    /// Per-key pressure ("polyphonic aftertouch").
    PolyPressure { channel: Channel, key: u8, pressure: u8 },
    /// A knob, fader, pedal or button (`controller` 0..=127; 1 = mod
    /// wheel, 64 = sustain pedal).
    ControlChange { channel: Channel, controller: u8, value: u8 },
    ProgramChange { channel: Channel, program: u8 },
    /// Whole-channel pressure ("aftertouch").
    ChannelPressure { channel: Channel, pressure: u8 },
    /// -8192..=8191, 0 = centred.
    PitchBend { channel: Channel, value: i16 },
}

impl MidiMessage {
    /// Parse one complete message (status byte first). `None` for system
    /// messages, truncated messages, or data bytes above 127.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (&status, data) = bytes.split_first()?;
        if !(0x80..0xF0).contains(&status) {
            return None; // a data byte, or a system message
        }
        let channel = Channel(status & 0x0F);
        // Every data byte must have its top bit clear.
        let byte = |i: usize| data.get(i).copied().filter(|b| *b < 0x80);

        Some(match status & 0xF0 {
            0x80 => Self::NoteOff { channel, key: byte(0)?, velocity: byte(1)? },
            0x90 => match (byte(0)?, byte(1)?) {
                // The MIDI spec: note-on with velocity 0 is a note-off with velocity 64.
                (key, 0) => Self::NoteOff { channel, key, velocity: 64 },
                (key, velocity) => Self::NoteOn { channel, key, velocity },
            },
            0xA0 => Self::PolyPressure { channel, key: byte(0)?, pressure: byte(1)? },
            0xB0 => Self::ControlChange { channel, controller: byte(0)?, value: byte(1)? },
            0xC0 => Self::ProgramChange { channel, program: byte(0)? },
            0xD0 => Self::ChannelPressure { channel, pressure: byte(0)? },
            0xE0 => {
                // 14 bits, least significant 7 first.
                let raw = u16::from(byte(0)?) | (u16::from(byte(1)?) << 7);
                Self::PitchBend { channel, value: raw as i16 - 8192 }
            }
            _ => unreachable!("status is 0x80..0xF0"),
        })
    }

    /// The wire bytes of this message. Out-of-range values are clamped
    /// (data to 0..=127, pitch bend to -8192..=8191), so the output is
    /// always valid MIDI.
    pub fn encode(&self) -> EncodedMessage {
        let d = |value: u8| value.min(127);
        let status = |kind: u8, channel: Channel| kind | channel.0;
        match *self {
            Self::NoteOn { channel, key, velocity } => EncodedMessage::three(status(0x90, channel), d(key), d(velocity)),
            Self::NoteOff { channel, key, velocity } => EncodedMessage::three(status(0x80, channel), d(key), d(velocity)),
            Self::PolyPressure { channel, key, pressure } => {
                EncodedMessage::three(status(0xA0, channel), d(key), d(pressure))
            }
            Self::ControlChange { channel, controller, value } => {
                EncodedMessage::three(status(0xB0, channel), d(controller), d(value))
            }
            Self::ProgramChange { channel, program } => EncodedMessage::two(status(0xC0, channel), d(program)),
            Self::ChannelPressure { channel, pressure } => EncodedMessage::two(status(0xD0, channel), d(pressure)),
            Self::PitchBend { channel, value } => {
                let raw = (value.clamp(-8192, 8191) + 8192) as u16;
                EncodedMessage::three(status(0xE0, channel), (raw & 0x7F) as u8, (raw >> 7) as u8)
            }
        }
    }

    pub fn channel(&self) -> Channel {
        match *self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::PolyPressure { channel, .. }
            | Self::ControlChange { channel, .. }
            | Self::ProgramChange { channel, .. }
            | Self::ChannelPressure { channel, .. }
            | Self::PitchBend { channel, .. } => channel,
        }
    }
}

impl fmt::Display for MidiMessage {
    /// Human-readable, e.g. `ch1 note on  C4 vel 100`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let channel = self.channel();
        match *self {
            Self::NoteOn { key, velocity, .. } => {
                write!(f, "{channel} note on  {:<4} vel {velocity}", NoteName(key))
            }
            Self::NoteOff { key, velocity, .. } => {
                write!(f, "{channel} note off {:<4} vel {velocity}", NoteName(key))
            }
            Self::PolyPressure { key, pressure, .. } => {
                write!(f, "{channel} pressure {:<4} {pressure}", NoteName(key))
            }
            Self::ControlChange { controller, value, .. } => write!(f, "{channel} cc {controller} = {value}"),
            Self::ProgramChange { program, .. } => write!(f, "{channel} program {program}"),
            Self::ChannelPressure { pressure, .. } => write!(f, "{channel} pressure {pressure}"),
            Self::PitchBend { value, .. } => write!(f, "{channel} pitch bend {value}"),
        }
    }
}

/// The bytes of one message, without allocating (so it can be built on
/// real-time threads). Use it as a slice: `&*encoded` or `encoded.as_slice()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedMessage {
    bytes: [u8; 3],
    len: u8,
}

impl EncodedMessage {
    fn two(a: u8, b: u8) -> Self {
        Self { bytes: [a, b, 0], len: 2 }
    }

    fn three(a: u8, b: u8, c: u8) -> Self {
        Self { bytes: [a, b, c], len: 3 }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl std::ops::Deref for EncodedMessage {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

/// A message, when it happened, and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub time: Timestamp,
    pub port: PortId,
    pub message: MidiMessage,
}

/// Displays a MIDI key as a note name: `NoteName(60)` is `C4` (middle C),
/// `NoteName(61)` is `C#4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteName(pub u8);

impl fmt::Display for NoteName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        let octave = i32::from(self.0 / 12) - 1;
        let name = format!("{}{octave}", NAMES[usize::from(self.0 % 12)]);
        // `pad` honours width/alignment like `{:<4}`.
        f.pad(&name)
    }
}

/// Frequency in Hz of `key` in twelve-tone equal temperament, A4 (key 69) = 440 Hz.
pub fn key_frequency(key: u8) -> f32 {
    440.0 * 2f32.powf((f32::from(key) - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(index: u8) -> Channel {
        Channel::new(index).unwrap()
    }

    #[test]
    fn every_message_round_trips() {
        let messages = [
            MidiMessage::NoteOn { channel: ch(0), key: 60, velocity: 100 },
            MidiMessage::NoteOff { channel: ch(15), key: 127, velocity: 0 },
            MidiMessage::PolyPressure { channel: ch(3), key: 1, pressure: 2 },
            MidiMessage::ControlChange { channel: ch(9), controller: 64, value: 127 },
            MidiMessage::ProgramChange { channel: ch(1), program: 42 },
            MidiMessage::ChannelPressure { channel: ch(2), pressure: 77 },
            MidiMessage::PitchBend { channel: ch(4), value: -8192 },
            MidiMessage::PitchBend { channel: ch(4), value: 0 },
            MidiMessage::PitchBend { channel: ch(4), value: 8191 },
        ];
        for message in messages {
            assert_eq!(MidiMessage::parse(&message.encode()), Some(message), "{message:?}");
        }
    }

    #[test]
    fn known_bytes() {
        assert_eq!(
            MidiMessage::parse(&[0x92, 60, 100]),
            Some(MidiMessage::NoteOn { channel: ch(2), key: 60, velocity: 100 })
        );
        // Pitch bend centre is 0x2000: LSB 0, MSB 64.
        assert_eq!(
            MidiMessage::parse(&[0xE0, 0, 64]),
            Some(MidiMessage::PitchBend { channel: ch(0), value: 0 })
        );
        assert_eq!(&*MidiMessage::ProgramChange { channel: ch(0), program: 5 }.encode(), &[0xC0, 5]);
    }

    #[test]
    fn note_on_with_zero_velocity_is_note_off() {
        assert_eq!(
            MidiMessage::parse(&[0x90, 60, 0]),
            Some(MidiMessage::NoteOff { channel: ch(0), key: 60, velocity: 64 })
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(MidiMessage::parse(&[]), None);
        assert_eq!(MidiMessage::parse(&[0x90, 60]), None); // truncated
        assert_eq!(MidiMessage::parse(&[0x90, 200, 1]), None); // data byte > 127
        assert_eq!(MidiMessage::parse(&[60, 100]), None); // no status byte
        assert_eq!(MidiMessage::parse(&[0xF0, 1, 2, 0xF7]), None); // SysEx
        assert_eq!(MidiMessage::parse(&[0xFE]), None); // active sensing
    }

    #[test]
    fn encode_clamps_out_of_range_values() {
        let encoded = MidiMessage::NoteOn { channel: ch(0), key: 200, velocity: 255 }.encode();
        assert_eq!(&*encoded, &[0x90, 127, 127]);
        let bend = MidiMessage::PitchBend { channel: ch(0), value: i16::MAX }.encode();
        assert_eq!(MidiMessage::parse(&bend), Some(MidiMessage::PitchBend { channel: ch(0), value: 8191 }));
    }

    #[test]
    fn channels() {
        assert_eq!(Channel::new(16), None);
        assert_eq!(ch(0).number(), 1);
        assert_eq!(ch(15).to_string(), "ch16");
    }

    #[test]
    fn note_names_and_frequencies() {
        assert_eq!(NoteName(60).to_string(), "C4");
        assert_eq!(NoteName(61).to_string(), "C#4");
        assert_eq!(NoteName(0).to_string(), "C-1");
        assert_eq!(NoteName(127).to_string(), "G9");
        assert_eq!(format!("[{:<4}]", NoteName(60)), "[C4  ]");
        assert_eq!(key_frequency(69), 440.0);
        assert!((key_frequency(60) - 261.6256).abs() < 1e-3);
    }

    #[test]
    fn display() {
        let on = MidiMessage::NoteOn { channel: ch(0), key: 60, velocity: 100 };
        assert_eq!(on.to_string(), "ch1 note on  C4   vel 100");
    }
}
