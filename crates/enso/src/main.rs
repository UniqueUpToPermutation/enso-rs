//! The enso app: a patch editor for the audio engine, playable from MIDI.
//!
//! Run with:   cargo run -p enso
//!             cargo run -p enso -- <audio output name>
//!
//! Right-click the background to add nodes, right-click a node to remove
//! it, drag between pins to wire (orange squares = MIDI, blue circles =
//! audio). Wiring changes rebuild the audio workstream while it plays
//! (blocks keep their state); knob changes are sent straight to the blocks.

use std::time::{Duration, Instant};

use eframe::egui;
use enso_audio::{
    AudioConfig, AudioDriver, AudioStream, OutputDevice, StatsHandle,
    enso_block::{Engine, Workstream, WorkstreamId},
};
use enso_core::{Clock, MidiEvent};
use enso_io::midi::MidiInputs;
use enso_ui::{MidiPort, PatchEditor, PatchRoot};

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
    engine: Engine,
    audio: WorkstreamId,
    /// `None` if no audio device could be opened (the editor still works:
    /// the workstream is then stepped silently from the UI).
    stream: Option<(AudioStream, StatsHandle)>,
    audio_error: Option<String>,
    inputs: MidiInputs<MidiForwardClone>,
    editor: PatchEditor,
    last_poll: Instant,
    stats: enso_audio::Stats,
}

/// `MidiInputs` needs a `Clone` sink; a cloneable wrapper around the
/// engine's `MailSender`.
#[derive(Clone)]
struct MidiForwardClone(enso_audio::enso_block::MailSender<MidiEvent>);

impl enso_io::midi::MidiSink for MidiForwardClone {
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

        // The audio patch: a real-time workstream built from the editor's graph.
        let mut editor = PatchEditor::with_starter_patch(config);
        let mut engine = Engine::new();
        let workstream = Workstream::build_realtime(editor.root()).expect("the starter patch builds");
        let audio = engine.add("audio", workstream);
        let root = engine.root::<PatchRoot>(audio).expect("just added");
        let midi_inbox = engine.get(root.source).expect("built").input;
        let output = engine.get(root.output).expect("built").input;
        let midi = engine.sender(midi_inbox).expect("exposed");

        let (stream, audio_error) = match device {
            Ok(device) => {
                // The stepping side moves to the audio thread.
                let driver = AudioDriver::new(engine.take(audio).expect("just added"), output, config);
                let stats = driver.stats();
                match device.start(driver, clock) {
                    Ok(stream) => (Some((stream, stats)), None),
                    Err(err) => (None, Some(err.to_string())),
                }
            }
            Err(err) => (None, Some(err.to_string())),
        };
        if let Some(err) = &audio_error {
            log::error!("no audio: {err}");
        }

        let mut inputs = MidiInputs::new(clock, MidiForwardClone(midi));
        inputs.refresh();

        Self {
            engine,
            audio,
            stream,
            audio_error,
            inputs,
            editor,
            last_poll: Instant::now(),
            stats: enso_audio::Stats::default(),
        }
    }

    /// Housekeeping a few times a second: MIDI hot-plug, audio stats.
    fn poll(&mut self) {
        if self.last_poll.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.last_poll = Instant::now();
        self.inputs.refresh();
        if let Some((_, stats)) = &self.stream {
            self.stats = stats.read();
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("enso");
            ui.separator();
            match (&self.stream, &self.audio_error) {
                (Some((stream, _)), _) => {
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
                    format!("{} block(s) crashed and were switched off", self.stats.disabled_blocks),
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
        self.editor.sync(&mut self.engine, self.audio);

        // Without a device, step the workstream here so changes still apply.
        if self.stream.is_none() {
            self.engine.step_all();
        }
        self.engine.collect_garbage();

        // Keep the stats moving even when the mouse is still.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}
