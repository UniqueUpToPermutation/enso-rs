//! Print every MIDI message from every input, with timestamps.
//!
//! Run with:   cargo run -p enso-io --example midi_monitor
//!
//! Plug devices in and out while it runs: inputs are picked up and dropped
//! within a second. Ctrl+C quits.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use enso_io::{
    enso_core::{Clock, MidiEvent},
    midi::{MidiInputs, output_names},
};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    match output_names() {
        Ok(names) => log::info!("MIDI outputs: {names:?}"),
        Err(err) => log::error!("{err}"),
    }

    let clock = Clock::new();
    let (sender, receiver) = mpsc::channel::<MidiEvent>();
    let mut inputs = MidiInputs::new(clock, sender);
    inputs.refresh();
    if inputs.connected().is_empty() {
        log::info!("no MIDI inputs yet; waiting for one to be plugged in");
    }
    let mut last_refresh = Instant::now();

    loop {
        if let Ok(event) = receiver.recv_timeout(Duration::from_millis(250)) {
            let name = inputs.connected().iter().find(|i| i.port() == event.port).map_or("?", |i| i.name());
            // How long the event waited between arriving and being printed.
            let waited = clock.now().duration_since(event.time);
            println!("{:>10}  {name:<24}  {:<28} (+{}us)", event.time.to_string(), event.message.to_string(), waited.as_micros());
        }
        if last_refresh.elapsed() >= Duration::from_secs(1) {
            inputs.refresh(); // pick up plugged/unplugged devices
            last_refresh = Instant::now();
        }
    }
}
