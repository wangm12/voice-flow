use std::io::Read;
use std::path::Path;

use alkove_core::timbre::{self, Level};
use alkove_core::timbre::{Center, DIM};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fileutil::{enforce_private_mode, write_private_atomic};
use crate::models::LoadedModels;
use crate::{Error, Result};

pub const PROFILE_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROFILE_BYTES: usize = 512 * 1024;
pub const ENROLLMENT_MIN_SAMPLES: u64 = 15 * 16_000;
pub const ENROLLMENT_MAX_SAMPLES: u64 = 30 * 16_000;
pub const ENROLLMENT_MIN_VALID_SAMPLES: u64 = 10 * 16_000;
pub const ENROLLMENT_MIN_THIRD_VALID_SAMPLES: u64 = 15_000 * 16 / 10; // 1.5 seconds

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleInterval {
    pub start_sample: u64,
    pub end_sample: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsistencyVerdict {
    pub left_third: u8,
    pub right_third: u8,
    pub score: f64,
    pub threshold: f64,
    pub same: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentQuality {
    pub recipe: String,
    pub cue_confidence_threshold: f32,
    pub cue_frame_samples: u16,
    pub minimum_valid_speech_samples: u64,
    pub valid_speech_samples: u64,
    pub valid_intervals: Vec<SampleInterval>,
    pub clipped_samples: u64,
    pub clipping_threshold: f32,
    pub maximum_clipping_fraction: f64,
    pub clipping_fraction: f64,
    pub third_valid_speech_samples: [u64; 3],
    pub consistency_verdicts: Vec<ConsistencyVerdict>,
    pub consistency_is_authentication: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSource {
    pub kind: String,
    pub container: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub sample_count: u64,
    pub duration_seconds: f64,
    pub sample_format: String,
    pub source_sha256: String,
    pub enrollment_quality: EnrollmentQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileModels {
    pub focus_model_id: String,
    pub cue_model_id: String,
    pub timbre_model_id: String,
    pub models_digest_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePayload {
    pub schema_version: u32,
    pub source: ProfileSource,
    pub models: ProfileModels,
    // Focus consumes this embedding. The Timbre id is retained separately for verification.
    pub focus_embedding: Vec<f32>,
    pub verification_id: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    pub schema_version: u32,
    pub source: ProfileSource,
    pub models: ProfileModels,
    pub focus_embedding: Vec<f32>,
    pub verification_id: Vec<f32>,
    pub checksum_sha256: String,
}

impl ProfileFile {
    pub fn from_center(
        source: ProfileSource,
        models: &LoadedModels,
        center: &Center,
    ) -> Result<Self> {
        let focus_embedding = normalized_vector(&center.embedding)?;
        let verification_id = normalized_vector(&center.id)?;
        let mut profile = Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            source,
            models: ProfileModels {
                focus_model_id: models.focus_id.clone(),
                cue_model_id: models.cue_id.clone(),
                timbre_model_id: models.timbre_id.clone(),
                models_digest_sha256: models.digest_sha256.clone(),
            },
            focus_embedding,
            verification_id,
            checksum_sha256: String::new(),
        };
        profile.validate(models)?;
        profile.checksum_sha256 = profile.compute_checksum()?;
        Ok(profile)
    }

    pub fn validate(&self, models: &LoadedModels) -> Result<()> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(Error::Profile("unsupported profile schema"));
        }
        if self.source.kind != "raw16k"
            || self.source.container != "wav"
            || self.source.sample_rate_hz != 16_000
            || self.source.channels != 1
        {
            return Err(Error::Profile("profile source must be mono raw16k WAV"));
        }
        if self.source.sample_count == 0
            || !self.source.duration_seconds.is_finite()
            || self.source.duration_seconds <= 0.0
        {
            return Err(Error::Profile("profile source metadata is invalid"));
        }
        if !(ENROLLMENT_MIN_SAMPLES..=ENROLLMENT_MAX_SAMPLES).contains(&self.source.sample_count) {
            return Err(Error::Profile("enrollment source must be 15 to 30 seconds"));
        }
        let duration = self.source.sample_count as f64 / 16_000.0;
        if (self.source.duration_seconds - duration).abs() > 1e-9 {
            return Err(Error::Profile(
                "profile duration metadata does not match its sample count",
            ));
        }
        if !matches!(
            self.source.sample_format.as_str(),
            "pcm16" | "pcm24" | "pcm32" | "float32"
        ) {
            return Err(Error::Profile("profile source sample format is invalid"));
        }
        let source_sha = &self.source.source_sha256;
        if source_sha.len() != 64
            || !source_sha
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(Error::Profile("profile source checksum is invalid"));
        }
        validate_enrollment_quality(&self.source.enrollment_quality, self.source.sample_count)?;
        models.require_profile_ids(
            &self.models.focus_model_id,
            &self.models.cue_model_id,
            &self.models.timbre_model_id,
            &self.models.models_digest_sha256,
        )?;
        validate_unit_vector(&self.focus_embedding)?;
        validate_unit_vector(&self.verification_id)?;
        Ok(())
    }

    pub fn payload(&self) -> ProfilePayload {
        ProfilePayload {
            schema_version: self.schema_version,
            source: self.source.clone(),
            models: self.models.clone(),
            focus_embedding: self.focus_embedding.clone(),
            verification_id: self.verification_id.clone(),
        }
    }

    pub fn compute_checksum(&self) -> Result<String> {
        let bytes = serde_json::to_vec(&self.payload())
            .map_err(|_| Error::Profile("profile could not be encoded"))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn serialize(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|_| Error::Profile("profile could not be encoded"))
    }

    pub fn read(path: &Path, models: &LoadedModels) -> Result<(Self, String)> {
        let metadata =
            std::fs::metadata(path).map_err(|_| Error::Profile("profile file is unavailable"))?;
        if metadata.len() == 0 || metadata.len() as usize > MAX_PROFILE_BYTES {
            return Err(Error::Profile("profile file size is invalid"));
        }
        let file = std::fs::File::open(path)
            .map_err(|_| Error::Profile("profile file could not be read"))?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_PROFILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Profile("profile file could not be read"))?;
        Self::decode(&bytes, models)
    }

    pub fn decode(bytes: &[u8], models: &LoadedModels) -> Result<(Self, String)> {
        if bytes.is_empty() || bytes.len() > MAX_PROFILE_BYTES {
            return Err(Error::Profile("profile file size is invalid"));
        }
        let profile: Self =
            serde_json::from_slice(bytes).map_err(|_| Error::Profile("profile file is invalid"))?;
        profile.validate(models)?;
        if profile.checksum_sha256.len() != 64
            || profile.compute_checksum()? != profile.checksum_sha256
        {
            return Err(Error::Profile("profile checksum is invalid"));
        }
        let file_sha = hex::encode(Sha256::digest(bytes));
        Ok((profile, file_sha))
    }

    pub fn write_new(&self, path: &Path) -> Result<String> {
        let bytes = self.serialize()?;
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(Error::Profile("profile file size is invalid"));
        }
        write_private_atomic(path, &bytes, false)?;
        enforce_private_mode(path)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

pub fn validate_enrollment_quality(quality: &EnrollmentQuality, source_samples: u64) -> Result<()> {
    if quality.recipe != "raw-cue-third-timbre-balanced-v1"
        || quality.cue_confidence_threshold.to_bits() != 0.5_f32.to_bits()
        || quality.cue_frame_samples != 480
        || quality.minimum_valid_speech_samples != ENROLLMENT_MIN_VALID_SAMPLES
        || quality.clipping_threshold.to_bits() != 0.999_f32.to_bits()
        || quality.maximum_clipping_fraction.to_bits() != 0.01_f64.to_bits()
        || quality.consistency_is_authentication
    {
        return Err(Error::Profile(
            "profile enrollment-quality recipe is unsupported",
        ));
    }
    if quality.valid_speech_samples < ENROLLMENT_MIN_VALID_SAMPLES
        || quality.valid_speech_samples > source_samples
        || quality
            .third_valid_speech_samples
            .iter()
            .any(|&n| n < ENROLLMENT_MIN_THIRD_VALID_SAMPLES)
    {
        return Err(Error::Profile(
            "profile does not meet the enrollment speech-duration gate",
        ));
    }
    let third_bounds = [
        (0, source_samples / 3),
        (source_samples / 3, source_samples * 2 / 3),
        (source_samples * 2 / 3, source_samples),
    ];
    for (index, &(start, end)) in third_bounds.iter().enumerate() {
        if quality.third_valid_speech_samples[index] > end - start {
            return Err(Error::Profile(
                "profile third-segment speech count exceeds its source interval",
            ));
        }
    }
    if quality.clipped_samples > source_samples
        || !quality.clipping_fraction.is_finite()
        || quality.clipping_fraction < 0.0
    {
        return Err(Error::Profile("profile clipping metadata is invalid"));
    }
    let expected_clip_fraction = quality.clipped_samples as f64 / source_samples as f64;
    if (quality.clipping_fraction - expected_clip_fraction).abs() > 1e-12
        || quality.clipping_fraction > quality.maximum_clipping_fraction
    {
        return Err(Error::Profile("profile exceeds the clipping gate"));
    }
    let mut previous_end = 0_u64;
    let mut interval_samples = 0_u64;
    for (index, interval) in quality.valid_intervals.iter().enumerate() {
        if interval.start_sample >= interval.end_sample
            || interval.end_sample > source_samples
            || interval.start_sample % 480 != 0
            || (interval.end_sample % 480 != 0 && interval.end_sample != source_samples)
            || (index > 0 && interval.start_sample <= previous_end)
        {
            return Err(Error::Profile("profile valid-speech intervals are invalid"));
        }
        interval_samples = interval_samples
            .checked_add(interval.end_sample - interval.start_sample)
            .ok_or(Error::Profile("profile valid-speech duration overflowed"))?;
        previous_end = interval.end_sample;
    }
    if interval_samples != quality.valid_speech_samples {
        return Err(Error::Profile(
            "profile valid-speech duration does not match its intervals",
        ));
    }
    if quality.consistency_verdicts.len() != 3 {
        return Err(Error::Profile(
            "profile temporal consistency results are incomplete",
        ));
    }
    let expected_pairs = [(0, 1), (0, 2), (1, 2)];
    for (verdict, &(left, right)) in quality.consistency_verdicts.iter().zip(&expected_pairs) {
        let (left_start, left_end) = third_bounds[left];
        let (right_start, right_end) = third_bounds[right];
        let left_secs = (left_end - left_start) as f64 / 16_000.0;
        let right_secs = (right_end - right_start) as f64 / 16_000.0;
        let expected_threshold = timbre::threshold(Level::Balanced, right_secs, left_secs);
        if verdict.left_third != left as u8
            || verdict.right_third != right as u8
            || !verdict.score.is_finite()
            || !(-1.01..=1.01).contains(&verdict.score)
            || !verdict.threshold.is_finite()
            || verdict.threshold.to_bits() != expected_threshold.to_bits()
            || verdict.same != (verdict.score >= verdict.threshold)
            || !verdict.same
        {
            return Err(Error::Profile(
                "profile temporal speaker-consistency gate failed",
            ));
        }
    }
    Ok(())
}

pub fn normalized_vector(vector: &[f32]) -> Result<Vec<f32>> {
    if vector.len() != DIM {
        return Err(Error::Profile(
            "Timbre vector must contain exactly 192 values",
        ));
    }
    if vector.iter().any(|x| !x.is_finite()) {
        return Err(Error::Profile("Timbre vector contains a non-finite value"));
    }
    let squared_norm = vector
        .iter()
        .fold(0.0_f64, |sum, &x| sum + (x as f64) * (x as f64));
    let norm = squared_norm.sqrt();
    if !norm.is_finite() || norm <= 1e-12 {
        return Err(Error::Profile(
            "Timbre vector has a zero or invalid L2 norm",
        ));
    }
    let normalized: Vec<f32> = vector.iter().map(|&x| (x as f64 / norm) as f32).collect();
    validate_unit_vector(&normalized)?;
    Ok(normalized)
}

fn validate_unit_vector(vector: &[f32]) -> Result<()> {
    if vector.len() != DIM {
        return Err(Error::Profile(
            "profile vector must contain exactly 192 values",
        ));
    }
    if vector.iter().any(|x| !x.is_finite()) {
        return Err(Error::Profile("profile vector contains a non-finite value"));
    }
    let squared_norm = vector
        .iter()
        .fold(0.0_f64, |sum, &x| sum + (x as f64) * (x as f64));
    let norm = squared_norm.sqrt();
    if !norm.is_finite() || !(0.99..=1.01).contains(&norm) {
        return Err(Error::Profile(
            "profile vector must have a finite, nonzero, normalized L2 norm",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_requires_exact_dimension_and_finite_nonzero_values() {
        assert!(normalized_vector(&[0.0; 191]).is_err());
        assert!(normalized_vector(&[f32::NAN; DIM]).is_err());
        assert!(normalized_vector(&[0.0; DIM]).is_err());
        let mut input = vec![3.0_f32; DIM];
        let unit = normalized_vector(&input).unwrap();
        validate_unit_vector(&unit).unwrap();
        input[0] = f32::INFINITY;
        assert!(normalized_vector(&input).is_err());
    }

    #[test]
    fn checksum_is_independent_of_checksum_field() {
        let consistency_threshold = timbre::threshold(Level::Balanced, 5.0, 5.0);
        let source = ProfileSource {
            kind: "raw16k".into(),
            container: "wav".into(),
            sample_rate_hz: 16_000,
            channels: 1,
            sample_count: 240_000,
            duration_seconds: 15.0,
            sample_format: "pcm16".into(),
            source_sha256: "0".repeat(64),
            enrollment_quality: EnrollmentQuality {
                recipe: "raw-cue-third-timbre-balanced-v1".into(),
                cue_confidence_threshold: 0.5,
                cue_frame_samples: 480,
                minimum_valid_speech_samples: ENROLLMENT_MIN_VALID_SAMPLES,
                valid_speech_samples: ENROLLMENT_MIN_VALID_SAMPLES,
                valid_intervals: vec![SampleInterval {
                    start_sample: 0,
                    end_sample: ENROLLMENT_MIN_VALID_SAMPLES,
                }],
                clipped_samples: 0,
                clipping_threshold: 0.999,
                maximum_clipping_fraction: 0.01,
                clipping_fraction: 0.0,
                third_valid_speech_samples: [ENROLLMENT_MIN_THIRD_VALID_SAMPLES; 3],
                consistency_verdicts: vec![
                    ConsistencyVerdict {
                        left_third: 0,
                        right_third: 1,
                        score: 0.5,
                        threshold: consistency_threshold,
                        same: true,
                    },
                    ConsistencyVerdict {
                        left_third: 0,
                        right_third: 2,
                        score: 0.5,
                        threshold: consistency_threshold,
                        same: true,
                    },
                    ConsistencyVerdict {
                        left_third: 1,
                        right_third: 2,
                        score: 0.5,
                        threshold: consistency_threshold,
                        same: true,
                    },
                ],
                consistency_is_authentication: false,
            },
        };
        let file = ProfileFile {
            schema_version: 1,
            source,
            models: ProfileModels {
                focus_model_id: "focus".into(),
                cue_model_id: "cue".into(),
                timbre_model_id: "timbre".into(),
                models_digest_sha256: "d".into(),
            },
            focus_embedding: normalized_vector(&vec![1.0; DIM]).unwrap(),
            verification_id: normalized_vector(&vec![2.0; DIM]).unwrap(),
            checksum_sha256: String::new(),
        };
        let checksum = file.compute_checksum().unwrap();
        let serialized = file.serialize().unwrap();
        let decoded: ProfileFile = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(decoded.compute_checksum().unwrap(), checksum);
        assert_ne!(serialized.len(), 0);
    }
}
