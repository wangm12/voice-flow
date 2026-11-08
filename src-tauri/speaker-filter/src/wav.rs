use std::io::Read;
use std::path::Path;

use hound::{SampleFormat, WavReader};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const MAX_AUDIO_SAMPLES: usize = 16_000 * 60 * 15;
const MAX_WAV_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WavAudio {
    pub samples: Vec<f32>,
    pub sample_format: &'static str,
    pub source_sha256: String,
}

pub fn read_raw16k_mono(path: &Path) -> Result<WavAudio> {
    let metadata = std::fs::metadata(path).map_err(|_| Error::Input("input WAV is unavailable"))?;
    if metadata.len() > MAX_WAV_BYTES {
        return Err(Error::Input("input WAV exceeds the 15-minute size limit"));
    }
    let file =
        std::fs::File::open(path).map_err(|_| Error::Input("input file could not be read"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_WAV_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Input("input file could not be read"))?;
    if bytes.len() as u64 > MAX_WAV_BYTES {
        return Err(Error::Input("input WAV exceeds the 15-minute size limit"));
    }
    let source_sha256 = hex::encode(Sha256::digest(&bytes));
    let mut reader = WavReader::new(std::io::Cursor::new(bytes.as_slice()))
        .map_err(|_| Error::Input("input file is not a readable WAV"))?;
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE_HZ || spec.channels != 1 {
        return Err(Error::Input("input WAV must be mono 16 kHz"));
    }
    if reader.duration() == 0 || reader.duration() as usize > MAX_AUDIO_SAMPLES {
        return Err(Error::Input(
            "input WAV duration is invalid or exceeds the 15-minute limit",
        ));
    }

    let (sample_format, samples) = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) => {
            let mut out = Vec::with_capacity(reader.duration() as usize);
            for sample in reader.samples::<i16>() {
                let value = sample.map_err(|_| Error::Input("input WAV sample data is invalid"))?
                    as f32
                    / 32768.0;
                require_finite(value)?;
                out.push(value);
                if out.len() > MAX_AUDIO_SAMPLES {
                    return Err(Error::Input("input WAV exceeds the 15-minute sample limit"));
                }
            }
            ("pcm16", out)
        }
        (SampleFormat::Int, 24 | 32) => {
            let denom = 2_f64.powi(spec.bits_per_sample as i32 - 1) as f32;
            let mut out = Vec::with_capacity(reader.duration() as usize);
            for sample in reader.samples::<i32>() {
                let value = sample.map_err(|_| Error::Input("input WAV sample data is invalid"))?
                    as f32
                    / denom;
                require_finite(value)?;
                out.push(value);
                if out.len() > MAX_AUDIO_SAMPLES {
                    return Err(Error::Input("input WAV exceeds the 15-minute sample limit"));
                }
            }
            (
                if spec.bits_per_sample == 24 {
                    "pcm24"
                } else {
                    "pcm32"
                },
                out,
            )
        }
        (SampleFormat::Float, 32) => {
            let mut out = Vec::with_capacity(reader.duration() as usize);
            for sample in reader.samples::<f32>() {
                let value = sample.map_err(|_| Error::Input("input WAV sample data is invalid"))?;
                require_finite(value)?;
                out.push(value);
                if out.len() > MAX_AUDIO_SAMPLES {
                    return Err(Error::Input("input WAV exceeds the 15-minute sample limit"));
                }
            }
            ("float32", out)
        }
        _ => {
            return Err(Error::Input(
                "input WAV encoding must be PCM16/24/32 or float32",
            ))
        }
    };
    if samples.is_empty()
        || samples.len() > MAX_AUDIO_SAMPLES
        || samples.len() != reader.duration() as usize
    {
        return Err(Error::Input("input WAV sample count is invalid"));
    }
    Ok(WavAudio {
        samples,
        sample_format,
        source_sha256,
    })
}

pub fn write_raw16k_mono(path: &Path, samples: &[f32]) -> Result<()> {
    use crate::fileutil::{create_private_temp, enforce_private_mode, install_temp};
    use hound::{WavSpec, WavWriter};

    if samples.is_empty()
        || samples.len() > MAX_AUDIO_SAMPLES
        || samples.iter().any(|x| !x.is_finite())
    {
        return Err(Error::Output("output audio is invalid"));
    }
    let (temp_path, file) = create_private_temp(path)?;
    let result = (|| {
        let spec = WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE_HZ,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer =
            WavWriter::new(file, spec).map_err(|_| Error::Output("could not create output WAV"))?;
        for &sample in samples {
            writer
                .write_sample(sample)
                .map_err(|_| Error::Output("could not write output WAV"))?;
        }
        writer
            .finalize()
            .map_err(|_| Error::Output("could not finalize output WAV"))?;
        enforce_private_mode(&temp_path)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temp_path)
            .map_err(|_| Error::Output("output WAV could not be reopened"))?;
        file.sync_all()
            .map_err(|_| Error::Output("output WAV could not be synced"))?;
        install_temp(&temp_path, path, true)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn require_finite(value: f32) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(Error::Input("input WAV contains a non-finite sample"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "voiceflow-speaker-filter-{name}-{}-{}.wav",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn private_float_wav_round_trip_keeps_the_exact_sample_count() {
        let path = temp_path("roundtrip");
        let samples: Vec<f32> = (0..1_025)
            .map(|i| (i as f32 * 0.013).sin() * 0.25)
            .collect();
        write_raw16k_mono(&path, &samples).unwrap();
        let decoded = read_raw16k_mono(&path).unwrap();
        assert_eq!(decoded.samples.len(), samples.len());
        assert_eq!(decoded.samples, samples);
        assert_eq!(decoded.sample_format, "float32");
        assert_eq!(decoded.source_sha256.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn writer_rejects_non_finite_output_without_creating_a_file() {
        let path = temp_path("non-finite");
        assert!(write_raw16k_mono(&path, &[f32::NAN]).is_err());
        assert!(!path.exists());
    }
}
