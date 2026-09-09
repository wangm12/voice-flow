//! Accurate-ASR cascade gate, timed Accurate shot, and winner selection.
#![allow(dead_code)]

use crate::asr::{AsrOptions, AsrProvider};
use std::time::Duration;

pub struct CascadeInput {
    pub accurate_asr_configured: bool,
    pub primary_failed: bool,
    pub low_confidence: bool,
    pub hallucination_hit: bool,
    pub mixed_cjk_english: bool,
    pub proper_noun_count: usize,
    pub noun_threshold: usize,
}

pub fn should_run_accurate(input: &CascadeInput) -> bool {
    if !input.accurate_asr_configured {
        return false;
    }
    if input.primary_failed {
        return true;
    }
    input.low_confidence
        || input.hallucination_hit
        || input.mixed_cjk_english
        || input.proper_noun_count >= input.noun_threshold
}

pub fn is_mixed_cjk_english(text: &str) -> bool {
    let has_cjk = text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    let has_latin = text.chars().any(|c| c.is_ascii_alphabetic());
    has_cjk && has_latin
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeWinner {
    Primary,
    Accurate,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CascadeTimeout;

fn usable_text(text: Option<&str>) -> bool {
    text.is_some_and(|value| !value.trim().is_empty())
}

pub fn pick_winner(
    primary: Option<&str>,
    accurate: Result<Option<String>, CascadeTimeout>,
) -> CascadeWinner {
    let accurate_ok = matches!(accurate, Ok(Some(ref text)) if !text.trim().is_empty());
    if accurate_ok {
        return CascadeWinner::Accurate;
    }
    if usable_text(primary) {
        CascadeWinner::Primary
    } else {
        CascadeWinner::None
    }
}

pub fn winning_text<'a>(
    winner: CascadeWinner,
    primary: Option<&'a str>,
    accurate: Option<&'a str>,
) -> Option<&'a str> {
    match winner {
        CascadeWinner::Primary => primary.filter(|text| !text.trim().is_empty()),
        CascadeWinner::Accurate => accurate.filter(|text| !text.trim().is_empty()),
        CascadeWinner::None => None,
    }
}

pub fn cleanup_winner_once<T>(
    primary: Option<&str>,
    accurate: Result<Option<String>, CascadeTimeout>,
    cleanup: impl FnOnce(&str) -> T,
) -> Option<T> {
    let accurate_text = accurate.as_ref().ok().and_then(|text| text.clone());
    let winner = pick_winner(primary, accurate);
    winning_text(winner, primary, accurate_text.as_deref()).map(cleanup)
}

pub fn hallucination_bag_hit(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if crate::spoken_revision::is_hallucination_text(trimmed) {
        return true;
    }
    trimmed
        .split_inclusive(['.', '!', '?', '。', '！', '？', '\n'])
        .any(|sentence| crate::spoken_revision::is_hallucination_text(sentence.trim()))
}

fn usable_accurate_text(text: &str) -> Option<String> {
    if text.trim().is_empty() || hallucination_bag_hit(text) {
        None
    } else {
        Some(text.to_owned())
    }
}

pub async fn accurate_shot(
    provider: &dyn AsrProvider,
    audio: Vec<u8>,
    options: AsrOptions,
    timeout: Duration,
) -> Result<Option<String>, CascadeTimeout> {
    match tokio::time::timeout(timeout, provider.transcribe_batch(audio, options)).await {
        Err(_) => Err(CascadeTimeout),
        Ok(Ok(transcript)) => Ok(usable_accurate_text(&transcript.text)),
        Ok(Err(_)) => Ok(None),
    }
}

pub fn cascade_input_for(
    configured: bool,
    primary_failed: bool,
    primary_text: Option<&str>,
    low_confidence: bool,
    proper_noun_count: usize,
    noun_threshold: usize,
) -> CascadeInput {
    CascadeInput {
        accurate_asr_configured: configured,
        primary_failed,
        low_confidence,
        hallucination_hit: primary_text.is_some_and(hallucination_bag_hit),
        mixed_cjk_english: primary_text.is_some_and(is_mixed_cjk_english),
        proper_noun_count,
        noun_threshold,
    }
}

pub async fn maybe_run_accurate<F, Fut>(
    input: &CascadeInput,
    run: F,
) -> Result<Option<String>, CascadeTimeout>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Option<String>, CascadeTimeout>>,
{
    if should_run_accurate(input) {
        run().await
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(trigger: fn(&mut CascadeInput)) -> CascadeInput {
        let mut input = CascadeInput {
            accurate_asr_configured: true,
            primary_failed: false,
            low_confidence: false,
            hallucination_hit: false,
            mixed_cjk_english: false,
            proper_noun_count: 0,
            noun_threshold: 3,
        };
        trigger(&mut input);
        input
    }

    #[test]
    fn no_accurate_configured_never_runs() {
        assert!(!should_run_accurate(&CascadeInput {
            accurate_asr_configured: false,
            primary_failed: true,
            low_confidence: true,
            hallucination_hit: true,
            mixed_cjk_english: true,
            proper_noun_count: 9,
            noun_threshold: 3,
        }));
    }

    #[test]
    fn each_trigger_fires_when_configured() {
        assert!(should_run_accurate(&configured(|input| input
            .primary_failed =
            true)));
        assert!(should_run_accurate(&configured(|input| input
            .low_confidence =
            true)));
        assert!(should_run_accurate(&configured(|input| input
            .hallucination_hit =
            true)));
        assert!(should_run_accurate(&configured(|input| input
            .mixed_cjk_english =
            true)));
        assert!(should_run_accurate(&configured(|input| input
            .proper_noun_count =
            3)));
        assert!(!should_run_accurate(&configured(|input| input
            .proper_noun_count =
            2)));
        assert!(!should_run_accurate(&configured(|_| {})));
    }

    #[test]
    fn mixed_cjk_english_detects_晓雯_and_python() {
        assert!(is_mixed_cjk_english("晓雯在写 Python"));
        assert!(!is_mixed_cjk_english("只有中文"));
        assert!(!is_mixed_cjk_english("Python only"));
    }

    #[test]
    fn pick_winner_accurate_ok_beats_primary() {
        assert_eq!(
            pick_winner(Some("primary draft"), Ok(Some("accurate".into()))),
            CascadeWinner::Accurate
        );
    }

    #[test]
    fn pick_winner_timeout_keeps_primary() {
        assert_eq!(
            pick_winner(Some("primary draft"), Err(CascadeTimeout)),
            CascadeWinner::Primary
        );
    }

    #[test]
    fn pick_winner_primary_none_accurate_ok_uses_accurate() {
        assert_eq!(
            pick_winner(None, Ok(Some("accurate only".into()))),
            CascadeWinner::Accurate
        );
    }

    #[test]
    fn pick_winner_both_none_is_none() {
        assert_eq!(pick_winner(None, Ok(None)), CascadeWinner::None);
        assert_eq!(pick_winner(None, Err(CascadeTimeout)), CascadeWinner::None);
        assert_eq!(pick_winner(Some("   "), Ok(None)), CascadeWinner::None);
    }

    #[test]
    fn pick_winner_accurate_empty_or_hallucination_keeps_primary() {
        assert_eq!(
            pick_winner(Some("primary draft"), Ok(None)),
            CascadeWinner::Primary
        );
        assert_eq!(
            pick_winner(Some("primary draft"), Ok(Some(String::new()))),
            CascadeWinner::Primary
        );
    }

    #[test]
    fn cascade_input_for_fills_mix_and_hallucination() {
        let input = cascade_input_for(
            true,
            false,
            Some("晓雯在写 Python。Thanks for watching."),
            false,
            3,
            3,
        );
        assert!(input.mixed_cjk_english);
        assert!(input.hallucination_hit);
        assert!(should_run_accurate(&input));
    }

    #[test]
    fn cleanup_runs_once_on_winner_not_the_draft() {
        let mut calls = 0usize;
        let cleaned = cleanup_winner_once(
            Some("primary draft"),
            Ok(Some("accurate winner".into())),
            |text| {
                calls += 1;
                format!("cleaned:{text}")
            },
        );
        assert_eq!(cleaned.as_deref(), Some("cleaned:accurate winner"));
        assert_eq!(calls, 1);
    }

    fn mock_transcript(text: &str) -> crate::asr::Transcript {
        crate::asr::Transcript {
            text: text.to_owned(),
            segments: Vec::new(),
            words: Vec::new(),
            limits: crate::asr::RateLimits::default(),
        }
    }

    #[tokio::test]
    async fn accurate_shot_timeout_keeps_timeout_error() {
        let provider = crate::asr::MockAsrProvider::new(
            Ok(mock_transcript("too late")),
            std::time::Duration::from_millis(80),
        );
        let result = accurate_shot(
            &provider,
            b"wav".to_vec(),
            crate::asr::AsrOptions::default(),
            std::time::Duration::from_millis(10),
        )
        .await;
        assert_eq!(result, Err(CascadeTimeout));
        assert_eq!(provider.calls(), 1);
    }

    #[tokio::test]
    async fn accurate_shot_ok_returns_text_error_or_empty_is_none() {
        let ok = crate::asr::MockAsrProvider::new(
            Ok(mock_transcript("晓雯在写 Python")),
            std::time::Duration::ZERO,
        );
        assert_eq!(
            accurate_shot(
                &ok,
                b"wav".to_vec(),
                crate::asr::AsrOptions::default(),
                std::time::Duration::from_millis(50),
            )
            .await,
            Ok(Some("晓雯在写 Python".into()))
        );

        let empty = crate::asr::MockAsrProvider::new(
            Err(crate::asr::AsrError::EmptyResult),
            std::time::Duration::ZERO,
        );
        assert_eq!(
            accurate_shot(
                &empty,
                b"wav".to_vec(),
                crate::asr::AsrOptions::default(),
                std::time::Duration::from_millis(50),
            )
            .await,
            Ok(None)
        );

        let hallucination = crate::asr::MockAsrProvider::new(
            Ok(mock_transcript("Thanks for watching the show.")),
            std::time::Duration::ZERO,
        );
        assert_eq!(
            accurate_shot(
                &hallucination,
                b"wav".to_vec(),
                crate::asr::AsrOptions::default(),
                std::time::Duration::from_millis(50),
            )
            .await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn maybe_run_accurate_skips_http_when_gate_is_closed() {
        let mut runs = 0usize;
        let skipped = maybe_run_accurate(
            &CascadeInput {
                accurate_asr_configured: false,
                primary_failed: true,
                low_confidence: true,
                hallucination_hit: true,
                mixed_cjk_english: true,
                proper_noun_count: 9,
                noun_threshold: 3,
            },
            || {
                runs += 1;
                async { Ok(Some("should not run".into())) }
            },
        )
        .await;
        assert_eq!(skipped, Ok(None));
        assert_eq!(runs, 0);

        let fired = maybe_run_accurate(&configured(|input| input.low_confidence = true), || {
            runs += 1;
            async { Ok(Some("accurate".into())) }
        })
        .await;
        assert_eq!(fired, Ok(Some("accurate".into())));
        assert_eq!(runs, 1);
    }
}
