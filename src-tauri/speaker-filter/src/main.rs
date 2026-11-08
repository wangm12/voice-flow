use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use voiceflow_speaker_filter::enrollment::enroll_raw_audio;
use voiceflow_speaker_filter::fileutil::{
    absolute_destination, enforce_private_mode, reject_same_file, require_absolute, sha256_file,
    write_private_atomic,
};
use voiceflow_speaker_filter::filter::{
    FilterSession, Recipe, FOCUS_DELAY_SAMPLES, MAX_HOST_BATCH_SAMPLES,
};
use voiceflow_speaker_filter::models::{upstream_revision, LoadedModels};
use voiceflow_speaker_filter::profile::ProfileFile;
use voiceflow_speaker_filter::wav::{
    read_raw16k_mono, write_raw16k_mono, MAX_AUDIO_SAMPLES, SAMPLE_RATE_HZ,
};
use voiceflow_speaker_filter::wire::serve_stdio;
use voiceflow_speaker_filter::{Error, Result};

#[derive(Debug)]
struct FilterArgs {
    models: PathBuf,
    profile: PathBuf,
    input: PathBuf,
    output: PathBuf,
    recipe: Recipe,
    paced: bool,
    report: Option<PathBuf>,
}

#[derive(Serialize)]
struct EnrollSummary<'a> {
    status: &'static str,
    schema_version: u32,
    source: &'static str,
    source_samples: u64,
    duration_seconds: f64,
    source_sample_format: &'a str,
    source_sha256: String,
    valid_speech_samples: u64,
    valid_speech_interval_count: usize,
    clipped_samples: u64,
    consistency_check_passed: bool,
    models_digest_sha256: &'a str,
    profile_checksum_sha256: &'a str,
    profile_file_sha256: String,
}

#[derive(Serialize)]
struct FilterReport {
    schema_version: u32,
    status: &'static str,
    upstream_revision: &'static str,
    recipe: &'static str,
    paced: bool,
    sample_rate_hz: u32,
    channels: u16,
    input_samples: u64,
    output_samples: u64,
    model_delay_samples: u64,
    discarded_leading_samples: u64,
    hop_padding_samples: u64,
    flush_padding_samples: u64,
    cue_observation_frames: u64,
    models_digest_sha256: String,
    profile_sha256: String,
    input_sha256: String,
    output_sha256: String,
    processing_elapsed_ms: f64,
    session_init_elapsed_ms: f64,
    real_time_factor: f64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("voiceflow-speaker-filter: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }
    match args[0].as_str() {
        "enroll" => run_enroll(parse_enroll(&args[1..])?),
        "filter" => run_filter(parse_filter(&args[1..])?),
        "serve" => run_serve(parse_serve(&args[1..])?),
        _ => Err(Error::InvalidArgument("unknown command; use --help")),
    }
}

fn parse_serve(args: &[String]) -> Result<PathBuf> {
    let (values, flags) = parse_options(args)?;
    if !flags.is_empty() {
        return Err(Error::InvalidArgument("serve does not accept flags"));
    }
    require_exact_keys(&values, &["models"])?;
    let models = path_arg(&values, "models")?;
    require_absolute(&models, "--models must be an absolute directory")?;
    Ok(models)
}

fn run_serve(models_dir: PathBuf) -> Result<()> {
    let models = LoadedModels::load(&models_dir)?;
    serve_stdio(&models)
}

fn parse_enroll(args: &[String]) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let (values, flags) = parse_options(args)?;
    if !flags.is_empty() {
        return Err(Error::InvalidArgument("enroll does not accept flags"));
    }
    require_exact_keys(&values, &["models", "input", "output"])?;
    let models = path_arg(&values, "models")?;
    let input = path_arg(&values, "input")?;
    let output = path_arg(&values, "output")?;
    require_absolute(&models, "--models must be an absolute directory")?;
    require_absolute(&input, "--input must be an absolute file path")?;
    require_absolute(&output, "--output must be an absolute file path")?;
    reject_same_file(&input, &output)?;
    Ok((models, input, output))
}

fn parse_filter(args: &[String]) -> Result<FilterArgs> {
    let (values, flags) = parse_options(args)?;
    for flag in flags.iter() {
        if flag != "paced" {
            return Err(Error::InvalidArgument(
                "filter accepts only the --paced flag",
            ));
        }
    }
    let mut allowed = BTreeSet::from(["models", "profile", "input", "output", "recipe"]);
    if values.contains_key("report") {
        allowed.insert("report");
    }
    if values.keys().any(|key| !allowed.contains(key.as_str()))
        || ["models", "profile", "input", "output", "recipe"]
            .iter()
            .any(|key| !values.contains_key(*key))
    {
        return Err(Error::InvalidArgument("filter requires --models, --profile, --input, --output, and --recipe; --report is optional"));
    }
    let models = path_arg(&values, "models")?;
    let profile = path_arg(&values, "profile")?;
    let input = path_arg(&values, "input")?;
    let output = path_arg(&values, "output")?;
    let recipe = Recipe::parse(values.get("recipe").expect("required"))?;
    let report = values.get("report").map(PathBuf::from);
    for (path, flag) in [
        (&models, "--models"),
        (&profile, "--profile"),
        (&input, "--input"),
        (&output, "--output"),
    ] {
        require_absolute(path, flag)?;
    }
    if let Some(report_path) = &report {
        require_absolute(report_path, "--report must be an absolute file path")?;
    }
    reject_same_file(&input, &output)?;
    reject_distinct_paths(&[&input, &profile, &output], report.as_deref())?;
    Ok(FilterArgs {
        models,
        profile,
        input,
        output,
        recipe,
        paced: flags.contains("paced"),
        report,
    })
}

fn parse_options(args: &[String]) -> Result<(BTreeMap<String, String>, BTreeSet<String>)> {
    let mut values = BTreeMap::new();
    let mut flags = BTreeSet::new();
    let mut at = 0;
    while at < args.len() {
        let arg = args[at]
            .strip_prefix("--")
            .ok_or(Error::InvalidArgument("options must start with --"))?;
        if arg.is_empty() {
            return Err(Error::InvalidArgument("empty option"));
        }
        if arg == "paced" {
            if !flags.insert(arg.to_string()) {
                return Err(Error::InvalidArgument("duplicate option"));
            }
            at += 1;
            continue;
        }
        let value = args
            .get(at + 1)
            .ok_or(Error::InvalidArgument("option value is missing"))?;
        if value.starts_with("--") || value.is_empty() {
            return Err(Error::InvalidArgument("option value is missing"));
        }
        if values.insert(arg.to_string(), value.clone()).is_some() {
            return Err(Error::InvalidArgument("duplicate option"));
        }
        at += 2;
    }
    Ok((values, flags))
}

fn require_exact_keys(values: &BTreeMap<String, String>, required: &[&str]) -> Result<()> {
    let expected: BTreeSet<&str> = required.iter().copied().collect();
    if values.len() != expected.len() || values.keys().any(|key| !expected.contains(key.as_str())) {
        return Err(Error::InvalidArgument(
            "command options are incomplete or unknown",
        ));
    }
    Ok(())
}

fn path_arg(values: &BTreeMap<String, String>, key: &str) -> Result<PathBuf> {
    values
        .get(key)
        .map(PathBuf::from)
        .ok_or(Error::InvalidArgument("required path option is missing"))
}

fn run_enroll((models_dir, input_path, profile_path): (PathBuf, PathBuf, PathBuf)) -> Result<()> {
    let models = LoadedModels::load(&models_dir)?;
    let audio = read_raw16k_mono(&input_path)?;
    let enrolled = enroll_raw_audio(
        &models,
        &audio.samples,
        audio.sample_format,
        &audio.source_sha256,
    )?;
    let profile_file_sha256 = enrolled.profile.write_new(&profile_path)?;
    let summary = EnrollSummary {
        status: "ok",
        schema_version: enrolled.profile.schema_version,
        source: "raw16k",
        source_samples: audio.samples.len() as u64,
        duration_seconds: audio.samples.len() as f64 / SAMPLE_RATE_HZ as f64,
        source_sample_format: audio.sample_format,
        source_sha256: audio.source_sha256.clone(),
        valid_speech_samples: enrolled.valid_speech_samples,
        valid_speech_interval_count: enrolled
            .profile
            .source
            .enrollment_quality
            .valid_intervals
            .len(),
        clipped_samples: enrolled.clipped_samples,
        consistency_check_passed: enrolled
            .consistency_verdicts
            .iter()
            .all(|verdict| verdict.same),
        models_digest_sha256: &models.digest_sha256,
        profile_checksum_sha256: &enrolled.profile.checksum_sha256,
        profile_file_sha256,
    };
    print_json(&summary)
}

fn run_filter(args: FilterArgs) -> Result<()> {
    let models = LoadedModels::load(&args.models)?;
    let (profile, profile_file_sha256) = ProfileFile::read(&args.profile, &models)?;
    let audio = read_raw16k_mono(&args.input)?;
    if audio.samples.len() > MAX_AUDIO_SAMPLES {
        return Err(Error::Input(
            "input audio exceeds the 15-minute sample limit",
        ));
    }
    reject_distinct_paths(
        &[&args.input, &args.profile, &args.output],
        args.report.as_deref(),
    )?;
    let input_sha256 = audio.source_sha256.clone();
    let init_started = Instant::now();
    let mut session = FilterSession::new(&models, &profile, args.recipe)?;
    let session_init_elapsed_ms = init_started.elapsed().as_secs_f64() * 1000.0;

    let processing_started = Instant::now();
    let mut output = Vec::with_capacity(audio.samples.len());
    let mut fed = 0_usize;
    for batch in audio.samples.chunks(MAX_HOST_BATCH_SAMPLES) {
        output.extend(session.push(batch)?);
        fed += batch.len();
        if args.paced {
            pace_until(processing_started, fed);
        }
    }
    output.extend(session.finish()?);
    let processing_elapsed_ms = processing_started.elapsed().as_secs_f64() * 1000.0;
    let metrics = session.metrics();
    if output.len() != audio.samples.len() || metrics.output_samples != metrics.input_samples {
        return Err(Error::Model(
            "filtered output length does not match the source timeline",
        ));
    }
    write_raw16k_mono(&args.output, &output)?;
    let output_sha256 = sha256_file(&args.output)?;
    let audio_ms = audio.samples.len() as f64 / SAMPLE_RATE_HZ as f64 * 1000.0;
    let report = FilterReport {
        schema_version: 1,
        status: "ok",
        upstream_revision: upstream_revision(),
        recipe: args.recipe.as_str(),
        paced: args.paced,
        sample_rate_hz: SAMPLE_RATE_HZ,
        channels: 1,
        input_samples: audio.samples.len() as u64,
        output_samples: output.len() as u64,
        model_delay_samples: FOCUS_DELAY_SAMPLES as u64,
        discarded_leading_samples: FOCUS_DELAY_SAMPLES as u64,
        hop_padding_samples: metrics.hop_padding_samples,
        flush_padding_samples: metrics.flush_padding_samples,
        cue_observation_frames: metrics.cue_observation_frames,
        models_digest_sha256: models.digest_sha256,
        profile_sha256: profile_file_sha256,
        input_sha256,
        output_sha256,
        processing_elapsed_ms,
        session_init_elapsed_ms,
        real_time_factor: processing_elapsed_ms / audio_ms,
    };
    if let Some(report_path) = args.report {
        let bytes = serde_json::to_vec(&report)
            .map_err(|_| Error::Output("filter report could not be encoded"))?;
        write_private_atomic(&report_path, &bytes, true)?;
        enforce_private_mode(&report_path)?;
    }
    print_json(&report)
}

fn reject_distinct_paths(inputs: &[&Path], optional: Option<&Path>) -> Result<()> {
    let mut canonical = Vec::with_capacity(inputs.len() + usize::from(optional.is_some()));
    for path in inputs {
        let normalized = match std::fs::canonicalize(path) {
            Ok(path) => path,
            Err(_) => absolute_destination(path)
                .map_err(|_| Error::Input("input or profile file is unavailable"))?,
        };
        canonical.push(normalized);
    }
    if let Some(path) = optional {
        canonical.push(absolute_destination(path)?);
    }
    let mut sorted = canonical;
    sorted.sort();
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::InvalidArgument(
            "input, profile, output, and report paths must be distinct",
        ));
    }
    Ok(())
}

fn pace_until(started: Instant, samples_fed: usize) {
    let target = Duration::from_secs_f64(samples_fed as f64 / SAMPLE_RATE_HZ as f64);
    if let Some(remaining) = target.checked_sub(started.elapsed()) {
        std::thread::sleep(remaining);
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let line =
        serde_json::to_string(value).map_err(|_| Error::Output("summary could not be encoded"))?;
    println!("{line}");
    Ok(())
}

fn print_help() {
    println!(
        "VoiceFlow speaker-filter native pilot helper\n\n\
         Commands:\n\
         \x20 enroll --models ABS --input ABS_WAV --output ABS_PROFILE\n\
         \x20 filter --models ABS --profile ABS_PROFILE --input ABS_WAV --output ABS_WAV --recipe g0|g1 [--paced] [--report ABS_JSON]\n\
         \x20 serve --models ABS\n\n\
         WAV input: mono 16 kHz PCM16/24/32 or float32. Output WAV: mono 16 kHz float32.\n\
         Enrollment requires a 15–30 s source and at least 10 s of raw Cue-accepted frames.\n\
         Model directory must contain the three checksum-pinned .alk files.\n\
         Recipe g0 runs Focus then observes Cue. Recipe g1 adds upstream InputGain before Focus.\n\
         Cue is observational only and never gates or deletes samples."
    );
}
