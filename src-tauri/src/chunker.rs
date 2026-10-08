//! Small, dependency-free energy VAD and overlapping chunk planner.
use sha2::{Digest, Sha256};

const SAMPLE_RATE: usize = 16_000;
const FRAME: usize = SAMPLE_RATE * 30 / 1_000;
const HOP: usize = SAMPLE_RATE * 10 / 1_000;
const OVERLAP: usize = SAMPLE_RATE * 3 / 2;

#[derive(Debug, Clone, Copy)]
pub struct ChunkerConfig {
    pub chunk_length_secs: usize,
}
impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            chunk_length_secs: 35,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    pub index: usize,
    pub samples: Vec<f32>,
    /// Start of this exact sample window in the source timeline.
    pub source_start_sample: usize,
    pub start_secs: f32,
    pub end_secs: f32,
    /// Cryptographic identity of the exact samples submitted for ASR. Chunk
    /// indexes and timestamps describe position, but they are not sufficient
    /// to reuse a transcript after final silence compression changes coverage.
    pub identity: AudioChunkIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioChunkIdentity {
    source_start_sample: usize,
    sample_count: usize,
    sha256: [u8; 32],
}

impl AudioChunkIdentity {
    #[cfg(test)]
    pub fn from_samples(samples: &[f32]) -> Self {
        Self::from_samples_at(0, samples)
    }

    pub fn from_samples_at(source_start_sample: usize, samples: &[f32]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update((source_start_sample as u64).to_le_bytes());
        hasher.update((samples.len() as u64).to_le_bytes());
        for sample in samples {
            hasher.update(sample.to_bits().to_le_bytes());
        }
        Self {
            source_start_sample,
            sample_count: samples.len(),
            sha256: hasher.finalize().into(),
        }
    }

    pub fn sample_count(self) -> usize {
        self.sample_count
    }
}

#[derive(Debug, Clone)]
pub struct Vad {
    pub noise_floor: f32,
    pub threshold: f32,
    pub fixed_floor: f32,
}
impl Vad {
    pub fn new() -> Self {
        Self {
            noise_floor: 0.0,
            threshold: 0.01,
            fixed_floor: 0.01,
        }
    }
    pub fn calibrate(&mut self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
        self.noise_floor = if self.noise_floor == 0.0 {
            rms
        } else {
            self.noise_floor * 0.8 + rms * 0.2
        };
        self.threshold = (self.noise_floor * 2.5).max(self.fixed_floor);
    }
    pub fn is_speech(&self, samples: &[f32]) -> bool {
        rms(samples) >= self.threshold
    }
}

pub struct Chunker {
    config: ChunkerConfig,
    samples: Vec<f32>,
    next_index: usize,
    vad: Vad,
    calibration_samples: Vec<f32>,
    calibrated: bool,
    /// Absolute sample index of `samples[0]` (number of samples consumed so
    /// far, excluding the current buffer). Advanced by `keep` on every take.
    total_offset: usize,
}
impl Chunker {
    pub fn new(config: ChunkerConfig) -> Self {
        Self {
            config: ChunkerConfig {
                chunk_length_secs: config.chunk_length_secs.clamp(15, 60),
            },
            samples: Vec::new(),
            next_index: 0,
            vad: Vad::new(),
            calibration_samples: Vec::with_capacity(SAMPLE_RATE / 2),
            calibrated: false,
            total_offset: 0,
        }
    }
    pub fn push(&mut self, input: &[f32]) -> Vec<AudioChunk> {
        if !self.calibrated {
            let remaining = SAMPLE_RATE / 2 - self.calibration_samples.len();
            self.calibration_samples
                .extend_from_slice(&input[..input.len().min(remaining)]);
            if self.calibration_samples.len() == SAMPLE_RATE / 2 {
                self.vad.calibrate(&self.calibration_samples);
                self.calibrated = true;
                self.calibration_samples.clear();
            }
        }
        self.samples.extend_from_slice(input);
        let mut out = Vec::new();
        let min = (self.config.chunk_length_secs.saturating_sub(5)) * SAMPLE_RATE;
        let max = (self.config.chunk_length_secs + 5) * SAMPLE_RATE;
        while self.samples.len() >= max {
            // Wait until the complete search window is available. Cutting at
            // the nominal target made results depend on how the audio device
            // happened to divide callbacks around that threshold.
            let cut = nearest_silence(&self.samples, min, max, &self.vad);
            out.push(self.take(cut));
        }
        out
    }
    pub fn finish(&mut self) -> Option<AudioChunk> {
        if self.samples.is_empty() {
            None
        } else {
            Some(self.take(self.samples.len()))
        }
    }
    fn take(&mut self, end: usize) -> AudioChunk {
        let end = end.min(self.samples.len());
        let keep = end.saturating_sub(OVERLAP);
        let chunk_samples = self.samples[..end].to_vec();
        // `total_offset` is the absolute sample index of `samples[0]`.
        let start = self.total_offset;
        let chunk = AudioChunk {
            index: self.next_index,
            samples: chunk_samples,
            source_start_sample: start,
            start_secs: start as f32 / SAMPLE_RATE as f32,
            end_secs: (start + end) as f32 / SAMPLE_RATE as f32,
            identity: AudioChunkIdentity::from_samples_at(start, &self.samples[..end]),
        };
        self.samples = self.samples[keep..].to_vec();
        self.total_offset += keep;
        self.next_index += 1;
        chunk
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }
}
fn nearest_silence(samples: &[f32], start: usize, end: usize, vad: &Vad) -> usize {
    let start = start.max(HOP);
    let end = end.min(samples.len());
    let target = start + (end.saturating_sub(start) / 2);
    let mut best: Option<(usize, usize)> = None;
    for at in (start..=end).step_by(HOP) {
        let a = at.saturating_sub(FRAME);
        if !vad.is_speech(&samples[a..at]) {
            let distance = at.abs_diff(target);
            if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, at));
            }
        }
    }
    best.map_or(target, |(_, at)| at)
}

pub fn encode_wav(samples: &[f32]) -> Result<Vec<u8>, String> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut bytes, spec).map_err(|e| e.to_string())?;
    for sample in samples {
        writer
            .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(bytes.into_inner())
}

#[cfg(test)]
fn merge_transcripts(chunks: Vec<(usize, String)>) -> String {
    merge_transcripts_with_windows(
        chunks
            .into_iter()
            .map(|(index, text)| (index, text, 0.0, 0.0))
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimedWord {
    pub text: String,
    /// Provider-local time within the exact chunk submitted for transcription.
    pub start_secs: Option<f32>,
    pub end_secs: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimedTranscriptChunk {
    pub index: usize,
    pub text: String,
    /// Absolute source-audio window covered by this transcript.
    pub source_start_secs: f32,
    pub source_end_secs: f32,
    pub words: Vec<TimedWord>,
}

/// Remove a repeated prefix only when provider word timings map unambiguously
/// to both text strings and identify the same words at the same point in the
/// actual shared audio window. Missing, malformed, or differently-aligned
/// timings retain repeated text because duplication is safer than deletion.
pub fn merge_transcripts_with_timing(mut chunks: Vec<TimedTranscriptChunk>) -> String {
    chunks.sort_by_key(|chunk| chunk.index);
    let mut result = String::new();
    let mut previous: Option<TimedTranscriptChunk> = None;

    for chunk in chunks
        .into_iter()
        .filter(|chunk| !chunk.text.trim().is_empty())
    {
        if result.is_empty() {
            result.push_str(&chunk.text);
            previous = Some(chunk);
            continue;
        }

        let duplicate_prefix = previous
            .as_ref()
            .filter(|prev| chunk.index == prev.index.saturating_add(1))
            .and_then(|prev| matching_timed_prefix_len(prev, &chunk));
        let remainder = duplicate_prefix
            .and_then(|word_count| text_prefix_end(&chunk.text, word_count))
            .map(|end| &chunk.text[end..])
            .unwrap_or(&chunk.text);
        result = append_remainder(&result, remainder);
        previous = Some(chunk);
    }
    result
}

/// Window timestamps alone cannot identify where speech occurred inside each
/// window, so repeated text is retained when word timings are unavailable.
#[cfg(test)]
fn merge_transcripts_with_windows(mut chunks: Vec<(usize, String, f32, f32)>) -> String {
    chunks.sort_by_key(|chunk| chunk.0);
    let mut result = String::new();
    for (_, text, _start, _end) in chunks {
        if !text.trim().is_empty() {
            if result.is_empty() {
                result.push_str(&text);
            } else {
                result = append_remainder(&result, &text);
            }
        }
    }
    result
}

/// This legacy helper has no positional metadata. Keep repeated wording rather
/// than estimating how many words fit into an overlap duration.
#[cfg(test)]
fn merge_transcripts_with_overlap_secs(
    mut chunks: Vec<(usize, String)>,
    _overlap_secs: f32,
) -> String {
    chunks.sort_by_key(|chunk| chunk.0);
    let mut result = String::new();
    for (_, text) in chunks {
        if !text.trim().is_empty() {
            if result.is_empty() {
                result.push_str(&text);
            } else {
                result = append_remainder(&result, &text);
            }
        }
    }
    result
}

#[derive(Debug)]
struct AlignedWord {
    normalized: String,
    text_token_count: usize,
    source_interval_secs: Option<(f32, f32)>,
}

fn matching_timed_prefix_len(
    left: &TimedTranscriptChunk,
    right: &TimedTranscriptChunk,
) -> Option<usize> {
    let overlap_start = left.source_start_secs.max(right.source_start_secs);
    let overlap_end = left.source_end_secs.min(right.source_end_secs);
    if !overlap_start.is_finite()
        || !overlap_end.is_finite()
        || overlap_end <= overlap_start
        || left.text.contains(['\n', '\r'])
        || right.text.contains(['\n', '\r'])
    {
        return None;
    }

    let left_words = aligned_words(left)?;
    let right_words = aligned_words(right)?;
    let max_words = left_words.len().min(right_words.len());
    if max_words < 2 {
        return None;
    }

    for count in (2..=max_words).rev() {
        let left_suffix = &left_words[left_words.len() - count..];
        let right_prefix = &right_words[..count];
        let same_audio = left_suffix.iter().zip(right_prefix).all(|(lhs, rhs)| {
            if lhs.normalized != rhs.normalized {
                return false;
            }
            let (Some((left_start, left_end)), Some((right_start, right_end))) =
                (lhs.source_interval_secs, rhs.source_interval_secs)
            else {
                return false;
            };
            left_start.max(right_start) < overlap_end
                && left_end.min(right_end) > overlap_start
                && (left_start - right_start).abs() <= 0.35
                && (left_end - right_end).abs() <= 0.35
        });
        if same_audio {
            return Some(right_prefix.iter().map(|word| word.text_token_count).sum());
        }
    }
    None
}

fn aligned_words(chunk: &TimedTranscriptChunk) -> Option<Vec<AlignedWord>> {
    if !chunk.source_start_secs.is_finite()
        || !chunk.source_end_secs.is_finite()
        || chunk.source_start_secs < 0.0
        || chunk.source_end_secs <= chunk.source_start_secs
    {
        return None;
    }
    let duration = chunk.source_end_secs - chunk.source_start_secs;
    let text_tokens: Vec<String> = chunk
        .text
        .split_whitespace()
        .filter_map(|token| {
            let normalized = normalize_overlap_token(token);
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect();
    if text_tokens.len() < 2 {
        return None;
    }

    let mut aligned = Vec::with_capacity(chunk.words.len());
    let mut previous_interval: Option<(f32, f32)> = None;
    let mut text_index: usize = 0;
    for timing in &chunk.words {
        let timing_tokens: Vec<String> = timing
            .text
            .split_whitespace()
            .map(normalize_overlap_token)
            .filter(|token| !token.is_empty())
            .collect();
        let text_end = text_index.checked_add(timing_tokens.len())?;
        if timing_tokens.is_empty()
            || text_tokens.get(text_index..text_end)? != timing_tokens.as_slice()
        {
            return None;
        }
        let local_interval = timing
            .start_secs
            .zip(timing.end_secs)
            .filter(|(start, end)| {
                start.is_finite() && end.is_finite() && *start >= 0.0 && *end >= *start
            })
            .filter(|(start, end)| {
                previous_interval.is_none_or(|(previous_start, previous_end)| {
                    *start >= previous_start && *end >= previous_end
                })
            });
        if let Some(interval) = local_interval {
            previous_interval = Some(interval);
        }
        let source_interval_secs = local_interval
            .filter(|(_, end)| *end <= duration + 0.01)
            .map(|(start, end)| {
                (
                    chunk.source_start_secs + start,
                    chunk.source_start_secs + end,
                )
            })
            .filter(|(start, end)| start.is_finite() && end.is_finite());
        aligned.push(AlignedWord {
            normalized: timing_tokens.join(" "),
            text_token_count: timing_tokens.len(),
            source_interval_secs,
        });
        text_index = text_end;
    }
    if text_index != text_tokens.len() {
        return None;
    }
    Some(aligned)
}

fn text_prefix_end(text: &str, word_count: usize) -> Option<usize> {
    text.split_whitespace()
        .nth(word_count.saturating_sub(1))
        .map(|word| {
            let start = word.as_ptr() as usize - text.as_ptr() as usize;
            start + word.len()
        })
}

fn normalize_overlap_token(token: &str) -> String {
    token
        .trim_matches(|ch: char| {
            ch.is_ascii_punctuation() || "，。！？；：、（）【】《》“”‘’".contains(ch)
        })
        .chars()
        .flat_map(char::to_lowercase)
        .collect()
}

fn append_remainder(left: &str, remainder: &str) -> String {
    if remainder.is_empty() {
        return left.to_owned();
    }
    let Some(first) = remainder.chars().find(|ch| !ch.is_whitespace()) else {
        return format!("{left}{remainder}");
    };
    let Some(last) = left.chars().last() else {
        return left.to_owned();
    };
    let separator = match (last, remainder.chars().next()) {
        (_, Some(ch)) if ch.is_whitespace() => "",
        (ch, _) if ch.is_whitespace() => "",
        (_, _) if is_punctuation(first) => "",
        (ch, _) if is_cjk(ch) && is_cjk(first) => "",
        (ch, _) if is_punctuation(ch) && is_cjk(first) => "",
        _ => " ",
    };
    format!("{left}{separator}{remainder}")
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{2ffff}'
    )
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || matches!(
            ch,
            '，' | '。' | '！' | '？' | '；' | '：' | '、' | '）' | '】' | '》'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed_chunk(
        index: usize,
        text: &str,
        source_start_secs: f32,
        source_end_secs: f32,
        words: &[(&str, f32, f32)],
    ) -> TimedTranscriptChunk {
        TimedTranscriptChunk {
            index,
            text: text.to_owned(),
            source_start_secs,
            source_end_secs,
            words: words
                .iter()
                .map(|(text, start_secs, end_secs)| TimedWord {
                    text: (*text).to_owned(),
                    start_secs: Some(*start_secs),
                    end_secs: Some(*end_secs),
                })
                .collect(),
        }
    }

    #[test]
    fn adaptive_threshold() {
        let mut vad = Vad::new();
        vad.calibrate(&vec![0.004; 8_000]);
        assert!(vad.threshold >= 0.01);
        assert!(vad.is_speech(&[0.02; 480]));
    }
    #[test]
    fn overlap_and_elastic_boundary() {
        let mut c = Chunker::new(ChunkerConfig::default());
        let mut chunks = Vec::new();
        chunks.extend(c.push(&vec![0.0; 30 * SAMPLE_RATE]));
        chunks.extend(c.push(&vec![0.0; 10 * SAMPLE_RATE]));
        assert!(!chunks.is_empty());
        assert!(chunks[0].samples.len() >= 25 * SAMPLE_RATE);
        assert!(chunks[0].samples.len() <= 35 * SAMPLE_RATE);
        if chunks.len() > 1 {
            assert_eq!(
                chunks[0].samples[chunks[0].samples.len() - OVERLAP..],
                chunks[1].samples[..OVERLAP]
            );
        }
    }

    #[test]
    fn chunking_is_invariant_to_input_push_size() {
        let mut audio = vec![0.001; SAMPLE_RATE / 2];
        for _ in 0..8 {
            audio.extend((0..6 * SAMPLE_RATE).map(|index| if index % 2 == 0 { 0.2 } else { -0.2 }));
            audio.extend(vec![0.0; SAMPLE_RATE]);
        }

        let chunk_with_size = |push_size: usize| {
            let mut chunker = Chunker::new(ChunkerConfig {
                chunk_length_secs: 15,
            });
            let mut chunks = Vec::new();
            for part in audio.chunks(push_size) {
                chunks.extend(chunker.push(part));
            }
            chunks.extend(chunker.finish());
            chunks
        };

        let baseline = chunk_with_size(audio.len());
        assert!(baseline.len() >= 3);
        for push_size in [137, 1_600, 4_096, SAMPLE_RATE] {
            assert_eq!(
                chunk_with_size(push_size),
                baseline,
                "push size {push_size}"
            );
        }
    }

    #[test]
    fn sample_identity_covers_exact_float_bits_and_length() {
        let original = [0.0, 0.25, -0.5];
        let same = [0.0, 0.25, -0.5];
        let changed_zero = [-0.0, 0.25, -0.5];
        assert_eq!(
            AudioChunkIdentity::from_samples(&original),
            AudioChunkIdentity::from_samples(&same)
        );
        assert_ne!(
            AudioChunkIdentity::from_samples(&original),
            AudioChunkIdentity::from_samples(&changed_zero)
        );
        assert_ne!(
            AudioChunkIdentity::from_samples_at(0, &original),
            AudioChunkIdentity::from_samples_at(3, &same)
        );
        assert_eq!(
            AudioChunkIdentity::from_samples(&original).sample_count(),
            3
        );
        assert_ne!(
            AudioChunkIdentity::from_samples(&original),
            AudioChunkIdentity::from_samples(&original[..2])
        );
    }
    #[test]
    fn timed_overlap_deduplicates_only_words_at_the_same_source_time() {
        assert_eq!(
            merge_transcripts_with_timing(vec![
                timed_chunk(
                    0,
                    "We are ready.",
                    0.0,
                    30.0,
                    &[("We", 5.0, 5.2), ("are", 5.25, 5.4), ("ready", 5.5, 5.9)],
                ),
                timed_chunk(
                    1,
                    "We are ready.",
                    29.5,
                    60.0,
                    &[
                        ("We", 15.5, 15.7),
                        ("are", 15.75, 15.9),
                        ("ready", 16.0, 16.4)
                    ],
                ),
            ]),
            "We are ready. We are ready.",
            "repeated wording in shared silence is a second utterance"
        );

        assert_eq!(
            merge_transcripts_with_timing(vec![
                timed_chunk(
                    0,
                    "we are ready",
                    0.0,
                    30.0,
                    &[
                        ("we", 29.5, 29.6),
                        ("are", 29.7, 29.8),
                        ("ready", 29.9, 30.0)
                    ],
                ),
                timed_chunk(
                    1,
                    "we are ready to go",
                    29.5,
                    60.0,
                    &[
                        ("we", 0.0, 0.1),
                        ("are", 0.2, 0.3),
                        ("ready", 0.4, 0.5),
                        ("to", 0.65, 0.75),
                        ("go", 0.8, 0.9),
                    ],
                ),
            ]),
            "we are ready to go",
            "only timestamp-aligned speech in the actual overlap is deduplicated"
        );
    }

    #[test]
    fn captured_multi_token_timing_keeps_boundary_alignment_conservative() {
        // Captured Groq metadata groups `of $1,250` into one timed word entry,
        // while the transcript contains two whitespace tokens. Keep that
        // provider timing as one group; do not invent sub-word timestamps.
        let left = timed_chunk(
            0,
            "The budget of $1,250. The fourth task is to keep",
            0.0,
            35.0,
            &[
                ("The", 25.0, 25.3),
                ("budget", 25.3, 26.0),
                ("of $1,250", 26.82, 27.68),
                ("The", 33.36, 33.62),
                ("fourth", 33.62, 33.92),
                ("task", 33.92, 34.32),
                ("is", 34.32, 34.54),
                ("to", 34.54, 34.74),
                ("keep", 34.74, 34.96),
            ],
        );
        let right = timed_chunk(
            1,
            "The fourth task is to keep recording.",
            33.5,
            69.888_31,
            &[
                ("The", 0.0, 0.14),
                ("fourth", 0.14, 0.4),
                ("task", 0.4, 0.82),
                ("is", 0.82, 1.02),
                ("to", 1.02, 1.2),
                ("keep", 1.2, 1.46),
                ("recording.", 35.52, 36.6),
            ],
        );
        assert_eq!(
            merge_transcripts_with_timing(vec![left.clone(), right.clone()]),
            "The budget of $1,250. The fourth task is to keep recording."
        );

        let intentional_repeat = merge_transcripts_with_timing(vec![
            timed_chunk(
                0,
                "We are ready",
                0.0,
                35.0,
                &[("We", 5.0, 5.2), ("are", 5.25, 5.4), ("ready", 5.5, 5.9)],
            ),
            timed_chunk(
                1,
                "We are ready again",
                33.5,
                60.0,
                &[
                    ("We", 15.5, 15.7),
                    ("are", 15.75, 15.9),
                    ("ready", 16.0, 16.4),
                    ("again", 16.5, 16.9),
                ],
            ),
        ]);
        assert_eq!(intentional_repeat, "We are ready We are ready again");

        let mut bad_timing = right.clone();
        bad_timing.words[2].start_secs = None;
        assert_eq!(
            merge_transcripts_with_timing(vec![left.clone(), bad_timing]),
            "The budget of $1,250. The fourth task is to keep The fourth task is to keep recording."
        );

        assert_eq!(
            merge_transcripts_with_timing(vec![
                timed_chunk(0, "重复这句话", 0.0, 35.0, &[]),
                timed_chunk(1, "重复这句话", 33.5, 69.888_31, &[]),
            ]),
            "重复这句话重复这句话"
        );
    }

    #[test]
    fn missing_or_untrustworthy_word_positions_keep_repeated_text() {
        assert_eq!(
            merge_transcripts_with_windows(vec![
                (0, "We are ready.".into(), 0.0, 30.0),
                (1, "We are ready.".into(), 29.5, 60.0),
            ],),
            "We are ready. We are ready.",
            "overlapping audio windows alone do not reveal where speech occurred"
        );

        let mut malformed = timed_chunk(
            0,
            "hello there",
            0.0,
            30.0,
            &[("hello", 28.0, 28.2), ("there", 28.3, 28.5)],
        );
        malformed.words[1].start_secs = Some(-1.0);
        assert_eq!(
            merge_transcripts_with_timing(vec![
                malformed,
                timed_chunk(
                    1,
                    "hello there again",
                    29.5,
                    60.0,
                    &[
                        ("hello", 0.0, 0.2),
                        ("there", 0.3, 0.5),
                        ("again", 0.6, 0.8)
                    ],
                ),
            ]),
            "hello there hello there again"
        );
    }

    #[test]
    fn chinese_without_word_alignment_keeps_repeated_referents() {
        assert_eq!(
            merge_transcripts_with_overlap_secs(
                vec![
                    (0, "这是一个测试内容".into()),
                    (1, "一个测试内容继续".into())
                ],
                1.0,
            ),
            "这是一个测试内容一个测试内容继续",
            "without aligned word timing, overlapping character strings remain"
        );
        assert_eq!(
            merge_transcripts(vec![(0, "你好".into()), (1, "世界".into())]),
            "你好世界"
        );
        assert_eq!(
            merge_transcripts_with_overlap_secs(
                vec![(0, "这是一个测试".into()), (1, "测试内容".into())],
                1.5,
            ),
            "这是一个测试测试内容",
            "a short repeated referent stays when overlap is ambiguous"
        );
    }

    #[test]
    fn merge_skips_failed_chunks_without_placeholder_text() {
        assert_eq!(
            merge_transcripts(vec![(0, "hello world".into()), (2, "and done".into())]),
            "hello world and done"
        );
        let merged = merge_transcripts(vec![(0, "你好".into()), (1, "世界".into())]);
        assert!(!merged.contains("识别失败"));
        assert!(!merged.contains('['));
    }

    #[test]
    fn merge_keeps_mixed_script_boundaries_readable() {
        assert_eq!(
            merge_transcripts(vec![(0, "部署到".into()), (1, "v2 --dry-run".into())]),
            "部署到 v2 --dry-run"
        );
        assert_eq!(
            merge_transcripts_with_overlap_secs(
                vec![(0, "hello 世界项目".into()), (1, "世界项目 today".into())],
                1.0,
            ),
            "hello 世界项目世界项目 today"
        );
    }

    #[test]
    fn window_only_and_multiline_merges_preserve_repeated_layout() {
        assert_eq!(
            merge_transcripts_with_windows(vec![
                (0, "we are deploying".into(), 0.0, 35.0),
                (
                    1,
                    "we are deploying\n  --dry-run\n  --fast".into(),
                    33.5,
                    50.0
                ),
            ]),
            "we are deploying we are deploying\n  --dry-run\n  --fast"
        );
        assert_eq!(
            merge_transcripts_with_windows(vec![
                (0, "repeat this phrase".into(), 0.0, 35.0),
                (1, "repeat this phrase again".into(), 35.0, 50.0),
            ]),
            "repeat this phrase repeat this phrase again",
            "chunks without actual audio overlap retain repeated speech"
        );
        assert_eq!(
            merge_transcripts_with_overlap_secs(
                vec![
                    (0, "first line\nsecond line".into()),
                    (1, "first line\nsecond line\n  code".into())
                ],
                1.5,
            ),
            "first line\nsecond line first line\nsecond line\n  code",
            "multiline repeated text is left intact"
        );
    }

    #[test]
    fn timestamps_are_monotonic_and_non_drifting() {
        let mut c = Chunker::new(ChunkerConfig {
            chunk_length_secs: 15,
        });
        let mut chunks = Vec::new();
        // Push ~60s of "speech" (constant loud signal) in 1s blocks.
        for _ in 0..60 {
            chunks.extend(c.push(&vec![0.5; SAMPLE_RATE]));
        }
        if let Some(last) = c.finish() {
            chunks.push(last);
        }
        assert!(chunks.len() >= 2, "expected multiple chunks");
        let mut prev_end = 0.0_f32;
        for chunk in &chunks {
            assert!(chunk.start_secs < chunk.end_secs);
            // Each chunk must start no earlier than the previous chunk started,
            // and must overlap the previous chunk's end (no gaps).
            assert!(
                chunk.start_secs <= prev_end.max(chunk.start_secs),
                "chunk {} start {} regressed",
                chunk.index,
                chunk.start_secs
            );
            assert!(
                chunk.start_secs <= prev_end,
                "gap detected before chunk {} (start {} > prev_end {})",
                chunk.index,
                chunk.start_secs,
                prev_end
            );
            prev_end = chunk.end_secs;
        }
    }
}
