//! Sound cards, via cpal (WASAPI on Windows, CoreAudio on macOS, ALSA on
//! Linux).

use cpal::{
    FromSample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use enso_core::Clock;

use crate::{AudioConfig, Processor};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("audio device error: {0}")]
    Cpal(#[from] cpal::Error),
    #[error("no audio output matches {pattern:?} (available: {available:?})")]
    NoDevice { pattern: String, available: Vec<String> },
    #[error("the system has no default audio output")]
    NoDefaultDevice,
    #[error("unsupported sample format {0}")]
    UnsupportedFormat(SampleFormat),
    #[error("the processor runs at {processor} Hz but the device at {device} Hz")]
    SampleRateMismatch { processor: u32, device: u32 },
}

/// Names of the audio outputs on this system.
pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devices) => devices.map(|d| device_name(&d)).collect(),
        Err(err) => {
            log::error!("failed to list audio outputs: {err}");
            Vec::new()
        }
    }
}

fn device_name(device: &cpal::Device) -> String {
    device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "<unnamed>".into())
}

/// A chosen audio output and its format, not yet playing.
pub struct OutputDevice {
    device: cpal::Device,
    name: String,
    config: cpal::StreamConfig,
    format: SampleFormat,
}

impl OutputDevice {
    /// The system's default output.
    pub fn open_default() -> Result<Self, DeviceError> {
        let device = cpal::default_host().default_output_device().ok_or(DeviceError::NoDefaultDevice)?;
        Self::from_device(device)
    }

    /// The first output whose name contains `pattern` (case-insensitive).
    pub fn open(pattern: &str) -> Result<Self, DeviceError> {
        let needle = pattern.to_lowercase();
        let host = cpal::default_host();
        let device = host
            .output_devices()?
            .find(|d| device_name(d).to_lowercase().contains(&needle))
            .ok_or_else(|| DeviceError::NoDevice { pattern: pattern.into(), available: output_device_names() })?;
        Self::from_device(device)
    }

    fn from_device(device: cpal::Device) -> Result<Self, DeviceError> {
        let supported = device.default_output_config()?;
        Ok(Self { name: device_name(&device), format: supported.sample_format(), config: supported.into(), device })
    }

    /// Ask for buffers of `frames` frames (smaller = lower latency, more
    /// CPU overhead). The OS may round it or ignore it: e.g. Windows'
    /// shared mode keeps its own ~10 ms period.
    pub fn with_buffer_frames(mut self, frames: u32) -> Self {
        self.config.buffer_size = cpal::BufferSize::Fixed(frames);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn channels(&self) -> u16 {
        self.config.channels
    }

    /// The config to create the [`AudioEngine`](crate::AudioEngine) with.
    pub fn audio_config(&self) -> AudioConfig {
        AudioConfig::new(self.config.sample_rate)
    }

    /// Start playing: `processor` moves to the audio thread. Sound stops
    /// when the returned stream is dropped. `clock` must be the clock your
    /// MIDI events are stamped with.
    pub fn start(self, processor: Processor, clock: Clock) -> Result<AudioStream, DeviceError> {
        let rate = processor.config().sample_rate;
        if rate != self.config.sample_rate {
            return Err(DeviceError::SampleRateMismatch { processor: rate, device: self.config.sample_rate });
        }
        let stream = match self.format {
            SampleFormat::F32 => self.build::<f32>(processor, clock),
            SampleFormat::F64 => self.build::<f64>(processor, clock),
            SampleFormat::I16 => self.build::<i16>(processor, clock),
            SampleFormat::I32 => self.build::<i32>(processor, clock),
            SampleFormat::I24 => self.build::<cpal::I24>(processor, clock),
            SampleFormat::U16 => self.build::<u16>(processor, clock),
            SampleFormat::I8 => self.build::<i8>(processor, clock),
            SampleFormat::U8 => self.build::<u8>(processor, clock),
            other => return Err(DeviceError::UnsupportedFormat(other)),
        }?;
        stream.play()?;
        Ok(AudioStream { stream, name: self.name, config: self.config })
    }

    /// Build a stream for sample type `T`. The processor always renders
    /// f32 into a scratch buffer (allocated here, up front), converted to
    /// `T` as it's copied out. For f32 devices the conversion is a copy.
    fn build<T>(&self, mut processor: Processor, clock: Clock) -> Result<cpal::Stream, DeviceError>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = usize::from(self.config.channels);
        let rate = f64::from(self.config.sample_rate);
        // Plenty for any real device; bigger callbacks are done in pieces.
        let mut scratch = vec![0.0f32; 8192 * channels];

        let stream = self.device.build_output_stream(
            self.config,
            move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
                let now = clock.now();
                let total = data.len() / channels;
                let mut done = 0;
                for piece in data.chunks_mut(scratch.len()) {
                    let frames = piece.len() / channels;
                    // Each piece covers its own part of the window ending at `now`.
                    let after = (total - done - frames) as f64 / rate;
                    let piece_now = now - std::time::Duration::from_secs_f64(after);
                    let scratch = &mut scratch[..piece.len()];
                    processor.process(scratch, channels, piece_now);
                    for (out, &sample) in piece.iter_mut().zip(scratch.iter()) {
                        *out = T::from_sample(sample);
                    }
                    done += frames;
                }
            },
            // Runs on cpal's thread, not inside the audio callback: logging is fine.
            |err| log::error!("audio stream: {err}"),
            None,
        )?;
        Ok(stream)
    }
}

/// A playing output. Dropping it stops the sound (and frees the processor).
pub struct AudioStream {
    stream: cpal::Stream,
    name: String,
    config: cpal::StreamConfig,
}

impl AudioStream {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.config.channels
    }

    /// The driver's estimate of frames per callback, if it knows.
    pub fn buffer_frames(&self) -> Option<u32> {
        self.stream.buffer_size().ok()
    }
}
