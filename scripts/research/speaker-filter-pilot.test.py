#!/usr/bin/env python3
"""Focused tests for the local speaker-filter pilot's trust boundaries."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("speaker-filter-pilot.py")
SPEC = importlib.util.spec_from_file_location("speaker_filter_pilot", MODULE_PATH)
pilot = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(pilot)


def riff_chunk(tag: bytes, body: bytes) -> bytes:
    return tag + struct.pack("<I", len(body)) + body + (b"\0" if len(body) & 1 else b"")


def float_wav(samples, *, extensible: bool = True, channels: int = 1, rate: int = 16000, guid: bytes | None = None) -> bytes:
    payload = struct.pack(f"<{len(samples)}f", *samples)
    if extensible:
        subtype = guid or bytes.fromhex("0300000000001000800000aa00389b71")
        fmt = struct.pack("<HHIIHHHHI", 0xFFFE, channels, rate, rate * channels * 4,
                          channels * 4, 32, 22, 32, 4) + subtype
    else:
        fmt = struct.pack("<HHIIHH", 3, channels, rate, rate * channels * 4,
                          channels * 4, 32)
    body = b"WAVE" + riff_chunk(b"fmt ", fmt) + riff_chunk(b"data", payload)
    return b"RIFF" + struct.pack("<I", len(body)) + body


def private_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)


class FloatWaveReaderTests(unittest.TestCase):
    def test_reads_extensible_ieee_float_samples_without_quantizing(self):
        samples = (0.125, -0.25, 0.33333334)
        info, decoded = pilot.float_wave_from_bytes(float_wav(samples), "extensible test")
        self.assertEqual(info["sample_format"], "IEEE_FLOAT32_EXTENSIBLE")
        expected_f32 = struct.unpack(f"<{len(samples)}f", struct.pack(f"<{len(samples)}f", *samples))
        self.assertEqual(decoded, expected_f32)
        self.assertEqual(info["samples"], len(samples))

    def test_reads_plain_ieee_float_tag(self):
        info, decoded = pilot.float_wave_from_bytes(
            float_wav((0.125, -0.25), extensible=False), "plain float test"
        )
        self.assertEqual(info["sample_format"], "IEEE_FLOAT32")
        self.assertEqual(decoded, (0.125, -0.25))

    def test_rejects_pcm_guid_and_non_float_or_wrong_shape(self):
        pcm_guid = bytes.fromhex("0100000000001000800000aa00389b71")
        cases = (
            (float_wav((0.1,), guid=pcm_guid), "unsupported extensible subtype"),
            (float_wav((0.1,), channels=2), "non-mono float"),
            (float_wav((0.1,), rate=8000), "wrong-rate float"),
        )
        for data, label in cases:
            with self.subTest(label=label), self.assertRaises(pilot.PilotError):
                pilot.float_wave_from_bytes(data, label)

    def test_rejects_nonfinite_samples_and_trailing_bytes(self):
        data = float_wav((float("nan"),))
        with self.assertRaisesRegex(pilot.PilotError, "non-finite"):
            pilot.float_wave_from_bytes(data, "nan sample")
        valid = float_wav((0.1,))
        with self.assertRaises(pilot.PilotError):
            pilot.float_wave_from_bytes(valid + b"x", "trailing byte")


class ProductionPcm16Tests(unittest.TestCase):
    def test_matches_rust_f32_clamp_scale_and_truncate_reference_vectors(self):
        # The first two values are known f64-vs-f32 precision traps from the Rust
        # encoder. The rest exercise clamp endpoints and signed truncation.
        inputs = (
            0.9749137759208679,
            -0.7409588694572449,
            1.0,
            -1.0,
            1.2,
            -1.2,
            0.5,
            -0.5,
            0.00004578,
            -0.00004578,
        )
        self.assertEqual(
            pilot.encode_production_pcm16(inputs),
            (31945, -24279, 32767, -32767, 32767, -32767, 16383, -16383, 1, -1),
        )

    def test_rejects_nonfinite_input_instead_of_emitting_pcm(self):
        with self.assertRaisesRegex(pilot.PilotError, "non-finite"):
            pilot.encode_production_pcm16((0.0, math.inf))

    def test_timing_label_distinguishes_paced_replay_from_unpaced_processing(self):
        paced = pilot.timing_semantics(True)
        unpaced = pilot.timing_semantics(False)
        self.assertIn("includes simulated replay waits", paced["native_real_time_factor"])
        self.assertIn("not compute-only RTF", paced["native_real_time_factor"])
        self.assertIn("unpaced input", unpaced["native_real_time_factor"])
        self.assertIn("excludes model-file loading", unpaced["native_real_time_factor"])
        self.assertIn("session initialization", unpaced["native_real_time_factor"])
        self.assertNotIn("excludes paced", paced["native_real_time_factor"])
        self.assertNotIn("excludes replay", paced["native_real_time_factor"])
        self.assertIn("not an RTF", paced["native_command_wall_elapsed_ms"])


class ManifestPreflightTests(unittest.TestCase):
    def make_minimal_manifest(self, root: Path, *, expected_input_sha: str | None = None):
        samples = (0, 1000, -1000, 32767)
        audio = pilot.wav_bytes(samples)
        input_path = root / "input.wav"
        profile_path = root / "enrollment.wav"
        private_write(input_path, audio)
        private_write(profile_path, audio)
        audio_sha = hashlib.sha256(audio).hexdigest()
        pcm_sha = hashlib.sha256(struct.pack("<4h", *samples)).hexdigest()
        profile = {
            "profile_id": "public-ascend-speaker-7",
            "source_speaker_id": 7,
            "input_audio_path": str(profile_path),
            "input_audio_sha256": audio_sha,
            "input_pcm_sha256": pcm_sha,
            "input_samples": len(samples),
        }
        case = {
            "case_id": "target-en-spk7-00001",
            "role": "target_mixture",
            "language": "en",
            "profile_id": profile["profile_id"],
            "target_speaker_id": 7,
            "mix_recipe": {
                "input_audio_path": str(input_path),
                "input_audio_sha256": expected_input_sha or audio_sha,
                "input_pcm_sha256": pcm_sha,
                "input_samples": len(samples),
            },
        }
        return {
            "artifact_type": "voiceflow_speaker_filter_pilot_cases",
            "schema_version": 1,
            "profiles": [profile],
            "cases": [case],
        }

    def test_source_hash_mismatch_fails_before_models_or_native_helper(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-preflight-") as tmp:
            root = Path(tmp)
            manifest = self.make_minimal_manifest(root, expected_input_sha="0" * 64)
            manifest_path = root / "manifest.json"
            private_write(manifest_path, json.dumps(manifest).encode())
            args = type("Args", (), {
                "manifest": str(manifest_path),
                "output_dir": str(root / "run"),
                "models": str(root / "missing-models"),
                "native_bin": str(root / "missing-native-helper"),
                "recipe": "g0",
                "case_id": None,
                "paced": False,
                "g0_report": None,
                "asr_pair_report": None,
            })()
            with mock.patch.object(pilot, "run_native") as run_native:
                with self.assertRaisesRegex(pilot.PilotError, "case input .* SHA-256 identity mismatch"):
                    pilot.run_command(args)
                run_native.assert_not_called()
            self.assertFalse((root / "run").exists())

    def test_duplicate_or_unsafe_case_ids_fail_preflight(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-invalid-case-") as tmp:
            manifest = self.make_minimal_manifest(Path(tmp))
            manifest["cases"].append(dict(manifest["cases"][0]))
            with self.assertRaisesRegex(pilot.PilotError, "duplicate case_id"):
                pilot.validate_selected_cases(manifest, None)
            manifest["cases"][1]["case_id"] = "../unsafe"
            with self.assertRaisesRegex(pilot.PilotError, "safe case_id"):
                pilot.validate_selected_cases(manifest, None)

    def test_case_budget_is_bounded(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-case-budget-") as tmp:
            manifest = self.make_minimal_manifest(Path(tmp))
            first = manifest["cases"][0]
            manifest["cases"] = [
                {**first, "case_id": f"target-en-spk7-{index:05d}"}
                for index in range(pilot.MAX_CASES + 1)
            ]
            with self.assertRaisesRegex(pilot.PilotError, "pilot case budget exceeded"):
                pilot.validate_selected_cases(manifest, None)

    def test_target_profile_source_speaker_must_match(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-speaker-mismatch-") as tmp:
            manifest = self.make_minimal_manifest(Path(tmp))
            manifest["cases"][0]["target_speaker_id"] = 12
            cases, profiles = pilot.validate_selected_cases(manifest, None)
            with self.assertRaisesRegex(pilot.PilotError, "target/profile speaker identity mismatch"):
                pilot.preflight_pilot_audio(cases, profiles)

    def test_g1_rejects_incomplete_or_ineligible_g0_evidence(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-g1-gate-") as tmp:
            report_path = Path(tmp) / "g0.json"
            report = {
                "artifact_type": "voiceflow_speaker_filter_pilot_report",
                "recipe": "g0",
                "status": "complete",
                "manifest_sha256": "a" * 64,
                "g1_trigger": {
                    "eligible": False,
                    "low_volume_target_effectively_zero_or_truncated": False,
                },
            }
            private_write(report_path, json.dumps(report).encode())
            with self.assertRaisesRegex(pilot.PilotError, "G1 is gated"):
                pilot.g1_precondition(report_path, None, "a" * 64)
            report["artifact_type"] = "unrelated-report"
            report["g1_trigger"]["eligible"] = True
            report["g1_trigger"]["low_volume_target_effectively_zero_or_truncated"] = True
            report_path.write_text(json.dumps(report))
            with self.assertRaisesRegex(pilot.PilotError, "native speaker-filter G0 report"):
                pilot.g1_precondition(report_path, None, "a" * 64)
            report["artifact_type"] = "voiceflow_speaker_filter_pilot_report"
            report["manifest_sha256"] = "b" * 64
            report_path.write_text(json.dumps(report))
            with self.assertRaisesRegex(pilot.PilotError, "G1 G0 report/selected manifest identity mismatch"):
                pilot.g1_precondition(report_path, None, "a" * 64)


class AsrPairIdentityTests(unittest.TestCase):
    def _fixture(self, root: Path, *, result_input_sha: str | None = None):
        input_samples = (0, 16384, -16384, 32767)
        input_data = pilot.wav_bytes(input_samples)
        input_path = root / "raw.wav"
        output_path = root / "filtered-float.wav"
        private_write(input_path, input_data)
        output_data = float_wav(tuple(sample / 32768.0 for sample in input_samples))
        private_write(output_path, output_data)
        input_info, _ = pilot.wave_from_bytes(input_data, str(input_path))
        output_info, _ = pilot.float_wave_from_bytes(output_data, str(output_path))
        manifest = {"cases": [{
            "case_id": "target-en-spk7-00001",
            "role": "target_mixture",
            "language": "en",
            "asr_pair_selection": "short",
            "profile_id": "public-ascend-speaker-7",
            "publisher_reference": "test publisher text",
            "reference_origin": "publisher_manual_asr_annotation",
            "reference_project_review_status": "pending",
            "mix_recipe": {
                "input_audio_path": str(input_path),
                "input_audio_sha256": input_info["audio_sha256"],
                "input_pcm_sha256": input_info["pcm_sha256"],
                "input_samples": input_info["samples"],
            },
        }]}
        result = {
            "case_id": "target-en-spk7-00001",
            "status": "complete",
            "input_sha256": result_input_sha or input_info["audio_sha256"],
            "input_samples": input_info["samples"],
            "output_path": str(output_path),
            "output_sha256": output_info["audio_sha256"],
            "profile_sha256": "a" * 64,
            "native_report": {
                "status": "ok",
                "recipe": "g0",
                "input_sha256": input_info["audio_sha256"],
                "output_sha256": output_info["audio_sha256"],
                "input_samples": input_info["samples"],
                "output_samples": output_info["samples"],
            },
        }
        return manifest, result

    def test_pair_conversion_binds_raw_and_filtered_audio_to_native_receipt(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-asr-pairs-") as tmp:
            root = Path(tmp)
            manifest, result = self._fixture(root)
            artifacts = pilot.create_asr_pair_manifest(root / "pairs", manifest, [result], "g0")
            self.assertEqual(artifacts["pair_count"], 1)
            self.assertEqual(artifacts["audio_row_count"], 2)
            rows = json.loads(Path(artifacts["manifest_path"]).read_text())["cases"]
            self.assertEqual({row["condition"] for row in rows}, {"raw-short", "g0-short"})

    def test_pair_conversion_rejects_a_changed_result_source_identity(self):
        with tempfile.TemporaryDirectory(prefix="speaker-filter-asr-mismatch-") as tmp:
            root = Path(tmp)
            manifest, result = self._fixture(root, result_input_sha="0" * 64)
            with self.assertRaisesRegex(pilot.PilotError, "G0 result input identity mismatch"):
                pilot.create_asr_pair_manifest(root / "pairs", manifest, [result], "g0")


if __name__ == "__main__":
    unittest.main(verbosity=2)
