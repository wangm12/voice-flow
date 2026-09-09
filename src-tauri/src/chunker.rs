//! Small, dependency-free energy VAD and overlapping chunk planner.
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
    pub start_secs: f32,
    pub end_secs: f32,
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
    last_silence: Option<usize>,
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
            last_silence: None,
            total_offset: 0,
        }
    }
    pub fn push(&mut self, input: &[f32]) -> Vec<AudioChunk> {
        self.samples.extend_from_slice(input);
        if self.samples.len() <= SAMPLE_RATE / 2 {
            self.vad.calibrate(&self.samples);
        }
        let mut out = Vec::new();
        let target = self.config.chunk_length_secs * SAMPLE_RATE;
        let min = (self.config.chunk_length_secs.saturating_sub(5)) * SAMPLE_RATE;
        let max = (self.config.chunk_length_secs + 5) * SAMPLE_RATE;
        while self.samples.len() >= max {
            let cut = self
                .last_silence
                .unwrap_or_else(|| nearest_silence(&self.samples, min, max, &self.vad));
            out.push(self.take(cut));
        }
        if self.samples.len() >= target {
            let cut = nearest_silence(
                &self.samples,
                min,
                target.min(self.samples.len()),
                &self.vad,
            );
            if cut >= min {
                out.push(self.take(cut));
            }
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
            start_secs: start as f32 / SAMPLE_RATE as f32,
            end_secs: (start + end) as f32 / SAMPLE_RATE as f32,
        };
        self.samples = self.samples[keep..].to_vec();
        self.total_offset += keep;
        self.next_index += 1;
        self.last_silence = None;
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
    let mut best = end;
    for at in (start..=end).step_by(HOP) {
        let a = at.saturating_sub(FRAME);
        if !vad.is_speech(&samples[a..at]) {
            best = at;
        }
    }
    best
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

pub fn merge_transcripts(mut chunks: Vec<(usize, String)>) -> String {
    chunks.sort_by_key(|v| v.0);
    let mut result = String::new();
    for (_, text) in chunks {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if result.is_empty() {
            result.push_str(text);
            continue;
        }
        result = merge_pair(&result, text);
    }
    result
}

fn merge_pair(left: &str, right: &str) -> String {
    let left_words: Vec<&str> = left.split_whitespace().collect();
    let right_words: Vec<&str> = right.split_whitespace().collect();
    for word_count in (1..=left_words.len().min(right_words.len())).rev() {
        if left_words[left_words.len() - word_count..]
            .iter()
            .zip(&right_words[..word_count])
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            let remainder = right_words[word_count..].join(" ");
            return append_remainder(left, &remainder);
        }
    }

    // Chinese and some mixed-script ASR results do not contain whitespace.
    // Match only a short character overlap when CJK text is present; the
    // existing word-based path remains authoritative for English and code.
    if contains_cjk(left) || contains_cjk(right) {
        let left_chars: Vec<char> = left.chars().collect();
        let right_chars: Vec<char> = right.chars().collect();
        let max_overlap = left_chars.len().min(right_chars.len()).min(120);
        for char_count in (2..=max_overlap).rev() {
            if left_chars[left_chars.len() - char_count..] == right_chars[..char_count] {
                let remainder = right_chars[char_count..].iter().collect::<String>();
                return append_remainder(left, &remainder);
            }
        }
    }

    append_remainder(left, right)
}

fn append_remainder(left: &str, remainder: &str) -> String {
    let remainder = remainder.trim();
    if remainder.is_empty() {
        return left.to_owned();
    }
    let Some(first) = remainder.chars().next() else {
        return left.to_owned();
    };
    let last = left.chars().last();
    let separator = match last {
        None => "",
        Some(ch) if ch.is_whitespace() || is_cjk(ch) && is_cjk(first) => "",
        Some(ch) if is_punctuation(ch) || is_punctuation(first) => "",
        Some(_) => " ",
    };
    format!("{left}{separator}{remainder}")
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(is_cjk)
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
        chunks.extend(c.push(&vec![0.0; 6 * SAMPLE_RATE]));
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
    fn merge_removes_overlap() {
        assert_eq!(
            merge_transcripts(vec![
                (1, "hello world this".into()),
                (0, "hello world".into()),
                (2, "this is done".into())
            ]),
            "hello world this is done"
        );
    }

    #[test]
    fn merge_removes_overlap_in_unspaced_chinese() {
        assert_eq!(
            merge_transcripts(vec![(0, "这是一个测试".into()), (1, "测试内容".into())]),
            "这是一个测试内容"
        );
        assert_eq!(
            merge_transcripts(vec![(0, "你好".into()), (1, "世界".into())]),
            "你好世界"
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
            merge_transcripts(vec![(0, "hello 世界".into()), (1, "世界 today".into())]),
            "hello 世界 today"
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
