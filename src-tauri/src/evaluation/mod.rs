//! Explicit opt-in evaluator. No application startup, microphone, Keychain or History import.
mod admission_progress;
pub(crate) mod artifacts;
pub(crate) mod audio_legacy;
pub(crate) mod authorization;
mod execution;
mod filter_report;
pub(crate) mod local_cleanup;
pub use execution::{EvaluationError, ExecutionError};
pub mod contracts;
pub mod controls;
#[cfg(test)]
pub(crate) mod fixtures;
mod identity;
mod journal;
pub(crate) mod legacy;
mod local_asr;
#[cfg(test)]
mod negative_tests;
pub(crate) mod quantile;
mod resume;
mod runner;
mod trace_report;
#[cfg(test)]
pub(crate) use runner::{
    defer_cancelled_capture, select_capture, shutdown_then_publish, CaptureJournal,
    CaptureSelection,
};
pub mod scoring;
#[path = "../../source_identity.rs"]
pub mod source_identity;
pub mod speech_input;
mod speech_profiles;
#[cfg(test)]
mod speech_tests;
#[cfg(test)]
mod tests;
pub mod validation;
use admission_progress::{Progress, Scope};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use validation::{error, InputError};

const HELP:&str="VoiceFlow audio evaluator (no uploads unless --run)\n  voiceflow-eval --trace-jsonl /external/dictation-performance.jsonl --trace-report /external/trace-report.json [--trace-context /external/short-branches.json]\n  voiceflow-eval --manifest /external/speech.json --validate\n  voiceflow-eval --experiment /external/negative-recipe-v2.json --non-speech /external/negative.json --validate\n  voiceflow-eval --manifest /external/speech.json --corpus-sidecar /external/corpus.json --experiment /external/recipe.json --non-speech /external/negative.json --validate\n  voiceflow-eval --manifest /external/speech.json --asr groq_whisper_large_v3_turbo --cleanup groq_gpt_oss_20b --out /external/report.json --run\n  voiceflow-eval --manifest /external/speech.json --corpus-sidecar /external/corpus.json --experiment /external/recipe.json --out /external/run.json --run\nOffline trace mode consumes explicitly selected content-free metadata only; no App, model, provider, credentials or audio are used.\nV2 negative-only requires SHA-bound non_speech input, cleanupOff/no postprocessing, no manifest/sidecar, and fresh-only execution (no resume/checkpoint). V1 speech contracts remain unchanged. V3 selects SHA-bound ASRSpeechIndexV1 through input.speech_index; functional_unscored has no selected reference and scoring:null, publisher_reference_asr uses original publisher ASR annotations only and the versioned ASR scorer. Both require cleanupOff/no postprocessing/file_adapter and no manifest/sidecar. V3 local ASR may explicitly select quality_replay with positive TTL and strict local CheckpointV2; V3 cloud and V2 negative-only remain fresh-only. Compiled source profiles preserve fixed-three and admit the independently audited60-member ASCEND cohort through exact proof/member binding; pending project cleanup review remains pending.\n\nExplicit VOICEFLOW_EVAL_<PROVIDER>_API_KEY or _KEY_FILE required for live cloud runs.\n--checkpoint selects a NEW external SHA checkpoint output (V1 cloud schema1 or V3 local schema2); --resume reads a prior checkpoint. Both require --experiment/--run and explicit quality_replay cache_policy + max_age_ms. Only complete accepted same-request text candidates are reusable; current guards/finalizer rerun. Reliability, GUI, cloud-anchor and performance stay fresh. V3 local replay requires current owned load attestation (runtime/Metal/image identity, actual parameter dtype/layout and derived tokenizer); missing facts reject before inference. V1 local ASR remains non-cacheable; V1 cloud CheckpointV1 semantics stay unchanged.\n--admission-progress-socket /private/tmp/owned0700/p.sock requires explicit --validate (Unix only). Optional fixed-buffer nonblocking datagrams report admission phases and cleanup-only read/file-hash/verified-file counters; at most128 packets of1024B including LF. Caller owns a canonical existing socket/short0700 parent. No stderr progress, model spawn or extra content reads. Full original terminal errors and stdout remain on their normal streams; their liveness is not guaranteed. Partial/missing terminal means sampled lower bounds, not exact interruption state.\nVerified local cleanup: recipe CleanupControls.candidate_id=vf14-qwen3-4b-instruct-2507-untrained plus --cleanup-candidate-manifest and NEW --cleanup-work-root. Validate hashes only; run launches the pinned owned child after journal setup, bypasses credentials only for that route, uses shared cleanup parser/guards/finalizer, and never attaches/downloads. Local replay stays fresh-only.\nControl fixtures: typed vocabulary/context16 KiB each, local rules256 KiB, aggregate4 MiB. Recipe auto/zh/en and explicit fixtures are capability-checked before keys; custom decode remains unsupported. Local ASR recipe candidates: local_qwen3_asr_0_6b / local_qwen3_asr_1_7b. --models-root selects existing read-only pinned source directories; --models-work-root selects a NEW exclusive external mutable directory required by --run. --validate hashes declared source files and checks a supplied work/output path does not exist, without copying, writing or starting/loading a runtime. Local candidates require --experiment, do not read cloud ASR credentials, and never download or fall back to cloud. Workspaces are retained, including partial failures; existing roots are never resumed. Ordinary model/settings paths are not selected implicitly. Reply capture requires explicit recipe opt-in.
Scoring: voiceflow-fixture-scoring-v1 automatically records stage/denominator summaries; optional recipe scoring annotations use a SHA-bound locator and one explicit paired cluster dimension/seed. Human pilot/locked/ranking/training remain NOT_RUN.\nInput budgets: scoring reference/group expansion8 MiB; annotation expansion4 MiB/20,000 spans; JSON 1 MiB; each WAV 25 MiB/10 minutes; 200 cases per manifest; 1000 total cases/512 MiB audio; 32 jobs/20 repeats; 10,000 expanded case records; report 256 MiB.\n";
fn parse(args: &[String]) -> Result<BTreeMap<String, String>, InputError> {
    let mut options = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if ![
            "--help",
            "--admission-progress-socket",
            "--cleanup-candidate-manifest",
            "--cleanup-work-root",
            "--models-root",
            "--models-work-root",
            "--manifest",
            "--corpus-sidecar",
            "--experiment",
            "--non-speech",
            "--resume",
            "--checkpoint",
            "--validate",
            "--run",
            "--out",
            "--asr",
            "--cleanup",
            "--gap-ms",
            "--json",
            "--trace-jsonl",
            "--trace-context",
            "--trace-report",
            "--filter-evidence",
            "--filter-report",
        ]
        .contains(&arg.as_str())
        {
            return Err(error(
                Path::new("arguments"),
                "unknown_argument",
                format!("Unknown argument: {arg}"),
            ));
        }
        if options.contains_key(arg) {
            return Err(error(
                Path::new("arguments"),
                "duplicate_argument",
                format!("Repeated argument: {arg}"),
            ));
        }
        let value = if ["--help", "--validate", "--run", "--json"].contains(&arg.as_str()) {
            String::new()
        } else {
            index += 1;
            args.get(index)
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| {
                    error(
                        Path::new("arguments"),
                        "missing_argument",
                        format!("Missing value for {arg}"),
                    )
                })?
        };
        options.insert(arg.clone(), value);
        index += 1;
    }
    if options.contains_key("--run") && options.contains_key("--validate") {
        return Err(error(
            Path::new("arguments"),
            "conflicting_arguments",
            "Choose --validate or --run.",
        ));
    }
    if options.contains_key("--experiment")
        && ["--asr", "--cleanup", "--gap-ms"]
            .iter()
            .any(|v| options.contains_key(*v))
    {
        return Err(error(
            Path::new("arguments"),
            "conflicting_arguments",
            "--experiment cannot mix with legacy --asr/--cleanup/--gap-ms controls.",
        ));
    }
    if ["--resume", "--checkpoint"]
        .iter()
        .any(|k| options.contains_key(*k))
        && (!options.contains_key("--run") || !options.contains_key("--experiment"))
    {
        return Err(error(
            Path::new("arguments"),
            "conflicting_arguments",
            "--resume/--checkpoint require --experiment and --run.",
        ));
    }
    if options.contains_key("--admission-progress-socket")
        && (!options.contains_key("--validate") || options.contains_key("--run"))
    {
        return Err(error(
            Path::new("arguments"),
            "conflicting_arguments",
            "--admission-progress-socket requires explicit --validate and cannot run providers",
        ));
    }
    let trace_args = ["--trace-jsonl", "--trace-context", "--trace-report"];
    if trace_args.iter().any(|key| options.contains_key(*key))
        && (!options.contains_key("--trace-jsonl")
            || !options.contains_key("--trace-report")
            || options
                .keys()
                .any(|key| !trace_args.contains(&key.as_str())))
    {
        return Err(error(Path::new("arguments"), "conflicting_arguments",
            "offline trace mode requires --trace-jsonl and --trace-report and cannot mix with other evaluator options"));
    }
    let filter_args = ["--filter-evidence", "--filter-report"];
    if filter_args.iter().any(|key| options.contains_key(*key))
        && (!filter_args.iter().all(|key| options.contains_key(*key))
            || options
                .keys()
                .any(|key| !filter_args.contains(&key.as_str())))
    {
        return Err(error(Path::new("arguments"), "conflicting_arguments", "offline filter mode requires --filter-evidence and --filter-report together and cannot mix with other evaluator options"));
    }
    Ok(options)
}
/// One admission result owns the exact normalized input and admitted resources.
/// The optional observer finalizes this Result; execution still owns all later effects.
struct AdmittedInputs {
    inputs: validation::ValidatedInputs,
    baseline: Option<contracts::BaselineIdentity>,
    local: Option<local_asr::Plan>,
    cleanup_plan: Option<local_cleanup::Plan>,
    output: Option<PathBuf>,
    replay: resume::Admission,
}
fn admit(
    options: &BTreeMap<String, String>,
    progress: &mut Progress,
) -> Result<AdmittedInputs, InputError> {
    let selected = |key: &str| options.get(key).map(Path::new);
    let inputs = progress.scope(Scope::Inputs, |_| {
        let inputs = validation::validate_selection(
            selected("--manifest"),
            selected("--corpus-sidecar"),
            selected("--experiment"),
            selected("--non-speech"),
        )?;
        if inputs.selection.non_speech_only()
            && (selected("--resume").is_some() || selected("--checkpoint").is_some())
        {
            return Err(error(
                Path::new("arguments"),
                "resume_not_applicable",
                "negative-only input requires fresh execution without resume/checkpoint",
            ));
        }
        Ok(inputs)
    })?;
    let baseline = progress.scope(Scope::Preflight, |progress| {
        if let Some(path) = selected("--experiment") {
            runner::preflight(&inputs, path, progress)
        } else {
            Ok(None)
        }
    })?;
    let local = progress.scope(Scope::LocalAsrAdmission, |_| {
        local_asr::Plan::admit(
            &inputs,
            selected("--models-root"),
            selected("--models-work-root"),
            options.contains_key("--run"),
        )
    })?;
    let cleanup_plan = progress.scope(Scope::LocalCleanupAdmission, |progress| {
        let needs_local_cleanup = inputs.recipe.as_ref().is_some_and(|r| r.jobs.iter()
            .any(|j| j.cleanup.candidate_id.as_deref() == Some(local_cleanup::ID)));
        if needs_local_cleanup {
            let manifest = selected("--cleanup-candidate-manifest").ok_or_else(|| error(
                Path::new("arguments"), "missing_cleanup_manifest", "fixed local cleanup requires explicit --cleanup-candidate-manifest"))?;
            let workspace = selected("--cleanup-work-root").ok_or_else(|| error(
                Path::new("arguments"), "missing_cleanup_work_root", "fixed local cleanup requires a NEW --cleanup-work-root, including path admission on validate"))?;
            Ok(Some(local_cleanup::Plan::admit(manifest, workspace, progress)?))
        } else {
            if selected("--cleanup-candidate-manifest").is_some() || selected("--cleanup-work-root").is_some() {
                return Err(error(Path::new("arguments"), "conflicting_arguments",
                    "local cleanup paths require an explicit local cleanup experiment candidate"));
            }
            Ok(None)
        }
    })?;
    // Output/replay validation still precedes credentials, setup and model child acquisition.
    let (output, replay) = progress.scope(Scope::OutputResumeAdmission, |_| {
        let output = selected("--out").map(|path| {
            if inputs.recipe.is_some() { runner::output_path(path) } else {
                let parent = validation::external(path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")))?;
                let name = path.file_name().ok_or_else(|| error(path, "path_escape", "output filename required"))?;
                Ok(legacy::unique_output_path(&parent.join(name), None))
            }
        }).transpose()?;
        let replay = resume::Admission::read(&inputs, selected("--resume"), selected("--checkpoint"))?;
        if let Some(plan) = &cleanup_plan {
            if output.as_deref() == Some(plan.workspace_path())
                || replay.output.as_deref() == Some(plan.workspace_path())
                || local.as_ref().and_then(|p| p.working_path()) == Some(plan.workspace_path()) {
                return Err(error(Path::new("arguments"), "conflicting_arguments",
                    "cleanup workspace, ASR workspace, report and checkpoint must have distinct new paths"));
            }
        }
        if replay.output.as_ref().zip(output.as_ref()).is_some_and(|(a, b)| a == b) {
            return Err(error(Path::new("arguments"), "conflicting_arguments", "report and checkpoint must select distinct new paths"));
        }
        Ok((output, replay))
    })?;
    Ok(AdmittedInputs {
        inputs,
        baseline,
        local,
        cleanup_plan,
        output,
        replay,
    })
}
pub async fn main(args: Vec<String>) -> Result<i32, EvaluationError> {
    let options = parse(&args)?;
    if args.is_empty() || options.contains_key("--help") {
        print!("{HELP}");
        return Ok(0);
    }
    if options.contains_key("--trace-jsonl") {
        return trace_report::run(
            Path::new(options.get("--trace-jsonl").expect("trace input checked")),
            options.get("--trace-context").map(Path::new),
            Path::new(options.get("--trace-report").expect("trace report checked")),
        )
        .map_err(EvaluationError::Input);
    }
    if options.contains_key("--filter-evidence") {
        return filter_report::run(
            Path::new(
                options
                    .get("--filter-evidence")
                    .expect("filter evidence checked"),
            ),
            Path::new(
                options
                    .get("--filter-report")
                    .expect("filter report checked"),
            ),
        )
        .map_err(EvaluationError::Input);
    }
    let mut progress =
        Progress::connect(options.get("--admission-progress-socket").map(Path::new))?;
    let admitted = admit(&options, &mut progress);
    let AdmittedInputs {
        inputs,
        baseline,
        local,
        cleanup_plan,
        output,
        replay,
    } = progress.finish(admitted)?;
    if !options.contains_key("--run") {
        if options.contains_key("--json") {
            let mut facts = serde_json::json!({"validated":true,"provider_attempts":0,"sample_count":inputs.cases.len(),"recipe_sha256":inputs.recipe_sha256,"local_source_models":local.as_ref().map(|p|p.source_summary()),"local_cleanup_source":cleanup_plan.as_ref().map(|p|p.summary()),"scoring_method":scoring::METHOD,"scoring_annotation_identity":inputs.scoring.annotation_identity});
            if let Some(baseline) = &baseline {
                facts["baseline"] = serde_json::json!(baseline);
            }
            match &inputs.selection {
                validation::InputSelection::Speech {
                    manifests,
                    selected,
                    corpus_sha256,
                    ..
                } => {
                    facts["corpus_sha256"] = serde_json::json!(corpus_sha256);
                    facts["manifest"] = serde_json::json!(manifests[*selected].parsed);
                }
                validation::InputSelection::AsrSpeech { snapshot, negative } => {
                    facts["input"] = serde_json::json!(snapshot.identity(negative.as_ref()));
                    facts["asr_speech_index"] = serde_json::json!(snapshot.index);
                }
                validation::InputSelection::NonSpeech(n) => {
                    facts["input"] = serde_json::json!({"kind":"non_speech","index_sha256":n.sha256,"selected_document":n.selected_path,"canonical_document":n.canonical_path});
                    facts["non_speech"] = serde_json::json!(n.parsed);
                }
            }
            println!("{facts}");
        } else {
            println!(
                "Validated {} reviewed audio cases. No providers contacted. provider_attempts=0.",
                inputs.cases.len()
            );
        }
        return Ok(0);
    }
    let output = output.ok_or_else(|| {
        error(
            Path::new("arguments"),
            "missing_argument",
            "Select an external --out report path.",
        )
    })?;
    if inputs.recipe.is_some() {
        let baseline = baseline.ok_or_else(|| {
            error(
                Path::new("arguments"),
                "compiled_source_identity_unavailable",
                "experiment admission did not retain artifact identity",
            )
        })?;
        let report = runner::run(&inputs, baseline, &output, local, replay, cleanup_plan).await?;
        println!("RunRecordV{}: {}", report.schema_version, output.display());
        if let Some(line) = scoring::paired_workflow_report_line(&report) {
            println!("{line}");
        }
        if let Some(line) = scoring::paired_filter_quality_report_line(&report) {
            println!("{line}");
        }
        return Ok(
            if matches!(report.terminal, contracts::StageState::Cancelled { .. }) {
                130
            } else if matches!(report.terminal, contracts::StageState::Failed { .. }) {
                1
            } else {
                0
            },
        );
    }
    let asr = options.get("--asr").ok_or_else(|| {
        error(
            Path::new("arguments"),
            "missing_argument",
            "Select comma-separated ASR candidate IDs.",
        )
    })?;
    let cleanup = options.get("--cleanup").ok_or_else(|| {
        error(
            Path::new("arguments"),
            "missing_argument",
            "Select comma-separated cleanup candidate IDs.",
        )
    })?;
    legacy::parse_id_filter(
        asr,
        runner::CLOUD_ASR_IDS,
        "legacy cloud ASR candidate (local candidates require --experiment)",
    )
    .map_err(|v| error(Path::new("arguments"), "unsupported_candidate", v))?;
    legacy::parse_id_filter(cleanup, runner::CLEANUP_IDS, "cleanup candidate")
        .map_err(|v| error(Path::new("arguments"), "unsupported_candidate", v))?;
    let gap = options
        .get("--gap-ms")
        .map(|v| v.parse::<u64>())
        .transpose()
        .map_err(|_| {
            error(
                Path::new("arguments"),
                "schema",
                "Cleanup request gap must be 0–5000 ms.",
            )
        })?
        .unwrap_or(250);
    if gap > 5000 {
        return Err(error(
            Path::new("arguments"),
            "schema",
            "Cleanup request gap must be 0–5000 ms.",
        )
        .into());
    }
    runner::legacy_required_keys(asr, cleanup)?;
    // The legacy scheduler is shared with the old ignored test and retains fixed controls/model overrides.
    std::env::set_var("VOICEFLOW_AUDIO_EVAL_OPT_IN", "1");
    std::env::set_var(
        "VOICEFLOW_AUDIO_EVAL_MANIFEST",
        inputs
            .selection
            .speech_manifest()
            .expect("legacy speech selection")
            .canonical_path
            .clone(),
    );
    std::env::set_var("VOICEFLOW_AUDIO_EVAL_ASR_IDS", asr);
    std::env::set_var("VOICEFLOW_LIVE_CLEANUP_CANDIDATE_IDS", cleanup);
    std::env::set_var("VOICEFLOW_AUDIO_EVAL_REPORT", &output);
    std::env::set_var("VOICEFLOW_LIVE_MIN_REQUEST_GAP_MS", gap.to_string());
    println!("Running legacy_fixed reviewed audio through production adapters. Reports contain fixture transcripts.");
    audio_legacy::execute(&inputs).await?;
    Ok(0)
}
