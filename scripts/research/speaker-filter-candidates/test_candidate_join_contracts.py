#!/usr/bin/env python3
"""Adversarial contract checks for the two exact-hash candidate joiners.

Uses tiny synthetic PCM16 fixtures and the native-probe response schema; it
does not load a model, access a network, or depend on private run artifacts.
"""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import wave
from unittest.mock import patch

from candidate_asr_join_contracts import (
    CASE_LANGUAGES,
    CASE_ROLES,
    ASR_PCM16_CONVERSION_DESC,
    PROBE_EXECUTABLE_SHA256,
    PROFILE_SPEAKERS,
    QWEN_MODEL_ID,
    QWEN_REVISION,
    SOURCE_ARTIFACT_PINS,
    write_private_json,
)
from join_nomo_pvad_asr import build_summary as build_nomo_summary
from join_real_tse_asr import build_summary as build_real_tse_summary


HERE = Path(__file__).resolve().parent
JOINERS = {
    "nomo": HERE / "join_nomo_pvad_asr.py",
    "real_tse": HERE / "join_real_tse_asr.py",
}
IDS = list(CASE_ROLES)
CASE_PROFILE_IDS = {
    "other-only-en-spk17-00114": "public-ascend-speaker-7",
    "other-only-en-spk17-00601": "public-ascend-speaker-12",
    "other-only-en-spk3-00318": "public-ascend-speaker-20",
    "other-only-en-spk3-00418": "public-ascend-speaker-12",
    "other-only-mixed-spk17-01210": "public-ascend-speaker-20",
    "other-only-mixed-spk3-00383": "public-ascend-speaker-7",
    "other-only-mixed-spk3-00644": "public-ascend-speaker-20",
    "other-only-mixed-spk3-00712": "public-ascend-speaker-12",
    "other-only-zh-spk17-00204": "public-ascend-speaker-7",
    "other-only-zh-spk17-00828": "public-ascend-speaker-12",
    "other-only-zh-spk17-01105": "public-ascend-speaker-20",
    "other-only-zh-spk3-00252": "public-ascend-speaker-7",
    "target-en-spk20-00840": "public-ascend-speaker-20",
    "target-mixed-spk20-00649": "public-ascend-speaker-20",
    "target-zh-spk12-01068": "public-ascend-speaker-12",
    "target-en-spk7-00886": "public-ascend-speaker-7",
    "target-mixed-spk12-00462": "public-ascend-speaker-12",
    "target-zh-spk20-00018": "public-ascend-speaker-20",
}


def file_sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path: Path, data: object) -> None:
    path.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")


def wav(path: Path, marker: int, samples: int) -> tuple[str, int]:
    path.parent.mkdir(parents=True, exist_ok=True)
    raw = b"".join(struct.pack("<h", ((marker + i * 37) % 12_000) - 6_000) for i in range(samples))
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(16_000)
        audio.writeframes(raw)
    return file_sha(path), samples


def _f32(value: float) -> float:
    return struct.unpack("<f", struct.pack("<f", value))[0]


def pcm16_conversion(source: Path) -> bytes:
    with wave.open(str(source), "rb") as audio:
        values = struct.unpack("<" + "h" * audio.getnframes(), audio.readframes(audio.getnframes()))
    converted = []
    for sample in values:
        normalized = _f32(sample / 32768.0)
        converted.append(max(-32768, min(32767, int(_f32(normalized * 32767.0)))))
    return struct.pack("<" + "h" * len(converted), *converted)


def write_pcm16_payload(path: Path, payload: bytes) -> tuple[str, int]:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(16_000)
        audio.writeframes(payload)
    return file_sha(path), len(payload) // 2


def write_float32_wav(path: Path, source: Path, *, scale: float = 0.5) -> tuple[str, int]:
    with wave.open(str(source), "rb") as audio:
        values = struct.unpack("<" + "h" * audio.getnframes(), audio.readframes(audio.getnframes()))
    samples = [_f32(max(-1.0, min(1.0, _f32(value / 32768.0) * scale))) for value in values]
    data = struct.pack("<" + "f" * len(samples), *samples)
    fmt = (struct.pack("<HHIIHHH", 0xFFFE, 1, 16_000, 64_000, 4, 32, 22)
           + struct.pack("<HI", 32, 0)
           + bytes.fromhex("0300000000001000800000aa00389b71"))
    fmt_chunk = b"fmt " + struct.pack("<I", len(fmt)) + fmt
    data_chunk = b"data" + struct.pack("<I", len(data)) + data
    body = b"WAVE" + fmt_chunk + data_chunk
    raw = b"RIFF" + struct.pack("<I", len(body)) + body
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(raw)
    return file_sha(path), len(samples)


def float32_conversion(source: Path) -> bytes:
    raw = source.read_bytes()
    pos = 12
    payload = None
    while pos + 8 <= len(raw):
        name = raw[pos:pos + 4]
        size = int.from_bytes(raw[pos + 4:pos + 8], "little")
        if name == b"data":
            payload = raw[pos + 8:pos + 8 + size]
            break
        pos += 8 + size + (size & 1)
    if payload is None:
        raise AssertionError("fixture float32 WAV has no data chunk")
    values = struct.unpack("<" + "f" * (len(payload) // 4), payload)
    converted = [max(-32768, min(32767, int(_f32(max(-1.0, min(1.0, x)) * 32767.0)))) for x in values]
    return struct.pack("<" + "h" * len(converted), *converted)


def response(*, samples: int, text: object, status: str = "completed", request_id: str) -> dict:
    return {
        "cleanup_result": None,
        "error": None if status == "completed" else {"code": "fixture_failure"},
        "loaded_model_id": QWEN_MODEL_ID,
        "loaded_model_version": QWEN_REVISION,
        "model_version": QWEN_REVISION,
        "op": "transcribe",
        "protocol_version": 1,
        "request_id": request_id,
        "result": {
            "chunks_completed": 1,
            "chunks_total": 1,
            "completion": "complete",
            "covered_samples": samples,
            "generation_tokens": 1,
            "language": "fixture-language",
            "text": text,
        },
        "runtime_version": "1.1.0",
        "status": status,
    }


def asr_row(*, case_id: str, audio_path: Path, audio_hash: str, samples: int,
            role: str, language: str, condition: str, text: object,
            request_id: str, case_ids: list[str] | None = None) -> dict:
    case = {
        "id": case_id,
        "audio_path": str(audio_path),
        "audio_sha256": audio_hash,
        "reference": "fixture-reference",
        "language": language,
        "condition": condition,
        "role": role,
    }
    if case_ids is not None:
        case["case_ids"] = case_ids
    return {
        "case": case,
        "audio_sha256": audio_hash,
        "audio_seconds": samples / 16_000,
        "elapsed_seconds": 0.01,
        "response": response(samples=samples, text=text, request_id=request_id),
    }


def probe(rows: list[dict], manifest_path: Path) -> dict:
    return {
        "artifact_type": "local_speaker_filter_native_asr_probe",
        "schema_version": 1,
        "manifest_sha256": file_sha(manifest_path),
        "executable_sha256": PROBE_EXECUTABLE_SHA256,
        "model_id": QWEN_MODEL_ID,
        "revision": QWEN_REVISION,
        "cloud_calls": 0,
        "microphone_used": False,
        "gui_e2e_verified": False,
        "rows": rows,
    }


class Fixture:
    def __init__(self, kind: str, root: Path):
        self.kind = kind
        self.root = root
        self.files = root / "audio"
        self.files.mkdir(parents=True)
        self.prepared_path = root / "prepared.json"
        self.pilot_path = root / "pilot.json"
        self.historical_path = root / "historical.json"
        self.manifest_path = root / "candidate-manifest.json"
        self.new_path = root / "new.json"
        self.output_path = root / "joined.json"
        self.mutated_case_id: str | None = None
        self.pilot = self._create()

    def _create(self) -> dict:
        self.frozen_dir = self.root / "frozen-g0"
        self.frozen_case_dir = self.frozen_dir / "case-results"
        self.frozen_profiles_dir = self.frozen_dir / "profiles"
        self.source_run_dir = self.root / "source-g0"
        self.source_case_dir = self.source_run_dir / "case-results"
        self.source_profiles_dir = self.source_run_dir / "profiles"
        for directory in (self.frozen_case_dir, self.frozen_profiles_dir, self.source_case_dir, self.source_profiles_dir):
            directory.mkdir(parents=True)

        profile_rows = []
        profile_files = {}
        for profile_id, speaker in PROFILE_SPEAKERS.items():
            path = self.files / f"enroll-{speaker}.wav"
            digest, samples = wav(path, 700 + speaker, 1600 + speaker)
            profile_files[profile_id] = (path, digest, samples)
            profile_rows.append({
                "profile_id": profile_id,
                "source_speaker_id": speaker,
                "input_audio_path": str(path),
                "input_audio_sha256": digest,
                "input_samples": samples,
            })
        self.profile_files = profile_files
        prepared_cases = []
        candidate_cases = []
        historical_rows = []
        historical_manifest_rows = []
        candidate_manifest_rows = []
        new_rows = []
        source_provenance_rows = []
        for index, case_id in enumerate(IDS):
            role = CASE_ROLES[case_id]
            language = CASE_LANGUAGES[case_id]
            profile_id = CASE_PROFILE_IDS[case_id]
            speaker = PROFILE_SPEAKERS[profile_id]
            profile_path, profile_hash, _profile_samples = profile_files[profile_id]
            sample_count = 320 + 16 * index
            source_path = self.files / f"{case_id}-prepared-source.wav"
            source_hash, _ = wav(source_path, 10 + index, sample_count)
            reference_hash = hashlib.sha256(f"reference:{case_id}".encode()).hexdigest()
            mix_recipe = {
                "recipe_id": "fixture-source-preserve-v1",
                "input_audio_path": str(source_path),
                "input_audio_sha256": source_hash,
            }
            source_fields = {}
            if role == "other_only":
                source_speaker_match = __import__("re").search(r"spk(\d+)", case_id)
                assert source_speaker_match is not None
                source_speaker = int(source_speaker_match.group(1))
                source_suffix = case_id.rsplit("-", 1)[1]
                component_path = self.files / f"{case_id}-other-component.wav"
                component_hash, _ = wav(component_path, 100 + index, sample_count)
                source_fields = {
                    "source_case_id": f"ascend-test-{source_suffix}",
                    "source_path": str(component_path),
                    "source_audio_sha256": component_hash,
                    "source_speaker_id": source_speaker,
                    "source_session_id": 1,
                    "source_parent_block_id": f"ascend:test:spk{source_speaker}:ses1:fixture",
                }
                mix_recipe["target_component"] = None
                mix_recipe["interferer_component"] = {
                    "source_case_id": source_fields["source_case_id"],
                    "source_speaker_id": source_speaker,
                    "source_parent_block_id": source_fields["source_parent_block_id"],
                    "gain": 1.0,
                    "offset_samples": 0,
                }
            else:
                source_suffix = case_id.rsplit("-", 1)[1]
                target_path = self.files / f"{case_id}-target-source.wav"
                target_hash, _ = wav(target_path, 200 + index, sample_count)
                source_fields = {
                    "target_source_case_id": f"ascend-validation-{source_suffix}",
                    "target_speaker_id": speaker,
                    "target_session_id": 1,
                    "target_parent_block_id": f"ascend:validation:spk{speaker}:ses1:fixture",
                    "target_source_audio_path": str(target_path),
                    "target_source_audio_sha256": target_hash,
                }
                mix_recipe.update({
                    "target_source_case_id": source_fields["target_source_case_id"],
                    "target_source_path": str(target_path),
                    "target_source_audio_sha256": target_hash,
                    "target_speaker_id": speaker,
                    "target_session_id": 1,
                    "target_parent_block_id": source_fields["target_parent_block_id"],
                    "interferer": None,
                })
            prepared_case = {
                "case_id": case_id,
                "role": role,
                "language": language,
                "profile_id": profile_id,
                "profile_source_speaker_id": speaker,
                "soft_level_challenge": role == "target_soft_level",
                "publisher_reference_sha256": reference_hash,
                "asr_pair_selection": {"other_only": None, "target_soft_level": "soft", "target_mixture": "short"}[role],
                "mix_recipe": mix_recipe,
                **source_fields,
            }
            prepared_cases.append(prepared_case)

            raw_path = self.files / f"{case_id}-raw.wav"
            raw_hash, _ = write_pcm16_payload(raw_path, pcm16_conversion(source_path))
            g0_float_path = self.files / f"{case_id}-g0-float.wav"
            g0_float_hash, _ = write_float32_wav(g0_float_path, source_path)
            g0_path = self.files / f"{case_id}-g0.wav"
            g0_hash, _ = write_pcm16_payload(g0_path, float32_conversion(g0_float_path))
            candidate_path = self.files / f"{case_id}-{self.kind}-candidate.wav"
            raw_passthrough = self.kind == "nomo" and (
                role != "other_only" or case_id == "other-only-en-spk17-00114"
            )
            if raw_passthrough:
                shutil.copyfile(raw_path, candidate_path)
                output_hash = file_sha(candidate_path)
            else:
                output_hash, _ = wav(candidate_path, 900 + index, sample_count)

            condition = "nomo_whole_utterance_gate" if self.kind == "nomo" else "real_tse_spk_emb_causal_100"
            if output_hash != raw_hash:
                manifest_id = f"{self.kind}-filter-{output_hash[:16]}"
                candidate_manifest_rows.append({
                    "id": manifest_id,
                    "audio_path": str(candidate_path),
                    "audio_sha256": output_hash,
                    "reference": "fixture-reference",
                    "language": language,
                    "condition": condition,
                    "role": role,
                    "case_ids": [case_id],
                })
                new_rows.append(asr_row(
                    case_id=manifest_id, audio_path=candidate_path, audio_hash=output_hash,
                    samples=sample_count, role=role, language=language, condition=condition,
                    text="recognized fixture text", request_id=f"new-{index}", case_ids=[case_id],
                ))
            base_condition = {"other_only": "other_only", "target_soft_level": "soft", "target_mixture": "short"}[role]
            for stage, stage_path, digest in (("raw", raw_path, raw_hash), ("g0", g0_path, g0_hash)):
                manifest_id = f"{case_id}-{stage}"
                condition_name = f"{stage}-{base_condition}"
                historical_manifest_rows.append({
                    "id": manifest_id,
                    "audio_path": str(stage_path),
                    "audio_sha256": digest,
                    "reference": "fixture-reference",
                    "language": language,
                    "condition": condition_name,
                    "role": role,
                })
                historical_rows.append(asr_row(
                    case_id=manifest_id, audio_path=stage_path, audio_hash=digest,
                    samples=sample_count, role=role, language=language,
                    condition=condition_name, text="baseline fixture text", request_id=f"old-{index}-{stage}",
                ))

            profile_json = {
                "schema_version": 1,
                "source": {
                    "source_sha256": profile_hash,
                    "sample_rate_hz": 16_000,
                    "channels": 1,
                    "sample_format": "pcm16",
                },
            }
            frozen_profile_path = self.frozen_profiles_dir / f"{profile_id}.json"
            source_profile_path = self.source_profiles_dir / f"{profile_id}.json"
            write_json(frozen_profile_path, profile_json)
            write_json(source_profile_path, profile_json)
            frozen_profile_hash = file_sha(frozen_profile_path)
            source_profile_hash = file_sha(source_profile_path)
            assert frozen_profile_hash == source_profile_hash

            common = {
                "case_id": case_id,
                "role": role,
                "soft_level_challenge": role == "target_soft_level",
                "language": language,
                "profile_id": profile_id,
                "profile_source_speaker_id": speaker,
            }
            frozen_case = {
                **common,
                "recipe": "g0",
                "status": "complete",
                "input_path": str(source_path),
                "input_sha256": source_hash,
                "input_samples": sample_count,
                "profile_path": str(frozen_profile_path),
                "profile_sha256": frozen_profile_hash,
                "output_path": str(g0_float_path),
                "output_sha256": g0_float_hash,
                "output_samples": sample_count,
                "publisher_reference_sha256": reference_hash,
                "asr_pair_selection": prepared_case["asr_pair_selection"],
                "native_report": {
                    "status": "ok", "sample_rate_hz": 16_000, "channels": 1,
                    "input_samples": sample_count, "output_samples": sample_count,
                },
            }
            for prepared_field, frozen_field in (
                ("target_source_case_id", "target_source_case_id"),
                ("target_speaker_id", "target_source_speaker_id"),
                ("target_session_id", "target_source_session_id"),
                ("target_parent_block_id", "target_source_parent_block_id"),
                ("target_source_audio_sha256", "target_source_audio_sha256"),
            ):
                if prepared_field in prepared_case:
                    frozen_case[frozen_field] = prepared_case[prepared_field]
            write_json(self.frozen_case_dir / f"{case_id}.json", frozen_case)
            source_case = copy.deepcopy(frozen_case)
            source_case["profile_path"] = str(source_profile_path)
            source_case["profile_sha256"] = source_profile_hash
            write_json(self.source_case_dir / f"{case_id}.json", source_case)

            source_provenance = {
                "case_id": case_id,
                "role": role,
                "language": language,
                "pair_type": base_condition,
                "source_annotation_sha256": reference_hash,
                "raw_input_path": str(source_path),
                "raw_input_sha256": source_hash,
                "raw_pcm16_path": str(raw_path),
                "raw_pcm16_audio_sha256": raw_hash,
                "native_float_output_path": str(g0_float_path),
                "native_float_output_sha256": g0_float_hash,
                "filtered_pcm16_path": str(g0_path),
                "filtered_pcm16_audio_sha256": g0_hash,
                "pcm16_conversion": ASR_PCM16_CONVERSION_DESC,
                "profile_id": profile_id,
                "profile_sha256": source_profile_hash,
            }
            source_provenance.update({
                "target_source_case_id": prepared_case.get("target_source_case_id"),
                "target_source_speaker_id": prepared_case.get("target_speaker_id"),
                "target_session_id": prepared_case.get("target_session_id"),
                "target_parent_block_id": prepared_case.get("target_parent_block_id"),
            })
            source_provenance_rows.append(source_provenance)

            if self.kind == "nomo":
                candidate_cases.append({
                    **common,
                    "input_path": str(raw_path),
                    "input_sha256": raw_hash,
                    "input_samples": sample_count,
                    "output_path": str(candidate_path),
                    "output_sha256": output_hash,
                    "output_samples": sample_count,
                    "output_equals_raw_exactly": raw_passthrough,
                    "baseline_raw_asr_case_id": f"{case_id}-raw",
                    "baseline_raw_asr_audio_sha256": raw_hash,
                    "baseline_g0_asr_case_id": f"{case_id}-g0",
                    "baseline_g0_asr_audio_sha256": g0_hash,
                    "target_probability_threshold": 0.7,
                    "max_probability": 0.2,
                    "frames_at_or_above_threshold": 0,
                    "decision": "pass_raw" if raw_passthrough else "zero_full_utterance",
                    "publisher_reference": "fixture-reference",
                    "reference_project_review_status": "pending",
                })
            else:
                candidate_cases.append({
                    **common,
                    "enrollment_path": str(profile_path),
                    "enrollment_sha256": profile_hash,
                    "model_input_path": str(raw_path),
                    "model_input_sha256": raw_hash,
                    "model_input_exact_raw_asr_wav": True,
                    "model_input_sample_rate_hz": 16_000,
                    "model_input_samples": sample_count,
                    "raw_baseline_asr_case_id": f"{case_id}-raw",
                    "raw_baseline_asr_audio_sha256": raw_hash,
                    "g0_baseline_asr_case_id": f"{case_id}-g0",
                    "g0_baseline_asr_audio_sha256": g0_hash,
                    "candidate_output_path": str(candidate_path),
                    "candidate_output_sha256": output_hash,
                    "candidate_output_samples": sample_count,
                    "candidate_output_all_zero": False,
                    "candidate_output_rms_dbfs": -30.0,
                    "model_output_length_delta_samples": 0,
                    "whole_utterance_real_time_factor": 0.3,
                    "reference": "fixture-reference",
                    "reference_project_review_status": "pending",
                })

        prepared = {"profiles": profile_rows, "cases": prepared_cases}
        write_json(self.prepared_path, prepared)
        prepared_hash = file_sha(self.prepared_path)
        manifest = {"cases": candidate_manifest_rows}
        write_json(self.manifest_path, manifest)
        historical_manifest_path = self.historical_path.parent / "asr-pairs-manifest.json"
        write_json(historical_manifest_path, {"cases": historical_manifest_rows})
        historical_manifest_hash = file_sha(historical_manifest_path)
        provenance_path = self.historical_path.parent / "asr-pairs-provenance.json"
        write_json(provenance_path, {
            "artifact_type": "voiceflow_speaker_filter_asr_pairs_provenance",
            "schema_version": 1,
            "recipe": "g0",
            "cases": source_provenance_rows,
        })
        provenance_hash = file_sha(provenance_path)
        profile_results = {}
        for profile_id, (path, _audio_sha, _samples) in profile_files.items():
            frozen_path = self.frozen_profiles_dir / f"{profile_id}.json"
            profile_results[profile_id] = {
                "status": "complete",
                "profile_path": str(frozen_path),
                "profile_sha256": file_sha(frozen_path),
            }
        run_header = {
            "artifact_type": "voiceflow_speaker_filter_pilot_run_header",
            "recipe": "g0",
            "manifest_path": str(self.prepared_path),
            "manifest_sha256": prepared_hash,
            "case_ids": IDS,
        }
        write_json(self.frozen_dir / "run-header.json", run_header)
        frozen_report = {
            "artifact_type": "voiceflow_speaker_filter_pilot_report",
            "recipe": "g0",
            "status": "complete",
            "manifest_path": str(self.prepared_path),
            "manifest_sha256": prepared_hash,
            "case_count": 18,
            "completed_cases": 18,
            "profile_results": profile_results,
        }
        write_json(self.frozen_dir / "pilot-report.json", frozen_report)
        source_profile_results = {}
        for profile_id, (path, _audio_sha, _samples) in profile_files.items():
            source_path = self.source_profiles_dir / f"{profile_id}.json"
            source_profile_results[profile_id] = {
                "status": "complete",
                "profile_path": str(source_path),
                "profile_sha256": file_sha(source_path),
            }
        source_run_report_path = self.root / "source-g0-pilot.json"
        source_run_report = {
            "artifact_type": "voiceflow_speaker_filter_pilot_report",
            "recipe": "g0",
            "status": "complete",
            "manifest_path": str(self.prepared_path),
            "manifest_sha256": prepared_hash,
            "case_count": len(prepared_cases),
            "completed_cases": len(prepared_cases),
            "run_directory": str(self.source_run_dir),
            "profile_results": source_profile_results,
        }
        write_json(source_run_report_path, source_run_report)
        conversion_path = self.historical_path.parent / "asr-pairs-conversion-report.json"
        write_json(conversion_path, {
            "artifact_type": "voiceflow_speaker_filter_asr_pair_conversion_report",
            "schema_version": 1,
            "source_run_report_path": str(source_run_report_path),
            "source_run_report_sha256": file_sha(source_run_report_path),
            "source_manifest_path": str(self.prepared_path),
            "source_manifest_sha256": prepared_hash,
            "artifacts": {
                "manifest_path": str(historical_manifest_path),
                "manifest_sha256": historical_manifest_hash,
                "provenance_path": str(provenance_path),
                "provenance_sha256": provenance_hash,
                "pair_count": len(IDS),
                "audio_row_count": len(historical_manifest_rows),
            },
            "provider_calls": 0,
            "paid_inference": False,
        })
        conversion_hash = file_sha(conversion_path)
        write_json(self.historical_path, probe(historical_rows, historical_manifest_path))
        write_json(self.new_path, probe(new_rows, self.manifest_path))

        if self.kind == "nomo":
            pilot = {
                "artifact_type": "voiceflow_nomo_pvad_whole_utterance_pilot",
                "schema_version": 1,
                "status": "complete",
                "source": {
                    "commit": "cb828468d183424fd574984f00b3ffbcce0be9bc",
                    "checkpoint_sha256": "0acf2f3e75409b172771663fd1bee31af2760dba4e25b1353d2c26726b6412d1",
                },
                "enrollment_encoder": {
                    "revision": "v1.0.2",
                    "checkpoint_sha256": "0eb4057106b2573dd7b132cf0c36273ab29afd192c1610f80baa9c556dbb963c",
                },
                "input_artifacts": {
                    "prepared_manifest_path": str(self.prepared_path),
                    "prepared_manifest_sha256": prepared_hash,
                    "frozen_case_dir": str(self.frozen_dir),
                    "historical_asr_results_path": str(self.historical_path),
                    "historical_asr_results_sha256": file_sha(self.historical_path),
                },
                "profiles": {
                    profile_id: {
                        "source_speaker_id": PROFILE_SPEAKERS[profile_id],
                        "enrollment_source_sha256": profile_hash,
                    }
                    for profile_id, (profile_path, profile_hash, _count) in profile_files.items()
                },
                "asr_manifest_path": str(self.manifest_path),
                "asr_new_audio_manifest": manifest,
                "cases": candidate_cases,
            }
        else:
            pilot = {
                "artifact_type": "voiceflow_real_tse_causal_private_pilot",
                "schema_version": 1,
                "status": "complete",
                "source": {
                    "commit": "2a540977a348fbaa92e623210505430e2cec608d",
                    "model_sha256": "42b73219eefefcbba4b5da0dcb89dce292359c9f93b1a951c7b6d196da2155b3",
                },
                "model": {
                    "name": "spk_emb_causal_100",
                    "checkpoint_loaded_with_weights_only": True,
                    "strict_state_dict_load": True,
                },
                "input_artifacts": {
                    "prepared_manifest_path": str(self.prepared_path),
                    "prepared_manifest_sha256": prepared_hash,
                    "frozen_case_dir": str(self.frozen_dir),
                    "historical_asr_results_path": str(self.historical_path),
                    "historical_asr_results_sha256": file_sha(self.historical_path),
                },
                "asr_new_audio_manifest_path": str(self.manifest_path),
                "asr_new_audio_manifest": manifest,
                "cases": candidate_cases,
            }
        self.pilot = pilot
        write_json(self.pilot_path, pilot)
        self.source_pins = {
            "prepared_manifest": prepared_hash,
            "historical_asr_manifest": historical_manifest_hash,
            "historical_asr_conversion_report": conversion_hash,
            "historical_asr_provenance": provenance_hash,
            "historical_asr_source_run": file_sha(source_run_report_path),
            "frozen_g0_report": file_sha(self.frozen_dir / "pilot-report.json"),
            "frozen_g0_run_header": file_sha(self.frozen_dir / "run-header.json"),
        }
        return pilot

    def run(self, *, output_path: Path | None = None) -> subprocess.CompletedProcess:
        output = output_path or self.output_path
        pilot = json.loads(self.pilot_path.read_text(encoding="utf-8"))
        if self.historical_path.exists():
            pilot["input_artifacts"]["historical_asr_results_sha256"] = file_sha(self.historical_path)
        write_json(self.pilot_path, pilot)
        builder = build_nomo_summary if self.kind == "nomo" else build_real_tse_summary
        try:
            with patch.dict(SOURCE_ARTIFACT_PINS, self.source_pins):
                summary = builder(self.pilot_path, self.historical_path, self.new_path)
                write_private_json(output, summary)
            return subprocess.CompletedProcess([], 0, json.dumps({"status": summary["status"]}), "")
        except Exception as exc:
            return subprocess.CompletedProcess([], 2, "", f"join rejected: {exc}")


class CandidateJoinContracts(unittest.TestCase):
    def test_canonical_18_case_join_and_roles(self) -> None:
        for kind in JOINERS:
            with self.subTest(joiner=kind), tempfile.TemporaryDirectory() as temp:
                fixture = Fixture(kind, Path(temp))
                result = fixture.run()
                self.assertEqual(result.returncode, 0, result.stderr)
                joined = json.loads(fixture.output_path.read_text(encoding="utf-8"))
                self.assertEqual(joined["schema_version"], 4)
                self.assertEqual(joined["case_count"], 18)
                self.assertEqual(joined["canonical_role_counts"], {
                    "other_only": 12, "target_soft_level": 3, "target_mixture": 3,
                })
                self.assertEqual(len({row["case_id"] for row in joined["cases"]}), 18)
                self.assertTrue(all(row["candidate_asr_status"] == "completed" for row in joined["cases"]))
                self.assertTrue(all(row["raw_asr_status"] == "completed" for row in joined["cases"]))
                self.assertTrue(all(row["g0_asr_status"] == "completed" for row in joined["cases"]))
                candidate_request_ids = [row["candidate_asr_request_id"] for row in joined["cases"]]
                self.assertTrue(all(isinstance(request_id, str) and request_id for request_id in candidate_request_ids))
                self.assertEqual(len(candidate_request_ids), len(set(candidate_request_ids)))

    def _mutated_run(self, kind: str, mutation: str) -> tuple[Fixture, subprocess.CompletedProcess]:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        fixture = Fixture(kind, Path(temp.name))
        pilot = fixture.pilot
        old = json.loads(fixture.historical_path.read_text(encoding="utf-8"))
        new = json.loads(fixture.new_path.read_text(encoding="utf-8"))
        manifest = json.loads(fixture.manifest_path.read_text(encoding="utf-8"))
        new_rows = new["rows"]
        historical_rows = old["rows"]
        pilot_cases = pilot["cases"]
        output_path = fixture.output_path

        if mutation == "duplicate_case_id":
            pilot_cases[1]["case_id"] = pilot_cases[0]["case_id"]
            write_json(fixture.pilot_path, pilot)
        elif mutation == "wrong_loaded_model":
            new_rows[0]["response"]["loaded_model_id"] = "wrong-model"
            write_json(fixture.new_path, new)
        elif mutation == "wrong_model_revision":
            new_rows[0]["response"]["model_version"] = "other-revision"
            write_json(fixture.new_path, new)
        elif mutation == "wrong_top_level_revision":
            new["revision"] = "other-revision"
            write_json(fixture.new_path, new)
        elif mutation == "wrong_operation":
            new_rows[0]["response"]["op"] = "cleanup"
            write_json(fixture.new_path, new)
        elif mutation == "wrong_runtime":
            new_rows[0]["response"]["runtime_version"] = "wrong-runtime"
            write_json(fixture.new_path, new)
        elif mutation == "missing_model_attestation":
            new_rows[0]["response"].pop("loaded_model_id", None)
            fixture.mutated_case_id = new_rows[0]["case"]["case_ids"][0]
            write_json(fixture.new_path, new)
        elif mutation == "wrong_manifest_digest":
            new["manifest_sha256"] = "0" * 64
            write_json(fixture.new_path, new)
        elif mutation == "wrong_condition":
            new_rows[0]["case"]["condition"] = "wrong-condition"
            write_json(fixture.new_path, new)
        elif mutation == "wrong_protocol":
            new_rows[0]["response"]["protocol_version"] = 2
            write_json(fixture.new_path, new)
        elif mutation == "wrong_language_parameter":
            new_rows[0]["case"]["language"] = "zh"
            write_json(fixture.new_path, new)
        elif mutation == "conflicting_duplicate_hash":
            duplicate = copy.deepcopy(historical_rows[0])
            duplicate["case"]["id"] = "distinct-case-same-bytes"
            duplicate["response"]["request_id"] = "distinct-request-id"
            duplicate["response"]["result"]["text"] = "conflicting transcript"
            historical_rows.append(duplicate)
            write_json(fixture.historical_path, old)
        elif mutation == "duplicate_asr_case_id":
            duplicate = copy.deepcopy(new_rows[0])
            duplicate["response"]["request_id"] = "new-duplicate-case-request"
            new_rows.append(duplicate)
            write_json(fixture.new_path, new)
        elif mutation == "duplicate_request_id":
            new_rows[1]["response"]["request_id"] = new_rows[0]["response"]["request_id"]
            write_json(fixture.new_path, new)
        elif mutation == "wrong_candidate_output_digest":
            hash_field = "output_sha256" if kind == "nomo" else "candidate_output_sha256"
            pilot_cases[0][hash_field] = "0" * 64
            write_json(fixture.pilot_path, pilot)
        elif mutation == "wrong_new_audio_file_digest":
            new_rows[0]["audio_sha256"] = "f" * 64
            new_rows[0]["case"]["audio_sha256"] = "f" * 64
            write_json(fixture.new_path, new)
        elif mutation == "case_profile_relabel_7_to_12":
            case_id = "other-only-en-spk17-00114"
            candidate_case = next(case for case in pilot_cases if case["case_id"] == case_id)
            candidate_case["profile_id"] = "public-ascend-speaker-12"
            candidate_case["profile_source_speaker_id"] = 12
            if kind == "real_tse":
                profile_path, profile_hash, _samples = fixture.profile_files["public-ascend-speaker-12"]
                candidate_case["enrollment_path"] = str(profile_path)
                candidate_case["enrollment_sha256"] = profile_hash
            write_json(fixture.pilot_path, pilot)
        elif mutation == "wrong_historical_manifest_digest":
            old["manifest_sha256"] = "0" * 64
            write_json(fixture.historical_path, old)
        elif mutation == "short_candidate_output_wav":
            row = new_rows[0]
            case_id = row["case"]["case_ids"][0]
            candidate_case = next(case for case in pilot_cases if case["case_id"] == case_id)
            audio_path = Path(row["case"]["audio_path"])
            short_hash, _short_samples = wav(audio_path, 999, 7)
            hash_field = "output_sha256" if kind == "nomo" else "candidate_output_sha256"
            candidate_case[hash_field] = short_hash
            manifest_item = next(item for item in manifest["cases"] if item["id"] == row["case"]["id"])
            manifest_item["audio_sha256"] = short_hash
            row["audio_sha256"] = short_hash
            row["case"]["audio_sha256"] = short_hash
            row["audio_seconds"] = 7 / 16_000
            write_json(fixture.manifest_path, manifest)
            pilot["asr_new_audio_manifest"] = manifest
            write_json(fixture.pilot_path, pilot)
            new["manifest_sha256"] = file_sha(fixture.manifest_path)
            write_json(fixture.new_path, new)
        elif mutation == "truncated_candidate_output_wav":
            row = new_rows[0]
            case_id = row["case"]["case_ids"][0]
            candidate_case = next(case for case in pilot_cases if case["case_id"] == case_id)
            audio_path = Path(row["case"]["audio_path"])
            raw_wav = audio_path.read_bytes()
            self.assertGreater(len(raw_wav), 48)
            self.assertEqual(raw_wav[36:40], b"data")
            self.assertGreaterEqual(int.from_bytes(raw_wav[40:44], "little"), 2)
            # Keep the data chunk's claimed frame count, but remove a sample and
            # make the outer RIFF length truthful. A decoder must detect that
            # the WAV header promises more PCM bytes than actually exist.
            truncated = raw_wav[:-2]
            truncated = truncated[:4] + (len(truncated) - 8).to_bytes(4, "little") + truncated[8:]
            audio_path.write_bytes(truncated)
            truncated_hash = file_sha(audio_path)
            hash_field = "output_sha256" if kind == "nomo" else "candidate_output_sha256"
            candidate_case[hash_field] = truncated_hash
            manifest_item = next(item for item in manifest["cases"] if item["id"] == row["case"]["id"])
            manifest_item["audio_sha256"] = truncated_hash
            row["audio_sha256"] = truncated_hash
            row["case"]["audio_sha256"] = truncated_hash
            write_json(fixture.manifest_path, manifest)
            pilot["asr_new_audio_manifest"] = manifest
            write_json(fixture.pilot_path, pilot)
            new["manifest_sha256"] = file_sha(fixture.manifest_path)
            write_json(fixture.new_path, new)
        elif mutation == "missing_request_id":
            row = new_rows[0]
            row["response"].pop("request_id", None)
            fixture.mutated_case_id = row["case"]["case_ids"][0]
            write_json(fixture.new_path, new)
        elif mutation == "missing_historical_request_id":
            row = next(row for row in historical_rows if row["case"]["id"] == f"{IDS[0]}-raw")
            row["response"].pop("request_id", None)
            fixture.mutated_case_id = IDS[0]
            write_json(fixture.historical_path, old)
        elif mutation == "cross_report_duplicate_request_id":
            new_rows[0]["response"]["request_id"] = historical_rows[0]["response"]["request_id"]
            write_json(fixture.new_path, new)
        elif mutation == "pending_row_state_with_completed_response":
            row = new_rows[0]
            row["state"] = "pending"
            fixture.mutated_case_id = row["case"]["case_ids"][0]
            write_json(fixture.new_path, new)
        elif mutation == "manifest_case_misbinding":
            manifest = json.loads(fixture.manifest_path.read_text(encoding="utf-8"))
            manifest["cases"][0]["case_ids"] = [IDS[-1]]
            write_json(fixture.manifest_path, manifest)
            pilot["asr_new_audio_manifest"] = manifest
            write_json(fixture.pilot_path, pilot)
            new["manifest_sha256"] = file_sha(fixture.manifest_path)
            write_json(fixture.new_path, new)
        elif mutation in {"pending_with_text", "failed_with_text", "non_string_text", "incomplete_coverage"}:
            # Pick an ASR-generated candidate, rather than Nomo's historical raw passthrough.
            row = next(row for row in new_rows if row["case"]["case_ids"])
            fixture.mutated_case_id = row["case"]["case_ids"][0]
            if mutation == "pending_with_text":
                row["response"]["status"] = "pending"
                row["response"]["error"] = None
                row["response"]["result"]["text"] = "unexecuted transcript must be ignored"
            elif mutation == "failed_with_text":
                row["response"]["status"] = "error"
                row["response"]["error"] = {"code": "fixture_failure"}
                row["response"]["result"]["text"] = "failed transcript must be ignored"
            elif mutation == "non_string_text":
                row["response"]["result"]["text"] = {"bad": "value"}
            else:
                row["response"]["result"]["covered_samples"] -= 1
            write_json(fixture.new_path, new)
        elif mutation == "missing_candidate_row":
            fixture.mutated_case_id = new_rows[0]["case"]["case_ids"][0]
            new_rows.pop(0)
            write_json(fixture.new_path, new)
        elif mutation == "missing_raw_row":
            case_id = "other-only-en-spk17-00601"  # Nomo fixture zeroes this; REAL output is manifest-bound.
            fixture.mutated_case_id = case_id
            old["rows"] = [row for row in historical_rows if row["case"].get("id") != f"{case_id}-raw"]
            write_json(fixture.historical_path, old)
        elif mutation == "missing_g0_row":
            case_id = IDS[0]
            fixture.mutated_case_id = case_id
            old["rows"] = [row for row in historical_rows if row["case"].get("id") != f"{case_id}-g0"]
            write_json(fixture.historical_path, old)
        else:
            raise AssertionError(f"unknown mutation: {mutation}")

        result = fixture.run(output_path=output_path)
        return fixture, result

    def test_identity_mismatches_and_duplicate_identities_reject(self) -> None:
        reject = (
            "duplicate_case_id",
            "wrong_loaded_model",
            "wrong_model_revision",
            "wrong_top_level_revision",
            "wrong_operation",
            "wrong_language_parameter",
            "conflicting_duplicate_hash",
            "duplicate_asr_case_id",
            "duplicate_request_id",
            "wrong_candidate_output_digest",
            "wrong_new_audio_file_digest",
            "case_profile_relabel_7_to_12",
            "wrong_historical_manifest_digest",
            "short_candidate_output_wav",
            "truncated_candidate_output_wav",
            "wrong_manifest_digest",
            "wrong_condition",
            "wrong_protocol",
            "wrong_runtime",
            "cross_report_duplicate_request_id",
            "manifest_case_misbinding",
        )
        for kind in JOINERS:
            for mutation in reject:
                with self.subTest(joiner=kind, mutation=mutation):
                    fixture, result = self._mutated_run(kind, mutation)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn("join rejected:", result.stderr)
                    self.assertFalse(fixture.output_path.exists())

    def test_noncompleted_and_malformed_text_remain_unknown_not_blank(self) -> None:
        expected = {
            "pending_with_text": ("unexecuted", "asr_not_completed"),
            "failed_with_text": ("failed", "asr_failed"),
            "non_string_text": ("invalid", "asr_text_not_bounded_string"),
            "incomplete_coverage": ("incomplete", "asr_not_full_audio_coverage"),
            "missing_candidate_row": ("missing", "missing_candidate_asr_row"),
            "missing_request_id": ("unknown", "asr_request_identity_missing"),
            "missing_model_attestation": ("unknown", "asr_model_or_operation_attestation_missing"),
            "pending_row_state_with_completed_response": ("unexecuted", "asr_row_not_completed"),
        }
        for kind in JOINERS:
            for mutation, (status, error) in expected.items():
                with self.subTest(joiner=kind, mutation=mutation):
                    fixture, result = self._mutated_run(kind, mutation)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    joined = json.loads(fixture.output_path.read_text(encoding="utf-8"))
                    self.assertEqual(joined["case_count"], 18)
                    row_id = fixture.mutated_case_id
                    self.assertIsNotNone(row_id)
                    row = next(item for item in joined["cases"] if item["case_id"] == row_id)
                    self.assertEqual(row["candidate_asr_status"], status)
                    self.assertEqual(row["candidate_asr_error"], error)
                    self.assertFalse(row["candidate_nonempty"])
                    self.assertFalse(row["candidate_blank"])
                    self.assertEqual(joined["role_summary"][row["role"]]["cases"],
                                     sum(role == row["role"] for role in CASE_ROLES.values()))

    def test_missing_historical_request_id_stays_unresolved(self) -> None:
        for kind in JOINERS:
            fixture, result = self._mutated_run(kind, "missing_historical_request_id")
            self.assertEqual(result.returncode, 0, result.stderr)
            joined = json.loads(fixture.output_path.read_text(encoding="utf-8"))
            row = next(item for item in joined["cases"] if item["case_id"] == IDS[0])
            self.assertEqual(row["raw_asr_status"], "unknown")
            self.assertEqual(row["raw_asr_error"], "asr_request_identity_missing")
            self.assertIsNone(row["raw_text"])
            self.assertFalse(row["raw_nonempty"])

    def test_missing_raw_and_g0_rows_are_explicit_and_do_not_drop_cases(self) -> None:
        for kind in JOINERS:
            for mutation, case_id, stage in (
                ("missing_raw_row", "other-only-en-spk17-00601", "raw"),
                ("missing_g0_row", IDS[0], "g0"),
            ):
                with self.subTest(joiner=kind, mutation=mutation):
                    fixture, result = self._mutated_run(kind, mutation)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    joined = json.loads(fixture.output_path.read_text(encoding="utf-8"))
                    self.assertEqual(joined["case_count"], 18)
                    row = next(item for item in joined["cases"] if item["case_id"] == case_id)
                    self.assertEqual(row[f"{stage}_asr_status"], "missing")
                    self.assertEqual(row[f"{stage}_asr_error"], "missing_asr_row")


if __name__ == "__main__":
    unittest.main(verbosity=2)
