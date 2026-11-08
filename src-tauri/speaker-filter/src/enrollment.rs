use alkove_core::timbre::{self, Center, Level, Timbre, Voiceprint};
use alkove_core::{Cue, SPEECH_DEFAULTS};
use alkove_engine::Model;

use crate::models::LoadedModels;
use crate::profile::{
    ConsistencyVerdict, EnrollmentQuality, ProfileFile, ProfileSource, SampleInterval,
    ENROLLMENT_MAX_SAMPLES, ENROLLMENT_MIN_SAMPLES, ENROLLMENT_MIN_THIRD_VALID_SAMPLES,
    ENROLLMENT_MIN_VALID_SAMPLES,
};
use crate::wav::{MAX_AUDIO_SAMPLES, SAMPLE_RATE_HZ};
use crate::{Error, Result};

const CUE_FRAME_SAMPLES: usize = 480;
const CUE_FLUSH_SAMPLES: usize = CUE_FRAME_SAMPLES * 2;
const CLIPPING_THRESHOLD: f32 = 0.999;
const MAX_CLIPPING_FRACTION: f64 = 0.01;
const ENROLLMENT_RECIPE: &str = "raw-cue-third-timbre-balanced-v1";

pub struct EnrollmentResult {
    pub profile: ProfileFile,
    pub valid_speech_samples: u64,
    pub clipped_samples: u64,
    pub consistency_verdicts: Vec<ConsistencyVerdict>,
}

pub fn enroll_raw_audio(
    models: &LoadedModels,
    samples: &[f32],
    sample_format: &str,
    source_sha256: &str,
) -> Result<EnrollmentResult> {
    let sample_count = samples.len() as u64;
    if !(ENROLLMENT_MIN_SAMPLES..=ENROLLMENT_MAX_SAMPLES).contains(&sample_count)
        || sample_count > MAX_AUDIO_SAMPLES as u64
    {
        return Err(Error::Input(
            "enrollment WAV must contain 15 to 30 seconds of mono raw16k audio",
        ));
    }
    if samples.iter().any(|x| !x.is_finite()) {
        return Err(Error::Input("enrollment WAV contains a non-finite sample"));
    }
    if samples.iter().all(|&x| x == 0.0) {
        return Err(Error::Input("enrollment WAV is all zeros"));
    }
    if source_sha256.len() != 64
        || !source_sha256
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(Error::Input("enrollment source checksum is invalid"));
    }
    let clipped_samples = samples
        .iter()
        .filter(|&&x| x.abs() >= CLIPPING_THRESHOLD)
        .count() as u64;
    let clipping_fraction = clipped_samples as f64 / sample_count as f64;
    if clipping_fraction > MAX_CLIPPING_FRACTION {
        return Err(Error::Input("enrollment WAV exceeds the 1% clipping limit"));
    }

    let cue_threshold = SPEECH_DEFAULTS.confidence as f32;
    let valid_intervals = cue_intervals(&models.cue, samples, 0, cue_threshold)?;
    let valid_speech_samples = union_samples(&valid_intervals)?;
    if valid_speech_samples < ENROLLMENT_MIN_VALID_SAMPLES {
        return Err(Error::Input(
            "enrollment requires at least 10 seconds of raw Cue-accepted speech",
        ));
    }

    let thirds = [
        (0, samples.len() / 3),
        (samples.len() / 3, samples.len() * 2 / 3),
        (samples.len() * 2 / 3, samples.len()),
    ];
    let mut third_valid_speech_samples = [0_u64; 3];
    let mut third_centers: Vec<Center> = Vec::with_capacity(3);
    let mut timbre = Timbre::new(&models.timbre)
        .map_err(|_| Error::Model("Timbre model could not be initialized"))?;
    for (index, &(start, end)) in thirds.iter().enumerate() {
        let third = &samples[start..end];
        let intervals = cue_intervals(&models.cue, third, start as u64, cue_threshold)?;
        let count = union_samples(&intervals)?;
        if count < ENROLLMENT_MIN_THIRD_VALID_SAMPLES {
            return Err(Error::Input(
                "each enrollment third requires at least 1.5 seconds of raw Cue-accepted speech",
            ));
        }
        third_valid_speech_samples[index] = count;
        let print = timbre
            .embed(third, "raw")
            .map_err(|_| Error::Input("Timbre could not embed an enrollment third"))?;
        third_centers.push(
            print
                .centers
                .into_iter()
                .next()
                .ok_or(Error::Model("Timbre returned no enrollment center"))?,
        );
    }

    let consistency_verdicts = compare_thirds(&third_centers)?;
    if consistency_verdicts.iter().any(|verdict| !verdict.same) {
        return Err(Error::Input(
            "enrollment temporal speaker-consistency check failed",
        ));
    }

    let overall = timbre
        .embed(samples, "raw")
        .map_err(|_| Error::Input("Timbre could not embed the enrollment WAV"))?;
    let center = overall
        .centers
        .into_iter()
        .next()
        .ok_or(Error::Model("Timbre returned no enrollment center"))?;
    let source = ProfileSource {
        kind: "raw16k".into(),
        container: "wav".into(),
        sample_rate_hz: SAMPLE_RATE_HZ,
        channels: 1,
        sample_count,
        duration_seconds: samples.len() as f64 / SAMPLE_RATE_HZ as f64,
        sample_format: sample_format.into(),
        source_sha256: source_sha256.to_string(),
        enrollment_quality: EnrollmentQuality {
            recipe: ENROLLMENT_RECIPE.into(),
            cue_confidence_threshold: cue_threshold,
            cue_frame_samples: CUE_FRAME_SAMPLES as u16,
            minimum_valid_speech_samples: ENROLLMENT_MIN_VALID_SAMPLES,
            valid_speech_samples,
            valid_intervals,
            clipped_samples,
            clipping_threshold: CLIPPING_THRESHOLD,
            maximum_clipping_fraction: MAX_CLIPPING_FRACTION,
            clipping_fraction,
            third_valid_speech_samples,
            consistency_verdicts: consistency_verdicts.clone(),
            consistency_is_authentication: false,
        },
    };
    let profile = ProfileFile::from_center(source, models, &center)?;
    Ok(EnrollmentResult {
        profile,
        valid_speech_samples,
        clipped_samples,
        consistency_verdicts,
    })
}

fn cue_intervals(
    model: &std::sync::Arc<Model>,
    samples: &[f32],
    source_offset: u64,
    confidence: f32,
) -> Result<Vec<SampleInterval>> {
    let mut cue =
        Cue::new(model).map_err(|_| Error::Model("Cue model could not be initialized"))?;
    let mut intervals = Vec::new();
    let mut invalid_probability = false;
    let mut observe = |probability: f32, frame: i64| {
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            invalid_probability = true;
            return;
        }
        if probability < confidence || frame < 0 {
            return;
        }
        let local_start = (frame as u64).saturating_mul(CUE_FRAME_SAMPLES as u64);
        if local_start >= samples.len() as u64 {
            return;
        }
        let start = source_offset.saturating_add(local_start);
        let end = source_offset
            .saturating_add((local_start + CUE_FRAME_SAMPLES as u64).min(samples.len() as u64));
        if start < end {
            intervals.push(SampleInterval {
                start_sample: start,
                end_sample: end,
            });
        }
    };
    for chunk in samples.chunks(1024) {
        cue.push(chunk, &mut observe);
    }
    // Cue emits one frame late; the padded second frame releases the final valid source frame.
    cue.push(&[0.0; CUE_FLUSH_SAMPLES], &mut observe);
    if invalid_probability {
        return Err(Error::Model("Cue produced an invalid probability"));
    }
    Ok(union_intervals(intervals))
}

fn union_intervals(mut intervals: Vec<SampleInterval>) -> Vec<SampleInterval> {
    intervals.sort_by_key(|range| (range.start_sample, range.end_sample));
    let mut union: Vec<SampleInterval> = Vec::with_capacity(intervals.len());
    for interval in intervals {
        if let Some(last) = union.last_mut() {
            if interval.start_sample <= last.end_sample {
                last.end_sample = last.end_sample.max(interval.end_sample);
                continue;
            }
        }
        union.push(interval);
    }
    union
}

fn union_samples(intervals: &[SampleInterval]) -> Result<u64> {
    intervals.iter().try_fold(0_u64, |sum, interval| {
        sum.checked_add(interval.end_sample - interval.start_sample)
            .ok_or(Error::Input("valid-speech sample count overflowed"))
    })
}

fn compare_thirds(centers: &[Center]) -> Result<Vec<ConsistencyVerdict>> {
    if centers.len() != 3 {
        return Err(Error::Model("Timbre did not return three temporal centers"));
    }
    let mut verdicts = Vec::with_capacity(3);
    for (left, right) in [(0_usize, 1_usize), (0, 2), (1, 2)] {
        let print = Voiceprint::new(vec![centers[left].clone()], "raw");
        let heard = [centers[right].clone()];
        let verdict = timbre::verify(&print, &heard, Level::Balanced);
        let cutoff = timbre::threshold(
            Level::Balanced,
            centers[right].seconds,
            centers[left].seconds,
        );
        verdicts.push(ConsistencyVerdict {
            left_third: left as u8,
            right_third: right as u8,
            score: verdict.score,
            threshold: cutoff,
            same: verdict.same,
        });
    }
    Ok(verdicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_union_counts_each_source_sample_once() {
        let intervals = union_intervals(vec![
            SampleInterval {
                start_sample: 480,
                end_sample: 960,
            },
            SampleInterval {
                start_sample: 0,
                end_sample: 480,
            },
            SampleInterval {
                start_sample: 240,
                end_sample: 720,
            },
            SampleInterval {
                start_sample: 960,
                end_sample: 1_100,
            },
        ]);
        assert_eq!(intervals.len(), 1);
        assert_eq!(intervals[0].start_sample, 0);
        assert_eq!(intervals[0].end_sample, 1_100);
        assert_eq!(union_samples(&intervals).unwrap(), 1_100);
    }
}
