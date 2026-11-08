use alkove_core::{Cue, Focus};

use crate::gain::{InputFloor, InputGain};
use crate::models::LoadedModels;
use crate::profile::ProfileFile;
use crate::wav::MAX_AUDIO_SAMPLES;
use crate::{Error, Result};

pub const FOCUS_HOP: usize = 256;
pub const FOCUS_DELAY_SAMPLES: usize = 256;
pub const MAX_HOST_BATCH_SAMPLES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipe {
    G0,
    G1,
}

impl Recipe {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "g0" => Ok(Self::G0),
            "g1" => Ok(Self::G1),
            _ => Err(Error::InvalidArgument("--recipe must be g0 or g1")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::G0 => "g0",
            Self::G1 => "g1",
        }
    }
}

pub struct FilterSession {
    engine: FilterEngine<NativeFocus, NativeCue>,
}

impl FilterSession {
    pub fn new(models: &LoadedModels, profile: &ProfileFile, recipe: Recipe) -> Result<Self> {
        profile.validate(models)?;
        if profile.checksum_sha256.len() != 64
            || profile.compute_checksum()? != profile.checksum_sha256
        {
            return Err(Error::Profile("profile checksum is invalid"));
        }
        if profile.focus_embedding.len() != 192 {
            return Err(Error::Profile(
                "Focus embedding must contain exactly 192 values",
            ));
        }
        let mut focus = Focus::new(&models.focus)
            .map_err(|_| Error::Model("Focus model could not be initialized"))?;
        // Validation above guarantees an exact, finite, normalized 192-value Focus embedding.
        focus.set_print(Some(&profile.focus_embedding));
        let cue = Cue::new(&models.cue)
            .map_err(|_| Error::Model("Cue model could not be initialized"))?;
        Ok(Self {
            engine: FilterEngine::new(NativeFocus(focus), NativeCue(cue), recipe),
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        self.engine.push(samples)
    }

    pub fn finish(&mut self) -> Result<Vec<f32>> {
        self.engine.finish()
    }

    pub fn metrics(&self) -> FilterMetrics {
        self.engine.metrics()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilterMetrics {
    pub input_samples: u64,
    pub output_samples: u64,
    pub processed_hops: u64,
    pub cue_observation_frames: u64,
    pub hop_padding_samples: u64,
    pub flush_padding_samples: u64,
}

trait FocusStep {
    fn step(&mut self, input: &[f32; FOCUS_HOP], output: &mut [f32; FOCUS_HOP]);
}

struct NativeFocus(Focus);

impl FocusStep for NativeFocus {
    fn step(&mut self, input: &[f32; FOCUS_HOP], output: &mut [f32; FOCUS_HOP]) {
        self.0.step(input, output);
    }
}

trait CueObserve {
    fn observe(&mut self, input: &[f32; FOCUS_HOP]) -> Result<u64>;
}

struct NativeCue(Cue);

impl CueObserve for NativeCue {
    fn observe(&mut self, input: &[f32; FOCUS_HOP]) -> Result<u64> {
        let mut frames = 0;
        let mut invalid_probability = false;
        self.0.push(input, |probability, _| {
            // Cue is an observation only. No probability or event deletes or gates audio.
            if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
                invalid_probability = true;
            }
            frames += 1;
        });
        if invalid_probability {
            Err(Error::Model("Cue produced an invalid probability"))
        } else {
            Ok(frames)
        }
    }
}

struct FilterEngine<F, C> {
    focus: F,
    cue: C,
    gain: Option<InputGain>,
    floor: InputFloor,
    pending: [f32; FOCUS_HOP],
    pending_len: usize,
    delay_to_drop: usize,
    finished: bool,
    metrics: FilterMetrics,
}

impl<F: FocusStep, C: CueObserve> FilterEngine<F, C> {
    fn new(focus: F, cue: C, recipe: Recipe) -> Self {
        Self {
            focus,
            cue,
            gain: (recipe == Recipe::G1).then(InputGain::default),
            floor: InputFloor::default(),
            pending: [0.0; FOCUS_HOP],
            pending_len: 0,
            delay_to_drop: FOCUS_DELAY_SAMPLES,
            finished: false,
            metrics: FilterMetrics::default(),
        }
    }

    fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        if self.finished {
            return Err(Error::Protocol("audio arrived after stream finish"));
        }
        if samples.iter().any(|x| !x.is_finite()) {
            return Err(Error::Input("audio contains a non-finite sample"));
        }
        let new_total = self
            .metrics
            .input_samples
            .checked_add(samples.len() as u64)
            .ok_or(Error::Input("audio sample count overflowed"))?;
        if new_total > MAX_AUDIO_SAMPLES as u64 {
            return Err(Error::Input(
                "input audio exceeds the 15-minute sample limit",
            ));
        }
        self.metrics.input_samples = new_total;
        let mut output = Vec::with_capacity(samples.len());
        for &sample in samples {
            self.pending[self.pending_len] = sample;
            self.pending_len += 1;
            if self.pending_len == FOCUS_HOP {
                let hop = self.pending;
                self.process_hop(&hop, &mut output)?;
                self.pending.fill(0.0);
                self.pending_len = 0;
            }
        }
        let remaining_valid = self
            .metrics
            .input_samples
            .saturating_sub(self.metrics.output_samples) as usize;
        output.truncate(remaining_valid);
        self.metrics.output_samples += output.len() as u64;
        Ok(output)
    }

    fn finish(&mut self) -> Result<Vec<f32>> {
        if self.finished {
            return Err(Error::Protocol("stream was already finished"));
        }
        if self.metrics.input_samples == 0 {
            return Err(Error::Input("input audio is empty"));
        }
        let mut output = Vec::with_capacity(FOCUS_HOP * 2);
        if self.pending_len > 0 {
            self.metrics.hop_padding_samples = (FOCUS_HOP - self.pending_len) as u64;
            let hop = self.pending;
            self.process_hop(&hop, &mut output)?;
            self.pending.fill(0.0);
            self.pending_len = 0;
        }
        // Focus emits one hop late. Flush exactly one zero hop, then trim to valid source samples.
        self.metrics.flush_padding_samples = FOCUS_DELAY_SAMPLES as u64;
        self.process_hop(&[0.0; FOCUS_HOP], &mut output)?;
        let remaining_valid = self
            .metrics
            .input_samples
            .saturating_sub(self.metrics.output_samples) as usize;
        output.truncate(remaining_valid);
        self.metrics.output_samples += output.len() as u64;
        self.finished = true;
        if self.metrics.output_samples != self.metrics.input_samples {
            return Err(Error::Model(
                "Focus output could not be aligned to the complete input",
            ));
        }
        Ok(output)
    }

    fn process_hop(&mut self, raw: &[f32; FOCUS_HOP], output: &mut Vec<f32>) -> Result<()> {
        let mut gained = *raw;
        if let Some(gain) = &mut self.gain {
            let floor = self.floor.observe(raw);
            gain.hop(raw, floor);
            gain.apply(raw, &mut gained);
            if gained.iter().any(|x| !x.is_finite()) {
                return Err(Error::Input("input gain produced a non-finite sample"));
            }
        }
        let mut focused = [0.0; FOCUS_HOP];
        self.focus.step(&gained, &mut focused);
        if focused.iter().any(|x| !x.is_finite()) {
            return Err(Error::Model("Focus produced a non-finite sample"));
        }
        self.metrics.cue_observation_frames += self.cue.observe(&focused)?;
        self.metrics.processed_hops += 1;
        let drop = self.delay_to_drop.min(FOCUS_HOP);
        self.delay_to_drop -= drop;
        output.extend_from_slice(&focused[drop..]);
        Ok(())
    }

    fn metrics(&self) -> FilterMetrics {
        self.metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct MockFocus {
        previous: [f32; FOCUS_HOP],
        first: bool,
    }

    impl Default for MockFocus {
        fn default() -> Self {
            Self {
                previous: [0.0; FOCUS_HOP],
                first: true,
            }
        }
    }

    impl FocusStep for MockFocus {
        fn step(&mut self, input: &[f32; FOCUS_HOP], output: &mut [f32; FOCUS_HOP]) {
            if self.first {
                output.fill(0.0);
                self.first = false;
            } else {
                *output = self.previous;
            }
            self.previous = *input;
        }
    }

    #[derive(Default)]
    struct MockCue {
        pending: usize,
    }

    impl CueObserve for MockCue {
        fn observe(&mut self, input: &[f32; FOCUS_HOP]) -> Result<u64> {
            self.pending += input.len();
            let frames = self.pending / 480;
            self.pending %= 480;
            Ok(frames as u64)
        }
    }

    fn run_chunks(input: &[f32], chunks: &[usize]) -> (Vec<f32>, FilterMetrics) {
        let mut pipeline = FilterEngine::new(MockFocus::default(), MockCue::default(), Recipe::G0);
        let mut output = Vec::new();
        let mut at = 0;
        let mut chunk_i = 0;
        while at < input.len() {
            let size = chunks[chunk_i % chunks.len()].max(1).min(input.len() - at);
            output.extend(pipeline.push(&input[at..at + size]).unwrap());
            at += size;
            chunk_i += 1;
        }
        output.extend(pipeline.finish().unwrap());
        (output, pipeline.metrics())
    }

    #[test]
    fn stream_and_file_chunking_have_identical_aligned_output_and_exact_length() {
        let input: Vec<f32> = (0..13_007)
            .map(|i| ((i as f32 * 0.013).sin()) * 0.25)
            .collect();
        let (file_chunks, file_metrics) = run_chunks(&input, &[1024]);
        let (stream_chunks, stream_metrics) = run_chunks(&input, &[1, 127, 255, 769, 1024, 31]);
        assert_eq!(file_chunks, input);
        assert_eq!(stream_chunks, file_chunks);
        assert_eq!(file_chunks.len(), input.len());
        assert_eq!(stream_metrics.output_samples, input.len() as u64);
        assert_eq!(file_metrics.output_samples, input.len() as u64);
        assert_eq!(file_metrics.processed_hops, stream_metrics.processed_hops);
    }

    #[test]
    fn cue_observation_never_changes_the_audio_output() {
        let input: Vec<f32> = (0..512).map(|i| i as f32 / 1024.0).collect();
        let (output, _) = run_chunks(&input, &[64]);
        assert_eq!(output, input);
    }
}
