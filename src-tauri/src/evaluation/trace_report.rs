//! Bounded offline consumer for content-free dictation terminal traces.
use super::artifacts::AtomicArtifact;
use super::quantile::linear_quantile;
use super::{runner, validation};
use crate::metrics::{
    CacheStatus, DeliveryOutcome, DictationClass, ReportedScope, ReportedStageDuration, Stage,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_TRACE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_TRACE_LINE_BYTES: usize = 64 * 1024;
const MAX_TRACE_ROWS: usize = 100_000;
const MAX_FACTS_PER_ROW: usize = 256;
const MAX_CONTEXT_BYTES: u64 = 1024 * 1024;
const MAX_CONTEXT_ROWS: usize = 100_000;
const MAX_REPORT_BYTES: usize = 64 * 1024 * 1024;
const SLOW_SHORT_THRESHOLD_MS: u64 = 8_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedTrace {
    timestamp_ms: u64,
    session_generation: u64,
    class: DictationClass,
    outcome: DeliveryOutcome,
    total_ms: u64,
    stages: Vec<(Stage, u64, u64)>,
    reported_stages: Vec<ReportedStageDuration>,
    #[serde(default)]
    cache_status: Option<CacheStatus>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Branch {
    Short,
    Long,
    Unknown,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ContextSource {
    Synthetic,
    CapturedObserved,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShortContext {
    trace_sha256: String,
    source: ContextSource,
    terminals: Vec<ShortContextRow>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShortContextRow {
    line: usize,
    branch: Branch,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BranchEvidence {
    Short,
    Long,
    Unknown,
    NotProvided,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceSource {
    Synthetic,
    CapturedObserved,
    NotProvided,
}
#[derive(Debug, Clone, Serialize)]
struct HostInterval {
    start_ms: u64,
    duration_ms: u64,
}
#[derive(Debug, Clone, Serialize)]
struct StageObservation {
    stage: Stage,
    intervals: Vec<HostInterval>,
    union_duration_ms: u64,
}
#[derive(Debug, Clone, Serialize)]
struct TerminalRow {
    source_line: usize,
    timestamp_ms: u64,
    session_generation: u64,
    class: DictationClass,
    outcome: DeliveryOutcome,
    total_ms: u64,
    cache_status: Option<CacheStatus>,
    branch_evidence: BranchEvidence,
    evidence_source: EvidenceSource,
    host_stages: Vec<StageObservation>,
    host_covered_ms: u64,
    unattributed_ms: u64,
    reported_stages: Vec<ReportedStageDuration>,
}
#[derive(Debug, Clone, Serialize)]
struct QuantileSummary {
    sample_count: usize,
    p50_ms: Option<f64>,
    p95_ms: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
struct StageSummary {
    stage: Stage,
    per_terminal_stage_union_ms: QuantileSummary,
}
#[derive(Debug, Clone, Serialize)]
struct ReportedSummary {
    stage: Stage,
    scope: ReportedScope,
    fact_count: usize,
    duration_ms: QuantileSummary,
}
#[derive(Debug, Clone, Serialize)]
struct SlowShortRow {
    source_line: usize,
    total_ms: u64,
    outcome: DeliveryOutcome,
    class: DictationClass,
    host_stages: Vec<StageObservation>,
    host_covered_ms: u64,
    unattributed_ms: u64,
}
#[derive(Debug, Clone, Serialize)]
struct UnknownSlowRow {
    source_line: usize,
    total_ms: u64,
    outcome: DeliveryOutcome,
    branch_evidence: BranchEvidence,
}
#[derive(Debug, Serialize)]
struct TraceReport {
    schema_version: u32,
    evidence_domain: &'static str,
    input_sha256: String,
    input_bytes: usize,
    input_status: &'static str,
    terminal_count: usize,
    stage_sampling_unit: &'static str,
    stage_summaries_are_descriptive_not_exclusive: bool,
    terminals: Vec<TerminalRow>,
    host_stage_summaries: Vec<StageSummary>,
    child_reported_summaries: Vec<ReportedSummary>,
    slow_short_threshold_ms_exclusive: u64,
    slow_short_terminals: Vec<SlowShortRow>,
    slow_unknown_membership: Vec<UnknownSlowRow>,
}

fn input_error(
    path: &Path,
    code: &'static str,
    detail: impl Into<String>,
) -> validation::InputError {
    validation::error(path, code, detail)
}
fn stages() -> [Stage; 13] {
    [
        Stage::AsrInference,
        Stage::FileValidation,
        Stage::RuntimeStartup,
        Stage::ModelLoad,
        Stage::PromptPreparation,
        Stage::QueueWait,
        Stage::MlxGateWait,
        Stage::Prefill,
        Stage::Decode,
        Stage::FinalGuard,
        Stage::Paste,
        Stage::TargetConfirmation,
        Stage::LmStudioStatusProbe,
    ]
}
fn union_duration(intervals: &[(u64, u64)]) -> u64 {
    let mut spans = intervals.to_vec();
    spans.sort_unstable();
    let Some((mut start, first_duration)) = spans.first().copied() else {
        return 0;
    };
    let mut end = start + first_duration;
    let mut total = 0u64;
    for (next_start, duration) in spans.into_iter().skip(1) {
        let next_end = next_start + duration;
        if next_start > end {
            total += end - start;
            start = next_start;
            end = next_end;
        } else {
            end = end.max(next_end);
        }
    }
    total + (end - start)
}
fn quantiles(values: &[f64]) -> QuantileSummary {
    let sample_count = values
        .iter()
        .filter(|v| v.is_finite() && **v >= 0.0)
        .count();
    QuantileSummary {
        sample_count,
        p50_ms: linear_quantile(values, 0.5),
        p95_ms: linear_quantile(values, 0.95),
    }
}

fn read_rows(
    path: &Path,
    bytes: &[u8],
) -> Result<Vec<(usize, PersistedTrace)>, validation::InputError> {
    let mut rows = Vec::new();
    if bytes.is_empty() {
        return Ok(rows);
    }
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        if index - start > MAX_TRACE_LINE_BYTES {
            return Err(input_error(
                path,
                "trace_line_budget",
                format!("trace line {} exceeds 65536 bytes", rows.len() + 1),
            ));
        }
        let line = bytes[start..index]
            .strip_suffix(b"\r")
            .unwrap_or(&bytes[start..index]);
        if line.is_empty() {
            return Err(input_error(
                path,
                "trace_schema",
                format!("empty trace line {}", rows.len() + 1),
            ));
        }
        push_row(path, line, rows.len() + 1, &mut rows)?;
        start = index + 1;
    }
    if start < bytes.len() {
        let line_bytes = &bytes[start..];
        if line_bytes.len() > MAX_TRACE_LINE_BYTES {
            return Err(input_error(
                path,
                "trace_line_budget",
                format!("trace line {} exceeds 65536 bytes", rows.len() + 1),
            ));
        }
        let line = line_bytes.strip_suffix(b"\r").unwrap_or(line_bytes);
        if line.is_empty() {
            return Err(input_error(
                path,
                "trace_schema",
                format!("empty trace line {}", rows.len() + 1),
            ));
        }
        push_row(path, line, rows.len() + 1, &mut rows)?;
    }
    Ok(rows)
}
fn push_row(
    path: &Path,
    bytes: &[u8],
    line: usize,
    rows: &mut Vec<(usize, PersistedTrace)>,
) -> Result<(), validation::InputError> {
    if rows.len() >= MAX_TRACE_ROWS {
        return Err(input_error(
            path,
            "trace_row_budget",
            "trace input exceeds 100000 terminal rows",
        ));
    }
    let row: PersistedTrace = serde_json::from_slice(bytes).map_err(|_| {
        input_error(
            path,
            "trace_schema",
            format!("invalid or unknown trace fields at input line {line}"),
        )
    })?;
    if row.stages.len().saturating_add(row.reported_stages.len()) > MAX_FACTS_PER_ROW {
        return Err(input_error(
            path,
            "trace_fact_budget",
            format!("too many facts at input line {line}"),
        ));
    }
    for (_, start, duration) in &row.stages {
        let end = start.checked_add(*duration).ok_or_else(|| {
            input_error(
                path,
                "trace_interval",
                format!("host interval overflow at input line {line}"),
            )
        })?;
        if end > row.total_ms {
            return Err(input_error(
                path,
                "trace_interval",
                format!("host interval exceeds terminal total at input line {line}"),
            ));
        }
    }
    rows.push((line, row));
    Ok(())
}
fn read_context(
    path: &Path,
    trace_sha256: &str,
    row_lines: &BTreeSet<usize>,
) -> Result<(ContextSource, BTreeMap<usize, Branch>), validation::InputError> {
    let bytes = validation::read_bounded(path, MAX_CONTEXT_BYTES)?;
    let context: ShortContext = validation::decode_json(path, &bytes)?;
    if context.trace_sha256 != trace_sha256 || !validation::digest(&context.trace_sha256) {
        return Err(input_error(
            path,
            "trace_context_binding",
            "short-branch context does not match trace input SHA-256",
        ));
    }
    if context.terminals.len() > MAX_CONTEXT_ROWS {
        return Err(input_error(
            path,
            "trace_context_budget",
            "short-branch context exceeds 100000 rows",
        ));
    }
    let mut branches = BTreeMap::new();
    for item in context.terminals {
        if !row_lines.contains(&item.line) || branches.insert(item.line, item.branch).is_some() {
            return Err(input_error(
                path,
                "trace_context_binding",
                "duplicate or out-of-range short-branch line binding",
            ));
        }
    }
    Ok((context.source, branches))
}

fn terminal_row(
    line: usize,
    row: PersistedTrace,
    branch: Option<Branch>,
    source: Option<ContextSource>,
) -> TerminalRow {
    let mut intervals_by_stage: BTreeMap<Stage, Vec<(u64, u64)>> = BTreeMap::new();
    let mut all_intervals = Vec::with_capacity(row.stages.len());
    for (stage, start, duration) in row.stages {
        intervals_by_stage
            .entry(stage)
            .or_default()
            .push((start, duration));
        all_intervals.push((start, duration));
    }
    let host_stages = stages()
        .into_iter()
        .filter_map(|stage| {
            intervals_by_stage
                .remove(&stage)
                .map(|intervals| StageObservation {
                    stage,
                    union_duration_ms: union_duration(&intervals),
                    intervals: intervals
                        .into_iter()
                        .map(|(start_ms, duration_ms)| HostInterval {
                            start_ms,
                            duration_ms,
                        })
                        .collect(),
                })
        })
        .collect();
    let host_covered_ms = union_duration(&all_intervals);
    TerminalRow {
        source_line: line,
        timestamp_ms: row.timestamp_ms,
        session_generation: row.session_generation,
        class: row.class,
        outcome: row.outcome,
        total_ms: row.total_ms,
        cache_status: row.cache_status,
        branch_evidence: match branch {
            Some(Branch::Short) => BranchEvidence::Short,
            Some(Branch::Long) => BranchEvidence::Long,
            Some(Branch::Unknown) => BranchEvidence::Unknown,
            None => BranchEvidence::NotProvided,
        },
        evidence_source: match source {
            Some(ContextSource::Synthetic) => EvidenceSource::Synthetic,
            Some(ContextSource::CapturedObserved) => EvidenceSource::CapturedObserved,
            None => EvidenceSource::NotProvided,
        },
        host_stages,
        host_covered_ms,
        unattributed_ms: row.total_ms - host_covered_ms,
        reported_stages: row.reported_stages,
    }
}

fn build_report(
    input_path: &Path,
    context_path: Option<&Path>,
    rows: Vec<(usize, PersistedTrace)>,
    input_bytes: &[u8],
) -> Result<TraceReport, validation::InputError> {
    let input_sha256 = validation::sha256(input_bytes);
    let row_lines: BTreeSet<usize> = rows.iter().map(|(line, _)| *line).collect();
    let context = context_path
        .map(|path| read_context(path, &input_sha256, &row_lines))
        .transpose()?;
    let context_source = context.as_ref().map(|(source, _)| *source);
    let context_map = context.map(|(_, map)| map).unwrap_or_default();
    let terminals: Vec<_> = rows
        .into_iter()
        .map(|(line, row)| {
            let branch = context_map.get(&line).copied();
            if branch == Some(Branch::Short) && row.class == DictationClass::Long {
                return Err(input_error(
                    input_path,
                    "trace_context_binding",
                    format!("long-class terminal cannot be marked short at input line {line}"),
                ));
            }
            if branch == Some(Branch::Short) && row.outcome == DeliveryOutcome::PreviewOnly {
                return Err(input_error(
                    input_path,
                    "trace_context_binding",
                    format!("preview-only terminal cannot be marked short at input line {line}"),
                ));
            }
            Ok(terminal_row(line, row, branch, context_source))
        })
        .collect::<Result<_, _>>()?;

    let mut stage_samples: BTreeMap<Stage, Vec<f64>> = stages()
        .into_iter()
        .map(|stage| (stage, Vec::new()))
        .collect();
    let mut reported_samples: Vec<((Stage, ReportedScope), Vec<f64>)> = Vec::new();
    for terminal in &terminals {
        for observation in &terminal.host_stages {
            stage_samples
                .entry(observation.stage)
                .or_default()
                .push(observation.union_duration_ms as f64);
        }
        for fact in &terminal.reported_stages {
            if let Some((_, values)) = reported_samples
                .iter_mut()
                .find(|((stage, scope), _)| *stage == fact.stage && *scope == fact.scope)
            {
                values.push(fact.duration_ms as f64);
            } else {
                reported_samples.push(((fact.stage, fact.scope), vec![fact.duration_ms as f64]));
            }
        }
    }
    let host_stage_summaries = stages()
        .into_iter()
        .map(|stage| StageSummary {
            stage,
            per_terminal_stage_union_ms: quantiles(
                stage_samples
                    .get(&stage)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            ),
        })
        .collect();
    let child_reported_summaries = reported_samples
        .into_iter()
        .map(|((stage, scope), values)| ReportedSummary {
            stage,
            scope,
            fact_count: values.len(),
            duration_ms: quantiles(&values),
        })
        .collect();
    let slow_short_terminals = terminals
        .iter()
        .filter(|row| {
            row.branch_evidence == BranchEvidence::Short && row.total_ms > SLOW_SHORT_THRESHOLD_MS
        })
        .map(|row| SlowShortRow {
            source_line: row.source_line,
            total_ms: row.total_ms,
            outcome: row.outcome,
            class: row.class,
            host_stages: row.host_stages.clone(),
            host_covered_ms: row.host_covered_ms,
            unattributed_ms: row.unattributed_ms,
        })
        .collect();
    let slow_unknown_membership = terminals
        .iter()
        .filter(|row| {
            matches!(
                row.branch_evidence,
                BranchEvidence::Unknown | BranchEvidence::NotProvided
            ) && row.total_ms > SLOW_SHORT_THRESHOLD_MS
        })
        .map(|row| UnknownSlowRow {
            source_line: row.source_line,
            total_ms: row.total_ms,
            outcome: row.outcome,
            branch_evidence: row.branch_evidence,
        })
        .collect();
    Ok(TraceReport {
        schema_version: 1,
        evidence_domain: "explicit_content_free_dictation_trace_metadata; not_audio_evaluation_or_app_performance_proof",
        input_sha256,
        input_bytes: input_bytes.len(),
        input_status: if terminals.is_empty() { "empty_no_observed_terminals" } else { "observed_terminal_rows" },
        terminal_count: terminals.len(),
        stage_sampling_unit: "one_host_interval_union_sample_per_terminal_that_observed_that_stage; missing_stage_is_not_zero",
        stage_summaries_are_descriptive_not_exclusive: true,
        terminals,
        host_stage_summaries,
        child_reported_summaries,
        slow_short_threshold_ms_exclusive: SLOW_SHORT_THRESHOLD_MS,
        slow_short_terminals,
        slow_unknown_membership,
    })
}

pub(super) fn run(
    input: &Path,
    context: Option<&Path>,
    output: &Path,
) -> Result<i32, validation::InputError> {
    let input_path = validation::external(input)?;
    let input_bytes = validation::read_bounded(&input_path, MAX_TRACE_BYTES)?;
    let rows = read_rows(&input_path, &input_bytes)?;
    let context_path = context.map(validation::external).transpose()?;
    if context_path.as_deref() == Some(input_path.as_path()) {
        return Err(input_error(
            output,
            "conflicting_paths",
            "trace and context inputs must be distinct files",
        ));
    }
    let output_path = runner::output_path(output)?;
    if output_path == input_path || context_path.as_deref() == Some(output_path.as_path()) {
        return Err(input_error(
            output,
            "conflicting_paths",
            "trace report must use a distinct new external path",
        ));
    }
    let report = build_report(&input_path, context_path.as_deref(), rows, &input_bytes)?;
    let encoded = serde_json::to_vec_pretty(&report)
        .map_err(|_| input_error(output, "trace_report", "trace report serialization failed"))?;
    if encoded.len().saturating_add(1) > MAX_REPORT_BYTES {
        return Err(input_error(
            output,
            "trace_report_budget",
            "trace report exceeds 64 MiB",
        ));
    }
    let mut artifact = AtomicArtifact::new(&output_path).map_err(|_| {
        input_error(
            output,
            "trace_report_write",
            "trace report output could not be safely initialized",
        )
    })?;
    artifact.write_json(&report).map_err(|_| {
        input_error(
            output,
            "trace_report_write",
            "trace report could not be safely installed",
        )
    })?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let root = std::env::temp_dir();
            loop {
                let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
                let path = root.join(format!(
                    "voiceflow-task25-trace-test-{}-{sequence}",
                    std::process::id()
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create owned test directory: {error}"),
                }
            }
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn host_interval_union_handles_overlap_and_gaps() {
        assert_eq!(union_duration(&[(0, 50), (25, 50), (100, 10)]), 85);
        assert_eq!(union_duration(&[(0, 10), (10, 0)]), 10);
        assert_eq!(union_duration(&[]), 0);
    }

    #[tokio::test]
    async fn slow_short_sentence_attributed_by_stage_through_actual_cli_report_writer() {
        let directory = TestDirectory::new();
        let input_path = directory.0.join("dictation-performance.jsonl");
        let context_path = directory.0.join("branches.json");
        let output_path = directory.0.join("report.json");
        let traces = [
            serde_json::json!({"timestamp_ms":1,"session_generation":55,"class":"unclassified","outcome":"failed","total_ms":9000,"stages":[["asr_inference",0,5000],["asr_inference",4000,3000],["decode",6000,1000],["lm_studio_status_probe",0,1000]],"reported_stages":[{"stage":"prefill","duration_ms":90,"scope":"full_prompt"},{"stage":"decode","duration_ms":40,"scope":"generation"}],"cache_status":"unknown"}),
            serde_json::json!({"timestamp_ms":2,"session_generation":55,"class":"unclassified","outcome":"paste_unconfirmed","total_ms":8000,"stages":[],"reported_stages":[]}),
            serde_json::json!({"timestamp_ms":3,"session_generation":55,"class":"long","outcome":"paste_confirmed","total_ms":9001,"stages":[["model_load",0,500]],"reported_stages":[]}),
            serde_json::json!({"timestamp_ms":4,"session_generation":56,"class":"unclassified","outcome":"cancelled","total_ms":8001,"stages":[],"reported_stages":[]}),
            serde_json::json!({"timestamp_ms":5,"session_generation":57,"class":"unclassified","outcome":"failed","total_ms":8001,"stages":[["target_confirmation",7000,0]],"reported_stages":[]}),
        ];
        let bytes = traces
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        fs::write(&input_path, &bytes).expect("write synthetic metadata only");
        let input_sha256 = validation::sha256(bytes.as_bytes());
        let context = serde_json::json!({"trace_sha256":input_sha256,"source":"synthetic","terminals":[
            {"line":1,"branch":"short"},{"line":2,"branch":"short"},{"line":3,"branch":"long"},{"line":4,"branch":"unknown"},{"line":5,"branch":"short"}
        ]});
        fs::write(&context_path, serde_json::to_vec(&context).unwrap())
            .expect("write branch metadata");

        let args = vec![
            "--trace-jsonl".into(),
            input_path.display().to_string(),
            "--trace-report".into(),
            output_path.display().to_string(),
            "--trace-context".into(),
            context_path.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(args.clone()).await.unwrap(), 0);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&output_path).unwrap()).unwrap();
        assert_eq!(saved["terminal_count"], 5);
        assert_eq!(saved["input_status"], "observed_terminal_rows");
        assert_eq!(saved["terminals"][0]["session_generation"], 55);
        assert_eq!(saved["terminals"][1]["session_generation"], 55);
        assert_eq!(saved["terminals"][0]["outcome"], "failed");
        assert_eq!(saved["terminals"][0]["host_covered_ms"], 7000);
        assert_eq!(saved["terminals"][0]["unattributed_ms"], 2000);
        assert_eq!(
            saved["terminals"][0]["host_stages"][0]["intervals"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            saved["terminals"][0]["reported_stages"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(saved["slow_short_terminals"].as_array().unwrap().len(), 2);
        assert_eq!(saved["slow_short_terminals"][0]["source_line"], 1);
        assert_eq!(saved["slow_short_terminals"][1]["source_line"], 5);
        assert_eq!(saved["slow_unknown_membership"][0]["source_line"], 4);
        assert_eq!(saved["host_stage_summaries"][0]["stage"], "asr_inference");
        assert_eq!(
            saved["host_stage_summaries"][12]["stage"],
            "lm_studio_status_probe"
        );
        assert_eq!(
            saved["host_stage_summaries"][12]["per_terminal_stage_union_ms"]["sample_count"],
            1
        );
        assert_eq!(
            saved["host_stage_summaries"][12]["per_terminal_stage_union_ms"]["p50_ms"].as_f64(),
            Some(1000.0)
        );
        assert!(saved["terminals"][0]["host_stages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stage| stage["stage"] == "lm_studio_status_probe"
                && stage["union_duration_ms"] == 1000));
        assert_eq!(
            saved["host_stage_summaries"][1]["per_terminal_stage_union_ms"]["sample_count"],
            0
        );
        assert_eq!(
            saved["host_stage_summaries"][1]["per_terminal_stage_union_ms"]["p50_ms"],
            serde_json::Value::Null
        );
        assert_eq!(
            saved["child_reported_summaries"].as_array().unwrap().len(),
            2
        );
        let original = fs::read(&output_path).unwrap();
        assert!(crate::evaluation::main(args).await.is_err());
        assert_eq!(fs::read(&output_path).unwrap(), original);
    }

    #[test]
    fn trace_cli_requires_both_files_and_rejects_audio_or_model_admission_options() {
        let valid = vec![
            "--trace-jsonl".into(),
            "/external/trace.jsonl".into(),
            "--trace-report".into(),
            "/external/report.json".into(),
        ];
        assert!(super::super::parse(&valid).is_ok());
        for args in [
            vec!["--trace-jsonl".into(), "/external/trace.jsonl".into()],
            vec!["--trace-report".into(), "/external/report.json".into()],
            vec![
                "--trace-jsonl".into(),
                "/external/trace.jsonl".into(),
                "--trace-report".into(),
                "/external/report.json".into(),
                "--manifest".into(),
                "/external/audio.json".into(),
            ],
        ] {
            assert_eq!(
                super::super::parse(&args).unwrap_err().code,
                "conflicting_arguments"
            );
        }
    }

    #[tokio::test]
    async fn slow_rows_without_branch_context_remain_unknown_and_8000_is_excluded() {
        let directory = TestDirectory::new();
        let input_path = directory.0.join("traces.jsonl");
        let output_path = directory.0.join("report.json");
        let rows = [
            serde_json::json!({"timestamp_ms":1,"session_generation":7,"class":"unclassified","outcome":"cancelled","total_ms":8001,"stages":[],"reported_stages":[]}),
            serde_json::json!({"timestamp_ms":2,"session_generation":8,"class":"unclassified","outcome":"failed","total_ms":8000,"stages":[],"reported_stages":[]}),
            serde_json::json!({"timestamp_ms":3,"session_generation":9,"class":"unclassified","outcome":"failed","total_ms":u64::MAX,"stages":[["asr_inference",0,1],["decode",u64::MAX-1,1]],"reported_stages":[]}),
        ];
        fs::write(
            &input_path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let args = vec![
            "--trace-jsonl".into(),
            input_path.display().to_string(),
            "--trace-report".into(),
            output_path.display().to_string(),
        ];
        assert_eq!(crate::evaluation::main(args).await.unwrap(), 0);
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(output_path).unwrap()).unwrap();
        assert!(saved["slow_short_terminals"].as_array().unwrap().is_empty());
        assert_eq!(
            saved["slow_unknown_membership"].as_array().unwrap().len(),
            2
        );
        assert_eq!(saved["slow_unknown_membership"][0]["source_line"], 1);
        assert_eq!(
            saved["slow_unknown_membership"][0]["branch_evidence"],
            "not_provided"
        );
        assert_eq!(saved["slow_unknown_membership"][1]["source_line"], 3);
        assert_eq!(saved["terminals"][0]["branch_evidence"], "not_provided");
        assert_eq!(saved["terminals"][1]["total_ms"], 8000);
        assert_eq!(saved["terminals"][2]["host_covered_ms"].as_u64(), Some(2));
        assert_eq!(
            saved["terminals"][2]["unattributed_ms"].as_u64(),
            Some(u64::MAX - 2)
        );
    }

    #[tokio::test]
    async fn explicit_short_context_conflicting_with_long_class_is_rejected() {
        let directory = TestDirectory::new();
        let input_path = directory.0.join("traces.jsonl");
        let context_path = directory.0.join("branches.json");
        let output_path = directory.0.join("report.json");
        let trace = serde_json::json!({"timestamp_ms":1,"session_generation":1,"class":"long","outcome":"failed","total_ms":9000,"stages":[],"reported_stages":[]});
        let bytes = format!("{trace}\n");
        fs::write(&input_path, &bytes).unwrap();
        let context = serde_json::json!({"trace_sha256":validation::sha256(bytes.as_bytes()),"source":"captured_observed","terminals":[{"line":1,"branch":"short"}]});
        fs::write(&context_path, serde_json::to_vec(&context).unwrap()).unwrap();
        let args = vec![
            "--trace-jsonl".into(),
            input_path.display().to_string(),
            "--trace-report".into(),
            output_path.display().to_string(),
            "--trace-context".into(),
            context_path.display().to_string(),
        ];
        let error = crate::evaluation::main(args).await.unwrap_err();
        assert_eq!(error.code(), "trace_context_binding");
        assert!(!output_path.exists());
    }

    #[test]
    fn trace_consumer_sanitizes_schema_errors_and_rejects_bad_intervals_and_context() {
        let directory = TestDirectory::new();
        let path = directory.0.join("traces.jsonl");
        let unsafe_row = br#"{"timestamp_ms":1,"session_generation":1,"class":"unclassified","outcome":"failed","total_ms":1,"stages":[],"reported_stages":[],"private_raw_text":"do not repeat this"}"#;
        let error = read_rows(&path, unsafe_row).unwrap_err();
        assert!(!error.safe_detail.contains("private_raw_text"));
        assert!(!error.safe_detail.contains("do not repeat this"));
        let overflow = br#"{"timestamp_ms":1,"session_generation":1,"class":"unclassified","outcome":"failed","total_ms":1,"stages":[["asr_inference",18446744073709551615,1]],"reported_stages":[]}"#;
        assert_eq!(
            read_rows(&path, overflow).unwrap_err().code,
            "trace_interval"
        );
        let out_of_range = br#"{"timestamp_ms":1,"session_generation":1,"class":"unclassified","outcome":"failed","total_ms":1,"stages":[["asr_inference",0,2]],"reported_stages":[]}"#;
        assert_eq!(
            read_rows(&path, out_of_range).unwrap_err().code,
            "trace_interval"
        );
        let huge_line = vec![b' '; MAX_TRACE_LINE_BYTES + 1];
        assert_eq!(
            read_rows(&path, &huge_line).unwrap_err().code,
            "trace_line_budget"
        );

        let valid = serde_json::json!({"timestamp_ms":1,"session_generation":1,"class":"unclassified","outcome":"failed","total_ms":1,"stages":[],"reported_stages":[]});
        let bytes = format!("{valid}\n");
        let parsed = read_rows(&path, bytes.as_bytes()).unwrap();
        let lines = parsed.iter().map(|(line, _)| *line).collect();
        let wrong_sha_path = directory.0.join("context.json");
        fs::write(&wrong_sha_path, br#"{"trace_sha256":"0000000000000000000000000000000000000000000000000000000000000000","source":"synthetic","terminals":[{"line":1,"branch":"short"}]}"#).unwrap();
        assert_eq!(
            read_context(
                &wrong_sha_path,
                &validation::sha256(bytes.as_bytes()),
                &lines
            )
            .unwrap_err()
            .code,
            "trace_context_binding"
        );

        let duplicate_path = directory.0.join("duplicate-context.json");
        let sha = validation::sha256(bytes.as_bytes());
        fs::write(&duplicate_path, format!(r#"{{"trace_sha256":"{sha}","source":"synthetic","terminals":[{{"line":1,"branch":"short"}},{{"line":1,"branch":"long"}}]}}"#)).unwrap();
        assert_eq!(
            read_context(&duplicate_path, &sha, &lines)
                .unwrap_err()
                .code,
            "trace_context_binding"
        );

        let out_of_range_path = directory.0.join("out-of-range-context.json");
        fs::write(&out_of_range_path, format!(r#"{{"trace_sha256":"{sha}","source":"synthetic","terminals":[{{"line":2,"branch":"short"}}]}}"#)).unwrap();
        assert_eq!(
            read_context(&out_of_range_path, &sha, &lines)
                .unwrap_err()
                .code,
            "trace_context_binding"
        );
    }

    #[test]
    fn trace_fact_limit_accepts_256_combined_facts_and_rejects_257() {
        let path = Path::new("trace.jsonl");
        let row = |fact_count: usize| {
            serde_json::json!({
                "timestamp_ms":1,"session_generation":1,"class":"unclassified","outcome":"failed","total_ms":0,
                "stages":std::iter::repeat_n(serde_json::json!(["asr_inference",0,0]), fact_count).collect::<Vec<_>>(),
                "reported_stages":[]
            })
        };
        let valid = format!("{}", row(256));
        assert_eq!(read_rows(path, valid.as_bytes()).unwrap().len(), 1);
        let over = format!("{}", row(257));
        assert_eq!(
            read_rows(path, over.as_bytes()).unwrap_err().code,
            "trace_fact_budget"
        );
    }
}
