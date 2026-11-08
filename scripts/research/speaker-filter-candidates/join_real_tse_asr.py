#!/usr/bin/env python3
"""Strictly join a REAL-TSE pilot to exact, pinned native-Qwen evidence."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from candidate_asr_join_contracts import (
    CASE_LANGUAGES,
    CASE_ROLES,
    JoinContractError,
    QWEN_MODEL_ID,
    QWEN_REVISION,
    baseline_identity,
    candidate_evidence,
    evidence_fields,
    load_candidate_manifest,
    load_probe,
    read_json_bounded,
    summarize_roles,
    summary_status,
    validate_global_request_identity,
    validate_historical_source_chain,
    validate_candidate_model_pin,
    validate_case_manifest,
    write_private_json,
)


def build_summary(pilot_path: Path, historical_path: Path, new_path: Path) -> dict:
    pilot, pilot_path, pilot_digest = read_json_bounded(pilot_path)
    candidate_pin = validate_candidate_model_pin(pilot, "real_tse")
    cases, by_id, prepared_manifest, source_context = validate_case_manifest(pilot, "real_tse", pilot_path)
    historical = load_probe(historical_path, max_rows=200)
    new_probe = load_probe(new_path, max_rows=18)
    if (historical.report["model_id"], historical.report["revision"], historical.report["executable_sha256"]) != (
        new_probe.report["model_id"], new_probe.report["revision"], new_probe.report["executable_sha256"]
    ):
        raise JoinContractError("historical and candidate ASR effective provider identities differ")
    validate_global_request_identity(historical, new_probe)
    historical_source_chain = validate_historical_source_chain(
        pilot, historical_path, historical, source_context, cases)
    manifest_identity, manifest_by_case = load_candidate_manifest(pilot, "real_tse", new_probe, by_id)

    output_cases = []
    for case in cases:
        case_id = case["case_id"]
        raw_row, raw = baseline_identity(
            historical, case_id=case_id, stage="raw", expected_hash=case["_join_raw_hash"],
            role=case["role"], language=case["language"], expected_samples=case["_join_samples"],
        )
        g0_row, g0 = baseline_identity(
            historical, case_id=case_id, stage="g0", expected_hash=case["_join_g0_hash"],
            role=case["role"], language=case["language"], expected_samples=case["_join_samples"],
        )
        candidate = candidate_evidence(case, new_probe, historical, manifest_by_case.get(case_id))
        if manifest_by_case.get(case_id) is not None:
            source = "new_native_probe_exact_manifest_identity"
        elif case["_join_output_audio"].sha256 == case["_join_raw_hash"]:
            source = "historical_same_case_raw_exact_audio"
        elif case["_join_output_audio"].sha256 == case["_join_g0_hash"]:
            source = "historical_same_case_g0_exact_audio"
        else:
            source = "missing_exact_audio_identity"
        output_cases.append({
            "case_id": case_id,
            "role": CASE_ROLES[case_id],
            "soft_level_challenge": case["soft_level_challenge"],
            "language": CASE_LANGUAGES[case_id],
            "profile_id": case["profile_id"],
            "profile_source_speaker_id": case["profile_source_speaker_id"],
            "candidate_output_all_zero": case["candidate_output_all_zero"],
            "prepared_input_audio_sha256": case["_join_prepared_audio"].sha256,
            "model_input_sha256": case["_join_input_audio"].sha256,
            "model_input_path": str(case["_join_input_audio"].path),
            "raw_audio_sha256": case["_join_raw_hash"],
            "g0_audio_sha256": case["_join_g0_hash"],
            "candidate_audio_sha256": case["_join_output_audio"].sha256,
            "candidate_output_path": str(case["_join_output_audio"].path),
            "candidate_output_samples": case["_join_output_audio"].samples,
            "candidate_asr_source": source,
            "case_outcome_retained_on_missing_asr": True,
            **evidence_fields(raw, "raw_asr"),
            **evidence_fields(g0, "g0_asr"),
            **evidence_fields(candidate, "candidate_asr"),
            "raw_text": raw.text if raw.status == "completed" else None,
            "raw_nonempty": bool(raw.status == "completed" and raw.text and raw.text.strip()),
            "g0_text": g0.text if g0.status == "completed" else None,
            "g0_nonempty": bool(g0.status == "completed" and g0.text and g0.text.strip()),
            "candidate_text": candidate.text if candidate.status == "completed" else None,
            "candidate_nonempty": bool(candidate.status == "completed" and candidate.text and candidate.text.strip()),
            "candidate_blank": bool(candidate.status == "completed" and candidate.text is not None and not candidate.text.strip()),
            "candidate_rms_dbfs": case["candidate_output_rms_dbfs"],
            "candidate_output_length_delta_samples": case["model_output_length_delta_samples"],
            "candidate_model_rtf": case["whole_utterance_real_time_factor"],
            "reference": case.get("reference"),
            "reference_project_review_status": case.get("reference_project_review_status", "pending"),
            "historical_raw_case_id": raw_row["case"]["id"] if raw_row else None,
            "historical_g0_case_id": g0_row["case"]["id"] if g0_row else None,
            "candidate_probe_row_id": candidate.row_id,
        })

    roles = summarize_roles(output_cases)
    unresolved = sum(
        row[f"{prefix}_asr_status"] != "completed"
        for row in output_cases for prefix in ("raw", "g0", "candidate")
    )
    return {
        "artifact_type": "voiceflow_real_tse_exact_asr_join",
        "schema_version": 4,
        "status": summary_status(output_cases) if unresolved == 0 else "complete_with_unknown_or_failed_cases",
        "decision_scope": "strict_exact_identity_public_counterexample_discovery_only",
        "quality_use": "nonempty text is not a target-quality pass; references remain pending and all outcomes stay in the canonical denominator",
        "candidate_model": candidate_pin,
        "asr_effective_identity": {
            "model_id": QWEN_MODEL_ID,
            "revision": QWEN_REVISION,
            "executable_sha256": new_probe.report["executable_sha256"],
            "operation": "transcribe",
            "cleanup_result": None,
            "protocol_version": 1,
            "runtime_version": "1.1.0",
            "case_language_parameter": "matched per row to the canonical input language",
        },
        "cloud_calls": 0,
        "microphone_used": False,
        "gui_e2e_verified": False,
        "pilot_report_path": str(pilot_path),
        "pilot_report_sha256": pilot_digest,
        "historical_asr_report_path": str(historical.path),
        "historical_asr_report_sha256": historical.digest,
        "new_asr_report_path": str(new_probe.path),
        "new_asr_report_sha256": new_probe.digest,
        "prepared_manifest": prepared_manifest,
        "source_identity_chain": {
            "prepared_manifest": prepared_manifest,
            "frozen_g0_report": {
                "path": str(source_context["frozen_report_path"]),
                "sha256": source_context["frozen_report_sha256"],
            },
            "frozen_g0_run_header": {
                "path": str(source_context["run_header_path"]),
                "sha256": source_context["run_header_sha256"],
            },
            "historical_asr_manifest": {
                "path": historical_source_chain["manifest_path"],
                "sha256": historical_source_chain["manifest_sha256"],
            },
            "historical_asr_conversion_report": {
                "path": historical_source_chain["conversion_report_path"],
                "sha256": historical_source_chain["conversion_report_sha256"],
            },
            "historical_asr_provenance": {
                "path": historical_source_chain["provenance_path"],
                "sha256": historical_source_chain["provenance_sha256"],
            },
            "historical_source_g0_report": {
                "path": historical_source_chain["source_run_report_path"],
                "sha256": historical_source_chain["source_run_report_sha256"],
            },
            "case_bindings": historical_source_chain["case_bindings"],
        },
        "candidate_asr_manifest": manifest_identity,
        "case_count": len(output_cases),
        "canonical_role_counts": {role: sum(item == role for item in CASE_ROLES.values()) for role in ("other_only", "target_soft_level", "target_mixture")},
        "candidate_outcome_counts": {
            "completed": sum(row["candidate_asr_status"] == "completed" for row in output_cases),
            "nonempty": sum(row["candidate_nonempty"] for row in output_cases),
            "blank": sum(row["candidate_blank"] for row in output_cases),
            "failed_or_missing": sum(row["candidate_asr_status"] in {"failed", "missing"} for row in output_cases),
            "unexecuted": sum(row["candidate_asr_status"] == "unexecuted" for row in output_cases),
            "incomplete": sum(row["candidate_asr_status"] == "incomplete" for row in output_cases),
            "unknown": sum(row["candidate_asr_status"] == "unknown" for row in output_cases),
            "invalid": sum(row["candidate_asr_status"] == "invalid" for row in output_cases),
        },
        "unresolved_asr_stages": unresolved,
        "role_summary": roles,
        "other_only_nonempty": sum(row["role"] == "other_only" and row["candidate_nonempty"] for row in output_cases),
        "other_only_cases": sum(row["role"] == "other_only" for row in output_cases),
        "soft_target_blank": sum(row["role"] == "target_soft_level" and row["candidate_blank"] for row in output_cases),
        "soft_target_cases": sum(row["role"] == "target_soft_level" for row in output_cases),
        "normal_target_mixture_blank": sum(row["role"] == "target_mixture" and row["candidate_blank"] for row in output_cases),
        "normal_target_mixture_cases": sum(row["role"] == "target_mixture" for row in output_cases),
        "target_containing_blank": sum(row["role"] != "other_only" and row["candidate_blank"] for row in output_cases),
        "unreviewed_references": sum(row["reference_project_review_status"] != "reviewed" for row in output_cases),
        "cases": output_cases,
        "limitations": [
            "Only the canonical 12 other-only, three explicit soft-target, and three ordinary target-mixture public-dev cases are joined.",
            "Candidate, raw, and G0 entries require exact same-case identities and actual PCM16 WAV SHA-256 verification.",
            "Only completed ASR with pinned model attestations, complete chunk/sample coverage, and exact string text counts as completed or blank.",
            "Missing, failed, pending, incomplete, malformed-text, or unknown rows stay in the denominator and are not counted as blank output.",
            "Publisher references remain project-review pending; no formal CER/WER or production quality acceptance is established.",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pilot-report", type=Path, required=True)
    parser.add_argument("--historical-asr-results", type=Path, required=True)
    parser.add_argument("--new-asr-results", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        summary = build_summary(args.pilot_report, args.historical_asr_results, args.new_asr_results)
        write_private_json(args.output, summary)
    except (JoinContractError, OSError, TypeError, KeyError, ValueError) as exc:
        parser.error(f"join rejected: {exc}")
    print(json.dumps({"status": summary["status"], "output": str(args.output),
                      "case_count": summary["case_count"], "role_summary": summary["role_summary"],
                      "candidate_outcome_counts": summary["candidate_outcome_counts"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
