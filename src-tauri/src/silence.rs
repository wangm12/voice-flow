//! Leading/trailing trim and long-pause compression for 16 kHz mono PCM.

const SAMPLE_RATE: usize = 16_000;
const FRAME: usize = SAMPLE_RATE * 30 / 1_000;
const HOP: usize = SAMPLE_RATE * 10 / 1_000;
const MIN_ACTIVITY_RMS: f32 = 0.0008;
const MAX_ADAPTIVE_RMS: f32 = 0.01;
const PAD_SAMPLES: usize = SAMPLE_RATE * 200 / 1_000;
const LONG_GAP: usize = SAMPLE_RATE * 3 / 2;
const KEPT_GAP: usize = SAMPLE_RATE * 400 / 1_000;

pub fn trim_and_compress(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    let speech = speech_ranges(samples);
    if speech.is_empty() {
        return Vec::new();
    }
    let first = speech[0].0.saturating_sub(PAD_SAMPLES);
    let last = (speech[speech.len() - 1].1 + PAD_SAMPLES).min(samples.len());
    let mut out = Vec::with_capacity(last.saturating_sub(first));
    let mut cursor = first;
    for (start, end) in speech {
        let gap_from = cursor;
        let gap_to = start;
        if gap_to > gap_from {
            let gap = gap_to - gap_from;
            let keep = if gap > LONG_GAP {
                KEPT_GAP.min(gap)
            } else {
                gap
            };
            out.extend_from_slice(&samples[gap_to - keep..gap_to]);
        }
        out.extend_from_slice(&samples[start..end]);
        cursor = end;
    }
    let tail_to = last;
    if tail_to > cursor {
        out.extend_from_slice(&samples[cursor..tail_to]);
    }
    out
}

fn speech_ranges(samples: &[f32]) -> Vec<(usize, usize)> {
    let frame_rms: Vec<(usize, f32)> = (0..samples.len())
        .step_by(HOP)
        .map(|index| {
            let end = (index + FRAME).min(samples.len());
            (index, rms(&samples[index..end]))
        })
        .collect();
    let energies: Vec<f32> = frame_rms.iter().map(|(_, energy)| *energy).collect();
    let Some(threshold) = activity_threshold(&energies) else {
        return Vec::new();
    };

    let mut ranges = Vec::new();
    let mut start = None;
    for (index, energy) in frame_rms {
        let end = (index + FRAME).min(samples.len());
        let spoken = energy.is_finite() && energy >= threshold;
        if spoken && start.is_none() {
            start = Some(index);
        }
        if !spoken {
            if let Some(from) = start.take() {
                ranges.push((from, index));
            }
        }
        if end == samples.len() {
            if let Some(from) = start.take() {
                ranges.push((from, end));
            }
            break;
        }
    }
    merge_close(ranges)
}

fn merge_close(ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.0 <= last.1 + HOP {
                last.1 = last.1.max(range.1);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Adaptive activity threshold shared by trimming and finalize-time VAD.
pub(crate) fn activity_threshold(energies: &[f32]) -> Option<f32> {
    let peak = energies
        .iter()
        .copied()
        .filter(|energy| energy.is_finite())
        .fold(0.0_f32, f32::max);
    if peak < MIN_ACTIVITY_RMS {
        return None;
    }

    // Use the quieter part of this recording to estimate the local floor, and
    // cap it relative to peak energy so a sustained quiet utterance is not
    // mistaken for noise. Exact digital silence remains distinguishable.
    let mut ordered_energy: Vec<f32> = energies
        .iter()
        .copied()
        .filter(|energy| energy.is_finite())
        .collect();
    ordered_energy.sort_by(f32::total_cmp);
    let lower_energy = ordered_energy[ordered_energy.len() / 5];
    let adaptive_threshold = if lower_energy >= peak * 0.5 {
        peak * 0.08
    } else {
        (lower_energy * 1.5).min(peak * 0.1)
    };
    Some(adaptive_threshold.clamp(MIN_ACTIVITY_RMS, MAX_ADAPTIVE_RMS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| if i % 2 == 0 { 0.2 } else { -0.2 })
            .collect()
    }

    #[test]
    fn trims_leading_and_trailing_silence() {
        let mut samples = vec![0.0; SAMPLE_RATE];
        samples.extend(tone(SAMPLE_RATE));
        samples.extend(vec![0.0; SAMPLE_RATE]);
        let out = trim_and_compress(&samples);
        assert!(out.len() < samples.len());
        assert!(out.len() > SAMPLE_RATE);
        assert!(rms(&out) > 0.05);
    }

    #[test]
    fn compresses_long_internal_pauses() {
        let mut samples = tone(SAMPLE_RATE / 2);
        samples.extend(vec![0.0; SAMPLE_RATE * 2]);
        samples.extend(tone(SAMPLE_RATE / 2));
        let out = trim_and_compress(&samples);
        assert!(out.len() < SAMPLE_RATE + KEPT_GAP + SAMPLE_RATE);
        assert!(out.len() > SAMPLE_RATE / 2);
    }

    #[test]
    fn empty_or_silent_audio_returns_empty() {
        assert!(trim_and_compress(&[]).is_empty());
        assert!(trim_and_compress(&vec![0.0; SAMPLE_RATE]).is_empty());
    }

    #[test]
    fn retains_low_volume_speech_alone_and_after_a_loud_phrase() {
        let quiet: Vec<f32> = tone(SAMPLE_RATE)
            .into_iter()
            .map(|sample| sample * 0.02)
            .collect();
        let quiet_output = trim_and_compress(&quiet);
        assert_eq!(quiet_output.len(), quiet.len());
        assert!(rms(&quiet_output) > MIN_ACTIVITY_RMS);

        let mut mixed = tone(SAMPLE_RATE);
        mixed.extend(vec![0.0; SAMPLE_RATE * 3]);
        mixed.extend_from_slice(&quiet);
        let mixed_output = trim_and_compress(&mixed);
        assert!(
            mixed_output.len() < mixed.len(),
            "long digital silence is compressed"
        );
        assert!(
            mixed_output.ends_with(&quiet),
            "quiet speech after a loud phrase remains intact"
        );
    }
}
