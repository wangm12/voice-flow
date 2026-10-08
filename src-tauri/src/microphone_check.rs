//! Local input metering only: no sample buffer, spool, provider, or transcript.
use cpal::traits::{DeviceTrait, StreamTrait};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub const MAX_SECONDS: u64 = 30;

#[derive(Clone, Default, Serialize)]
pub struct Levels {
    pub level: f32,
    pub peak: f32,
    pub received_frames: bool,
    pub signal_detected: bool,
    pub clipping_detected: bool,
}

impl Levels {
    fn observe<T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        samples: &[T],
        channels: usize,
        gain: f32,
    ) where
        f32: cpal::FromSample<T>,
    {
        if samples.is_empty() {
            return;
        }
        let mut sum = 0.0;
        let mut peak: f32 = 0.0;
        let mut count = 0;
        for frame in samples.chunks(channels.max(1)) {
            let sample = frame
                .iter()
                .map(|sample| sample.to_sample::<f32>())
                .sum::<f32>()
                / frame.len() as f32;
            let sample = if sample.is_finite() {
                sample * gain
            } else {
                0.0
            };
            peak = peak.max(sample.abs());
            sum += sample * sample;
            count += 1;
        }
        let rms = (sum / count as f32).sqrt();
        self.received_frames = true;
        self.signal_detected |= rms > 0.008;
        self.clipping_detected |= peak >= 0.99;
        self.peak = self.peak.max(peak.min(1.0));
        // A dB scale keeps quiet inputs visible without calling noise speech.
        self.level = if rms > 0.0 {
            ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

#[derive(Clone, Serialize)]
pub struct Status {
    pub session_id: String,
    pub device_name: String,
    pub state: String,
    pub elapsed_secs: u64,
    pub input_gain: f32,
    #[serde(flatten)]
    pub levels: Levels,
    pub error: Option<String>,
}

pub struct Check {
    // Owned on the audio engine thread, like recording / warm streams.
    _stream: cpal::Stream,
    pub session_id: String,
    app: AppHandle,
    device_name: String,
    input_gain: f32,
    started: Instant,
    levels: Arc<Mutex<Levels>>,
    error: Arc<Mutex<Option<String>>>,
}

impl Check {
    pub fn open(
        app: AppHandle,
        session_id: String,
        selection: &str,
        input_gain: f32,
    ) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = crate::audio::resolve_input_device(&host, selection)
            .map_err(|error| error.to_string())?;
        let device_name = device.name().map_err(|error| error.to_string())?;
        let supported = device
            .default_input_config()
            .map_err(|error| error.to_string())?;
        let channels = supported.channels() as usize;
        let config = supported.config();
        let levels = Arc::new(Mutex::new(Levels::default()));
        let callback_levels = levels.clone();
        let error = Arc::new(Mutex::new(None));
        let callback_error = error.clone();
        let err_fn = move |failure: cpal::StreamError| {
            *crate::lock_recover(&callback_error) = Some(failure.to_string());
        };
        macro_rules! build {
            ($ty:ty) => {
                device.build_input_stream(
                    &config,
                    move |samples: &[$ty], _| {
                        crate::lock_recover(&callback_levels)
                            .observe(samples, channels, input_gain);
                    },
                    err_fn,
                    None,
                )
            };
        }
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::U16 => build!(u16),
            _ => return Err("unsupported microphone sample format".into()),
        }
        .map_err(|error| error.to_string())?;
        stream.play().map_err(|error| error.to_string())?;
        Ok(Self {
            _stream: stream,
            session_id,
            app,
            device_name,
            input_gain,
            started: Instant::now(),
            levels,
            error,
        })
    }

    pub fn status(&self, state: &str) -> Status {
        Status {
            session_id: self.session_id.clone(),
            device_name: self.device_name.clone(),
            state: state.into(),
            elapsed_secs: self.started.elapsed().as_secs(),
            input_gain: self.input_gain,
            levels: crate::lock_recover(&self.levels).clone(),
            error: crate::lock_recover(&self.error).clone(),
        }
    }

    pub fn terminal_state(&self) -> Option<&'static str> {
        if !self.app.get_webview_window("main").is_some_and(|window| {
            window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
        }) {
            Some("interrupted")
        } else if !crate::permissions::check().microphone
            || crate::lock_recover(&self.error).is_some()
        {
            Some("error")
        } else if self.started.elapsed() >= Duration::from_secs(MAX_SECONDS) {
            Some("completed")
        } else {
            None
        }
    }

    pub fn emit(&self, state: &str) {
        let _ = self
            .app
            .emit("microphone-check://state", self.status(state));
    }
}

/// Late stop requests cannot cancel a newer check; pre-dispatch stops remain bounded.
#[derive(Default)]
pub struct Cancellations(std::collections::VecDeque<String>);
impl Cancellations {
    pub fn remember(&mut self, id: String) {
        if !self.0.contains(&id) {
            self.0.push_back(id);
        }
        while self.0.len() > 64 {
            self.0.pop_front();
        }
    }
    pub fn take(&mut self, id: &str) -> bool {
        let Some(index) = self.0.iter().position(|value| value == id) else {
            return false;
        };
        self.0.remove(index);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_no_frames_silence_signal_and_gain_clipping() {
        let mut levels = Levels::default();
        levels.observe::<f32>(&[], 1, 1.0);
        assert!(!levels.received_frames);
        levels.observe(&[0.0_f32; 16], 1, 1.0);
        assert!(levels.received_frames);
        assert!(!levels.signal_detected);
        assert_eq!(levels.level, 0.0);
        levels.observe(&[0.02_f32; 16], 1, 1.0);
        assert!(levels.signal_detected);
        assert!(levels.level > 0.0 && levels.level < 1.0);
        levels.observe(&[0.6_f32; 16], 1, 2.0);
        assert!(levels.clipping_detected);
        levels.observe(&[0.0_f32; 16], 1, 1.0);
        assert!(levels.signal_detected && levels.clipping_detected);
        assert_eq!(levels.level, 0.0);
    }
    #[test]
    fn stereo_meter_matches_mono_recording_and_handles_non_finite_input() {
        let mut levels = Levels::default();
        levels.observe(&[0.5_f32, -0.5, f32::NAN, f32::INFINITY], 2, 1.0);
        assert_eq!(levels.level, 0.0);
        assert!(!levels.signal_detected);
        levels.observe(&[8192_i16; 16], 2, 1.0);
        assert!(levels.signal_detected);
    }
    #[test]
    fn cancellations_are_scoped_consumed_and_bounded() {
        let mut cancellations = Cancellations::default();
        cancellations.remember("old".into());
        assert!(!cancellations.take("new"));
        assert!(cancellations.take("old"));
        assert!(!cancellations.take("old"));
        for index in 0..100 {
            cancellations.remember(index.to_string());
        }
        assert_eq!(cancellations.0.len(), 64);
        assert!(!cancellations.take("0"));
        assert!(cancellations.take("99"));
    }
}
