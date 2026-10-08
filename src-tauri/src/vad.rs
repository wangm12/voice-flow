//! Finalize-time energy VAD with SmoothedVad hangover/onset (no ONNX).

use std::collections::VecDeque;

const SAMPLE_RATE: usize = 16_000;
const FRAME: usize = SAMPLE_RATE * 30 / 1_000;
const HOP: usize = SAMPLE_RATE * 10 / 1_000;
const PREFILL_FRAMES: usize = 45;
const ONSET_FRAMES: usize = 6;
const HANGOVER_FRAMES: usize = 45;

struct BufferedFrame {
    index: usize,
    emitted: bool,
}

pub struct SmoothedVad {
    prefill_frames: usize,
    hangover_frames: usize,
    onset_frames: usize,
    frame_buffer: VecDeque<BufferedFrame>,
    hangover_counter: usize,
    onset_counter: usize,
    in_speech: bool,
}

impl SmoothedVad {
    pub fn new() -> Self {
        Self {
            prefill_frames: PREFILL_FRAMES,
            hangover_frames: HANGOVER_FRAMES,
            onset_frames: ONSET_FRAMES,
            frame_buffer: VecDeque::new(),
            hangover_counter: 0,
            onset_counter: 0,
            in_speech: false,
        }
    }

    /// Returns hop-frame indices that should be included after processing one frame.
    pub fn push(&mut self, frame_index: usize, is_voice: bool) -> Vec<usize> {
        self.frame_buffer.push_back(BufferedFrame {
            index: frame_index,
            emitted: false,
        });
        while self.frame_buffer.len() > self.prefill_frames + 1 {
            self.frame_buffer.pop_front();
        }

        let mut emitted = Vec::new();
        match (self.in_speech, is_voice) {
            (false, true) => {
                self.onset_counter += 1;
                if self.onset_counter >= self.onset_frames {
                    self.in_speech = true;
                    self.hangover_counter = self.hangover_frames;
                    self.onset_counter = 0;
                    for buffered in self.frame_buffer.iter_mut() {
                        if !buffered.emitted {
                            buffered.emitted = true;
                            emitted.push(buffered.index);
                        }
                    }
                }
            }
            (true, true) => {
                self.hangover_counter = self.hangover_frames;
                if let Some(last) = self.frame_buffer.back_mut() {
                    if !last.emitted {
                        last.emitted = true;
                        emitted.push(last.index);
                    }
                }
            }
            (true, false) => {
                if self.hangover_counter > 0 {
                    self.hangover_counter -= 1;
                    if let Some(last) = self.frame_buffer.back_mut() {
                        if !last.emitted {
                            last.emitted = true;
                            emitted.push(last.index);
                        }
                    }
                } else {
                    self.in_speech = false;
                }
            }
            (false, false) => {
                self.onset_counter = 0;
            }
        }
        emitted
    }
}

impl Default for SmoothedVad {
    fn default() -> Self {
        Self::new()
    }
}

pub fn filter_speech(samples: &[f32], enabled: bool) -> Vec<f32> {
    if !enabled {
        return crate::silence::trim_and_compress(samples);
    }
    let energies: Vec<f32> = (0..samples.len())
        .step_by(HOP)
        .map(|start| rms(&samples[start..(start + FRAME).min(samples.len())]))
        .collect();
    let Some(threshold) = crate::silence::activity_threshold(&energies) else {
        return Vec::new();
    };
    let mut vad = SmoothedVad::new();
    let mut keep = vec![false; energies.len()];
    for (index, energy) in energies.iter().enumerate() {
        for accepted in vad.push(index, energy.is_finite() && *energy >= threshold) {
            keep[accepted] = true;
        }
    }
    let mut filtered = Vec::new();
    for (index, accepted) in keep.into_iter().enumerate() {
        if accepted {
            let start = index * HOP;
            filtered.extend_from_slice(&samples[start..(start + HOP).min(samples.len())]);
        }
    }
    crate::silence::trim_and_compress(&filtered)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}
#[cfg(test)]
mod tests {
    use super::{filter_speech, SmoothedVad, HOP, SAMPLE_RATE};

    fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| if i % 2 == 0 { 0.2 } else { -0.2 })
            .collect()
    }

    fn silence(samples: usize) -> Vec<f32> {
        vec![0.0; samples]
    }

    #[test]
    fn smoothed_vad_requires_onset_before_speech() {
        let mut vad = SmoothedVad::new();
        let mut emitted = Vec::new();
        for frame in 0..ONSET_FRAMES - 1 {
            emitted.extend(vad.push(frame, true));
        }
        assert!(emitted.is_empty());
        emitted.extend(vad.push(ONSET_FRAMES - 1, true));
        assert!(!emitted.is_empty());
    }

    #[test]
    fn smoothed_vad_hangover_keeps_trailing_silence() {
        let mut vad = SmoothedVad::new();
        for frame in 0..ONSET_FRAMES {
            let _ = vad.push(frame, true);
        }
        let mut trailing = Vec::new();
        for frame in ONSET_FRAMES..ONSET_FRAMES + 3 {
            trailing.extend(vad.push(frame, false));
        }
        assert!(!trailing.is_empty());
    }

    #[test]
    fn smoothed_vad_prefill_includes_frames_before_onset() {
        let mut vad = SmoothedVad::new();
        for frame in 0..10 {
            let _ = vad.push(frame, false);
        }
        let mut emitted = Vec::new();
        for frame in 10..10 + ONSET_FRAMES {
            emitted.extend(vad.push(frame, true));
        }
        assert!(emitted.contains(&10));
    }

    #[test]
    fn filter_speech_disabled_matches_trim_and_compress() {
        let mut samples = silence(SAMPLE_RATE);
        samples.extend(tone(SAMPLE_RATE));
        samples.extend(silence(SAMPLE_RATE));
        let trimmed = crate::silence::trim_and_compress(&samples);
        assert_eq!(filter_speech(&samples, false), trimmed);
    }

    #[test]
    fn filter_speech_removes_leading_and_trailing_silence() {
        let mut samples = silence(SAMPLE_RATE * 2);
        samples.extend(tone(SAMPLE_RATE));
        samples.extend(silence(SAMPLE_RATE * 2));
        let out = filter_speech(&samples, true);
        assert!(!out.is_empty());
        assert!(out.len() < samples.len());
    }

    #[test]
    fn filter_speech_empty_on_silent_input() {
        assert!(filter_speech(&silence(SAMPLE_RATE * 2), true).is_empty());
        assert!(filter_speech(&[], true).is_empty());
    }

    #[test]
    fn filter_speech_rejects_brief_noise_bursts() {
        let mut samples = silence(SAMPLE_RATE);
        samples.extend(tone(HOP * 3));
        samples.extend(silence(SAMPLE_RATE));
        assert!(filter_speech(&samples, true).is_empty());
    }

    const ONSET_FRAMES: usize = super::ONSET_FRAMES;
}

#[cfg(test)]
mod coverage_tests {
    #[test]
    fn full_tail_and_quiet_speech_are_preserved() {
        for size in [16000, 16001, 16159] {
            for amplitude in [0.2, 0.005] {
                let input: Vec<f32> = (0..size)
                    .map(|i| if i % 2 == 0 { amplitude } else { -amplitude })
                    .collect();
                assert_eq!(super::filter_speech(&input, true), input);
            }
        }
    }
}
