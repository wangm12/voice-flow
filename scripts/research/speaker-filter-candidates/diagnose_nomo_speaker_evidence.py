#!/usr/bin/env python3
"""Development-only NomoPVAD and enrollment-cosine evidence diagnostic.

The script first authenticates the complete existing 90-case development
bundle and pinned local model files. It does not change candidate outputs,
set a speaker-verification threshold, call ASR/cleanup, or create profiles.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from dataclasses import dataclass
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import platform
import re
import resource
import statistics
import subprocess
import sys
import tempfile
import time
import wave
from typing import Any


RESEARCH_ROOT = Path("/Users/mingjie/Documents/voice-dictation-research")
SPEAKER_ROOT = RESEARCH_ROOT / "speaker-filter/2026-10-25"
PREPARED_DIR = SPEAKER_ROOT / "prepared-v5"
PREPARED_MANIFEST = PREPARED_DIR / "manifest.json"
FINAL_SUMMARY = SPEAKER_ROOT / "final-pilot-summary-v2.json"
G0_DIR = SPEAKER_ROOT / "g0-full-v1"
G0_REPORT = G0_DIR / "pilot-report.json"
G0_HEADER = G0_DIR / "run-header.json"
G0_CASE_RESULTS = G0_DIR / "case-results"
ASR_ROOT = SPEAKER_ROOT / "asr-pairs-fixed-all-v1"
ASR_MANIFEST = ASR_ROOT / "asr-pairs-manifest.json"
ASR_PROVENANCE = ASR_ROOT / "asr-pairs-provenance.json"
ASR_CONVERSION_REPORT = ASR_ROOT / "asr-pairs-conversion-report.json"
ASR_RESULTS = ASR_ROOT / "asr-pair-results.json"

NOMO_ROOT = RESEARCH_ROOT / "production-goal/2026-10-25/candidates/nomo-pvad-1.1"
NOMO_REPO = NOMO_ROOT / "upstream/nomo-pvad"
NOMO_PVAD_WEIGHTS = NOMO_REPO / "weights/nomo_pvad.pt"
ENCODER_DIR = NOMO_ROOT / "encoder"
ENCODER_WEIGHTS = ENCODER_DIR / "pretrained_eres2netv2.ckpt"
NOMO_VENV = NOMO_ROOT / "venv"
PRIVATE_ROOT = RESEARCH_ROOT / "production-goal/2026-10-25/candidates/nomo-evidence-diagnostic-v1"

PINNED_SHA256 = {
    "final_summary": "0559c72e0a39cc3a50227fe6581847e14096fceae7c2ba164e844be89465d693",
    "prepared_manifest": "9f487a00132addab06ed2ef0166b7564e2390c4996f2a5c67428fed84328a631",
    "g0_report": "c33b1e47decf9cf620fb72b14eb040b2e80423b684371aa4c70e78dbec4d17b8",
    "g0_run_header": "e3065bc047f91a97ad7e9be7064a44e7d54d2b5790055ae3afbf810015833996",
    "asr_manifest": "807b7f50dea377f628946a2cc9e1d1eed409e9c0a30f83515a9a853892cdcf2a",
    "asr_provenance": "57728c4dd592bb4524ca7e23875eea1fbd66fc9e57d995ba25a91816812ff6bb",
    "asr_conversion_report": "6b32e679b92ab81ab81916c6e815c8065931c62d4e916fbdd0f32e75e33a0f16",
    "asr_results": "ed41e69499f72958dab8fcdeac1a20ed8a2d51df576a616d06cbbb142b4aefb0",
    "nomo_pvad_weights": "0acf2f3e75409b172771663fd1bee31af2760dba4e25b1353d2c26726b6412d1",
    "encoder_weights": "0eb4057106b2573dd7b132cf0c36273ab29afd192c1610f80baa9c556dbb963c",
}
NOMO_COMMIT = "cb828468d183424fd574984f00b3ffbcce0be9bc"
ENCODER_MODEL_ID = "iic/speech_eres2netv2_sv_zh-cn_16k-common"
ENCODER_REVISION = "v1.0.2"
ENCODER_BYTES = 71_768_231
NOMO_VERSION = "1.1"
HISTORICAL_ASR_MODEL_ID = "qwen3-asr-0.6b"
HISTORICAL_ASR_REVISION = "eae2b51f96265328f1e7beced788adb0e4536f92"
SR = 16_000
CHUNK_SAMPLES = 2_560
PVAD_DOCUMENTED_DEFAULT_THRESHOLD = 0.70
MAX_JSON_BYTES = 64 * 1024 * 1024
MAX_AUDIO_BYTES = 32 * 1024 * 1024
EXPECTED_ROLE_COUNTS = {
    "target_mixture": 66,
    "target_soft_level": 6,
    "other_only": 12,
    "negative_silence": 3,
    "negative_synthetic_noise": 3,
}
EXPECTED_LANGUAGE_ROLE_COUNTS = {
    ("zh", "target_mixture"): 22,
    ("en", "target_mixture"): 22,
    ("mixed", "target_mixture"): 22,
    ("zh", "target_soft_level"): 2,
    ("en", "target_soft_level"): 2,
    ("mixed", "target_soft_level"): 2,
    ("zh", "other_only"): 4,
    ("en", "other_only"): 4,
    ("mixed", "other_only"): 4,
    ("other", "negative_silence"): 3,
    ("other", "negative_synthetic_noise"): 3,
}
ALLOWED_ROLES = frozenset(EXPECTED_ROLE_COUNTS)
ALLOWED_LANGUAGES = frozenset({"zh", "en", "mixed", "other"})
PROFILE_SPEAKERS = {f"public-ascend-speaker-{speaker}": speaker for speaker in (7, 12, 20)}
CASE_SPEAKER_RE = re.compile(r"(?:^|-)(?:spk)(\d+)(?:-|$)")


class DiagnosticError(ValueError):
    """A pinned input, source, model, or runtime identity is invalid."""


@dataclass(frozen=True)
class WavInfo:
    path: Path
    sha256: str
    channels: int
    sample_width: int
    sample_rate_hz: int
    frames: int
    data_bytes: int
    pcm_sha256: str
    peak_abs_pcm16: int
    rms_pcm16: float


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def bounded_json(path: Path, *, expected_sha256: str | None = None) -> tuple[dict[str, Any], str]:
    if path.is_symlink():
        raise DiagnosticError(f"refusing symlinked JSON: {path}")
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or resolved.stat().st_size < 2 or resolved.stat().st_size > MAX_JSON_BYTES:
        raise DiagnosticError(f"JSON file missing or outside size bound: {resolved}")
    actual = sha256_file(resolved)
    if expected_sha256 is not None and actual != expected_sha256:
        raise DiagnosticError(f"SHA-256 mismatch for {resolved}: expected {expected_sha256}, got {actual}")
    try:
        value = json.loads(resolved.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DiagnosticError(f"invalid JSON {resolved}: {exc}") from exc
    if not isinstance(value, dict):
        raise DiagnosticError(f"expected JSON object: {resolved}")
    return value, actual


def check_sha_file(path: Path, expected_sha256: object, *, label: str) -> str:
    if not isinstance(expected_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise DiagnosticError(f"{label} expected SHA-256 is malformed")
    if path.is_symlink():
        raise DiagnosticError(f"refusing symlinked {label}: {path}")
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or resolved.stat().st_size > MAX_AUDIO_BYTES * 8:
        raise DiagnosticError(f"{label} is not a bounded regular file: {resolved}")
    actual = sha256_file(resolved)
    if actual != expected_sha256:
        raise DiagnosticError(f"{label} SHA-256 mismatch: {resolved}")
    return actual


def inspect_pcm16_wav(path_value: object, expected_sha256: object, *, expected_frames: int | None = None) -> WavInfo:
    if not isinstance(path_value, (str, Path)) or not str(path_value) or len(str(path_value)) > 4096:
        raise DiagnosticError("WAV path must be a nonempty bounded string")
    path = Path(path_value)
    if path.is_symlink():
        raise DiagnosticError(f"refusing symlinked WAV: {path}")
    resolved = path.resolve(strict=True)
    size = resolved.stat().st_size
    if not resolved.is_file() or size < 44 or size > MAX_AUDIO_BYTES:
        raise DiagnosticError(f"WAV is not a bounded regular file: {resolved}")
    try:
        with wave.open(str(resolved), "rb") as stream:
            channels = stream.getnchannels()
            width = stream.getsampwidth()
            sample_rate = stream.getframerate()
            frames = stream.getnframes()
            compression = stream.getcomptype()
            if (channels, width, sample_rate, compression) != (1, 2, SR, "NONE"):
                raise DiagnosticError(
                    f"expected canonical mono PCM16 16 kHz WAV; got {(channels, width, sample_rate, compression)}: {resolved}"
                )
            if frames <= 0 or (expected_frames is not None and frames != expected_frames):
                raise DiagnosticError(f"WAV frame count mismatch: {resolved} has {frames}, expected {expected_frames}")
            pcm = stream.readframes(frames)
            if len(pcm) != frames * channels * width:
                raise DiagnosticError(f"WAV header claims PCM bytes unavailable in file: {resolved}")
    except (wave.Error, EOFError, OSError) as exc:
        if isinstance(exc, DiagnosticError):
            raise
        raise DiagnosticError(f"invalid WAV: {resolved}: {exc}") from exc
    digest = sha256_file(resolved)
    if not isinstance(expected_sha256, str) or digest != expected_sha256:
        raise DiagnosticError(f"WAV SHA-256 mismatch: {resolved}")
    # The small integer pass avoids retaining PCM or embeddings in the report.
    samples = memoryview(pcm).cast("h")
    peak = max((abs(int(value)) for value in samples), default=0)
    squares = sum(int(value) * int(value) for value in samples)
    rms = math.sqrt(squares / len(samples)) if samples else 0.0
    return WavInfo(resolved, digest, channels, width, sample_rate, frames, len(pcm), hashlib.sha256(pcm).hexdigest(), peak, rms)


def _require_int(value: object, label: str, *, minimum: int = 0) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise DiagnosticError(f"{label} must be an integer >= {minimum}")
    return value


def _require_string(value: object, label: str, *, maximum: int = 4096) -> str:
    if not isinstance(value, str) or not value or len(value) > maximum:
        raise DiagnosticError(f"{label} must be a nonempty bounded string")
    return value


def canonical_case_identity(case: dict[str, Any], profiles: dict[str, dict[str, Any]]) -> dict[str, Any]:
    case_id = _require_string(case.get("case_id"), "case_id", maximum=256)
    role = _require_string(case.get("role"), f"{case_id} role", maximum=64)
    language = _require_string(case.get("language"), f"{case_id} language", maximum=32)
    if role not in ALLOWED_ROLES or language not in ALLOWED_LANGUAGES:
        raise DiagnosticError(f"unsupported canonical role/language for {case_id}: {role}/{language}")
    profile_id = _require_string(case.get("profile_id"), f"{case_id} profile_id", maximum=128)
    profile = profiles.get(profile_id)
    if profile is None or profile_id not in PROFILE_SPEAKERS:
        raise DiagnosticError(f"case references a non-pinned enrollment profile: {case_id}")
    profile_speaker_id = PROFILE_SPEAKERS[profile_id]
    if case.get("profile_source_speaker_id") != profile_speaker_id:
        raise DiagnosticError(f"profile id/source speaker mismatch: {case_id}")
    if profile.get("source_speaker_id") != profile_speaker_id:
        raise DiagnosticError(f"prepared profile source-speaker mismatch: {profile_id}")
    recipe = case.get("mix_recipe")
    if not isinstance(recipe, dict):
        raise DiagnosticError(f"missing source recipe: {case_id}")
    speaker_id: int | None
    source_kind: str
    if role in {"target_mixture", "target_soft_level"}:
        speaker_id = _require_int(case.get("target_speaker_id"), f"{case_id} target_speaker_id", minimum=1)
        if speaker_id != profile_speaker_id or recipe.get("target_speaker_id") != speaker_id:
            raise DiagnosticError(f"target-speaker/profile assignment graft: {case_id}")
        target_source_id = case.get("target_source_case_id")
        if target_source_id != recipe.get("target_source_case_id") or not isinstance(target_source_id, str):
            raise DiagnosticError(f"target source case identity mismatch: {case_id}")
        if (case.get("target_source_audio_sha256") != recipe.get("target_source_audio_sha256")
                or case.get("target_source_audio_path") != recipe.get("target_source_path")
                or not isinstance(case.get("target_source_audio_sha256"), str)):
            raise DiagnosticError(f"target source audio identity mismatch: {case_id}")
        target_component = case.get("target_component")
        if not isinstance(target_component, dict):
            raise DiagnosticError(f"target mixture components are malformed: {case_id}")
        if role == "target_mixture" and not isinstance(recipe.get("interferer"), dict):
            raise DiagnosticError(f"target mixture lacks its canonical interferer: {case_id}")
        if role == "target_soft_level" and recipe.get("interferer") is not None:
            raise DiagnosticError(f"soft-level target case has an unexpected interferer: {case_id}")
        source_kind = "target_speaker"
    elif role == "other_only":
        component = recipe.get("interferer_component")
        if not isinstance(component, dict) or recipe.get("target_component") is not None:
            raise DiagnosticError(f"other-only recipe has an unexpected target component: {case_id}")
        speaker_id = _require_int(case.get("source_speaker_id"), f"{case_id} source speaker_id", minimum=1)
        if component.get("source_speaker_id") != speaker_id:
            raise DiagnosticError(f"other-only component/source speaker mismatch: {case_id}")
        if (case.get("source_case_id") != component.get("source_case_id")
                or case.get("source_parent_block_id") != component.get("source_parent_block_id")):
            raise DiagnosticError(f"other-only source identity mismatch: {case_id}")
        if speaker_id in PROFILE_SPEAKERS.values():
            raise DiagnosticError(f"other-only case overlaps a registered temporary speaker: {case_id}")
        case_match = CASE_SPEAKER_RE.search(case_id)
        if case_match is None or int(case_match.group(1)) != speaker_id:
            raise DiagnosticError(f"other-only case ID/source speaker mismatch: {case_id}")
        source_kind = "interferer_speaker"
    else:
        speaker_id = None
        if language != "other" or recipe.get("target_component") is not None or recipe.get("interferer_component") is not None:
            raise DiagnosticError(f"negative case source/language flags mismatch: {case_id}")
        source_kind = "no_speaker_negative"
    soft_expected = role == "target_soft_level"
    if role in {"target_mixture", "target_soft_level"} and case.get("soft_level_challenge") is not soft_expected:
        raise DiagnosticError(f"canonical soft-level flag mismatch: {case_id}")
    if role == "other_only" and case.get("soft_level_challenge") not in (False, None):
        raise DiagnosticError(f"other-only case has a soft-target flag: {case_id}")
    return {
        "case_id": case_id,
        "role": role,
        "language": language,
        "profile_id": profile_id,
        "profile_speaker_id": profile_speaker_id,
        "source_speaker_id": speaker_id,
        "source_speaker_kind": source_kind,
        "soft_level_challenge": case.get("soft_level_challenge"),
    }


def verify_model_assets() -> dict[str, Any]:
    version_file = NOMO_REPO / "VERSION"
    if version_file.read_text(encoding="utf-8").strip() != NOMO_VERSION:
        raise DiagnosticError("Nomo repository VERSION differs from pinned 1.1")
    commit = subprocess.run(
        ["git", "-C", str(NOMO_REPO), "rev-parse", "HEAD"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    if commit != NOMO_COMMIT:
        raise DiagnosticError(f"Nomo source revision mismatch: {commit}")
    dirty = subprocess.run(
        ["git", "-C", str(NOMO_REPO), "status", "--porcelain=v1"],
        check=True, capture_output=True, text=True,
    ).stdout
    if dirty:
        raise DiagnosticError("pinned Nomo upstream checkout is dirty")
    checksums = (NOMO_REPO / "weights/CHECKSUMS.txt").read_text(encoding="utf-8")
    if f"{PINNED_SHA256['nomo_pvad_weights']}  nomo_pvad.pt" not in checksums:
        raise DiagnosticError("Nomo weight checksum is not declared in its pinned source")
    nomo_sha = check_sha_file(NOMO_PVAD_WEIGHTS, PINNED_SHA256["nomo_pvad_weights"], label="Nomo PVAD weights")
    encoder_receipt, _ = bounded_json(ENCODER_DIR / "acquisition.json")
    if (encoder_receipt.get("model_id") != ENCODER_MODEL_ID
            or encoder_receipt.get("revision") != ENCODER_REVISION
            or encoder_receipt.get("file") != ENCODER_WEIGHTS.name
            or encoder_receipt.get("sha256") != PINNED_SHA256["encoder_weights"]
            or encoder_receipt.get("bytes") != ENCODER_BYTES):
        raise DiagnosticError("pinned ERes2NetV2 encoder acquisition identity mismatch")
    encoder_sha = check_sha_file(ENCODER_WEIGHTS, PINNED_SHA256["encoder_weights"], label="ERes2NetV2 enrollment weights")
    if ENCODER_WEIGHTS.stat().st_size != ENCODER_BYTES:
        raise DiagnosticError("ERes2NetV2 encoder byte size differs from its pinned receipt")
    expected_prefix = NOMO_VENV.resolve()
    if Path(sys.prefix).resolve() != expected_prefix:
        raise DiagnosticError(f"must run with the pre-existing candidate venv at {expected_prefix}; got {sys.prefix}")
    versions = {}
    for package in ("numpy", "torch", "torchaudio", "modelscope", "soundfile"):
        try:
            versions[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            versions[package] = None
    if versions["torch"] != "2.11.0" or versions["torchaudio"] != "2.11.0" or versions["modelscope"] != "1.40.2":
        raise DiagnosticError(f"candidate venv dependency pins mismatch: {versions}")
    return {
        "nomo_version": NOMO_VERSION,
        "nomo_git_commit": commit,
        "nomo_git_clean": True,
        "nomo_pvad_weights": {"path": str(NOMO_PVAD_WEIGHTS), "sha256": nomo_sha, "bytes": NOMO_PVAD_WEIGHTS.stat().st_size},
        "enrollment_encoder": {
            "modelscope_id": ENCODER_MODEL_ID,
            "revision": ENCODER_REVISION,
            "path": str(ENCODER_WEIGHTS),
            "sha256": encoder_sha,
            "bytes": ENCODER_WEIGHTS.stat().st_size,
            "license_claim_in_existing_receipt": encoder_receipt.get("license"),
        },
        "runtime": {
            "python": platform.python_version(),
            "sys_prefix": str(expected_prefix),
            "packages": versions,
            "pcm16_reader": "stdlib wave + numpy int16-to-float32 /32768; soundfile is absent and no dependency was installed",
            "device": "cpu",
            "torch_threads": 1,
            "torch_interop_threads": 1,
        },
    }


def _pinned_summary_inputs(summary: dict[str, Any]) -> None:
    receipts = summary.get("receipts")
    if not isinstance(receipts, dict):
        raise DiagnosticError("final pilot summary lacks artifact receipts")
    required = {
        "prepared_cases": PINNED_SHA256["prepared_manifest"],
        "g0": PINNED_SHA256["g0_report"],
        "g0_asr_pair_manifest": PINNED_SHA256["asr_manifest"],
        "local_asr_results": PINNED_SHA256["asr_results"],
    }
    for key, digest in required.items():
        item = receipts.get(key)
        if not isinstance(item, dict) or item.get("sha256") != digest:
            raise DiagnosticError(f"final pilot summary pin mismatch: {key}")
    pair = receipts["g0_asr_pair_manifest"]
    if pair.get("provenance_sha256") != PINNED_SHA256["asr_provenance"]:
        raise DiagnosticError("final summary does not bind the expected ASR provenance file")
    if pair.get("rows") != 180 or receipts["prepared_cases"].get("case_count") != 90:
        raise DiagnosticError("final summary has an unexpected development cohort size")


def _verify_completed_asr(row: dict[str, Any], *, expected_samples: int, label: str) -> bool:
    response = row.get("response")
    if not isinstance(response, dict):
        raise DiagnosticError(f"{label} response missing or malformed")
    if response.get("status") != "completed" or response.get("error") is not None:
        raise DiagnosticError(f"{label} is not a completed pinned local ASR response")
    if (response.get("op") != "transcribe"
            or response.get("loaded_model_id") != HISTORICAL_ASR_MODEL_ID
            or response.get("loaded_model_version") != HISTORICAL_ASR_REVISION
            or response.get("model_version") != HISTORICAL_ASR_REVISION
            or response.get("cleanup_result") is not None):
        raise DiagnosticError(f"{label} response model/operation identity mismatch")
    request_id = response.get("request_id")
    if not isinstance(request_id, str) or not request_id or len(request_id) > 256:
        raise DiagnosticError(f"{label} request identity missing or malformed")
    result = response.get("result")
    if not isinstance(result, dict) or result.get("completion") != "complete":
        raise DiagnosticError(f"{label} has no full-completion result")
    if result.get("covered_samples") != expected_samples:
        raise DiagnosticError(f"{label} ASR sample coverage does not match its source frames")
    text = result.get("text")
    if not isinstance(text, str):
        raise DiagnosticError(f"{label} ASR result text is not a string")
    return bool(text.strip())


def _preflight_asr_pair(case: dict[str, Any], provenance: dict[str, Any],
                        manifest_by_id: dict[str, dict[str, Any]],
                        result_by_id: dict[str, dict[str, Any]],
                        request_ids: set[str], expected_samples: int) -> dict[str, Any]:
    case_id = case["case_id"]
    pair_type = _require_string(provenance.get("pair_type"), f"{case_id} pair type", maximum=64)
    values: dict[str, Any] = {}
    for stage in ("raw", "g0"):
        row_id = f"{case_id}-{stage}"
        manifest_row = manifest_by_id.get(row_id)
        result_row = result_by_id.get(row_id)
        if manifest_row is None or result_row is None:
            raise DiagnosticError(f"fixed ASR evidence is missing a paired row for {row_id}")
        condition = f"{stage}-{pair_type}"
        audio_path = provenance.get("raw_pcm16_path" if stage == "raw" else "filtered_pcm16_path")
        audio_sha = provenance.get("raw_pcm16_audio_sha256" if stage == "raw" else "filtered_pcm16_audio_sha256")
        if (manifest_row.get("audio_path") != audio_path
                or manifest_row.get("audio_sha256") != audio_sha
                or manifest_row.get("condition") != condition
                or manifest_row.get("role") != case["role"]
                or manifest_row.get("language") != case["language"]):
            raise DiagnosticError(f"fixed ASR manifest identity mismatch: {row_id}")
        audio = inspect_pcm16_wav(audio_path, audio_sha, expected_frames=expected_samples)
        result_case = result_row.get("case")
        if (not isinstance(result_case, dict)
                or result_case.get("id") != row_id
                or result_case.get("audio_path") != audio_path
                or result_case.get("audio_sha256") != audio_sha
                or result_case.get("language") != case["language"]
                or result_case.get("role") != case["role"]
                or result_case.get("condition") != condition
                or result_row.get("audio_sha256") != audio_sha):
            raise DiagnosticError(f"fixed ASR result identity mismatch: {row_id}")
        response = result_row.get("response")
        if isinstance(response, dict):
            request_id = response.get("request_id")
            if isinstance(request_id, str) and request_id:
                if request_id in request_ids:
                    raise DiagnosticError(f"duplicate request ID in pinned historical ASR results: {request_id}")
                request_ids.add(request_id)
        nonempty = _verify_completed_asr(result_row, expected_samples=expected_samples, label=row_id)
        values[stage] = {"path": str(audio.path), "sha256": audio.sha256, "nonempty": nonempty}
    return values


def preflight_inputs() -> dict[str, Any]:
    summary, summary_sha = bounded_json(FINAL_SUMMARY, expected_sha256=PINNED_SHA256["final_summary"])
    if summary.get("decision") != "no_go_for_production":
        raise DiagnosticError("candidate diagnostic requires the pinned historical no-go summary")
    _pinned_summary_inputs(summary)
    manifest, manifest_sha = bounded_json(PREPARED_MANIFEST, expected_sha256=PINNED_SHA256["prepared_manifest"])
    g0, g0_sha = bounded_json(G0_REPORT, expected_sha256=PINNED_SHA256["g0_report"])
    run_header, run_header_sha = bounded_json(G0_HEADER, expected_sha256=PINNED_SHA256["g0_run_header"])
    asr_manifest, asr_manifest_sha = bounded_json(ASR_MANIFEST, expected_sha256=PINNED_SHA256["asr_manifest"])
    asr_provenance, provenance_sha = bounded_json(ASR_PROVENANCE, expected_sha256=PINNED_SHA256["asr_provenance"])
    conversion_report, conversion_sha = bounded_json(ASR_CONVERSION_REPORT, expected_sha256=PINNED_SHA256["asr_conversion_report"])
    asr_results, asr_results_sha = bounded_json(ASR_RESULTS, expected_sha256=PINNED_SHA256["asr_results"])

    if manifest.get("artifact_type") != "voiceflow_speaker_filter_pilot_cases" or manifest.get("schema_version") != 1:
        raise DiagnosticError("unsupported prepared-v5 manifest schema")
    if g0.get("status") != "complete" or g0.get("recipe") != "g0" or g0.get("case_count") != 90 or g0.get("completed_cases") != 90:
        raise DiagnosticError("pinned G0 report is not the complete 90-case G0 source run")
    if run_header.get("manifest_sha256") != manifest_sha or run_header.get("recipe") != "g0" or run_header.get("case_ids") is None:
        raise DiagnosticError("pinned G0 run header does not bind prepared-v5")
    if conversion_report.get("source_run_report_sha256") != g0_sha or conversion_report.get("source_manifest_sha256") != manifest_sha:
        raise DiagnosticError("fixed ASR conversion report does not bind the pinned G0 and prepared source")
    if (asr_results.get("artifact_type") != "local_speaker_filter_native_asr_probe"
            or asr_results.get("manifest_sha256") != asr_manifest_sha
            or asr_results.get("model_id") != HISTORICAL_ASR_MODEL_ID
            or asr_results.get("revision") != HISTORICAL_ASR_REVISION
            or asr_results.get("cloud_calls") != 0
            or asr_results.get("microphone_used") is not False):
        raise DiagnosticError("historical raw/G0 ASR artifact identity is invalid")

    profiles_list = manifest.get("profiles")
    cases_list = manifest.get("cases")
    if not isinstance(profiles_list, list) or not isinstance(cases_list, list) or len(profiles_list) != 3 or len(cases_list) != 90:
        raise DiagnosticError("prepared-v5 must contain exactly its three temporary profiles and 90 cases")
    profiles: dict[str, dict[str, Any]] = {}
    profile_audio: dict[str, WavInfo] = {}
    for profile in profiles_list:
        if not isinstance(profile, dict):
            raise DiagnosticError("malformed prepared profile row")
        profile_id = _require_string(profile.get("profile_id"), "profile_id", maximum=128)
        if profile_id in profiles or profile_id not in PROFILE_SPEAKERS:
            raise DiagnosticError(f"unexpected or duplicate public profile: {profile_id}")
        speaker_id = PROFILE_SPEAKERS[profile_id]
        if profile.get("source_speaker_id") != speaker_id or profile.get("profile_kind") != "temporary_public_pilot_voiceprint_not_formal_user_profile":
            raise DiagnosticError(f"temporary enrollment profile identity mismatch: {profile_id}")
        profile_path = Path(_require_string(profile.get("input_audio_path"), f"{profile_id} path"))
        if PREPARED_DIR not in profile_path.parents or "enrollment-inputs" not in profile_path.parts:
            raise DiagnosticError(f"profile audio path is outside the pinned prepared enrollment directory: {profile_id}")
        frames = _require_int(profile.get("input_samples"), f"{profile_id} input_samples", minimum=1)
        audio = inspect_pcm16_wav(profile_path, profile.get("input_audio_sha256"), expected_frames=frames)
        if profile.get("input_duration_seconds") is None or abs(profile["input_duration_seconds"] - frames / SR) > 1 / SR:
            raise DiagnosticError(f"profile duration metadata mismatch: {profile_id}")
        profiles[profile_id] = profile
        profile_audio[profile_id] = audio
    if set(profiles) != set(PROFILE_SPEAKERS):
        raise DiagnosticError("temporary profile set differs from the pinned three-speaker allowlist")

    cases: list[dict[str, Any]] = []
    case_by_id: dict[str, dict[str, Any]] = {}
    input_audio: dict[str, WavInfo] = {}
    observed_role_counts: Counter[str] = Counter()
    observed_language_role_counts: Counter[tuple[str, str]] = Counter()
    for case in cases_list:
        if not isinstance(case, dict):
            raise DiagnosticError("malformed prepared case row")
        identity = canonical_case_identity(case, profiles)
        case_id = identity["case_id"]
        if case_id in case_by_id:
            raise DiagnosticError(f"duplicate prepared case identity: {case_id}")
        recipe = case.get("mix_recipe")
        if not isinstance(recipe, dict):
            raise DiagnosticError(f"missing prepared audio recipe: {case_id}")
        input_path = Path(_require_string(recipe.get("input_audio_path"), f"{case_id} input path"))
        if PREPARED_DIR not in input_path.parents or "inputs" not in input_path.parts:
            raise DiagnosticError(f"case input path is outside the pinned prepared-v5 inputs: {case_id}")
        audio_hash = _require_string(recipe.get("input_audio_sha256"), f"{case_id} input hash", maximum=64)
        if recipe.get("sample_rate_hz", SR) != SR:
            raise DiagnosticError(f"case source rate is not the pinned 16 kHz timeline: {case_id}")
        expected_frames_value = recipe.get("input_samples")
        expected_frames = _require_int(expected_frames_value, f"{case_id} input_samples", minimum=1) if expected_frames_value is not None else None
        audio = inspect_pcm16_wav(input_path, audio_hash, expected_frames=expected_frames)
        if expected_frames is not None and audio.frames != expected_frames:
            raise DiagnosticError(f"case sample count differs from the canonical recipe: {case_id}")
        if identity["role"] in {"target_mixture", "target_soft_level"}:
            pcm_sha = _require_string(recipe.get("input_pcm_sha256"), f"{case_id} input PCM hash", maximum=64)
            if audio.pcm_sha256 != pcm_sha:
                raise DiagnosticError(f"prepared target input PCM digest mismatch: {case_id}")
            source_audio = inspect_pcm16_wav(
                case.get("target_source_audio_path"), case.get("target_source_audio_sha256"),
                expected_frames=_require_int(case.get("target_component", {}).get("source_samples"),
                                             f"{case_id} target source samples", minimum=1))
            if source_audio.path.parent != (PREPARED_DIR / "source-audio"):
                raise DiagnosticError(f"target source audio is outside the prepared source-audio directory: {case_id}")
        elif identity["role"] == "other_only":
            if case.get("source_audio_sha256") != audio.sha256 or case.get("source_pcm_sha256") != audio.pcm_sha256:
                raise DiagnosticError(f"other-only source and prepared audio hashes diverge: {case_id}")
            source_audio = inspect_pcm16_wav(case.get("source_path"), case.get("source_audio_sha256"), expected_frames=audio.frames)
            if source_audio.pcm_sha256 != case.get("source_pcm_sha256"):
                raise DiagnosticError(f"other-only source PCM hash mismatch: {case_id}")
        else:
            if case.get("prepared_input_path") != str(input_path) or case.get("prepared_input_sha256") != audio.sha256:
                raise DiagnosticError(f"negative prepared input identity mismatch: {case_id}")
            source_audio = inspect_pcm16_wav(case.get("source_path"), case.get("source_audio_sha256"), expected_frames=audio.frames)
            if source_audio.sha256 != audio.sha256:
                raise DiagnosticError(f"negative source and prepared input hashes diverge: {case_id}")
        case_header = {**identity, "input_audio_path": str(audio.path), "input_audio_sha256": audio.sha256,
                       "input_samples": audio.frames, "input_sample_rate_hz": audio.sample_rate_hz,
                       "input_pcm_sha256": audio.pcm_sha256, "input_peak_pcm16": audio.peak_abs_pcm16,
                       "input_rms_pcm16": audio.rms_pcm16,
                       "prepared_case": case}
        case_by_id[case_id] = case_header
        input_audio[case_id] = audio
        cases.append(case_header)
        observed_role_counts[identity["role"]] += 1
        observed_language_role_counts[(identity["language"], identity["role"])] += 1

    if set(run_header["case_ids"]) != set(case_by_id) or len(run_header["case_ids"]) != 90:
        raise DiagnosticError("G0 run-header case IDs do not exactly match prepared-v5")
    if dict(observed_role_counts) != EXPECTED_ROLE_COUNTS:
        raise DiagnosticError(f"prepared-v5 role counts differ from its frozen expected counts: {dict(observed_role_counts)}")
    if dict(observed_language_role_counts) != EXPECTED_LANGUAGE_ROLE_COUNTS:
        raise DiagnosticError("prepared-v5 language/role counts differ from the frozen candidate pool")

    g0_profiles = g0.get("profile_results")
    if not isinstance(g0_profiles, dict) or set(g0_profiles) != set(profiles):
        raise DiagnosticError("G0 profile results do not match the three prepared temporary profiles")
    g0_profile_hashes: dict[str, str] = {}
    for profile_id, report in g0_profiles.items():
        if not isinstance(report, dict) or report.get("status") != "complete":
            raise DiagnosticError(f"G0 profile artifact is incomplete: {profile_id}")
        profile_path = Path(_require_string(report.get("profile_path"), f"{profile_id} G0 profile path"))
        profile_hash = check_sha_file(profile_path, report.get("profile_sha256"), label=f"G0 profile {profile_id}")
        g0_profile_hashes[profile_id] = profile_hash

    provenance_cases = asr_provenance.get("cases")
    manifest_rows = asr_manifest.get("cases")
    result_rows = asr_results.get("rows")
    if not isinstance(provenance_cases, list) or len(provenance_cases) != 90:
        raise DiagnosticError("fixed ASR provenance must contain all 90 canonical cases")
    if not isinstance(manifest_rows, list) or len(manifest_rows) != 180 or not isinstance(result_rows, list) or len(result_rows) != 180:
        raise DiagnosticError("fixed ASR manifest/results must contain exactly the 180 raw/G0 pairs")
    provenance_by_id = {row.get("case_id"): row for row in provenance_cases if isinstance(row, dict)}
    manifest_by_id = {row.get("id"): row for row in manifest_rows if isinstance(row, dict)}
    result_by_id = {row.get("case", {}).get("id"): row for row in result_rows
                     if isinstance(row, dict) and isinstance(row.get("case"), dict)}
    if len(provenance_by_id) != 90 or len(manifest_by_id) != 180 or len(result_by_id) != 180:
        raise DiagnosticError("fixed ASR artifacts contain duplicate or malformed canonical IDs")
    all_request_ids: set[str] = set()
    raw_nonempty = 0
    g0_nonempty = 0
    for case_id, case_header in case_by_id.items():
        case = case_header["prepared_case"]
        prov = provenance_by_id.get(case_id)
        if not isinstance(prov, dict):
            raise DiagnosticError(f"fixed ASR source provenance is missing: {case_id}")
        if (prov.get("role") != case_header["role"]
                or prov.get("language") != case_header["language"]
                or prov.get("raw_input_path") != case_header["input_audio_path"]
                or prov.get("raw_input_sha256") != case_header["input_audio_sha256"]
                or prov.get("profile_id") != case_header["profile_id"]
                or prov.get("profile_sha256") != g0_profile_hashes[case_header["profile_id"]]):
            raise DiagnosticError(f"fixed ASR provenance does not bind prepared source/profile: {case_id}")
        result_path = G0_CASE_RESULTS / f"{case_id}.json"
        result, _result_file_sha = bounded_json(result_path)
        if (result.get("case_id") != case_id
                or result.get("recipe") != "g0"
                or result.get("status") != "complete"
                or result.get("role") != case_header["role"]
                or result.get("language") != case_header["language"]
                or result.get("profile_id") != case_header["profile_id"]
                or result.get("input_path") != case_header["input_audio_path"]
                or result.get("input_sha256") != case_header["input_audio_sha256"]
                or result.get("input_samples") != case_header["input_samples"]
                or result.get("profile_sha256") != g0_profile_hashes[case_header["profile_id"]]):
            raise DiagnosticError(f"pinned G0 case result does not bind prepared input/profile: {case_id}")
        g0_output_path = Path(_require_string(result.get("output_path"), f"{case_id} G0 output path"))
        g0_output_hash = check_sha_file(g0_output_path, result.get("output_sha256"), label=f"{case_id} G0 float output")
        if (prov.get("native_float_output_path") != str(g0_output_path)
                or prov.get("native_float_output_sha256") != g0_output_hash
                or result.get("output_samples") != case_header["input_samples"]):
            raise DiagnosticError(f"G0 output does not bind its ASR provenance or preserve source samples: {case_id}")
        pair_identities = _preflight_asr_pair(
            case, prov, manifest_by_id, result_by_id, all_request_ids, case_header["input_samples"])
        if pair_identities["raw"]["nonempty"]:
            raw_nonempty += 1
        if pair_identities["g0"]["nonempty"]:
            g0_nonempty += 1
        case_header["historical_raw_asr_nonempty"] = pair_identities["raw"]["nonempty"]
        case_header["historical_g0_asr_nonempty"] = pair_identities["g0"]["nonempty"]
        case_header["historical_raw_asr_audio_sha256"] = pair_identities["raw"]["sha256"]
        case_header["historical_g0_asr_audio_sha256"] = pair_identities["g0"]["sha256"]
        case_header["g0_case_result_sha256"] = _result_file_sha

    if len(all_request_ids) != 180:
        raise DiagnosticError(f"historical raw/G0 ASR request identities are not unique and complete: {len(all_request_ids)}")

    model_identity = verify_model_assets()
    file_identities = {
        "final_pilot_summary": {"path": str(FINAL_SUMMARY), "sha256": summary_sha},
        "prepared_manifest": {"path": str(PREPARED_MANIFEST), "sha256": manifest_sha},
        "g0_report": {"path": str(G0_REPORT), "sha256": g0_sha},
        "g0_run_header": {"path": str(G0_HEADER), "sha256": run_header_sha},
        "historical_asr_manifest": {"path": str(ASR_MANIFEST), "sha256": asr_manifest_sha},
        "historical_asr_provenance": {"path": str(ASR_PROVENANCE), "sha256": provenance_sha},
        "historical_asr_conversion_report": {"path": str(ASR_CONVERSION_REPORT), "sha256": conversion_sha},
        "historical_asr_results": {"path": str(ASR_RESULTS), "sha256": asr_results_sha},
    }
    roles = {role: sum(c["role"] == role for c in cases) for role in sorted(ALLOWED_ROLES)}
    languages = {language: sum(c["language"] == language for c in cases) for language in sorted(ALLOWED_LANGUAGES)}
    return {
        "artifact_type": "voiceflow_nomo_speaker_evidence_preflight",
        "schema_version": 1,
        "status": "preflight_passed_no_model_loaded",
        "source_inputs": file_identities,
        "model_identity": model_identity,
        "case_count": len(cases),
        "role_counts": roles,
        "language_counts": languages,
        "language_role_counts": {f"{language}/{role}": count for (language, role), count in sorted(observed_language_role_counts.items())},
        "case_ids": [case["case_id"] for case in cases],
        "profile_identities": {
            profile_id: {
                "source_speaker_id": PROFILE_SPEAKERS[profile_id],
                "path": str(profile_audio[profile_id].path),
                "sha256": profile_audio[profile_id].sha256,
                "samples": profile_audio[profile_id].frames,
                "sample_rate_hz": profile_audio[profile_id].sample_rate_hz,
                "profile_kind": "temporary_public_pilot_voiceprint_not_formal_user_profile",
            }
            for profile_id in sorted(profile_audio)
        },
        "audio_policy": {
            "required_format": "mono PCM16 16 kHz, actual frame bytes checked",
            "resampling": "none; any rate mismatch rejects admission",
            "nomo_chunk_samples": CHUNK_SAMPLES,
            "nomo_chunk_duration_ms": 160,
            "tail_policy": "pad only the final partial chunk to 2560 samples, following pinned infer.py; bound reported interval end to valid source samples",
        },
        "historical_asr_baseline": {
            "model_id": asr_results["model_id"],
            "revision": asr_results["revision"],
            "rows": 180,
            "unique_request_ids": len(all_request_ids),
            "raw_nonempty": raw_nonempty,
            "g0_nonempty": g0_nonempty,
            "cloud_calls": 0,
            "microphone_used": False,
            "asr_rerun": False,
        },
        "case_records": cases,
        "source_limitations": [
            "These 90 rows are existing public-corpus development cases only; the separate 18-case quarantined source expansion is not opened or scored.",
            "Publisher speaker labels and references remain unreviewed as ground truth; source publication and license do not establish speaker consent or verified identity.",
            "The three enrollment inputs are temporary public pilot profiles, not user registrations or identity credentials.",
        ],
    }


def _summarize(values: list[float]) -> dict[str, Any]:
    if not values:
        return {"n": 0, "min": None, "p10": None, "p25": None, "median": None, "p75": None, "p90": None, "max": None}
    ordered = sorted(float(value) for value in values)

    def quantile(q: float) -> float:
        pos = (len(ordered) - 1) * q
        low = math.floor(pos)
        high = math.ceil(pos)
        if low == high:
            return ordered[low]
        return ordered[low] + (ordered[high] - ordered[low]) * (pos - low)

    return {
        "n": len(ordered),
        "min": ordered[0],
        "p10": quantile(0.10),
        "p25": quantile(0.25),
        "median": statistics.median(ordered),
        "p75": quantile(0.75),
        "p90": quantile(0.90),
        "max": ordered[-1],
    }


def _positive_intervals(probabilities: list[float], valid_samples: int) -> list[dict[str, Any]]:
    intervals: list[dict[str, Any]] = []
    start: int | None = None
    for index, probability in enumerate(probabilities):
        is_positive = probability >= PVAD_DOCUMENTED_DEFAULT_THRESHOLD
        if is_positive and start is None:
            start = index * CHUNK_SAMPLES
        if start is not None and (not is_positive or index == len(probabilities) - 1):
            end_chunk = index if not is_positive else index + 1
            end = min(end_chunk * CHUNK_SAMPLES, valid_samples)
            if end > start:
                intervals.append({
                    "start_sample": start,
                    "end_sample_exclusive": end,
                    "start_seconds": round(start / SR, 6),
                    "end_seconds_exclusive": round(end / SR, 6),
                    "duration_seconds": round((end - start) / SR, 6),
                })
            start = None
    return intervals


def _read_f32_16k(path: Path, expected_sha256: str, expected_samples: int):
    # Use a bounded PCM16 decoder because the pre-existing candidate venv lacks
    # the soundfile reader used by upstream infer.py. This preserves the fixed PCM source
    # rate and sample timeline without installing a dependency or resampling.
    import numpy as np
    if sha256_file(path) != expected_sha256:
        raise DiagnosticError(f"Nomo inference input changed after preflight: {path}")
    try:
        with wave.open(str(path), "rb") as stream:
            metadata = (stream.getnchannels(), stream.getsampwidth(), stream.getframerate(), stream.getnframes(), stream.getcomptype())
            if metadata != (1, 2, SR, expected_samples, "NONE"):
                raise DiagnosticError(f"Nomo input is not canonical PCM16/16k: {path} metadata={metadata}")
            pcm = stream.readframes(expected_samples)
    except (wave.Error, EOFError, OSError) as exc:
        raise DiagnosticError(f"invalid Nomo input WAV: {path}: {exc}") from exc
    if len(pcm) != expected_samples * 2:
        raise DiagnosticError(f"Nomo input decoded byte count mismatch: {path}")
    samples = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / np.float32(32768.0)
    if samples.ndim != 1 or len(samples) != expected_samples:
        raise DiagnosticError(f"decoded Nomo input shape mismatch: {path}")
    if not np.isfinite(samples).all():
        raise DiagnosticError(f"decoded Nomo input contains non-finite samples: {path}")
    return np.ascontiguousarray(samples, dtype=np.float32)


def _smoke_ids(cases: list[dict[str, Any]]) -> list[str]:
    primary_roles = ("target_mixture", "target_soft_level", "other_only")
    selected: dict[tuple[str, str], dict[str, Any]] = {}
    for case in cases:
        key = (case["language"], case["role"])
        if case["role"] in primary_roles and key not in selected:
            selected[key] = case
    picked = list(selected.values())
    chosen_profiles = {case["profile_id"] for case in picked}
    for profile_id in sorted(PROFILE_SPEAKERS):
        if profile_id in chosen_profiles:
            continue
        candidate = next((c for c in cases if c["profile_id"] == profile_id and c["role"] == "target_mixture"), None)
        if candidate is not None:
            picked.append(candidate)
            chosen_profiles.add(profile_id)
    for role in ("negative_silence", "negative_synthetic_noise"):
        candidate = next((c for c in cases if c["role"] == role), None)
        if candidate is not None:
            picked.append(candidate)
    # Preserve canonical manifest order in the output.
    ids = {case["case_id"] for case in picked}
    return [case["case_id"] for case in cases if case["case_id"] in ids]


def _case_group_speaker(case: dict[str, Any]) -> str | None:
    speaker_id = case.get("source_speaker_id")
    return str(speaker_id) if speaker_id is not None else None


def _group_summaries(rows: list[dict[str, Any]]) -> tuple[dict[str, Any], dict[str, Any]]:
    groups: dict[tuple[str, str, str | None, str], list[dict[str, Any]]] = defaultdict(list)
    target_other: dict[tuple[str, str], dict[str, list[float]]] = defaultdict(lambda: {"target": [], "other_only": []})
    for row in rows:
        key = (row["language"], row["role"], row.get("source_speaker_id"), row["profile_id"])
        groups[key].append(row)
        score = row.get("sv_cosine_to_assigned_profile")
        if isinstance(score, (int, float)) and math.isfinite(float(score)):
            target_other[(row["language"], row["profile_id"])][
                "other_only" if row["role"] == "other_only" else "target"
            ].append(float(score))
    summaries = {}
    for (language, role, speaker_id, profile_id), members in sorted(groups.items(), key=lambda item: tuple(str(x) for x in item[0])):
        sv = [float(row["sv_cosine_to_assigned_profile"]) for row in members
              if isinstance(row.get("sv_cosine_to_assigned_profile"), (int, float))]
        posterior_max = [float(row["pvad_max_probability"]) for row in members
                         if isinstance(row.get("pvad_max_probability"), (int, float))]
        posterior_median = [float(row["pvad_median_probability"]) for row in members
                            if isinstance(row.get("pvad_median_probability"), (int, float))]
        summaries[f"{language}/{role}/speaker-{speaker_id or 'none'}/{profile_id}"] = {
            "cases": len(members),
            "scored_sv_cases": len(sv),
            "sv_cosine": _summarize(sv),
            "pvad_max_probability": _summarize(posterior_max),
            "pvad_median_probability": _summarize(posterior_median),
            "any_chunk_at_or_above_upstream_0_70": sum(bool(row.get("pvad_any_at_0_70")) for row in members),
            "total_positive_intervals_at_upstream_0_70": sum(len(row.get("pvad_positive_intervals", [])) for row in members),
            "unknown_or_failed_cases": sum(row.get("status") != "completed" for row in members),
        }
    overlap = {}
    for (language, profile_id), classes in sorted(target_other.items()):
        target = classes["target"]
        other = classes["other_only"]
        if not target or not other:
            overlap[f"{language}/{profile_id}"] = {
                "target_n": len(target), "other_only_n": len(other), "overlap_observable": False,
                "reason": "one class has no scored examples in this language/profile group",
            }
            continue
        target_range = (min(target), max(target))
        other_range = (min(other), max(other))
        low = max(target_range[0], other_range[0])
        high = min(target_range[1], other_range[1])
        overlap[f"{language}/{profile_id}"] = {
            "target_n": len(target),
            "other_only_n": len(other),
            "target_range": list(target_range),
            "other_only_range": list(other_range),
            "observed_range_intersection": [low, high] if low <= high else None,
            "target_points_inside_other_range": sum(other_range[0] <= value <= other_range[1] for value in target),
            "other_points_inside_target_range": sum(target_range[0] <= value <= target_range[1] for value in other),
            "overlap_observable": True,
            "interpretation": "descriptive min/max overlap only; small, source-correlated development groups; no decision threshold or population claim",
        }
    return summaries, overlap


def run_diagnostic(preflight: dict[str, Any], *, smoke: bool, wall_limit_seconds: float = 580.0) -> dict[str, Any]:
    # Thread controls are established before importing the Nomo package/torch.
    for variable in (
        "OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS", "VECLIB_MAXIMUM_THREADS",
        "NUMEXPR_NUM_THREADS",
    ):
        os.environ[variable] = "1"
    os.environ["MODELSCOPE_CACHE"] = str(PRIVATE_ROOT / "modelscope-cache")
    os.environ["MODELSCOPE_OFFLINE"] = "1"
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"

    sys.path.insert(0, str(NOMO_REPO))
    from nomo_pvad import CHUNK_SAMPLES as NOMO_CHUNK_SAMPLES, NomoPVAD, NomoPVADSession, SR as NOMO_SR
    import torch

    if NOMO_CHUNK_SAMPLES != CHUNK_SAMPLES or NOMO_SR != SR:
        raise DiagnosticError("pinned Nomo runtime framing differs from the validated 16 kHz/2560-sample contract")
    torch.set_num_threads(1)
    torch.set_num_interop_threads(1)
    started = time.monotonic()
    device = torch.device("cpu")
    model = NomoPVAD(str(NOMO_PVAD_WEIGHTS), str(ENCODER_DIR), device="cpu")
    model_loaded_at = time.monotonic()
    if Path(model._eres2netv2_dir).resolve() != ENCODER_DIR.resolve():
        raise DiagnosticError("Nomo attempted to use an encoder directory other than the explicitly pinned local directory")

    profiles = preflight["profile_identities"]
    sessions = {}
    profile_embeddings = {}
    for profile_id, info in profiles.items():
        enrollment = _read_f32_16k(Path(info["path"]), info["sha256"], info["samples"])
        session = NomoPVADSession(model)
        session.set_enrollment(enrollment)
        if session.spk_emb is None or tuple(session.spk_emb.shape) != (1, 192):
            raise DiagnosticError(f"set_enrollment did not produce a 1x192 profile vector: {profile_id}")
        if not torch.isfinite(session.spk_emb).all().item():
            raise DiagnosticError(f"set_enrollment produced a non-finite profile vector: {profile_id}")
        profile_norm = float(torch.linalg.vector_norm(session.spk_emb, dim=-1).item())
        if not math.isfinite(profile_norm) or abs(profile_norm - 1.0) > 1e-4:
            raise DiagnosticError(f"Nomo enrollment embedding is not L2-normalized: {profile_id} norm={profile_norm}")
        sessions[profile_id] = session
        profile_embeddings[profile_id] = session.spk_emb

    selected_ids = set(_smoke_ids(preflight["case_records"])) if smoke else set(preflight["case_ids"])
    # Keep the entire cohort in each output, including unscored rows outside a
    # bounded smoke, model errors, and wall-limit omissions.
    result_rows: dict[str, dict[str, Any]] = {}
    case_lookup = {item["case_id"]: item for item in preflight["case_records"]}
    for case_id in preflight["case_ids"]:
        case = case_lookup[case_id]
        if case_id not in selected_ids:
            result_rows[case_id] = {**_case_result_identity(case), "status": "not_run_outside_bounded_smoke"}
    deadline = started + wall_limit_seconds
    status = "complete_development_diagnostic" if not smoke else "bounded_development_smoke_complete"
    stop_reason = None
    for case_id in preflight["case_ids"]:
        if case_id not in selected_ids:
            continue
        if time.monotonic() >= deadline:
            status = "partial_wall_limit"
            stop_reason = "10-minute total run bound reached; remaining cases stay explicitly unscored"
            for pending_id in selected_ids:
                if pending_id not in result_rows:
                    result_rows[pending_id] = {**_case_result_identity(case_lookup[pending_id]), "status": "not_run_wall_limit"}
            break
        case = case_lookup[case_id]
        session = sessions[case["profile_id"]]
        try:
            waveform = _read_f32_16k(Path(case["input_audio_path"]), case["input_audio_sha256"], case["input_samples"])
            session.reset()
            inference_start = time.monotonic()
            cosine: float | None = None
            sv_status = "undefined_non_speech_negative" if case["role"] in {"negative_silence", "negative_synthetic_noise"} else "unknown"
            if case["role"] not in {"negative_silence", "negative_synthetic_noise"}:
                if case["input_peak_pcm16"] == 0:
                    sv_status = "unknown_zero_audio"
                else:
                    test_embedding = model.extract_emb(waveform)
                    if tuple(test_embedding.shape) != (1, 192) or not torch.isfinite(test_embedding).all().item():
                        raise DiagnosticError(f"test embedding shape/non-finite identity mismatch: {case_id}")
                    score_tensor = torch.nn.functional.cosine_similarity(
                        profile_embeddings[case["profile_id"]], test_embedding, dim=-1, eps=1e-6)
                    cosine = float(score_tensor.item())
                    if not math.isfinite(cosine) or not -1.00001 <= cosine <= 1.00001:
                        raise DiagnosticError(f"speaker cosine is outside [-1,1]: {case_id}")
                    sv_status = "scored_cosine_no_threshold"
            probabilities: list[float] = []
            valid_frame_counts: list[int] = []
            for offset in range(0, len(waveform), CHUNK_SAMPLES):
                valid = min(CHUNK_SAMPLES, len(waveform) - offset)
                chunk = waveform[offset:offset + CHUNK_SAMPLES]
                if valid < CHUNK_SAMPLES:
                    import numpy as np
                    chunk = np.pad(chunk, (0, CHUNK_SAMPLES - valid), mode="constant")
                probability = float(session.process_chunk(chunk))
                if not math.isfinite(probability) or not 0.0 <= probability <= 1.0:
                    raise DiagnosticError(f"Nomo returned a non-finite/out-of-range PVAD posterior: {case_id}")
                probabilities.append(probability)
                valid_frame_counts.append(valid)
            if sum(valid_frame_counts) != len(waveform) or len(probabilities) != math.ceil(len(waveform) / CHUNK_SAMPLES):
                raise DiagnosticError(f"Nomo chunk/tail coverage does not match valid source samples: {case_id}")
            intervals = _positive_intervals(probabilities, len(waveform))
            pvad_elapsed = time.monotonic() - inference_start
            rms_dbfs = (20.0 * math.log10(max(case["input_rms_pcm16"] / 32768.0, 1e-12)))
            result_rows[case_id] = {
                **_case_result_identity(case),
                "status": "completed",
                "assigned_profile_cosine_status": sv_status,
                "sv_cosine_to_assigned_profile": cosine,
                "sv_score_range": [-1.0, 1.0] if cosine is not None else None,
                "sv_score_semantics": "ERes2NetV2 embedding cosine, not probability and no threshold applied" if cosine is not None else None,
                "input_rms_dbfs": round(rms_dbfs, 4),
                "pvad_frame_count": len(probabilities),
                "pvad_valid_source_samples": len(waveform),
                "pvad_tail_padding_samples": (CHUNK_SAMPLES - len(waveform) % CHUNK_SAMPLES) % CHUNK_SAMPLES,
                "pvad_max_probability": max(probabilities) if probabilities else None,
                "pvad_median_probability": statistics.median(probabilities) if probabilities else None,
                "pvad_mean_probability": statistics.fmean(probabilities) if probabilities else None,
                "pvad_fraction_chunks_at_or_above_upstream_0_70": (
                    sum(value >= PVAD_DOCUMENTED_DEFAULT_THRESHOLD for value in probabilities) / len(probabilities)
                    if probabilities else None
                ),
                "pvad_any_at_0_70": any(value >= PVAD_DOCUMENTED_DEFAULT_THRESHOLD for value in probabilities),
                "pvad_positive_intervals": intervals,
                "positive_interval_source_seconds": round(sum(i["duration_seconds"] for i in intervals), 6),
                "compute_seconds_sv_and_pvad": round(pvad_elapsed, 6),
        "input_resampling": "none; exact source PCM16 16 kHz frames; decoded by stdlib wave/numpy because soundfile is absent in the existing venv",
                "session_state_reset_before_case": True,
                "enrollment_set_via_official_session_api": True,
                "posterior_frame_threshold": PVAD_DOCUMENTED_DEFAULT_THRESHOLD,
            }
        except Exception as exc:  # preserve the case denominator and continue safely
            try:
                session.reset()
            except Exception:
                pass
            result_rows[case_id] = {
                **_case_result_identity(case),
                "status": "failed_or_unknown",
                "assigned_profile_cosine_status": "unknown",
                "sv_cosine_to_assigned_profile": None,
                "pvad_max_probability": None,
                "pvad_median_probability": None,
                "pvad_any_at_0_70": None,
                "pvad_positive_intervals": [],
                "failure_type": type(exc).__name__,
                "failure_message": str(exc)[:500],
            }
    rows = [result_rows[case_id] for case_id in preflight["case_ids"] if case_id in result_rows]
    group_summary, overlap_summary = _group_summaries(rows)
    completed = sum(row.get("status") == "completed" for row in rows)
    scored_sv = sum(row.get("sv_cosine_to_assigned_profile") is not None for row in rows)
    output = {
        "artifact_type": "voiceflow_nomo_assigned_profile_speaker_evidence_diagnostic",
        "schema_version": 1,
        "status": status,
        "decision_scope": "development_only_diagnostic_no_threshold_or_gate",
        "model_identity": preflight["model_identity"],
        "source_input_identities": preflight["source_inputs"],
        "cohort": {
            "canonical_case_count": 90,
            "included_case_count": len(rows),
            "selected_for_scoring": len(selected_ids),
            "completed_cases": completed,
            "failed_or_unknown_cases": sum(row.get("status") == "failed_or_unknown" for row in rows),
            "not_run_cases": sum(str(row.get("status", "")).startswith("not_run") for row in rows),
            "scored_sv_cases": scored_sv,
            "role_counts": preflight["role_counts"],
            "language_role_counts": preflight["language_role_counts"],
            "no_case_dropped_from_report": True,
            "negative_sv_rows_are_undefined_not_valid_identity": True,
        },
        "runtime_observations": {
            "device": "cpu",
            "torch_threads": 1,
            "torch_interop_threads": 1,
            "model_load_seconds": round(model_loaded_at - started, 6),
            "total_elapsed_seconds": round(time.monotonic() - started, 6),
            "process_max_rss_bytes": _max_rss_bytes(),
            "thread_and_compute_times_are_diagnostic_only": True,
            "no_streaming_or_stop_to_drain_claim": True,
        },
        "sv_definition": {
            "encoder": "the exact ERes2NetV2 encoder exposed by the pinned Nomo `NomoPVAD.extract_emb` / `NomoPVADSession.set_enrollment` APIs",
            "assigned_profile_only": True,
            "score": "cosine similarity of L2-normalized 192-D embeddings; numeric range [-1,1]",
            "threshold": None,
            "threshold_reason": "No ERes2NetV2 SV decision threshold is applied or selected in this diagnostic. ModelScope's verification pipeline reads a model-specific yesOrno_thr from its model config; this cosine is descriptive only.",
            "negative_silence_and_synthetic_noise": "SV undefined; no embedding similarity is emitted for these no-speech negative roles",
            "raw_embedding_vectors_saved": False,
        },
        "pvad_definition": {
            "output": "per-160-ms P(target) posterior from NomoPVADSession.process_chunk",
            "probability_range": [0.0, 1.0],
            "interval_threshold": PVAD_DOCUMENTED_DEFAULT_THRESHOLD,
            "threshold_source": "NomoPVAD pinned upstream infer.py default; PVAD posterior threshold, not an SV cosine threshold",
            "posterior_intervals": "runs of posterior frames at or above 0.70, not verified speech boundaries; each score is computed over the upstream 2.4-second rolling buffer",
            "tail_padding_samples_per_case": "recorded per case; positive interval ends clipped to valid source sample count",
            "state_reset": "session sliding buffer reset before every case; assigned profile embedding remains set through official session API",
        },
        "descriptive_group_summary": group_summary,
        "target_vs_other_cosine_range_overlap_by_language_and_profile": overlap_summary,
        "cases": rows,
        "limitations": [
            "This is an exploratory development diagnostic on 90 existing public-voice cases and three temporary enrollment profiles, not a new locked test or a production gate.",
            "The case identities use publisher source metadata. Speaker labels and reference text are not human-audited ground truth, and public corpus availability/license does not establish speaker consent.",
            "There are only three temporary target profiles and two source interferer IDs; distribution overlap is descriptive and does not support confidence bounds, population independence, or user-identity claims.",
            "The cosine score shares the ERes2NetV2 enrollment representation used by Nomo's conditioning path; it is a separate scoring rule, not an independent acoustic model.",
            "A standalone cosine gate is not implemented. Whether score overlap suggests a research direction does not establish improved extraction or safe gating.",
            "No target filtering, zeroing, threshold search, ASR, cleanup, microphone, GUI, cloud, training, or product integration is performed.",
        ],
        "stop_reason": stop_reason,
    }
    serialized = json.dumps(output, ensure_ascii=False, separators=(",", ":"))
    if len(serialized.encode("utf-8")) > 100 * 1024 * 1024:
        raise DiagnosticError("diagnostic report would exceed the 100 MB output cap")
    return output


def _max_rss_bytes() -> int:
    value = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(value if sys.platform == "darwin" else value * 1024)


def _case_result_identity(case: dict[str, Any]) -> dict[str, Any]:
    return {
        "case_id": case["case_id"],
        "role": case["role"],
        "language": case["language"],
        "source_speaker_id": case.get("source_speaker_id"),
        "profile_id": case["profile_id"],
        "profile_source_speaker_id": case["profile_speaker_id"],
        "soft_level_challenge": case.get("soft_level_challenge"),
        "source_split": case["prepared_case"].get("source_split"),
        "project_partition_status": case["prepared_case"].get("project_partition_status"),
        "input_audio_path": case["input_audio_path"],
        "input_audio_sha256": case["input_audio_sha256"],
        "input_samples": case["input_samples"],
        "input_sample_rate_hz": SR,
        "historical_raw_asr_nonempty": case.get("historical_raw_asr_nonempty"),
        "historical_g0_asr_nonempty": case.get("historical_g0_asr_nonempty"),
        "historical_raw_asr_audio_sha256": case.get("historical_raw_asr_audio_sha256"),
        "historical_g0_asr_audio_sha256": case.get("historical_g0_asr_audio_sha256"),
        "g0_case_result_sha256": case.get("g0_case_result_sha256"),
    }


def write_json_private(path: Path, data: dict[str, Any], *, new_file: bool) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    try:
        path.parent.chmod(0o700)
    except OSError:
        pass
    raw = (json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    if len(raw) > 100 * 1024 * 1024:
        raise DiagnosticError("private output exceeds the 100 MB cap")
    if new_file and path.exists():
        raise DiagnosticError(f"refusing to overwrite existing diagnostic artifact: {path}")
    temp = path.with_name(path.name + f".tmp-{os.getpid()}")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    fd = os.open(temp, flags, 0o600)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        if not new_file and path.exists():
            os.replace(temp, path)
        else:
            os.link(temp, path)
            temp.unlink()
        path.chmod(0o600)
        directory_fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temp.exists():
            temp.unlink()


class DiagnosticContractTests:
    """Small no-model checks for identity, format, and negative-score rules."""

    def run(self) -> dict[str, Any]:
        checks = []
        with tempfile.TemporaryDirectory(prefix="nomo-evidence-contract-") as tmp:
            root = Path(tmp)
            valid_wav = root / "valid.wav"
            with wave.open(str(valid_wav), "wb") as stream:
                stream.setnchannels(1)
                stream.setsampwidth(2)
                stream.setframerate(SR)
                stream.writeframes(b"\x01\x00" * 32)
            valid_sha = sha256_file(valid_wav)
            assert inspect_pcm16_wav(valid_wav, valid_sha, expected_frames=32).frames == 32
            checks.append("canonical_PCM16_16k_audio_identity_passes")
            for bad_rate in (8_000, 48_000):
                bad_wav = root / f"bad-{bad_rate}.wav"
                with wave.open(str(bad_wav), "wb") as stream:
                    stream.setnchannels(1)
                    stream.setsampwidth(2)
                    stream.setframerate(bad_rate)
                    stream.writeframes(b"\x01\x00" * 32)
                try:
                    inspect_pcm16_wav(bad_wav, sha256_file(bad_wav))
                except DiagnosticError:
                    checks.append(f"noncanonical_{bad_rate}Hz_rejected_without_resampling")
                else:
                    raise AssertionError(f"unexpected sample-rate acceptance: {bad_rate}")
            try:
                inspect_pcm16_wav(valid_wav, "0" * 64)
            except DiagnosticError:
                checks.append("audio_digest_graft_rejected")
            else:
                raise AssertionError("unexpected audio digest acceptance")
            profile_rows = {
                name: {"source_speaker_id": speaker}
                for name, speaker in PROFILE_SPEAKERS.items()
            }
            source_path = str(root / "source.wav")
            source_sha = "a" * 64
            case = {
                "case_id": "target-en-spk7-00001", "role": "target_mixture", "language": "en",
                "profile_id": "public-ascend-speaker-7", "profile_source_speaker_id": 7,
                "target_speaker_id": 7, "soft_level_challenge": False,
                "target_component": {"source_path": source_path, "source_samples": 32},
                "target_source_audio_path": source_path, "target_source_audio_sha256": source_sha,
                "mix_recipe": {
                    "target_speaker_id": 7, "target_source_case_id": "ascend-validation-00001",
                    "target_source_path": source_path, "target_source_audio_sha256": source_sha,
                    "interferer": {"source_speaker_id": 3},
                },
                "target_source_case_id": "ascend-validation-00001",
            }
            canonical_case_identity(case, profile_rows)
            checks.append("canonical_profile_to_target_binding_passes")
            case["profile_id"] = "public-ascend-speaker-12"
            case["profile_source_speaker_id"] = 12
            try:
                canonical_case_identity(case, profile_rows)
            except DiagnosticError:
                checks.append("valid_profile_graft_rejected_by_case_source_identity")
            else:
                raise AssertionError("profile graft was accepted")
            case["profile_id"] = "public-ascend-speaker-7"
            case["profile_source_speaker_id"] = 7
            case["target_source_audio_sha256"] = "b" * 64
            try:
                canonical_case_identity(case, profile_rows)
            except DiagnosticError:
                checks.append("target_source_digest_graft_rejected")
            else:
                raise AssertionError("target source digest graft was accepted")
            case["target_source_audio_sha256"] = source_sha
            case["target_speaker_id"] = 12
            case["mix_recipe"]["target_speaker_id"] = 12
            try:
                canonical_case_identity(case, profile_rows)
            except DiagnosticError:
                checks.append("mismatched_case_and_recipe_speaker_rejected")
            else:
                raise AssertionError("speaker identity mismatch was accepted")
            other = {
                "case_id": "other-only-en-spk3-00001", "role": "other_only", "language": "en",
                "profile_id": "public-ascend-speaker-7", "profile_source_speaker_id": 7,
                "soft_level_challenge": None, "source_speaker_id": 3,
                "source_case_id": "ascend-test-00001", "source_parent_block_id": "ascend:test:spk3:ses1:x",
                "mix_recipe": {"target_component": None, "interferer_component": {
                    "source_speaker_id": 3, "source_case_id": "ascend-test-00001",
                    "source_parent_block_id": "ascend:test:spk3:ses1:x",
                }},
            }
            canonical_case_identity(other, profile_rows)
            checks.append("other_only_interferer_binding_passes")
            other["mix_recipe"]["interferer_component"]["source_speaker_id"] = 17
            try:
                canonical_case_identity(other, profile_rows)
            except DiagnosticError:
                checks.append("other_only_speaker_graft_rejected")
            else:
                raise AssertionError("other-only speaker graft was accepted")
            negative = {
                "case_id": "negative-silence-3s", "role": "negative_silence", "language": "other",
                "profile_id": "public-ascend-speaker-7", "profile_source_speaker_id": 7,
                "mix_recipe": {"target_component": None, "interferer_component": None},
            }
            identity = canonical_case_identity(negative, profile_rows)
            if identity["source_speaker_id"] is not None or identity["source_speaker_kind"] != "no_speaker_negative":
                raise AssertionError("negative case became a valid speaker identity")
            checks.append("negative_silence_has_no_valid_speaker_identity")
            try:
                check_sha_file(valid_wav, "f" * 64, label="test model weight")
            except DiagnosticError:
                checks.append("model_weight_digest_mismatch_rejected")
            else:
                raise AssertionError("model weight digest graft was accepted")
        return {"checks": checks, "passed": len(checks), "failed": 0}


def _emit_summary(report_path: Path, report: dict[str, Any]) -> None:
    result = {
        "status": report.get("status"),
        "report_path": str(report_path),
        "case_count": report.get("cohort", {}).get("canonical_case_count"),
        "selected": report.get("cohort", {}).get("selected_for_scoring"),
        "completed": report.get("cohort", {}).get("completed_cases"),
        "scored_sv": report.get("cohort", {}).get("scored_sv_cases"),
        "elapsed_seconds": report.get("runtime_observations", {}).get("total_elapsed_seconds"),
        "raw_embeddings_saved": report.get("sv_definition", {}).get("raw_embedding_vectors_saved"),
    }
    print(json.dumps(result, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--self-test", action="store_true", help="run bounded no-model contract checks")
    action.add_argument("--preflight-only", action="store_true", help="validate all 90 inputs/assets without loading a model")
    action.add_argument("--run", action="store_true", help="score all 90 canonical development cases")
    action.add_argument("--smoke", action="store_true", help="score a representative bounded development subset")
    parser.add_argument("--output", type=Path, help="new private JSON output path; must not already exist")
    args = parser.parse_args()
    if args.self_test:
        result = DiagnosticContractTests().run()
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return
    if not (args.preflight_only or args.run or args.smoke):
        args.preflight_only = True
    if args.output is None:
        parser.error("--output is required for preflight and diagnostic runs")
    if PRIVATE_ROOT not in args.output.resolve(strict=False).parents:
        parser.error(f"output must remain under the private diagnostic root {PRIVATE_ROOT}")
    try:
        preflight = preflight_inputs()
        if args.preflight_only:
            preflight_receipt = {key: value for key, value in preflight.items()
                                 if key not in {"case_records", "case_ids"}}
            preflight_receipt["case_id_set_sha256"] = hashlib.sha256(
                "\n".join(preflight["case_ids"]).encode("utf-8")).hexdigest()
            preflight_receipt["preflight_only"] = True
            write_json_private(args.output, preflight_receipt, new_file=True)
            _emit_summary(args.output, {"status": preflight_receipt["status"], "cohort": {"canonical_case_count": 90,
                         "selected_for_scoring": 0, "completed_cases": 0, "scored_sv_cases": 0},
                         "runtime_observations": {}, "sv_definition": {"raw_embedding_vectors_saved": False}})
            return
        # Freeze a first state before loading neural code. Replacing this exact
        # run file is allowed only after this process created it.
        initial = {
            "artifact_type": "voiceflow_nomo_assigned_profile_speaker_evidence_diagnostic",
            "schema_version": 1,
            "status": "preflight_passed_model_not_loaded",
            "preflight_sha256": hashlib.sha256(json.dumps(preflight, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest(),
            "case_count": 90,
            "raw_embeddings_saved": False,
        }
        write_json_private(args.output, initial, new_file=True)
        report = run_diagnostic(preflight, smoke=args.smoke, wall_limit_seconds=580.0)
        report["preflight_sha256"] = initial["preflight_sha256"]
        report["output_written_by_same_process"] = True
        write_json_private(args.output, report, new_file=False)
        _emit_summary(args.output, report)
    except (DiagnosticError, OSError, TypeError, KeyError, ValueError, RuntimeError) as exc:
        print(json.dumps({"status": "rejected_or_failed", "error_type": type(exc).__name__, "error": str(exc)[:1000]}, ensure_ascii=False), file=sys.stderr)
        raise SystemExit(2)


if __name__ == "__main__":
    main()
