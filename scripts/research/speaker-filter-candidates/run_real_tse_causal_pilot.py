#!/usr/bin/env python3
"""Run a private bounded REAL-TSE causal target-extraction smoke.

Uses the pinned WeSep ``spk_emb_causal_100`` checkpoint and the WeSpeaker code
revision referenced by the official WeSep README. Checkpoints are deserialized
with ``weights_only=True`` before loading. The upstream Python API evaluates a
whole waveform at once; this pilot does not claim an incremental streaming API.

Inputs and enrollments are the existing 16 kHz mono PCM16 frozen public-dev
counterexamples. The raw Qwen input bytes are used as the exact model input;
the filtered result is clipped/quantized to PCM16 without loudness normalization.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import random
import subprocess
import sys
import time
import types
import wave
import zlib

import numpy as np

REPO_COMMIT = "2a540977a348fbaa92e623210505430e2cec608d"
WESPEAKER_COMMIT = "8f53b6485d9f88a207bd17e7f8dba899495ec794"
QWEN_MODEL = "qwen3-asr-0.6b"
QWEN_REVISION = "eae2b51f96265328f1e7beced788adb0e4536f92"
SAMPLE_RATE = 16_000
EXPECTED_CASES = 18
EXPECTED_ROLE_COUNTS = {"other_only": 12, "target_soft_level": 3, "target_mixture": 3}
SEED = 42


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_json(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def private_package_namespace(name: str, package_path: Path) -> None:
    """Expose only the upstream model modules, bypassing CLI auto-downloads."""
    package = types.ModuleType(name)
    package.__path__ = [str(package_path)]
    package.__package__ = name
    package.__file__ = str(package_path / "__init__.py")
    sys.modules[name] = package


def read_pcm16(path: Path) -> tuple[np.ndarray, bytes]:
    if path.is_symlink():
        raise ValueError(f"refusing symlinked audio: {path}")
    with wave.open(str(path), "rb") as audio:
        actual = (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype())
        if actual != (1, 2, SAMPLE_RATE, "NONE"):
            raise ValueError(f"expected 16 kHz mono PCM16 WAV, got {actual}: {path}")
        raw = audio.readframes(audio.getnframes())
        if len(raw) != audio.getnframes() * 2:
            raise ValueError(f"truncated PCM16 WAV: {path}")
    pcm_i16 = np.frombuffer(raw, dtype="<i2")
    return (pcm_i16.astype(np.float32) / np.float32(32768.0)).copy(), raw


def write_pcm16_wav(path: Path, pcm: np.ndarray) -> dict:
    if pcm.ndim != 1 or not np.isfinite(pcm).all():
        raise ValueError(f"invalid/non-finite separated audio: {path}")
    clipped = np.clip(pcm.astype(np.float32, copy=False), -1.0, 1.0)
    clipped_samples = int(np.count_nonzero(clipped != pcm))
    pcm_i16 = np.trunc(clipped * np.float32(32767.0)).astype("<i2")
    raw = pcm_i16.tobytes()
    temp = path.with_name(f".{path.name}.part")
    if path.exists() or path.is_symlink() or temp.exists() or temp.is_symlink():
        raise FileExistsError(path)
    try:
        with wave.open(str(temp), "wb") as audio:
            audio.setnchannels(1)
            audio.setsampwidth(2)
            audio.setframerate(SAMPLE_RATE)
            audio.setcomptype("NONE", "not compressed")
            audio.writeframes(raw)
        os.chmod(temp, 0o600)
        os.replace(temp, path)
        os.chmod(path, 0o600)
    except Exception:
        temp.unlink(missing_ok=True)
        raise
    return {
        "pcm16_sha256": hashlib.sha256(raw).hexdigest(),
        "output_sha256": sha256(path),
        "output_bytes": path.stat().st_size,
        "samples": int(pcm_i16.size),
        "sample_rate_hz": SAMPLE_RATE,
        "clipped_samples": clipped_samples,
        "all_zero": bool(np.count_nonzero(pcm_i16) == 0),
        "pcm16_peak_dbfs": float(20 * np.log10(max(float(np.max(np.abs(pcm_i16))) / 32768.0, 1e-12))),
        "pcm16_rms_dbfs": float(20 * np.log10(max(float(np.sqrt(np.mean(pcm_i16.astype(np.float64) ** 2))) / 32768.0, 1e-12))),
    }


def set_pretrained_null(config: object) -> int:
    if isinstance(config, dict):
        removed = 0
        for key, value in list(config.items()):
            if key == "pretrained" and value is not None:
                config[key] = None
                removed += 1
            else:
                removed += set_pretrained_null(value)
        return removed
    if isinstance(config, list):
        return sum(set_pretrained_null(item) for item in config)
    return 0


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-dir", type=Path, required=True)
    parser.add_argument("--wespeaker-dir", type=Path, required=True)
    parser.add_argument("--model-bundle", type=Path, required=True,
                        help="private acquired spk_emb_causal_100 directory")
    parser.add_argument("--acquisition-receipt", type=Path, required=True)
    parser.add_argument("--frozen-case-dir", type=Path, required=True)
    parser.add_argument("--prepared-manifest", type=Path, required=True)
    parser.add_argument("--historical-asr-results", type=Path, required=True)
    parser.add_argument("--run-root", type=Path, required=True,
                        help="new private candidate output directory")
    args = parser.parse_args()

    repo = args.repo_dir.resolve(strict=True)
    wespeaker = args.wespeaker_dir.resolve(strict=True)
    model_bundle = args.model_bundle.resolve(strict=True)
    acquisition_path = args.acquisition_receipt.resolve(strict=True)
    frozen_dir = args.frozen_case_dir.resolve(strict=True)
    prepared_path = args.prepared_manifest.resolve(strict=True)
    historical_path = args.historical_asr_results.resolve(strict=True)
    run_root = args.run_root.expanduser().resolve()
    for path, expected, name in (
        (repo, REPO_COMMIT, "WeSep"),
        (wespeaker, WESPEAKER_COMMIT, "WeSpeaker"),
    ):
        actual = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"],
                                check=True, text=True, capture_output=True).stdout.strip()
        if actual != expected:
            raise ValueError(f"{name} source pin mismatch: {actual}")
    if run_root.exists():
        raise FileExistsError(f"run root must be new: {run_root}")

    receipt = json.loads(acquisition_path.read_text(encoding="utf-8"))
    if receipt.get("model_name") != "spk_emb_causal_100" or receipt.get("model_license") is None:
        raise ValueError("private acquisition receipt is missing the selected model/license status")
    files_by_name = {Path(item["destination"]).name: item for item in receipt.get("selected_members", [])}
    config_path = model_bundle / "config.yaml"
    checkpoint_path = model_bundle / "avg_model.pt"
    for path in (config_path, checkpoint_path):
        expected = files_by_name.get(path.name)
        if expected is None or path.is_symlink() or sha256(path) != expected["sha256"]:
            raise ValueError(f"selected model file failed acquisition receipt verification: {path}")

    import yaml
    import torch
    import torchaudio

    torch.set_num_threads(max(1, min(8, os.cpu_count() or 1)))
    torch.set_num_interop_threads(1)
    random.seed(SEED)
    np.random.seed(SEED)
    torch.manual_seed(SEED)
    config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    if config["dataset_args"].get("resample_rate") != SAMPLE_RATE:
        raise ValueError("selected model config does not target 16 kHz")
    model_config = config["model_args"]["tse_model"]
    if model_config["separator"].get("causal") is not True:
        raise ValueError("selected checkpoint config is not causal")
    if model_config["speaker"]["features"]["spkemb"].get("enabled") is not True:
        raise ValueError("selected checkpoint is not the speaker-embedding causal variant")
    cleared_external_pretrains = set_pretrained_null(config)

    # Import the model implementation but skip both package __init__ modules:
    # they import CLI helpers that initialize/download optional VAD models.
    private_package_namespace("wesep", repo / "wesep")
    private_package_namespace("wespeaker", wespeaker / "wespeaker")
    from wesep.models.tse_bsrnn_spk import TSE_BSRNN_SPK

    load_started = time.perf_counter()
    checkpoint = torch.load(checkpoint_path, map_location="cpu", weights_only=True)
    if not isinstance(checkpoint, dict) or not isinstance(checkpoint.get("models"), (list, tuple)) or len(checkpoint["models"]) != 1:
        raise ValueError("safe checkpoint payload did not have the expected one-model structure")
    state = checkpoint["models"][0]
    if not isinstance(state, dict) or not all(isinstance(key, str) and isinstance(value, torch.Tensor) for key, value in state.items()):
        raise ValueError("safe checkpoint state is not a string-to-tensor mapping")
    encoder_state_keys = [key for key in state if key.startswith("spk_ft.encoder.spk_model.")]
    if len(encoder_state_keys) < 100:
        raise ValueError("selected checkpoint is missing its configured ECAPA speaker-encoder tensors")
    model = TSE_BSRNN_SPK(model_config)
    incompatible = model.load_state_dict(state, strict=True)
    if incompatible.missing_keys or incompatible.unexpected_keys:
        raise ValueError("strict checkpoint load returned incompatible keys")
    model.eval().to(torch.device("cpu"))
    model_load_seconds = time.perf_counter() - load_started

    prepared = json.loads(prepared_path.read_text(encoding="utf-8"))
    profile_inputs = {}
    for profile in prepared.get("profiles", []):
        path = Path(profile["input_audio_path"]).resolve(strict=True)
        if sha256(path) != profile["input_audio_sha256"]:
            raise ValueError(f"prepared enrollment input hash mismatch: {path}")
        samples, _raw = read_pcm16(path)
        if samples.size != profile["input_samples"]:
            raise ValueError(f"prepared enrollment sample count mismatch: {profile['profile_id']}")
        profile_inputs[profile["profile_id"]] = {
            "path": path,
            "sha256": profile["input_audio_sha256"],
            "source_speaker_id": profile["source_speaker_id"],
            "samples": int(samples.size),
        }

    historical = json.loads(historical_path.read_text(encoding="utf-8"))
    if historical.get("model_id") != QWEN_MODEL or historical.get("revision") != QWEN_REVISION:
        raise ValueError("historical ASR results are not the pinned local Qwen3-ASR 0.6B run")
    if historical.get("cloud_calls") != 0 or historical.get("microphone_used") is not False:
        raise ValueError("historical ASR report violates local-only constraints")
    historical_rows = {row.get("case", {}).get("id"): row for row in historical.get("rows", [])}

    case_paths = sorted((frozen_dir / "case-results").glob("*.json"))
    if len(case_paths) != EXPECTED_CASES:
        raise ValueError(f"expected {EXPECTED_CASES} canonical frozen cases, found {len(case_paths)}")
    cases = [json.loads(path.read_text(encoding="utf-8")) for path in case_paths]
    roles: dict[str, int] = {}
    for case in cases:
        roles[case["role"]] = roles.get(case["role"], 0) + 1
    if roles != EXPECTED_ROLE_COUNTS:
        raise ValueError(f"canonical case role counts changed: {roles}")
    if sum(case.get("soft_level_challenge") is True for case in cases) != 3:
        raise ValueError("expected exactly three explicit soft-level target guard cases")
    if any(case.get("source_speaker_id") not in (None, 3, 7, 12, 17, 20) for case in cases):
        raise ValueError("frozen smoke includes unexpected source speaker IDs")

    run_root.mkdir(parents=True, mode=0o700)
    os.chmod(run_root, 0o700)
    audio_dir = run_root / "audio"
    case_result_dir = run_root / "case-results"
    audio_dir.mkdir(mode=0o700)
    case_result_dir.mkdir(mode=0o700)
    ledger = []
    unique_new: dict[str, dict] = {}
    times = {}
    case_load_started = time.perf_counter()
    for case in cases:
        case_id = case["case_id"]
        raw_row = historical_rows.get(f"{case_id}-raw")
        g0_row = historical_rows.get(f"{case_id}-g0")
        if raw_row is None or g0_row is None:
            raise ValueError(f"missing frozen raw/G0 ASR rows for {case_id}")
        raw_path = Path(raw_row["case"]["audio_path"]).resolve(strict=True)
        g0_path = Path(g0_row["case"]["audio_path"]).resolve(strict=True)
        raw_sha, g0_sha = raw_row["audio_sha256"], g0_row["audio_sha256"]
        if sha256(raw_path) != raw_sha or sha256(g0_path) != g0_sha:
            raise ValueError(f"historical raw/G0 audio hash mismatch for {case_id}")
        mix_np, raw_pcm_bytes = read_pcm16(raw_path)
        profile = profile_inputs.get(case["profile_id"])
        if profile is None:
            raise ValueError(f"unknown enrollment profile for {case_id}: {case['profile_id']}")
        enroll_np, _enroll_raw = read_pcm16(profile["path"])
        if len(raw_pcm_bytes) != mix_np.size * 2:
            raise ValueError(f"raw PCM byte/sample mismatch for {case_id}")
        mix = torch.from_numpy(mix_np).reshape(1, -1)
        enroll = torch.from_numpy(enroll_np).reshape(1, -1)
        begin = time.perf_counter()
        with torch.inference_mode():
            outputs = model(mix, enroll)
        if isinstance(outputs, (list, tuple)):
            outputs = outputs[0]
        if not isinstance(outputs, torch.Tensor) or outputs.ndim != 3 or outputs.shape[0] != 1 or outputs.shape[1] != 1:
            raise ValueError(f"unexpected TSE model output shape for {case_id}: {getattr(outputs, 'shape', None)}")
        raw_model_output = outputs[0, 0].detach().cpu().numpy().astype(np.float32, copy=True)
        scoring_seconds = time.perf_counter() - begin
        times[case_id] = scoring_seconds
        if not np.isfinite(raw_model_output).all():
            raise ValueError(f"TSE output contains non-finite samples: {case_id}")
        model_samples = int(raw_model_output.size)
        input_samples = int(mix_np.size)
        output = raw_model_output[:input_samples]
        if output.size < input_samples:
            output = np.pad(output, (0, input_samples - output.size), mode="constant")
        output_path = audio_dir / f"{case_id}.wav"
        output_audio = write_pcm16_wav(output_path, output)
        if output_audio["samples"] != input_samples:
            raise ValueError(f"candidate output does not match exact valid input sample count: {case_id}")
        if output_audio["sample_rate_hz"] != SAMPLE_RATE:
            raise ValueError(f"candidate output sample rate changed: {case_id}")
        raw_wave_sha = sha256(raw_path)
        if raw_sha != raw_wave_sha:
            raise ValueError(f"raw baseline changed while running: {case_id}")
        row = {
            "case_id": case_id,
            "role": case["role"],
            "soft_level_challenge": bool(case.get("soft_level_challenge", False)),
            "language": case["language"],
            "profile_id": case["profile_id"],
            "profile_source_speaker_id": profile["source_speaker_id"],
            "enrollment_path": str(profile["path"]),
            "enrollment_sha256": profile["sha256"],
            "model_input_path": str(raw_path),
            "model_input_sha256": raw_sha,
            "model_input_exact_raw_asr_wav": True,
            "model_input_sample_rate_hz": SAMPLE_RATE,
            "model_input_samples": input_samples,
            "model_input_duration_seconds": input_samples / SAMPLE_RATE,
            "raw_baseline_asr_case_id": f"{case_id}-raw",
            "raw_baseline_asr_audio_sha256": raw_sha,
            "g0_baseline_asr_case_id": f"{case_id}-g0",
            "g0_baseline_asr_audio_sha256": g0_sha,
            "model_output_samples_before_alignment": model_samples,
            "model_output_length_delta_samples": model_samples - input_samples,
            "output_alignment_policy": "keep sample zero fixed; right-crop model tail or right-zero-pad to exact valid input samples; no leading trim or time warp",
            "candidate_output_path": str(output_path),
            "candidate_output_sha256": output_audio["output_sha256"],
            "candidate_pcm16_sha256": output_audio["pcm16_sha256"],
            "candidate_output_bytes": output_audio["output_bytes"],
            "candidate_output_samples": output_audio["samples"],
            "candidate_output_duration_seconds": output_audio["samples"] / SAMPLE_RATE,
            "candidate_output_all_zero": output_audio["all_zero"],
            "candidate_output_clipped_samples": output_audio["clipped_samples"],
            "candidate_output_peak_dbfs": output_audio["pcm16_peak_dbfs"],
            "candidate_output_rms_dbfs": output_audio["pcm16_rms_dbfs"],
            "whole_utterance_inference_seconds": scoring_seconds,
            "whole_utterance_real_time_factor": scoring_seconds / max(input_samples / SAMPLE_RATE, 1e-12),
            "publisher_reference_sha256": case.get("publisher_reference_sha256"),
            "reference_project_review_status": case.get("reference_project_review_status", "pending"),
        }
        ledger.append(row)
        case_result = {
            "artifact_type": "voiceflow_real_tse_causal_case_result",
            "schema_version": 1,
            "status": "complete",
            **row,
        }
        write_json(case_result_dir / f"{case_id}.json", case_result)
        old_matches = [item for item in historical.get("rows", []) if item.get("audio_sha256") == output_audio["output_sha256"]]
        if not old_matches:
            unique_new.setdefault(output_audio["output_sha256"], {
                "id": f"real-tse-filter-{output_audio['output_sha256'][:16]}",
                "audio_path": str(output_path),
                "audio_sha256": output_audio["output_sha256"],
                "reference": raw_row.get("case", {}).get("reference", ""),
                "language": case["language"],
                "condition": "real_tse_spk_emb_causal_100",
                "role": case["role"],
                "case_ids": [],
            })["case_ids"].append(case_id)

    ledger.sort(key=lambda row: row["case_id"])
    role_summary: dict[str, dict] = {}
    for row in ledger:
        counts = role_summary.setdefault(row["role"], {
            "cases": 0,
            "soft_level_cases": 0,
            "candidate_all_zero": 0,
            "candidate_nonzero": 0,
        })
        counts["cases"] += 1
        counts["soft_level_cases"] += int(row["soft_level_challenge"])
        counts["candidate_all_zero"] += int(row["candidate_output_all_zero"])
        counts["candidate_nonzero"] += int(not row["candidate_output_all_zero"])
    report = {
        "artifact_type": "voiceflow_real_tse_causal_private_pilot",
        "schema_version": 1,
        "run_id": run_root.name,
        "status": "complete",
        "decision_scope": "bounded_public_counterexample_discovery_smoke_only",
        "candidate": "REAL-TSE WeSep spk_emb_causal_100 whole-utterance target speech extraction",
        "source": {
            "repository": "https://github.com/REAL-TSE/wesep-real-tse",
            "commit": REPO_COMMIT,
            "root_license_status": "no root LICENSE file or GitHub license record; source files inspected include Apache-2.0 headers",
            "model_license_status": "not stated in official README or the linked checkpoint archive; redistribution rights unverified",
            "source_archive_name": receipt["source_archive_name"],
            "source_archive_bytes": receipt["source_archive_bytes"],
            "source_archive_sha256": None,
            "selected_member_compressed_bytes": receipt["total_compressed_bytes_fetched"],
            "whole_zip_downloaded": False,
            "other_checkpoint_members_fetched": 0,
            "model_sha256": sha256(checkpoint_path),
            "config_sha256": sha256(config_path),
            "model_bundle_receipt_path": str(acquisition_path),
            "model_bundle_receipt_sha256": sha256(acquisition_path),
            "speaker_encoder_repository": "https://github.com/wenet-e2e/wespeaker",
            "speaker_encoder_commit": WESPEAKER_COMMIT,
            "speaker_encoder_weight_source": "221 ECAPA tensor keys included in selected TSE state; no separate encoder checkpoint loaded",
        },
        "model": {
            "name": "spk_emb_causal_100",
            "sample_rate_hz": SAMPLE_RATE,
            "causal_config": True,
            "causal_training_chunk_samples": config["dataset_args"].get("chunk_len"),
            "causal_training_chunk_seconds": config["dataset_args"].get("chunk_len", 0) / SAMPLE_RATE,
            "speaker_embedding_dimensions": 192,
            "checkpoint_loaded_with_weights_only": True,
            "state_dict_tensor_count": len(state),
            "bundled_speaker_encoder_tensor_count": len(encoder_state_keys),
            "strict_state_dict_load": True,
            "external_pretrained_config_entries_cleared": cleared_external_pretrains,
            "external_weight_downloads": 0,
            "pytorch_parameters": sum(parameter.numel() for parameter in model.parameters()),
        },
        "runtime": {
            "python": platform.python_version(),
            "platform": platform.platform(),
            "machine": platform.machine(),
            "device": "cpu",
            "torch": importlib.metadata.version("torch"),
            "torchaudio": torchaudio.__version__,
            "numpy": np.__version__,
            "pyyaml": importlib.metadata.version("PyYAML"),
            "model_load_seconds": model_load_seconds,
            "torch_threads": torch.get_num_threads(),
            "seed": SEED,
        },
        "input_artifacts": {
            "frozen_case_dir": str(frozen_dir),
            "prepared_manifest_path": str(prepared_path),
            "prepared_manifest_sha256": sha256(prepared_path),
            "historical_asr_results_path": str(historical_path),
            "historical_asr_results_sha256": sha256(historical_path),
            "case_result_count": len(ledger),
            "case_role_counts": {key: sum(row["role"] == key for row in ledger) for key in sorted(EXPECTED_ROLE_COUNTS)},
            "other_only_cases": sum(row["role"] == "other_only" for row in ledger),
            "explicit_fixed_gain_soft_target_cases": sum(row["soft_level_challenge"] for row in ledger),
            "normal_target_mixture_cases": sum(row["role"] == "target_mixture" for row in ledger),
            "source_speakers": [3, 7, 12, 17, 20],
            "future_quarantined_holdout_used": False,
        },
        "operating_path": {
            "inference_mode": "one complete utterance per model call after capture",
            "model_input_bytes": "exact existing raw PCM16 ASR baseline WAV; no per-candidate normalization or resampling",
            "sample_rate_conversion_exercised": False,
            "model_input_rate_hz": SAMPLE_RATE,
            "candidate_output_rate_hz": SAMPLE_RATE,
            "valid_samples_policy": "preserve exact input sample count; crop only model-produced right tail or right-pad zeros if shorter",
            "state_reset": "one independent full model forward call per case; no reusable recurrent chunk state API exercised",
            "latency_claim": "none; whole-utterance CPU helper timing is not <=200 ms drain, live streaming, or product stop-to-insert latency",
            "overlap_capability": "target-speaker extraction model; this smoke measures actual target-conditioned outputs on fixed public cases",
        },
        "case_counts": {
            "total": len(ledger),
            "other_only": sum(row["role"] == "other_only" for row in ledger),
            "other_only_all_zero_output": sum(row["role"] == "other_only" and row["candidate_output_all_zero"] for row in ledger),
            "other_only_nonzero_output": sum(row["role"] == "other_only" and not row["candidate_output_all_zero"] for row in ledger),
            "explicit_soft_target": sum(row["soft_level_challenge"] for row in ledger),
            "soft_target_all_zero_output": sum(row["soft_level_challenge"] and row["candidate_output_all_zero"] for row in ledger),
            "normal_target_mixture": sum(row["role"] == "target_mixture" for row in ledger),
            "target_containing_all_zero_output": sum(row["role"] != "other_only" and row["candidate_output_all_zero"] for row in ledger),
            "unique_new_asr_audio_hashes": len(unique_new),
        },
        "timing": {
            "per_case_full_utterance_seconds": times,
            "median_seconds": float(np.median(list(times.values()))),
            "p95_observed_seconds": float(np.percentile(list(times.values()), 95)),
            "median_real_time_factor": float(np.median([times[row["case_id"]] / row["model_input_duration_seconds"] for row in ledger])),
            "scope": "single-process CPU full-utterance model forward only; excludes model load, enrollment audio load, Qwen ASR, recorder stop, stream drain, and cursor insert",
        },
        "cases": ledger,
        "asr_new_audio_manifest_path": str(run_root / "asr-new-audio-manifest.json"),
        "asr_new_audio_manifest": {"cases": list(unique_new.values())},
        "limitations": [
            "Only 18 fixed public-dev counterexamples are used: 12 other-only, three normal target mixtures, and three explicitly marked fixed-gain soft target-only cases.",
            "Publisher transcript annotations remain project-review pending; nonempty transcript checks are not CER/WER acceptance.",
            "No new 18-speaker acquisition cohort or quarantined future holdout was used.",
            "The official Python path processes full utterances; causal model configuration alone does not establish an incremental stateful streaming API.",
            "Input was already 16 kHz PCM16, so 48 kHz down/up-sampling and production timeline retiming were not exercised.",
            "No microphone, AirPods, live enrollment, GUI, Screen Recording, cleanup, cloud, app integration, cursor delivery, or post-stop latency path was exercised.",
            "Model-weight license is not declared by the official README/archive; do not redistribute or ship until cleared.",
        ],
    }
    write_json(run_root / "asr-new-audio-manifest.json", report["asr_new_audio_manifest"])
    write_json(run_root / "pilot-report.json", report)
    print(json.dumps({"status": "complete", "report": str(run_root / "pilot-report.json"),
                      "cases": len(ledger), "case_counts": report["case_counts"],
                      "role_summary": role_summary, "new_asr_hashes": len(unique_new)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
