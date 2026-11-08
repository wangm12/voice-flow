#!/usr/bin/env python3
"""Strict, bounded evidence and identity validation shared by candidate joins."""
from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import re
import struct
import wave


QWEN_MODEL_ID = "qwen3-asr-0.6b"
QWEN_REVISION = "eae2b51f96265328f1e7beced788adb0e4536f92"
PROBE_EXECUTABLE_SHA256 = "44de9cb78956de1cd0fcf4bee9a7896b700b3e103f48308c925337a85eb7a698"
PROBE_RUNTIME_VERSION = "1.1.0"
ASR_PCM16_CONVERSION_DESC = "VoiceFlow audio.rs::encode: f32 clamp[-1,1], f32 multiply by 32767, truncate toward zero; no normalization"
SAMPLE_RATE = 16_000
# These source-artifact pins define the exact local public-dev cohort being
# joined. The conversion report connects the full ASR pair manifest and its
# per-case source provenance back to the prepared inputs and G0 run.
SOURCE_ARTIFACT_PINS = {
    "prepared_manifest": "9f487a00132addab06ed2ef0166b7564e2390c4996f2a5c67428fed84328a631",
    "historical_asr_manifest": "807b7f50dea377f628946a2cc9e1d1eed409e9c0a30f83515a9a853892cdcf2a",
    "historical_asr_conversion_report": "6b32e679b92ab81ab81916c6e815c8065931c62d4e916fbdd0f32e75e33a0f16",
    "historical_asr_provenance": "57728c4dd592bb4524ca7e23875eea1fbd66fc9e57d995ba25a91816812ff6bb",
    "historical_asr_source_run": "c33b1e47decf9cf620fb72b14eb040b2e80423b684371aa4c70e78dbec4d17b8",
    "frozen_g0_report": "44e93c524849c71a3acbece1899cdc9760e469c98441a9e98473dda20b55244c",
    "frozen_g0_run_header": "d6d0dd57a5e70658bcce7c13a1151c58a064d39cd8edc1e4e53f83cbfbd19606",
}
MAX_JSON_BYTES = 4 * 1024 * 1024
MAX_PROBE_ROWS = 200
MAX_NEW_PROBE_ROWS = 18
MAX_AUDIO_BYTES = 32 * 1024 * 1024
MAX_TOTAL_AUDIO_BYTES = 512 * 1024 * 1024
MAX_TEXT_CHARS = 16_384
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

CASE_ROLES = {
    "other-only-en-spk17-00114": "other_only",
    "other-only-en-spk17-00601": "other_only",
    "other-only-en-spk3-00318": "other_only",
    "other-only-en-spk3-00418": "other_only",
    "other-only-mixed-spk17-01210": "other_only",
    "other-only-mixed-spk3-00383": "other_only",
    "other-only-mixed-spk3-00644": "other_only",
    "other-only-mixed-spk3-00712": "other_only",
    "other-only-zh-spk17-00204": "other_only",
    "other-only-zh-spk17-00828": "other_only",
    "other-only-zh-spk17-01105": "other_only",
    "other-only-zh-spk3-00252": "other_only",
    "target-en-spk20-00840": "target_soft_level",
    "target-mixed-spk20-00649": "target_soft_level",
    "target-zh-spk12-01068": "target_soft_level",
    "target-en-spk7-00886": "target_mixture",
    "target-mixed-spk12-00462": "target_mixture",
    "target-zh-spk20-00018": "target_mixture",
}
CASE_LANGUAGES = {
    "other-only-en-spk17-00114": "en",
    "other-only-en-spk17-00601": "en",
    "other-only-en-spk3-00318": "en",
    "other-only-en-spk3-00418": "en",
    "other-only-mixed-spk17-01210": "mixed",
    "other-only-mixed-spk3-00383": "mixed",
    "other-only-mixed-spk3-00644": "mixed",
    "other-only-mixed-spk3-00712": "mixed",
    "other-only-zh-spk17-00204": "zh",
    "other-only-zh-spk17-00828": "zh",
    "other-only-zh-spk17-01105": "zh",
    "other-only-zh-spk3-00252": "zh",
    "target-en-spk20-00840": "en",
    "target-mixed-spk20-00649": "mixed",
    "target-zh-spk12-01068": "zh",
    "target-en-spk7-00886": "en",
    "target-mixed-spk12-00462": "mixed",
    "target-zh-spk20-00018": "zh",
}
PROFILE_SPEAKERS = {f"public-ascend-speaker-{n}": n for n in (7, 12, 20)}


class JoinContractError(ValueError):
    """An identity, model, or bounded-schema mismatch invalidates the join."""


@dataclass(frozen=True)
class AudioIdentity:
    path: Path
    sha256: str
    samples: int
    sample_rate_hz: int
    byte_size: int


@dataclass(frozen=True)
class ASREvidence:
    status: str
    text: str | None
    error: str | None
    row_id: str | None
    request_id: str | None = None


@dataclass
class ProbeIndex:
    path: Path
    digest: str
    report: dict
    by_case_id: dict[str, dict]
    by_audio_hash: dict[str, list[dict]]
    request_ids: set[str]


def is_sha256(value: object) -> bool:
    return isinstance(value, str) and SHA256_RE.fullmatch(value) is not None


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_json_bounded(path: Path, *, max_bytes: int = MAX_JSON_BYTES) -> tuple[dict, Path, str]:
    if path.is_symlink():
        raise JoinContractError(f"refusing symlinked JSON input: {path}")
    resolved = path.resolve(strict=True)
    if not resolved.is_file():
        raise JoinContractError(f"JSON input is not a regular file: {resolved}")
    size = resolved.stat().st_size
    if size < 2 or size > max_bytes:
        raise JoinContractError(f"JSON input size outside bound 2..{max_bytes} bytes: {resolved} ({size})")
    raw = resolved.read_bytes()
    try:
        data = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise JoinContractError(f"invalid JSON input {resolved}: {exc}") from exc
    if not isinstance(data, dict):
        raise JoinContractError(f"expected JSON object: {resolved}")
    return data, resolved, hashlib.sha256(raw).hexdigest()


def audio_identity(path_value: object, *, expected_hash: object | None = None) -> AudioIdentity:
    if not isinstance(path_value, str) or not path_value or len(path_value) > 4096:
        raise JoinContractError("audio path must be a nonempty bounded string")
    path = Path(path_value)
    if path.is_symlink():
        raise JoinContractError(f"refusing symlinked audio identity: {path}")
    try:
        resolved = path.resolve(strict=True)
    except OSError as exc:
        raise JoinContractError(f"audio identity path is unavailable: {path}: {exc}") from exc
    if not resolved.is_file():
        raise JoinContractError(f"audio identity is not a regular file: {resolved}")
    byte_size = resolved.stat().st_size
    if byte_size < 44 or byte_size > MAX_AUDIO_BYTES:
        raise JoinContractError(f"audio identity size outside bound 44..{MAX_AUDIO_BYTES}: {resolved}")
    try:
        with wave.open(str(resolved), "rb") as audio:
            params = (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype())
            samples = audio.getnframes()
            if params != (1, 2, SAMPLE_RATE, "NONE") or samples <= 0:
                raise JoinContractError(f"expected mono 16 kHz PCM16 WAV, got {params} / {samples}: {resolved}")
            decoded = audio.readframes(samples)
            if len(decoded) != samples * params[0] * params[1]:
                raise JoinContractError(f"truncated PCM16 WAV data: {resolved}")
    except (wave.Error, EOFError) as exc:
        raise JoinContractError(f"invalid WAV identity: {resolved}: {exc}") from exc
    digest = sha256_file(resolved)
    if not is_sha256(expected_hash):
        raise JoinContractError(f"missing or malformed expected SHA-256 for {resolved}")
    if digest != expected_hash:
        raise JoinContractError(f"audio SHA-256 mismatch for {resolved}: expected {expected_hash}, got {digest}")
    return AudioIdentity(resolved, digest, samples, SAMPLE_RATE, byte_size)


def _require_short_string(value: object, label: str, *, max_chars: int = 4096) -> str:
    if not isinstance(value, str) or not value or len(value) > max_chars:
        raise JoinContractError(f"{label} must be a nonempty string of at most {max_chars} characters")
    return value


def _semantic_response(row: dict) -> str:
    response = row.get("response")
    if not isinstance(response, dict):
        return "<missing-response>"
    result = response.get("result")
    return json.dumps({
        "status": response.get("status"),
        "error": response.get("error"),
        "loaded_model_id": response.get("loaded_model_id"),
        "loaded_model_version": response.get("loaded_model_version"),
        "model_version": response.get("model_version"),
        "op": response.get("op"),
        "cleanup_result": response.get("cleanup_result"),
        "protocol_version": response.get("protocol_version"),
        "runtime_version": response.get("runtime_version"),
        "result": result,
    }, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def _validate_row_provider_identity(response: object, row_id: str) -> None:
    if not isinstance(response, dict):
        return
    for field, expected in (
        ("loaded_model_id", QWEN_MODEL_ID),
        ("loaded_model_version", QWEN_REVISION),
        ("model_version", QWEN_REVISION),
    ):
        actual = response.get(field)
        if actual is not None and actual != expected:
            raise JoinContractError(f"ASR row {row_id} {field} mismatch: {actual!r}")
    op = response.get("op")
    if op is not None and op != "transcribe":
        raise JoinContractError(f"ASR row {row_id} operation mismatch: {op!r}")
    protocol = response.get("protocol_version")
    if protocol is not None and (isinstance(protocol, bool) or protocol != 1):
        raise JoinContractError(f"ASR row {row_id} protocol version mismatch: {protocol!r}")
    runtime = response.get("runtime_version")
    if runtime is not None and runtime != PROBE_RUNTIME_VERSION:
        raise JoinContractError(f"ASR row {row_id} runtime version mismatch: {runtime!r}")
    if response.get("cleanup_result") is not None:
        raise JoinContractError(f"ASR row {row_id} includes an unexpected cleanup result")


def load_probe(path: Path, *, max_rows: int) -> ProbeIndex:
    report, resolved, report_digest = read_json_bounded(path)
    if report.get("artifact_type") != "local_speaker_filter_native_asr_probe" or report.get("schema_version") != 1:
        raise JoinContractError(f"unsupported ASR probe schema: {resolved}")
    if len(report) > 24:
        raise JoinContractError(f"ASR probe object exceeds the field bound: {resolved}")
    if report.get("model_id") != QWEN_MODEL_ID or report.get("revision") != QWEN_REVISION:
        raise JoinContractError(f"ASR probe model/revision mismatch: {resolved}")
    if report.get("executable_sha256") != PROBE_EXECUTABLE_SHA256:
        raise JoinContractError(f"ASR probe executable pin mismatch: {resolved}")
    if not is_sha256(report.get("manifest_sha256")):
        raise JoinContractError(f"ASR probe manifest digest is malformed: {resolved}")
    if isinstance(report.get("cloud_calls"), bool) or report.get("cloud_calls") != 0:
        raise JoinContractError(f"ASR probe is not verified local-only: {resolved}")
    if report.get("microphone_used") is not False:
        raise JoinContractError(f"ASR probe microphone flag must be false: {resolved}")
    rows = report.get("rows")
    if not isinstance(rows, list) or len(rows) > max_rows:
        raise JoinContractError(f"ASR probe row list exceeds bound {max_rows}: {resolved}")

    by_case_id: dict[str, dict] = {}
    by_audio_hash: dict[str, list[dict]] = {}
    request_ids: set[str] = set()
    total_audio_bytes = 0
    for index, row in enumerate(rows):
        if not isinstance(row, dict) or len(row) > 16:
            raise JoinContractError(f"malformed or oversized ASR row at index {index}: {resolved}")
        case = row.get("case")
        if not isinstance(case, dict) or len(case) > 16:
            raise JoinContractError(f"malformed ASR case identity at index {index}: {resolved}")
        case_id = _require_short_string(case.get("id"), f"ASR row {index} case id", max_chars=256)
        if case_id in by_case_id:
            raise JoinContractError(f"duplicate ASR case identity: {case_id}")
        audio_hash = row.get("audio_sha256")
        nested_hash = case.get("audio_sha256")
        if not is_sha256(audio_hash) or audio_hash != nested_hash:
            raise JoinContractError(f"ASR row {case_id} has malformed or divergent audio digests")
        language = _require_short_string(case.get("language"), f"ASR row {case_id} language", max_chars=32)
        if language not in {"en", "zh", "mixed", "other"}:
            raise JoinContractError(f"ASR row {case_id} language identity is unsupported: {language!r}")
        _require_short_string(case.get("condition"), f"ASR row {case_id} condition", max_chars=128)
        _require_short_string(case.get("role"), f"ASR row {case_id} role", max_chars=64)
        audio = audio_identity(case.get("audio_path"), expected_hash=audio_hash)
        total_audio_bytes += audio.byte_size
        if total_audio_bytes > MAX_TOTAL_AUDIO_BYTES:
            raise JoinContractError(f"ASR probe audio bytes exceed bound {MAX_TOTAL_AUDIO_BYTES}: {resolved}")
        duration = row.get("audio_seconds")
        if isinstance(duration, bool) or not isinstance(duration, (int, float)) or not math.isfinite(duration):
            raise JoinContractError(f"ASR row {case_id} has invalid duration")
        if duration <= 0 or abs(float(duration) - audio.samples / SAMPLE_RATE) > 1 / SAMPLE_RATE:
            raise JoinContractError(f"ASR row {case_id} duration disagrees with its WAV identity")
        response = row.get("response")
        if isinstance(response, dict) and len(response) > 32:
            raise JoinContractError(f"ASR response exceeds the field bound for {case_id}")
        if isinstance(response, dict) and isinstance(response.get("result"), dict) and len(response["result"]) > 32:
            raise JoinContractError(f"ASR result exceeds the field bound for {case_id}")
        _validate_row_provider_identity(response, case_id)
        if isinstance(response, dict):
            request_id = response.get("request_id")
            if request_id is not None:
                request_id = _require_short_string(request_id, f"ASR row {case_id} request id", max_chars=256)
                if request_id in request_ids:
                    raise JoinContractError(f"duplicate ASR request identity: {request_id}")
                request_ids.add(request_id)
        by_case_id[case_id] = row
        by_audio_hash.setdefault(audio_hash, []).append(row)

    for audio_hash, duplicates in by_audio_hash.items():
        if len(duplicates) < 2:
            continue
        first_case = duplicates[0]["case"]
        first_response = duplicates[0].get("response")
        first_language = first_case["language"]
        first_semantics = _semantic_response(duplicates[0])
        for duplicate in duplicates[1:]:
            if duplicate["case"]["language"] != first_language:
                raise JoinContractError(f"same audio hash has different effective language parameters: {audio_hash}")
            if _semantic_response(duplicate) != first_semantics:
                raise JoinContractError(f"conflicting ASR results for identical audio/model/request identity: {audio_hash}")
        # Repeated exact audio is expected in the frozen historical manifest.
        # Case IDs remain unique and only exact semantic responses may be reused.
        _ = first_response

    return ProbeIndex(resolved, report_digest, report, by_case_id, by_audio_hash, request_ids)


def validate_global_request_identity(historical: ProbeIndex, candidate: ProbeIndex) -> None:
    overlap = historical.request_ids & candidate.request_ids
    if overlap:
        raise JoinContractError(f"historical and candidate ASR reports reuse request identities: {sorted(overlap)}")


def classify_asr(row: dict | None, *, expected_samples: int, expected_case_id: str) -> ASREvidence:
    if row is None:
        return ASREvidence("missing", None, "missing_asr_row", None)
    case = row.get("case")
    if not isinstance(case, dict) or case.get("id") != expected_case_id:
        return ASREvidence("invalid", None, "asr_case_identity_mismatch", None)
    response = row.get("response")
    if not isinstance(response, dict):
        return ASREvidence("unknown", None, "missing_or_malformed_response", expected_case_id)
    request_id = response.get("request_id")
    if request_id is not None and (not isinstance(request_id, str) or not request_id or len(request_id) > 256):
        return ASREvidence("unknown", None, "asr_request_identity_invalid", expected_case_id)
    row_state = row.get("state")
    if row_state in {"pending", "queued", "running", "accepted", "in_progress"}:
        return ASREvidence("unexecuted", None, "asr_row_not_completed", expected_case_id, request_id)
    if row_state is not None and row_state not in {"completed", "complete"}:
        return ASREvidence("unknown", None, "asr_row_state_unknown", expected_case_id, request_id)
    status = response.get("status")
    if status in {"pending", "queued", "running", "accepted", "in_progress"}:
        return ASREvidence("unexecuted", None, "asr_not_completed", expected_case_id, request_id)
    if status in {"error", "failed", "cancelled", "rejected"} or response.get("error") is not None:
        return ASREvidence("failed", None, "asr_failed", expected_case_id, request_id)
    if status != "completed":
        return ASREvidence("unknown", None, "asr_completion_status_unknown", expected_case_id, request_id)
    if request_id is None:
        return ASREvidence("unknown", None, "asr_request_identity_missing", expected_case_id)
    if any(response.get(field) != expected for field, expected in (
        ("loaded_model_id", QWEN_MODEL_ID),
        ("loaded_model_version", QWEN_REVISION),
        ("model_version", QWEN_REVISION),
        ("op", "transcribe"),
        ("protocol_version", 1),
        ("runtime_version", PROBE_RUNTIME_VERSION),
    )):
        return ASREvidence("unknown", None, "asr_model_or_operation_attestation_missing", expected_case_id, request_id)
    if response.get("cleanup_result") is not None:
        return ASREvidence("invalid", None, "unexpected_cleanup_result", expected_case_id, request_id)
    result = response.get("result")
    if not isinstance(result, dict):
        return ASREvidence("invalid", None, "asr_result_missing_or_malformed", expected_case_id, request_id)
    completed = result.get("chunks_completed")
    total = result.get("chunks_total")
    covered = result.get("covered_samples")
    if (result.get("completion") != "complete"
            or isinstance(completed, bool) or not isinstance(completed, int) or completed <= 0
            or isinstance(total, bool) or not isinstance(total, int) or total <= 0
            or completed != total
            or isinstance(covered, bool) or not isinstance(covered, int) or covered != expected_samples):
        return ASREvidence("incomplete", None, "asr_not_full_audio_coverage", expected_case_id, request_id)
    text = result.get("text")
    if not isinstance(text, str) or len(text) > MAX_TEXT_CHARS:
        return ASREvidence("invalid", None, "asr_text_not_bounded_string", expected_case_id, request_id)
    return ASREvidence("completed", text, None, expected_case_id, request_id)


def baseline_identity(index: ProbeIndex, *, case_id: str, stage: str, expected_hash: str,
                      role: str, language: str, expected_samples: int) -> tuple[dict | None, ASREvidence]:
    expected_id = f"{case_id}-{stage}"
    row = index.by_case_id.get(expected_id)
    if row is None:
        return None, classify_asr(None, expected_samples=expected_samples, expected_case_id=expected_id)
    case = row["case"]
    expected_condition_prefix = "raw" if stage == "raw" else "g0"
    expected_condition_base = {
        "other_only": "other_only",
        "target_soft_level": "soft",
        "target_mixture": "short",
    }[role]
    expected_condition = f"{expected_condition_prefix}-{expected_condition_base}"
    if (row["audio_sha256"] != expected_hash
            or case.get("role") != role
            or case.get("language") != language
            or case.get("condition") != expected_condition):
        raise JoinContractError(f"{stage} baseline identity mismatch for canonical case {case_id}")
    evidence = classify_asr(row, expected_samples=expected_samples, expected_case_id=expected_id)
    return row, evidence


def _resolved_file_hash(path_value: object, label: str, *, max_bytes: int = MAX_AUDIO_BYTES) -> tuple[Path, str]:
    if not isinstance(path_value, str) or not path_value or len(path_value) > 4096:
        raise JoinContractError(f"{label} path must be a nonempty bounded string")
    path = Path(path_value)
    if path.is_symlink():
        raise JoinContractError(f"refusing symlinked {label}: {path}")
    resolved = path.resolve(strict=True)
    if not resolved.is_file() or resolved.stat().st_size > max_bytes:
        raise JoinContractError(f"{label} is not a bounded regular file: {resolved}")
    return resolved, sha256_file(resolved)


def _float32(value: float) -> float:
    return struct.unpack("<f", struct.pack("<f", value))[0]


def _pcm16_payload(path: Path) -> tuple[bytes, int]:
    try:
        with wave.open(str(path), "rb") as audio:
            if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getcomptype()) != (1, 2, SAMPLE_RATE, "NONE"):
                raise JoinContractError(f"expected mono PCM16 source WAV: {path}")
            samples = audio.getnframes()
            payload = audio.readframes(samples)
    except (wave.Error, EOFError) as exc:
        raise JoinContractError(f"invalid PCM16 source WAV {path}: {exc}") from exc
    if len(payload) != samples * 2:
        raise JoinContractError(f"truncated PCM16 source WAV: {path}")
    return payload, samples


def _float32_wav_payload(path: Path) -> tuple[bytes, int]:
    raw = path.read_bytes()
    if len(raw) > MAX_AUDIO_BYTES or len(raw) < 44 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise JoinContractError(f"invalid or oversized float32 WAV container: {path}")
    riff_end = 8 + int.from_bytes(raw[4:8], "little")
    if riff_end != len(raw):
        raise JoinContractError(f"float32 WAV RIFF length does not match file bytes: {path}")
    position = 12
    fmt = None
    data = None
    while position + 8 <= riff_end:
        chunk_id = raw[position:position + 4]
        chunk_size = int.from_bytes(raw[position + 4:position + 8], "little")
        start = position + 8
        end = start + chunk_size
        if end > riff_end:
            raise JoinContractError(f"truncated float32 WAV chunk: {path}")
        chunk = raw[start:end]
        if chunk_id == b"fmt ":
            fmt = chunk
        elif chunk_id == b"data":
            if data is not None:
                raise JoinContractError(f"multiple float32 WAV data chunks are unsupported: {path}")
            data = chunk
        position = end + (chunk_size & 1)
    if position != riff_end or fmt is None or data is None or len(fmt) < 16:
        raise JoinContractError(f"incomplete float32 WAV format/data chunks: {path}")
    format_tag, channels, sample_rate, _byte_rate, block_align, bits = struct.unpack_from("<HHIIHH", fmt)
    if format_tag == 0xFFFE:
        if len(fmt) < 40 or int.from_bytes(fmt[24:28], "little") != 3:
            raise JoinContractError(f"WAV extensible subtype is not IEEE float32: {path}")
        if fmt[28:40] != bytes.fromhex("00001000800000aa00389b71"):
            raise JoinContractError(f"WAV extensible float GUID is invalid: {path}")
    elif format_tag != 3:
        raise JoinContractError(f"WAV format is not IEEE float32: {path}")
    if (channels, sample_rate, block_align, bits) != (1, SAMPLE_RATE, 4, 32) or len(data) % 4:
        raise JoinContractError(f"float32 WAV must be mono 16 kHz IEEE float32: {path}")
    return data, len(data) // 4


def _encode_pcm16_payload(source_path: Path, *, source_kind: str) -> tuple[bytes, int]:
    if source_kind == "pcm16":
        payload, samples = _pcm16_payload(source_path)
        source_values = struct.iter_unpack("<h", payload)
        converted = bytearray()
        for (sample,) in source_values:
            f = _float32(sample / 32768.0)
            value = int(_float32(f * 32767.0))
            converted.extend(struct.pack("<h", max(-32768, min(32767, value))))
        return bytes(converted), samples
    if source_kind == "float32":
        payload, samples = _float32_wav_payload(source_path)
        converted = bytearray()
        for (sample,) in struct.iter_unpack("<f", payload):
            if not math.isfinite(sample):
                raise JoinContractError(f"nonfinite float sample in source WAV: {source_path}")
            clipped = max(-1.0, min(1.0, sample))
            value = int(_float32(clipped * 32767.0))
            converted.extend(struct.pack("<h", max(-32768, min(32767, value))))
        return bytes(converted), samples
    raise JoinContractError(f"unsupported source PCM conversion: {source_kind}")


def load_prepared_source_context(pilot: dict, pilot_path: Path) -> dict:
    input_artifacts = pilot.get("input_artifacts")
    if not isinstance(input_artifacts, dict):
        raise JoinContractError("pilot input artifact binding is missing")
    prep_path_value = input_artifacts.get("prepared_manifest_path")
    prep_sha = input_artifacts.get("prepared_manifest_sha256")
    if not isinstance(prep_path_value, str) or not is_sha256(prep_sha):
        raise JoinContractError("pilot prepared-manifest digest is malformed")
    prep, prepared_path, actual_prep_sha = read_json_bounded(Path(prep_path_value))
    if actual_prep_sha != prep_sha or actual_prep_sha != SOURCE_ARTIFACT_PINS["prepared_manifest"]:
        raise JoinContractError("pilot prepared-manifest digest does not match the pinned source file")

    profile_rows = prep.get("profiles")
    if not isinstance(profile_rows, list) or len(profile_rows) != 3:
        raise JoinContractError("prepared manifest must bind exactly three public enrollment profiles")
    prepared_profiles: dict[str, dict] = {}
    for profile in profile_rows:
        if not isinstance(profile, dict):
            raise JoinContractError("malformed prepared enrollment profile")
        profile_id = _require_short_string(profile.get("profile_id"), "profile id", max_chars=128)
        if profile_id in prepared_profiles or profile_id not in PROFILE_SPEAKERS:
            raise JoinContractError(f"duplicate or unexpected public profile identity: {profile_id}")
        speaker_id = PROFILE_SPEAKERS[profile_id]
        if profile.get("source_speaker_id") != speaker_id:
            raise JoinContractError(f"profile source-speaker identity mismatch: {profile_id}")
        profile_audio = audio_identity(profile.get("input_audio_path"), expected_hash=profile.get("input_audio_sha256"))
        if profile.get("input_samples") != profile_audio.samples:
            raise JoinContractError(f"profile sample-count mismatch: {profile_id}")
        prepared_profiles[profile_id] = {"speaker_id": speaker_id, "audio": profile_audio, "row": profile}
    if set(prepared_profiles) != set(PROFILE_SPEAKERS):
        raise JoinContractError("prepared enrollment profile identity set mismatch")

    prepared_rows = prep.get("cases")
    if not isinstance(prepared_rows, list) or len(prepared_rows) < len(CASE_ROLES) or len(prepared_rows) > MAX_PROBE_ROWS:
        raise JoinContractError("prepared case inventory is outside the bounded cohort size")
    prepared_cases: dict[str, dict] = {}
    for row in prepared_rows:
        if not isinstance(row, dict) or len(row) > 96:
            raise JoinContractError("malformed or oversized prepared case identity")
        case_id = _require_short_string(row.get("case_id"), "prepared case id", max_chars=256)
        if case_id in prepared_cases:
            raise JoinContractError(f"duplicate case identity in prepared manifest: {case_id}")
        prepared_cases[case_id] = row
    if not set(CASE_ROLES).issubset(prepared_cases):
        raise JoinContractError("prepared manifest is missing one or more canonical counterexample cases")

    frozen_dir_value = input_artifacts.get("frozen_case_dir")
    if not isinstance(frozen_dir_value, str) or not frozen_dir_value or len(frozen_dir_value) > 4096:
        raise JoinContractError("frozen G0 case directory binding is missing")
    frozen_dir = Path(frozen_dir_value)
    if frozen_dir.is_symlink():
        raise JoinContractError(f"refusing symlinked frozen G0 directory: {frozen_dir}")
    frozen_dir = frozen_dir.resolve(strict=True)
    frozen_report, frozen_report_path, frozen_report_sha = read_json_bounded(frozen_dir / "pilot-report.json")
    run_header, run_header_path, run_header_sha = read_json_bounded(frozen_dir / "run-header.json")
    if (frozen_report_sha != SOURCE_ARTIFACT_PINS["frozen_g0_report"]
            or run_header_sha != SOURCE_ARTIFACT_PINS["frozen_g0_run_header"]):
        raise JoinContractError("frozen G0 report/run-header differs from the pinned source artifacts")
    frozen_manifest_path = Path(_require_short_string(frozen_report.get("manifest_path"), "frozen G0 manifest path"))
    header_manifest_path = Path(_require_short_string(run_header.get("manifest_path"), "frozen run-header manifest path"))
    if (frozen_report.get("artifact_type") != "voiceflow_speaker_filter_pilot_report"
            or frozen_report.get("recipe") != "g0"
            or frozen_report.get("status") != "complete"
            or frozen_report.get("case_count") != 18
            or frozen_report.get("completed_cases") != 18
            or frozen_report.get("manifest_sha256") != actual_prep_sha
            or frozen_manifest_path.resolve() != prepared_path
            or header_manifest_path.resolve() != prepared_path
            or run_header.get("recipe") != "g0"
            or run_header.get("manifest_sha256") != actual_prep_sha):
        raise JoinContractError("frozen G0 run is not bound to the pinned prepared manifest")
    header_case_ids = run_header.get("case_ids")
    if not isinstance(header_case_ids, list) or len(header_case_ids) != 18 or set(header_case_ids) != set(CASE_ROLES):
        raise JoinContractError("frozen G0 run-header case inventory is not the canonical 18")

    profile_results = frozen_report.get("profile_results")
    if not isinstance(profile_results, dict) or set(profile_results) != set(PROFILE_SPEAKERS):
        raise JoinContractError("frozen G0 profile inventory is not the expected three profiles")
    for profile_id, profile in prepared_profiles.items():
        result = profile_results.get(profile_id)
        if not isinstance(result, dict) or result.get("status") != "complete":
            raise JoinContractError(f"frozen G0 profile result is missing or incomplete: {profile_id}")
        profile_path, profile_sha = _resolved_file_hash(result.get("profile_path"), f"frozen profile {profile_id}")
        if profile_sha != result.get("profile_sha256"):
            raise JoinContractError(f"frozen G0 profile file digest mismatch: {profile_id}")
        profile_doc, _resolved_profile_path, _ = read_json_bounded(profile_path)
        source = profile_doc.get("source")
        if (not isinstance(source, dict)
                or source.get("source_sha256") != profile["audio"].sha256
                or source.get("sample_rate_hz") != SAMPLE_RATE
                or source.get("channels") != 1
                or source.get("sample_format") != "pcm16"):
            raise JoinContractError(f"frozen G0 profile enrollment source mismatch: {profile_id}")
        profile["frozen_profile_path"] = profile_path
        profile["frozen_profile_sha256"] = profile_sha

    frozen_cases: dict[str, dict] = {}
    for case_id in CASE_ROLES:
        result_path = frozen_dir / "case-results" / f"{case_id}.json"
        result, resolved_result_path, _ = read_json_bounded(result_path)
        if result.get("case_id") != case_id or result.get("status") != "complete" or result.get("recipe") != "g0":
            raise JoinContractError(f"frozen G0 case result is missing or invalid: {case_id}")
        frozen_cases[case_id] = result

    return {
        "prepared": prep,
        "prepared_path": prepared_path,
        "prepared_sha256": actual_prep_sha,
        "prepared_profiles": prepared_profiles,
        "prepared_cases": prepared_cases,
        "frozen_dir": frozen_dir,
        "frozen_report": frozen_report,
        "frozen_report_path": frozen_report_path,
        "frozen_report_sha256": frozen_report_sha,
        "run_header": run_header,
        "run_header_path": run_header_path,
        "run_header_sha256": run_header_sha,
        "frozen_cases": frozen_cases,
    }


def validate_case_manifest(pilot: dict, kind: str, pilot_path: Path) -> tuple[list[dict], dict[str, dict], dict, dict]:
    expected_artifact = {
        "nomo": "voiceflow_nomo_pvad_whole_utterance_pilot",
        "real_tse": "voiceflow_real_tse_causal_private_pilot",
    }[kind]
    if pilot.get("artifact_type") != expected_artifact or pilot.get("schema_version") != 1:
        raise JoinContractError(f"unsupported {kind} pilot schema: {pilot_path}")
    if len(pilot) > 32:
        raise JoinContractError(f"pilot object exceeds the field bound: {pilot_path}")
    if pilot.get("status") != "complete":
        raise JoinContractError(f"pilot status is not complete: {pilot_path}")
    cases = pilot.get("cases")
    if not isinstance(cases, list) or len(cases) != len(CASE_ROLES):
        raise JoinContractError(f"pilot must contain exactly {len(CASE_ROLES)} canonical cases")
    by_id: dict[str, dict] = {}
    for item in cases:
        if not isinstance(item, dict) or len(item) > 64:
            raise JoinContractError("malformed or oversized pilot case")
        case_id = _require_short_string(item.get("case_id"), "pilot case id", max_chars=256)
        if case_id in by_id:
            raise JoinContractError(f"duplicate canonical pilot case identity: {case_id}")
        by_id[case_id] = item
    if set(by_id) != set(CASE_ROLES):
        missing = sorted(set(CASE_ROLES) - set(by_id))
        extra = sorted(set(by_id) - set(CASE_ROLES))
        raise JoinContractError(f"pilot global case identity mismatch; missing={missing}; extra={extra}")

    source_context = load_prepared_source_context(pilot, pilot_path)
    prepared_path = source_context["prepared_path"]
    prep_sha = source_context["prepared_sha256"]
    prepared_profiles = source_context["prepared_profiles"]
    prepared_cases = source_context["prepared_cases"]
    frozen_cases = source_context["frozen_cases"]

    profile_case_hashes: dict[str, tuple[str, str]] = {}
    for case_id, item in by_id.items():
        expected_role = CASE_ROLES[case_id]
        expected_language = CASE_LANGUAGES[case_id]
        if item.get("role") != expected_role or item.get("language") != expected_language:
            raise JoinContractError(f"canonical role/language mismatch for {case_id}")
        expected_soft = expected_role == "target_soft_level"
        if item.get("soft_level_challenge") is not expected_soft:
            raise JoinContractError(f"canonical soft-level marker mismatch for {case_id}")
        prepared_case = prepared_cases.get(case_id)
        if not isinstance(prepared_case, dict):
            raise JoinContractError(f"canonical prepared source case is missing: {case_id}")
        frozen_case = frozen_cases[case_id]
        if (prepared_case.get("role") != expected_role
                or prepared_case.get("language") != expected_language
                or prepared_case.get("soft_level_challenge", False) is not expected_soft
                or frozen_case.get("role") != expected_role
                or frozen_case.get("language") != expected_language
                or frozen_case.get("soft_level_challenge") is not expected_soft):
            raise JoinContractError(f"prepared/frozen source role identity mismatch for {case_id}")
        profile_id = item.get("profile_id")
        speaker_id = PROFILE_SPEAKERS.get(profile_id)
        if speaker_id is None or item.get("profile_source_speaker_id") != speaker_id:
            raise JoinContractError(f"profile-to-speaker binding mismatch for {case_id}")
        if (prepared_case.get("profile_id") != profile_id
                or prepared_case.get("profile_source_speaker_id") != speaker_id
                or frozen_case.get("profile_id") != profile_id):
            raise JoinContractError(f"canonical prepared/frozen profile binding mismatch for {case_id}")
        profile = prepared_profiles[profile_id]
        recipe = prepared_case.get("mix_recipe")
        if not isinstance(recipe, dict):
            raise JoinContractError(f"prepared source audio recipe is missing for {case_id}")
        prepared_audio = audio_identity(recipe.get("input_audio_path"), expected_hash=recipe.get("input_audio_sha256"))
        source_speaker_match = re.search(r"spk(\d+)", case_id)
        if source_speaker_match is None:
            raise JoinContractError(f"canonical case has no source-speaker identity: {case_id}")
        case_speaker_id = int(source_speaker_match.group(1))
        if expected_role == "other_only":
            component = recipe.get("interferer_component")
            if (not isinstance(component, dict)
                    or component.get("source_speaker_id") != case_speaker_id
                    or component.get("target_component") is not None):
                raise JoinContractError(f"prepared other-speaker source identity mismatch for {case_id}")
            source_suffix = case_id.rsplit("-", 1)[1]
            _interferer_path, interferer_sha = _resolved_file_hash(prepared_case.get("source_path"), f"prepared interferer {case_id}")
            if (interferer_sha != prepared_case.get("source_audio_sha256")
                    or component.get("source_case_id") != prepared_case.get("source_case_id")
                    or prepared_case.get("source_case_id") != f"ascend-test-{source_suffix}"
                    or component.get("source_speaker_id") != prepared_case.get("source_speaker_id")
                    or component.get("source_parent_block_id") != prepared_case.get("source_parent_block_id")
                    or prepared_case.get("source_speaker_id") != case_speaker_id):
                raise JoinContractError(f"prepared interferer audio digest mismatch for {case_id}")
            if recipe.get("target_component") is not None:
                raise JoinContractError(f"other-only source unexpectedly has a target component: {case_id}")
        else:
            target_path_value = prepared_case.get("target_source_audio_path")
            target_sha = prepared_case.get("target_source_audio_sha256")
            recipe_target_path = recipe.get("target_source_path")
            recipe_target_sha = recipe.get("target_source_audio_sha256")
            source_suffix = case_id.rsplit("-", 1)[1]
            _target_path, target_file_sha = _resolved_file_hash(target_path_value, f"prepared target enrollment source {case_id}")
            if (target_file_sha != target_sha
                    or target_sha != recipe_target_sha
                    or not isinstance(recipe_target_path, str)
                    or Path(recipe_target_path).resolve() != _target_path
                    or prepared_case.get("target_source_case_id") != recipe.get("target_source_case_id")
                    or prepared_case.get("target_source_case_id") != f"ascend-validation-{source_suffix}"
                    or prepared_case.get("target_speaker_id") != case_speaker_id
                    or prepared_case.get("target_speaker_id") != speaker_id
                    or recipe.get("target_speaker_id") != speaker_id):
                raise JoinContractError(f"prepared target-source identity mismatch for {case_id}")
        frozen_input = audio_identity(frozen_case.get("input_path"), expected_hash=frozen_case.get("input_sha256"))
        if (prepared_audio.path != frozen_input.path
                or prepared_audio.sha256 != frozen_input.sha256
                or prepared_audio.samples != frozen_input.samples
                or frozen_case.get("input_samples") != frozen_input.samples):
            raise JoinContractError(f"prepared source input does not match frozen G0 case audio for {case_id}")
        for prepared_field, frozen_field in (
            ("target_source_case_id", "target_source_case_id"),
            ("target_speaker_id", "target_source_speaker_id"),
            ("target_session_id", "target_source_session_id"),
            ("target_parent_block_id", "target_source_parent_block_id"),
            ("target_source_audio_sha256", "target_source_audio_sha256"),
            ("publisher_reference_sha256", "publisher_reference_sha256"),
            ("asr_pair_selection", "asr_pair_selection"),
        ):
            if prepared_case.get(prepared_field) != frozen_case.get(frozen_field):
                raise JoinContractError(f"prepared/frozen source field {prepared_field} differs for {case_id}")
        g0_profile = source_context["frozen_report"]["profile_results"][profile_id]
        frozen_profile_path = Path(frozen_case.get("profile_path", ""))
        if (frozen_profile_path.is_symlink()
                or frozen_profile_path.resolve() != profile["frozen_profile_path"]
                or frozen_case.get("profile_sha256") != profile["frozen_profile_sha256"]
                or g0_profile.get("profile_sha256") != profile["frozen_profile_sha256"]):
            raise JoinContractError(f"frozen G0 profile artifact identity mismatch for {case_id}")
        g0_output_path, g0_output_sha = _resolved_file_hash(frozen_case.get("output_path"), f"frozen G0 output {case_id}")
        native_report = frozen_case.get("native_report")
        if (g0_output_sha != frozen_case.get("output_sha256")
                or frozen_case.get("output_samples") != frozen_input.samples
                or not isinstance(native_report, dict)
                or native_report.get("status") != "ok"
                or native_report.get("sample_rate_hz") != SAMPLE_RATE
                or native_report.get("channels") != 1
                or native_report.get("input_samples") != frozen_input.samples
                or native_report.get("output_samples") != frozen_input.samples):
            raise JoinContractError(f"frozen G0 source output identity/timeline mismatch for {case_id}")
        prepared_case["_join_prepared_audio"] = prepared_audio
        prepared_case["_join_frozen_g0_output_path"] = g0_output_path
        prepared_case["_join_frozen_g0_output_sha256"] = g0_output_sha
        item["_join_prepared_case"] = prepared_case
        item["_join_frozen_case"] = frozen_case
        if kind == "nomo":
            pilot_profiles = pilot.get("profiles")
            if not isinstance(pilot_profiles, dict) or not isinstance(pilot_profiles.get(profile_id), dict):
                raise JoinContractError(f"Nomo pilot profile binding is missing for {profile_id}")
            expected_enroll_sha = pilot_profiles[profile_id].get("enrollment_source_sha256")
            if (expected_enroll_sha != profile["audio"].sha256
                    or pilot_profiles[profile_id].get("source_speaker_id") != speaker_id):
                raise JoinContractError(f"Nomo enrollment digest binding mismatch for {case_id}")
        else:
            enrollment_audio = audio_identity(item.get("enrollment_path"), expected_hash=item.get("enrollment_sha256"))
            if (enrollment_audio.sha256 != profile["audio"].sha256
                    or enrollment_audio.path != profile["audio"].path):
                raise JoinContractError(f"REAL-TSE enrollment file binding mismatch for {case_id}")
        enrollment_pair = (profile_id, profile["audio"].sha256)
        previous = profile_case_hashes.setdefault(profile_id, enrollment_pair)
        if previous != enrollment_pair:
            raise JoinContractError(f"profile audio identity changes across cases: {profile_id}")

        if kind == "nomo":
            input_audio = audio_identity(item.get("input_path"), expected_hash=item.get("input_sha256"))
            if item.get("input_samples") != input_audio.samples:
                raise JoinContractError(f"Nomo model-input sample count mismatch for {case_id}")
            if input_audio.samples != prepared_audio.samples:
                raise JoinContractError(f"Nomo model-input timeline differs from prepared source case for {case_id}")
            if item.get("baseline_raw_asr_case_id") != f"{case_id}-raw":
                raise JoinContractError(f"Nomo raw baseline case binding mismatch for {case_id}")
            if item.get("baseline_g0_asr_case_id") != f"{case_id}-g0":
                raise JoinContractError(f"Nomo G0 baseline case binding mismatch for {case_id}")
            if not is_sha256(item.get("baseline_raw_asr_audio_sha256")) or item["baseline_raw_asr_audio_sha256"] != input_audio.sha256:
                raise JoinContractError(f"Nomo raw input digest mismatch for {case_id}")
            if not is_sha256(item.get("baseline_g0_asr_audio_sha256")):
                raise JoinContractError(f"Nomo G0 digest malformed for {case_id}")
            output_audio = audio_identity(item.get("output_path"), expected_hash=item.get("output_sha256"))
            if (item.get("output_samples") != output_audio.samples
                    or output_audio.samples != input_audio.samples
                    or item.get("output_equals_raw_exactly") is not (output_audio.sha256 == input_audio.sha256)):
                raise JoinContractError(f"Nomo output/sample binding mismatch for {case_id}")
            item["_join_samples"] = output_audio.samples
            item["_join_input_audio"] = input_audio
            item["_join_output_audio"] = output_audio
            item["_join_prepared_audio"] = prepared_audio
            item["_join_raw_hash"] = input_audio.sha256
            item["_join_g0_hash"] = item["baseline_g0_asr_audio_sha256"]
            item["_join_raw_id"] = f"{case_id}-raw"
            item["_join_g0_id"] = f"{case_id}-g0"
        else:
            input_audio = audio_identity(item.get("model_input_path"), expected_hash=item.get("model_input_sha256"))
            if (item.get("model_input_sample_rate_hz") != SAMPLE_RATE
                    or item.get("model_input_samples") != input_audio.samples
                    or item.get("model_input_exact_raw_asr_wav") is not True
                    or input_audio.samples != prepared_audio.samples):
                raise JoinContractError(f"REAL-TSE model-input identity mismatch for {case_id}")
            if item.get("raw_baseline_asr_case_id") != f"{case_id}-raw":
                raise JoinContractError(f"REAL-TSE raw baseline case binding mismatch for {case_id}")
            if item.get("g0_baseline_asr_case_id") != f"{case_id}-g0":
                raise JoinContractError(f"REAL-TSE G0 baseline case binding mismatch for {case_id}")
            if (not is_sha256(item.get("raw_baseline_asr_audio_sha256"))
                    or item["raw_baseline_asr_audio_sha256"] != input_audio.sha256
                    or not is_sha256(item.get("g0_baseline_asr_audio_sha256"))):
                raise JoinContractError(f"REAL-TSE baseline digest mismatch for {case_id}")
            output_audio = audio_identity(item.get("candidate_output_path"), expected_hash=item.get("candidate_output_sha256"))
            if (item.get("candidate_output_samples") != output_audio.samples
                    or output_audio.samples != input_audio.samples
                    or item.get("candidate_output_rate_hz", SAMPLE_RATE) != SAMPLE_RATE
                    or item.get("candidate_output_all_zero") is not False):
                raise JoinContractError(f"REAL-TSE output/sample binding mismatch for {case_id}")
            item["_join_samples"] = output_audio.samples
            item["_join_input_audio"] = input_audio
            item["_join_output_audio"] = output_audio
            item["_join_prepared_audio"] = prepared_audio
            item["_join_raw_hash"] = input_audio.sha256
            item["_join_g0_hash"] = item["g0_baseline_asr_audio_sha256"]
            item["_join_raw_id"] = f"{case_id}-raw"
            item["_join_g0_id"] = f"{case_id}-g0"

    return (list(by_id.values()), by_id,
            {"path": str(prepared_path), "sha256": prep_sha}, source_context)


def validate_historical_source_chain(pilot: dict, historical_path: Path, historical: ProbeIndex,
                                     source_context: dict, cases: list[dict]) -> dict:
    input_artifacts = pilot.get("input_artifacts")
    if not isinstance(input_artifacts, dict):
        raise JoinContractError("historical input artifact binding is missing")
    declared_result_path = input_artifacts.get("historical_asr_results_path")
    declared_result_sha = input_artifacts.get("historical_asr_results_sha256")
    if (not isinstance(declared_result_path, str)
            or Path(declared_result_path).resolve() != historical.path
            or declared_result_sha != historical.digest):
        raise JoinContractError("supplied historical ASR report differs from the pilot's declared artifact")

    prepared_path = source_context["prepared_path"]
    prepared_sha = source_context["prepared_sha256"]
    historical_manifest_path = historical.path.parent / "asr-pairs-manifest.json"
    historical_manifest, resolved_historical_manifest, historical_manifest_sha = read_json_bounded(historical_manifest_path)
    if (resolved_historical_manifest != historical_manifest_path.resolve()
            or historical_manifest_sha != historical.report.get("manifest_sha256")
            or historical_manifest_sha != SOURCE_ARTIFACT_PINS["historical_asr_manifest"]):
        raise JoinContractError("historical ASR manifest digest does not match its pinned file")

    conversion_path = historical.path.parent / "asr-pairs-conversion-report.json"
    conversion, resolved_conversion, conversion_sha = read_json_bounded(conversion_path)
    if conversion_sha != SOURCE_ARTIFACT_PINS["historical_asr_conversion_report"]:
        raise JoinContractError("historical ASR conversion report differs from the pinned provenance")
    if (conversion.get("artifact_type") != "voiceflow_speaker_filter_asr_pair_conversion_report"
            or conversion.get("schema_version") != 1
            or conversion.get("provider_calls") != 0
            or conversion.get("paid_inference") is not False):
        raise JoinContractError("historical ASR conversion report schema/local-only identity mismatch")
    artifacts = conversion.get("artifacts")
    if not isinstance(artifacts, dict):
        raise JoinContractError("historical ASR conversion report is missing artifact provenance")
    declared_manifest_path = artifacts.get("manifest_path")
    if (not isinstance(declared_manifest_path, str)
            or Path(declared_manifest_path).resolve() != resolved_historical_manifest
            or artifacts.get("manifest_sha256") != historical_manifest_sha
            or artifacts.get("pair_count") != len(source_context["prepared_cases"])
            or artifacts.get("audio_row_count") != 2 * len(source_context["prepared_cases"])):
        raise JoinContractError("historical ASR manifest does not match conversion provenance")
    provenance_path_value = artifacts.get("provenance_path")
    provenance_sha_declared = artifacts.get("provenance_sha256")
    if not isinstance(provenance_path_value, str) or not is_sha256(provenance_sha_declared):
        raise JoinContractError("historical ASR per-case provenance binding is malformed")
    provenance, provenance_path, provenance_sha = read_json_bounded(Path(provenance_path_value))
    if (provenance_sha != provenance_sha_declared
            or provenance_sha != SOURCE_ARTIFACT_PINS["historical_asr_provenance"]
            or provenance.get("artifact_type") != "voiceflow_speaker_filter_asr_pairs_provenance"
            or provenance.get("schema_version") != 1
            or provenance.get("recipe") != "g0"):
        raise JoinContractError("historical ASR per-case provenance digest/schema mismatch")
    if (conversion.get("source_manifest_sha256") != prepared_sha
            or not isinstance(conversion.get("source_manifest_path"), str)
            or Path(conversion["source_manifest_path"]).resolve() != prepared_path):
        raise JoinContractError("historical ASR conversion is not bound to the prepared source manifest")
    source_run_path_value = conversion.get("source_run_report_path")
    source_run_sha = conversion.get("source_run_report_sha256")
    if not isinstance(source_run_path_value, str) or not is_sha256(source_run_sha):
        raise JoinContractError("historical G0 source-run report binding is malformed")
    source_run, source_run_path, actual_source_run_sha = read_json_bounded(Path(source_run_path_value))
    if (actual_source_run_sha != source_run_sha
            or actual_source_run_sha != SOURCE_ARTIFACT_PINS["historical_asr_source_run"]
            or source_run.get("artifact_type") != "voiceflow_speaker_filter_pilot_report"
            or source_run.get("status") != "complete"
            or source_run.get("recipe") != "g0"
            or source_run.get("manifest_sha256") != prepared_sha
            or not isinstance(source_run.get("manifest_path"), str)
            or Path(source_run["manifest_path"]).resolve() != prepared_path):
        raise JoinContractError("historical ASR source G0 run report is not pinned to the prepared manifest")
    if (source_run.get("case_count") != len(source_context["prepared_cases"])
            or source_run.get("completed_cases") != len(source_context["prepared_cases"])):
        raise JoinContractError("historical G0 source-run case count differs from the prepared inventory")
    source_profile_results = source_run.get("profile_results")
    if not isinstance(source_profile_results, dict) or set(source_profile_results) != set(PROFILE_SPEAKERS):
        raise JoinContractError("historical G0 source profile map is incomplete")
    source_run_dir_value = source_run.get("run_directory")
    if (not isinstance(source_run_dir_value, str) or not source_run_dir_value
            or len(source_run_dir_value) > 4096 or Path(source_run_dir_value).is_symlink()
            or not Path(source_run_dir_value).is_dir()):
        raise JoinContractError("historical G0 source-run case directory is unavailable")
    source_run_dir = Path(source_run_dir_value).resolve(strict=True)

    manifest_items = historical_manifest.get("cases")
    if (not isinstance(manifest_items, list)
            or len(manifest_items) != 2 * len(source_context["prepared_cases"])
            or len(manifest_items) > MAX_PROBE_ROWS):
        raise JoinContractError("historical ASR manifest is outside the bounded row count")
    manifest_by_id: dict[str, dict] = {}
    manifest_audio_total = 0
    for entry in manifest_items:
        if not isinstance(entry, dict) or len(entry) > 16:
            raise JoinContractError("malformed historical ASR manifest row")
        entry_id = _require_short_string(entry.get("id"), "historical manifest ID", max_chars=256)
        if entry_id in manifest_by_id:
            raise JoinContractError(f"duplicate historical manifest identity: {entry_id}")
        audio = audio_identity(entry.get("audio_path"), expected_hash=entry.get("audio_sha256"))
        manifest_audio_total += audio.byte_size
        if manifest_audio_total > MAX_TOTAL_AUDIO_BYTES:
            raise JoinContractError("historical manifest audio exceeds the total byte bound")
        for field, maximum in (("language", 32), ("condition", 128), ("role", 64)):
            _require_short_string(entry.get(field), f"historical manifest {entry_id} {field}", max_chars=maximum)
        manifest_by_id[entry_id] = {**entry, "_audio": audio}

    # Every returned row must map to one explicit manifest entry. Missing rows
    # are retained later as missing evidence; an unmanifested response is an
    # invalid identity and cannot be joined.
    for row_id, row in historical.by_case_id.items():
        entry = manifest_by_id.get(row_id)
        if entry is None:
            raise JoinContractError(f"historical ASR row has no manifest case: {row_id}")
        case = row["case"]
        if any(case.get(field) != entry.get(field) for field in ("id", "audio_path", "audio_sha256", "language", "condition", "role")):
            raise JoinContractError(f"historical ASR row differs from its manifest entry: {row_id}")

    prepared_cases = source_context["prepared_cases"]
    source_run_cases: dict[str, dict] = {}
    for case_id in CASE_ROLES:
        source_case, _source_case_path, _source_case_sha = read_json_bounded(source_run_dir / "case-results" / f"{case_id}.json")
        if source_case.get("case_id") != case_id or source_case.get("status") != "complete" or source_case.get("recipe") != "g0":
            raise JoinContractError(f"historical source G0 case result is missing or invalid: {case_id}")
        source_run_cases[case_id] = source_case
    provenance_rows = provenance.get("cases")
    if not isinstance(provenance_rows, list) or len(provenance_rows) != len(prepared_cases):
        raise JoinContractError("historical ASR provenance must cover every prepared source case")
    provenance_by_id: dict[str, dict] = {}
    for row in provenance_rows:
        if not isinstance(row, dict) or len(row) > 32:
            raise JoinContractError("malformed historical ASR per-case provenance")
        case_id = _require_short_string(row.get("case_id"), "ASR provenance case id", max_chars=256)
        if case_id in provenance_by_id:
            raise JoinContractError(f"duplicate ASR provenance case identity: {case_id}")
        provenance_by_id[case_id] = row
    if set(provenance_by_id) != set(prepared_cases):
        raise JoinContractError("historical ASR provenance case set differs from the prepared manifest")

    source_bindings: list[dict] = []
    candidate_cases_by_id = {case["case_id"]: case for case in cases}
    for case_id in CASE_ROLES:
        prepared_case = prepared_cases[case_id]
        provenance_row = provenance_by_id[case_id]
        source_case = source_run_cases[case_id]
        raw_id = f"{case_id}-raw"
        g0_id = f"{case_id}-g0"
        raw_entry = manifest_by_id.get(raw_id)
        g0_entry = manifest_by_id.get(g0_id)
        if raw_entry is None or g0_entry is None:
            raise JoinContractError(f"historical ASR manifest is missing raw/G0 identity for {case_id}")
        role = CASE_ROLES[case_id]
        language = CASE_LANGUAGES[case_id]
        expected_pair_type = {"other_only": "other_only", "target_soft_level": "soft", "target_mixture": "short"}[role]
        if (raw_entry["role"] != role or g0_entry["role"] != role
                or raw_entry["language"] != language or g0_entry["language"] != language
                or raw_entry["condition"] != f"raw-{expected_pair_type}"
                or g0_entry["condition"] != f"g0-{expected_pair_type}"):
            raise JoinContractError(f"historical raw/G0 manifest role or condition mismatch for {case_id}")
        recipe = prepared_case.get("mix_recipe")
        if not isinstance(recipe, dict):
            raise JoinContractError(f"prepared source recipe is missing for historical case {case_id}")
        prepared_audio = audio_identity(recipe.get("input_audio_path"), expected_hash=recipe.get("input_audio_sha256"))
        raw_input_path, raw_input_sha = _resolved_file_hash(provenance_row.get("raw_input_path"), f"ASR raw source {case_id}")
        if (raw_input_path != prepared_audio.path
                or raw_input_sha != prepared_audio.sha256
                or provenance_row.get("raw_input_sha256") != prepared_audio.sha256
                or provenance_row.get("role") != role
                or provenance_row.get("language") != language
                or provenance_row.get("profile_id") != prepared_case.get("profile_id")):
            raise JoinContractError(f"historical ASR provenance source/profile differs from prepared case {case_id}")
        source_case_input, source_case_input_sha = _resolved_file_hash(source_case.get("input_path"), f"historical G0 source input {case_id}")
        if (source_case_input != prepared_audio.path
                or source_case_input_sha != prepared_audio.sha256
                or source_case.get("input_sha256") != prepared_audio.sha256
                or source_case.get("input_samples") != prepared_audio.samples
                or source_case.get("profile_id") != prepared_case.get("profile_id")):
            raise JoinContractError(f"historical G0 source case differs from prepared input/profile for {case_id}")
        source_case_output, source_case_output_sha = _resolved_file_hash(source_case.get("output_path"), f"historical G0 source output {case_id}")
        provenance_output, provenance_output_sha = _resolved_file_hash(
            provenance_row.get("native_float_output_path"), f"ASR provenance G0 float output {case_id}")
        if (source_case_output != provenance_output
                or source_case_output_sha != provenance_output_sha
                or source_case.get("output_sha256") != provenance_output_sha
                or source_case.get("output_samples") != prepared_audio.samples):
            raise JoinContractError(f"historical G0 output differs from its source-case record for {case_id}")
        expected_raw_payload, expected_raw_samples = _encode_pcm16_payload(prepared_audio.path, source_kind="pcm16")
        actual_raw_payload, actual_raw_samples = _pcm16_payload(raw_entry["_audio"].path)
        if (expected_raw_samples != actual_raw_samples
                or expected_raw_payload != actual_raw_payload
                or provenance_row.get("pcm16_conversion") != ASR_PCM16_CONVERSION_DESC):
            raise JoinContractError(f"historical raw ASR audio is not the pinned conversion of {case_id}")
        for stage, entry, prov_path_field, prov_hash_field in (
            ("raw", raw_entry, "raw_pcm16_path", "raw_pcm16_audio_sha256"),
            ("g0", g0_entry, "filtered_pcm16_path", "filtered_pcm16_audio_sha256"),
        ):
            path, digest = _resolved_file_hash(provenance_row.get(prov_path_field), f"ASR {stage} PCM16 {case_id}")
            if (path != entry["_audio"].path
                    or digest != entry["_audio"].sha256
                    or provenance_row.get(prov_hash_field) != entry["audio_sha256"]):
                raise JoinContractError(f"historical ASR {stage} file differs from per-case provenance for {case_id}")
            if stage == "g0":
                native_path, native_sha = _resolved_file_hash(
                    provenance_row.get("native_float_output_path"), f"historical G0 float output {case_id}")
                if native_sha != provenance_row.get("native_float_output_sha256"):
                    raise JoinContractError(f"historical G0 float output digest mismatch for {case_id}")
                expected_g0_payload, expected_g0_samples = _encode_pcm16_payload(native_path, source_kind="float32")
                actual_g0_payload, actual_g0_samples = _pcm16_payload(entry["_audio"].path)
                if expected_g0_samples != actual_g0_samples or expected_g0_payload != actual_g0_payload:
                    raise JoinContractError(f"historical G0 ASR audio is not the pinned output conversion of {case_id}")
        profile_id = prepared_case.get("profile_id")
        profile = source_context["prepared_profiles"].get(profile_id)
        profile_result = source_profile_results.get(profile_id)
        if (profile is None or not isinstance(profile_result, dict)
                or provenance_row.get("profile_sha256") != profile_result.get("profile_sha256")
                or provenance_row.get("profile_id") != profile_id):
            raise JoinContractError(f"historical ASR enrollment profile identity mismatch for {case_id}")
        source_profile_path, source_profile_sha = _resolved_file_hash(
            profile_result.get("profile_path"), f"historical G0 profile {profile_id}")
        source_profile_doc, _source_profile_doc_path, _source_profile_doc_sha = read_json_bounded(source_profile_path)
        source_profile_audio = source_profile_doc.get("source")
        if (source_profile_sha != profile_result.get("profile_sha256")
                or source_profile_sha != provenance_row.get("profile_sha256")
                or not isinstance(source_profile_audio, dict)
                or source_profile_audio.get("source_sha256") != profile["audio"].sha256):
            raise JoinContractError(f"historical ASR profile file mismatch for {case_id}")
        for prepared_field, provenance_field in (
            ("target_source_case_id", "target_source_case_id"),
            ("target_speaker_id", "target_source_speaker_id"),
            ("target_session_id", "target_session_id"),
            ("target_parent_block_id", "target_parent_block_id"),
        ):
            if prepared_case.get(prepared_field) != provenance_row.get(provenance_field):
                raise JoinContractError(f"historical ASR target source {prepared_field} mismatch for {case_id}")
        if (prepared_case.get("publisher_reference_sha256") is not None
                and provenance_row.get("source_annotation_sha256") != prepared_case.get("publisher_reference_sha256")):
            raise JoinContractError(f"historical ASR source annotation identity mismatch for {case_id}")
        if (raw_entry["_audio"].samples != prepared_audio.samples
                or g0_entry["_audio"].samples != prepared_audio.samples):
            raise JoinContractError(f"historical ASR sample timeline differs from source case {case_id}")
        candidate_case = candidate_cases_by_id.get(case_id)
        if candidate_case is None:
            raise JoinContractError(f"candidate pilot case is missing from the source chain: {case_id}")
        candidate_raw_entry = manifest_by_id[f"{case_id}-raw"]
        candidate_g0_entry = manifest_by_id[f"{case_id}-g0"]
        if (candidate_case["_join_raw_hash"] != candidate_raw_entry["audio_sha256"]
                or candidate_case["_join_input_audio"].path != candidate_raw_entry["_audio"].path
                or candidate_case["_join_g0_hash"] != candidate_g0_entry["audio_sha256"]
                or candidate_case["_join_prepared_audio"].sha256 != prepared_audio.sha256):
            raise JoinContractError(f"candidate pilot inputs are cross-bound from historical rows for {case_id}")
        source_bindings.append({
            "case_id": case_id,
            "role": role,
            "language": language,
            "profile_id": profile_id,
            "profile_source_speaker_id": prepared_case.get("profile_source_speaker_id"),
            "prepared_enrollment_audio_path": str(source_context["prepared_profiles"][profile_id]["audio"].path),
            "prepared_enrollment_audio_sha256": source_context["prepared_profiles"][profile_id]["audio"].sha256,
            "prepared_input_audio_sha256": prepared_audio.sha256,
            "prepared_input_path": str(prepared_audio.path),
            "source_case_id": prepared_case.get("source_case_id"),
            "source_speaker_id": prepared_case.get("source_speaker_id"),
            "source_session_id": prepared_case.get("source_session_id"),
            "source_parent_block_id": prepared_case.get("source_parent_block_id"),
            "source_audio_sha256": prepared_case.get("source_audio_sha256"),
            "target_source_case_id": prepared_case.get("target_source_case_id"),
            "target_source_speaker_id": prepared_case.get("target_speaker_id"),
            "target_source_session_id": prepared_case.get("target_session_id"),
            "target_source_parent_block_id": prepared_case.get("target_parent_block_id"),
            "target_source_audio_sha256": prepared_case.get("target_source_audio_sha256"),
            "historical_raw_manifest_id": raw_id,
            "historical_raw_audio_sha256": raw_entry["audio_sha256"],
            "historical_g0_manifest_id": g0_id,
            "historical_g0_audio_sha256": g0_entry["audio_sha256"],
            "candidate_input_audio_sha256": candidate_case["_join_input_audio"].sha256,
            "candidate_input_audio_path": str(candidate_case["_join_input_audio"].path),
            "candidate_output_audio_sha256": candidate_case["_join_output_audio"].sha256,
            "candidate_output_audio_path": str(candidate_case["_join_output_audio"].path),
            "candidate_output_samples": candidate_case["_join_output_audio"].samples,
        })

    return {
        "manifest_path": str(resolved_historical_manifest),
        "manifest_sha256": historical_manifest_sha,
        "conversion_report_path": str(resolved_conversion),
        "conversion_report_sha256": conversion_sha,
        "provenance_path": str(provenance_path),
        "provenance_sha256": provenance_sha,
        "source_run_report_path": str(source_run_path),
        "source_run_report_sha256": actual_source_run_sha,
        "case_bindings": source_bindings,
        "manifest_by_id": manifest_by_id,
        "provenance_by_id": provenance_by_id,
    }


def validate_candidate_model_pin(pilot: dict, kind: str) -> dict:
    if kind == "nomo":
        source = pilot.get("source")
        encoder = pilot.get("enrollment_encoder")
        if (not isinstance(source, dict) or len(source) > 24
                or source.get("commit") != "cb828468d183424fd574984f00b3ffbcce0be9bc"):
            raise JoinContractError("Nomo source commit mismatch")
        if source.get("checkpoint_sha256") != "0acf2f3e75409b172771663fd1bee31af2760dba4e25b1353d2c26726b6412d1":
            raise JoinContractError("Nomo checkpoint digest mismatch")
        if (not isinstance(encoder, dict) or len(encoder) > 16
                or encoder.get("revision") != "v1.0.2"
                or encoder.get("checkpoint_sha256") != "0eb4057106b2573dd7b132cf0c36273ab29afd192c1610f80baa9c556dbb963c"):
            raise JoinContractError("Nomo enrollment encoder pin mismatch")
        return {"candidate": "nomo_pvad_1.1_whole_utterance", "source_commit": source["commit"], "model_sha256": source["checkpoint_sha256"]}
    source = pilot.get("source")
    model = pilot.get("model")
    if (not isinstance(source, dict) or len(source) > 24
            or source.get("commit") != "2a540977a348fbaa92e623210505430e2cec608d"
            or source.get("model_sha256") != "42b73219eefefcbba4b5da0dcb89dce292359c9f93b1a951c7b6d196da2155b3"):
        raise JoinContractError("REAL-TSE source commit/checkpoint pin mismatch")
    if (not isinstance(model, dict) or len(model) > 24
            or model.get("name") != "spk_emb_causal_100"
            or model.get("checkpoint_loaded_with_weights_only") is not True
            or model.get("strict_state_dict_load") is not True):
        raise JoinContractError("REAL-TSE model loading identity mismatch")
    return {"candidate": model["name"], "source_commit": source["commit"], "model_sha256": source["model_sha256"]}


def load_candidate_manifest(pilot: dict, kind: str, new_probe: ProbeIndex,
                            candidate_by_id: dict[str, dict]) -> tuple[dict, dict[str, dict]]:
    path_field = "asr_manifest_path" if kind == "nomo" else "asr_new_audio_manifest_path"
    manifest_path_value = pilot.get(path_field)
    if not isinstance(manifest_path_value, str):
        raise JoinContractError("candidate ASR manifest path is missing or malformed")
    manifest, manifest_path, manifest_digest = read_json_bounded(Path(manifest_path_value))
    embedded = pilot.get("asr_new_audio_manifest")
    if not isinstance(embedded, dict) or embedded != manifest:
        raise JoinContractError("embedded ASR manifest does not equal its digest-bound file")
    if new_probe.report.get("manifest_sha256") != manifest_digest:
        raise JoinContractError("new ASR report manifest digest mismatch")
    manifest_cases = manifest.get("cases")
    if not isinstance(manifest_cases, list) or len(manifest_cases) > len(CASE_ROLES):
        raise JoinContractError("candidate ASR manifest exceeds the 18-case bound")
    by_source_case: dict[str, dict] = {}
    by_manifest_id: dict[str, dict] = {}
    for item in manifest_cases:
        if not isinstance(item, dict) or len(item) > 16:
            raise JoinContractError("malformed candidate ASR manifest item")
        manifest_id = _require_short_string(item.get("id"), "candidate ASR manifest ID", max_chars=128)
        if manifest_id in by_manifest_id:
            raise JoinContractError(f"duplicate candidate ASR manifest identity: {manifest_id}")
        audio_hash = item.get("audio_sha256")
        if not is_sha256(audio_hash):
            raise JoinContractError(f"malformed candidate manifest digest: {manifest_id}")
        case_ids = item.get("case_ids")
        if not isinstance(case_ids, list) or not case_ids or len(case_ids) > len(CASE_ROLES):
            raise JoinContractError(f"candidate manifest case_ids malformed: {manifest_id}")
        if any(not isinstance(case_id, str) or case_id not in CASE_ROLES for case_id in case_ids):
            raise JoinContractError(f"candidate manifest has noncanonical case identity: {manifest_id}")
        if len(set(case_ids)) != len(case_ids):
            raise JoinContractError(f"duplicate case identity within candidate manifest item: {manifest_id}")
        audio = audio_identity(item.get("audio_path"), expected_hash=audio_hash)
        language = _require_short_string(item.get("language"), f"manifest {manifest_id} language", max_chars=32)
        _require_short_string(item.get("condition"), f"manifest {manifest_id} condition", max_chars=128)
        role = _require_short_string(item.get("role"), f"manifest {manifest_id} role", max_chars=64)
        if role not in set(CASE_ROLES.values()):
            raise JoinContractError(f"candidate manifest role is noncanonical: {role}")
        expected_condition = "nomo_whole_utterance_gate" if kind == "nomo" else "real_tse_spk_emb_causal_100"
        if item["condition"] != expected_condition:
            raise JoinContractError(f"candidate manifest effective condition mismatch: {manifest_id}")
        matching_output_paths = set()
        for case_id in case_ids:
            if case_id in by_source_case:
                raise JoinContractError(f"duplicate global case identity in candidate manifest: {case_id}")
            candidate = candidate_by_id[case_id]
            if (audio.sha256 != candidate["_join_output_audio"].sha256
                    or role != candidate["role"]
                    or language != candidate["language"]):
                raise JoinContractError(f"candidate ASR manifest is misbound to {case_id}")
            matching_output_paths.add(candidate["_join_output_audio"].path)
            by_source_case[case_id] = item
        if audio.path not in matching_output_paths:
            raise JoinContractError(f"candidate ASR manifest path is not one of its bound output files: {manifest_id}")
        by_manifest_id[manifest_id] = item

    # Every new probe row must correspond to exactly one digest-bound manifest entry.
    row_by_manifest_id = new_probe.by_case_id
    extra_rows = set(row_by_manifest_id) - set(by_manifest_id)
    if extra_rows:
        raise JoinContractError(f"new ASR report has rows outside its candidate manifest: {sorted(extra_rows)}")
    for manifest_id, item in by_manifest_id.items():
        row = row_by_manifest_id.get(manifest_id)
        if row is None:
            continue  # A missing/pending result is kept in each mapped case denominator.
        case = row["case"]
        expected_case_ids = item["case_ids"]
        expected_case = candidate_by_id[expected_case_ids[0]]
        if (row["audio_sha256"] != item["audio_sha256"]
                or case.get("audio_path") != item["audio_path"]
                or case.get("case_ids") != expected_case_ids
                or case.get("language") != item["language"]
                or case.get("condition") != item["condition"]
                or case.get("role") != item["role"]):
            raise JoinContractError(f"new ASR row identity differs from manifest {manifest_id}")
        if expected_case["_join_output_audio"].sha256 != item["audio_sha256"]:
            raise JoinContractError(f"new ASR manifest digest no longer matches candidate case {expected_case_ids[0]}")
    return {"path": str(manifest_path), "sha256": manifest_digest}, by_source_case


def candidate_evidence(case: dict, new_probe: ProbeIndex, historical_probe: ProbeIndex,
                       manifest_item: dict | None) -> ASREvidence:
    case_id = case["case_id"]
    candidate_hash = case["_join_output_audio"].sha256
    expected_samples = case["_join_samples"]
    if manifest_item is not None:
        row_id = manifest_item["id"]
        row = new_probe.by_case_id.get(row_id)
        if row is None:
            return ASREvidence("missing", None, "missing_candidate_asr_row", row_id)
        return classify_asr(row, expected_samples=expected_samples, expected_case_id=row_id)
    # Reuse only the same case's exact raw/G0 audio result; unrelated coincident
    # hashes do not inherit another case's transcript or language parameters.
    for stage in ("raw", "g0"):
        expected_id = f"{case_id}-{stage}"
        row = historical_probe.by_case_id.get(expected_id)
        if row is not None and row["audio_sha256"] == candidate_hash:
            return classify_asr(row, expected_samples=expected_samples, expected_case_id=expected_id)
    return ASREvidence("missing", None, "missing_candidate_asr_row", None)


def evidence_fields(evidence: ASREvidence, prefix: str) -> dict:
    text = evidence.text if evidence.status == "completed" else None
    return {
        f"{prefix}_status": evidence.status,
        f"{prefix}_text": text,
        f"{prefix}_error": evidence.error,
        f"{prefix}_request_id": evidence.request_id,
        f"{prefix}_nonempty": bool(evidence.status == "completed" and text and text.strip()),
        f"{prefix}_blank": bool(evidence.status == "completed" and text is not None and not text.strip()),
    }


def summarize_roles(cases: list[dict]) -> dict:
    summary: dict[str, dict] = {}
    for row in cases:
        counts = summary.setdefault(row["role"], {
            "cases": 0,
            "candidate_completed": 0,
            "candidate_nonempty": 0,
            "candidate_blank": 0,
            "candidate_failed": 0,
            "candidate_unexecuted": 0,
            "candidate_incomplete": 0,
            "candidate_unknown": 0,
            "candidate_invalid": 0,
        })
        counts["cases"] += 1
        status = row["candidate_asr_status"]
        if status == "completed":
            counts["candidate_completed"] += 1
            counts["candidate_nonempty"] += int(row["candidate_nonempty"])
            counts["candidate_blank"] += int(row["candidate_blank"])
        elif status in {"failed", "missing"}:
            counts["candidate_failed"] += 1
        elif status == "unexecuted":
            counts["candidate_unexecuted"] += 1
        elif status == "incomplete":
            counts["candidate_incomplete"] += 1
        elif status == "unknown":
            counts["candidate_unknown"] += 1
        else:
            counts["candidate_invalid"] += 1
    return summary


def summary_status(cases: list[dict]) -> str:
    if all(row["candidate_asr_status"] == "completed" for row in cases):
        return "complete"
    return "complete_with_unknown_or_failed_cases"


def write_private_json(path: Path, payload: object) -> None:
    output = path.resolve()
    if output.exists() or output.is_symlink():
        raise JoinContractError(f"refusing to overwrite joined receipt: {output}")
    output.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=2)
        stream.write("\n")

