#!/usr/bin/env python3
"""Run a private, bounded Nomo-PVAD whole-utterance discovery pilot.

Nomo-PVAD produces target-speaker probabilities, not separated audio. This
pilot tests a deliberately limited stop-time route: if any 160 ms detector
frame reaches the upstream recommended default threshold (0.70), preserve the
entire original 16 kHz PCM16 utterance; otherwise create an equal-duration
zero PCM16 file. It cannot remove another speaker during a target-positive
utterance and is not a production filter implementation.

No microphone, cloud calls, training, ASR, or cleanup are used here.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import platform
import shutil
import sys
import subprocess
import time
import wave

import numpy as np

PINNED_CODE_COMMIT = "cb828468d183424fd574984f00b3ffbcce0be9bc"
PINNED_CHECKPOINT_BYTES = 36_576_677
PINNED_CHECKPOINT_SHA256 = "0acf2f3e75409b172771663fd1bee31af2760dba4e25b1353d2c26726b6412d1"
PINNED_ENCODER_BYTES = 71_768_231
PINNED_ENCODER_SHA256 = "0eb4057106b2573dd7b132cf0c36273ab29afd192c1610f80baa9c556dbb963c"
SR = 16_000
CHUNK_SAMPLES = 2_560
FROZEN_THRESHOLD = 0.70
MAX_CASES = 18


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_private_json(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def read_pcm16(path: Path) -> tuple[np.ndarray, bytes, int]:
    if path.is_symlink():
        raise ValueError(f"refusing symlinked audio: {path}")
    with wave.open(str(path), "rb") as audio:
        params = (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype())
        if params != (1, 2, SR, "NONE"):
            raise ValueError(f"expected 16 kHz mono PCM16 WAV, got {params}: {path}")
        raw = audio.readframes(audio.getnframes())
        if len(raw) != audio.getnframes() * 2:
            raise ValueError(f"truncated PCM16 WAV: {path}")
    samples_i16 = np.frombuffer(raw, dtype="<i2")
    samples_f32 = (samples_i16.astype(np.float32) / np.float32(32768.0)).copy()
    return samples_f32, raw, len(samples_i16)


def write_silence_wav(path: Path, frame_count: int) -> None:
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(SR)
        audio.setcomptype("NONE", "not compressed")
        audio.writeframes(b"\x00\x00" * frame_count)
    os.chmod(path, 0o600)


def qwen_rows_by_case_id(path: Path) -> dict[str, dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("model_id") != "qwen3-asr-0.6b" or data.get("revision") != "eae2b51f96265328f1e7beced788adb0e4536f92":
        raise ValueError("historical ASR result is not the pinned local Qwen3-ASR 0.6B run")
    if data.get("cloud_calls") != 0 or data.get("microphone_used") is not False:
        raise ValueError("historical ASR result violates the local-only pilot constraints")
    rows = {}
    for row in data.get("rows", []):
        case = row.get("case", {})
        if case.get("id"):
            rows[case["id"]] = row
    return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-dir", type=Path, required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--encoder-dir", type=Path, required=True)
    parser.add_argument("--frozen-case-dir", type=Path, required=True,
                        help="Existing frozen-counterexample-g0-v1 directory")
    parser.add_argument("--prepared-manifest", type=Path, required=True)
    parser.add_argument("--historical-asr-results", type=Path, required=True)
    parser.add_argument("--candidate-root", type=Path, required=True,
                        help="New private candidate run folder; it must not exist")
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()

    upstream = args.upstream_dir.resolve(strict=True)
    checkpoint = args.checkpoint.resolve(strict=True)
    encoder_dir = args.encoder_dir.resolve(strict=True)
    frozen_dir = args.frozen_case_dir.resolve(strict=True)
    prepared_manifest_path = args.prepared_manifest.resolve(strict=True)
    historical_asr_path = args.historical_asr_results.resolve(strict=True)
    candidate_root = args.candidate_root.resolve()
    if candidate_root.exists():
        raise SystemExit(f"candidate run directory already exists: {candidate_root}")
    candidate_root.mkdir(parents=True, mode=0o700)
    os.chmod(candidate_root, 0o700)

    actual_commit = subprocess.run(
        ["git", "-C", str(upstream), "rev-parse", "HEAD"],
        check=True,
        text=True,
        capture_output=True,
    ).stdout.strip()
    if actual_commit != PINNED_CODE_COMMIT:
        raise ValueError(f"upstream source commit mismatch: {actual_commit}")
    if checkpoint.stat().st_size != PINNED_CHECKPOINT_BYTES or sha256(checkpoint) != PINNED_CHECKPOINT_SHA256:
        raise ValueError("Nomo-PVAD model checkpoint does not match the pinned release hash")
    encoder = encoder_dir / "pretrained_eres2netv2.ckpt"
    if encoder.is_symlink() or encoder.stat().st_size != PINNED_ENCODER_BYTES or sha256(encoder) != PINNED_ENCODER_SHA256:
        raise ValueError("ERes2NetV2 enrollment checkpoint does not match the pinned ModelScope revision")

    # Use the exact candidate source checkout and explicit local encoder path.
    sys.path.insert(0, str(upstream))
    from nomo_pvad import NomoPVAD, NomoPVADSession

    prepared = json.loads(prepared_manifest_path.read_text(encoding="utf-8"))
    profile_inputs = {}
    for profile in prepared.get("profiles", []):
        path = Path(profile["input_audio_path"]).resolve(strict=True)
        if sha256(path) != profile["input_audio_sha256"]:
            raise ValueError(f"pinned enrollment source hash mismatch: {path}")
        profile_inputs[profile["profile_id"]] = {
            "path": path,
            "sha256": profile["input_audio_sha256"],
            "speaker_id": profile["source_speaker_id"],
            "samples": profile["input_samples"],
        }

    historical_rows = qwen_rows_by_case_id(historical_asr_path)
    case_result_paths = sorted((frozen_dir / "case-results").glob("*.json"))
    if len(case_result_paths) != MAX_CASES:
        raise ValueError(f"expected exactly {MAX_CASES} frozen counterexamples, found {len(case_result_paths)}")
    cases = [json.loads(path.read_text(encoding="utf-8")) for path in case_result_paths]
    # Frozen source manifests call all target-containing clips target_mixture;
    # the three per language with the explicit soft-level flag form this
    # pilot's soft-target guard cohort.
    for case in cases:
        if case.get("role") == "target_mixture" and case.get("soft_level_challenge") is True:
            case["source_role"] = case["role"]
            case["role"] = "target_soft_level"
    expected_roles = {"other_only": 12, "target_mixture": 3, "target_soft_level": 3}
    role_counts: dict[str, int] = {}
    for case in cases:
        role_counts[case["role"]] = role_counts.get(case["role"], 0) + 1
    if role_counts != expected_roles:
        raise ValueError(f"expected canonical 12 other-only + 3 target-mixture + 3 target-soft labels, found {role_counts}")
    soft_target_count = sum(case.get("soft_level_challenge") is True for case in cases)
    if soft_target_count != 3:
        raise ValueError(f"expected 3 explicitly flagged soft target cases among the 6 target-containing cases, found {soft_target_count}")
    for case in cases:
        if case.get("status") != "complete":
            raise ValueError(f"frozen counterexample is not complete: {case.get('case_id')}")
        case_id = case["case_id"]
        raw_asr = historical_rows.get(f"{case_id}-raw")
        g0_asr = historical_rows.get(f"{case_id}-g0")
        if raw_asr is None or g0_asr is None:
            raise ValueError(f"missing pinned raw/G0 Qwen rows for {case_id}")
        raw_audio_path = Path(raw_asr["case"]["audio_path"]).resolve(strict=True)
        g0_audio_path = Path(g0_asr["case"]["audio_path"]).resolve(strict=True)
        if sha256(raw_audio_path) != raw_asr["audio_sha256"] or sha256(g0_audio_path) != g0_asr["audio_sha256"]:
            raise ValueError(f"historical raw/G0 audio hash mismatch for {case_id}")
        case["historical_raw_asr"] = raw_asr
        case["historical_g0_asr"] = g0_asr
        case["raw_asr_audio_path"] = str(raw_audio_path)
        case["raw_asr_audio_sha256"] = raw_asr["audio_sha256"]
        case["g0_asr_audio_sha256"] = g0_asr["audio_sha256"]

    start_load = time.perf_counter()
    model = NomoPVAD(str(checkpoint), str(encoder_dir), device="cpu")
    model_load_seconds = time.perf_counter() - start_load
    profiles = {}
    for profile_id, item in profile_inputs.items():
        samples, _raw, n = read_pcm16(item["path"])
        if n != item["samples"]:
            raise ValueError(f"enrollment sample count mismatch for {profile_id}")
        session = NomoPVADSession(model)
        started = time.perf_counter()
        session.set_enrollment(samples)
        profiles[profile_id] = {
            "embedding": session.spk_emb.detach().cpu().numpy().astype(np.float32).reshape(-1),
            "enrollment_elapsed_seconds": time.perf_counter() - started,
            "enrollment_source_sha256": item["sha256"],
            "source_speaker_id": item["speaker_id"],
            "source_samples": n,
        }

    outputs_dir = candidate_root / "audio"
    outputs_dir.mkdir(mode=0o700)
    ledger_cases = []
    unique_novel_audio: dict[str, dict] = {}
    inference_times = []
    for case in cases:
        case_id = case["case_id"]
        raw_path = Path(case["raw_asr_audio_path"])
        wav, raw_pcm_bytes, sample_count = read_pcm16(raw_path)
        if sha256(raw_path) != case["raw_asr_audio_sha256"]:
            raise ValueError(f"raw baseline input changed after admission: {case_id}")
        profile_id = case["profile_id"]
        profile = profiles[profile_id]
        session = NomoPVADSession(model)
        session.set_enrollment_emb(profile["embedding"])
        begin = time.perf_counter()
        probabilities = np.asarray(session.score_utterance(wav), dtype=np.float32)
        scoring_seconds = time.perf_counter() - begin
        inference_times.append(scoring_seconds)
        if probabilities.size == 0 or not np.isfinite(probabilities).all():
            raise ValueError(f"invalid Nomo-PVAD probabilities for {case_id}")
        if (probabilities < 0).any() or (probabilities > 1).any():
            raise ValueError(f"out-of-range Nomo-PVAD probability for {case_id}")
        target_frames = np.flatnonzero(probabilities >= FROZEN_THRESHOLD)
        pass_raw = bool(target_frames.size)

        out_path = outputs_dir / f"{case_id}.wav"
        if pass_raw:
            shutil.copyfile(raw_path, out_path)
        else:
            write_silence_wav(out_path, sample_count)
        os.chmod(out_path, 0o600)
        out_hash = sha256(out_path)
        with wave.open(str(out_path), "rb") as output_wav:
            output_duration = output_wav.getnframes() / output_wav.getframerate()
            if output_wav.getnframes() != sample_count or output_wav.getframerate() != SR:
                raise ValueError(f"candidate output changed duration or rate: {case_id}")
            output_size = output_wav.getnframes() * output_wav.getsampwidth()
        raw_sha = case["raw_asr_audio_sha256"]
        if pass_raw and out_hash != raw_sha:
            raise ValueError(f"pass-through candidate output is not byte-identical to raw input: {case_id}")

        old_same_audio = [r for r in historical_rows.values() if r.get("audio_sha256") == out_hash]
        row = {
            "case_id": case_id,
            "role": case["role"],
            "language": case["language"],
            "soft_level_challenge": bool(case.get("soft_level_challenge", False)),
            "profile_id": profile_id,
            "profile_source_speaker_id": profile["source_speaker_id"],
            "input_path": str(raw_path),
            "input_sha256": raw_sha,
            "input_samples": sample_count,
            "input_duration_seconds": sample_count / SR,
            "target_probability_threshold": FROZEN_THRESHOLD,
            "threshold_source": "Nomo-PVAD v1.1 README default; fixed before this pilot",
            "frame_interval_ms": 160,
            "frame_count": int(probabilities.size),
            "max_probability": float(probabilities.max()),
            "min_probability": float(probabilities.min()),
            "mean_probability": float(probabilities.mean()),
            "frames_at_or_above_threshold": int(target_frames.size),
            "positive_frame_indices": target_frames.astype(int).tolist(),
            "decision": "pass_full_raw" if pass_raw else "zero_full_utterance",
            "is_target_speaker_detected": pass_raw,
            "output_path": str(out_path),
            "output_sha256": out_hash,
            "output_bytes": output_size,
            "output_samples": sample_count,
            "output_duration_seconds": output_duration,
            "output_equals_raw_exactly": out_hash == raw_sha,
            "model_score_seconds": scoring_seconds,
            "baseline_raw_asr_case_id": f"{case_id}-raw",
            "baseline_raw_asr_audio_sha256": raw_sha,
            "baseline_g0_asr_case_id": f"{case_id}-g0",
            "baseline_g0_asr_audio_sha256": case["g0_asr_audio_sha256"],
            "exact_prior_asr_audio_match_available": bool(old_same_audio),
            "prior_asr_audio_match_case_ids": [r["case"]["id"] for r in old_same_audio],
            "publisher_reference": case.get("publisher_reference"),
            "reference_project_review_status": case.get("reference_project_review_status", "pending"),
        }
        ledger_cases.append(row)
        if not old_same_audio:
            unique_novel_audio.setdefault(out_hash, {
                "id": f"nomo-filter-{out_hash[:16]}",
                "audio_path": str(out_path),
                "audio_sha256": out_hash,
                "reference": case.get("publisher_reference", ""),
                "language": case["language"],
                "condition": "nomo_whole_utterance_gate",
                "role": case["role"],
                "case_ids": [],
            })["case_ids"].append(case_id)

    ledger_cases.sort(key=lambda row: row["case_id"])
    run_report = {
        "artifact_type": "voiceflow_nomo_pvad_whole_utterance_pilot",
        "schema_version": 1,
        "run_id": args.run_id,
        "status": "complete",
        "decision_scope": "bounded_public_counterexample_discovery_smoke_only",
        "candidate": "Nomo-PVAD streaming target-speaker detector; not target-speaker extraction",
        "source": {
            "repository": "https://github.com/tuya/nomo-pvad",
            "tag": "release-1.1",
            "annotated_tag_sha": "27a9bf9fca92ad78aa297ec5e06f3cd14a5f76cc",
            "commit": PINNED_CODE_COMMIT,
            "license": "Apache-2.0",
            "checkpoint_bytes": PINNED_CHECKPOINT_BYTES,
            "checkpoint_sha256": PINNED_CHECKPOINT_SHA256,
            "checkpoint_license": "Apache-2.0; fine-tuned from 3D-Speaker ERes2NetV2 under upstream declaration",
        },
        "enrollment_encoder": {
            "model_id": "iic/speech_eres2netv2_sv_zh-cn_16k-common",
            "revision": "v1.0.2",
            "license": "Apache-2.0 (official ModelScope model card)",
            "checkpoint_bytes": PINNED_ENCODER_BYTES,
            "checkpoint_sha256": PINNED_ENCODER_SHA256,
            "sample_rate_hz": SR,
            "speaker_embedding_dimensions": 192,
        },
        "runtime": {
            "python": platform.python_version(),
            "platform": platform.platform(),
            "machine": platform.machine(),
            "device": "cpu",
            "torch": importlib.metadata.version("torch"),
            "torchaudio": importlib.metadata.version("torchaudio"),
            "numpy": importlib.metadata.version("numpy"),
            "modelscope": importlib.metadata.version("modelscope"),
            "modelscope_cache_private": bool(os.environ.get("MODELSCOPE_CACHE")),
        },
        "operating_point": {
            "threshold": FROZEN_THRESHOLD,
            "threshold_source": "official upstream README default, selected before inspecting this run",
            "rule": "pass the entire original audio if any 160 ms score >= 0.70; otherwise zero the entire equal-duration output",
            "output_sample_rate_hz": SR,
            "output_timeline": "byte-identical raw passthrough or equal-duration zero PCM16; no per-frame trims",
            "overlap_capability": "none: target-positive utterances remain raw and can still contain other speakers",
            "cross_language_scope": "detector/code advertises 16 kHz target detection; enrollment speaker backbone is Chinese-trained; EN/mixed generalization is exploratory",
        },
        "input_artifacts": {
            "frozen_case_dir": str(frozen_dir),
            "frozen_case_result_count": len(cases),
            "prepared_manifest_path": str(prepared_manifest_path),
            "prepared_manifest_sha256": sha256(prepared_manifest_path),
            "historical_asr_results_path": str(historical_asr_path),
            "historical_asr_results_sha256": sha256(historical_asr_path),
        },
        "case_counts": {
            "total": len(ledger_cases),
            "other_only": sum(r["role"] == "other_only" for r in ledger_cases),
            "target_mixture": sum(r["role"] == "target_mixture" for r in ledger_cases),
            "target_soft_level": sum(r["soft_level_challenge"] for r in ledger_cases),
            "other_only_zeroed": sum(r["role"] == "other_only" and r["decision"] == "zero_full_utterance" for r in ledger_cases),
            "other_only_passed_raw": sum(r["role"] == "other_only" and r["decision"] == "pass_full_raw" for r in ledger_cases),
            "target_containing_zeroed": sum(r["role"] != "other_only" and r["decision"] == "zero_full_utterance" for r in ledger_cases),
            "target_containing_passed_raw": sum(r["role"] != "other_only" and r["decision"] == "pass_full_raw" for r in ledger_cases),
            "soft_target_zeroed": sum(r["soft_level_challenge"] and r["decision"] == "zero_full_utterance" for r in ledger_cases),
            "soft_target_passed_raw": sum(r["soft_level_challenge"] and r["decision"] == "pass_full_raw" for r in ledger_cases),
            "unique_new_asr_audio_hashes": len(unique_novel_audio),
        },
        "timing": {
            "model_load_seconds": model_load_seconds,
            "profile_enrollment_seconds": {key: value["enrollment_elapsed_seconds"] for key, value in profiles.items()},
            "per_case_model_score_seconds": {row["case_id"]: row["model_score_seconds"] for row in ledger_cases},
            "model_score_seconds_p50": float(np.median(inference_times)),
            "model_score_seconds_p95_observed": float(np.percentile(inference_times, 95)),
            "scope": "single-process CPU helper-only timing; not production stop-to-insert",
        },
        "profiles": {
            key: {name: value for name, value in info.items() if name != "embedding"}
            for key, info in profiles.items()
        },
        "cases": ledger_cases,
        "asr_manifest_path": str(candidate_root / "asr-new-audio-manifest.json"),
        "asr_new_audio_manifest": {"cases": list(unique_novel_audio.values())},
        "limitations": [
            "A target-speaker detector does not produce a separated signal.",
            "Target-positive cases pass the complete raw utterance, so overlap and nearby speakers remain audible and can enter ASR.",
            "Zeroing an utterance without a target score can reject the user's entire utterance; the six soft cases are an explicit guard.",
            "The public corpus has three target speakers and two distinct interfering source speakers; this is not a population estimate or locked holdout.",
            "Publisher references remain project-review pending; this is not formal CER/WER acceptance.",
            "No microphone, AirPods, live enrollment, cleanup, app integration, GUI, cloud, or cursor delivery was exercised.",
        ],
    }
    write_private_json(candidate_root / "asr-new-audio-manifest.json", run_report["asr_new_audio_manifest"])
    write_private_json(candidate_root / "pilot-report.json", run_report)
    print(json.dumps({"status": "complete", "report": str(candidate_root / "pilot-report.json"),
                      "cases": len(ledger_cases), "counts": run_report["case_counts"],
                      "new_asr_audio_hashes": len(unique_novel_audio)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
