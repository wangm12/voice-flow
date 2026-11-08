#!/usr/bin/env python3
"""Prepare and run a bounded, local-only VoiceFlow speaker-filter pilot.

This tool reads an already-acquired, pinned public corpus. It never downloads
corpus data, trains a model, invokes ASR, or sends audio to a network service.
All generated audio, profiles, weights, receipts, and reports must live outside
the VoiceFlow repository in a fresh private output directory.

The ASR text retained in the case manifest is publisher text with project
review pending. It is not project-reviewed gold and the corpus split is not a
project locked holdout. Public licensing does not establish speaker consent.
"""
from __future__ import annotations

import argparse
import csv
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import sys
import time
import wave


REPO = Path(__file__).resolve().parents[2]
DEFAULT_CORPUS = Path(
    "/Users/mingjie/Documents/voice-dictation-research/corpora/public-pilot/"
    "ascend-737e9800-v1"
)
DEFAULT_CANDIDATES = Path(
    "/Users/mingjie/Documents/voice-dictation-research/corpora/public-pilot/"
    "ascend-737e9800-pilot-candidates-v2"
)
DEFAULT_OUTPUT_ROOT = Path(
    "/Users/mingjie/Documents/voice-dictation-research/speaker-filter/2026-10-25"
)
SOURCE_URL = "https://huggingface.co/datasets/CAiRE/ASCEND"
SOURCE_REVISION = "737e9800ae31be9932ba8464c80366559bd28424"
SOURCE_LICENSE = "CC-BY-SA-4.0"
SOURCE_EXPECTED = {
    "main/validation-00000-of-00001.parquet": {
        "bytes": 106532266,
        "sha256": "3bdec53d2abfd3dd4f0d86a6df4e27e60f20660edc9b66055ae0ef8ec05cf7e2",
        "rows": 1130,
    },
    "main/test-00000-of-00001.parquet": {
        "bytes": 105756434,
        "sha256": "a4c81d2b5ed6124f052089a695972808c16e0ce0c365ec9773c5d1a8fcf043a7",
        "rows": 1315,
    },
}
LANGUAGES = ("zh", "en", "mixed")
CASE_ROLES = {
    "target_mixture",
    "target_soft_level",
    "other_only",
    "negative_silence",
    "negative_synthetic_noise",
}
CASE_LANGUAGES = {*LANGUAGES, "other"}
CASE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")
TARGET_SPEAKERS = (7, 12, 20)
OTHER_SPEAKERS = (3, 17)
SAMPLE_RATE = 16000
FRAME_SAMPLES = 160  # 10 ms at 16 kHz, used only for signal-presence summaries.
MIB = 1024 * 1024
MAX_CORPUS_BYTES = 2 * 1024 * 1024 * 1024
MAX_ROWS = 3000
MAX_CASES = 128
MAX_WAV_BYTES = 25 * MIB
MAX_JSON_BYTES = 8 * MIB
MAX_REPORT_BYTES = 256 * 1024
ASCEND_NAME = re.compile(
    r"ses(?P<session>\d+)_spk(?P<speaker>\d+)"
    r"(?:_(?P<variant>\d+))?_L(?P<line>\d+)_"
    r"(?P<start>\d+(?:\.\d+)?)_(?P<duration>\d+(?:\.\d+)?)\.wav"
)
EN_NEGATION = re.compile(
    r"\b(?:no|not|never|don't|doesn't|didn't|can't|cannot|won't|"
    r"isn't|aren't|wasn't|without|neither|none)\b",
    re.IGNORECASE,
)
ZH_NEGATION = re.compile(
    r"没有|不是|不要|不必|不行|不能|不会|不想|不可|不太|并非|"
    r"没法|未曾|未必|无|未|没|不|别(?!人)"
)


class PilotError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PilotError(message)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path, cap: int | None = None) -> str:
    value = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while block := stream.read(MIB):
            size += len(block)
            if cap is not None:
                require(size <= cap, f"file exceeds byte cap: {path}")
            value.update(block)
    return value.hexdigest()


def read_json(path: Path, cap: int = MAX_JSON_BYTES) -> dict:
    require(path.is_file() and not path.is_symlink(), f"regular JSON file required: {path}")
    require(path.stat().st_size <= cap, f"JSON byte budget exceeded: {path}")

    def object_pairs(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key in {path}: {key}")
            result[key] = value
        return result

    try:
        value = json.loads(path.read_bytes(), object_pairs_hook=object_pairs)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PilotError(f"invalid JSON {path}: {error}") from error
    require(isinstance(value, dict), f"JSON object required: {path}")
    return value


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n").encode()


def external_path(value: str | Path, *, must_exist: bool = False) -> Path:
    path = Path(value)
    require(path.is_absolute(), f"absolute external path required: {path}")
    resolved = path.resolve(strict=must_exist)
    require(not resolved.is_relative_to(REPO), f"pilot artifacts must stay outside repo: {resolved}")
    return resolved


def mkdir_private(path: Path) -> None:
    path.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(path, 0o700)


def write_once(path: Path, data: bytes, mode: int = 0o600) -> None:
    require(not path.exists() and not path.is_symlink(), f"immutable output already exists: {path}")
    mkdir_private(path.parent)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), mode)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def write_json_once(path: Path, value: object) -> None:
    write_once(path, json_bytes(value))


def wave_from_bytes(data: bytes, label: str) -> tuple[dict, tuple[int, ...]]:
    require(44 < len(data) <= MAX_WAV_BYTES, f"WAV byte budget: {label}")
    require(data[:4] == b"RIFF" and data[8:12] == b"WAVE", f"RIFF/WAVE required: {label}")
    require(struct.unpack_from("<I", data, 4)[0] + 8 == len(data), f"complete RIFF required: {label}")
    cursor = 12
    fmt_chunks = 0
    data_chunks = 0
    while cursor < len(data):
        require(cursor + 8 <= len(data), f"truncated WAV chunk header: {label}")
        name = data[cursor : cursor + 4]
        size = struct.unpack_from("<I", data, cursor + 4)[0]
        cursor += 8
        if name == b"fmt ":
            fmt_chunks += 1
        if name == b"data":
            data_chunks += 1
        cursor += size + (size & 1)
        require(cursor <= len(data), f"truncated WAV chunk body: {label}")
    require(fmt_chunks == 1 and data_chunks == 1, f"one fmt/data WAV chunk required: {label}")
    try:
        with wave.open(io.BytesIO(data), "rb") as reader:
            channels = reader.getnchannels()
            width = reader.getsampwidth()
            rate = reader.getframerate()
            frames = reader.getnframes()
            compression = reader.getcomptype()
            pcm = reader.readframes(frames)
    except (wave.Error, EOFError) as error:
        raise PilotError(f"invalid WAV {label}: {error}") from error
    require(
        channels == 1 and width == 2 and rate == SAMPLE_RATE and compression == "NONE",
        f"source must be 16 kHz mono PCM16: {label}",
    )
    require(0 < frames <= SAMPLE_RATE * 600 and len(pcm) == frames * 2, f"WAV sample budget: {label}")
    samples = struct.unpack(f"<{frames}h", pcm)
    info = {
        "audio_sha256": sha256_bytes(data),
        "pcm_sha256": sha256_bytes(pcm),
        "bytes": len(data),
        "sample_rate_hz": rate,
        "channels": channels,
        "bits_per_sample": 16,
        "samples": frames,
        "duration_seconds": frames / rate,
    }
    return info, samples


def verify_pcm16_artifact(
    path_value: str,
    expected_audio_sha256: str,
    expected_pcm_sha256: str | None,
    expected_samples: int | None,
    label: str,
) -> tuple[Path, dict, tuple[int, ...]]:
    require(isinstance(path_value, str) and path_value, f"{label} path required")
    candidate = Path(path_value)
    require(candidate.is_absolute(), f"absolute {label} path required")
    require(not candidate.is_symlink(), f"regular {label} file required")
    path = external_path(candidate, must_exist=True)
    require(path.is_file() and not path.is_symlink(), f"regular {label} file required")
    require(path.stat().st_size <= MAX_WAV_BYTES, f"{label} byte budget exceeded")
    require(
        isinstance(expected_audio_sha256, str)
        and re.fullmatch(r"[0-9a-f]{64}", expected_audio_sha256) is not None,
        f"{label} audio SHA-256 required",
    )
    if expected_pcm_sha256 is not None:
        require(
            isinstance(expected_pcm_sha256, str)
            and re.fullmatch(r"[0-9a-f]{64}", expected_pcm_sha256) is not None,
            f"{label} PCM SHA-256 required",
        )
    data = path.read_bytes()
    info, samples = wave_from_bytes(data, str(path))
    require(info["audio_sha256"] == expected_audio_sha256, f"{label} audio SHA-256 identity mismatch")
    if expected_pcm_sha256 is not None:
        require(info["pcm_sha256"] == expected_pcm_sha256, f"{label} PCM SHA-256 identity mismatch")
    if expected_samples is not None:
        require(
            isinstance(expected_samples, int) and not isinstance(expected_samples, bool)
            and info["samples"] == expected_samples,
            f"{label} sample-count identity mismatch",
        )
    return path, info, samples


def validate_selected_cases(manifest: dict, requested_ids: list[str] | None) -> tuple[list[dict], list[dict]]:
    raw_cases = manifest.get("cases")
    require(isinstance(raw_cases, list) and raw_cases, "non-empty case list required")
    require(len(raw_cases) <= MAX_CASES, f"pilot case budget exceeded: {len(raw_cases)} > {MAX_CASES}")
    seen_case_ids = set()
    for case in raw_cases:
        require(isinstance(case, dict), "case entries must be objects")
        case_id = case.get("case_id")
        require(isinstance(case_id, str) and CASE_ID.fullmatch(case_id) is not None, "safe case_id required")
        require(case_id not in seen_case_ids, f"duplicate case_id: {case_id}")
        seen_case_ids.add(case_id)
        require(case.get("role") in CASE_ROLES, f"unsupported case role: {case_id}")
        require(case.get("language") in CASE_LANGUAGES, f"unsupported case language: {case_id}")
        require(isinstance(case.get("profile_id"), str) and case["profile_id"], f"profile_id required: {case_id}")
    requested = requested_ids or []
    require(len(requested) == len(set(requested)), "duplicate --case-id selection")
    if requested:
        unknown = set(requested) - seen_case_ids
        require(not unknown, f"unknown case ids: {sorted(unknown)}")
        requested_set = set(requested)
        selected = [case for case in raw_cases if case["case_id"] in requested_set]
    else:
        selected = list(raw_cases)
    require(selected, "selected case list is empty")

    raw_profiles = manifest.get("profiles")
    require(isinstance(raw_profiles, list) and raw_profiles, "non-empty enrolled public profile list required")
    profile_by_id = {}
    for profile in raw_profiles:
        require(isinstance(profile, dict), "profile entries must be objects")
        profile_id = profile.get("profile_id")
        require(isinstance(profile_id, str) and CASE_ID.fullmatch(profile_id) is not None, "safe profile_id required")
        require(profile_id not in profile_by_id, f"duplicate profile_id: {profile_id}")
        profile_by_id[profile_id] = profile
    profile_ids = {case["profile_id"] for case in selected}
    missing_profiles = profile_ids - set(profile_by_id)
    require(not missing_profiles, f"case references unknown profile: {sorted(missing_profiles)}")
    profiles = [profile for profile in raw_profiles if profile["profile_id"] in profile_ids]
    return selected, profiles


def preflight_pilot_audio(cases: list[dict], profiles: list[dict]) -> dict[str, dict]:
    """Verify source identity and audio bounds before launching the native helper."""
    audit = {}
    for profile in profiles:
        profile_id = profile["profile_id"]
        require(
            isinstance(profile.get("source_speaker_id"), int)
            and not isinstance(profile.get("source_speaker_id"), bool),
            f"profile source speaker id required: {profile_id}",
        )
        path, info, _ = verify_pcm16_artifact(
            profile.get("input_audio_path"),
            profile.get("input_audio_sha256"),
            profile.get("input_pcm_sha256"),
            profile.get("input_samples"),
            f"enrollment input {profile_id}",
        )
        audit[f"profile:{profile_id}"] = {
            "path": str(path),
            "audio_sha256": info["audio_sha256"],
            "pcm_sha256": info["pcm_sha256"],
            "samples": info["samples"],
        }

    source_cache = {}

    def verify_source(path_value, audio_sha, pcm_sha, label):
        cache_key = (path_value, audio_sha, pcm_sha)
        if cache_key not in source_cache:
            path, info, _ = verify_pcm16_artifact(path_value, audio_sha, pcm_sha, None, label)
            source_cache[cache_key] = {
                "path": str(path),
                "audio_sha256": info["audio_sha256"],
                "pcm_sha256": info["pcm_sha256"],
                "samples": info["samples"],
            }
        return source_cache[cache_key]

    profiles_by_id = {profile["profile_id"]: profile for profile in profiles}
    for case in cases:
        case_id = case["case_id"]
        recipe = case.get("mix_recipe")
        require(isinstance(recipe, dict), f"mix recipe required: {case_id}")
        input_path, input_info, _ = verify_pcm16_artifact(
            recipe.get("input_audio_path") or case.get("prepared_input_path"),
            recipe.get("input_audio_sha256"),
            recipe.get("input_pcm_sha256"),
            recipe.get("input_samples"),
            f"case input {case_id}",
        )
        audit[case_id] = {
            "path": str(input_path),
            "audio_sha256": input_info["audio_sha256"],
            "pcm_sha256": input_info["pcm_sha256"],
            "samples": input_info["samples"],
        }
        profile = profiles_by_id[case["profile_id"]]
        if case["role"].startswith("target_"):
            target_speaker = case.get("target_speaker_id")
            require(
                isinstance(target_speaker, int) and not isinstance(target_speaker, bool)
                and target_speaker == profile["source_speaker_id"],
                f"target/profile speaker identity mismatch: {case_id}",
            )
        target_source_path = recipe.get("target_source_path")
        target_source_sha = recipe.get("target_source_audio_sha256")
        if target_source_path is not None or target_source_sha is not None:
            require(target_source_path is not None and target_source_sha is not None, f"incomplete target source identity: {case_id}")
            source = verify_source(
                target_source_path,
                target_source_sha,
                recipe.get("target_source_pcm_sha256"),
                f"target source {case_id}",
            )
            expected_top_level_sha = case.get("target_source_audio_sha256")
            if expected_top_level_sha is not None:
                require(source["audio_sha256"] == expected_top_level_sha, f"target source metadata mismatch: {case_id}")
        interferer = recipe.get("interferer")
        if isinstance(interferer, dict) and interferer.get("source_path") is not None:
            source = verify_source(
                interferer["source_path"],
                interferer.get("source_audio_sha256"),
                interferer.get("source_pcm_sha256"),
                f"interferer source {case_id}",
            )
            if interferer.get("source_audio_sha256") is not None:
                require(source["audio_sha256"] == interferer["source_audio_sha256"], f"interferer metadata mismatch: {case_id}")
        component = case.get("target_component")
        if isinstance(component, dict) and component.get("source_path") is not None:
            target_path = recipe.get("target_source_path")
            require(Path(component["source_path"]) == Path(target_path), f"target component path mismatch: {case_id}")
    return audit


def float_wave_from_bytes(data: bytes, label: str) -> tuple[dict, tuple[float, ...]]:
    require(44 < len(data) <= MAX_WAV_BYTES, f"float WAV byte budget: {label}")
    require(data[:4] == b"RIFF" and data[8:12] == b"WAVE", f"RIFF/WAVE required: {label}")
    require(struct.unpack_from("<I", data, 4)[0] + 8 == len(data), f"complete float RIFF required: {label}")
    cursor = 12
    fmt_body = None
    pcm_body = None
    fmt_count = 0
    data_count = 0
    while cursor < len(data):
        require(cursor + 8 <= len(data), f"truncated float WAV chunk header: {label}")
        name = data[cursor : cursor + 4]
        size = struct.unpack_from("<I", data, cursor + 4)[0]
        cursor += 8
        body = data[cursor : cursor + size]
        require(len(body) == size, f"truncated float WAV chunk body: {label}")
        if name == b"fmt ":
            fmt_count += 1
            fmt_body = body
        if name == b"data":
            data_count += 1
            pcm_body = body
        cursor += size + (size & 1)
    require(fmt_count == 1 and data_count == 1 and fmt_body is not None and pcm_body is not None, f"one float fmt/data chunk required: {label}")
    require(len(fmt_body) >= 16, f"float WAV fmt chunk too short: {label}")
    format_tag, channels, rate, byte_rate, block_align, bits = struct.unpack_from("<HHIIHH", fmt_body, 0)
    subtype_guid = bytes.fromhex("0300000000001000800000aa00389b71")
    ieee_float = format_tag == 3 or (
        format_tag == 0xFFFE
        and len(fmt_body) >= 40
        and fmt_body[24:40] == subtype_guid
    )
    require(ieee_float and channels == 1 and rate == SAMPLE_RATE and bits == 32, f"IEEE float32 16k mono WAV required: {label}")
    require(byte_rate == SAMPLE_RATE * 4 and block_align == 4 and len(pcm_body) % 4 == 0, f"float WAV format mismatch: {label}")
    frames = len(pcm_body) // 4
    require(0 < frames <= SAMPLE_RATE * 600, f"float WAV sample budget: {label}")
    samples = struct.unpack(f"<{frames}f", pcm_body)
    require(all(math.isfinite(sample) for sample in samples), f"non-finite float sample: {label}")
    info = {
        "audio_sha256": sha256_bytes(data),
        "pcm_sha256": sha256_bytes(pcm_body),
        "bytes": len(data),
        "sample_rate_hz": rate,
        "channels": channels,
        "bits_per_sample": bits,
        "sample_format": "IEEE_FLOAT32" if format_tag == 3 else "IEEE_FLOAT32_EXTENSIBLE",
        "samples": frames,
        "duration_seconds": frames / rate,
    }
    return info, samples


def wav_bytes(samples) -> bytes:
    packed = struct.pack(f"<{len(samples)}h", *samples)
    stream = io.BytesIO()
    with wave.open(stream, "wb") as writer:
        writer.setnchannels(1)
        writer.setsampwidth(2)
        writer.setframerate(SAMPLE_RATE)
        writer.writeframes(packed)
    return stream.getvalue()


def encode_production_pcm16(samples: tuple[float, ...] | list[float]) -> tuple[int, ...]:
    """Match VoiceFlow audio.rs::encode using IEEE f32 math and truncation."""
    result = []
    for sample in samples:
        require(math.isfinite(float(sample)), "non-finite input to PCM16 encode")
        sample_f32 = struct.unpack("<f", struct.pack("<f", float(sample)))[0]
        clamped_f32 = max(-1.0, min(1.0, sample_f32))
        product_f32 = struct.unpack("<f", struct.pack("<f", clamped_f32 * 32767.0))[0]
        result.append(int(product_f32))
    return tuple(result)


def int16_to_normalized_float(samples: tuple[int, ...]) -> tuple[float, ...]:
    return tuple(sample / 32768.0 for sample in samples)


def pcm_stats(samples) -> dict:
    if not samples:
        return {
            "rms_dbfs": None,
            "peak_dbfs": None,
            "zero_samples": 0,
            "clipped_samples": 0,
        }
    square_sum = 0
    peak = 0
    zero = 0
    clipped = 0
    for sample in samples:
        absolute = abs(sample)
        square_sum += sample * sample
        peak = max(peak, absolute)
        zero += sample == 0
        clipped += absolute >= 32767
    rms = math.sqrt(square_sum / len(samples)) / 32768.0
    peak_value = peak / 32768.0
    return {
        "rms_dbfs": round(20 * math.log10(max(rms, 1e-12)), 3),
        "peak_dbfs": round(20 * math.log10(max(peak_value, 1e-12)), 3),
        "zero_samples": zero,
        "clipped_samples": clipped,
    }


def float_stats(samples) -> dict:
    if not samples:
        return {
            "rms_dbfs": None,
            "peak_dbfs": None,
            "zero_samples": 0,
            "clipped_samples": 0,
        }
    square_sum = 0.0
    peak = 0.0
    zero = 0
    clipped = 0
    for sample in samples:
        absolute = abs(sample)
        square_sum += sample * sample
        peak = max(peak, absolute)
        zero += sample == 0.0
        clipped += absolute >= 1.0
    rms = math.sqrt(square_sum / len(samples))
    return {
        "rms_dbfs": round(20 * math.log10(max(rms, 1e-12)), 3),
        "peak_dbfs": round(20 * math.log10(max(peak, 1e-12)), 3),
        "zero_samples": zero,
        "clipped_samples": clipped,
    }

def stable_key(value: str, seed: str) -> str:
    return hashlib.sha256(f"{seed}\0{value}".encode()).hexdigest()


def negation_candidate(language: str, text: str) -> bool:
    if language == "en":
        return EN_NEGATION.search(text) is not None
    if language == "zh":
        return ZH_NEGATION.search(text) is not None
    return EN_NEGATION.search(text) is not None or ZH_NEGATION.search(text) is not None


def row_from_metadata(split: str, row: dict, ordinal: int, parquet_sha: str) -> dict:
    source_id = row.get("id")
    require(isinstance(source_id, str) and 0 < len(source_id) <= 32, "ASCEND string id required")
    require(source_id.isascii() and source_id.isdigit(), f"ASCEND source id grammar: {source_id}")
    require(row.get("language") in LANGUAGES, f"ASCEND language unknown: {source_id}")
    speaker = row.get("original_speaker_id")
    session = row.get("session_id")
    require(type(speaker) is int and speaker > 0 and type(session) is int and session > 0, "publisher speaker/session ids required")
    text = row.get("transcription")
    require(isinstance(text, str) and len(text.encode("utf-8")) <= 16384, "ASCEND transcript bound")
    source_path = row.get("path")
    require(isinstance(source_path, str) and len(source_path.encode("utf-8")) <= 4096, f"ASCEND path bound: {source_id}")
    filename = Path(source_path).name
    require(filename not in ("", ".", ".."), f"ASCEND path basename required: {source_id}")
    match = ASCEND_NAME.fullmatch(filename)
    require(match is not None, f"ASCEND audio filename grammar: {source_id}")
    parsed = match.groupdict()
    require(int(parsed["speaker"]) == speaker and int(parsed["session"]) == session, f"ASCEND speaker/session path mismatch: {source_id}")
    duration = float(row.get("duration"))
    require(math.isfinite(duration) and 0.05 <= duration <= 600.0, f"ASCEND duration bound: {source_id}")
    case_id = f"ascend-{split}-{source_id}"
    return {
        "case_id": case_id,
        "source": SOURCE_URL,
        "source_revision": SOURCE_REVISION,
        "source_license": SOURCE_LICENSE,
        "source_split": split,
        "project_partition_status": (
            "development_candidate_pool" if split == "validation"
            else "publisher_test_split_unassigned_project_role"
        ),
        "source_id": source_id,
        "row_ordinal": ordinal,
        "source_filename": filename,
        "publisher_source_path": source_path,
        "source_container_path": f"main/{split}-00000-of-00001.parquet",
        "source_parquet_sha256": parquet_sha,
        "source_speaker_id": speaker,
        "source_session_id": session,
        "source_recording_variant": parsed["variant"],
        "source_parent_block_id": (
            f"ascend:{split}:spk{speaker}:ses{session}:"
            f"variant{parsed['variant'] or 'omitted'}"
        ),
        "language": row["language"],
        "publisher_duration_seconds": duration,
        "source_annotation": {
            "text": text,
            "sha256": sha256_bytes(text.encode("utf-8")),
            "origin": "publisher_manual_asr_annotation",
            "project_review_status": "pending",
            "contains_unk": "[UNK]" in text,
            "negation_candidate_from_publisher_text": negation_candidate(row["language"], text),
            "negation_review_status": "heuristic_candidate_not_human_reviewed",
        },
        "source_topic": row.get("topic"),
    }


def audit_source(
    corpus_root: Path,
    candidate_root: Path,
    max_source_bytes: int,
) -> tuple[dict, dict[str, list[dict]]]:
    require(corpus_root.is_dir() and not corpus_root.is_symlink(), f"ASCEND source directory required: {corpus_root}")
    receipt_path = corpus_root / "acquisition.json"
    receipt = read_json(receipt_path)
    require(receipt.get("artifact_type") == "public_acquisition_receipt", "ASCEND acquisition receipt required")
    require(receipt.get("source") == SOURCE_URL and receipt.get("revision") == SOURCE_REVISION, "ASCEND pinned source revision required")
    require(receipt.get("license") == SOURCE_LICENSE, "ASCEND license receipt mismatch")
    receipt_sha = sha256_file(receipt_path, 2 * MIB)
    files = {item.get("path"): item for item in receipt.get("files", []) if isinstance(item, dict)}
    metadata_rows: dict[str, list[dict]] = {}
    observed_total = 0
    parquet_receipts = {}
    for relative, expected in SOURCE_EXPECTED.items():
        entry = files.get(relative)
        require(isinstance(entry, dict), f"ASCEND receipt missing {relative}")
        path = corpus_root / relative
        require(path.is_file() and not path.is_symlink(), f"ASCEND parquet required: {path}")
        size = path.stat().st_size
        observed_total += size
        require(observed_total <= max_source_bytes <= MAX_CORPUS_BYTES, "source corpus byte cap exceeded")
        digest = sha256_file(path, max_source_bytes)
        require(size == expected["bytes"] == entry.get("actual_bytes"), f"ASCEND parquet byte size mismatch: {relative}")
        require(digest == expected["sha256"] == entry.get("actual_sha256") == entry.get("publisher_sha256"), f"ASCEND parquet checksum mismatch: {relative}")
        parquet_receipts[relative] = {
            "path": str(path),
            "bytes": size,
            "sha256": digest,
            "publisher_sha256": entry.get("publisher_sha256"),
            "rows_expected": expected["rows"],
        }
    try:
        import pyarrow.parquet as pq
    except ImportError as error:
        raise PilotError(
            "prepare requires the already-pinned pyarrow 23.0.1 dependency from "
            "scripts/research/requirements-public-corpus.txt; install it in a private external target"
        ) from error
    for split in ("validation", "test"):
        relative = f"main/{split}-00000-of-00001.parquet"
        parquet_path = corpus_root / relative
        parquet = pq.ParquetFile(parquet_path)
        require(parquet.metadata.num_rows == SOURCE_EXPECTED[relative]["rows"], f"ASCEND {split} row count mismatch")
        require(parquet.metadata.num_rows <= MAX_ROWS, f"ASCEND row cap exceeded: {split}")
        for index in range(parquet.metadata.num_row_groups):
            require(parquet.metadata.row_group(index).total_byte_size <= 64 * MIB, f"ASCEND decoded row group over 64 MiB: {split}")
        records = []
        seen_ids = set()
        ordinal = 0
        columns = ["id", "path", "duration", "language", "original_speaker_id", "session_id", "transcription", "topic"]
        for batch in parquet.iter_batches(batch_size=32, columns=columns, use_threads=False):
            for row in batch.to_pylist():
                record = row_from_metadata(split, row, ordinal, parquet_receipts[relative]["sha256"])
                ordinal += 1
                require(record["source_id"] not in seen_ids, f"duplicate ASCEND {split} source id")
                seen_ids.add(record["source_id"])
                records.append(record)
        metadata_rows[split] = records
    candidate_path = candidate_root / "candidates.json"
    candidate_info = {}
    if candidate_path.is_file():
        candidates = read_json(candidate_path)
        require(candidates.get("source") == SOURCE_URL and candidates.get("revision") == SOURCE_REVISION, "existing ASCEND candidates are not pinned source")
        require(candidates.get("license") == SOURCE_LICENSE, "existing ASCEND candidate license mismatch")
        require(candidates.get("review_status") == "pending", "existing ASCEND candidate review status changed")
        candidate_info = {
            "path": str(candidate_path),
            "sha256": sha256_file(candidate_path, 16 * MIB),
            "case_count": len(candidates.get("cases", [])),
            "review_status": candidates.get("review_status"),
            "project_annotation_status_counts": dict(
                Counter((item.get("source_annotation") or {}).get("project_review_status", "missing")
                        for item in candidates.get("cases", []))
            ),
        }
    audit = {
        "source": SOURCE_URL,
        "revision": SOURCE_REVISION,
        "license": SOURCE_LICENSE,
        "attribution": receipt.get("attribution"),
        "acquisition_receipt_path": str(receipt_path),
        "acquisition_receipt_sha256": receipt_sha,
        "source_parquet_files": parquet_receipts,
        "source_parquet_bytes_total": observed_total,
        "new_source_download_bytes": 0,
        "public_license_does_not_establish_speaker_consent": True,
        "existing_candidate_manifest": candidate_info or None,
        "human_annotation_status": "publisher annotations only; project review pending",
    }
    return audit, metadata_rows


def eligible(record: dict, split: str, speaker: int, language: str | None = None) -> bool:
    annotation = record["source_annotation"]
    duration = record["publisher_duration_seconds"]
    return (
        record["source_split"] == split
        and record["source_speaker_id"] == speaker
        and (language is None or record["language"] == language)
        and 1.0 <= duration <= 15.0
        and annotation["text"].strip() != ""
        and not annotation["contains_unk"]
    )


def select_rows(metadata_rows: dict[str, list[dict]], seed: str) -> tuple[dict, dict]:
    validation = metadata_rows["validation"]
    test = metadata_rows["test"]
    enrollment: dict[int, list[dict]] = {}
    targets: list[dict] = []
    selected_target_ids = set()
    coverage = {
        "target_expected_per_language": 24,
        "target_expected_by_speaker_per_language": 8,
        "short_definition_seconds_at_most": 2.5,
        "negation_detection": "heuristic over publisher transcript; not independently reviewed",
        "soft_input_recipe": "six target-only cases at fixed target gain 0.10 (approximately -20 dB); exact measured input level reported",
        "minimum_coverage": {
            "short_target_cases_per_speaker_language": 2,
            "negation_candidate_per_speaker_language": 1,
            "soft_target_only_cases_per_language": 2,
        },
    }
    for speaker in TARGET_SPEAKERS:
        pool = [
            row for row in validation
            if eligible(row, "validation", speaker)
            and row["source_session_id"] == 1
            and 2.5 <= row["publisher_duration_seconds"] <= 8.0
        ]
        pool.sort(key=lambda row: stable_key(row["case_id"], seed + f":enroll:{speaker}"))
        chosen = []
        total_seconds = 0.0
        for row in pool:
            if len(chosen) >= 10:
                break
            if total_seconds >= 20.0:
                break
            if total_seconds + row["publisher_duration_seconds"] > 27.5:
                continue
            chosen.append(row)
            total_seconds += row["publisher_duration_seconds"]
        require(
            18.0 <= total_seconds <= 27.5,
            f"not enough 18-27.5 seconds of separate enrollment audio for public speaker {speaker}",
        )
        enrollment[speaker] = chosen
    for language in LANGUAGES:
        for speaker in TARGET_SPEAKERS:
            pool = [
                row for row in validation
                if eligible(row, "validation", speaker, language)
                and row["source_session_id"] != 1
            ]
            require(len(pool) >= 8, f"insufficient disjoint target pool for {language}, source speaker {speaker}")
            chosen = []
            used = set()

            def take(candidates, count):
                candidates = sorted(
                    (row for row in candidates if row["case_id"] not in used),
                    key=lambda row: (
                        row["publisher_duration_seconds"],
                        stable_key(row["case_id"], seed + f":target:{language}:{speaker}"),
                    ),
                )
                for row in candidates[:count]:
                    chosen.append(row)
                    used.add(row["case_id"])

            short_pool = [row for row in pool if row["publisher_duration_seconds"] <= 2.5]
            take(short_pool, 2)
            negation_pool = [
                row for row in pool
                if row["source_annotation"]["negation_candidate_from_publisher_text"]
            ]
            take(negation_pool, 1)
            remaining = [row for row in pool if row["case_id"] not in used]
            remaining.sort(key=lambda row: stable_key(row["case_id"], seed + f":fill:{language}:{speaker}"))
            chosen.extend(remaining[: max(0, 8 - len(chosen))])
            require(len(chosen) == 8 and len({row["case_id"] for row in chosen}) == 8, f"target selection count mismatch: {language}/{speaker}")
            for row in chosen:
                selected_target_ids.add(row["case_id"])
                targets.append(row)
    interference: list[dict] = []
    used_interference = set()
    target_groups: dict[str, list[dict]] = defaultdict(list)
    for row in targets:
        target_groups[row["language"]].append(row)
    for language in LANGUAGES:
        language_targets = sorted(
            target_groups[language],
            key=lambda row: stable_key(row["case_id"], seed + f":interference:{language}"),
        )
        for index, target in enumerate(language_targets):
            other_speaker = OTHER_SPEAKERS[index % len(OTHER_SPEAKERS)]
            pool = [
                row for row in test
                if eligible(row, "test", other_speaker, language)
                and row["case_id"] not in used_interference
            ]
            require(pool, f"missing disjoint test speaker audio for {language}/speaker {other_speaker}")
            pool.sort(key=lambda row: stable_key(row["case_id"], seed + f":other:{language}:{other_speaker}"))
            selected = pool[0]
            used_interference.add(selected["case_id"])
            interference.append({**selected, "paired_target_case_id": target["case_id"]})
    other_only: list[dict] = []
    used_other_only = set(used_interference)
    profile_ids = {speaker: f"public-ascend-speaker-{speaker}" for speaker in TARGET_SPEAKERS}
    for profile_index, speaker in enumerate(TARGET_SPEAKERS):
        for case_index in range(4):
            language = LANGUAGES[(case_index + profile_index) % len(LANGUAGES)]
            other_speaker = OTHER_SPEAKERS[(case_index + profile_index) % len(OTHER_SPEAKERS)]
            pool = [
                row for row in test
                if eligible(row, "test", other_speaker, language)
                and row["case_id"] not in used_other_only
            ]
            if not pool:
                pool = [
                    row for row in test
                    if eligible(row, "test", OTHER_SPEAKERS[1 - OTHER_SPEAKERS.index(other_speaker)], language)
                    and row["case_id"] not in used_other_only
                ]
            require(pool, f"missing other-only source for {language}")
            pool.sort(key=lambda row: stable_key(row["case_id"], seed + f":other-only:{speaker}:{case_index}"))
            selected = pool[0]
            used_other_only.add(selected["case_id"])
            other_only.append({**selected, "profile_id": profile_ids[speaker]})
    for target in targets:
        target["profile_id"] = profile_ids[target["source_speaker_id"]]
        target["short_candidate"] = target["publisher_duration_seconds"] <= 2.5
        target["negation_candidate"] = target["source_annotation"]["negation_candidate_from_publisher_text"]
    return {
        "enrollment": enrollment,
        "targets": targets,
        "interference": interference,
        "other_only": other_only,
        "profiles_by_speaker": profile_ids,
    }, {
        **coverage,
        "selected_target_counts": dict(Counter(row["language"] for row in targets)),
        "selected_target_counts_by_language_and_speaker": {
            language: {str(speaker): sum(row["language"] == language and row["source_speaker_id"] == speaker for row in targets)
                       for speaker in TARGET_SPEAKERS}
            for language in LANGUAGES
        },
        "target_source_speaker_count": len({row["source_speaker_id"] for row in targets}),
        "interferer_source_speaker_ids": sorted({row["source_speaker_id"] for row in interference}),
        "target_interferer_speaker_identity_disjoint": (
            {row["source_speaker_id"] for row in targets}.isdisjoint(
                {row["source_speaker_id"] for row in interference}
            )
        ),
        "profile_target_audio_disjoint": (
            selected_target_ids.isdisjoint(
                {row["case_id"] for clips in enrollment.values() for row in clips}
            )
        ),
        "profile_target_source_sessions_disjoint": all(
            all(row["source_session_id"] == 1 for row in clips)
            for clips in enrollment.values()
        ) and all(row["source_session_id"] != 1 for row in targets),
        "distinct_target_speakers": len({row["source_speaker_id"] for row in targets}),
        "distinct_other_speakers": len({row["source_speaker_id"] for row in interference}),
        "total_distinct_public_speakers": len(
            {row["source_speaker_id"] for row in targets + interference}
        ),
        "target_speaker_gap_to_12": max(0, 12 - len({row["source_speaker_id"] for row in targets})),
        "short_target_count_from_metadata": sum(row["short_candidate"] for row in targets),
        "negation_candidate_count_from_publisher_text": sum(row["negation_candidate"] for row in targets),
        "other_only_expected": 12,
        "negative_expected": 6,
        "holdout_claim": "not a project locked holdout; publisher test speaker identities only",
        "transcript_review": "pending; no project-reviewed gold references",
    }


def copy_source_wav(
    source_root: Path,
    destination: Path,
    source_bytes: bytes,
    expected_duration: float | None = None,
) -> dict:
    info, _ = wave_from_bytes(source_bytes, str(destination))
    if expected_duration is not None:
        require(abs(info["duration_seconds"] - expected_duration) <= 0.001, f"source duration mismatch: {destination.name}")
    write_once(destination, source_bytes)
    return {"path": str(destination), **info}


def extract_selected_audio(
    corpus_root: Path,
    output_root: Path,
    selected_rows: list[dict],
    max_bytes: int,
) -> dict[str, dict]:
    import pyarrow.parquet as pq

    selected = {(row["source_split"], row["source_id"]): row for row in selected_rows}
    result = {}
    source_audio_bytes = 0
    for split in ("validation", "test"):
        relative = f"main/{split}-00000-of-00001.parquet"
        parquet_path = corpus_root / relative
        parquet = pq.ParquetFile(parquet_path)
        columns = ["id", "path", "duration", "language", "original_speaker_id", "session_id", "transcription", "topic", "audio"]
        ordinal = 0
        for batch in parquet.iter_batches(batch_size=8, columns=columns, use_threads=False):
            for row in batch.to_pylist():
                source_id = row["id"]
                key = (split, source_id)
                if key not in selected:
                    ordinal += 1
                    continue
                record = selected[key]
                require(
                    row["audio"]["path"] == Path(row["path"]).name == record["source_filename"],
                    f"ASCEND embedded/path mismatch: {record['case_id']}",
                )
                audio = row["audio"]["bytes"]
                require(isinstance(audio, bytes), f"ASCEND embedded bytes missing: {record['case_id']}")
                source_audio_bytes += len(audio)
                require(source_audio_bytes <= max_bytes <= MAX_CORPUS_BYTES, "extracted corpus audio byte cap exceeded")
                destination = output_root / "source-audio" / f"{split}-{source_id}.wav"
                source_info = copy_source_wav(corpus_root, destination, audio, record["publisher_duration_seconds"])
                record["source_audio"] = source_info
                record["source_path"] = str(destination)
                record["source_audio_bytes_extracted"] = len(audio)
                record["source_audio_sha256"] = source_info["audio_sha256"]
                record["source_pcm_sha256"] = source_info["pcm_sha256"]
                record["source_rms_dbfs"] = pcm_stats(wave_from_bytes(audio, destination.name)[1])["rms_dbfs"]
                result[record["case_id"]] = record
                ordinal += 1
        require(
            sum(1 for key in selected if key[0] == split) == sum(1 for item in result if item.startswith(f"ascend-{split}-")),
            f"not all selected {split} audio rows extracted",
        )
    return result


def combine_enrollment_clips(
    profile_id: str,
    clips: list[dict],
    extracted: dict[str, dict],
    output_root: Path,
) -> dict:
    combined: list[int] = []
    sources = []
    gap_samples = int(0.1 * SAMPLE_RATE)
    for row in clips:
        source = extracted[row["case_id"]]
        source_path = source["source_path"]
        info, samples = wave_from_bytes(Path(source_path).read_bytes(), source_path)
        start = len(combined)
        combined.extend(samples)
        end = len(combined)
        if len(clips) > 1:
            combined.extend([0] * gap_samples)
        sources.append({
            "case_id": row["case_id"],
            "source_speaker_id": row["source_speaker_id"],
            "source_session_id": row["source_session_id"],
            "source_parent_block_id": row["source_parent_block_id"],
            "source_audio_sha256": source["source_audio_sha256"],
            "timeline_start_sample": start,
            "timeline_end_sample_exclusive": end,
            "source_seconds": info["duration_seconds"],
            "project_review_status": row["source_annotation"]["project_review_status"],
        })
    path = output_root / "enrollment-inputs" / f"{profile_id}.wav"
    data = wav_bytes(combined)
    info = copy_source_wav(output_root, path, data)
    return {
        "profile_id": profile_id,
        "source_speaker_id": clips[0]["source_speaker_id"],
        "input_audio_path": str(path),
        "input_audio_sha256": info["audio_sha256"],
        "input_pcm_sha256": info["pcm_sha256"],
        "input_samples": info["samples"],
        "input_duration_seconds": info["duration_seconds"],
        "source_clip_count": len(sources),
        "publisher_segment_duration_sum_seconds": round(sum(item["source_seconds"] for item in sources), 3),
        "clip_gap_samples": gap_samples,
        "source_review_status": "publisher annotations; project review pending",
        "profile_kind": "temporary_public_pilot_voiceprint_not_formal_user_profile",
        "source_clips": sources,
    }


def scaled_sample(sample: int, gain: float) -> int:
    return max(-32768, min(32767, int(round(sample * gain))))


def make_target_input(
    target: dict,
    interferer: dict | None,
    extracted: dict[str, dict],
    output_path: Path,
    *,
    target_gain: float,
    interferer_gain: float,
    interferer_offset_samples: int,
) -> tuple[dict, dict]:
    target_source = extracted[target["case_id"]]
    target_path = target_source["source_path"]
    target_info, target_samples = wave_from_bytes(Path(target_path).read_bytes(), target_path)
    target_scaled = [scaled_sample(sample, target_gain) for sample in target_samples]
    mixed = list(target_scaled)
    interference_info = None
    if interferer is not None:
        interference_source = extracted[interferer["case_id"]]
        interference_path = interference_source["source_path"]
        interference_info, interference_samples = wave_from_bytes(Path(interference_path).read_bytes(), interference_path)
        max_offset = max(0, len(target_samples) // 2)
        offset = min(interferer_offset_samples, max_offset)
        offset = max(0, offset)
        usable = min(len(interference_samples), len(target_samples) - offset)
        for index in range(usable):
            value = mixed[offset + index] + scaled_sample(interference_samples[index], interferer_gain)
            mixed[offset + index] = max(-32768, min(32767, value))
        interferer_offset_samples = offset
        interference_info = {
            "source_case_id": interferer["case_id"],
            "source_path": interference_source["source_path"],
            "source_audio_sha256": interference_source["source_audio_sha256"],
            "source_pcm_sha256": interference_source["source_pcm_sha256"],
            "source_speaker_id": interferer["source_speaker_id"],
            "source_session_id": interferer["source_session_id"],
            "source_parent_block_id": interferer["source_parent_block_id"],
            "source_duration_seconds": interference_info["duration_seconds"],
            "gain": interferer_gain,
            "offset_samples": interferer_offset_samples,
            "offset_seconds": round(interferer_offset_samples / SAMPLE_RATE, 6),
            "source_start_sample": 0,
            "used_samples": usable,
        }
    data = wav_bytes(mixed)
    input_info = copy_source_wav(output_path.parent, output_path, data)
    recipe = {
        "recipe_id": "mix-public-ascend-v1",
        "sample_rate_hz": SAMPLE_RATE,
        "target_source_case_id": target["case_id"],
        "target_source_path": target_source["source_path"],
        "target_source_audio_sha256": target_source["source_audio_sha256"],
        "target_source_pcm_sha256": target_source["source_pcm_sha256"],
        "target_speaker_id": target["source_speaker_id"],
        "target_session_id": target["source_session_id"],
        "target_parent_block_id": target["source_parent_block_id"],
        "target_source_start_sample": 0,
        "target_gain": target_gain,
        "target_source_duration_seconds": target_info["duration_seconds"],
        "input_samples": input_info["samples"],
        "interferer": interference_info,
        "input_audio_path": str(output_path),
        "input_audio_sha256": input_info["audio_sha256"],
        "input_pcm_sha256": input_info["pcm_sha256"],
        "input_rms_dbfs": pcm_stats(mixed)["rms_dbfs"],
        "input_peak_dbfs": pcm_stats(mixed)["peak_dbfs"],
        "input_clipped_samples": pcm_stats(mixed)["clipped_samples"],
        "mix_clipping_policy": "saturating PCM16; gains are fixed by case class and recorded",
    }
    target_component = {
        "source_path": target_source["source_path"],
        "source_samples": target_info["samples"],
        "source_rms_dbfs": pcm_stats(target_samples)["rms_dbfs"],
        "scaled_rms_dbfs": pcm_stats(target_scaled)["rms_dbfs"],
        "gain": target_gain,
        "active_frame_floor_dbfs": -80.0,
    }
    return recipe, target_component


def copy_simple_input(source_path: Path, destination: Path) -> dict:
    data = source_path.read_bytes()
    info, _ = wave_from_bytes(data, str(source_path))
    write_once(destination, data)
    return {"path": str(destination), **info}


def prepare_command(args) -> int:
    corpus_root = external_path(args.corpus_root, must_exist=True)
    candidate_root = external_path(args.candidate_root, must_exist=True)
    output_root = external_path(args.output_dir, must_exist=False)
    require(not output_root.exists(), f"choose a fresh output directory: {output_root}")
    require(args.max_source_bytes > 0 and args.max_source_bytes <= MAX_CORPUS_BYTES, "source byte cap must be at most 2 GiB")
    seed = args.seed
    audit, metadata_rows = audit_source(corpus_root, candidate_root, args.max_source_bytes)
    selected, coverage = select_rows(metadata_rows, seed)
    all_selected = (
        [row for rows in selected["enrollment"].values() for row in rows]
        + selected["targets"]
        + selected["interference"]
        + selected["other_only"]
    )
    unique_rows = {}
    for row in all_selected:
        unique_rows[(row["source_split"], row["source_id"])] = row
    require(len(unique_rows) == len(all_selected), "source audio reuse across roles is not allowed")
    mkdir_private(output_root)
    extracted = extract_selected_audio(
        corpus_root,
        output_root,
        list(unique_rows.values()),
        args.max_extracted_bytes,
    )
    profiles = []
    for speaker in TARGET_SPEAKERS:
        profile_id = selected["profiles_by_speaker"][speaker]
        profiles.append(combine_enrollment_clips(
            profile_id,
            selected["enrollment"][speaker],
            extracted,
            output_root,
        ))
    case_rows = []
    target_by_language = defaultdict(list)
    interferer_for_target = {
        row["paired_target_case_id"]: row for row in selected["interference"]
    }
    for target in selected["targets"]:
        target_by_language[target["language"]].append(target)
    soft_target_ids = set()
    for language in LANGUAGES:
        pool = [
            row for row in target_by_language[language]
            if row["publisher_duration_seconds"] >= 1.5
        ]
        require(len(pool) >= 2, f"not enough target-level stress cases for {language}")
        pool.sort(key=lambda row: (
            extracted[row["case_id"]]["source_rms_dbfs"],
            stable_key(row["case_id"], seed + f":soft:{language}"),
        ))
        speakers_seen = set()
        for row in pool:
            if row["source_speaker_id"] in speakers_seen:
                continue
            soft_target_ids.add(row["case_id"])
            speakers_seen.add(row["source_speaker_id"])
            if len(speakers_seen) >= 2:
                break
        require(len([row for row in pool if row["case_id"] in soft_target_ids]) >= 2, f"soft target-only case selection failed: {language}")
    short_case_ids = set()
    for language in LANGUAGES:
        candidates = [
            row for row in selected["targets"]
            if row["language"] == language
            and row["publisher_duration_seconds"] <= 2.5
        ]
        candidates.sort(key=lambda row: (row["publisher_duration_seconds"], stable_key(row["case_id"], seed + f":short-pair:{language}")))
        if candidates:
            short_case_ids.add(candidates[0]["case_id"])
    soft_pair_ids = set()
    for language in LANGUAGES:
        candidates = [
            row for row in selected["targets"]
            if row["language"] == language and row["case_id"] in soft_target_ids
            and row["case_id"] not in short_case_ids
        ]
        candidates.sort(key=lambda row: stable_key(row["case_id"], seed + f":soft-pair:{language}"))
        if candidates:
            soft_pair_ids.add(candidates[0]["case_id"])
    profile_ids = selected["profiles_by_speaker"]
    target_mix_offsets = (1600, 3200, 6400, 8000)
    language_index = {language: index for index, language in enumerate(LANGUAGES)}
    for language in LANGUAGES:
        targets = sorted(
            target_by_language[language],
            key=lambda row: stable_key(row["case_id"], seed + f":case-order:{language}"),
        )
        for index, target in enumerate(targets):
            case_id = (
                f"target-{language}-spk{target['source_speaker_id']}-"
                f"{target['source_id']}"
            )
            soft = target["case_id"] in soft_target_ids
            interferer = None if soft else interferer_for_target[ target["case_id"] ]
            target_gain = 0.10 if soft else 0.35
            interferer_gain = 0.0 if soft else 0.35
            offset = 0 if soft else target_mix_offsets[(index + language_index[language]) % len(target_mix_offsets)]
            input_path = output_root / "inputs" / f"{case_id}.wav"
            recipe, target_component = make_target_input(
                target,
                interferer,
                extracted,
                input_path,
                target_gain=target_gain,
                interferer_gain=interferer_gain,
                interferer_offset_samples=offset,
            )
            case = {
                "case_id": case_id,
                "role": "target_soft_level" if soft else "target_mixture",
                "language": language,
                "profile_id": profile_ids[target["source_speaker_id"]],
                "profile_source_speaker_id": target["source_speaker_id"],
                "target_source_case_id": target["case_id"],
                "target_speaker_id": target["source_speaker_id"],
                "target_session_id": target["source_session_id"],
                "target_parent_block_id": target["source_parent_block_id"],
                "target_source_audio_path": extracted[target["case_id"]]["source_path"],
                "target_source_audio_sha256": extracted[target["case_id"]]["source_audio_sha256"],
                "target_source_pcm_sha256": extracted[target["case_id"]]["source_pcm_sha256"],
                "source_split": target["source_split"],
                "project_partition_status": target["project_partition_status"],
                "publisher_reference": target["source_annotation"]["text"],
                "publisher_reference_sha256": target["source_annotation"]["sha256"],
                "reference_origin": target["source_annotation"]["origin"],
                "reference_project_review_status": target["source_annotation"]["project_review_status"],
                "reference_contains_unk": target["source_annotation"]["contains_unk"],
                "negation_candidate_from_publisher_text": target["negation_candidate"],
                "negation_review_status": "heuristic_candidate_not_human_reviewed",
                "publisher_duration_seconds": target["publisher_duration_seconds"],
                "short_candidate_at_most_2_5s": target["short_candidate"],
                "source_rms_dbfs": extracted[target["case_id"]]["source_rms_dbfs"],
                "soft_level_challenge": soft,
                "mix_recipe": recipe,
                "target_component": target_component,
                "asr_pair_selection": (
                    "short" if target["case_id"] in short_case_ids
                    else "soft" if target["case_id"] in soft_pair_ids
                    else None
                ),
                "asr_pair_audio_limit_seconds": 30,
            }
            case_rows.append(case)
    for index, row in enumerate(selected["other_only"]):
        case_id = f"other-only-{row['language']}-spk{row['source_speaker_id']}-{row['source_id']}"
        input_path = output_root / "inputs" / f"{case_id}.wav"
        source_audio = extracted[row["case_id"]]
        copy_simple_input(Path(source_audio["source_path"]), input_path)
        case_rows.append({
            "case_id": case_id,
            "role": "other_only",
            "language": row["language"],
            "profile_id": row["profile_id"],
            "profile_source_speaker_id": next(
                speaker for speaker, profile_id in profile_ids.items()
                if profile_id == row["profile_id"]
            ),
            "source_case_id": row["case_id"],
            "source_path": source_audio["source_path"],
            "source_audio_sha256": source_audio["source_audio_sha256"],
            "source_pcm_sha256": source_audio["source_pcm_sha256"],
            "source_speaker_id": row["source_speaker_id"],
            "source_session_id": row["source_session_id"],
            "source_parent_block_id": row["source_parent_block_id"],
            "source_split": row["source_split"],
            "project_partition_status": row["project_partition_status"],
            "publisher_reference": row["source_annotation"]["text"],
            "publisher_reference_sha256": row["source_annotation"]["sha256"],
            "reference_origin": row["source_annotation"]["origin"],
            "reference_project_review_status": row["source_annotation"]["project_review_status"],
            "reference_contains_unk": row["source_annotation"]["contains_unk"],
            "mix_recipe": {
                "recipe_id": "other-only-source-preserve-v1",
                "input_audio_path": str(input_path),
                "input_audio_sha256": sha256_file(input_path, MAX_WAV_BYTES),
                "target_component": None,
                "interferer_component": {
                    "source_case_id": row["case_id"],
                    "source_speaker_id": row["source_speaker_id"],
                    "source_parent_block_id": row["source_parent_block_id"],
                    "gain": 1.0,
                    "offset_samples": 0,
                },
            },
            "asr_pair_selection": None,
        })
    negative_root = candidate_root
    negative_names = (
        "negative-silence-3s.wav",
        "negative-silence-5s.wav",
        "negative-silence-8s.wav",
        "negative-noise-3s.wav",
        "negative-noise-5s.wav",
        "negative-noise-8s.wav",
    )
    negative_source_meta = read_json(negative_root / "negative-source.json")
    negative_draft = read_json(negative_root / "negative-draft.json")
    negative_candidate_review = negative_draft.get("review_status", "not specified")
    for index, name in enumerate(negative_names):
        source_path = negative_root / name
        require(source_path.is_file() and not source_path.is_symlink(), f"prepared synthetic negative missing: {source_path}")
        case_id = Path(name).stem
        input_path = output_root / "inputs" / f"{case_id}.wav"
        source_info = copy_simple_input(source_path, input_path)
        profile_id = profile_ids[TARGET_SPEAKERS[index % len(TARGET_SPEAKERS)]]
        case_rows.append({
            "case_id": case_id,
            "role": "negative_silence" if "silence" in name else "negative_synthetic_noise",
            "language": "other",
            "profile_id": profile_id,
            "profile_source_speaker_id": TARGET_SPEAKERS[index % len(TARGET_SPEAKERS)],
            "source_path": str(source_path),
            "source_audio_sha256": sha256_file(source_path, MAX_WAV_BYTES),
            "prepared_input_path": source_info["path"],
            "prepared_input_sha256": source_info["audio_sha256"],
            "source_kind": "synthetic",
            "generator_description": (
                negative_source_meta.get("cases", [{}])[index].get("generator")
                if isinstance(negative_source_meta.get("cases"), list)
                and index < len(negative_source_meta["cases"])
                else "existing prepared silence or seeded synthetic noise"
            ),
            "source_review_status": negative_candidate_review,
            "human_speech_presence": (
                "no human speech by construction" if "silence" in name
                else "synthetic non-speech generator; not a real ambient recording"
            ),
            "mix_recipe": {
                "recipe_id": "synthetic-negative-preserve-v1",
                "input_audio_path": source_info["path"],
                "input_audio_sha256": source_info["audio_sha256"],
                "target_component": None,
                "interferer_component": None,
            },
            "asr_pair_selection": None,
        })
    target_counts = Counter(case["language"] for case in case_rows if case["role"].startswith("target_"))
    actual_target_speakers = sorted({
        case["target_speaker_id"] for case in case_rows if "target_speaker_id" in case
    })
    actual_other_speakers = sorted({
        case["source_speaker_id"] for case in case_rows if case["role"] == "other_only"
    })
    actual_public_speakers = set(actual_target_speakers + actual_other_speakers)
    short_counts = Counter(
        case["language"] for case in case_rows
        if case["role"].startswith("target_") and case["short_candidate_at_most_2_5s"]
    )
    negation_counts = Counter(
        case["language"] for case in case_rows
        if case["role"].startswith("target_") and case["negation_candidate_from_publisher_text"]
    )
    soft_counts = Counter(case["language"] for case in case_rows if case["role"] == "target_soft_level")
    coverage.update({
        "actual_target_counts": dict(target_counts),
        "actual_short_candidate_counts": dict(short_counts),
        "actual_negation_candidate_counts": dict(negation_counts),
        "actual_soft_level_target_only_counts": dict(soft_counts),
        "actual_target_source_speaker_ids": actual_target_speakers,
        "actual_other_only_source_speaker_ids": actual_other_speakers,
        "actual_other_only_cases": sum(case["role"] == "other_only" for case in case_rows),
        "actual_negative_cases": sum(case["role"].startswith("negative_") for case in case_rows),
        "actual_total_cases": len(case_rows),
        "short_coverage_missing_by_language": {
            language: max(0, 2 - short_counts[language]) for language in LANGUAGES
        },
        "negation_coverage_missing_by_language": {
            language: max(0, 1 - negation_counts[language]) for language in LANGUAGES
        },
        "soft_coverage_missing_by_language": {
            language: max(0, 2 - soft_counts[language]) for language in LANGUAGES
        },
        "project_speaker_coverage_gap_to_12": max(
            0,
            12 - len(actual_public_speakers),
        ),
        "real_room_microphone": "not represented",
        "natural_low_voice": "unverified; soft-level cases use documented fixed -20 dB gain",
        "music_and_real_ambient_negative": "missing",
    })
    coverage["coverage_status"] = "coverage_gap" if (
        any(coverage["short_coverage_missing_by_language"].values())
        or any(coverage["negation_coverage_missing_by_language"].values())
        or any(coverage["soft_coverage_missing_by_language"].values())
        or coverage["project_speaker_coverage_gap_to_12"] > 0
    ) else "targeted_minimums_met_but_not_independent_holdout"
    cases_manifest = {
        "artifact_type": "voiceflow_speaker_filter_pilot_cases",
        "schema_version": 1,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "source_audit": audit,
        "selection_seed": seed,
        "selection_method": {
            "target_speech": "72 selected from pinned ASCEND validation; 24/language, 8 from each of source speakers 7/12/20",
            "enrollment": "temporary local voiceprint input composed from separate ASCEND validation session-1 clips; target evaluation uses sessions 2-4",
            "interferers": "publisher test split speaker ids 3/17, audio case ids unique and disjoint from target/enrollment source audio",
            "other_only": "12 unique ASCEND publisher test-split recordings; four tested per temporary profile",
            "negative": "six already-prepared synthetic silence/noise cases from the public corpus preparation directory",
            "mixing": "same-rate PCM16 linear mix; target/interferer source gains fixed per role, no time stretch or resampling",
            "reference_status": "publisher manual annotations retained verbatim; project review pending",
            "consent": "not inferred from public release or CC license; no person names or identity claims",
        },
        "coverage": coverage,
        "profiles": profiles,
        "cases": case_rows,
    }
    manifest_path = output_root / "manifest.json"
    write_json_once(manifest_path, cases_manifest)
    summary = {
        "artifact_type": "voiceflow_speaker_filter_pilot_preparation_report",
        "schema_version": 1,
        "manifest_path": str(manifest_path),
        "manifest_sha256": sha256_file(manifest_path, MAX_JSON_BYTES),
        "source_audit": audit,
        "coverage": coverage,
        "profile_count": len(profiles),
        "case_count": len(case_rows),
        "prepared_input_wav_count": len(case_rows) + len(profiles),
        "derived_audio_total_bytes": sum(
            Path(case.get("mix_recipe", {}).get("input_audio_path") or case.get("prepared_input_path", "")).stat().st_size
            for case in case_rows
        ),
        "provider_calls": 0,
        "paid_inference": False,
        "training": False,
    }
    write_json_once(output_root / "preparation-report.json", summary)
    csv_path = output_root / "case-list.csv"
    with csv_path.open("x", newline="", encoding="utf-8") as stream:
        os.chmod(csv_path, 0o600)
        writer = csv.DictWriter(
            stream,
            fieldnames=[
                "case_id", "role", "language", "input_path",
                "reference", "reference_project_review_status",
                "source_speaker_id", "source_session_id", "parent_block_id",
                "profile_id", "profile_source_speaker_id", "source_split",
                "target_source_case_id", "interferer_source_case_id",
            ],
        )
        writer.writeheader()
        for case in case_rows:
            recipe = case.get("mix_recipe", {})
            target_source_id = case.get("target_source_case_id") or case.get("source_case_id")
            interferer = recipe.get("interferer") or recipe.get("interferer_component") or {}
            writer.writerow({
                "case_id": case["case_id"],
                "role": case["role"],
                "language": case["language"],
                "input_path": recipe.get("input_audio_path") or case.get("prepared_input_path"),
                "reference": case.get("publisher_reference", ""),
                "reference_project_review_status": case.get("reference_project_review_status", case.get("source_review_status", "")),
                "source_speaker_id": case.get("target_speaker_id", case.get("source_speaker_id", "")),
                "source_session_id": case.get("target_session_id", case.get("source_session_id", "")),
                "parent_block_id": case.get("target_parent_block_id", case.get("source_parent_block_id", "")),
                "profile_id": case.get("profile_id", ""),
                "profile_source_speaker_id": case.get("profile_source_speaker_id", ""),
                "source_split": case.get("source_split", ""),
                "target_source_case_id": target_source_id,
                "interferer_source_case_id": interferer.get("source_case_id", ""),
            })
        stream.flush()
        os.fsync(stream.fileno())
    print(json.dumps({
        "prepared": str(output_root),
        "manifest": str(manifest_path),
        "manifest_sha256": summary["manifest_sha256"],
        "cases": len(case_rows),
        "profiles": len(profiles),
        "coverage_status": coverage["coverage_status"],
        "target_counts": coverage["actual_target_counts"],
        "other_only": coverage["actual_other_only_cases"],
        "negative": coverage["actual_negative_cases"],
        "provider_calls": 0,
    }, ensure_ascii=False))
    return 0


def profile_hash(path: Path) -> str:
    require(path.is_file() and not path.is_symlink(), f"profile output missing: {path}")
    require(path.stat().st_size <= 256 * 1024, f"profile output byte budget exceeded: {path}")
    os.chmod(path, 0o600)
    return sha256_file(path, 256 * 1024)


def verify_models(model_root: Path) -> dict:
    receipt_path = model_root.parent / "model-acquisition.json"
    receipt = read_json(receipt_path)
    require(receipt.get("artifact_type") == "speaker_filter_model_acquisition_receipt", "pinned private model receipt required")
    require(receipt.get("source_commit") == "28fe044787952af5b6b6e59b3dd92de2b12eee36", "pinned model source commit mismatch")
    require(receipt.get("models_whitelist") == ["focus", "cue", "timbre"], "only Focus/Cue/Timbre are allowed")
    require(receipt.get("autoupdate") is False, "model auto-update must remain disabled")
    model_items = [item for item in receipt.get("downloaded_files", []) if item.get("artifact_type") == "weight"]
    require(len(model_items) == 3, "exactly three pinned model weights required")
    expected_names = {
        "alkove-focus-1-preview.alk",
        "alkove-cue-1-preview.alk",
        "alkove-timbre-1-preview.alk",
    }
    require(
        {Path(item.get("path", "")).name for item in model_items} == expected_names,
        "pinned model receipt must identify one Focus, Cue, and Timbre weight",
    )
    total = 0
    verified = []
    for item in model_items:
        path = Path(item["path"])
        require(
            not path.is_symlink() and path.parent.resolve() == model_root.resolve(),
            "model weight must be a regular direct model-root child",
        )
        digest = sha256_file(path, 4 * MIB)
        require(path.stat().st_size == item["bytes"] and digest == item["sha256"], f"pinned model verification failed: {path.name}")
        total += path.stat().st_size
        verified.append({"name": path.name, "bytes": path.stat().st_size, "sha256": digest})
    require(total == 2917844, f"pinned model weight byte total mismatch: {total}")
    by_key = {
        "focus": next(item for item in verified if item["name"] == "alkove-focus-1-preview.alk"),
        "cue": next(item for item in verified if item["name"] == "alkove-cue-1-preview.alk"),
        "timbre": next(item for item in verified if item["name"] == "alkove-timbre-1-preview.alk"),
    }
    digest = hashlib.sha256()
    digest.update(b"voiceflow-speaker-filter-model-set-v1\0")
    for key in ("focus", "cue", "timbre"):
        digest.update(key.encode())
        digest.update(b"\0")
        digest.update(bytes.fromhex(by_key[key]["sha256"]))
    return {
        "receipt_path": str(receipt_path),
        "receipt_sha256": sha256_file(receipt_path, 4 * MIB),
        "source_commit": receipt["source_commit"],
        "models": verified,
        "weight_bytes_total": total,
        "models_digest_sha256": digest.hexdigest(),
    }


def local_asr_trigger(report_path: Path | None) -> tuple[bool, dict | None]:
    if report_path is None:
        return False, None
    report = read_json(report_path, 2 * MIB)
    require(report.get("artifact_type") in (
        "voiceflow_speaker_filter_asr_pair_review",
        "voiceflow_speaker_filter_asr_pair_report",
        "voiceflow_native_asr_pair_report",
    ), "unsupported local-ASR evidence report")
    trigger = report.get("g1_trigger")
    eligible = isinstance(trigger, dict) and trigger.get("eligible") is True
    return eligible, {
        "path": str(report_path),
        "sha256": sha256_file(report_path, 2 * MIB),
        "trigger": trigger,
        "status": report.get("status"),
    }


def g1_precondition(
    g0_report_path: Path | None,
    asr_report_path: Path | None,
    expected_manifest_sha256: str,
) -> dict:
    require(g0_report_path is not None, "G1 requires a G0 report")
    require(
        isinstance(expected_manifest_sha256, str)
        and re.fullmatch(r"[0-9a-f]{64}", expected_manifest_sha256) is not None,
        "G1 requires the selected manifest SHA-256",
    )
    g0 = read_json(g0_report_path, 8 * MIB)
    require(g0.get("artifact_type") == "voiceflow_speaker_filter_pilot_report", "G1 requires a native speaker-filter G0 report")
    require(g0.get("recipe") == "g0" and g0.get("status") == "complete", "G1 requires completed G0 evidence")
    require(
        g0.get("manifest_sha256") == expected_manifest_sha256,
        "G1 G0 report/selected manifest identity mismatch",
    )
    trigger = g0.get("g1_trigger")
    require(isinstance(trigger, dict), "G1 requires a valid G0 trigger record")
    wave_trigger = (
        trigger.get("eligible") is True
        and trigger.get("low_volume_target_effectively_zero_or_truncated") is True
    )
    asr_trigger, asr_evidence = local_asr_trigger(asr_report_path)
    require(wave_trigger or asr_trigger, "G1 is gated: no low-volume waveform loss or paired local-ASR degradation evidence")
    return {
        "g0_report_path": str(g0_report_path),
        "g0_report_sha256": sha256_file(g0_report_path, 8 * MIB),
        "manifest_sha256": expected_manifest_sha256,
        "g0_waveform_trigger": wave_trigger,
        "local_asr_trigger": asr_trigger,
        "local_asr_evidence": asr_evidence,
    }


def run_native(binary: Path, argv: list[str], timeout_seconds: int) -> tuple[int, bytes, bytes, float, bool]:
    start = time.monotonic()
    try:
        process = subprocess.run(
            [str(binary), *argv],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout_seconds,
            check=False,
            env={**os.environ, "RUST_BACKTRACE": "0", "RUST_LOG": "error"},
        )
        elapsed = time.monotonic() - start
        return process.returncode, process.stdout[:128 * 1024], process.stderr[:128 * 1024], elapsed, False
    except subprocess.TimeoutExpired as error:
        elapsed = time.monotonic() - start
        stdout = error.stdout if isinstance(error.stdout, bytes) else b""
        stderr = error.stderr if isinstance(error.stderr, bytes) else b""
        return 124, stdout[:128 * 1024], stderr[:128 * 1024], elapsed, True


def timing_semantics(paced: bool) -> dict:
    if paced:
        rtf_meaning = (
            "native-helper processing/audio-duration ratio includes simulated replay waits from --paced; excludes "
            "model-file loading, WAV read/write, and session initialization; descriptive paced latency, not compute-only RTF"
        )
    else:
        rtf_meaning = (
            "native-helper processing/audio-duration ratio with unpaced input; includes Focus/Cue inference, hop "
            "buffering, and flush; excludes model-file loading, WAV read/write, and session initialization; descriptive pilot only"
        )
    return {
        "native_real_time_factor": rtf_meaning,
        "native_command_wall_elapsed_ms": (
            "wall-clock command duration including helper work and any --paced replay waits; raw milliseconds, "
            "not divided by audio duration and not an RTF"
        ),
    }


def target_frame_metrics(case: dict, output_samples: tuple[int, ...]) -> dict:
    component = case.get("target_component")
    if not component or case.get("role") != "target_soft_level":
        return {"applicable": False}
    _, source_samples = wave_from_bytes(Path(component["source_path"]).read_bytes(), component["source_path"])
    gain = float(component["gain"])
    target = tuple(scaled_sample(sample, gain) for sample in source_samples)
    frame_rows = []
    for start in range(0, min(len(target), len(output_samples)), FRAME_SAMPLES):
        end = min(start + FRAME_SAMPLES, len(target), len(output_samples))
        if end <= start:
            continue
        target_frame = target[start:end]
        output_frame = output_samples[start:end]
        target_rms = math.sqrt(sum(sample * sample for sample in target_frame) / len(target_frame)) / 32768.0
        output_rms = math.sqrt(sum(sample * sample for sample in output_frame) / len(output_frame)) / 32768.0
        if target_rms >= 1e-4:
            frame_rows.append({
                "start_sample": start,
                "target_rms": target_rms,
                "output_rms": output_rms,
                "loss": output_rms <= max(1e-4, target_rms * 0.001),
            })
    if not frame_rows:
        return {
            "applicable": True,
            "active_target_frames": 0,
            "active_frame_coverage": None,
            "target_output_gain_db": None,
            "effective_zero": False,
            "truncated": False,
            "signal_gate_unknown": True,
        }
    kept = sum(not row["loss"] for row in frame_rows)
    target_energy = sum(row["target_rms"] ** 2 for row in frame_rows)
    output_energy = sum(row["output_rms"] ** 2 for row in frame_rows)
    gain_ratio = math.sqrt(output_energy / target_energy) if target_energy else 0.0
    ordered_loss = [row["loss"] for row in frame_rows]
    edge_count = min(4, len(ordered_loss))
    onset_lost = edge_count >= 3 and all(ordered_loss[:3])
    tail_lost = edge_count >= 3 and all(ordered_loss[-3:])
    effective_zero = kept == 0
    return {
        "applicable": True,
        "active_target_frames": len(frame_rows),
        "kept_target_frames": kept,
        "lost_target_frames": len(frame_rows) - kept,
        "active_frame_coverage": round(kept / len(frame_rows), 4),
        "target_output_gain_db": round(20 * math.log10(max(gain_ratio, 1e-12)), 3),
        "onset_active_frames_lost": onset_lost,
        "tail_active_frames_lost": tail_lost,
        "effective_zero": effective_zero,
        "truncated": onset_lost or tail_lost,
        "signal_gate_unknown": False,
        "frame_floor": "target RMS >= -80 dBFS; output is lost below max(-80 dBFS, target frame -60 dB)",
    }


def write_case_result(output_dir: Path, case_id: str, result: dict) -> Path:
    path = output_dir / "case-results" / f"{case_id}.json"
    write_json_once(path, result)
    return path


def run_command(args) -> int:
    manifest_path = external_path(args.manifest, must_exist=True)
    manifest = read_json(manifest_path)
    require(manifest.get("artifact_type") == "voiceflow_speaker_filter_pilot_cases", "speaker-filter pilot manifest required")
    require(manifest.get("schema_version") == 1, "unsupported speaker-filter manifest version")
    manifest_sha256 = sha256_file(manifest_path, MAX_JSON_BYTES)
    require(sha256_file(manifest_path, MAX_JSON_BYTES) == manifest_sha256, "manifest changed during preflight")
    cases, profiles = validate_selected_cases(manifest, args.case_id)
    preflight_audio = preflight_pilot_audio(cases, profiles)
    output_dir = external_path(args.output_dir)
    require(not output_dir.exists(), f"choose a fresh run directory: {output_dir}")
    gate_evidence = None
    if args.recipe == "g1":
        gate_evidence = g1_precondition(
            external_path(args.g0_report, must_exist=True) if args.g0_report else None,
            external_path(args.asr_pair_report, must_exist=True) if args.asr_pair_report else None,
            manifest_sha256,
        )
    model_root = external_path(args.models, must_exist=True)
    model_audit = verify_models(model_root)
    binary = external_path(args.native_bin, must_exist=True)
    require(binary.is_file() and os.access(binary, os.X_OK), f"executable native helper required: {binary}")
    mkdir_private(output_dir)
    for name in ("profiles", "profile-results", "case-results", "filtered", "native-reports"):
        mkdir_private(output_dir / name)
    mkdir_private(output_dir / "filtered" / args.recipe)
    mkdir_private(output_dir / "native-reports" / args.recipe)
    header = {
        "artifact_type": "voiceflow_speaker_filter_pilot_run_header",
        "schema_version": 1,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "recipe": args.recipe,
        "paced": bool(args.paced),
        "manifest_path": str(manifest_path),
        "manifest_sha256": manifest_sha256,
        "model_audit": model_audit,
        "native_binary_path": str(binary),
        "native_binary_sha256": sha256_file(binary, 512 * MIB),
        "gate_evidence": gate_evidence,
        "provider_calls": 0,
        "paid_inference": False,
        "training": False,
        "case_ids": [case["case_id"] for case in cases],
        "timing_semantics": timing_semantics(bool(args.paced)),
    }
    write_json_once(output_dir / "run-header.json", header)
    enrollment_results = {}
    for profile in profiles:
        profile_id = profile["profile_id"]
        profile_input_path, profile_input_info, _ = verify_pcm16_artifact(
            profile.get("input_audio_path"),
            profile.get("input_audio_sha256"),
            profile.get("input_pcm_sha256"),
            profile.get("input_samples"),
            f"enrollment input {profile_id}",
        )
        require(
            profile_input_info["audio_sha256"] == preflight_audio[f"profile:{profile_id}"]["audio_sha256"],
            f"enrollment input changed after preflight: {profile_id}",
        )
        profile_output = output_dir / "profiles" / f"{profile_id}.json"
        argv = [
            "enroll",
            "--models", str(model_root),
            "--input", str(profile_input_path),
            "--output", str(profile_output),
        ]
        code, stdout, stderr, elapsed, timed_out = run_native(binary, argv, 180)
        if code == 0 and profile_output.is_file() and not timed_out:
            digest = profile_hash(profile_output)
            enrollment_results[profile_id] = {
                "status": "complete",
                "profile_path": str(profile_output),
                "profile_sha256": digest,
                "profile_bytes": profile_output.stat().st_size,
                "elapsed_ms": round(elapsed * 1000, 3),
                "stdout_sha256": sha256_bytes(stdout),
                "stderr_sha256": sha256_bytes(stderr),
            }
        else:
            enrollment_results[profile_id] = {
                "status": "failed",
                "exit_code": code,
                "timed_out": timed_out,
                "elapsed_ms": round(elapsed * 1000, 3),
                "stdout_sha256": sha256_bytes(stdout),
                "stderr_sha256": sha256_bytes(stderr),
                "stderr_excerpt": stderr.decode("utf-8", "replace")[:1000],
            }
        write_json_once(
            output_dir / "profile-results" / f"{profile_id}.json",
            {"profile_id": profile_id, **enrollment_results[profile_id]},
        )
    case_results = []
    run_start = time.monotonic()
    for case in cases:
        case_id = case["case_id"]
        recipe = case.get("mix_recipe") or {}
        input_path = Path(preflight_audio[case_id]["path"])
        input_data = input_path.read_bytes()
        input_info, input_samples = wave_from_bytes(input_data, str(input_path))
        require(
            input_info["audio_sha256"] == preflight_audio[case_id]["audio_sha256"],
            f"case input changed after preflight: {case_id}",
        )
        profile_id = case["profile_id"]
        enrolled = enrollment_results.get(profile_id)
        profile_path = Path(enrolled["profile_path"]) if enrolled and enrolled.get("status") == "complete" else None
        case_start = time.monotonic()
        output_path = output_dir / "filtered" / args.recipe / f"{case_id}.wav"
        native_report_path = output_dir / "native-reports" / args.recipe / f"{case_id}.json"
        result = {
            "case_id": case_id,
            "role": case["role"],
            "language": case["language"],
            "recipe": args.recipe,
            "paced": bool(args.paced),
            "profile_id": profile_id,
            "input_path": str(input_path),
            "input_sha256": input_info["audio_sha256"],
            "input_samples": input_info["samples"],
            "input_stats": pcm_stats(input_samples),
            "profile_path": str(profile_path) if profile_path else None,
            "profile_sha256": enrolled.get("profile_sha256") if enrolled else None,
            "target_source_case_id": case.get("target_source_case_id"),
            "target_source_audio_sha256": case.get("target_source_audio_sha256"),
            "target_source_speaker_id": case.get("target_speaker_id"),
            "target_source_parent_block_id": case.get("target_parent_block_id"),
            "target_source_session_id": case.get("target_session_id"),
            "publisher_reference_sha256": case.get("publisher_reference_sha256"),
            "reference_project_review_status": case.get("reference_project_review_status"),
            "soft_level_challenge": case.get("soft_level_challenge", False),
            "asr_pair_selection": case.get("asr_pair_selection"),
            "source_signal": None,
        }
        if case.get("target_component"):
            result["source_signal"] = {
                "source_path": case["target_component"]["source_path"],
                "source_rms_dbfs": case["target_component"]["source_rms_dbfs"],
                "scaled_rms_dbfs": case["target_component"]["scaled_rms_dbfs"],
                "gain": case["target_component"]["gain"],
            }
        if not enrolled or enrolled.get("status") != "complete":
            result.update({"status": "skipped_profile_enrollment_failed", "native_elapsed_ms": None})
            result_path = write_case_result(output_dir, case_id, result)
            result["result_path"] = str(result_path)
            case_results.append(result)
            continue
        argv = [
            "filter",
            "--models", str(model_root),
            "--profile", str(profile_path),
            "--input", str(input_path),
            "--output", str(output_path),
            "--recipe", args.recipe,
            "--report", str(native_report_path),
        ]
        if args.paced:
            argv.append("--paced")
        audio_seconds = result["input_samples"] / SAMPLE_RATE
        timeout_seconds = min(300, max(60, int(audio_seconds * 8 + 30)))
        code, stdout, stderr, elapsed, timed_out = run_native(binary, argv, timeout_seconds)
        result["native_elapsed_ms"] = round(elapsed * 1000, 3)
        result["native_command_wall_elapsed_ms"] = round(elapsed * 1000, 3)
        result["native_stdout_sha256"] = sha256_bytes(stdout)
        result["native_stderr_sha256"] = sha256_bytes(stderr)
        if code != 0 or timed_out or not output_path.is_file() or not native_report_path.is_file():
            result.update({
                "status": "failed",
                "exit_code": code,
                "timed_out": timed_out,
                "stderr_excerpt": stderr.decode("utf-8", "replace")[:1000],
            })
        else:
            output_data = output_path.read_bytes()
            output_info, output_float_samples = float_wave_from_bytes(output_data, str(output_path))
            output_pcm16_samples = encode_production_pcm16(output_float_samples)
            native_report = read_json(native_report_path, MAX_REPORT_BYTES)
            input_sha = result["input_sha256"]
            output_sha = output_info["audio_sha256"]
            profile_sha = enrolled["profile_sha256"]
            native_ok = (
                native_report.get("schema_version") == 1
                and native_report.get("status") == "ok"
                and native_report.get("upstream_revision") == model_audit["source_commit"]
                and native_report.get("recipe") == args.recipe
                and native_report.get("models_digest_sha256") == model_audit["models_digest_sha256"]
                and native_report.get("input_sha256") == input_sha
                and native_report.get("output_sha256") == output_sha
                and native_report.get("profile_sha256") == profile_sha
                and native_report.get("input_samples") == result["input_samples"]
                and native_report.get("output_samples") == result["input_samples"]
                and output_info["samples"] == result["input_samples"]
            )
            result.update({
                "status": "complete" if native_ok else "native_report_or_timeline_mismatch",
                "output_path": str(output_path),
                "output_sha256": output_sha,
                "output_pcm_sha256": output_info["pcm_sha256"],
                "output_samples": output_info["samples"],
                "output_duration_seconds": output_info["duration_seconds"],
                "output_stats": float_stats(output_float_samples),
                "production_pcm16_stats": pcm_stats(output_pcm16_samples),
                "production_pcm16_conversion": "f32 clamp[-1,1], f32 multiply by 32767, truncate toward zero; no normalization",
                "output_sample_format": output_info["sample_format"],
                "native_report_path": str(native_report_path),
                "native_report_sha256": sha256_file(native_report_path, MAX_REPORT_BYTES),
                "native_report": native_report,
                "timeline": {
                    key: native_report.get(key)
                    for key in (
                        "model_delay_samples",
                        "discarded_leading_samples",
                        "padded_trailing_samples",
                        "hop_padding_samples",
                        "flush_padding_samples",
                        "cue_observation_frames",
                    )
                },
                "native_processing_elapsed_ms": native_report.get("processing_elapsed_ms"),
                "native_real_time_factor": native_report.get("real_time_factor"),
                "native_real_time_factor_scope": "includes_paced_replay_waits" if args.paced else "unpaced_helper_processing",
                "target_frame_metrics": target_frame_metrics(case, output_pcm16_samples),
            })
            if not native_ok:
                result["native_validation_failure"] = "report hashes/sample counts/status do not match the input/output files"
        result["case_wall_elapsed_ms"] = round((time.monotonic() - case_start) * 1000, 3)
        result_path = write_case_result(output_dir, case_id, result)
        result["result_path"] = str(result_path)
        case_results.append(result)
        print(
            f"{case_id}: {result['status']} "
            f"duration={audio_seconds:.2f}s wall={result['case_wall_elapsed_ms']:.0f}ms",
            flush=True,
        )
    total_elapsed = time.monotonic() - run_start
    failures = [
        result for result in case_results
        if result["status"] != "complete"
    ]
    soft_results = [
        result for result in case_results
        if result.get("role") == "target_soft_level"
        and result.get("target_frame_metrics", {}).get("applicable")
    ]
    low_volume_loss = [
        result for result in soft_results
        if result.get("target_frame_metrics", {}).get("effective_zero")
        or result.get("target_frame_metrics", {}).get("truncated")
    ]
    by_role = Counter(result["role"] for result in case_results)
    by_language = Counter(result["language"] for result in case_results)
    valid_rtf = [
        result["native_real_time_factor"]
        for result in case_results
        if isinstance(result.get("native_real_time_factor"), (int, float))
        and math.isfinite(result["native_real_time_factor"])
    ]
    g1_trigger = {
        "eligible": bool(low_volume_loss),
        "low_volume_target_effectively_zero_or_truncated": bool(low_volume_loss),
        "criterion": (
            "for a fixed-gain soft target-only case, every target-active 10 ms frame falls below "
            "max(-80 dBFS, target-frame level - 60 dB), or three consecutive active onset/tail frames are absent; "
            "this is a signal-level preservation trigger, not an ASR-quality claim"
        ),
        "case_ids": [result["case_id"] for result in low_volume_loss],
    }
    asr_pair_artifacts = create_asr_pair_manifest(output_dir, manifest, case_results, args.recipe)
    report = {
        "artifact_type": "voiceflow_speaker_filter_pilot_report",
        "schema_version": 1,
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": "complete" if not failures else "completed_with_failures",
        "recipe": args.recipe,
        "paced": bool(args.paced),
        "manifest_path": str(manifest_path),
        "manifest_sha256": header["manifest_sha256"],
        "model_audit": model_audit,
        "native_binary_path": str(binary),
        "native_binary_sha256": header["native_binary_sha256"],
        "run_directory": str(output_dir),
        "profile_results": enrollment_results,
        "case_count": len(case_results),
        "case_counts_by_role": dict(by_role),
        "case_counts_by_language": dict(by_language),
        "completed_cases": sum(result["status"] == "complete" for result in case_results),
        "failed_or_skipped_cases": len(failures),
        "failure_case_ids": [result["case_id"] for result in failures],
        "wall_elapsed_seconds": round(total_elapsed, 3),
        "native_real_time_factor": {
            "n": len(valid_rtf),
            "median": None if not valid_rtf else round(sorted(valid_rtf)[len(valid_rtf) // 2], 6),
            "p95_observed": None if not valid_rtf else round(sorted(valid_rtf)[min(len(valid_rtf) - 1, math.ceil(len(valid_rtf) * 0.95) - 1)], 6),
            "meaning": timing_semantics(bool(args.paced))["native_real_time_factor"],
        },
        "timing_semantics": timing_semantics(bool(args.paced)),
        "low_volume_target_signal_gate": {
            "soft_target_cases": len(soft_results),
            "effective_zero_or_truncated_cases": len(low_volume_loss),
            "cases": [result["case_id"] for result in soft_results],
            "signal_gate_known_cases": sum(not result["target_frame_metrics"].get("signal_gate_unknown", False) for result in soft_results),
        },
        "asr_pair_artifacts": asr_pair_artifacts,
        "g1_trigger": g1_trigger,
        "audio_quality_metrics": {
            "reported": [
                "input/output RMS, peak, zero and clipped sample counts",
                "output duration and sample count equality",
                "model delay, discarded leading and padded trailing samples",
                "soft target-only active-frame retention",
                "other-only and synthetic-negative output level ratios",
            ],
            "not_claimed": [
                "WER/CER or ASR quality",
                "human listening quality",
                "consent",
                "speaker-independent statistical generalization",
            ],
        },
        "coverage": manifest["coverage"],
        "reference_status": "publisher manual annotations; project review pending; not reviewed gold",
        "holdout_status": "publisher test split speaker IDs are distinct from target IDs but this is not a project locked holdout",
        "scope": {
            "training": False,
            "paid_inference": False,
            "cloud_audio_calls": 0,
            "network_access_by_runner": False,
            "production_integration": False,
            "public_voice_profiles": "temporary local pilot profiles; not formal user profiles",
            "consent": "not inferred from publication or license",
        },
    }
    report_path = output_dir / "pilot-report.json"
    write_json_once(report_path, report)
    print(json.dumps({
        "report": str(report_path),
        "report_sha256": sha256_file(report_path, MAX_JSON_BYTES),
        "status": report["status"],
        "recipe": args.recipe,
        "cases": report["case_count"],
        "completed": report["completed_cases"],
        "failed": report["failed_or_skipped_cases"],
        "g1_trigger": g1_trigger,
        "paced": bool(args.paced),
    }, ensure_ascii=False))
    return 0 if not failures else 2


def create_asr_pair_manifest(
    output_dir: Path,
    prepared_manifest: dict,
    results: list[dict],
    recipe: str,
    all_cases: bool = False,
) -> dict:
    selected_cases = [
        case for case in prepared_manifest.get("cases", [])
        if all_cases or case.get("asr_pair_selection") in ("short", "soft")
    ]
    selected_by_id = {
        case["case_id"]: case for case in selected_cases
    }
    require(len(selected_by_id) == len(selected_cases), "duplicate ASR pair case identity")
    result_by_id = {result["case_id"]: result for result in results}
    require(len(result_by_id) == len(results), "duplicate native case result identity")
    if all_cases:
        require(
            set(selected_by_id) <= set(result_by_id)
            and all(result_by_id[case_id].get("status") == "complete" for case_id in selected_by_id),
            "all-case ASR pair derivation requires a completed result for every case",
        )
    cases = []
    provenance = []
    pcm16_root = output_dir / "asr-pcm16"
    mkdir_private(pcm16_root)
    for case_id, case in selected_by_id.items():
        result = result_by_id.get(case_id)
        if not result or result.get("status") != "complete":
            continue
        input_path = external_path(case["mix_recipe"]["input_audio_path"], must_exist=True)
        require(not input_path.is_symlink(), f"regular ASR source input required: {case_id}")
        require(input_path.stat().st_size <= MAX_WAV_BYTES, f"ASR source input exceeds WAV byte budget: {case_id}")
        input_data = input_path.read_bytes()
        input_info, input_samples = wave_from_bytes(input_data, str(input_path))
        expected_input_sha = case["mix_recipe"].get("input_audio_sha256")
        require(input_info["audio_sha256"] == expected_input_sha, f"ASR source input identity mismatch: {case_id}")
        require(result.get("input_sha256") == expected_input_sha, f"G0 result input identity mismatch: {case_id}")
        native_report = result.get("native_report")
        require(
            isinstance(native_report, dict)
            and native_report.get("status") == "ok"
            and native_report.get("recipe") == recipe
            and native_report.get("input_sha256") == expected_input_sha,
            f"native report input identity mismatch: {case_id}",
        )
        require(
            input_info["samples"] == result.get("input_samples") == native_report.get("input_samples"),
            f"ASR source timeline identity mismatch: {case_id}",
        )
        output_path = external_path(result["output_path"], must_exist=True)
        require(not output_path.is_symlink(), f"regular native output required: {case_id}")
        require(output_path.stat().st_size <= MAX_WAV_BYTES, f"native output exceeds WAV byte budget: {case_id}")
        raw_normalized = int16_to_normalized_float(input_samples)
        raw_pcm16 = encode_production_pcm16(raw_normalized)
        output_info, output_float = float_wave_from_bytes(output_path.read_bytes(), str(output_path))
        require(output_info["audio_sha256"] == result.get("output_sha256"), f"native output identity mismatch: {case_id}")
        require(output_info["audio_sha256"] == native_report.get("output_sha256"), f"native report output identity mismatch: {case_id}")
        require(output_info["samples"] == input_info["samples"] == native_report.get("output_samples"), f"native output timeline mismatch: {case_id}")
        filtered_pcm16 = encode_production_pcm16(output_float)
        raw_pcm16_path = pcm16_root / f"{case_id}-raw.wav"
        filtered_pcm16_path = pcm16_root / f"{case_id}-{recipe}.wav"
        write_once(raw_pcm16_path, wav_bytes(raw_pcm16))
        write_once(filtered_pcm16_path, wav_bytes(filtered_pcm16))
        raw_pcm16_sha = sha256_file(raw_pcm16_path, MAX_WAV_BYTES)
        filtered_pcm16_sha = sha256_file(filtered_pcm16_path, MAX_WAV_BYTES)
        pair_type = case.get("asr_pair_selection") or case.get("role") or "unspecified"
        for condition, audio_path, audio_sha in (
            ("raw", raw_pcm16_path, raw_pcm16_sha),
            (recipe, filtered_pcm16_path, filtered_pcm16_sha),
        ):
            cases.append({
                "id": f"{case_id}-{condition}",
                "audio_path": str(audio_path),
                "audio_sha256": audio_sha,
                "reference": case.get("publisher_reference"),
                "language": case["language"],
                "condition": f"{condition}-{pair_type}",
                "role": case.get("role"),
            })
        provenance.append({
            "case_id": case_id,
            "role": case.get("role"),
            "language": case["language"],
            "pair_type": pair_type,
            "reference": case.get("publisher_reference"),
            "reference_origin": case.get("reference_origin"),
            "reference_project_review_status": case.get("reference_project_review_status"),
            "source_annotation_sha256": case.get("publisher_reference_sha256"),
            "raw_input_path": str(input_path),
            "raw_input_sha256": sha256_file(input_path, MAX_WAV_BYTES),
            "raw_pcm16_path": str(raw_pcm16_path),
            "raw_pcm16_audio_sha256": raw_pcm16_sha,
            "native_float_output_path": str(output_path),
            "native_float_output_sha256": result["output_sha256"],
            "filtered_pcm16_path": str(filtered_pcm16_path),
            "filtered_pcm16_audio_sha256": filtered_pcm16_sha,
            "pcm16_conversion": "VoiceFlow audio.rs::encode: f32 clamp[-1,1], f32 multiply by 32767, truncate toward zero; no normalization",
            "profile_id": case["profile_id"],
            "profile_sha256": result["profile_sha256"],
            "target_source_case_id": case.get("target_source_case_id"),
            "target_source_speaker_id": case.get("target_speaker_id"),
            "target_session_id": case.get("target_session_id"),
            "target_parent_block_id": case.get("target_parent_block_id"),
        })
    path = output_dir / "asr-pairs-manifest.json"
    write_json_once(path, {"cases": cases})
    provenance_path = output_dir / "asr-pairs-provenance.json"
    write_json_once(provenance_path, {
        "artifact_type": "voiceflow_speaker_filter_asr_pairs_provenance",
        "schema_version": 1,
        "recipe": recipe,
        "cases": provenance,
        "reference_status": (
            "all pilot cases included; publisher annotations are project-review-pending and are not reviewed gold; "
            "negative silence/noise cases have no publisher text reference"
            if all_cases
            else "publisher manual annotations; project review pending; not reviewed gold"
        ),
        "conversion": "raw and native float32 outputs use the same VoiceFlow PCM16 encoder before local ASR",
    })
    return {
        "manifest_path": str(path),
        "manifest_sha256": sha256_file(path, MAX_JSON_BYTES),
        "provenance_path": str(provenance_path),
        "provenance_sha256": sha256_file(provenance_path, MAX_JSON_BYTES),
        "pair_count": len(provenance),
        "audio_row_count": len(cases),
        "selection": "all prepared cases" if all_cases else "manifest-declared short/soft ASR pairs",
        "conversion": "VoiceFlow audio.rs::encode, f32 clamp[-1,1], f32 multiply by 32767, truncate toward zero; no normalization",
    }


def asr_pairs_command(args) -> int:
    manifest_path = external_path(args.manifest, must_exist=True)
    g0_run = external_path(args.g0_run, must_exist=True)
    output_dir = external_path(args.output_dir)
    require(not output_dir.exists(), f"choose a fresh ASR pair output directory: {output_dir}")
    manifest = read_json(manifest_path)
    require(manifest.get("artifact_type") == "voiceflow_speaker_filter_pilot_cases", "speaker-filter pilot manifest required")
    cases, profiles = validate_selected_cases(manifest, None)
    preflight_pilot_audio(cases, profiles)
    report_path = g0_run / "pilot-report.json"
    report = read_json(report_path)
    require(report.get("artifact_type") == "voiceflow_speaker_filter_pilot_report", "native G0 pilot report required")
    require(report.get("recipe") == "g0" and report.get("status") == "complete", "completed G0 run required")
    require(report.get("manifest_sha256") == sha256_file(manifest_path, MAX_JSON_BYTES), "G0 report/manifest identity mismatch")
    case_results = [
        read_json(result_path, MAX_REPORT_BYTES)
        for result_path in sorted((g0_run / "case-results").glob("*.json"))
    ]
    require(len(case_results) == report.get("case_count"), "G0 case-result count/report mismatch")
    mkdir_private(output_dir)
    artifacts = create_asr_pair_manifest(output_dir, manifest, case_results, "g0", args.all_cases)
    summary = {
        "artifact_type": "voiceflow_speaker_filter_asr_pair_conversion_report",
        "schema_version": 1,
        "source_run_report_path": str(report_path),
        "source_run_report_sha256": sha256_file(report_path, MAX_JSON_BYTES),
        "source_manifest_path": str(manifest_path),
        "source_manifest_sha256": sha256_file(manifest_path, MAX_JSON_BYTES),
        "artifacts": artifacts,
        "conversion": "VoiceFlow f32 clamp, f32 multiply by 32767, truncate toward zero; no normalization",
        "reference_status": (
            "all pilot cases included; publisher annotations are project-review-pending and are not reviewed gold; "
            "negative silence/noise cases have no publisher text reference"
            if args.all_cases
            else "publisher manual annotations; project review pending; not reviewed gold"
        ),
        "provider_calls": 0,
        "paid_inference": False,
    }
    report_out = output_dir / "asr-pairs-conversion-report.json"
    write_json_once(report_out, summary)
    print(json.dumps({
        "manifest": artifacts["manifest_path"],
        "manifest_sha256": artifacts["manifest_sha256"],
        "provenance": artifacts["provenance_path"],
        "pairs": artifacts["pair_count"],
        "audio_rows": artifacts["audio_row_count"],
        "conversion_report": str(report_out),
    }, ensure_ascii=False))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare", help="prepare only from an already-acquired pinned public corpus")
    prepare.add_argument("--corpus-root", default=str(DEFAULT_CORPUS))
    prepare.add_argument("--candidate-root", default=str(DEFAULT_CANDIDATES))
    prepare.add_argument("--output-dir", required=True)
    prepare.add_argument("--seed", default="speaker-filter-pilot-2026-10-25-v1")
    prepare.add_argument("--max-source-bytes", type=int, default=MAX_CORPUS_BYTES)
    prepare.add_argument("--max-extracted-bytes", type=int, default=256 * MIB)
    prepare.set_defaults(action=prepare_command)
    run = commands.add_parser("run", help="run the native filter on prepared audio and report G0/G1 evidence")
    run.add_argument("--manifest", required=True)
    run.add_argument("--models", default=str(DEFAULT_OUTPUT_ROOT / "models"))
    run.add_argument("--native-bin", required=True)
    run.add_argument("--output-dir", required=True)
    run.add_argument("--recipe", choices=("g0", "g1"), default="g0")
    run.add_argument("--paced", action="store_true")
    run.add_argument("--case-id", action="append", help="run one selected case; can be repeated")
    run.add_argument("--g0-report", help="required evidence input when recipe is g1")
    run.add_argument("--asr-pair-report", help="optional paired local-ASR evidence report when recipe is g1")
    run.set_defaults(action=run_command)
    pairs = commands.add_parser("asr-pairs", help="derive exact production-encoded PCM16 pairs from an existing local G0 run")
    pairs.add_argument("--manifest", required=True)
    pairs.add_argument("--g0-run", required=True)
    pairs.add_argument("--output-dir", required=True)
    pairs.add_argument("--all-cases", action="store_true", help="derive a bounded raw/G0 pair for every completed case, including unlabeled negatives")
    pairs.set_defaults(action=asr_pairs_command)
    args = parser.parse_args()
    if getattr(args, "max_extracted_bytes", 1) <= 0:
        raise PilotError("positive extraction byte budget required")
    return args.action(args)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (PilotError, OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError) as error:
        print(f"speaker-filter-pilot: {error}", file=sys.stderr)
        sys.exit(2)
