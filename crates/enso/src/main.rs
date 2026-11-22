//! The enso app: a patch editor for the audio engine, playable from MIDI.
//!
//! Run with:   cargo run -p enso
//!             cargo run -p enso -- "audio output name"
//!
//! Right-click the background to add nodes, right-click a node to remove
//! it, drag between pins to wire (orange squares = MIDI, blue circles =
//! audio). Wiring changes are applied while it plays (nodes keep their
//! state); knob changes are sent straight to the nodes.

use std::time::{Duration, Instant};

use eframe::egui;
use enso_audio::{AudioConfig, AudioEngine, AudioStream, BlockAudioEngine, MidiSender, OutputDevice, Stats};
use enso_core::{Clock, MidiEvent};
use enso_io::midi::MidiInputs;
use enso_ui::{MidiPort, PatchEditor};

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,enso=info,enso_io=info"))
        .init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("enso").with_inner_size([1400.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native("enso", options, Box::new(|_cc| Ok(Box::new(Enso::new()))))
}

struct Enso {
    clock: Clock,
    audio: BlockAudioEngine,
    /// `None` if no audio device could be opened (the editor still works:
    /// the engine is then rendered silently from the UI).
    stream: Option<AudioStream>,
    audio_error: Option<String>,
    /// Scratch for rendering without a device.
    silent: Vec<f32>,
    inputs: MidiInputs<MidiForward>,
    editor: PatchEditor,
    last_poll: Instant,
    stats: Stats,
}

/// Lets `MidiInputs` (enso-io) send to the engine's `MidiSender`.
#[derive(Clone)]
struct MidiForward(MidiSender);

impl enso_io::midi::MidiSink for MidiForward {
    fn send(&mut self, event: MidiEvent) {
        self.0.send(event);
    }
}

impl Enso {
    fn new() -> Self {
        let clock = Clock::new();
        let device = match std::env::args().nth(1) {
            Some(pattern) => OutputDevice::open(&pattern),
            None => OutputDevice::open_default(),
        };
        let config = device.as_ref().map_or(AudioConfig::new(48_000), |d| d.audio_config());

        let mut audio = BlockAudioEngine::new(config);
        let mut editor = PatchEditor::with_starter_patch();
        editor.sync(&mut audio);

        let started = device.map_err(Into::into).and_then(|device| audio.start(device, clock));
        let (stream, audio_error) = match started {
            Ok(stream) => (Some(stream), None),
            Err(err) => {
                log::error!("no audio: {err}");
                (None, Some(err.to_string()))
            }
        };

        let mut inputs = MidiInputs::new(clock, MidiForward(audio.midi_sender()));
        inputs.refresh();

        Self {
            clock,
            audio,
            stream,
            audio_error,
            silent: vec![0.0; 2 * config.max_block],
            inputs,
            editor,
            last_poll: Instant::now(),
            stats: Stats::default(),
        }
    }

    /// Housekeeping a few times a second: MIDI hot-plug, audio stats.
    fn poll(&mut self) {
        if self.last_poll.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_poll = Instant::now();
        self.inputs.refresh();
        if self.stream.is_some() {
            self.stats = self.audio.stats();
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("enso");
            ui.separator();
            match (&self.stream, &self.audio_error) {
                (Some(stream), _) => {
                    let ms = 1000.0 * self.stats.frames as f32 / stream.sample_rate() as f32;
                    ui.label(format!(
                        "🔊 {} · {} Hz · {} frames ({ms:.1} ms)",
                        stream.name(),
                        stream.sample_rate(),
                        self.stats.frames
                    ));
                    ui.separator();
                    ui.label(format!(
                        "load {:.1}% (peak {:.1}%)",
                        self.stats.load * 100.0,
                        self.stats.peak_load * 100.0
                    ));
                }
                (None, Some(err)) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("no audio: {err}"));
                }
                (None, None) => {}
            }
            ui.separator();
            let names: Vec<_> = self.inputs.connected().iter().map(|i| i.name()).collect();
            if names.is_empty() {
                ui.label("🎹 no MIDI inputs");
            } else {
                ui.label(format!("🎹 {}", names.join(", ")));
            }
            if self.stats.disabled_blocks > 0 {
                ui.separator();
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!("{} node(s) crashed and were switched off", self.stats.disabled_blocks),
                );
            }
            if let Some(err) = self.editor.error() {
                ui.separator();
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
        });
    }
}

impl eframe::App for Enso {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();

        egui::Panel::top("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("help").show(ui, |ui| {
            ui.weak(
                "Right-click the background to add a node · right-click a node to remove it · \
                 drag between pins to wire (■ MIDI, ● audio) · scroll to zoom, drag the background to pan",
            );
        });

        let ports: Vec<MidiPort> =
            self.inputs.connected().iter().map(|i| MidiPort { id: i.port(), name: i.name().to_owned() }).collect();
        egui::CentralPanel::default().show(ui, |ui| self.editor.show(ui, &ports));
        self.editor.sync(&mut self.audio);

        // Without a device, render here (into nothing) so changes still apply.
        if self.stream.is_none() {
            self.audio.render(&mut self.silent, 2, self.clock.now());
        }

        // Keep the stats moving even when the mouse is still.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}
