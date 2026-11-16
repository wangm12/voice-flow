#!/usr/bin/env python3
"""Freeze and analyze VoiceFlow dictionary-learning human pilot evidence.

This is an allocation, build-provenance, and analysis tool, not a dictation or
observation runner. Its explicit build-pre-c1 subcommand runs the pinned local
build and may fetch npm, Rust, Swift, and llama.cpp dependencies. Allocation,
sealing, analysis, and evidence handling do not start VoiceFlow, record audio,
read app state, or call dictation/model services. Human results are accepted
only from a separately maintained local evidence ledger whose stage artifacts
are SHA-256 checked under an explicit artifact root. Empty ledgers remain
pending; fixture ledgers can never pass a human gate.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import os
from pathlib import Path
import plistlib
import platform
import re
import shutil
import tempfile
import stat
import statistics
import subprocess
import sys
from typing import Any


REPO = Path(__file__).resolve().parents[2]
TOOL_VERSION = "1.1.0"
MAX_JSON_BYTES = 16 * 1024 * 1024
SAFE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}\Z")
SHA256 = re.compile(r"[0-9a-f]{64}\Z")
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\Z")

TECHNICAL_PHASE = "pre_c1_legacy_dictionary_learning_technical_v1"
BENEFIT_PHASE = "pre_c1_legacy_dictionary_learning_benefit_v1"
UPSTREAM_TECHNICAL_BINDING_FIELDS = {
    "manifest_fingerprint", "evidence_ledger_sha256", "artifact_bindings_sha256",
    "build_sha256", "build_owner_receipt_sha256", "build_owner_signature_sha256",
    "build_owner_key_fingerprint", "asr_model_id", "asr_model_revision",
}
TECHNICAL_BUILD_IDENTITY_FIELDS = (
    "build_sha256", "build_owner_receipt_sha256", "build_owner_signature_sha256",
    "build_owner_key_fingerprint", "asr_model_id", "asr_model_revision",
)
B_CONFLICT_RESULT_CODE = "manual_pair_scope_conflict"
C1_BOUNDARY = "legacy_pre_c1_a_b_scope_conflict_protocol"
PRE_C1_SOURCE_REVISION = "f4a75a1866f16c8d52932e7de461c1861e7b493d"
PRE_C1_SETTINGS_SCHEMA_VERSION = 27
PRE_C1_HISTORY_SCHEMA_VERSION = 19
PRE_C1_TEST_BUNDLE_IDENTIFIER = "com.voiceflow.installable-test"
PRE_C1_BUILD_RECEIPT_TYPE = "voiceflow_dictionary_learning_pre_c1_build_owner_receipt"
PRE_C1_BUILD_RECEIPT_NAME = "build-owner-receipt.json"
PRE_C1_BUILD_CONFIG = json.dumps(
    {"bundle": {"macOS": {"signingIdentity": "-"}, "targets": ["app"]},
     "identifier": PRE_C1_TEST_BUNDLE_IDENTIFIER},
    separators=(",", ":"), sort_keys=True,
)
PRE_C1_NPM_BUILD_COMMAND = [
    "npm", "run", "tauri", "--", "build", "--bundles", "app",
    "--config", PRE_C1_BUILD_CONFIG, "--ci", "--", "--locked",
]
PRE_C1_BUILD_INPUTS = (
    "package-lock.json",
    "src-tauri/Cargo.lock",
    "src-tauri/tauri.conf.json",
    "src-tauri/llama-runtime/runtime-pin.env",
    "src-tauri/mlx-sidecar/Package.resolved",
    "src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved",
    "src-tauri/mlx-sidecar/sdk-overlay/patch-manifest.json",
    "src-tauri/mlx-sidecar/sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch",
)
TECH_CATEGORIES = (
    ("a_positive", 12),
    ("b_cross_scope", 8),
    ("correct_boundary", 8),
    ("forget_restart", 12),
)
BENEFIT_CATEGORIES = (
    ("target", 20),
    ("safety_cjk_homophone", 3),
    ("safety_latin_case", 2),
    ("safety_other", 5),
    ("ordinary", 10),
)
COMMON_PROVENANCE_ROLES = (
    "task_spec",
    "source_capture",
    "owner_receipt",
    "independent_label",
    "human_attestation",
    "state_before",
    "state_after",
)
OPTIONAL_TERMINAL_ROLE = "target_terminal"
TECHNICAL_TEMPLATE_ROLES = COMMON_PROVENANCE_ROLES + (OPTIONAL_TERMINAL_ROLE,)
BENEFIT_TEMPLATE_ROLES = TECHNICAL_TEMPLATE_ROLES + ("safety_observation",)
BENEFIT_PROVENANCE_ROLES = COMMON_PROVENANCE_ROLES + ("safety_observation",)
AUX_PROVENANCE_ROLES = (
    "owner_receipt",
    "independent_label",
    "state_before",
    "state_after",
)
YES_NO_UNKNOWN = {"yes", "no", "unknown"}


class PilotError(ValueError):
    """Input or protocol error that must not be converted into a pass."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PilotError(message)


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    ).encode("utf-8")


def sha256_json(value: Any) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def object_pairs_no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(path_value: str | Path) -> dict:
    path = Path(path_value)
    require(path.is_absolute(), f"absolute JSON path required: {path}")
    require(path.is_file() and not path.is_symlink(), f"regular JSON file required: {path}")
    require(path.stat().st_size <= MAX_JSON_BYTES, f"JSON byte budget exceeded: {path.name}")
    try:
        payload = path.read_bytes()
        require(len(payload) <= MAX_JSON_BYTES, f"JSON byte budget exceeded: {path.name}")
        value = json.loads(payload, object_pairs_hook=object_pairs_no_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PilotError(f"invalid JSON {path.name}: {error}") from error
    require(isinstance(value, dict), f"JSON object required: {path.name}")
    return value


def parse_json_document(payload: bytes, name: str) -> dict:
    require(len(payload) <= MAX_JSON_BYTES, f"JSON byte budget exceeded: {name}")
    try:
        value = json.loads(payload, object_pairs_hook=object_pairs_no_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PilotError(f"invalid JSON {name}: {error}") from error
    require(isinstance(value, dict), f"JSON object required: {name}")
    return value


def external_path(path_value: str | Path, *, must_exist: bool = False) -> Path:
    path = Path(path_value)
    require(path.is_absolute(), f"absolute external path required: {path}")
    resolved = path.resolve(strict=must_exist)
    require(not resolved.is_relative_to(REPO), "pilot artifacts must stay outside the VoiceFlow repository")
    return resolved


def write_once(path_value: str | Path, value: Any) -> None:
    payload = json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False).encode("utf-8") + b"\n"
    write_once_bytes(path_value, payload)


def write_once_bytes(path_value: str | Path, payload: bytes) -> None:
    path = external_path(path_value)
    require(not path.exists() and not path.is_symlink(), f"output already exists: {path}")
    path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())


def validate_id(value: Any, label: str) -> str:
    require(isinstance(value, str) and SAFE_ID.fullmatch(value) is not None, f"safe {label} required")
    return value


def validate_sha256(value: Any, label: str) -> str:
    require(isinstance(value, str) and SHA256.fullmatch(value) is not None, f"{label} SHA-256 required")
    return value


def ranked(seed: str, label: str) -> str:
    return hashlib.sha256(f"{seed}\0{label}".encode("utf-8")).hexdigest()


def verify_fingerprint(document: dict, field: str) -> str:
    expected = document.get(field)
    require(isinstance(expected, str) and SHA256.fullmatch(expected) is not None,
            f"{field} is missing or invalid")
    body = {key: value for key, value in document.items() if key != field}
    actual = sha256_json(body)
    require(actual == expected, f"{field} does not match document contents")
    return expected


def technical_assignments(seed: str, operator_a: str, operator_b: str) -> list[dict]:
    per_operator: dict[str, list[dict]] = {operator_a: [], operator_b: []}
    counter = 1
    for category, count in TECH_CATEGORIES:
        rows = [{"trial_id": f"T{counter + offset:03d}", "category": category} for offset in range(count)]
        counter += count
        rows.sort(key=lambda row: ranked(seed, f"technical\0{category}\0{row['trial_id']}"))
        midpoint = count // 2
        for operator_id, original_subset in ((operator_a, rows[:midpoint]), (operator_b, rows[midpoint:])):
            subset = list(original_subset)
            if category == "forget_restart":
                subset.sort(key=lambda row: ranked(seed, f"forget-stage\0{operator_id}\0{row['trial_id']}"))
            for index, row in enumerate(subset):
                assignment = dict(row)
                assignment.update({
                    "operator_id": operator_id,
                    "participant_id": None,
                    "arm": "technical",
                    "substage": None,
                    "family_id": None,
                    "matched_set": None,
                    "state_contract": "no_rule_or_dictionary_mutation",
                })
                if category == "forget_restart":
                    assignment["substage"] = "after_forget" if index < len(subset) // 2 else "after_restart"
                    assignment["state_contract"] = "forgotten_rule_remains_absent"
                per_operator[operator_id].append(assignment)

    result = []
    for operator_id in (operator_a, operator_b):
        rows = per_operator[operator_id]
        # The irreversible Forget and Restart boundary must be completed in
        # order by each operator. Randomize within each phase, never across it.
        rows.sort(key=lambda row: (
            {None: 0, "after_forget": 1, "after_restart": 2}[row["substage"]],
            ranked(seed, f"technical-order\0{operator_id}\0{row['trial_id']}"),
        ))
        for order, row in enumerate(rows, start=1):
            result.append({**row, "operator_order": order})
    return sorted(result, key=lambda row: row["trial_id"])


def technical_auxiliary(seed: str, operator_a: str, operator_b: str) -> list[dict]:
    rows = []
    for check_type, count in (("ime_composition", 5), ("focus_switch", 5)):
        candidates = [operator_a, operator_b]
        operator_load = Counter()
        for index in range(1, count + 1):
            check_id = f"{check_type}-{index:02d}"
            operator_id = min(
                candidates,
                key=lambda candidate: (
                    operator_load[candidate],
                    ranked(seed, f"aux\0{check_type}\0{check_id}\0{candidate}"),
                ),
            )
            operator_load[operator_id] += 1
            rows.append({
                "check_id": check_id,
                "check_type": check_type,
                "operator_id": operator_id,
            })
    return rows


def benefit_assignments(seed: str, participants: list[str], families: list[dict]) -> list[dict]:
    families_by_set: dict[str, list[str]] = defaultdict(list)
    for item in families:
        families_by_set[item["matched_set"]].append(item["family_id"])
    assignments = []
    for participant_id in participants:
        family_arms = {}
        for matched_set, pair in families_by_set.items():
            teach_family = min(pair, key=lambda family: ranked(seed, f"benefit-arm\0{participant_id}\0{matched_set}\0{family}"))
            for family in pair:
                family_arms[family] = "teach" if family == teach_family else "control"

        participant_rows = []
        counter = 1
        for family_entry in families:
            family = family_entry["family_id"]
            matched_set = family_entry["matched_set"]
            for _ in range(2):
                participant_rows.append({
                    "trial_id": f"{participant_id}-T{counter:03d}",
                    "category": "target",
                    "arm": family_arms[family],
                    "family_id": family,
                    "matched_set": matched_set,
                    "state_contract": "frozen_arm_state_unchanged",
                })
                counter += 1
        for category, count in BENEFIT_CATEGORIES[1:]:
            for _ in range(count):
                participant_rows.append({
                    "trial_id": f"{participant_id}-T{counter:03d}",
                    "category": category,
                    "arm": "all",
                    "family_id": None,
                    "matched_set": None,
                    "state_contract": "frozen_arm_state_unchanged",
                })
                counter += 1
        participant_rows.sort(key=lambda row: ranked(seed, f"benefit-order\0{participant_id}\0{row['trial_id']}"))
        for order, row in enumerate(participant_rows, start=1):
            assignments.append({
                **row,
                "participant_id": participant_id,
                "operator_id": None,
                "participant_order": order,
                "substage": None,
            })
    return sorted(assignments, key=lambda row: row["trial_id"])


def load_family_catalog(path_value: str | Path) -> tuple[list[dict], str]:
    external_path(path_value, must_exist=True)
    catalog = read_json(path_value)
    require(catalog.get("artifact_type") == "voiceflow_dictionary_learning_family_catalog",
            "family catalog artifact type mismatch")
    require(catalog.get("schema_version") == 1, "family catalog schema_version unsupported")
    require(set(catalog) == {"artifact_type", "schema_version", "families"}, "family catalog fields invalid")
    families = catalog.get("families")
    require(isinstance(families, list) and len(families) == 10,
            "family catalog must bind ten actual opaque families before randomization")
    ids = []
    matched_sets: dict[str, list[str]] = defaultdict(list)
    normalized = []
    for item in families:
        require(isinstance(item, dict) and set(item) == {"family_id", "matched_set"},
                "family catalog rows require only family_id and matched_set")
        family_id = validate_id(item.get("family_id"), "opaque family_id")
        matched_set = validate_id(item.get("matched_set"), "opaque matched_set")
        ids.append(family_id)
        matched_sets[matched_set].append(family_id)
        normalized.append({"family_id": family_id, "matched_set": matched_set})
    require(len(set(ids)) == 10, "family catalog family IDs must be unique")
    require(len(matched_sets) == 5 and all(len(pair) == 2 for pair in matched_sets.values()),
            "family catalog must freeze five matched sets of two families")
    require(all(not family_id.startswith("WF") for family_id in ids),
            "generated placeholder family IDs are not actual opaque families")
    return normalized, sha256_json(catalog)


def validate_upstream_technical(
    report_path: str | Path,
    manifest_path: str | Path,
    evidence_path: str | Path,
    artifact_root: str | Path,
    build_owner_public_key: str | Path | None = None,
) -> tuple[dict, dict]:
    """Regenerate a technical report from its actual frozen inputs and artifacts."""
    for path in (report_path, manifest_path, evidence_path, artifact_root):
        external_path(path, must_exist=True)
    report = read_json(report_path)
    require(report.get("stage") == "technical", "upstream report must be a technical-stage report")
    close_mode = report.get("close_mode")
    require(close_mode in {"open", "closed"}, "technical report must declare its evidence close mode")
    rebuilt = analyze(type("Args", (), {
        "manifest": str(manifest_path),
        "evidence": str(evidence_path),
        "artifact_root": str(artifact_root),
        "build_owner_public_key": str(build_owner_public_key) if build_owner_public_key is not None else None,
        "close": close_mode == "closed",
    })())
    require(canonical_json(report) == canonical_json(rebuilt),
            "technical report does not match re-analysis of the bound manifest, ledger and local artifacts")
    require(rebuilt.get("stage") == "technical" and rebuilt.get("status") == "pass"
            and rebuilt.get("evidence_class") == "actual_human"
            and rebuilt.get("human_gate_status") == "pass",
            "benefit allocation requires passing actual-human technical evidence")
    technical_manifest = read_json(manifest_path)
    manifest_fingerprint(technical_manifest)
    require(technical_manifest.get("stage") == "technical"
            and technical_manifest.get("freeze_status") == "frozen",
            "benefit allocation requires a frozen technical manifest")
    technical_facts = technical_manifest.get("freeze_facts")
    require(isinstance(technical_facts, dict), "technical frozen build/model identity missing")
    binding = {
        "manifest_fingerprint": rebuilt["manifest_fingerprint"],
        "evidence_ledger_sha256": rebuilt["evidence_ledger_sha256"],
        "artifact_bindings_sha256": rebuilt["artifact_bindings_sha256"],
    }
    require(binding["manifest_fingerprint"] == manifest_fingerprint(technical_manifest),
            "technical manifest changed while verifying its upstream report")
    for key in TECHNICAL_BUILD_IDENTITY_FIELDS:
        value = technical_facts.get(key)
        if key in {"build_sha256", "build_owner_receipt_sha256", "build_owner_signature_sha256"}:
            validate_sha256(value, f"technical {key}")
        else:
            require(isinstance(value, str) and value.strip(), f"technical {key} required")
        binding[key] = value
    return rebuilt, binding


def validate_benefit_build_identity(facts: dict, upstream_binding: dict) -> None:
    """Require benefit freeze facts to reuse the technical build and ASR model."""
    for key in TECHNICAL_BUILD_IDENTITY_FIELDS:
        require(facts.get(key) == upstream_binding.get(key),
                f"benefit frozen {key} must match the signed technical build/model identity")


def make_technical_draft(args: argparse.Namespace) -> dict:
    batch_id = validate_id(args.batch_id, "batch_id")
    operators = [validate_id(value, "operator_id") for value in args.operator]
    require(len(operators) == 2 and len(set(operators)) == 2, "technical allocation requires exactly two distinct --operator IDs")
    non_implementer = validate_id(args.non_implementer, "non_implementer")
    require(non_implementer in operators, "--non-implementer must name one of the two operators")
    seed = args.seed
    require(isinstance(seed, str) and seed.strip(), "non-empty reproducibility seed required")
    roster = [
        {"operator_id": operator_id,
         "implementation_role": "non_implementer" if operator_id == non_implementer else "implementer"}
        for operator_id in operators
    ]
    body = {
        "artifact_type": "voiceflow_dictionary_learning_pilot_allocation",
        "schema_version": 1,
        "tool_version": TOOL_VERSION,
        "stage": "technical",
        "phase_id": TECHNICAL_PHASE,
        "c1_boundary": C1_BOUNDARY,
        "batch_id": batch_id,
        "seed": seed,
        "freeze_status": "draft",
        "roster": roster,
        "assignments": technical_assignments(seed, operators[0], operators[1]),
        "auxiliary_observations": technical_auxiliary(seed, operators[0], operators[1]),
        "freeze_facts": None,
        "task_catalog_sha256": None,
        "task_specs": None,
        "upstream_technical_report_sha256": None,
        "upstream_technical_binding": None,
    }
    body["allocation_fingerprint"] = sha256_json(body)
    return body


def make_benefit_draft(args: argparse.Namespace) -> dict:
    batch_id = validate_id(args.batch_id, "pilot_id")
    participants = [validate_id(value, "participant_id") for value in args.participant]
    require(4 <= len(participants) <= 6, "benefit pilot requires 4–6 distinct pseudonymous participant IDs")
    require(len(set(participants)) == len(participants), "duplicate participant_id")
    seed = args.seed
    require(isinstance(seed, str) and seed.strip(), "non-empty reproducibility seed required")
    families, family_catalog_sha = load_family_catalog(args.family_catalog)
    technical_report, technical_binding = validate_upstream_technical(
        args.technical_report, args.technical_manifest, args.technical_evidence, args.artifact_root,
        args.build_owner_public_key,
    )
    body = {
        "artifact_type": "voiceflow_dictionary_learning_pilot_allocation",
        "schema_version": 1,
        "tool_version": TOOL_VERSION,
        "stage": "benefit",
        "phase_id": BENEFIT_PHASE,
        "c1_boundary": C1_BOUNDARY,
        "batch_id": batch_id,
        "seed": seed,
        "freeze_status": "draft",
        "participants": participants,
        "family_catalog": families,
        "family_catalog_sha256": family_catalog_sha,
        "assignments": benefit_assignments(seed, participants, families),
        "auxiliary_observations": [],
        "freeze_facts": None,
        "task_catalog_sha256": None,
        "task_specs": None,
        "upstream_technical_report_sha256": sha256_json(technical_report),
        "upstream_technical_binding": technical_binding,
    }
    body["allocation_fingerprint"] = sha256_json(body)
    return body


def validate_allocation(document: dict) -> str:
    require(document.get("artifact_type") == "voiceflow_dictionary_learning_pilot_allocation", "allocation artifact type mismatch")
    require(document.get("schema_version") == 1, "allocation schema_version unsupported")
    require(document.get("stage") in {"technical", "benefit"}, "allocation stage invalid")
    require(document.get("phase_id") == (TECHNICAL_PHASE if document["stage"] == "technical" else BENEFIT_PHASE),
            "allocation phase_id does not match stage")
    require(document.get("c1_boundary") == C1_BOUNDARY,
            "this workflow covers the legacy pre-C1 acceptance only")
    require(document.get("freeze_status") in {"draft", "frozen"}, "allocation freeze_status invalid")
    common = {
        "artifact_type", "schema_version", "tool_version", "stage", "phase_id", "c1_boundary",
        "batch_id", "seed", "freeze_status", "assignments", "auxiliary_observations",
        "freeze_facts", "task_catalog_sha256", "task_specs", "upstream_technical_report_sha256",
        "upstream_technical_binding", "allocation_fingerprint",
    }
    expected_fields = common | ({"roster"} if document["stage"] == "technical" else {
        "participants", "family_catalog", "family_catalog_sha256",
    })
    if document["freeze_status"] == "frozen":
        expected_fields.add("freeze_fingerprint")
    require(set(document) == expected_fields, "allocation fields do not match the versioned schema")
    validate_id(document.get("batch_id"), "batch_id")
    require(isinstance(document.get("seed"), str) and document["seed"].strip(), "allocation seed required")
    rows = document.get("assignments")
    require(isinstance(rows, list), "allocation assignments list required")
    trial_ids = [row.get("trial_id") for row in rows if isinstance(row, dict)]
    require(len(trial_ids) == len(rows) and len(set(trial_ids)) == len(rows), "allocation trial IDs must be unique")
    for trial_id in trial_ids:
        validate_id(trial_id, "trial_id")
    if document["stage"] == "technical":
        roster = document.get("roster")
        require(isinstance(roster, list) and len(roster) == 2, "technical allocation requires two operator declarations")
        require(all(isinstance(row, dict) and set(row) == {"operator_id", "implementation_role"} for row in roster),
                "technical roster declarations have an invalid shape")
        operator_ids = [row.get("operator_id") for row in roster if isinstance(row, dict)]
        require(len(operator_ids) == 2 and len(set(operator_ids)) == 2, "two distinct operator IDs required")
        require(all(validate_id(value, "operator_id") == value for value in operator_ids), "operator IDs invalid")
        require(all(row.get("implementation_role") in {"implementer", "non_implementer"} for row in roster),
                "operator implementation role declaration required")
        require(sum(row["implementation_role"] == "non_implementer" for row in roster) >= 1,
                "at least one operator must be declared a non-implementer")
        counts = Counter(row.get("category") for row in rows)
        require(counts == dict(TECH_CATEGORIES), "technical category allocation must remain 12/8/8/12")
        per_operator = Counter(row.get("operator_id") for row in rows)
        require(per_operator == {operator_ids[0]: 20, operator_ids[1]: 20}, "each technical operator must receive 20 trials")
        require(all(row.get("state_contract") == ("forgotten_rule_remains_absent" if row.get("category") == "forget_restart"
                                                     else "no_rule_or_dictionary_mutation") for row in rows),
                "technical state-transition contracts must match the assigned task category")
        require(all(row.get("substage") is None for row in rows if row.get("category") != "forget_restart"),
                "only forget/restart trials may have a substage")
        for category, count in TECH_CATEGORIES:
            subset = [row for row in rows if row.get("category") == category]
            split = Counter(row.get("operator_id") for row in subset)
            require(split == {operator_ids[0]: count // 2, operator_ids[1]: count // 2},
                    f"{category} must be evenly divided between operators")
        forget_stages = Counter(row.get("substage") for row in rows if row.get("category") == "forget_restart")
        require(forget_stages == {"after_forget": 6, "after_restart": 6}, "forget/restart allocation must be split 6/6")
        for operator_id in operator_ids:
            ordered = sorted((row for row in rows if row.get("operator_id") == operator_id), key=lambda row: row.get("operator_order", 0))
            require([row.get("operator_order") for row in ordered] == list(range(1, 21)),
                    f"{operator_id} operator_order must be unique and complete")
            stages = [{None: 0, "after_forget": 1, "after_restart": 2}.get(row.get("substage"), -1) for row in ordered]
            require(stages == sorted(stages), f"{operator_id} must complete after-forget tasks before restart tasks")
            require(Counter(row.get("substage") for row in ordered if row.get("category") == "forget_restart")
                    == {"after_forget": 3, "after_restart": 3},
                    f"{operator_id} must have three tasks on each side of its Forget/Restart boundary")
        aux = document.get("auxiliary_observations")
        require(isinstance(aux, list) and len(aux) == 10, "five IME and five focus-switch checks required")
        auxiliary_ids = [row.get("check_id") for row in aux if isinstance(row, dict)]
        require(len(auxiliary_ids) == len(aux) and len(set(auxiliary_ids)) == 10,
                "auxiliary check IDs must be present and unique")
        require(Counter(row.get("check_type") for row in aux) == {"ime_composition": 5, "focus_switch": 5},
                "auxiliary checks must be five IME and five focus switches")
        require(rows == technical_assignments(document["seed"], operator_ids[0], operator_ids[1]),
                "technical assignment rows do not match the frozen seed and operator roster")
        require(aux == technical_auxiliary(document["seed"], operator_ids[0], operator_ids[1]),
                "auxiliary checks do not match the frozen seed and operator roster")
    else:
        participants = document.get("participants")
        require(isinstance(participants, list) and 4 <= len(participants) <= 6, "benefit allocation requires 4–6 participants")
        require(all(validate_id(value, "participant_id") == value for value in participants), "participant IDs invalid")
        require(len(set(participants)) == len(participants), "participant IDs must be unique")
        require(document.get("auxiliary_observations") == [], "benefit allocation has no auxiliary observations")
        require(validate_sha256(document.get("upstream_technical_report_sha256"), "passing technical report"), "passing technical report SHA-256 required")
        require(isinstance(document.get("upstream_technical_binding"), dict)
                and set(document["upstream_technical_binding"]) == UPSTREAM_TECHNICAL_BINDING_FIELDS,
                "bound technical input and build/model identities required")
        for key, value in document["upstream_technical_binding"].items():
            if key in {"asr_model_id", "asr_model_revision", "build_owner_key_fingerprint"}:
                require(isinstance(value, str) and value.strip(), f"upstream technical {key} required")
            else:
                validate_sha256(value, f"upstream technical {key}")
        family_catalog = document.get("family_catalog")
        require(isinstance(family_catalog, list) and len(family_catalog) == 10,
                "benefit allocation must retain ten pre-draw opaque families")
        family_ids = [validate_id(item.get("family_id"), "family_id") for item in family_catalog if isinstance(item, dict)]
        require(len(family_ids) == 10 and len(set(family_ids)) == 10, "benefit family IDs must be unique")
        require(validate_sha256(document.get("family_catalog_sha256"), "family catalog"), "family catalog fingerprint required")
        require(document["family_catalog_sha256"] == sha256_json({
            "artifact_type": "voiceflow_dictionary_learning_family_catalog",
            "schema_version": 1,
            "families": family_catalog,
        }), "family catalog fingerprint does not match the pre-draw family identities")
        matched_sets: dict[str, list[str]] = defaultdict(list)
        for item in family_catalog:
            require(isinstance(item, dict) and set(item) == {"family_id", "matched_set"}, "family catalog row invalid")
            matched_sets[validate_id(item["matched_set"], "matched_set")].append(item["family_id"])
        require(len(matched_sets) == 5 and all(len(pair) == 2 for pair in matched_sets.values()),
                "benefit family catalog requires five matched sets of two")
        require(all(not family_id.startswith("WF") for family_id in family_ids), "placeholder families cannot be randomized")
        family_names = {item["family_id"] for item in family_catalog}
        require(len(rows) == len(participants) * 40,
                "benefit allocation must retain exactly 40 rows per participant")
        for participant_id in participants:
            person_rows = [row for row in rows if row.get("participant_id") == participant_id]
            require(len(person_rows) == 40, f"{participant_id} must retain all 40 allocated tasks")
            require(all(row.get("state_contract") == "frozen_arm_state_unchanged" for row in person_rows),
                    f"{participant_id} all tasks must bind the frozen-arm state contract")
            counts = Counter(row.get("category") for row in person_rows)
            require(counts == dict(BENEFIT_CATEGORIES), f"{participant_id} task counts must remain 20/10/10")
            target = [row for row in person_rows if row.get("category") == "target"]
            family_arms = defaultdict(set)
            family_sets = {}
            for row in target:
                family_arms[row.get("family_id")].add(row.get("arm"))
                family_sets[row.get("family_id")] = row.get("matched_set")
            require(set(family_arms) == family_names, f"{participant_id} must retain all ten target families")
            require(all(len(values) == 1 for values in family_arms.values()), f"{participant_id} target family split across arms")
            require(all(len([row for row in target if row.get("family_id") == family]) == 2 for family in family_names),
                    f"{participant_id} each target family must have two matched tasks")
            require(Counter(next(iter(values)) for values in family_arms.values()) == {"teach": 5, "control": 5},
                    f"{participant_id} needs five Teach and five control families")
            for matched_set, pair in matched_sets.items():
                require(all(family_sets[family] == matched_set for family in pair),
                        f"{participant_id} matched family set identity invalid")
                require({next(iter(family_arms[family])) for family in pair} == {"teach", "control"},
                        f"{participant_id} matched set must contain one Teach and one control family")
        require(rows == benefit_assignments(document["seed"], participants, family_catalog),
                "benefit assignment rows do not match the frozen seed, roster and pre-draw family catalog")
        if document["freeze_status"] == "draft":
            require(document.get("upstream_technical_report_sha256") is not None, "benefit allocation must name a passing technical report")
    if document["stage"] == "technical":
        require(document.get("upstream_technical_binding") is None, "technical stage has no upstream technical binding")

    if document["freeze_status"] == "draft":
        require(document.get("freeze_facts") is None and document.get("task_catalog_sha256") is None
                and document.get("task_specs") is None, "draft must not contain sealed facts")
        return verify_fingerprint(document, "allocation_fingerprint")

    draft_body = {key: document[key] for key in expected_fields if key not in {"freeze_fingerprint", "allocation_fingerprint"}}
    draft_body.update({"freeze_status": "draft", "freeze_facts": None, "task_catalog_sha256": None, "task_specs": None})
    actual_allocation_fingerprint = sha256_json(draft_body)
    require(document.get("allocation_fingerprint") == actual_allocation_fingerprint,
            "allocation fingerprint does not match the frozen assignment payload")
    expected_freeze_fingerprint = sha256_json({
        key: value for key, value in document.items() if key != "freeze_fingerprint"
    })
    require(document.get("freeze_fingerprint") == expected_freeze_fingerprint,
            "freeze fingerprint does not match frozen system and state facts")
    return actual_allocation_fingerprint


def expected_catalog_rows(manifest: dict) -> dict[str, dict]:
    result = {}
    for assignment in manifest["assignments"]:
        row = {
            "trial_id": assignment["trial_id"],
            "category": assignment["category"],
            "task_spec_sha256": None,
        }
        if manifest["stage"] == "benefit":
            row["family_id"] = assignment["family_id"]
            row["matched_set"] = assignment["matched_set"]
        result[assignment["trial_id"]] = row
    return result


def validate_task_catalog(manifest: dict, catalog: dict) -> tuple[str, dict[str, str]]:
    require(catalog.get("artifact_type") == "voiceflow_dictionary_learning_task_catalog", "task catalog artifact type mismatch")
    require(catalog.get("schema_version") == 1, "task catalog schema_version unsupported")
    require(catalog.get("stage") == manifest["stage"], "task catalog stage mismatch")
    tasks = catalog.get("tasks")
    require(isinstance(tasks, list), "task catalog tasks list required")
    expected = expected_catalog_rows(manifest)
    received: dict[str, dict] = {}
    for task in tasks:
        require(isinstance(task, dict), "task catalog row object required")
        trial_id = validate_id(task.get("trial_id"), "catalog trial_id")
        require(trial_id not in received, f"duplicate task catalog trial_id: {trial_id}")
        expected_fields = {"trial_id", "category", "task_spec_sha256"}
        if manifest["stage"] == "benefit":
            expected_fields |= {"family_id", "matched_set"}
        require(set(task) == expected_fields, f"task catalog fields invalid for {trial_id}")
        validate_sha256(task.get("task_spec_sha256"), f"task {trial_id} spec")
        received[trial_id] = task
    require(set(received) == set(expected), "task catalog trial IDs must exactly match allocation")
    task_specs = {}
    for trial_id, expected_row in expected.items():
        actual = received[trial_id]
        for key, value in expected_row.items():
            if key == "task_spec_sha256":
                continue
            require(actual.get(key) == value, f"task catalog {key} mismatch for {trial_id}")
        task_specs[trial_id] = actual["task_spec_sha256"]
    return sha256_json(catalog), task_specs


def validate_freeze_facts(
    manifest: dict, facts: dict, build_owner_public_key: str | Path | None = None,
) -> dict:
    required = {
        "artifact_type", "schema_version", "build_sha256", "asr_route", "asr_model_id",
        "asr_model_revision", "cleanup_mode", "term_injection_enabled", "fuzzy_matching_enabled",
        "permissions_sha256", "source_revision", "settings_schema_version",
        "history_schema_version", "test_bundle_identifier", "source_checkout_path", "test_app_path",
        "build_owner_receipt_path", "build_owner_receipt_sha256", "build_owner_signature_sha256",
        "build_owner_key_fingerprint",
    }
    if manifest["stage"] == "technical":
        required |= {"initial_state_sha256"}
    else:
        required |= {
            "state_sha256_by_participant",
            "effective_hint_sha256_by_participant",
            "active_rule_families_by_participant",
            "passive_learning_disabled_by_participant",
        }
    require(set(facts) == required, "freeze facts fields do not match the stage contract")
    require(facts.get("artifact_type") == "voiceflow_dictionary_learning_freeze_facts", "freeze facts artifact type mismatch")
    require(facts.get("schema_version") == 2, "freeze facts schema_version unsupported")
    for key in ("build_sha256", "permissions_sha256"):
        validate_sha256(facts.get(key), key)
    require(facts.get("source_revision") == PRE_C1_SOURCE_REVISION,
            "phase requires the pinned pre-C1 source revision")
    require(facts.get("settings_schema_version") == PRE_C1_SETTINGS_SCHEMA_VERSION,
            "phase requires the pre-C1 Settings schema")
    require(facts.get("history_schema_version") == PRE_C1_HISTORY_SCHEMA_VERSION,
            "phase requires the pre-C1 History schema")
    require(facts.get("test_bundle_identifier") == PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "phase requires the isolated Installable Test bundle identifier")
    require(facts.get("asr_route") == "on_device", "phase requires the frozen on-device ASR route")
    require(isinstance(facts.get("asr_model_id"), str) and facts["asr_model_id"].strip(), "asr_model_id required")
    require(isinstance(facts.get("asr_model_revision"), str) and facts["asr_model_revision"].strip(), "asr_model_revision required")
    require(facts.get("cleanup_mode") == "off", "phase requires cleanup_mode=off")
    require(facts.get("term_injection_enabled") is False, "phase requires term injection disabled")
    require(facts.get("fuzzy_matching_enabled") is False, "phase requires fuzzy matching disabled")
    if manifest["stage"] == "technical":
        validate_sha256(facts.get("initial_state_sha256"), "initial_state")
    else:
        participants = manifest["participants"]
        states = facts.get("state_sha256_by_participant")
        hints = facts.get("effective_hint_sha256_by_participant")
        active = facts.get("active_rule_families_by_participant")
        passive = facts.get("passive_learning_disabled_by_participant")
        require(isinstance(states, dict) and set(states) == set(participants), "participant state fingerprints must cover every participant exactly")
        require(isinstance(hints, dict) and set(hints) == set(participants), "participant effective-hint fingerprints must cover every participant exactly")
        require(isinstance(active, dict) and set(active) == set(participants), "active rule family attestations must cover every participant exactly")
        require(isinstance(passive, dict) and set(passive) == set(participants), "passive-learning attestations must cover every participant exactly")
        for participant_id in participants:
            validate_sha256(states[participant_id], f"{participant_id} state")
            validate_sha256(hints[participant_id], f"{participant_id} effective hint")
            assignment_families = defaultdict(set)
            for row in manifest["assignments"]:
                if row["participant_id"] == participant_id and row["category"] == "target":
                    assignment_families[row["arm"]].add(row["family_id"])
            expected_teach = sorted(assignment_families["teach"])
            expected_control = sorted(assignment_families["control"])
            require(isinstance(active[participant_id], list)
                    and len(active[participant_id]) == 5
                    and all(isinstance(value, str) and SAFE_ID.fullmatch(value) is not None
                            for value in active[participant_id])
                    and len(set(active[participant_id])) == 5,
                    f"{participant_id} must have five unique active Teach families")
            require(sorted(active[participant_id]) == expected_teach,
                    f"{participant_id} active rules must be exactly its assigned Teach families")
            require(passive[participant_id] is True,
                    f"{participant_id} passive learning must be disabled for the frozen trial")
            require(len(expected_teach) == 5 and len(expected_control) == 5,
                    f"{participant_id} must have five Teach and five control families")
    for key in ("build_owner_receipt_sha256", "build_owner_signature_sha256"):
        validate_sha256(facts.get(key), key)
    require(isinstance(facts.get("build_owner_key_fingerprint"), str)
            and facts["build_owner_key_fingerprint"].startswith("SHA256:"),
            "build_owner_key_fingerprint required")
    validate_pre_c1_build(facts, build_owner_public_key)
    return facts


def pre_c1_source_input_hashes(source_checkout: Path) -> dict[str, str]:
    hashes = {}
    for relative in PRE_C1_BUILD_INPUTS:
        path = source_checkout / relative
        require(path.is_file() and not path.is_symlink(), f"pinned build input is missing: {relative}")
        hashes[relative] = sha256_file(path)
    return hashes


def is_macho_executable(path: Path) -> bool:
    if not path.is_file() or path.is_symlink() or stat.S_IMODE(path.stat().st_mode) & 0o111 == 0:
        return False
    try:
        file_size = path.stat().st_size
        with path.open("rb") as stream:
            magic = stream.read(4)

            def execute_slice(offset: int, size: int | None = None) -> bool:
                if offset < 0 or (size is not None and size < 16) or offset + 16 > file_size:
                    return False
                stream.seek(offset)
                header = stream.read(16)
                if len(header) != 16:
                    return False
                slice_magic = header[:4]
                if slice_magic in {b"\xce\xfa\xed\xfe", b"\xcf\xfa\xed\xfe"}:
                    order = "little"
                elif slice_magic in {b"\xfe\xed\xfa\xce", b"\xfe\xed\xfa\xcf"}:
                    order = "big"
                else:
                    return False
                cpu_type = int.from_bytes(header[4:8], order)
                file_type = int.from_bytes(header[12:16], order)
                return cpu_type == 0x0100000C and file_type == 2

            if execute_slice(0):
                return True
            fat_specs = {
                b"\xca\xfe\xba\xbe": ("big", 20, False), b"\xca\xfe\xba\xbf": ("big", 32, True),
                b"\xbe\xba\xfe\xca": ("little", 20, False), b"\xbf\xba\xfe\xca": ("little", 32, True),
            }
            if magic not in fat_specs:
                return False
            order, entry_size, fat64 = fat_specs[magic]
            stream.seek(4)
            count_bytes = stream.read(4)
            if len(count_bytes) != 4:
                return False
            count = int.from_bytes(count_bytes, order)
            if count == 0 or count > 64 or 8 + count * entry_size > file_size:
                return False
            table = stream.read(count * entry_size)
            if len(table) != count * entry_size:
                return False
            for index in range(count):
                start = index * entry_size
                cpu_type = int.from_bytes(table[start:start + 4], order)
                offset_end = start + (16 if fat64 else 12)
                size_start = start + (16 if fat64 else 12)
                offset = int.from_bytes(table[start + 8:offset_end], order)
                size = int.from_bytes(table[size_start:size_start + (8 if fat64 else 4)], order)
                if cpu_type == 0x0100000C and offset + size <= file_size and execute_slice(offset, size):
                    return True
    except OSError:
        return False
    return False


def bundle_inventory_sha256(bundle: Path) -> str:
    resolved_bundle = bundle.resolve(strict=True)
    inventory = []
    for current, directories, files in os.walk(bundle, followlinks=False):
        directories.sort()
        files.sort()
        current_path = Path(current)
        for name in directories + files:
            path = current_path / name
            relative = path.relative_to(bundle).as_posix()
            if path.is_symlink():
                target = path.resolve(strict=True)
                require(target.is_relative_to(resolved_bundle), f"app bundle symlink escapes bundle: {relative}")
                inventory.append({"path": relative, "kind": "symlink", "target": os.readlink(path)})
            elif path.is_dir():
                inventory.append({"path": relative, "kind": "directory"})
            elif path.is_file():
                inventory.append({"path": relative, "kind": "file", "size": path.stat().st_size,
                                  "mode": stat.S_IMODE(path.stat().st_mode), "sha256": sha256_file(path)})
            else:
                raise PilotError(f"unsupported file in app bundle: {relative}")
    return sha256_json(inventory)


def code_signature_facts(app_path: Path) -> dict:
    verified = subprocess.run(
        ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app_path)],
        capture_output=True, text=True, check=False,
    )
    require(verified.returncode == 0, "Installable Test app must have a valid deep code signature")
    details = subprocess.run(
        ["/usr/bin/codesign", "-dv", "--verbose=4", str(app_path)],
        capture_output=True, text=True, check=False,
    )
    require(details.returncode == 0, "Installable Test code signature details are unavailable")
    lines = (details.stdout + "\n" + details.stderr).splitlines()
    identifier = next((line.removeprefix("Identifier=") for line in lines if line.startswith("Identifier=")), None)
    team = next((line.removeprefix("TeamIdentifier=") for line in lines if line.startswith("TeamIdentifier=")), None)
    authorities = [line.removeprefix("Authority=") for line in lines if line.startswith("Authority=")]
    code_directory = next((line for line in lines if "CodeDirectory" in line), "")
    adhoc = "Signature=adhoc" in lines or "(adhoc)" in code_directory
    require(identifier == PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "Installable Test signature identifier does not match the frozen bundle contract")
    return {
        "verification": "passed",
        "identifier": identifier,
        "team_identifier": team,
        "authorities": authorities,
        "ad_hoc": adhoc,
    }


def validate_code_signature(app_path: Path, expected: Any) -> None:
    actual = code_signature_facts(app_path)
    require(isinstance(expected, dict) and expected == actual and actual.get("ad_hoc") is True,
            "Installable Test app must retain the verified ad-hoc signature recorded by its build owner")


BUILD_OWNER_PRINCIPAL = "voiceflow-local-pilot-build-owner"
BUILD_OWNER_NAMESPACE = "voiceflow-pilot-build"


def ssh_ed25519_public_key(path_value: str | Path) -> tuple[Path, bytes, str, str]:
    path = external_path(path_value, must_exist=True)
    require(path.is_file() and not Path(path_value).is_symlink(), "trusted build owner public key must be a regular file")
    try:
        payload = path.read_bytes()
        fields = payload.decode("utf-8").strip().split()
    except (OSError, UnicodeDecodeError) as error:
        raise PilotError(f"build owner public key could not be read: {error}") from error
    require(len(fields) in {2, 3} and fields[0] == "ssh-ed25519",
            "build owner key must be one OpenSSH Ed25519 public key")
    fingerprint = subprocess.run(["/usr/bin/ssh-keygen", "-lf", "-", "-E", "sha256"],
                                 input=payload, capture_output=True, check=False)
    require(fingerprint.returncode == 0, "could not compute the build owner public key fingerprint")
    fingerprint_fields = fingerprint.stdout.decode("utf-8", errors="replace").split()
    require(len(fingerprint_fields) >= 2 and fingerprint_fields[1].startswith("SHA256:"),
            "could not compute the build owner public key fingerprint")
    return path, payload, " ".join(fields[:2]), fingerprint_fields[1]


def verify_build_owner_signature(payload: bytes, signature_bytes: bytes, receipt_path: Path,
                                 public_key_path: str | Path, expected_fingerprint: str) -> None:
    public_key, _, key_line, fingerprint = ssh_ed25519_public_key(public_key_path)
    require(fingerprint == expected_fingerprint,
            "trusted build owner key fingerprint does not match the frozen build facts")
    require(not public_key.is_relative_to(receipt_path.parent.resolve()),
            "trusted build owner public key must be outside the build artifact root")
    with tempfile.TemporaryDirectory(prefix="voiceflow-pilot-allowed-signers-") as temporary:
        allowed_signers = Path(temporary) / "allowed_signers"
        signed_file = Path(temporary) / "receipt.json"
        signature_file = Path(temporary) / "receipt.json.sig"
        allowed_signers.write_text(f"{BUILD_OWNER_PRINCIPAL} {key_line}\n", encoding="utf-8")
        signed_file.write_bytes(payload)
        signature_file.write_bytes(signature_bytes)
        for path in (allowed_signers, signed_file, signature_file):
            os.chmod(path, 0o600)
        completed = subprocess.run(
            ["/usr/bin/ssh-keygen", "-Y", "verify", "-f", str(allowed_signers), "-I", BUILD_OWNER_PRINCIPAL,
             "-n", BUILD_OWNER_NAMESPACE, "-s", str(signature_file)],
            input=payload, capture_output=True, check=False,
        )
    require(completed.returncode == 0,
            "pre-C1 build owner receipt signature verification failed")


def validate_pre_c1_sidecars(app_path: Path, source_checkout: Path, receipt: dict | None = None) -> dict:
    resources = app_path / "Contents" / "Resources"
    source_inputs = (receipt or {}).get("source_input_hashes")
    if not isinstance(source_inputs, dict):
        source_inputs = pre_c1_source_input_hashes(source_checkout)
    runtime_pin = (source_checkout / "src-tauri" / "llama-runtime" / "runtime-pin.env").read_text(encoding="utf-8")
    pin_values = {}
    for line in runtime_pin.splitlines():
        if "=" in line and not line.lstrip().startswith("#"):
            key, value = line.split("=", 1)
            pin_values[key] = value
    identity_path = resources / "llama-runtime" / "BUILD-IDENTITY.json"
    runtime_path = resources / "llama-runtime" / "bin" / "llama-server"
    require(identity_path.is_file() and not identity_path.is_symlink()
            and runtime_path.is_file() and not runtime_path.is_symlink(),
            "Installable Test bundle is missing the pinned llama.cpp runtime identity")
    try:
        identity = json.loads(identity_path.read_bytes(), object_pairs_hook=object_pairs_no_duplicates)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PilotError(f"bundled llama.cpp identity is invalid: {error}") from error
    require(isinstance(identity, dict) and identity.get("schema_version") == 1
            and identity.get("status") == "ready" and identity.get("runtime") == "llama.cpp"
            and identity.get("source", {}).get("repository") == pin_values.get("LLAMA_CPP_SOURCE_URL")
            and identity.get("source", {}).get("release_tag") == pin_values.get("LLAMA_CPP_RELEASE_TAG")
            and identity.get("source", {}).get("annotated_tag_object_sha1") == pin_values.get("LLAMA_CPP_TAG_OBJECT_SHA1")
            and identity.get("source", {}).get("commit") == pin_values.get("LLAMA_CPP_COMMIT")
            and identity.get("artifact", {}).get("path") == "bin/llama-server"
            and identity.get("artifact", {}).get("architecture") == pin_values.get("LLAMA_RUNTIME_ARCH") == "arm64"
            and identity.get("artifact", {}).get("minimum_macos") == pin_values.get("LLAMA_RUNTIME_MINIMUM_MACOS")
            and identity.get("artifact", {}).get("sha256_signed") == sha256_file(runtime_path),
            "bundled llama.cpp identity does not match its source pin or signed executable")

    dependency_manifest = resources / "ThirdPartyLicenses" / "DependencyManifest" / "dependency-identities.txt"
    require(dependency_manifest.is_file() and not dependency_manifest.is_symlink(),
            "Installable Test bundle is missing the MLX sidecar dependency identity manifest")
    dependency_text = dependency_manifest.read_text(encoding="utf-8")
    expected_lines = {
        f"Sidecar Package.resolved SHA-256: {source_inputs['src-tauri/mlx-sidecar/Package.resolved']}",
        "Xcode Package.resolved SHA-256: "
        + source_inputs["src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved"],
        "mlx-audio-swift source overlay patch SHA-256: "
        + source_inputs["src-tauri/mlx-sidecar/sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch"],
        "mlx-audio-swift source overlay manifest SHA-256: "
        + source_inputs["src-tauri/mlx-sidecar/sdk-overlay/patch-manifest.json"],
    }
    require(expected_lines.issubset(set(dependency_text.splitlines())),
            "bundled MLX sidecar dependency identities do not match the pinned source locks")
    return {
        "llama_identity_sha256": sha256_file(identity_path),
        "llama_executable_sha256": sha256_file(runtime_path),
        "mlx_dependency_identity_sha256": sha256_file(dependency_manifest),
    }


def run_build_step(command: list[str], cwd: Path, environment: dict[str, str], log_path: Path, label: str) -> None:
    fd = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "wb") as log:
        completed = subprocess.run(command, cwd=cwd, env=environment, stdout=log, stderr=subprocess.STDOUT,
                                   check=False)
    if completed.returncode != 0:
        try:
            tail = log_path.read_bytes()[-8192:].decode("utf-8", errors="replace")
        except OSError:
            tail = "build log could not be read"
        raise PilotError(f"{label} failed with exit status {completed.returncode}; see {log_path}\n{tail}")


def checked_output(command: list[str], cwd: Path | None = None,
                   environment: dict[str, str] | None = None) -> str:
    completed = subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True, check=False)
    require(completed.returncode == 0, f"command failed: {' '.join(command)}: {completed.stderr.strip()}")
    return completed.stdout.strip()


def create_pre_c1_build(args: argparse.Namespace) -> dict:
    require(platform.system() == "Darwin" and platform.machine() == "arm64",
            "pre-C1 Installable Test build owner requires Apple silicon macOS")
    private_key = external_path(args.owner_private_key, must_exist=True)
    require(private_key.is_file() and not Path(args.owner_private_key).is_symlink(),
            "build owner private key must be a regular file")
    require(stat.S_IMODE(private_key.stat().st_mode) & 0o077 == 0,
            "build owner private key must not be accessible to group or others")
    public_key, public_key_bytes, _, signing_fingerprint = ssh_ed25519_public_key(args.owner_public_key)
    require(private_key != public_key, "build owner public and private key paths must be distinct")
    private_public = subprocess.run(
        ["/usr/bin/ssh-keygen", "-y", "-P", "", "-f", str(private_key)],
        capture_output=True, text=True, check=False,
    )
    require(private_public.returncode == 0,
            "build owner private key must be an unencrypted Ed25519 key; no passphrase prompt is allowed")
    require(private_public.stdout.strip().split()[:2] == public_key_bytes.decode("utf-8").strip().split()[:2],
            "build owner private key does not match the independently selected public key")
    raw_root = Path(args.owner_root)
    owner_root = external_path(raw_root)
    require(not raw_root.exists() and not raw_root.is_symlink(),
            "pre-C1 build owner root must be a new path")
    require(not owner_root.is_relative_to(private_key.parent.resolve())
            and not public_key.is_relative_to(owner_root),
            "build output and signing keys must remain in separate external roots")
    owner_root.parent.mkdir(parents=True, exist_ok=True)
    owner_root.mkdir(mode=0o700)
    source_checkout = owner_root / "source"
    git_template = owner_root / "git-template"
    git_template.mkdir(mode=0o700)
    git_environment = {
        "PATH": "/usr/bin:/bin",
        "HOME": os.environ.get("HOME", str(Path.home())),
        "TMPDIR": os.environ.get("TMPDIR", "/tmp"),
        "LANG": "C",
        "LC_ALL": "C",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_TEMPLATE_DIR": str(git_template),
    }
    clone = subprocess.run(
        ["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "clone", "--no-hardlinks", "--no-checkout", "--no-tags", str(REPO), str(source_checkout)],
        env=git_environment, capture_output=True, text=True, check=False,
    )
    require(clone.returncode == 0, f"could not create separate pre-C1 checkout: {clone.stderr.strip()}")
    checkout = subprocess.run(
        ["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-C", str(source_checkout), "checkout", "--detach", PRE_C1_SOURCE_REVISION],
        env=git_environment, capture_output=True, text=True, check=False,
    )
    require(checkout.returncode == 0, f"could not check out pinned pre-C1 source: {checkout.stderr.strip()}")
    revision = checked_output(["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
                               "-C", str(source_checkout), "rev-parse", "HEAD"], environment=git_environment)
    require(revision == PRE_C1_SOURCE_REVISION, "build owner checkout is not at the pinned pre-C1 revision")
    status = checked_output(["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
                             "-C", str(source_checkout), "status", "--porcelain", "--untracked-files=all"],
                            environment=git_environment)
    require(not status, "new pre-C1 checkout is not clean before dependency installation")
    source_tree_oid = checked_output(["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
                                      "-C", str(source_checkout), "rev-parse", "HEAD^{tree}"],
                                     environment=git_environment)
    source_date_epoch = int(checked_output(
        ["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-C", str(source_checkout), "show", "-s", "--format=%ct", "HEAD"], environment=git_environment,
    ))
    source_input_hashes = pre_c1_source_input_hashes(source_checkout)
    store_text = (source_checkout / "src-tauri" / "src" / "store.rs").read_text(encoding="utf-8")
    require(re.search(r"const\s+SETTINGS_SCHEMA_VERSION\s*:\s*u32\s*=\s*27\s*;", store_text) is not None
            and re.search(r"const\s+HISTORY_SCHEMA_VERSION\s*:\s*i32\s*=\s*19\s*;", store_text) is not None,
            "pinned pre-C1 source does not contain Settings/History schemas 27/19")

    npm_config = owner_root / "npmrc"
    npm_global_config = owner_root / "npm-globalrc"
    fd = os.open(npm_config, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(b"\n")
    fd = os.open(npm_global_config, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(b"\n")
    (owner_root / "npm-cache").mkdir(mode=0o700)
    (owner_root / "cargo-home").mkdir(mode=0o700)
    temporary_directory = owner_root / "tmp"
    temporary_directory.mkdir(mode=0o700)
    toolchain_executables = {}
    for tool_name in ("npm", "node", "cargo", "rustc", "xcodebuild"):
        executable_path = shutil.which(tool_name)
        require(executable_path is not None and Path(executable_path).is_absolute(),
                f"build owner could not resolve required tool: {tool_name}")
        executable_path = Path(executable_path).absolute()
        require(executable_path.is_file() and os.access(executable_path, os.X_OK),
                f"build owner tool is not an executable file: {tool_name}")
        toolchain_executables[tool_name] = str(executable_path)
    tool_directories = list(dict.fromkeys(Path(toolchain_executables[name]).parent.as_posix()
                                          for name in ("npm", "node", "cargo", "rustc")))
    build_path = ":".join(tool_directories + [
        "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin",
    ])
    environment = {
        "PATH": build_path,
        "HOME": os.environ.get("HOME", str(Path.home())),
        "TMPDIR": str(temporary_directory),
        "CI": "1",
        "CARGO_HOME": str(owner_root / "cargo-home"),
        "RUSTUP_HOME": str(Path.home() / ".rustup"),
        "NPM_CONFIG_USERCONFIG": str(npm_config),
        "NPM_CONFIG_GLOBALCONFIG": str(npm_global_config),
        "NPM_CONFIG_CACHE": str(owner_root / "npm-cache"),
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_TERMINAL_PROMPT": "0",
        "SOURCE_DATE_EPOCH": str(source_date_epoch),
        "TZ": "UTC",
        "LC_ALL": "C",
        "PYTHONNOUSERSITE": "1",
        "VOICEFLOW_LLAMA_BUILD_JOBS": "4",
    }
    run_build_step(["npm", "ci"], source_checkout, environment, owner_root / "npm-ci.log", "npm ci")
    run_build_step(PRE_C1_NPM_BUILD_COMMAND, source_checkout, environment,
                   owner_root / "tauri-build.log", "Installable Test app build")
    clean_after_build = checked_output(
        ["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-C", str(source_checkout), "status", "--porcelain", "--untracked-files=all"],
        environment=git_environment,
    )
    require(not clean_after_build, "pinned pre-C1 source checkout was modified by the build")
    require(pre_c1_source_input_hashes(source_checkout) == source_input_hashes,
            "build changed a pinned source or dependency lock input")
    app_path = source_checkout / "src-tauri" / "target" / "release" / "bundle" / "macos" / "VoiceFlow.app"
    require(app_path.is_dir() and not app_path.is_symlink(), "Tauri did not produce the expected Installable Test app")
    plist_path = app_path / "Contents" / "Info.plist"
    try:
        with plist_path.open("rb") as stream:
            info = plistlib.load(stream)
    except (OSError, plistlib.InvalidFileException) as error:
        raise PilotError(f"built Test app Info.plist could not be read: {error}") from error
    require(info.get("CFBundleIdentifier") == PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "built Test app does not have the Installable Test identifier")
    executable_name = info.get("CFBundleExecutable")
    require(isinstance(executable_name, str) and executable_name and Path(executable_name).name == executable_name,
            "built Test app executable identity unavailable")
    executable = app_path / "Contents" / "MacOS" / executable_name
    require(executable.is_file() and not executable.is_symlink() and is_macho_executable(executable),
            "Tauri Test app must contain a real Mach-O executable")
    signature = code_signature_facts(app_path)
    sidecars = validate_pre_c1_sidecars(app_path, source_checkout, {"source_input_hashes": source_input_hashes})
    receipt_path = owner_root / PRE_C1_BUILD_RECEIPT_NAME
    require(not receipt_path.exists() and not receipt_path.is_symlink()
            and not receipt_path.with_name(receipt_path.name + ".sig").exists(),
            "build owner receipt already exists")
    body = {
        "artifact_type": PRE_C1_BUILD_RECEIPT_TYPE,
        "schema_version": 1,
        "tool_version": TOOL_VERSION,
        "owner_tool_sha256": sha256_file(Path(__file__)),
        "owner_signing_key_fingerprint": signing_fingerprint,
        "owner_root": str(owner_root),
        "source_revision": revision,
        "source_tree_oid": source_tree_oid,
        "source_checkout_path": str(source_checkout),
        "source_input_hashes": source_input_hashes,
        "toolchain_executables": toolchain_executables,
        "build_path": build_path,
        "test_app_path": str(app_path),
        "test_bundle_identifier": PRE_C1_TEST_BUNDLE_IDENTIFIER,
        "build_sha256": sha256_file(executable),
        "app_bundle_sha256": bundle_inventory_sha256(app_path),
        "info_plist_sha256": sha256_file(plist_path),
        "code_signature": signature,
        "sidecar_provenance_sha256": sha256_json(sidecars),
        "npm_ci_command": ["npm", "ci"],
        "npm_ci_exit_status": 0,
        "tauri_build_command": PRE_C1_NPM_BUILD_COMMAND,
        "tauri_build_exit_status": 0,
        "node_version": checked_output(["node", "--version"], environment=environment),
        "npm_version": checked_output(["npm", "--version"], environment=environment),
        "rustc_version": checked_output(["rustc", "-Vv"], environment=environment),
        "cargo_version": checked_output(["cargo", "-V"], environment=environment),
        "xcode_version": checked_output(["xcodebuild", "-version"], environment=environment),
        "platform": platform.system(),
        "architecture": platform.machine(),
        "source_date_epoch": source_date_epoch,
    }
    body["owner_receipt_sha256"] = sha256_json(body)
    payload = json.dumps(body, ensure_ascii=False, indent=2, allow_nan=False).encode("utf-8") + b"\n"
    with tempfile.TemporaryDirectory(prefix="voiceflow-pilot-build-signing-", dir=owner_root) as temporary:
        unsigned_path = Path(temporary) / PRE_C1_BUILD_RECEIPT_NAME
        unsigned_path.write_bytes(payload)
        os.chmod(unsigned_path, 0o600)
        signed = subprocess.run(
            ["/usr/bin/ssh-keygen", "-Y", "sign", "-f", str(private_key),
             "-n", BUILD_OWNER_NAMESPACE, str(unsigned_path)],
            capture_output=True, check=False,
        )
        signature_temp = unsigned_path.with_name(unsigned_path.name + ".sig")
        require(signed.returncode == 0 and signature_temp.is_file(),
                "could not sign the pre-C1 build owner receipt with the selected local key")
        signature_bytes = signature_temp.read_bytes()
    write_once_bytes(receipt_path, payload)
    signature_path = receipt_path.with_name(receipt_path.name + ".sig")
    write_once_bytes(signature_path, signature_bytes)
    receipt_sha256 = hashlib.sha256(payload).hexdigest()
    signature_sha256 = hashlib.sha256(signature_bytes).hexdigest()
    return {
        "status": "pre_c1_build_complete",
        "source_checkout_path": str(source_checkout),
        "test_app_path": str(app_path),
        "build_owner_receipt_path": str(receipt_path),
        "build_owner_receipt_sha256": receipt_sha256,
        "build_owner_signature_sha256": signature_sha256,
        "build_owner_key_fingerprint": signing_fingerprint,
        "build_sha256": body["build_sha256"],
        "freeze_facts_base": {
            "artifact_type": "voiceflow_dictionary_learning_freeze_facts",
            "schema_version": 2,
            "source_revision": PRE_C1_SOURCE_REVISION,
            "settings_schema_version": PRE_C1_SETTINGS_SCHEMA_VERSION,
            "history_schema_version": PRE_C1_HISTORY_SCHEMA_VERSION,
            "test_bundle_identifier": PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "source_checkout_path": str(source_checkout),
            "test_app_path": str(app_path),
            "build_sha256": body["build_sha256"],
            "build_owner_receipt_path": str(receipt_path),
            "build_owner_receipt_sha256": receipt_sha256,
            "build_owner_signature_sha256": signature_sha256,
            "build_owner_key_fingerprint": signing_fingerprint,
        },
    }


def validate_pre_c1_build(facts: dict, build_owner_public_key: str | Path | None) -> None:
    receipt_path_value = facts.get("build_owner_receipt_path")
    require(isinstance(receipt_path_value, str) and receipt_path_value,
            "tool-owned pre-C1 build receipt path required")
    receipt_path = external_path(receipt_path_value, must_exist=True)
    require(receipt_path.name == PRE_C1_BUILD_RECEIPT_NAME and receipt_path.is_file()
            and not Path(receipt_path_value).is_symlink(),
            "tool-owned pre-C1 build receipt must be a regular owner output file")
    owner_root = receipt_path.parent
    require(owner_root.is_dir() and not owner_root.is_symlink()
            and (stat.S_IMODE(owner_root.stat().st_mode) & 0o077) == 0,
            "pre-C1 build owner output root must be a private directory")
    require(build_owner_public_key is not None,
            "an independently trusted build owner public key is required to validate a frozen pre-C1 build")
    require(receipt_path.stat().st_size <= MAX_JSON_BYTES, "build owner receipt byte budget exceeded")
    receipt_bytes = receipt_path.read_bytes()
    receipt = parse_json_document(receipt_bytes, receipt_path.name)
    verify_fingerprint(receipt, "owner_receipt_sha256")
    signature_path = receipt_path.with_name(receipt_path.name + ".sig")
    require(signature_path.is_file() and not signature_path.is_symlink(),
            "signed build owner receipt signature is missing")
    require(signature_path.stat().st_size <= 64 * 1024, "build owner signature byte budget exceeded")
    signature_bytes = signature_path.read_bytes()
    require(hashlib.sha256(receipt_bytes).hexdigest() == facts.get("build_owner_receipt_sha256")
            and hashlib.sha256(signature_bytes).hexdigest() == facts.get("build_owner_signature_sha256"),
            "signed build owner receipt changed after the allocation was frozen")
    require(receipt.get("owner_signing_key_fingerprint") == facts.get("build_owner_key_fingerprint"),
            "build owner receipt signer does not match the frozen trusted key fingerprint")
    verify_build_owner_signature(receipt_bytes, signature_bytes, receipt_path, build_owner_public_key,
                                 facts["build_owner_key_fingerprint"])

    source_checkout_value = facts.get("source_checkout_path")
    app_path_value = facts.get("test_app_path")
    require(isinstance(source_checkout_value, str) and source_checkout_value,
            "separate pre-C1 source checkout path required")
    require(isinstance(app_path_value, str) and app_path_value, "Installable Test app path required")
    source_checkout = external_path(source_checkout_value, must_exist=True)
    app_path = external_path(app_path_value, must_exist=True)
    expected_receipt_fields = {
        "artifact_type", "schema_version", "tool_version", "owner_tool_sha256",
        "owner_signing_key_fingerprint", "owner_root", "source_revision", "source_tree_oid",
        "source_checkout_path", "source_input_hashes", "toolchain_executables", "build_path",
        "test_app_path", "test_bundle_identifier",
        "build_sha256", "app_bundle_sha256", "info_plist_sha256", "code_signature",
        "sidecar_provenance_sha256", "npm_ci_command", "npm_ci_exit_status", "tauri_build_command",
        "tauri_build_exit_status", "node_version", "npm_version", "rustc_version", "cargo_version",
        "xcode_version", "platform", "architecture", "source_date_epoch", "owner_receipt_sha256",
    }
    require(set(receipt) == expected_receipt_fields,
            "pre-C1 build owner receipt fields do not match the trusted build protocol")
    require(receipt.get("artifact_type") == PRE_C1_BUILD_RECEIPT_TYPE
            and receipt.get("schema_version") == 1 and receipt.get("tool_version") == TOOL_VERSION
            and receipt.get("owner_tool_sha256") == sha256_file(Path(__file__)),
            "pre-C1 build owner receipt was not issued by this tool version")
    require(receipt.get("owner_root") == str(owner_root)
            and receipt.get("source_checkout_path") == str(source_checkout)
            and receipt.get("test_app_path") == str(app_path)
            and source_checkout.is_relative_to(owner_root)
            and app_path.is_relative_to(source_checkout),
            "pre-C1 build owner receipt paths do not match freeze facts")
    require(receipt.get("source_revision") == PRE_C1_SOURCE_REVISION
            and facts.get("source_revision") == PRE_C1_SOURCE_REVISION
            and receipt.get("test_bundle_identifier") == PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "pre-C1 build owner receipt source or bundle identity mismatch")
    require(receipt.get("npm_ci_command") == ["npm", "ci"] and receipt.get("npm_ci_exit_status") == 0
            and receipt.get("tauri_build_command") == PRE_C1_NPM_BUILD_COMMAND
            and receipt.get("tauri_build_exit_status") == 0,
            "pre-C1 build owner receipt does not record successful bounded build commands")
    for field in ("node_version", "npm_version", "rustc_version", "cargo_version", "xcode_version"):
        require(isinstance(receipt.get(field), str) and receipt[field].strip(),
                f"pre-C1 build owner receipt {field} missing")
    toolchain = receipt.get("toolchain_executables")
    require(isinstance(toolchain, dict) and set(toolchain) == {"npm", "node", "cargo", "rustc", "xcodebuild"}
            and all(isinstance(value, str) and Path(value).is_absolute() for value in toolchain.values()),
            "pre-C1 build owner receipt toolchain paths are invalid")
    tool_directories = list(dict.fromkeys(Path(toolchain[name]).parent.as_posix()
                                          for name in ("npm", "node", "cargo", "rustc")))
    expected_build_path = ":".join(tool_directories + [
        "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin",
    ])
    require(receipt.get("build_path") == expected_build_path,
            "pre-C1 build owner receipt build PATH does not match its recorded toolchain")
    require(receipt.get("platform") == "Darwin" and receipt.get("architecture") == "arm64"
            and isinstance(receipt.get("source_date_epoch"), int)
            and not isinstance(receipt.get("source_date_epoch"), bool)
            and receipt["source_date_epoch"] >= 0,
            "pre-C1 build owner platform or source date is invalid")

    git_environment = {
        "PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_TERMINAL_PROMPT": "0",
    }
    revision = subprocess.run(["/usr/bin/git", "-C", str(source_checkout), "rev-parse", "HEAD"],
                              env=git_environment, capture_output=True, text=True, check=False)
    require(revision.returncode == 0 and revision.stdout.strip() == PRE_C1_SOURCE_REVISION,
            "source checkout HEAD does not match the pinned pre-C1 revision")
    source_tree = subprocess.run(["/usr/bin/git", "-C", str(source_checkout), "rev-parse", "HEAD^{tree}"],
                                 env=git_environment, capture_output=True, text=True, check=False)
    require(source_tree.returncode == 0 and source_tree.stdout.strip() == receipt.get("source_tree_oid"),
            "pre-C1 build owner source tree identity mismatch")
    worktree_status = subprocess.run(
        ["/usr/bin/git", "-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null",
         "-C", str(source_checkout),
         "status", "--porcelain", "--untracked-files=all"],
        env=git_environment, capture_output=True, text=True, check=False,
    )
    require(worktree_status.returncode == 0 and not worktree_status.stdout.strip(),
            "pre-C1 source checkout must be clean at seal time")
    require(receipt.get("source_input_hashes") == pre_c1_source_input_hashes(source_checkout),
            "pre-C1 build owner source or dependency lock inputs changed")
    store_source = source_checkout / "src-tauri" / "src" / "store.rs"
    require(store_source.is_file() and not store_source.is_symlink(), "pre-C1 schema source file unavailable")
    store_text = store_source.read_text(encoding="utf-8")
    require(re.search(r"const\s+SETTINGS_SCHEMA_VERSION\s*:\s*u32\s*=\s*27\s*;", store_text) is not None
            and re.search(r"const\s+HISTORY_SCHEMA_VERSION\s*:\s*i32\s*=\s*19\s*;", store_text) is not None,
            "pre-C1 source checkout must contain Settings/History schemas 27/19")

    require(app_path.is_dir() and not Path(app_path_value).is_symlink(), "Installable Test app bundle required")
    plist_path = app_path / "Contents" / "Info.plist"
    require(plist_path.is_file() and not plist_path.is_symlink(), "Test app Info.plist required")
    try:
        with plist_path.open("rb") as stream:
            info = plistlib.load(stream)
    except (OSError, plistlib.InvalidFileException) as error:
        raise PilotError(f"Test app Info.plist could not be read: {error}") from error
    require(info.get("CFBundleIdentifier") == PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "Test app bundle identifier does not match the frozen pre-C1 contract")
    executable_name = info.get("CFBundleExecutable")
    require(isinstance(executable_name, str) and executable_name and Path(executable_name).name == executable_name,
            "Test app executable identity unavailable")
    executable = app_path / "Contents" / "MacOS" / executable_name
    require(executable.is_file() and not executable.is_symlink()
            and executable.resolve().is_relative_to(app_path.resolve()),
            "Test app executable unavailable or outside the bundle")
    require(is_macho_executable(executable), "Installable Test executable must be an executable arm64 Mach-O")
    require(sha256_file(executable) == facts["build_sha256"] == receipt.get("build_sha256"),
            "build_sha256 does not match the tool-owned Installable Test executable")
    require(sha256_file(plist_path) == receipt.get("info_plist_sha256"),
            "Installable Test Info.plist differs from the tool-owned build receipt")
    require(bundle_inventory_sha256(app_path) == receipt.get("app_bundle_sha256"),
            "Installable Test app bundle differs from the tool-owned build receipt")
    validate_code_signature(app_path, receipt.get("code_signature"))
    sidecars = validate_pre_c1_sidecars(app_path, source_checkout, receipt)
    require(sha256_json(sidecars) == receipt.get("sidecar_provenance_sha256"),
            "Installable Test sidecar provenance differs from the tool-owned build receipt")


def seal_manifest(args: argparse.Namespace) -> dict:
    for input_path in (args.draft, args.task_catalog, args.freeze_facts, args.technical_report,
                       args.technical_manifest, args.technical_evidence, args.artifact_root):
        if input_path is not None:
            external_path(input_path, must_exist=True)
    draft = read_json(args.draft)
    allocation_fingerprint = validate_allocation(draft)
    require(draft.get("freeze_status") == "draft", "only an unsealed allocation draft can be sealed")
    catalog = read_json(args.task_catalog)
    facts = read_json(args.freeze_facts)
    catalog_sha, task_specs = validate_task_catalog(draft, catalog)
    facts = validate_freeze_facts(draft, facts, args.build_owner_public_key)
    upstream_sha = draft.get("upstream_technical_report_sha256")
    if draft["stage"] == "benefit":
        require(all((args.technical_report, args.technical_manifest, args.technical_evidence, args.artifact_root)),
                "benefit sealing requires report plus bound technical manifest, evidence and artifact root")
        report, binding = validate_upstream_technical(
            args.technical_report, args.technical_manifest, args.technical_evidence, args.artifact_root,
            args.build_owner_public_key,
        )
        require(sha256_json(report) == upstream_sha, "technical report fingerprint does not match benefit allocation")
        require(binding == draft.get("upstream_technical_binding"),
                "technical input bindings changed since benefit allocation")
        validate_benefit_build_identity(facts, binding)
    else:
        require(not any((args.technical_report, args.technical_manifest, args.technical_evidence, args.artifact_root)),
                "technical allocation must not name upstream technical evidence")

    body = {key: value for key, value in draft.items() if key != "allocation_fingerprint"}
    body.update({
        "freeze_status": "frozen",
        "allocation_fingerprint": allocation_fingerprint,
        "task_catalog_sha256": catalog_sha,
        "task_specs": task_specs,
        "freeze_facts": facts,
    })
    body["freeze_fingerprint"] = sha256_json(body)
    write_once(args.out, body)
    return body


def manifest_fingerprint(manifest: dict) -> str:
    allocation = validate_allocation(manifest)
    if manifest.get("freeze_status") == "draft":
        return allocation
    require(manifest.get("freeze_status") == "frozen", "freeze_status invalid")
    freeze_fp = verify_fingerprint(manifest, "freeze_fingerprint")
    require(manifest.get("allocation_fingerprint") == allocation, "allocation fingerprint mismatch")
    require(isinstance(manifest.get("task_specs"), dict), "frozen task spec map missing")
    require(isinstance(manifest.get("freeze_facts"), dict), "frozen facts missing")
    return freeze_fp


def outcome_fields(manifest: dict, assignment: dict) -> list[str]:
    if manifest["stage"] == "technical":
        fields = [
            "actual_delivery_verified", "independent_terminal_verified", "alias_exposure",
            "alias_eligible", "alias_applied_correctly", "new_unintended_mutation",
            "actual_correction", "eligible_correction", "observer_captured", "observer_correct",
        ]
        if assignment["category"] == "correct_boundary":
            fields.append("source_text_correct")
        if assignment["category"] == "b_cross_scope":
            fields += ["wrong_scope_alias_applied", "b_conflict_probe", "explicit_conflict", "a_rule_unchanged"]
        if assignment["category"] == "forget_restart":
            fields += ["forgot_effective", "applied_after_forget", "resurrection"]
            if assignment["substage"] == "after_restart":
                fields.append("restart_verified")
        return fields
    return [
        "terminal_evidence_verified", "intended_output_correct", "actual_delivery_verified",
        "manual_edit_required", "critical_misedit", "cross_scope_application",
        "unauthorized_output", "state_contamination", "resurrection",
    ]


def blank_artifact_ref() -> dict:
    return {"path": None, "sha256": None}


def make_evidence_template(manifest: dict) -> dict:
    fp = manifest_fingerprint(manifest)
    trials = []
    for assignment in manifest["assignments"]:
        trials.append({
            "trial_id": assignment["trial_id"],
            "observed_phase_id": None,
            "observed_arm": None,
            "observed_operator_id": None,
            "observed_participant_id": None,
            "state_integrity": "unknown",
            "source_capture": {
                "capture_id": None,
                "recording_generation": None,
                "live_capture_verified": "unknown",
            },
            "stage_hashes": {"raw_asr_sha256": None, "inserted_text_sha256": None},
            "outcomes": {name: None for name in outcome_fields(manifest, assignment)},
            "provenance": {role: blank_artifact_ref() for role in (
                BENEFIT_TEMPLATE_ROLES if manifest["stage"] == "benefit" else TECHNICAL_TEMPLATE_ROLES
            )},
        })
    aux = []
    for assignment in manifest.get("auxiliary_observations", []):
        aux.append({
            "check_id": assignment["check_id"],
            "observed_phase_id": None,
            "observed_operator_id": None,
            "state_integrity": "unknown",
            "outcomes": {
                "observer_active": None,
                "mid_ime_state_write": None,
                "wrong_field_write": None,
            },
            "provenance": {role: blank_artifact_ref() for role in AUX_PROVENANCE_ROLES},
        })
    participant_costs = []
    if manifest["stage"] == "benefit":
        participant_costs = [
            {
                "participant_id": participant_id,
                "inventory_complete": "unknown",
                "teach_ms": None,
                "reteach_ms": None,
                "undo_ms": None,
                "already_in_completion_ms": None,
                "provenance": blank_artifact_ref(),
            }
            for participant_id in manifest["participants"]
        ]
    return {
        "artifact_type": "voiceflow_dictionary_learning_pilot_evidence",
        "schema_version": 1,
        "stage": manifest["stage"],
        "batch_id": manifest["batch_id"],
        "manifest_fingerprint": fp,
        "evidence_class": "unfilled",
        "human_data_attested": False,
        "local_only_confirmed": False,
        "research_authorization_confirmed": False,
        "participant_authorization": {participant_id: False for participant_id in manifest.get("participants", [])},
        "trials": trials,
        "auxiliary_observations": aux,
        "participant_time_costs": participant_costs,
    }


def fill_event_map(rows: Any, id_key: str, expected: list[dict]) -> tuple[dict[str, dict], dict]:
    expected_ids = {row[id_key] for row in expected}
    mapped = {}
    duplicates = []
    extras = []
    if isinstance(rows, list):
        for row in rows:
            if not isinstance(row, dict) or not isinstance(row.get(id_key), str):
                continue
            row_id = row[id_key]
            if row_id not in expected_ids:
                extras.append(row_id)
            elif row_id in mapped:
                duplicates.append(row_id)
            else:
                mapped[row_id] = row
    missing = sorted(expected_ids - set(mapped))
    return mapped, {"missing": missing, "extra": sorted(extras), "duplicate": sorted(duplicates)}


def gate(status: str, **details) -> dict:
    return {"status": status, **details}


def add_missing(missing: list[dict], item_id: str, field: str) -> None:
    missing.append({"item_id": item_id, "field": field})


def provenance_gate_status(stage_data: dict[str, dict], missing: list[dict], roles: tuple[str, ...],
                           integrity_issues: list[dict]) -> tuple[str, int, list[dict]]:
    invalid = [item for item in missing if item["field"].startswith("provenance.")
               and (item["field"].endswith("sha256_mismatch") or item["field"].endswith("path_invalid"))]
    invalid.extend(item for item in missing if item["field"].startswith("stage_hashes.") and item["field"].endswith("_invalid"))
    invalid.extend(integrity_issues)
    complete = sum(
        all(item["provenance"].get(role) for role in roles)
        and item.get("stage_hashes_complete") is True
        and item.get("capture_identity") is not None
        and item.get("provenance_typed_receipts_complete", item.get("typed_receipts_complete")) is True
        for item in stage_data.values()
    )
    if invalid:
        return "fail", complete, invalid
    if complete == len(stage_data):
        return "pass", complete, []
    return "pending", complete, []


def verify_artifact_ref(
    artifact_root: Path | None,
    reference: Any,
    item_id: str,
    role: str,
    missing: list[dict],
) -> bool:
    if not isinstance(reference, dict) or set(reference) != {"path", "sha256"}:
        add_missing(missing, item_id, f"provenance.{role}")
        return False
    rel_value = reference.get("path")
    digest_value = reference.get("sha256")
    if not isinstance(rel_value, str) or not rel_value.strip() or not isinstance(digest_value, str) or not SHA256.fullmatch(digest_value):
        add_missing(missing, item_id, f"provenance.{role}")
        return False
    if artifact_root is None:
        add_missing(missing, item_id, "artifact_root")
        return False
    artifact_root = artifact_root.resolve()
    relative = Path(rel_value)
    if relative.is_absolute() or ".." in relative.parts:
        add_missing(missing, item_id, f"provenance.{role}.path_invalid")
        return False
    candidate = artifact_root / relative
    if candidate.is_symlink() or not candidate.is_file():
        add_missing(missing, item_id, f"provenance.{role}.file_unavailable")
        return False
    resolved = candidate.resolve()
    if not resolved.is_relative_to(artifact_root):
        add_missing(missing, item_id, f"provenance.{role}.path_invalid")
        return False
    if sha256_file(resolved) != digest_value:
        add_missing(missing, item_id, f"provenance.{role}.sha256_mismatch")
        return False
    return True


def read_artifact_document(
    artifact_root: Path | None, reference: Any, item_id: str, role: str,
    verified: bool, integrity_issues: list[dict],
) -> dict | None:
    if not verified or artifact_root is None or not isinstance(reference, dict):
        return None
    path = artifact_root / reference["path"]
    try:
        return read_json(path.resolve())
    except (PilotError, OSError) as error:
        integrity_issues.append({"item_id": item_id, "field": f"{role}_receipt_invalid", "reason": str(error)})
        return None


def receipt_binding_errors(
    document: dict | None, artifact_type: str, manifest: dict, assignment: dict,
    item_id: str, role: str, integrity_issues: list[dict], *, trial_bound: bool = True,
) -> bool:
    if document is None:
        return False
    actor_id = assignment["operator_id"] if manifest["stage"] == "technical" else assignment["participant_id"]
    expected = {
        "artifact_type": artifact_type,
        "schema_version": 1,
        "batch_id": manifest["batch_id"],
        "manifest_fingerprint": manifest_fingerprint(manifest),
        "phase_id": manifest["phase_id"],
        "actor_id": actor_id,
    }
    if trial_bound:
        expected["trial_id"] = assignment["trial_id"]
        expected["arm"] = assignment["arm"]
    errors = [{"item_id": item_id, "field": f"{role}.{key}", "expected": value, "observed": document.get(key)}
              for key, value in expected.items() if document.get(key) != value]
    integrity_issues.extend(errors)
    return not errors


def verified_bound_receipt(
    artifact_root: Path | None, row: dict, role: str, manifest: dict, assignment: dict,
    verified: dict, missing: list[dict], integrity_issues: list[dict],
    *, artifact_type: str, trial_bound: bool = True,
) -> dict | None:
    item_id = assignment["trial_id"]
    reference = (row.get("provenance") or {}).get(role)
    document = read_artifact_document(artifact_root, reference, item_id, role,
                                      bool(verified.get(role)), integrity_issues)
    if verified.get(role) and document is None:
        return None
    if document is None:
        return None
    binding_ok = receipt_binding_errors(document, artifact_type, manifest, assignment, item_id, role,
                                        integrity_issues, trial_bound=trial_bound)
    return document if binding_ok else None


def _decimal_string(value: Any, *, positive: bool = False) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*" if positive else r"(?:0|[1-9][0-9]*)", value) is not None


AUDIT_PAIR_FIELDS = {
    "pair_ref", "pair_key", "before_surface", "after_surface", "rule_kind", "record_state",
    "candidate_hits", "current_classification", "current_promote_threshold", "classification_authority",
    "recorded_scope", "origin", "tombstone_reason", "latest_record_change_at",
    "last_output_surface_match_at", "current_dictionary_contains_after", "replacement_policy",
    "replacement_eligibility", "eligibility_authority", "current_review",
}
AUDIT_PAIR_STABLE_FIELDS = (
    "pair_key", "before_surface", "after_surface", "rule_kind", "record_state",
    "current_classification", "current_promote_threshold", "classification_authority", "recorded_scope",
    "origin", "tombstone_reason", "current_dictionary_contains_after", "replacement_policy",
    "replacement_eligibility", "eligibility_authority", "current_review",
)


def audit_pair_inventory_projections(pairs: Any) -> dict[str, Any]:
    """Hash bounded native pair projections without retaining their text surfaces."""
    require(isinstance(pairs, list), "bounded audit pair projection must be a list")
    all_rows = []
    active_rows = []
    pending_rows = []
    tombstone_rows = []
    pending_by_ref = {}
    pair_refs = set()
    for pair in pairs:
        require(isinstance(pair, dict) and set(pair) == AUDIT_PAIR_FIELDS,
                "bounded audit pair projection fields are incomplete or unsupported")
        pair_ref = pair.get("pair_ref")
        require(isinstance(pair_ref, str) and UUID.fullmatch(pair_ref) is not None
                and pair_ref not in pair_refs,
                "bounded audit pair projection reference is invalid or duplicated")
        pair_refs.add(pair_ref)
        require(all(isinstance(pair.get(key), str) for key in (
            "pair_key", "before_surface", "after_surface", "rule_kind", "record_state",
            "classification_authority", "origin", "replacement_policy", "replacement_eligibility",
            "eligibility_authority", "latest_record_change_at",
        )), "bounded audit pair projection text fields are invalid")
        require(pair["rule_kind"] in {"word_pair", "style_preference", "cleanup_intensity"}
                and pair["record_state"] in {"pending", "db_promoted", "tombstoned"},
                "bounded audit pair rule kind or state is invalid")
        require(isinstance(pair.get("candidate_hits"), int) and not isinstance(pair["candidate_hits"], bool)
                and pair["candidate_hits"] >= 0,
                "bounded audit pair candidate hit count is invalid")
        classification = pair.get("current_classification")
        require(classification is None or (
            isinstance(classification, dict) and set(classification) == {"class", "reason"}
            and classification.get("class") in {"name", "term", "rejected"}
            and (classification.get("reason") is None or isinstance(classification.get("reason"), str))
        ), "bounded audit pair classification is invalid")
        threshold = pair.get("current_promote_threshold")
        require(threshold is None or (isinstance(threshold, int) and not isinstance(threshold, bool) and threshold >= 0),
                "bounded audit pair promotion threshold is invalid")
        scope = pair.get("recorded_scope")
        require(isinstance(scope, dict) and set(scope) == {"family", "mapping_id", "browser_host", "native_bundle"}
                and all(value is None or isinstance(value, str) for value in scope.values()),
                "bounded audit pair scope is invalid")
        require(pair.get("tombstone_reason") is None or isinstance(pair["tombstone_reason"], str),
                "bounded audit pair tombstone reason is invalid")
        require(pair.get("last_output_surface_match_at") is None
                or isinstance(pair["last_output_surface_match_at"], str),
                "bounded audit pair use timestamp is invalid")
        require(isinstance(pair.get("current_dictionary_contains_after"), bool),
                "bounded audit pair dictionary-membership fact is invalid")
        review = pair.get("current_review")
        require(isinstance(review, dict) and set(review) == {"confirm", "ignore", "undo"}
                and all(isinstance(value, bool) for value in review.values()),
                "bounded audit pair review facts are invalid")
        stable = {key: pair[key] for key in AUDIT_PAIR_STABLE_FIELDS}
        inventory_row = {**stable, "candidate_hits": pair["candidate_hits"]}
        all_rows.append(inventory_row)
        if pair["record_state"] == "db_promoted":
            active_rows.append(stable)
        elif pair["record_state"] == "pending":
            pending_rows.append(inventory_row)
            pending_by_ref[pair_ref] = inventory_row
        else:
            tombstone_rows.append(inventory_row)

    def digest(rows: list[dict]) -> str:
        return sha256_json(sorted(rows, key=lambda value: value["pair_key"]))

    return {
        "rule_inventory_sha256": digest(all_rows),
        "active_rule_inventory_sha256": digest(active_rows),
        "inactive_candidate_inventory_sha256": digest(pending_rows),
        "tombstone_inventory_sha256": digest(tombstone_rows),
        "pending_by_ref": pending_by_ref,
    }


def _audit_snapshot_valid(snapshot: Any, item_id: str, boundary: str,
                          integrity_issues: list[dict]) -> bool:
    if not isinstance(snapshot, dict) or snapshot.get("schema_version") != 1:
        integrity_issues.append({"item_id": item_id, "field": f"owner_receipt.audit_{boundary}_invalid"})
        return False
    valid = True
    if not isinstance(snapshot.get("runtime_id"), str) or UUID.fullmatch(snapshot["runtime_id"]) is None:
        valid = False
    for key in ("audit_revision", "clear_generation", "observer_epoch"):
        if not _decimal_string(snapshot.get(key)):
            valid = False
    coverage = snapshot.get("coverage")
    if not isinstance(coverage, dict) or any((
        coverage.get("kind") != "current_process",
        coverage.get("capacity") != 128,
        coverage.get("dropped_entries") != "0",
        coverage.get("dropped_updates") != "0",
        coverage.get("projection_consistency") != "stable_revision",
        coverage.get("atomicity") != "not_cross_store_transaction",
        coverage.get("consistency_scope") != "audit_metadata_revision_only",
        coverage.get("projection_mutation_interval") != "not_observed",
        coverage.get("projection_inflight_writers") is not None,
        not isinstance(coverage.get("pair_projection"), dict)
        or coverage["pair_projection"].get("kind") != "complete_within_budget",
    )):
        valid = False
    activity = snapshot.get("activity")
    if (not isinstance(activity, list) or len(activity) > 128
            or not isinstance(snapshot.get("pairs"), list)):
        valid = False
        activity = []
    else:
        try:
            audit_pair_inventory_projections(snapshot["pairs"])
        except PilotError:
            valid = False
    activity_ids = []
    sequences = []
    for item in activity:
        if not isinstance(item, dict) or not isinstance(item.get("activity_id"), str) \
                or UUID.fullmatch(item["activity_id"]) is None or not _decimal_string(item.get("sequence"), positive=True) \
                or item.get("lifecycle") not in {"in_progress", "closed", "superseded", "unknown"} \
                or not isinstance(item.get("fact"), dict) \
                or item["fact"].get("kind") not in {"post_paste_attempt", "review_action"}:
            valid = False
            continue
        activity_ids.append(item["activity_id"])
        sequences.append(int(item["sequence"]))
    if (len(activity_ids) != len(activity) or len(set(activity_ids)) != len(activity_ids)
            or len(set(sequences)) != len(sequences) or sequences != sorted(sequences)):
        valid = False
    if isinstance(coverage, dict):
        oldest = activity[0].get("sequence") if activity else None
        latest = activity[-1].get("sequence") if activity else None
        if coverage.get("oldest_sequence") != oldest or coverage.get("latest_sequence") != latest:
            valid = False
    if not valid:
        integrity_issues.append({"item_id": item_id, "field": f"owner_receipt.audit_{boundary}_coverage_or_shape_invalid"})
    return valid


LEARNING_RECORD_FIELDS = {
    "schema_version", "selected_state_id", "baseline_run", "edit_fixture_id", "purpose",
    "term_fixture_id", "stage", "reason", "eligible", "same_field_window", "attribution",
    "candidate_class", "hits_before", "hits_after", "promote_threshold", "promoted",
    "persistence", "persistence_version", "hint_used", "replacement_used", "guard_effect",
    "undo_delete", "next_unseen_utterance",
}
LEARNING_PURPOSES = {"correction", "baseline", "unseen", "negative", "undo", "delete", "restart"}
LEARNING_STAGES = {
    "selected_state", "selected_pair_read", "observer_registration", "observation", "classification",
    "candidate_store", "style_candidate_store", "store_guard", "persistence", "settings_persistence",
    "style_store", "local_prepare_usage", "undo_delete",
}


def task_owner_binding(manifest: dict, assignment: dict) -> dict:
    actor_id = assignment["operator_id"] if manifest["stage"] == "technical" else assignment["participant_id"]
    binding = {
        "batch_id": manifest["batch_id"],
        "manifest_fingerprint": manifest_fingerprint(manifest),
        "phase_id": manifest["phase_id"],
        "trial_id": assignment["trial_id"],
        "actor_id": actor_id,
        "arm": assignment["arm"],
    }
    task_specs = manifest.get("task_specs")
    if isinstance(task_specs, dict) and isinstance(task_specs.get(assignment["trial_id"]), str):
        binding["task_spec_sha256"] = task_specs[assignment["trial_id"]]
    return binding


def validate_learning_record(record: Any, item_id: str, integrity_issues: list[dict]) -> bool:
    valid_shape = isinstance(record, dict) and set(record) == LEARNING_RECORD_FIELDS
    if not valid_shape:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_record_shape_invalid"})
        return False
    valid = (
        record.get("schema_version") == 1
        and all(isinstance(record.get(key), str) and record[key].strip()
                for key in ("selected_state_id", "edit_fixture_id", "stage", "reason", "attribution",
                            "persistence", "guard_effect", "next_unseen_utterance"))
        and record.get("purpose") in LEARNING_PURPOSES
        and record.get("stage") in LEARNING_STAGES
        and (record.get("baseline_run") is None or isinstance(record.get("baseline_run"), str))
        and (record.get("term_fixture_id") is None or isinstance(record.get("term_fixture_id"), str))
        and (record.get("eligible") is None or isinstance(record.get("eligible"), bool))
        and record.get("same_field_window") in {"unknown", "same_field_owner_completed", "left_target"}
        and record.get("candidate_class") in {None, "name", "term", "rejected"}
        and (record.get("persistence_version") is None
             or (isinstance(record.get("persistence_version"), str)
                 and SHA256.fullmatch(record["persistence_version"]) is not None))
        and all(record.get(key) is None or (isinstance(record[key], int)
                and not isinstance(record[key], bool) and record[key] >= 0)
                for key in ("hits_before", "hits_after", "promote_threshold"))
        and all(record.get(key) is None or isinstance(record.get(key), bool)
                for key in ("promoted", "hint_used", "replacement_used"))
        and (record.get("undo_delete") is None or isinstance(record.get("undo_delete"), str))
    )
    if not valid:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_record_invalid"})
    return valid


def verify_research_learning_records(
    document: dict, item_id: str, expected_task_binding: dict | None,
    integrity_issues: list[dict],
) -> dict:
    events = document.get("events")
    learning_events = [event for event in events if isinstance(event, dict) and event.get("event") == "learning_record"]
    unknown = {"status": None, "candidate_refs": [], "reason": "learning_record_missing", "source": "research_stdio"}
    if not learning_events:
        return unknown
    binding = document.get("learning_binding")
    required = {"selected_state_id", "baseline_run", "edit_fixture_id", "purpose", "term_fixture_ids",
                "task_spec_sha256"}
    if not isinstance(binding, dict) or set(binding) != required:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_binding_invalid"})
        return unknown
    term_ids = binding.get("term_fixture_ids")
    if (not isinstance(term_ids, list) or not term_ids
            or any(not isinstance(value, str) or SAFE_ID.fullmatch(value) is None for value in term_ids)
            or len(set(term_ids)) != len(term_ids)):
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_term_binding_invalid"})
        return unknown
    if (not isinstance(binding.get("selected_state_id"), str) or not binding["selected_state_id"].strip()
            or not isinstance(binding.get("edit_fixture_id"), str) or not binding["edit_fixture_id"].strip()
            or binding.get("purpose") not in LEARNING_PURPOSES
            or (binding.get("baseline_run") is not None and not isinstance(binding.get("baseline_run"), str))
            or not isinstance(binding.get("task_spec_sha256"), str)
            or SHA256.fullmatch(binding["task_spec_sha256"]) is None
            or (expected_task_binding is not None
                and expected_task_binding.get("task_spec_sha256") != binding.get("task_spec_sha256"))):
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_binding_mismatch"})
        return unknown

    records = []
    for event in learning_events:
        record = event.get("record")
        if not validate_learning_record(record, item_id, integrity_issues):
            continue
        for key in ("selected_state_id", "baseline_run", "edit_fixture_id", "purpose"):
            if record.get(key) != binding.get(key):
                integrity_issues.append({"item_id": item_id,
                                         "field": f"owner_receipt.learning_record_{key}_mismatch"})
        if record.get("term_fixture_id") is not None and record["term_fixture_id"] not in term_ids:
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_record_term_mismatch"})
        records.append((int(event["sequence"]), record))
    if len(records) != len(learning_events):
        return unknown
    records.sort(key=lambda item: item[0])
    registrations = [(sequence, record) for sequence, record in records
                     if record["stage"] == "observer_registration"]
    observations = [(sequence, record) for sequence, record in records if record["stage"] == "observation"]
    positive_registrations = [(sequence, record) for sequence, record in registrations
                              if record.get("eligible") is True and record.get("reason") == "registered"]
    denied_registrations = [(sequence, record) for sequence, record in registrations
                            if record.get("eligible") is False and record.get("reason") == "no_grant"]
    if any(record.get("eligible") is False for _, record in registrations) and positive_registrations:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_registration_conflict"})
        return unknown
    candidates = [(sequence, record) for sequence, record in records
                  if record["stage"] == "candidate_store" and record.get("term_fixture_id") in term_ids
                  and record.get("candidate_class") in {"name", "term"}]
    if candidates and denied_registrations:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_no_grant_candidate_conflict"})
        return unknown
    if len({record["term_fixture_id"] for _, record in candidates}) != len(candidates):
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_duplicate_candidate"})
        return unknown
    matching_observations = [(sequence, record) for sequence, record in observations
                             if record.get("reason") == "single_token"
                             and record.get("same_field_window") == "same_field_owner_completed"]
    candidate_ids = []
    for candidate_sequence, candidate in candidates:
        if not any(sequence < candidate_sequence for sequence, _ in positive_registrations):
            integrity_issues.append({"item_id": item_id,
                                     "field": "owner_receipt.learning_candidate_without_registration"})
            return unknown
        if not any(sequence < candidate_sequence for sequence, _ in matching_observations):
            integrity_issues.append({"item_id": item_id,
                                     "field": "owner_receipt.learning_candidate_without_observation"})
            return unknown
        if candidate.get("reason") not in {"record_owner_returned", "store_error"}:
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.learning_candidate_reason_invalid"})
            return unknown
        candidate_ids.append(candidate["term_fixture_id"])
    if candidates:
        return {"status": True, "candidate_refs": sorted(candidate_ids),
                "reason": "learning_candidate_classified", "source": "research_stdio"}
    if denied_registrations:
        return {"status": False, "candidate_refs": [], "reason": "no_grant", "source": "research_stdio"}
    if positive_registrations and observations:
        return {"status": False, "candidate_refs": [], "reason": "no_qualifying_candidate", "source": "research_stdio"}
    return unknown


AUDIT_SKIP_REASONS = {
    "unverified", "read_owner_unavailable", "delivery_owner_mismatch", "read_lease_expired",
    "secure", "no_grant", "unreadable", "delivery_fact_mismatch", "fixture_baseline_mismatch",
    "ambiguous_inserted_range", "stale_or_cancelled", "protected_target", "no_mapping",
    "ax_or_learning_not_allowed", "scene_not_allowed", "target_changed",
}
AUDIT_CANDIDATE_DECISIONS = {
    "not_attempted", "rejected", "scope_conflict", "tombstone_or_ignored", "already_db_promoted",
    "pending", "duplicate_observation", "cap", "dictation_origin_unknown", "unknown",
    "unknown_scope", "db_promotion_failed", "db_promoted",
}
AUDIT_NONLEXICAL_OUTCOMES = {
    "unchanged", "nochange", "unreadable", "left_target", "target_changed", "ambiguous",
    "short_style_rewrite", "style_signal",
}


def _normal_audit_observer_capture(
    fact: dict, pair_ref: str | None, pairs_by_ref: dict[str, dict],
    item_id: str, integrity_issues: list[dict],
) -> dict:
    """Classify one native audit activity without treating persistence as observation authority."""
    registration = fact["registration"]
    classification = fact["classification"]
    decision = fact["candidate_decision"]
    candidate_store = fact["candidate_store"]
    store_status = candidate_store["status"]
    unknown = {"status": None, "candidate_refs": [], "reason": "audit_fact_ambiguous",
               "source": "bounded_audit", "candidate_persistence_status": store_status,
               "pending_candidate_writes": []}
    if registration["status"] == "checking":
        return {**unknown, "reason": "registration_incomplete"}
    if registration["status"] == "skipped":
        reason = registration.get("reason")
        if (reason not in AUDIT_SKIP_REASONS or fact.get("outcome") not in {"not_observed", "stale_or_cancelled"}
                or classification is not None or decision != "not_attempted"
                or candidate_store != {"status": "not_attempted", "reason": None}
                or pair_ref is not None):
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_skip_candidate_conflict"})
            return {**unknown, "reason": "contradictory_skip"}
        return {"status": False, "candidate_refs": [], "reason": reason,
                "source": "bounded_audit", "candidate_persistence_status": store_status}
    if registration.get("reason") != "registered" or fact.get("verified_paste") is not True:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_registration_fact_conflict"})
        return {**unknown, "reason": "registration_fact_conflict"}
    if fact["outcome"] in {"not_observed", "observing"}:
        return {**unknown, "reason": "observation_incomplete"}
    grouped_multi_token = fact["outcome"] in {"multiple_tokens", "multiple_tokens_unsupported"}
    if fact["outcome"] in AUDIT_NONLEXICAL_OUTCOMES:
        style_outcome = fact["outcome"] in {"short_style_rewrite", "style_signal"}
        if style_outcome and classification is None:
            if pair_ref is not None:
                style_pair = pairs_by_ref.get(pair_ref)
                if (not isinstance(style_pair, dict)
                        or style_pair.get("rule_kind") not in {"style_preference", "cleanup_intensity"}
                        or style_pair.get("record_state") not in {"pending", "db_promoted"}
                        or decision != "pending" or store_status != "completed"
                        or candidate_store.get("reason") != "candidate_db_returned"):
                    integrity_issues.append({"item_id": item_id,
                                             "field": "owner_receipt.audit_style_candidate_conflict"})
                    return {**unknown, "reason": "style_candidate_conflict"}
                return {"status": False, "candidate_refs": [], "reason": "style_candidate_recorded",
                        "source": "bounded_audit", "candidate_persistence_status": store_status}
            if (decision == "not_attempted" and store_status in {"not_attempted", "completed", "failed"}
                    and (candidate_store.get("reason") is None
                         or candidate_store.get("reason") in {"style_draft_returned", "store_error"})):
                return {"status": False, "candidate_refs": [], "reason": "style_candidate_without_lexical_pair",
                        "source": "bounded_audit", "candidate_persistence_status": store_status}
        if (classification is not None or decision != "not_attempted" or store_status != "not_attempted"
                or pair_ref is not None):
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_noncorrection_candidate_conflict"})
            return {**unknown, "reason": "contradictory_noncorrection"}
        return {"status": False, "candidate_refs": [], "reason": fact["outcome"],
                "source": "bounded_audit", "candidate_persistence_status": store_status}
    stale_after_observation = fact["outcome"] == "stale_or_cancelled"
    if fact["outcome"] != "single_token" and not stale_after_observation and not grouped_multi_token:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_outcome_invalid"})
        return {**unknown, "reason": "outcome_invalid"}
    if fact.get("attribution") != "whole_field_difference_only" and not grouped_multi_token and not (
            stale_after_observation and classification is None and decision == "not_attempted"
            and store_status == "not_attempted"):
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_single_token_attribution_invalid"})
        return {**unknown, "reason": "attribution_invalid"}
    classification_class = classification.get("class") if isinstance(classification, dict) else None
    if classification is None and decision == "not_attempted" and store_status == "not_attempted" and pair_ref is None:
        if grouped_multi_token:
            return {**unknown, "reason": "multiple_token_capture_unsupported"}
        return {"status": False, "candidate_refs": [],
                "reason": "stale_before_candidate" if stale_after_observation else "single_token_without_eligible_pair",
                "source": "bounded_audit", "candidate_persistence_status": store_status}
    if classification is None and decision in {"pending", "already_db_promoted", "db_promoted", "scope_conflict"}:
        existing_pair = pairs_by_ref.get(pair_ref) if pair_ref is not None else None
        pair_classification = existing_pair.get("current_classification") if isinstance(existing_pair, dict) else None
        if (isinstance(existing_pair, dict) and existing_pair.get("rule_kind") == "word_pair"
                and isinstance(existing_pair.get("before_surface"), str) and existing_pair["before_surface"]
                and isinstance(existing_pair.get("after_surface"), str) and existing_pair["after_surface"]
                and isinstance(pair_classification, dict) and pair_classification.get("class") in {"name", "term"}):
            classification_class = pair_classification["class"]
        else:
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_classification_missing"})
            return {**unknown, "reason": "candidate_classification_missing"}
    if (classification is not None and (
            not isinstance(classification, dict) or set(classification) != {"class", "reason"}
            or classification_class not in {"name", "term", "rejected"}
            or (classification.get("reason") is not None and not isinstance(classification.get("reason"), str)))):
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_classification_invalid"})
        return {**unknown, "reason": "classification_invalid"}
    if classification is None and classification_class not in {"name", "term"}:
        return {**unknown, "reason": "candidate_classification_missing"}
    if decision not in AUDIT_CANDIDATE_DECISIONS:
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_decision_invalid"})
        return {**unknown, "reason": "candidate_decision_invalid"}
    if (candidate_store.get("reason") is not None and not isinstance(candidate_store.get("reason"), str)
            or store_status not in {"completed", "failed", "not_attempted"}):
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_persistence_invalid"})
        return {**unknown, "reason": "candidate_persistence_invalid"}
    if classification_class == "rejected":
        if decision != "rejected" or store_status != "not_attempted" or pair_ref is not None:
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_rejected_candidate_conflict"})
            return {**unknown, "reason": "rejected_candidate_conflict"}
        return {"status": False, "candidate_refs": [], "reason": "candidate_rejected",
                "source": "bounded_audit", "candidate_persistence_status": store_status}
    if classification_class in {"name", "term"} and decision != "not_attempted":
        expected_store_reason = "db_promotion_recorded" if decision == "db_promoted" else "candidate_db_returned"
        if decision in {"pending", "db_promoted"} and not (
                store_status == "completed" and candidate_store.get("reason") == expected_store_reason):
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_status_conflict"})
            return {**unknown, "reason": "candidate_status_conflict"}
        if decision == "already_db_promoted" and store_status not in {"completed", "not_attempted"}:
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_status_conflict"})
            return {**unknown, "reason": "candidate_status_conflict"}
        if pair_ref is not None and pair_ref not in pairs_by_ref:
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_pair_missing"})
            return {**unknown, "reason": "candidate_pair_missing"}
        candidate_refs = []
        if decision in {"pending", "already_db_promoted", "db_promoted"}:
            pair = pairs_by_ref.get(pair_ref) if pair_ref is not None else None
            expected_record_state = "pending" if decision == "pending" else "db_promoted"
            if pair is None or pair.get("rule_kind") != "word_pair" \
                    or pair.get("record_state") != expected_record_state:
                integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_pair_invalid"})
                return {**unknown, "reason": "candidate_pair_invalid"}
            candidate_refs = [pair_ref]
        elif decision == "scope_conflict":
            # Scope conflicts can expose an existing pair ref; it is not the candidate for this task.
            candidate_refs = []
        pending_candidate_writes = ([{"pair_ref": pair_ref, "status": store_status,
                                      "reason": candidate_store.get("reason")}] if decision == "pending" else [])
        return {"status": True, "candidate_refs": candidate_refs,
                "pending_candidate_writes": pending_candidate_writes,
                "reason": f"{'grouped_multiple_token' if grouped_multi_token else 'lexical_observation'}_{decision}",
                "source": "bounded_audit",
                "candidate_persistence_status": store_status}
    integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_candidate_classification_conflict"})
    return {**unknown, "reason": "candidate_classification_conflict"}


def _is_verified_b_conflict_review_action(activity: dict) -> bool:
    fact = activity.get("fact")
    return (
        activity.get("lifecycle") == "closed"
        and activity.get("observer_epoch") is None
        and activity.get("pair_ref") is None
        and isinstance(fact, dict)
        and set(fact) == {
            "kind", "action", "source", "history_reference", "outcome",
            "candidate_store", "settings_store",
        }
        and fact.get("kind") == "review_action"
        and fact.get("action") == "add_manual_scoped_pair"
        and fact.get("source") == "dictionary_command"
        and fact.get("history_reference") == "not_provided"
        and fact.get("outcome") == "failed"
        and fact.get("candidate_store") == {"status": "failed", "reason": "store_error"}
        and fact.get("settings_store") == {"status": "not_attempted", "reason": None}
    )


def verify_owner_capture_receipt(
    document: dict | None, item_id: str, integrity_issues: list[dict],
    expected_build_sha256: str | None = None,
    expected_task_binding: dict | None = None,
    expected_task_category: str | None = None,
) -> tuple[str | None, str | None, str | None, dict | None]:
    """Return stable evidence identity and owner-derived observer facts."""
    if document is None:
        return None, None, None, None
    valid = True
    base = {
        "artifact_type": "voiceflow_dictionary_learning_owner_receipt",
        "schema_version": 1,
        "capture_closed": True,
        "local_terminal": True,
    }
    for key, value in base.items():
        if document.get(key) != value:
            valid = False
            integrity_issues.append({"item_id": item_id, "field": f"owner_receipt.{key}",
                                     "expected": value, "observed": document.get(key)})
    build_sha256 = document.get("build_sha256")
    if (not isinstance(build_sha256, str) or SHA256.fullmatch(build_sha256) is None
            or (expected_build_sha256 is not None and build_sha256 != expected_build_sha256)):
        valid = False
        integrity_issues.append({"item_id": item_id, "field": "owner_receipt.build_sha256_mismatch"})
    if expected_task_binding is not None:
        for key, expected in expected_task_binding.items():
            if document.get(key) != expected:
                valid = False
                integrity_issues.append({"item_id": item_id,
                                         "field": f"owner_receipt.task_binding_{key}_mismatch",
                                         "expected": expected, "observed": document.get(key)})

    if document.get("source") == "voiceflow_research_stdio_v1":
        for key, value in {"coverage": "owner_hooks_only", "dropped": "0",
                           "provider_termination": "unknown"}.items():
            if document.get(key) != value:
                valid = False
                integrity_issues.append({"item_id": item_id, "field": f"owner_receipt.{key}",
                                         "expected": value, "observed": document.get(key)})
        for key in ("run_id", "case_id", "job_id"):
            if not isinstance(document.get(key), str) or UUID.fullmatch(document[key]) is None:
                valid = False
                integrity_issues.append({"item_id": item_id, "field": f"owner_receipt.{key}_invalid"})
        generation = document.get("recording_generation")
        if not _decimal_string(generation, positive=True):
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.recording_generation_invalid"})
        events = document.get("events")
        if not isinstance(events, list) or not events:
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.events_missing"})
            events = []
        sequences = []
        for event in events:
            if not isinstance(event, dict):
                valid = False
                continue
            for key in ("run_id", "case_id", "job_id", "recording_generation"):
                if event.get(key) != document.get(key):
                    valid = False
                    integrity_issues.append({"item_id": item_id, "field": f"owner_receipt.event_{key}_mismatch"})
            if not _decimal_string(event.get("sequence")) or not _decimal_string(event.get("monotonic_ns")):
                valid = False
            else:
                sequences.append(int(event["sequence"]))
        if len(sequences) != len(events) or sequences != list(range(len(events))):
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.event_sequence_gap"})
        starts = [index for index, event in enumerate(events)
                  if isinstance(event, dict) and event.get("event") == "session_started"
                  and event.get("trigger_source") == "explicit_research_stdio"]
        inputs = [index for index, event in enumerate(events)
                  if isinstance(event, dict) and event.get("event") == "input_first_accepted"
                  and event.get("source") == "capture_worker_16khz_samples"
                  and isinstance(event.get("samples"), int) and not isinstance(event.get("samples"), bool)
                  and event["samples"] > 0]
        closed = [index for index, event in enumerate(events)
                  if isinstance(event, dict) and event.get("event") == "capture_closed"
                  and event.get("complete") is True
                  and isinstance(event.get("samples"), int) and not isinstance(event.get("samples"), bool)
                  and event["samples"] > 0]
        terminals = [index for index, event in enumerate(events)
                     if isinstance(event, dict) and event.get("event") == "session_terminal"]
        cancellations = [index for index, event in enumerate(events)
                         if isinstance(event, dict) and event.get("event") == "cancellation_requested"]
        lifecycle_ordered = (
            len(starts) == len(inputs) == len(closed) == len(terminals) == 1
            and starts[0] == 0 and starts[0] < inputs[0] < closed[0] < terminals[0] == len(events) - 1
            and len(cancellations) <= 1
            and (len(cancellations) == 1) == (events[terminals[0]].get("outcome") == "cancelled")
            and (not cancellations or cancellations[0] < terminals[0])
        )
        if not lifecycle_ordered:
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.lifecycle_order_invalid"})
        terminal = events[terminals[0]] if len(terminals) == 1 else {}
        if (terminal.get("outcome") not in {"succeeded", "failed", "cancelled"}
                or terminal.get("side_effect_boundary") not in {
                    "no_delivery_write_observed", "clipboard_write_attempted", "keyboard_may_have_posted"
                }
                or terminal.get("provider_termination") != "unknown"):
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.terminal_result_invalid"})
        if not valid:
            return None, None, None, None
        run_id = document["run_id"]
        learning_fact = verify_research_learning_records(document, item_id, expected_task_binding, integrity_issues)
        observer_fact = {
            "status": None,
            "capture_id": run_id,
            "mechanism_status": learning_fact["status"],
            "candidate_refs": [],
            "mechanism_candidate_refs": learning_fact["candidate_refs"],
            "reason": "research_stdio_not_human_acceptance_authority",
            "mechanism_reason": learning_fact["reason"],
            "source": "research_stdio",
            "human_acceptance_authority": False,
        }
        return f"research:{run_id}:{generation}", run_id, generation, observer_fact

    if document.get("source") == "voiceflow_installable_test_bounded_audit_v1":
        if document.get("test_bundle_identifier") != PRE_C1_TEST_BUNDLE_IDENTIFIER:
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.test_bundle_identifier_invalid"})
        if document.get("coverage") != "current_process_bounded_audit":
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.coverage_invalid"})
        attestation = document.get("operator_attestation")
        required_attestation = {"live_hotkey_capture": True, "task_id_verified": True, "capture_closed": True}
        if not isinstance(attestation, dict) or any(attestation.get(key) is not value for key, value in required_attestation.items()):
            valid = False
            integrity_issues.append({"item_id": item_id, "field": "owner_receipt.operator_attestation_incomplete"})
        before, after = document.get("audit_before"), document.get("audit_after")
        before_ok = _audit_snapshot_valid(before, item_id, "before", integrity_issues)
        after_ok = _audit_snapshot_valid(after, item_id, "after", integrity_issues)
        valid = valid and before_ok and after_ok
        activities = []
        primary_activity = None
        audit_pair_inventories = None
        changed_inactive_candidate_refs = []
        if before_ok and after_ok:
            try:
                before_pairs = audit_pair_inventory_projections(before["pairs"])
                after_pairs = audit_pair_inventory_projections(after["pairs"])
            except PilotError:
                before_pairs = after_pairs = None
                valid = False
                integrity_issues.append({"item_id": item_id,
                                         "field": "owner_receipt.audit_pair_inventory_invalid"})
            if before_pairs is not None and after_pairs is not None:
                audit_pair_inventories = {"before": before_pairs, "after": after_pairs}
                before_pending = before_pairs["pending_by_ref"]
                after_pending = after_pairs["pending_by_ref"]
                changed_inactive_candidate_refs = sorted(
                    pair_ref for pair_ref in set(before_pending) | set(after_pending)
                    if before_pending.get(pair_ref) != after_pending.get(pair_ref)
                )
                if before_pairs["active_rule_inventory_sha256"] != after_pairs["active_rule_inventory_sha256"]:
                    valid = False
                    integrity_issues.append({"item_id": item_id,
                                             "field": "owner_receipt.audit_active_pair_inventory_changed"})
                if before_pairs["tombstone_inventory_sha256"] != after_pairs["tombstone_inventory_sha256"]:
                    valid = False
                    integrity_issues.append({"item_id": item_id,
                                             "field": "owner_receipt.audit_tombstone_pair_inventory_changed"})
            same_process = all(before.get(key) == after.get(key)
                               for key in ("runtime_id", "clear_generation"))
            epoch_advanced_once = int(after["observer_epoch"]) == int(before["observer_epoch"]) + 1
            old = {entry.get("activity_id") for entry in before["activity"] if isinstance(entry, dict)}
            after_ids = {entry.get("activity_id") for entry in after["activity"] if isinstance(entry, dict)}
            new = [entry for entry in after["activity"] if isinstance(entry, dict)
                   and entry.get("activity_id") not in old]
            old_sequences = [int(entry["sequence"]) for entry in before["activity"]
                             if isinstance(entry, dict) and _decimal_string(entry.get("sequence"), positive=True)]
            try:
                revisions_advanced = int(after["audit_revision"]) > int(before["audit_revision"])
            except (TypeError, ValueError):
                revisions_advanced = False
            if (not same_process or not epoch_advanced_once or not revisions_advanced
                    or not old.issubset(after_ids) or not new):
                valid = False
                integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_task_delta_invalid"})
            else:
                activities = new
                primary_activity = next((entry for entry in activities
                                         if entry.get("activity_id") == document.get("activity_id")), None)
                if primary_activity is None:
                    valid = False
                    integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_primary_activity_missing"})
                if old_sequences and any(int(entry["sequence"]) <= max(old_sequences) for entry in activities):
                    valid = False
                    integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_sequence_not_new"})
                if any(
                    (entry.get("fact", {}).get("kind") == "post_paste_attempt"
                     and entry.get("observer_epoch") != after.get("observer_epoch"))
                    or (entry.get("fact", {}).get("kind") == "review_action"
                        and entry.get("observer_epoch") is not None)
                    for entry in activities
                ):
                    valid = False
                    integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_activity_epoch_mismatch"})

        observer_fact = {"status": None, "candidate_refs": [], "reason": "audit_delta_missing",
                         "source": "bounded_audit", "human_acceptance_authority": True}
        if activities:
            pairs_by_ref = {}
            duplicate_pair_refs = set()
            for pair in after.get("pairs", []):
                if not isinstance(pair, dict) or not isinstance(pair.get("pair_ref"), str):
                    continue
                if pair["pair_ref"] in pairs_by_ref:
                    duplicate_pair_refs.add(pair["pair_ref"])
                else:
                    pairs_by_ref[pair["pair_ref"]] = pair
            if duplicate_pair_refs:
                valid = False
                integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_pair_ref_duplicate"})
            observer_parts = []
            post_paste_activities = []
            b_conflict_probe_review_action = False
            review_action_count = 0
            for activity in activities:
                fact = activity.get("fact") if isinstance(activity.get("fact"), dict) else {}
                valid_activity_header = (
                    isinstance(activity.get("activity_id"), str) and UUID.fullmatch(activity["activity_id"]) is not None
                    and _decimal_string(activity.get("sequence"), positive=True)
                    and activity.get("lifecycle") in {"closed", "superseded"}
                    and isinstance(activity.get("pair_ref"), (str, type(None)))
                    and (activity.get("pair_ref") is None or UUID.fullmatch(activity["pair_ref"]) is not None)
                    and isinstance(fact.get("kind"), str)
                )
                if fact.get("kind") == "review_action":
                    valid_review = (
                        valid_activity_header and expected_task_category == "b_cross_scope"
                        and _is_verified_b_conflict_review_action(activity)
                    )
                    if not valid_review:
                        valid = False
                        integrity_issues.append({"item_id": item_id,
                                                 "field": "owner_receipt.audit_review_action_invalid"})
                    else:
                        b_conflict_probe_review_action = True
                        review_action_count += 1
                    continue
                valid_activity = (
                    valid_activity_header
                    and fact.get("kind") == "post_paste_attempt"
                    and isinstance(fact.get("verified_paste"), bool)
                    and isinstance(fact.get("outcome"), str)
                    and isinstance(fact.get("registration"), dict)
                    and fact["registration"].get("status") in {"checking", "started", "skipped"}
                    and (fact["registration"].get("reason") is None
                         or isinstance(fact["registration"].get("reason"), str))
                    and isinstance(fact.get("candidate_decision"), str)
                    and isinstance(fact.get("candidate_store"), dict)
                    and set(fact["candidate_store"]) == {"status", "reason"}
                    and fact["candidate_store"].get("status") in {"completed", "failed", "not_attempted"}
                    and (fact["candidate_store"].get("reason") is None
                         or isinstance(fact["candidate_store"].get("reason"), str))
                    and isinstance(fact.get("settings_store"), dict)
                    and set(fact["settings_store"]) == {"status", "reason"}
                    and fact["settings_store"].get("status") in {"completed", "failed", "not_attempted"}
                    and (fact["settings_store"].get("reason") is None
                         or isinstance(fact["settings_store"].get("reason"), str))
                    and (fact.get("classification") is None or isinstance(fact.get("classification"), dict))
                    and isinstance(fact.get("attribution"), str)
                )
                if not valid_activity:
                    valid = False
                    integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_activity_invalid"})
                    continue
                if activity.get("lifecycle") == "superseded" and fact.get("outcome") != "stale_or_cancelled":
                    valid = False
                    integrity_issues.append({"item_id": item_id,
                                             "field": "owner_receipt.audit_superseded_outcome_invalid"})
                    continue
                post_paste_activities.append(activity)
                if activity.get("lifecycle") == "superseded" and fact.get("outcome") == "stale_or_cancelled":
                    observer_parts.append(_normal_audit_observer_capture(
                        fact, activity.get("pair_ref"), pairs_by_ref, item_id, integrity_issues,
                    ))
                    continue
                if activity.get("lifecycle") != "closed":
                    observer_parts.append({"status": None, "candidate_refs": [],
                                           "reason": "activity_not_closed", "source": "bounded_audit"})
                    continue
                observer_parts.append(_normal_audit_observer_capture(
                    fact, activity.get("pair_ref"), pairs_by_ref, item_id, integrity_issues,
                ))
            if review_action_count > 1:
                valid = False
                integrity_issues.append({"item_id": item_id,
                                         "field": "owner_receipt.audit_review_action_duplicate"})
            if not post_paste_activities:
                valid = False
                integrity_issues.append({"item_id": item_id,
                                         "field": "owner_receipt.audit_capture_activity_missing"})
            if primary_activity is not None and primary_activity not in post_paste_activities:
                valid = False
                integrity_issues.append({"item_id": item_id,
                                         "field": "owner_receipt.audit_primary_activity_not_capture"})
            unsupported_multi = [part for part in observer_parts
                                 if part["status"] is None and part["reason"] == "multiple_token_capture_unsupported"]
            ambiguous_parts = [part for part in observer_parts
                               if part["status"] is None and part["reason"] != "multiple_token_capture_unsupported"]
            if not observer_parts:
                observer_status = None
                observer_reason = "audit_capture_activity_missing"
            elif unsupported_multi and any(part["status"] is True for part in observer_parts) and not ambiguous_parts:
                observer_status = True
                observer_reason = "grouped_multiple_token_candidates"
            elif any(part["status"] is None for part in observer_parts):
                observer_status = None
                observer_reason = next(part["reason"] for part in observer_parts if part["status"] is None)
            else:
                observer_status = any(part["status"] is True for part in observer_parts)
                observer_reason = "grouped_audit_activities" if len(observer_parts) > 1 else observer_parts[0]["reason"]
            pending_candidate_writes = sorted(
                {entry["pair_ref"]: entry for part in observer_parts
                 for entry in part.get("pending_candidate_writes", [])}.values(),
                key=lambda entry: entry["pair_ref"],
            )
            pending_write_refs = sorted(entry["pair_ref"] for entry in pending_candidate_writes)
            if (audit_pair_inventories is None
                    or pending_write_refs != changed_inactive_candidate_refs):
                valid = False
                integrity_issues.append({"item_id": item_id,
                                         "field": "owner_receipt.audit_pending_pair_delta_mismatch"})
            observer_fact = {
                "status": observer_status,
                "capture_id": document.get("activity_id"),
                "candidate_refs": sorted({ref for part in observer_parts for ref in part.get("candidate_refs", [])}),
                "reason": observer_reason,
                "source": "bounded_audit",
                "human_acceptance_authority": True,
                "activity_ids": [entry["activity_id"] for entry in post_paste_activities],
                "b_conflict_probe_review_action": b_conflict_probe_review_action,
                "pending_candidate_writes": pending_candidate_writes,
                "changed_inactive_candidate_refs": changed_inactive_candidate_refs,
                "audit_pair_inventories": audit_pair_inventories,
                "candidate_persistence_statuses": [part.get("candidate_persistence_status") for part in observer_parts],
            }

        if primary_activity is not None:
            if any(document.get(key) != expected for key, expected in {
                "audit_runtime_id": after.get("runtime_id"),
                "activity_id": primary_activity.get("activity_id"),
                "sequence": primary_activity.get("sequence"),
            }.items()):
                valid = False
                integrity_issues.append({"item_id": item_id, "field": "owner_receipt.audit_activity_binding_mismatch"})
        else:
            valid = False
        if not valid or primary_activity is None:
            return None, None, None, None
        runtime_id = after["runtime_id"]
        activity_id = primary_activity["activity_id"]
        generation = primary_activity["sequence"]
        return f"test:{runtime_id}:{after['observer_epoch']}", activity_id, generation, observer_fact

    integrity_issues.append({"item_id": item_id, "field": "owner_receipt.source_unsupported"})
    return None, None, None, None


def verify_source_capture(
    row: dict, assignment: dict, manifest: dict, verified: dict, artifact_root: Path | None,
    missing: list[dict], integrity_issues: list[dict],
) -> tuple[str | None, bool, str | None, dict | None]:
    item_id = assignment["trial_id"]
    capture = row.get("source_capture")
    if not isinstance(capture, dict):
        add_missing(missing, item_id, "source_capture")
        capture = {}
    source_doc = verified_bound_receipt(
        artifact_root, row, "source_capture", manifest, assignment, verified, missing, integrity_issues,
        artifact_type="voiceflow_dictionary_learning_source_capture_receipt",
    )
    owner_doc = verified_bound_receipt(
        artifact_root, row, "owner_receipt", manifest, assignment, verified, missing, integrity_issues,
        artifact_type="voiceflow_dictionary_learning_owner_receipt",
    )
    expected_build = (manifest.get("freeze_facts") or {}).get("build_sha256")
    owner_identity, owner_capture_id, owner_generation, observer_fact = verify_owner_capture_receipt(
        owner_doc, item_id, integrity_issues, expected_build, task_owner_binding(manifest, assignment),
        assignment["category"],
    )
    capture_identity = None
    if source_doc is not None and owner_identity is not None:
        owner_reference = (row.get("provenance") or {}).get("owner_receipt", {})
        source = owner_doc.get("source")
        identity_matches = source_doc.get("source") == source and source_doc.get("owner_receipt_sha256") == owner_reference.get("sha256")
        if source == "voiceflow_research_stdio_v1":
            identity_matches = identity_matches and all(source_doc.get(key) == owner_doc.get(key)
                                                        for key in ("run_id", "case_id", "job_id", "recording_generation"))
        elif source == "voiceflow_installable_test_bounded_audit_v1":
            identity_matches = identity_matches and all(source_doc.get(key) == owner_doc.get(key)
                                                        for key in ("audit_runtime_id", "activity_id", "sequence"))
        if not identity_matches:
            integrity_issues.append({"item_id": item_id, "field": "source_capture_owner_binding_mismatch"})
            observer_fact = None
        else:
            if capture.get("capture_id") is None:
                add_missing(missing, item_id, "source_capture.capture_id")
            elif capture.get("capture_id") != owner_capture_id:
                integrity_issues.append({"item_id": item_id, "field": "ledger_capture_id_mismatch"})
            if capture.get("recording_generation") is None:
                add_missing(missing, item_id, "source_capture.recording_generation")
            elif capture.get("recording_generation") != owner_generation:
                integrity_issues.append({"item_id": item_id, "field": "ledger_capture_generation_mismatch"})
        if capture.get("live_capture_verified") == "no":
            integrity_issues.append({"item_id": item_id, "field": "live_capture_not_confirmed"})
        elif capture.get("live_capture_verified") in {None, "unknown"}:
            add_missing(missing, item_id, "source_capture.live_capture_verified")
        elif capture.get("live_capture_verified") != "yes":
            integrity_issues.append({"item_id": item_id, "field": "source_capture.live_capture_verified_invalid"})
        stage_hashes = row.get("stage_hashes") if isinstance(row.get("stage_hashes"), dict) else {}
        stage_hashes_match = True
        for field in ("raw_asr_sha256", "inserted_text_sha256"):
            actual = stage_hashes.get(field)
            if actual is None:
                add_missing(missing, item_id, f"stage_hashes.{field}")
                stage_hashes_match = False
            elif actual != source_doc.get(field):
                integrity_issues.append({"item_id": item_id, "field": f"source_capture.{field}_mismatch"})
                stage_hashes_match = False
        inline_identity_match = (
            capture.get("capture_id") == owner_capture_id
            and capture.get("recording_generation") == owner_generation
            and capture.get("live_capture_verified") == "yes"
        )
        terminal_events = [event for event in owner_doc.get("events", [])
                           if isinstance(event, dict) and event.get("event") == "session_terminal"]
        if terminal_events:
            terminal = terminal_events[-1]
            if (row.get("outcomes", {}).get("actual_delivery_verified") == "yes"
                    and (terminal.get("outcome") == "cancelled"
                         or terminal.get("side_effect_boundary") == "no_delivery_write_observed")):
                integrity_issues.append({"item_id": item_id, "field": "owner_terminal_delivery_contradiction"})
                inline_identity_match = False
        if identity_matches and inline_identity_match and stage_hashes_match:
            capture_identity = owner_identity
        else:
            observer_fact = None
    else:
        observer_fact = None

    stage_hashes = row.get("stage_hashes")
    hashes_complete = isinstance(stage_hashes, dict)
    if not hashes_complete:
        add_missing(missing, item_id, "stage_hashes")
        return capture_identity, False, owner_reference_digest(row), observer_fact
    for field in ("raw_asr_sha256", "inserted_text_sha256"):
        digest = stage_hashes.get(field)
        if not isinstance(digest, str) or SHA256.fullmatch(digest) is None:
            hashes_complete = False
            if digest is None:
                add_missing(missing, item_id, f"stage_hashes.{field}")
            else:
                add_missing(missing, item_id, f"stage_hashes.{field}_invalid")
                integrity_issues.append({"item_id": item_id, "field": f"stage_hashes.{field}_invalid"})
    return capture_identity, hashes_complete, owner_reference_digest(row), observer_fact


def owner_reference_digest(row: dict) -> str | None:
    reference = ((row.get("provenance") or {}).get("owner_receipt"))
    digest = reference.get("sha256") if isinstance(reference, dict) else None
    return digest if isinstance(digest, str) and SHA256.fullmatch(digest) else None


def validate_human_and_task_receipts(
    row: dict, assignment: dict, manifest: dict, artifact_root: Path | None, verified: dict,
    outcomes: dict, observer_fact: dict | None, owner_receipt_digest: str | None,
    missing: list[dict], integrity_issues: list[dict],
) -> tuple[bool, bool, bool, dict | None, dict | None, dict | None, bool, bool]:
    item_id = assignment["trial_id"]
    human = verified_bound_receipt(
        artifact_root, row, "human_attestation", manifest, assignment, verified, missing, integrity_issues,
        artifact_type="voiceflow_dictionary_learning_human_attestation", trial_bound=False,
    )
    independent = verified_bound_receipt(
        artifact_root, row, "independent_label", manifest, assignment, verified, missing, integrity_issues,
        artifact_type="voiceflow_dictionary_learning_independent_label",
    )
    terminal = verified_bound_receipt(
        artifact_root, row, "target_terminal", manifest, assignment, verified, missing, integrity_issues,
        artifact_type="voiceflow_dictionary_learning_terminal_receipt",
    )
    actor_id = assignment["operator_id"] if manifest["stage"] == "technical" else assignment["participant_id"]
    human_ok = human is not None
    if human is not None:
        expected_role = "operator" if manifest["stage"] == "technical" else "participant"
        checks = {
            "subject_role": expected_role,
            "human_data_attested": True,
            "local_only_confirmed": True,
            "research_authorization_confirmed": True,
            "consent_confirmed": True,
        }
        if manifest["stage"] == "technical":
            actor_roles = {entry["operator_id"]: entry["implementation_role"] for entry in manifest["roster"]}
            checks["implementation_role"] = actor_roles[actor_id]
        for key, expected in checks.items():
            if human.get(key) != expected:
                integrity_issues.append({"item_id": item_id, "field": f"human_attestation.{key}",
                                         "expected": expected, "observed": human.get(key)})
                human_ok = False
    independent_ok = independent is not None
    observer_label_core_ok = independent is not None
    observer_label_binding_ok = manifest["stage"] == "technical"
    if independent is not None:
        expected_spec = (manifest.get("task_specs") or {}).get(item_id)
        labeler_id = independent.get("labeler_id")
        if not isinstance(labeler_id, str) or SAFE_ID.fullmatch(labeler_id) is None or labeler_id == actor_id or independent.get("reviewed_before_outcome") is not True \
                or independent.get("reference_sha256") != expected_spec:
            integrity_issues.append({"item_id": item_id, "field": "independent_label_not_independent_or_unbound"})
            independent_ok = False
            observer_label_core_ok = False
        if manifest["stage"] == "technical":
            label_fields = ["alias_exposure", "alias_eligible", "alias_applied_correctly", "new_unintended_mutation",
                            "actual_correction", "eligible_correction", "observer_captured", "observer_correct"]
            if assignment["category"] == "correct_boundary":
                label_fields.append("source_text_correct")
            if assignment["category"] == "b_cross_scope":
                label_fields += ["wrong_scope_alias_applied", "b_conflict_probe", "explicit_conflict", "a_rule_unchanged"]
            if assignment["category"] == "forget_restart":
                label_fields += ["forgot_effective", "applied_after_forget", "resurrection"]
                if assignment["substage"] == "after_restart":
                    label_fields.append("restart_verified")
        else:
            label_fields = ["intended_output_correct", "manual_edit_required", "critical_misedit",
                            "cross_scope_application", "unauthorized_output", "state_contamination", "resurrection"]
        for key in label_fields:
            observer_outcome_field = manifest["stage"] == "technical" and key in {
                "observer_captured", "observer_correct",
            }
            if key not in independent:
                add_missing(missing, item_id, f"independent_label.{key}")
                if not observer_outcome_field:
                    independent_ok = False
                if key in {"actual_correction", "eligible_correction"}:
                    observer_label_core_ok = False
                if key in {"observer_captured", "observer_correct"}:
                    observer_label_binding_ok = False
            elif independent.get(key) != outcomes.get(key):
                integrity_issues.append({"item_id": item_id, "field": f"independent_label.{key}_mismatch"})
                if not observer_outcome_field:
                    independent_ok = False
                if key in {"actual_correction", "eligible_correction"}:
                    observer_label_core_ok = False
                if key in {"observer_captured", "observer_correct"}:
                    observer_label_binding_ok = False
        if manifest["stage"] == "technical":
            required_observer_fields = {
                "observer_owner_receipt_sha256", "observer_capture_id", "observer_candidate_refs",
            }
            for key in sorted(required_observer_fields):
                if key not in independent:
                    add_missing(missing, item_id, f"independent_label.{key}")
                    observer_label_binding_ok = False
            label_refs = independent.get("observer_candidate_refs")
            refs_valid = (
                isinstance(label_refs, list)
                and all(isinstance(value, str) and UUID.fullmatch(value) is not None for value in label_refs)
                and len(set(label_refs)) == len(label_refs)
                and label_refs == sorted(label_refs)
            )
            if not refs_valid:
                integrity_issues.append({"item_id": item_id, "field": "independent_label.observer_candidate_refs_invalid"})
                observer_label_binding_ok = False
            expected_capture_id = observer_fact.get("capture_id") if isinstance(observer_fact, dict) else None
            expected_refs = observer_fact.get("candidate_refs") if isinstance(observer_fact, dict) else None
            owner_binding_ok = (
                isinstance(owner_receipt_digest, str)
                and SHA256.fullmatch(owner_receipt_digest) is not None
                and independent.get("observer_owner_receipt_sha256") == owner_receipt_digest
                and isinstance(expected_capture_id, str)
                and independent.get("observer_capture_id") == expected_capture_id
                and isinstance(row.get("source_capture"), dict)
                and row["source_capture"].get("capture_id") == expected_capture_id
            )
            if not owner_binding_ok:
                if observer_fact is None or expected_capture_id is None:
                    add_missing(missing, item_id, "independent_label.observer_owner_capture_binding")
                else:
                    integrity_issues.append({"item_id": item_id,
                                             "field": "independent_label.observer_owner_capture_binding_mismatch"})
                    observer_label_binding_ok = False
            capture_status = observer_fact.get("status") if isinstance(observer_fact, dict) else None
            authority = observer_fact.get("human_acceptance_authority") is True if isinstance(observer_fact, dict) else False
            if not authority or capture_status is None:
                add_missing(missing, item_id, "owner_receipt.observer_fact")
                observer_label_binding_ok = False
            elif outcomes.get("observer_captured") != ("yes" if capture_status else "no"):
                integrity_issues.append({"item_id": item_id, "field": "independent_label.observer_capture_fact_mismatch",
                                         "expected": "yes" if capture_status else "no",
                                         "observed": outcomes.get("observer_captured")})
                observer_label_binding_ok = False
            if isinstance(expected_refs, list) and refs_valid and label_refs != expected_refs:
                integrity_issues.append({"item_id": item_id, "field": "independent_label.observer_candidate_refs_mismatch",
                                         "expected": expected_refs, "observed": label_refs})
                observer_label_binding_ok = False
            if outcomes.get("observer_correct") == "yes":
                if (capture_status is not True or outcomes.get("eligible_correction") != "yes"
                        or not isinstance(expected_refs, list) or not expected_refs or label_refs != expected_refs
                        or not owner_binding_ok or not authority):
                    integrity_issues.append({"item_id": item_id, "field": "independent_label.observer_correct_without_bound_candidate"})
                    observer_label_binding_ok = False
            if capture_status is True and isinstance(expected_refs, list) and len(expected_refs) != 1:
                if outcomes.get("observer_correct") != "unknown":
                    add_missing(missing, item_id, "independent_label.observer_correctness_for_grouped_or_unpersisted_capture")
                observer_label_binding_ok = False
            observer_label_binding_ok = bool(
                observer_label_binding_ok and owner_binding_ok and refs_valid and authority and capture_status is not None
                and isinstance(expected_refs, list) and label_refs == expected_refs
            )
            if assignment["category"] == "b_cross_scope":
                probe_review_action = observer_fact.get("b_conflict_probe_review_action") is True \
                    if isinstance(observer_fact, dict) else False
                probe_claimed = outcomes.get("b_conflict_probe")
                probe_result = independent.get("b_conflict_probe_result")
                if probe_claimed == "yes" and not probe_review_action:
                    add_missing(missing, item_id, "owner_receipt.b_conflict_probe_review_action")
                if probe_claimed == "yes" and probe_result is None:
                    add_missing(missing, item_id, "independent_label.b_conflict_probe_result")
                elif probe_result is not None and (
                        probe_result != B_CONFLICT_RESULT_CODE or not probe_review_action or probe_claimed != "yes"):
                    integrity_issues.append({"item_id": item_id,
                                             "field": "independent_label.b_conflict_probe_receipt_mismatch"})
                    independent_ok = False
                if probe_review_action and probe_claimed == "no":
                    integrity_issues.append({"item_id": item_id,
                                             "field": "independent_label.b_conflict_probe_receipt_mismatch"})
                    independent_ok = False
                elif probe_review_action and probe_claimed == "unknown":
                    add_missing(missing, item_id, "independent_label.b_conflict_probe")
    terminal_ok = terminal is not None
    if terminal is not None:
        terminal_field = "independent_terminal_verified" if manifest["stage"] == "technical" else "terminal_evidence_verified"
        for key, outcome_key in (("terminal_verified", terminal_field), ("actual_delivery_verified", "actual_delivery_verified")):
            if terminal.get(key) != outcomes.get(outcome_key):
                integrity_issues.append({"item_id": item_id, "field": f"target_terminal.{key}_mismatch"})
                terminal_ok = False
    return (human_ok, independent_ok, terminal_ok, independent, terminal, human,
            observer_label_core_ok, observer_label_binding_ok)


SAFETY_HARM_FIELDS = (
    "critical_misedit", "cross_scope_application", "unauthorized_output",
    "state_contamination", "resurrection",
)


def validate_safety_observation(
    row: dict, assignment: dict, manifest: dict, artifact_root: Path | None,
    verified: dict, outcomes: dict, missing: list[dict], integrity_issues: list[dict],
) -> tuple[bool, bool]:
    """Verify a separate independent safety observation, without terminal readback."""
    document = verified_bound_receipt(
        artifact_root, row, "safety_observation", manifest, assignment, verified, missing, integrity_issues,
        artifact_type="voiceflow_dictionary_learning_safety_observation",
    )
    if document is None:
        return False, False
    actor_id = assignment["participant_id"]
    labeler_id = document.get("labeler_id")
    valid = (
        isinstance(labeler_id, str) and SAFE_ID.fullmatch(labeler_id) is not None
        and labeler_id != actor_id and document.get("safety_review_complete") is True
    )
    observed = document.get("outcomes")
    if not isinstance(observed, dict) or set(observed) != set(SAFETY_HARM_FIELDS):
        valid = False
        integrity_issues.append({"item_id": assignment["trial_id"], "field": "safety_observation.outcomes_shape_invalid"})
        observed = {}
    for field in SAFETY_HARM_FIELDS:
        if observed.get(field) != outcomes.get(field):
            valid = False
            integrity_issues.append({"item_id": assignment["trial_id"],
                                     "field": f"safety_observation.{field}_mismatch"})
    if not valid:
        integrity_issues.append({"item_id": assignment["trial_id"], "field": "safety_observation_not_independent_or_incomplete"})
    complete = valid and all(outcomes.get(field) in {"yes", "no"} for field in SAFETY_HARM_FIELDS)
    return valid, complete


def state_snapshot_fingerprint(document: dict) -> str:
    fields = ("global_dictionary_sha256", "rule_inventory_sha256", "active_rule_inventory_sha256",
              "inactive_candidate_inventory_sha256", "tombstone_inventory_sha256",
              "active_rule_families", "effective_hint_sha256", "passive_learning_enabled", "target_rule_state")
    return sha256_json({key: document.get(key) for key in fields})


def active_state_fingerprint(document: dict) -> str:
    """Fingerprint effective state while keeping inactive candidate audit separate."""
    fields = ("global_dictionary_sha256", "active_rule_inventory_sha256", "tombstone_inventory_sha256",
              "active_rule_families", "effective_hint_sha256", "passive_learning_enabled", "target_rule_state")
    return sha256_json({key: document.get(key) for key in fields})


def validate_state_receipts(
    row: dict, assignment: dict, manifest: dict, artifact_root: Path | None, verified: dict,
    missing: list[dict], integrity_issues: list[dict], observer_fact: dict | None = None,
) -> tuple[bool, bool, dict]:
    documents = {}
    snapshots_complete = True
    snapshot_invalid = False
    for boundary, role in (("before", "state_before"), ("after", "state_after")):
        document = verified_bound_receipt(
            artifact_root, row, role, manifest, assignment, verified, missing, integrity_issues,
            artifact_type="voiceflow_dictionary_learning_state_snapshot",
        )
        documents[boundary] = document
        if document is None:
            continue
        if document.get("boundary") != boundary:
            snapshot_invalid = True
            integrity_issues.append({"item_id": assignment["trial_id"], "field": f"{role}.boundary_mismatch"})
        for key, expected_value in (("arm", assignment["arm"]), ("state_contract", assignment["state_contract"])):
            if document.get(key) != expected_value:
                snapshot_invalid = True
                integrity_issues.append({"item_id": assignment["trial_id"], "field": f"{role}.{key}_mismatch",
                                         "expected": expected_value, "observed": document.get(key)})
        for field in ("global_dictionary_sha256", "rule_inventory_sha256", "active_rule_inventory_sha256",
                      "inactive_candidate_inventory_sha256", "tombstone_inventory_sha256"):
            if not isinstance(document.get(field), str) or SHA256.fullmatch(document[field]) is None:
                snapshots_complete = False
                add_missing(missing, assignment["trial_id"], f"{role}.{field}")
        active = document.get("active_rule_families")
        if not isinstance(active, list) or any(not isinstance(value, str) or SAFE_ID.fullmatch(value) is None for value in active):
            snapshots_complete = False
            add_missing(missing, assignment["trial_id"], f"{role}.active_rule_families")
        if not isinstance(document.get("passive_learning_enabled"), bool):
            snapshots_complete = False
            add_missing(missing, assignment["trial_id"], f"{role}.passive_learning_enabled")
        try:
            validate_sha256(document.get("effective_hint_sha256"), f"{role}.effective_hint")
        except PilotError:
            snapshots_complete = False
            add_missing(missing, assignment["trial_id"], f"{role}.effective_hint_sha256")
        if document.get("target_rule_state") not in {"present", "absent", "forgotten", "not_applicable"}:
            snapshots_complete = False
            add_missing(missing, assignment["trial_id"], f"{role}.target_rule_state")
        if document.get("state_fingerprint_sha256") != state_snapshot_fingerprint(document):
            snapshot_invalid = True
            integrity_issues.append({"item_id": assignment["trial_id"], "field": f"{role}.fingerprint_mismatch"})
        if document.get("active_state_fingerprint_sha256") != active_state_fingerprint(document):
            snapshot_invalid = True
            integrity_issues.append({"item_id": assignment["trial_id"],
                                     "field": f"{role}.active_fingerprint_mismatch"})
    snapshots = {
        boundary: {
            "fingerprint": (document or {}).get("state_fingerprint_sha256"),
            "active_fingerprint": (document or {}).get("active_state_fingerprint_sha256"),
            "target_rule_state": (document or {}).get("target_rule_state"),
        }
        for boundary, document in documents.items()
    }
    if any(documents[key] is None for key in ("before", "after")):
        return False, False, snapshots
    if snapshot_invalid:
        return True, False, snapshots
    if not snapshots_complete:
        return False, False, snapshots
    before, after = documents["before"], documents["after"]
    failures = []
    native_pair_inventories = (
        observer_fact.get("audit_pair_inventories") if isinstance(observer_fact, dict) else None
    )
    if isinstance(native_pair_inventories, dict):
        for boundary, document in (("before", before), ("after", after)):
            native_inventory = native_pair_inventories.get(boundary)
            if not isinstance(native_inventory, dict):
                failures.append(f"{boundary}_native_pair_inventory_missing")
                continue
            for field in ("rule_inventory_sha256", "active_rule_inventory_sha256",
                          "inactive_candidate_inventory_sha256", "tombstone_inventory_sha256"):
                if document.get(field) != native_inventory.get(field):
                    failures.append(f"{boundary}_{field}_not_bound_to_native_audit")
    effective_fields = ("global_dictionary_sha256", "active_rule_inventory_sha256", "tombstone_inventory_sha256",
                        "active_rule_families", "effective_hint_sha256", "passive_learning_enabled",
                        "target_rule_state")
    for field in effective_fields:
        if before.get(field) != after.get(field):
            failures.append(field)
    full_rule_inventory_changed = before.get("rule_inventory_sha256") != after.get("rule_inventory_sha256")
    candidate_inventory_changed = (
        before.get("inactive_candidate_inventory_sha256") != after.get("inactive_candidate_inventory_sha256")
    )
    if full_rule_inventory_changed != candidate_inventory_changed:
        failures.append("candidate_inventory_projection_mismatch")
    elif full_rule_inventory_changed:
        candidate_refs = observer_fact.get("candidate_refs") if isinstance(observer_fact, dict) else None
        pending_writes = observer_fact.get("pending_candidate_writes") if isinstance(observer_fact, dict) else None
        pending_write_refs = (
            [entry.get("pair_ref") for entry in pending_writes if isinstance(entry, dict)]
            if isinstance(pending_writes, list) else []
        )
        observed_candidate_write = (
            manifest["stage"] == "technical"
            and assignment["category"] in {"a_positive", "b_cross_scope"}
            and assignment["state_contract"] == "no_rule_or_dictionary_mutation"
            and isinstance(observer_fact, dict) and observer_fact.get("source") == "bounded_audit"
            and observer_fact.get("human_acceptance_authority") is True
            and observer_fact.get("status") is True
            and isinstance(candidate_refs, list) and len(candidate_refs) > 0
            and all(isinstance(value, str) and UUID.fullmatch(value) is not None for value in candidate_refs)
            and len(pending_write_refs) > 0 and len(set(pending_write_refs)) == len(pending_write_refs)
            and set(pending_write_refs).issubset(set(candidate_refs))
            and pending_write_refs == observer_fact.get("changed_inactive_candidate_refs")
            and all(
                isinstance(entry, dict) and set(entry) == {"pair_ref", "status", "reason"}
                and isinstance(entry.get("pair_ref"), str) and UUID.fullmatch(entry["pair_ref"]) is not None
                and entry.get("status") == "completed" and entry.get("reason") == "candidate_db_returned"
                for entry in pending_writes
            )
            and before.get("passive_learning_enabled") is True
            and after.get("passive_learning_enabled") is True
        )
        if not observed_candidate_write:
            failures.append("inactive_candidate_change_without_isolated_observer_evidence")
    if manifest["stage"] == "benefit":
        expected_active = sorted({
            assignment_row["family_id"] for assignment_row in manifest["assignments"]
            if assignment_row["participant_id"] == assignment["participant_id"]
            and assignment_row["category"] == "target" and assignment_row["arm"] == "teach"
        })
        for document in (before, after):
            active_families = document.get("active_rule_families")
            if (not isinstance(active_families, list) or len(active_families) != 5
                    or len(set(active_families)) != 5 or sorted(active_families) != expected_active
                    or document.get("passive_learning_enabled") is not False):
                failures.append("benefit_arm_state")
            if assignment["category"] == "target":
                family_should_be_active = assignment["arm"] == "teach"
                if (assignment["family_id"] in (active_families if isinstance(active_families, list) else [])) != family_should_be_active:
                    failures.append("benefit_family_arm_contamination")
        if manifest.get("freeze_status") == "frozen":
            freeze_facts = manifest["freeze_facts"]
            participant_id = assignment["participant_id"]
            expected_state = freeze_facts["state_sha256_by_participant"][participant_id]
            expected_hint = freeze_facts["effective_hint_sha256_by_participant"][participant_id]
            for boundary, document in documents.items():
                if document.get("state_fingerprint_sha256") != expected_state:
                    failures.append(f"{boundary}_snapshot_not_bound_to_frozen_state")
                if document.get("effective_hint_sha256") != expected_hint:
                    failures.append(f"{boundary}_hint_not_bound_to_frozen_hint")
    elif assignment["state_contract"] == "forgotten_rule_remains_absent":
        if before.get("target_rule_state") != "forgotten" or after.get("target_rule_state") != "forgotten":
            failures.append("forgotten_rule_not_absent")
    if failures:
        integrity_issues.append({"item_id": assignment["trial_id"], "field": "declared_state_transition_violated",
                                 "contract": assignment["state_contract"], "differences": sorted(set(failures))})
        return True, False, snapshots
    return True, True, snapshots


def validate_technical_state_sequence(manifest: dict, stage_data: dict[str, dict], integrity_issues: list[dict]) -> list[str]:
    """Bind ordinary trials to the frozen state and allow one Forget boundary."""
    if manifest.get("freeze_status") != "frozen":
        return []
    initial = manifest["freeze_facts"]["initial_state_sha256"]
    violations = []
    forgotten_fingerprints = set()
    for item in stage_data.values():
        assignment = item["assignment"]
        if not item["state_receipts_complete"]:
            continue
        snapshots = item["state_snapshots"]
        before, after = snapshots["before"], snapshots["after"]
        fingerprints = (before["active_fingerprint"], after["active_fingerprint"])
        if assignment["category"] != "forget_restart":
            if any(value != initial for value in fingerprints):
                violations.append(assignment["trial_id"])
        else:
            if any(value == initial for value in fingerprints) or any(
                snapshot["target_rule_state"] != "forgotten" for snapshot in (before, after)
            ):
                violations.append(assignment["trial_id"])
            forgotten_fingerprints.update(fingerprints)
    complete_forget_items = [item for item in stage_data.values()
                             if item["assignment"]["category"] == "forget_restart"
                             and item["state_receipts_complete"]]
    # The post-Forget and post-restart encounters must share the same observed
    # state, and that state must differ from the frozen initial state.
    if complete_forget_items and len(forgotten_fingerprints) != 1:
        violations.extend(item["assignment"]["trial_id"] for item in complete_forget_items)
    for trial_id in sorted(set(violations)):
        item = stage_data[trial_id]
        item["state_transition_ok"] = False
        item["typed_receipts_complete"] = False
        integrity_issues.append({"item_id": trial_id, "field": "state_sequence_not_bound_to_frozen_or_forget_boundary"})
    return sorted(set(violations))


def validate_auxiliary_receipts(
    row: dict, assignment: dict, manifest: dict, artifact_root: Path | None,
    verified: dict, missing: list[dict], integrity_issues: list[dict],
) -> tuple[bool, bool]:
    item_id = assignment["check_id"]
    binding = {
        "batch_id": manifest["batch_id"],
        "manifest_fingerprint": manifest_fingerprint(manifest),
        "phase_id": manifest["phase_id"],
        "check_id": item_id,
        "operator_id": assignment["operator_id"],
        "check_type": assignment["check_type"],
    }
    documents = {}
    complete = True
    valid = True
    for role, artifact_type in (("owner_receipt", "voiceflow_dictionary_learning_auxiliary_owner_receipt"),
                                ("independent_label", "voiceflow_dictionary_learning_auxiliary_label")):
        reference = (row.get("provenance") or {}).get(role)
        document = read_artifact_document(artifact_root, reference, item_id, role,
                                          bool(verified.get(role)), integrity_issues)
        documents[role] = document
        if document is None:
            complete = False
            continue
        expected = {"artifact_type": artifact_type, "schema_version": 1, **binding}
        for key, value in expected.items():
            if document.get(key) != value:
                integrity_issues.append({"item_id": item_id, "field": f"{role}.{key}_mismatch",
                                         "expected": value, "observed": document.get(key)})
                valid = False
    owner, label = documents["owner_receipt"], documents["independent_label"]
    outcome_names = ("observer_active", "mid_ime_state_write", "wrong_field_write")
    row_outcomes = row.get("outcomes") if isinstance(row.get("outcomes"), dict) else {}
    for name in outcome_names:
        if row_outcomes.get(name) not in YES_NO_UNKNOWN:
            add_missing(missing, item_id, f"outcomes.{name}")
            complete = False
    if owner is not None:
        if owner.get("observed_phase_id") != manifest["phase_id"] or owner.get("local_observation_verified") is not True:
            integrity_issues.append({"item_id": item_id, "field": "auxiliary_owner_observation_unverified"})
            valid = False
        owner_outcomes = owner.get("outcomes") if isinstance(owner.get("outcomes"), dict) else {}
        if any(owner_outcomes.get(name) != row_outcomes.get(name) for name in outcome_names):
            integrity_issues.append({"item_id": item_id, "field": "auxiliary_owner_outcome_mismatch"})
            valid = False
    if label is not None:
        labeler = label.get("labeler_id")
        if not isinstance(labeler, str) or SAFE_ID.fullmatch(labeler) is None \
                or labeler == assignment["operator_id"] or label.get("reviewed_before_outcome") is not True:
            integrity_issues.append({"item_id": item_id, "field": "auxiliary_label_not_independent"})
            valid = False
        label_outcomes = label.get("outcomes") if isinstance(label.get("outcomes"), dict) else {}
        if any(label_outcomes.get(name) != row_outcomes.get(name) for name in outcome_names):
            integrity_issues.append({"item_id": item_id, "field": "auxiliary_label_outcome_mismatch"})
            valid = False

    snapshots = {}
    for boundary, role in (("before", "state_before"), ("after", "state_after")):
        reference = (row.get("provenance") or {}).get(role)
        document = read_artifact_document(artifact_root, reference, item_id, role,
                                          bool(verified.get(role)), integrity_issues)
        snapshots[boundary] = document
        if document is None:
            complete = False
            continue
        expected = {
            "artifact_type": "voiceflow_dictionary_learning_state_snapshot",
            "schema_version": 1, **binding,
            "boundary": boundary,
            "arm": "technical",
            "state_contract": "no_dictionary_write_during_auxiliary",
        }
        for key, value in expected.items():
            if document.get(key) != value:
                integrity_issues.append({"item_id": item_id, "field": f"{role}.{key}_mismatch",
                                         "expected": value, "observed": document.get(key)})
                valid = False
        for field in ("global_dictionary_sha256", "rule_inventory_sha256", "active_rule_inventory_sha256",
                      "inactive_candidate_inventory_sha256", "tombstone_inventory_sha256"):
            if not isinstance(document.get(field), str) or SHA256.fullmatch(document[field]) is None:
                add_missing(missing, item_id, f"{role}.{field}")
                complete = False
        active = document.get("active_rule_families")
        if not isinstance(active, list) or any(not isinstance(value, str) or SAFE_ID.fullmatch(value) is None for value in active):
            add_missing(missing, item_id, f"{role}.active_rule_families")
            complete = False
        if not isinstance(document.get("passive_learning_enabled"), bool) \
                or document.get("target_rule_state") not in {"present", "absent", "forgotten", "not_applicable"}:
            add_missing(missing, item_id, f"{role}.state_fields")
            complete = False
        try:
            validate_sha256(document.get("effective_hint_sha256"), f"{role}.effective_hint")
        except PilotError:
            add_missing(missing, item_id, f"{role}.effective_hint_sha256")
            complete = False
        if document.get("state_fingerprint_sha256") != state_snapshot_fingerprint(document):
            integrity_issues.append({"item_id": item_id, "field": f"{role}.fingerprint_mismatch"})
            valid = False
        if document.get("active_state_fingerprint_sha256") != active_state_fingerprint(document):
            integrity_issues.append({"item_id": item_id, "field": f"{role}.active_fingerprint_mismatch"})
            valid = False
    if snapshots["before"] is not None and snapshots["after"] is not None:
        fields = ("global_dictionary_sha256", "rule_inventory_sha256", "active_rule_inventory_sha256",
                  "inactive_candidate_inventory_sha256", "tombstone_inventory_sha256",
                  "active_rule_families", "effective_hint_sha256", "passive_learning_enabled", "target_rule_state")
        if any(snapshots["before"].get(key) != snapshots["after"].get(key) for key in fields):
            integrity_issues.append({"item_id": item_id, "field": "auxiliary_state_changed"})
            valid = False
    if any(issue.get("item_id") == item_id and issue.get("field", "").endswith("_receipt_invalid")
           for issue in integrity_issues):
        valid = False
    if any(issue.get("item_id") == item_id and issue.get("field", "").startswith("provenance.")
           and (issue.get("field", "").endswith("sha256_mismatch")
                or issue.get("field", "").endswith("path_invalid")) for issue in missing):
        valid = False
    return complete, valid


def normalized_outcome(row: dict, name: str, missing: list[dict], item_id: str) -> str:
    outcomes = row.get("outcomes")
    value = outcomes.get(name) if isinstance(outcomes, dict) else None
    if value not in YES_NO_UNKNOWN:
        add_missing(missing, item_id, f"outcomes.{name}")
        return "unknown"
    return value


def normalized_integrity(row: dict, missing: list[dict], item_id: str) -> str:
    value = row.get("state_integrity")
    if value not in {"clean", "contaminated", "unknown"}:
        add_missing(missing, item_id, "state_integrity")
        return "unknown"
    if value == "unknown":
        add_missing(missing, item_id, "state_integrity")
    return value


def verify_assignment_observation(
    row: dict,
    assignment: dict,
    manifest: dict,
    missing: list[dict],
    integrity_issues: list[dict],
) -> None:
    item_id = assignment["trial_id"]
    observed_phase = row.get("observed_phase_id")
    if observed_phase is None:
        add_missing(missing, item_id, "observed_phase_id")
    elif observed_phase != manifest["phase_id"]:
        integrity_issues.append({"item_id": item_id, "field": "observed_phase_id", "expected": manifest["phase_id"], "observed": observed_phase})
    expected_arm = assignment["arm"]
    observed_arm = row.get("observed_arm")
    if observed_arm is None:
        add_missing(missing, item_id, "observed_arm")
    elif observed_arm != expected_arm:
        integrity_issues.append({"item_id": item_id, "field": "observed_arm", "expected": expected_arm, "observed": observed_arm})
    if manifest["stage"] == "technical":
        expected_operator = assignment["operator_id"]
        if row.get("observed_operator_id") is None:
            add_missing(missing, item_id, "observed_operator_id")
        elif row.get("observed_operator_id") != expected_operator:
            integrity_issues.append({"item_id": item_id, "field": "observed_operator_id", "expected": expected_operator, "observed": row.get("observed_operator_id")})
    else:
        expected_participant = assignment["participant_id"]
        if row.get("observed_participant_id") is None:
            add_missing(missing, item_id, "observed_participant_id")
        elif row.get("observed_participant_id") != expected_participant:
            integrity_issues.append({"item_id": item_id, "field": "observed_participant_id", "expected": expected_participant, "observed": row.get("observed_participant_id")})
    if normalized_integrity(row, missing, item_id) == "contaminated":
        integrity_issues.append({"item_id": item_id, "field": "state_integrity", "observed": "contaminated"})


def task_spec_matches(manifest: dict, row: dict, assignment: dict, integrity_issues: list[dict]) -> None:
    reference = (row.get("provenance") or {}).get("task_spec")
    expected = (manifest.get("task_specs") or {}).get(assignment["trial_id"])
    if isinstance(reference, dict) and isinstance(reference.get("sha256"), str) and expected is not None:
        if reference["sha256"] != expected:
            integrity_issues.append({"item_id": assignment["trial_id"], "field": "task_spec_sha256",
                                     "expected": expected, "observed": reference["sha256"]})


def technical_observer_summary(stage_data: dict[str, dict], integrity_issues: list[dict]) -> dict:
    actual_count = eligible_count = captured_count = correct_count = eligible_captured_count = 0
    actual_unknown = eligible_unknown = capture_unknown = eligible_capture_unknown = 0
    correctness_unknown = false_positive_captures = 0
    unknown_trials = []
    violations = []
    for item_id, item in stage_data.items():
        label = item.get("independent_label") if item.get("observer_label_core_ok") is True else None
        actual = label.get("actual_correction") if isinstance(label, dict) else "unknown"
        eligible = label.get("eligible_correction") if isinstance(label, dict) else "unknown"
        observer = item.get("observer_fact") if isinstance(item.get("observer_fact"), dict) else None
        capture_status = (
            observer.get("status")
            if item.get("distinct_recording") is True and observer and observer.get("human_acceptance_authority") is True
            else None
        )
        captured = "yes" if capture_status is True else "no" if capture_status is False else "unknown"
        correct = label.get("observer_correct") if isinstance(label, dict) else "unknown"
        label_capture = label.get("observer_captured") if isinstance(label, dict) else "unknown"
        label_binding_ok = item.get("observer_label_binding_ok") is True
        if actual == "yes":
            actual_count += 1
        elif actual not in {"yes", "no"}:
            actual_unknown += 1
        if eligible == "yes":
            eligible_count += 1
        elif eligible not in {"yes", "no"}:
            eligible_unknown += 1
        if captured == "yes":
            captured_count += 1
        elif captured == "unknown":
            capture_unknown += 1
        if eligible == "yes" and captured == "yes":
            eligible_captured_count += 1
        if eligible == "yes" and captured == "unknown":
            eligible_capture_unknown += 1
        if (label_capture in {"yes", "no"} and captured in {"yes", "no"}
                and label_capture != captured):
            violations.append(item_id)
            integrity_issues.append({"item_id": item_id, "field": "observer_capture_fact_conflict"})
        bad = ((eligible == "yes" and actual == "no")
               or (correct == "yes" and (captured != "yes" or eligible != "yes")))
        if bad:
            if item_id not in violations:
                violations.append(item_id)
            integrity_issues.append({"item_id": item_id, "field": "observer_correction_nesting_violation"})
        if captured == "yes" and eligible == "no":
            false_positive_captures += 1
        correctness_valid = (
            captured == "yes" and eligible in {"yes", "no"} and correct in {"yes", "no"}
            and label_binding_ok and item.get("observer_label_core_ok") is True
            and item.get("independent_label_ok") is True
            and isinstance(observer, dict) and isinstance(observer.get("candidate_refs"), list)
            and len(observer["candidate_refs"]) == 1
        )
        if correctness_valid:
            if correct == "yes":
                correct_count += 1
        elif captured == "yes":
            correctness_unknown += 1
        if (actual not in {"yes", "no"} or eligible not in {"yes", "no"}
                or captured == "unknown" or (captured == "yes" and not correctness_valid)):
            unknown_trials.append(item_id)
    recall_known = eligible_unknown == 0 and eligible_capture_unknown == 0
    recall = eligible_captured_count / eligible_count if recall_known and eligible_count else None
    precision_known = capture_unknown == 0 and correctness_unknown == 0
    precision = correct_count / captured_count if precision_known and captured_count else None
    return {
        "denominator": len(stage_data),
        "actual_corrections": actual_count,
        "actual_correction_unknown": actual_unknown,
        "eligible_corrections": eligible_count,
        "eligible_correction_unknown": eligible_unknown,
        "captured_corrections": captured_count,
        "eligible_captured_corrections": eligible_captured_count,
        "eligible_capture_recall": recall,
        "capture_unknown": capture_unknown,
        "eligible_capture_unknown": eligible_capture_unknown,
        "correct_captures": correct_count,
        "correctness_unknown": correctness_unknown,
        "false_positive_captures": false_positive_captures,
        "capture_precision": precision,
        "unknown": len(unknown_trials),
        "unknown_trials": unknown_trials,
        "violations": violations,
    }


def verified_b_conflict_probe(item: dict) -> bool:
    """Return true only for a task-bound, independently labelled native B probe."""
    outcomes = item.get("outcomes") if isinstance(item.get("outcomes"), dict) else {}
    label = item.get("independent_label") if isinstance(item.get("independent_label"), dict) else {}
    observer = item.get("observer_fact") if isinstance(item.get("observer_fact"), dict) else {}
    row = item.get("row") if isinstance(item.get("row"), dict) else {}
    source_capture = row.get("source_capture") if isinstance(row.get("source_capture"), dict) else {}
    capture_id = observer.get("capture_id")
    receipt_digest = item.get("source_receipt_digest")
    return bool(
        outcomes.get("b_conflict_probe") == "yes"
        and item.get("independent_label_ok") is True
        and label.get("b_conflict_probe_result") == B_CONFLICT_RESULT_CODE
        and observer.get("source") == "bounded_audit"
        and observer.get("human_acceptance_authority") is True
        and observer.get("b_conflict_probe_review_action") is True
        and item.get("distinct_recording") is True
        and isinstance(receipt_digest, str) and SHA256.fullmatch(receipt_digest) is not None
        and label.get("observer_owner_receipt_sha256") == receipt_digest
        and isinstance(capture_id, str)
        and label.get("observer_capture_id") == capture_id
        and source_capture.get("capture_id") == capture_id
    )


def analyze_technical(manifest: dict, evidence: dict, artifact_root: Path | None) -> dict:
    missing: list[dict] = []
    integrity_issues: list[dict] = []
    evidence_rows, row_ids = fill_event_map(evidence.get("trials"), "trial_id", manifest["assignments"])
    aux_rows, aux_ids = fill_event_map(evidence.get("auxiliary_observations"), "check_id", manifest["auxiliary_observations"])
    if row_ids["extra"] or row_ids["duplicate"]:
        integrity_issues.append({"item_id": "trial_ledger", "field": "row_ids", **row_ids})
    if aux_ids["extra"] or aux_ids["duplicate"]:
        integrity_issues.append({"item_id": "auxiliary_ledger", "field": "row_ids", **aux_ids})
    if evidence.get("evidence_class") == "actual_human":
        for field in ("human_data_attested", "local_only_confirmed", "research_authorization_confirmed"):
            if evidence.get(field) is False:
                integrity_issues.append({"item_id": "evidence_header", "field": f"{field}_denied"})
            elif evidence.get(field) is not True:
                add_missing(missing, "evidence_header", field)
    else:
        add_missing(missing, "evidence_header", "evidence_class=actual_human")

    stage_data = {}
    human_attested_actors = set()
    human_attestation_counts = Counter()
    for assignment in manifest["assignments"]:
        item_id = assignment["trial_id"]
        row = evidence_rows.get(item_id, {})
        if not row:
            add_missing(missing, item_id, "trial_record")
            row = {}
        verify_assignment_observation(row, assignment, manifest, missing, integrity_issues)
        provenance = row.get("provenance") if isinstance(row.get("provenance"), dict) else {}
        verified = {}
        for role in TECHNICAL_TEMPLATE_ROLES:
            verified[role] = verify_artifact_ref(artifact_root, provenance.get(role), item_id, role, missing)
        task_spec_matches(manifest, row, assignment, integrity_issues)
        outcomes = {name: normalized_outcome(row, name, missing, item_id) for name in outcome_fields(manifest, assignment)}
        capture_identity, stage_hashes_complete, source_receipt_digest, observer_fact = verify_source_capture(
            row, assignment, manifest, verified, artifact_root, missing, integrity_issues,
        )
        if isinstance(observer_fact, dict) and observer_fact.get("human_acceptance_authority") is False:
            capture_identity = None
            add_missing(missing, item_id, "owner_receipt.research_stdio_not_human_capture_authority")
        (human_ok, independent_label_ok, terminal_receipt_ok, independent_label, terminal_receipt,
         human_receipt, observer_label_core_ok, observer_label_binding_ok) = validate_human_and_task_receipts(
            row, assignment, manifest, artifact_root, verified, outcomes, observer_fact, source_receipt_digest,
            missing, integrity_issues,
        )
        terminal_receipts_ok = independent_label_ok and terminal_receipt_ok
        state_receipts_complete, state_transition_ok, state_snapshots = validate_state_receipts(
            row, assignment, manifest, artifact_root, verified, missing, integrity_issues, observer_fact,
        )
        if human_ok:
            human_attested_actors.add(assignment["operator_id"])
            human_attestation_counts[assignment["operator_id"]] += 1
        typed_receipts_complete = bool(
            capture_identity is not None and human_ok and terminal_receipts_ok
            and state_receipts_complete and state_transition_ok
        )
        stage_data[item_id] = {"assignment": assignment, "row": row, "outcomes": outcomes, "provenance": verified,
                               "capture_identity": capture_identity, "stage_hashes_complete": stage_hashes_complete,
                               "source_receipt_digest": source_receipt_digest, "human_receipt_ok": human_ok,
                               "terminal_receipts_ok": terminal_receipts_ok,
                               "state_receipts_complete": state_receipts_complete,
                               "state_transition_ok": state_transition_ok,
                               "state_snapshots": state_snapshots,
                               "typed_receipts_complete": typed_receipts_complete,
                               "independent_label": independent_label,
                               "independent_label_ok": independent_label_ok,
                               "observer_fact": observer_fact,
                               "observer_label_core_ok": observer_label_core_ok,
                               "observer_label_binding_ok": observer_label_binding_ok,
                               "terminal_receipt": terminal_receipt,
                               "human_receipt": human_receipt}

    validate_technical_state_sequence(manifest, stage_data, integrity_issues)

    capture_rows: dict[str, list[str]] = defaultdict(list)
    for item in stage_data.values():
        if item["capture_identity"] is not None:
            capture_rows[item["capture_identity"]].append(item["assignment"]["trial_id"])
    duplicate_captures = [ids for ids in capture_rows.values() if len(ids) > 1]
    duplicate_capture_ids = {trial_id for group in duplicate_captures for trial_id in group}
    owner_receipts: dict[str, list[str]] = defaultdict(list)
    for item in stage_data.values():
        digest = item["source_receipt_digest"]
        if digest is not None:
            owner_receipts[digest].append(item["assignment"]["trial_id"])
    duplicate_owner_receipts = [ids for ids in owner_receipts.values() if len(ids) > 1]
    capture_not_live = any(issue.get("field") == "live_capture_not_confirmed" for issue in integrity_issues)
    for item in stage_data.values():
        item["distinct_recording"] = bool(
            item["capture_identity"] is not None
            and item["assignment"]["trial_id"] not in duplicate_capture_ids
            and item["stage_hashes_complete"]
        )

    by_category: dict[str, list[dict]] = defaultdict(list)
    for value in stage_data.values():
        by_category[value["assignment"]["category"]].append(value)
    gates = {}

    all_terminal = all(
        item["outcomes"].get("independent_terminal_verified") == "yes"
        and item["outcomes"].get("actual_delivery_verified") == "yes"
        and item["terminal_receipts_ok"]
        for item in stage_data.values()
    )
    terminal_unknown = any(
        item["outcomes"].get("independent_terminal_verified") == "unknown"
        or item["outcomes"].get("actual_delivery_verified") == "unknown"
        or not item["terminal_receipts_ok"]
        for item in stage_data.values()
    )
    gates["all_40_real_deliveries_and_independent_terminals"] = gate(
        "pending" if terminal_unknown else ("pass" if all_terminal else "fail"),
        denominator=40,
        complete=sum(
            item["outcomes"].get("independent_terminal_verified") == "yes"
            and item["outcomes"].get("actual_delivery_verified") == "yes"
            and item["terminal_receipts_ok"]
            for item in stage_data.values()
        ),
    )
    provenance_status, provenance_complete, provenance_invalid = provenance_gate_status(stage_data, missing, COMMON_PROVENANCE_ROLES, integrity_issues)
    gates["positive_and_negative_stage_provenance"] = gate(
        provenance_status, denominator=40, complete=provenance_complete,
        required_artifact_roles=list(COMMON_PROVENANCE_ROLES), invalid_references=provenance_invalid,
    )

    a_rows = by_category["a_positive"]
    a_exposure = [item["outcomes"]["alias_exposure"] for item in a_rows]
    a_eligible = [item["outcomes"]["alias_eligible"] for item in a_rows]
    a_valid = [item for item in a_rows if item["outcomes"]["alias_exposure"] == "yes"
               and item["outcomes"]["alias_eligible"] == "yes" and item["distinct_recording"]]
    a_unverified = [item["assignment"]["trial_id"] for item in a_rows
                    if item["outcomes"]["alias_exposure"] == "yes" and item["outcomes"]["alias_eligible"] == "yes"
                    and not item["distinct_recording"]]
    a_inconsistent = [item["assignment"]["trial_id"] for item in a_rows
                      if item["outcomes"]["alias_exposure"] == "no" and item["outcomes"]["alias_eligible"] == "yes"]
    a_unknown = sum(value == "unknown" for value in a_exposure) + sum(value == "unknown" for value in a_eligible)
    a_possible = sum(
        item["outcomes"]["alias_exposure"] in {"yes", "unknown"}
        and item["outcomes"]["alias_eligible"] in {"yes", "unknown"}
        and not (item["outcomes"]["alias_exposure"] == "yes"
                 and item["outcomes"]["alias_eligible"] == "yes"
                 and item["outcomes"]["alias_applied_correctly"] == "no")
        and item["assignment"]["trial_id"] not in duplicate_capture_ids
        for item in a_rows
    )
    a_bad = [item["assignment"]["trial_id"] for item in a_valid if item["outcomes"]["alias_applied_correctly"] == "no"]
    a_incomplete = [item["assignment"]["trial_id"] for item in a_valid if item["outcomes"]["alias_applied_correctly"] == "unknown"
                    or item["outcomes"]["independent_terminal_verified"] != "yes"
                    or item["outcomes"]["actual_delivery_verified"] != "yes"]
    if a_bad or a_inconsistent:
        a_status = "fail"
    elif a_possible < 8:
        a_status = "fail"
    elif a_unknown or a_incomplete or a_unverified:
        a_status = "pending"
    elif len(a_valid) < 8:
        a_status = "fail"
    else:
        a_status = "pass"
    gates["a_positive"] = gate(a_status, denominator=12, eligible_aliases=len(a_valid),
                               maximum_reachable_eligible_aliases=a_possible, minimum_eligible_aliases=8,
                               unknown_exposure_or_eligibility=a_unknown, incorrect_eligible_trials=a_bad,
                               incomplete_eligible_trials=a_incomplete, unverified_or_duplicate_exposures=a_unverified,
                               inconsistent_exposure_labels=a_inconsistent)

    b_rows = by_category["b_cross_scope"]
    b_exposure = [item["outcomes"]["alias_exposure"] for item in b_rows]
    b_valid = [item for item in b_rows if item["outcomes"]["alias_exposure"] == "yes" and item["distinct_recording"]]
    b_unverified = [item["assignment"]["trial_id"] for item in b_rows
                    if item["outcomes"]["alias_exposure"] == "yes" and not item["distinct_recording"]]
    b_unknown = sum(value == "unknown" for value in b_exposure)
    b_possible = sum(
        item["outcomes"]["alias_exposure"] in {"yes", "unknown"}
        and item["assignment"]["trial_id"] not in duplicate_capture_ids
        for item in b_rows
    )
    b_wrong = [item["assignment"]["trial_id"] for item in b_rows if item["outcomes"]["wrong_scope_alias_applied"] == "yes"]
    b_unknown_application = [item["assignment"]["trial_id"] for item in b_rows if item["outcomes"]["wrong_scope_alias_applied"] == "unknown"]
    claimed_conflict_probes = [item for item in b_rows if item["outcomes"]["b_conflict_probe"] == "yes"]
    conflict_probes = [item for item in claimed_conflict_probes if verified_b_conflict_probe(item)]
    conflict_unverified = [item["assignment"]["trial_id"] for item in claimed_conflict_probes
                           if not verified_b_conflict_probe(item)]
    conflict_errors = []
    for item in conflict_probes:
        for field in ("explicit_conflict", "a_rule_unchanged"):
            if item["outcomes"][field] == "no":
                conflict_errors.append({"trial_id": item["assignment"]["trial_id"], "field": field})
    b_conflict_issue_fields = {
        "owner_receipt.audit_review_action_invalid",
        "owner_receipt.audit_review_action_duplicate",
        "independent_label.b_conflict_probe_receipt_mismatch",
        "independent_label.observer_owner_capture_binding_mismatch",
    }
    conflict_receipt_conflicts = [
        issue for issue in integrity_issues
        if issue.get("item_id") in {item["assignment"]["trial_id"] for item in b_rows}
        and issue.get("field") in b_conflict_issue_fields
    ]
    conflict_pending = not conflict_probes or any(
        item["outcomes"][field] == "unknown" for item in conflict_probes for field in ("explicit_conflict", "a_rule_unchanged")
    ) or bool(conflict_unverified)
    if b_wrong or conflict_errors or conflict_receipt_conflicts:
        b_status = "fail"
    elif b_possible < 4:
        b_status = "fail"
    elif b_unknown or b_unknown_application or conflict_pending or b_unverified:
        b_status = "pending"
    elif len(b_valid) < 4:
        b_status = "fail"
    else:
        b_status = "pass"
    gates["b_cross_scope"] = gate(b_status, denominator=8, actual_alias_exposures=len(b_valid),
                                  maximum_reachable_exposures=b_possible, minimum_exposures=4,
                                  unknown_exposure=b_unknown, wrong_scope_applications=b_wrong,
                                  unknown_application_trials=b_unknown_application, conflict_probes=len(conflict_probes),
                                  claimed_conflict_probes=len(claimed_conflict_probes),
                                  unverified_conflict_probes=conflict_unverified,
                                  conflict_receipt_conflicts=conflict_receipt_conflicts,
                                  conflict_errors=conflict_errors, unverified_or_duplicate_exposures=b_unverified)

    boundary_rows = by_category["correct_boundary"]
    boundary_bad = []
    boundary_unknown = []
    for item in boundary_rows:
        trial_id = item["assignment"]["trial_id"]
        outcomes = item["outcomes"]
        if outcomes["source_text_correct"] == "no" or outcomes["new_unintended_mutation"] == "yes":
            boundary_bad.append(trial_id)
        if outcomes["source_text_correct"] == "unknown" or outcomes["new_unintended_mutation"] == "unknown":
            boundary_unknown.append(trial_id)
    gates["correct_and_boundary"] = gate(
        "fail" if boundary_bad else ("pending" if boundary_unknown else "pass"),
        denominator=8, source_correct_and_unmodified=8 - len(boundary_bad) - len(boundary_unknown),
        violating_trials=boundary_bad, unknown_trials=boundary_unknown,
    )

    forget_rows = by_category["forget_restart"]
    forget_bad = []
    forget_unknown = []
    for item in forget_rows:
        assignment = item["assignment"]
        outcomes = item["outcomes"]
        required = ["alias_exposure", "forgot_effective", "applied_after_forget", "resurrection"]
        if assignment["substage"] == "after_restart":
            required.append("restart_verified")
        if outcomes["applied_after_forget"] == "yes" or outcomes["resurrection"] == "yes":
            forget_bad.append(assignment["trial_id"])
        elif assignment["substage"] == "after_restart" and outcomes["restart_verified"] == "no":
            forget_bad.append(assignment["trial_id"])
        elif any(outcomes[field] == "no" for field in ("alias_exposure", "forgot_effective")):
            forget_bad.append(assignment["trial_id"])
        elif outcomes["alias_exposure"] == "yes" and not item["distinct_recording"]:
            forget_unknown.append(assignment["trial_id"])
        if any(outcomes[field] == "unknown" for field in required):
            forget_unknown.append(assignment["trial_id"])
    gates["forget_and_restart"] = gate(
        "fail" if forget_bad else ("pending" if forget_unknown else "pass"),
        denominator=12,
        after_forget=Counter(row["assignment"]["substage"] for row in forget_rows)["after_forget"],
        after_restart=Counter(row["assignment"]["substage"] for row in forget_rows)["after_restart"],
        violating_trials=forget_bad,
        unknown_trials=forget_unknown,
        resurrection_count=sum(item["outcomes"]["resurrection"] == "yes" for item in forget_rows),
    )

    aux_bad = []
    aux_unknown = []
    aux_provenance_complete = 0
    aux_provenance_invalid = []
    for assignment in manifest.get("auxiliary_observations", []):
        check_id = assignment["check_id"]
        row = aux_rows.get(check_id)
        if not row:
            add_missing(missing, check_id, "auxiliary_record")
            aux_unknown.append(check_id)
            continue
        if row.get("observed_phase_id") is None:
            add_missing(missing, check_id, "observed_phase_id")
            aux_unknown.append(check_id)
        elif row.get("observed_phase_id") != manifest["phase_id"]:
            integrity_issues.append({"item_id": check_id, "field": "observed_phase_id", "expected": manifest["phase_id"], "observed": row.get("observed_phase_id")})
        if row.get("observed_operator_id") is None:
            add_missing(missing, check_id, "observed_operator_id")
            aux_unknown.append(check_id)
        elif row.get("observed_operator_id") != assignment["operator_id"]:
            integrity_issues.append({"item_id": check_id, "field": "observed_operator_id", "expected": assignment["operator_id"], "observed": row.get("observed_operator_id")})
        integrity = normalized_integrity(row, missing, check_id)
        if integrity == "contaminated":
            integrity_issues.append({"item_id": check_id, "field": "state_integrity", "observed": integrity})
        elif integrity == "unknown":
            aux_unknown.append(check_id)
        aux_provenance = {
            role: verify_artifact_ref(artifact_root, (row.get("provenance") or {}).get(role), check_id, role, missing)
            for role in AUX_PROVENANCE_ROLES
        }
        aux_receipts_complete, aux_receipts_valid = validate_auxiliary_receipts(
            row, assignment, manifest, artifact_root, aux_provenance, missing, integrity_issues,
        )
        aux_provenance_complete += aux_receipts_complete and aux_receipts_valid
        if not aux_receipts_valid:
            aux_provenance_invalid.append(check_id)
        outcomes = row.get("outcomes") if isinstance(row.get("outcomes"), dict) else {}
        check_values = {name: outcomes.get(name) for name in ("observer_active", "mid_ime_state_write", "wrong_field_write")}
        if any(value == "yes" for value in (check_values["mid_ime_state_write"], check_values["wrong_field_write"])):
            aux_bad.append(check_id)
        if check_values["observer_active"] != "yes" or check_values["mid_ime_state_write"] not in YES_NO_UNKNOWN or check_values["wrong_field_write"] not in YES_NO_UNKNOWN:
            for name, value in check_values.items():
                if value not in YES_NO_UNKNOWN:
                    add_missing(missing, check_id, f"outcomes.{name}")
            aux_unknown.append(check_id)
        elif "unknown" in check_values.values():
            aux_unknown.append(check_id)
    aux_count = Counter(row["check_type"] for row in manifest.get("auxiliary_observations", []))
    aux_status = "fail" if aux_bad or aux_provenance_invalid else (
        "pending" if aux_unknown or aux_provenance_complete < len(manifest.get("auxiliary_observations", [])) else "pass"
    )
    gates["ime_and_focus_observations"] = gate(aux_status,
                                                ime_composition=aux_count["ime_composition"],
                                                minimum_each=5,
                                                focus_switch=aux_count["focus_switch"],
                                                middle_state_or_wrong_field_writes=aux_bad,
                                                unknown_checks=aux_unknown,
                                                complete_local_provenance=aux_provenance_complete,
                                                invalid_local_provenance=aux_provenance_invalid)

    if integrity_issues:
        assignment_status = "fail"
    elif missing:
        assignment_status = "pending"
    else:
        assignment_status = "pass"
    gates["assignment_phase_arm_and_state_integrity"] = gate(
        assignment_status, denominator=40 + len(manifest.get("auxiliary_observations", [])),
        mismatches=integrity_issues,
    )
    distinct_recordings = sum(bool(item["distinct_recording"]) for item in stage_data.values())
    capture_not_live = any(issue.get("field") == "live_capture_not_confirmed" for issue in integrity_issues)
    capture_invalid = any(
        issue.get("field", "").startswith(("owner_receipt.", "source_capture."))
        or issue.get("field") in {"source_capture_owner_binding_mismatch", "ledger_capture_id_mismatch", "ledger_capture_generation_mismatch"}
        for issue in integrity_issues
    )
    gates["unique_real_recordings"] = gate(
        "fail" if duplicate_captures or duplicate_owner_receipts or capture_not_live or capture_invalid else ("pass" if distinct_recordings == 40 else "pending"),
        denominator=40,
        distinct_recordings=distinct_recordings,
        duplicate_trial_groups=duplicate_captures,
        reused_owner_receipt_groups=duplicate_owner_receipts,
        live_capture_not_confirmed=capture_not_live,
    )
    roster = manifest.get("roster", [])
    non_implementers = [person["operator_id"] for person in roster if person.get("implementation_role") == "non_implementer"]
    gates["independent_operator"] = gate(
        "pass" if len(non_implementers) >= 1 else "fail",
        operators=len(roster), declared_non_implementers=non_implementers,
    )
    human_ready = (
        evidence.get("evidence_class") == "actual_human"
        and evidence.get("human_data_attested") is True
        and evidence.get("local_only_confirmed") is True
        and evidence.get("research_authorization_confirmed") is True
        and all(human_attestation_counts[person["operator_id"]] == 20 for person in roster)
    )
    human_denied = any(issue.get("field", "").endswith("_denied") or issue.get("field", "").startswith("human_attestation.")
                       for issue in integrity_issues)
    human_status = "fail" if human_denied else ("pass" if human_ready else "pending")
    gates["actual_human_evidence"] = gate(human_status,
                                          evidence_class=evidence.get("evidence_class"), human_data_attested=evidence.get("human_data_attested"),
                                          local_only_confirmed=evidence.get("local_only_confirmed"),
                                          research_authorization_confirmed=evidence.get("research_authorization_confirmed"),
                                          actor_attestations=sorted(human_attested_actors),
                                          per_operator_complete=dict(human_attestation_counts),
                                          expected_per_operator=20)
    state_complete = sum(bool(item["state_receipts_complete"]) for item in stage_data.values())
    state_bad = [item["assignment"]["trial_id"] for item in stage_data.values()
                 if item["state_receipts_complete"] and not item["state_transition_ok"]]
    gates["declared_state_transitions"] = gate(
        "fail" if state_bad else ("pass" if state_complete == len(stage_data) else "pending"),
        denominator=len(stage_data), complete=state_complete, violating_trials=state_bad,
    )
    observer = technical_observer_summary(stage_data, integrity_issues)
    gates["observer_correction_accounting"] = gate(
        "fail" if observer["violations"] else ("pending" if observer["unknown"] else "pass"),
        **observer,
    )
    return assemble_report(manifest, evidence, gates, missing, integrity_issues)


def assemble_report(manifest: dict, evidence: dict, gates: dict, missing: list[dict], integrity_issues: list[dict], **extra) -> dict:
    statuses = [item["status"] for item in gates.values()]
    status = "fail" if "fail" in statuses else ("pending" if "pending" in statuses else "pass")
    missing_counts = Counter(item["field"] for item in missing)
    artifact_bindings = []
    for collection_name, id_key in (("trials", "trial_id"), ("auxiliary_observations", "check_id"), ("participant_time_costs", "participant_id")):
        collection = evidence.get(collection_name)
        if not isinstance(collection, list):
            continue
        for row in collection:
            if not isinstance(row, dict):
                continue
            item_id = row.get(id_key)
            provenance = row.get("provenance")
            if collection_name == "participant_time_costs":
                provenance = {"time_costs": provenance}
            if not isinstance(provenance, dict):
                continue
            for role, reference in provenance.items():
                if isinstance(reference, dict) and isinstance(reference.get("sha256"), str):
                    artifact_bindings.append({"item_id": item_id, "role": role, "sha256": reference["sha256"]})
    artifact_bindings.sort(key=lambda item: (str(item["item_id"]), item["role"], item["sha256"]))
    return {
        "artifact_type": "voiceflow_dictionary_learning_pilot_report",
        "schema_version": 1,
        "tool_version": TOOL_VERSION,
        "stage": manifest["stage"],
        "c1_boundary": manifest.get("c1_boundary"),
        "batch_id": manifest["batch_id"],
        "manifest_fingerprint": manifest_fingerprint(manifest),
        "task_catalog_sha256": manifest.get("task_catalog_sha256"),
        "evidence_ledger_sha256": sha256_json(evidence),
        "artifact_binding_count": len(artifact_bindings),
        "artifact_bindings_sha256": sha256_json(artifact_bindings),
        "evidence_class": evidence.get("evidence_class"),
        "status": status,
        "human_gate_status": gates.get("actual_human_evidence", gates.get("actual_human_local_consent_by_participant", {})).get("status", "pending"),
        "assigned_trial_count": len(manifest["assignments"]),
        "observed_trial_records": len(evidence.get("trials", [])) if isinstance(evidence.get("trials"), list) else 0,
        "gates": gates,
        "missing_evidence": {
            "items": missing,
            "counts_by_field": dict(sorted(missing_counts.items())),
            "integrity_issues": integrity_issues,
        },
        **extra,
    }


def apply_close_mode(report: dict, close: bool) -> dict:
    if report.get("evidence_class") == "unit_fixture" or report.get("status") == "fixture_only":
        return report
    report["close_mode"] = "closed" if close else "open"
    gates = report.get("gates")
    if not close or not isinstance(gates, dict):
        return report
    for value in gates.values():
        if isinstance(value, dict) and value.get("status") == "pending":
            value["status"] = "fail"
            value["closed_with_missing_evidence"] = True
    statuses = [value.get("status") for value in gates.values() if isinstance(value, dict)]
    report["status"] = "fail" if "fail" in statuses else ("pending" if "pending" in statuses else "pass")
    human_gate = gates.get("actual_human_evidence", gates.get("actual_human_local_consent_by_participant", {}))
    report["human_gate_status"] = human_gate.get("status", "pending") if isinstance(human_gate, dict) else "pending"
    return report


def summarize_benefit_rows(rows: list[dict]) -> dict:
    harms = ("critical_misedit", "cross_scope_application", "unauthorized_output", "state_contamination", "resurrection")
    correct = sum(
        item["outcomes"].get("intended_output_correct") == "yes"
        and item["outcomes"].get("actual_delivery_verified") == "yes"
        and item["outcomes"].get("manual_edit_required") == "no"
        and item.get("terminal_complete") is True
        and item.get("distinct_recording") is True
        for item in rows
    )
    unknown = sum(
        any(value == "unknown" for value in item["outcomes"].values())
        or not item.get("terminal_complete") or not item.get("distinct_recording")
        for item in rows
    )
    return {
        "allocated": len(rows),
        "terminal_complete": sum(item.get("terminal_complete") is True for item in rows),
        "distinct_live_captures": sum(item.get("distinct_recording") is True for item in rows),
        "correct_delivery_without_manual_edit": correct,
        "manual_edit_required": sum(item["outcomes"].get("manual_edit_required") == "yes" for item in rows),
        "unknown_or_unreadable": unknown,
        "harms_yes": {key: sum(item["outcomes"].get(key) == "yes" for item in rows) for key in harms},
        "harms_unknown": {key: sum(item["outcomes"].get(key) == "unknown" for item in rows) for key in harms},
    }


def safety_negative_gate(stage_data: dict[str, dict]) -> dict:
    negative = [item for item in stage_data.values() if item["assignment"]["category"].startswith("safety_")]
    harms = ("critical_misedit", "cross_scope_application", "unauthorized_output", "state_contamination", "resurrection")
    counts_yes = {key: sum(item["outcomes"].get(key) == "yes" for item in negative) for key in harms}
    counts_unknown = {key: sum(item["outcomes"].get(key) == "unknown" for item in negative) for key in harms}
    complete = sum(
        item.get("terminal_complete") is True and item.get("distinct_recording") is True
        and item.get("typed_receipts_complete") is True
        and all(item["outcomes"].get(key) in {"yes", "no"} for key in harms)
        for item in negative
    )
    if any(counts_yes.values()):
        status = "fail"
    elif complete != len(negative) or any(counts_unknown.values()):
        status = "pending"
    else:
        status = "pass"
    return gate(status, denominator=len(negative), complete=complete,
                critical_harms_yes=counts_yes, critical_harms_unknown=counts_unknown,
                categories=dict(Counter(item["assignment"]["category"] for item in negative)))


def safety_harm_gate(stage_data: dict[str, dict]) -> dict:
    counts_yes = {key: sum(item["outcomes"].get(key) == "yes" for item in stage_data.values())
                  for key in SAFETY_HARM_FIELDS}
    counts_unknown = {key: sum(item["outcomes"].get(key) == "unknown" for item in stage_data.values())
                      for key in SAFETY_HARM_FIELDS}
    complete = sum(item.get("safety_observation_complete") is True for item in stage_data.values())
    invalid = [item["assignment"]["trial_id"] for item in stage_data.values()
               if item.get("safety_observation_valid") is not True]
    unknown_observations = [item["assignment"]["trial_id"] for item in stage_data.values()
                            if item.get("safety_observation_valid") is True
                            and item.get("safety_observation_complete") is not True]
    if any(counts_yes.values()):
        status = "fail"
    elif any(counts_unknown.values()) or complete != len(stage_data):
        status = "pending"
    else:
        status = "pass"
    return gate(status, denominator=len(stage_data), complete=complete,
                independently_observed=len(stage_data) - len(invalid),
                missing_or_invalid_observations=invalid,
                unknown_safety_observations=unknown_observations,
                critical_harms_yes=counts_yes, critical_harms_unknown=counts_unknown,
                terminal_receipts_required=False)


def analyze_benefit(manifest: dict, evidence: dict, artifact_root: Path | None, *, close: bool = False) -> dict:
    missing: list[dict] = []
    integrity_issues: list[dict] = []
    assignments_by_id = {row["trial_id"]: row for row in manifest["assignments"]}
    evidence_rows, row_ids = fill_event_map(evidence.get("trials"), "trial_id", manifest["assignments"])
    if row_ids["extra"] or row_ids["duplicate"]:
        integrity_issues.append({"item_id": "trial_ledger", "field": "row_ids", **row_ids})
    if evidence.get("evidence_class") == "actual_human":
        for field in ("human_data_attested", "local_only_confirmed", "research_authorization_confirmed"):
            if evidence.get(field) is False:
                integrity_issues.append({"item_id": "evidence_header", "field": f"{field}_denied"})
            elif evidence.get(field) is not True:
                add_missing(missing, "evidence_header", field)
        authorizations = evidence.get("participant_authorization")
        if not isinstance(authorizations, dict):
            add_missing(missing, "evidence_header", "participant_authorization")
        else:
            if set(authorizations) != set(manifest["participants"]):
                integrity_issues.append({"item_id": "evidence_header", "field": "participant_authorization_roster_mismatch",
                                         "expected": sorted(manifest["participants"]), "observed": sorted(authorizations)})
            for participant_id in manifest["participants"]:
                if authorizations.get(participant_id) is False:
                    integrity_issues.append({"item_id": f"participant:{participant_id}", "field": "participant_authorization_denied"})
                elif authorizations.get(participant_id) is not True:
                    add_missing(missing, f"participant:{participant_id}", "participant_authorization")
    else:
        add_missing(missing, "evidence_header", "evidence_class=actual_human")

    stage_data = {}
    human_attestation_counts = Counter()
    capture_rows: dict[str, list[str]] = defaultdict(list)
    terminal_complete = 0
    terminal_unknown = 0
    terminal_known_failure = 0
    for trial_id, assignment in assignments_by_id.items():
        row = evidence_rows.get(trial_id, {})
        if not row:
            add_missing(missing, trial_id, "trial_record")
            row = {}
        verify_assignment_observation(row, assignment, manifest, missing, integrity_issues)
        provenance = row.get("provenance") if isinstance(row.get("provenance"), dict) else {}
        verified = {}
        for role in BENEFIT_TEMPLATE_ROLES:
            verified[role] = verify_artifact_ref(artifact_root, provenance.get(role), trial_id, role, missing)
        task_spec_matches(manifest, row, assignment, integrity_issues)
        outcomes = {name: normalized_outcome(row, name, missing, trial_id) for name in outcome_fields(manifest, assignment)}
        capture_identity, stage_hashes_complete, source_receipt_digest, observer_fact = verify_source_capture(
            row, assignment, manifest, verified, artifact_root, missing, integrity_issues,
        )
        if isinstance(observer_fact, dict) and observer_fact.get("human_acceptance_authority") is False:
            capture_identity = None
            add_missing(missing, trial_id, "owner_receipt.research_stdio_not_human_capture_authority")
        (human_ok, independent_label_ok, terminal_receipt_ok, independent_label, terminal_receipt,
         human_receipt, observer_label_core_ok, observer_label_binding_ok) = validate_human_and_task_receipts(
            row, assignment, manifest, artifact_root, verified, outcomes, observer_fact, source_receipt_digest,
            missing, integrity_issues,
        )
        terminal_receipts_ok = independent_label_ok and terminal_receipt_ok
        state_receipts_complete, state_transition_ok, state_snapshots = validate_state_receipts(
            row, assignment, manifest, artifact_root, verified, missing, integrity_issues,
        )
        safety_observation_valid, safety_observation_complete = validate_safety_observation(
            row, assignment, manifest, artifact_root, verified, outcomes, missing, integrity_issues,
        )
        if human_ok:
            human_attestation_counts[assignment["participant_id"]] += 1
        time_value = row.get("completion_ms")
        if assignment["category"] == "target":
            if not isinstance(time_value, int) or isinstance(time_value, bool) or time_value < 0:
                add_missing(missing, trial_id, "completion_ms")
                time_value = None
        if capture_identity is not None:
            capture_rows[capture_identity].append(trial_id)
        terminal_ok = (
            outcomes["terminal_evidence_verified"] == "yes"
            and terminal_receipts_ok
        )
        if terminal_ok:
            terminal_complete += 1
        elif (outcomes["terminal_evidence_verified"] == "no" and terminal_receipts_ok):
            terminal_known_failure += 1
        elif (outcomes["terminal_evidence_verified"] == "unknown"
              or not terminal_receipts_ok):
            terminal_unknown += 1
        stage_data[trial_id] = {"assignment": assignment, "row": row, "outcomes": outcomes,
                                "provenance": verified, "completion_ms": time_value,
                                "terminal_complete": terminal_ok,
                                "capture_identity": capture_identity,
                                "source_receipt_digest": source_receipt_digest,
                                "stage_hashes_complete": stage_hashes_complete,
                                "state_integrity": row.get("state_integrity", "unknown"),
                                "human_receipt_ok": human_ok,
                                "terminal_receipts_ok": terminal_receipts_ok,
                                "terminal_receipt_ok": terminal_receipt_ok,
                                "independent_label_ok": independent_label_ok,
                                "state_receipts_complete": state_receipts_complete,
                                "state_transition_ok": state_transition_ok,
                                "state_snapshots": state_snapshots,
                                "safety_observation_valid": safety_observation_valid,
                                "safety_observation_complete": safety_observation_complete,
                                "typed_receipts_complete": bool(capture_identity is not None and human_ok
                                                                 and terminal_receipts_ok and state_receipts_complete
                                                                 and state_transition_ok),
                                "provenance_typed_receipts_complete": bool(
                                    capture_identity is not None and human_ok and independent_label_ok
                                    and state_receipts_complete and state_transition_ok
                                ),
                                "independent_label": independent_label,
                                "observer_fact": observer_fact,
                                "observer_label_core_ok": observer_label_core_ok,
                                "observer_label_binding_ok": observer_label_binding_ok,
                                "terminal_receipt": terminal_receipt,
                                "human_receipt": human_receipt}

    duplicate_captures = [ids for ids in capture_rows.values() if len(ids) > 1]
    duplicate_capture_ids = {trial_id for group in duplicate_captures for trial_id in group}
    owner_receipts: dict[str, list[str]] = defaultdict(list)
    for item in stage_data.values():
        digest = item["source_receipt_digest"]
        if digest is not None:
            owner_receipts[digest].append(item["assignment"]["trial_id"])
    duplicate_owner_receipts = [ids for ids in owner_receipts.values() if len(ids) > 1]
    capture_not_live = any(issue.get("field") == "live_capture_not_confirmed" for issue in integrity_issues)
    capture_invalid = any(
        issue.get("field", "").startswith(("owner_receipt.", "source_capture."))
        or issue.get("field") in {"source_capture_owner_binding_mismatch", "ledger_capture_id_mismatch", "ledger_capture_generation_mismatch"}
        for issue in integrity_issues
    )
    for item in stage_data.values():
        item["distinct_recording"] = bool(
            item["capture_identity"] is not None
            and item["assignment"]["trial_id"] not in duplicate_capture_ids
            and item["stage_hashes_complete"]
        )
    total = len(manifest["assignments"])
    terminal_coverage = terminal_complete / total if total else None
    coverage_ok = terminal_coverage is not None and terminal_complete * 100 >= 95 * total
    coverage_threshold = (95 * total + 99) // 100
    # Missing terminal evidence stays in the full denominator and can only make
    # coverage pending when those unknowns could still reach the frozen gate.
    if coverage_ok:
        coverage_status = "pass"
    elif terminal_complete + terminal_unknown >= coverage_threshold and not duplicate_captures and not duplicate_owner_receipts:
        coverage_status = "pending"
    else:
        coverage_status = "fail"

    per_person: dict[str, dict] = {}
    teach_success_total = 0
    control_success_total = 0
    teach_unknown_total = 0
    control_unknown_total = 0
    person_deltas = []
    person_net_savings = []
    harm_unknown = Counter()
    harm_yes = Counter()
    for participant_id in manifest["participants"]:
        person_rows = [item for item in stage_data.values() if item["assignment"]["participant_id"] == participant_id]
        target_rows = [item for item in person_rows if item["assignment"]["category"] == "target"]
        arm_counts = {arm: [item for item in target_rows if item["assignment"]["arm"] == arm] for arm in ("teach", "control")}
        person_metrics = {}
        for arm in ("teach", "control"):
            rows = arm_counts[arm]
            successes = 0
            unknown = 0
            for item in rows:
                outcomes = item["outcomes"]
                values = [outcomes[name] for name in ("intended_output_correct", "actual_delivery_verified", "manual_edit_required", "terminal_evidence_verified")]
                if (any(value == "unknown" for value in values) or not item["terminal_complete"]
                    or not item["distinct_recording"]):
                    unknown += 1
                if (outcomes["intended_output_correct"] == "yes"
                    and outcomes["actual_delivery_verified"] == "yes"
                    and outcomes["manual_edit_required"] == "no"
                    and outcomes["terminal_evidence_verified"] == "yes"
                    and item["terminal_complete"] and item["distinct_recording"]):
                    successes += 1
            person_metrics[arm] = {
                "successes": successes,
                "denominator": 10,
                "rate": successes / 10,
                "unknown_outcomes": unknown,
            }
            if arm == "teach":
                teach_success_total += successes
                teach_unknown_total += unknown
            else:
                control_success_total += successes
                control_unknown_total += unknown
        difference_tasks = person_metrics["teach"]["successes"] - person_metrics["control"]["successes"]
        person_deltas.append({"participant_id": participant_id, "difference_tasks": difference_tasks,
                              "observed_delta_pp": difference_tasks * 10,
                              "unknown_target_outcomes": person_metrics["teach"]["unknown_outcomes"]
                              + person_metrics["control"]["unknown_outcomes"]})
        time_metrics = matched_time_savings(target_rows)
        person_net_savings.append({"participant_id": participant_id, **time_metrics})
        per_person[participant_id] = {
            "target_denominator": 20,
            "teach": person_metrics["teach"],
            "control": person_metrics["control"],
            "matched_delta_pp": (person_metrics["teach"]["rate"] - person_metrics["control"]["rate"]) * 100,
            "unknown_target_outcomes": person_metrics["teach"]["unknown_outcomes"]
            + person_metrics["control"]["unknown_outcomes"],
            "net_saved_ms": time_metrics.get("net_saved_ms"),
            "gross_matched_saved_ms": time_metrics.get("gross_matched_saved_ms"),
            "incremental_teach_reteach_undo_cost_ms": time_metrics.get("incremental_cost_ms"),
        }
        for item in person_rows:
            for key in ("critical_misedit", "cross_scope_application", "unauthorized_output", "state_contamination", "resurrection"):
                value = item["outcomes"][key]
                harm_yes[key] += value == "yes"
                harm_unknown[key] += value == "unknown"

    denominator_target = len(manifest["participants"]) * 10
    difference_tasks = teach_success_total - control_success_total
    observed_overall_delta = difference_tasks / denominator_target
    effect_threshold_met = meets_percentage_point_threshold(difference_tasks, denominator_target, 15)
    people_positive = sum(value["difference_tasks"] > 0 for value in person_deltas)
    people_gate_met = people_positive >= len(manifest["participants"]) - 1
    unknown_target_total = teach_unknown_total + control_unknown_total
    if not close and unknown_target_total:
        effect_status = "pending"
    else:
        effect_status = "pass" if effect_threshold_met and people_gate_met else "fail"

    costs = evidence.get("participant_time_costs")
    cost_map: dict[str, dict] = {}
    cost_duplicates = []
    cost_extras = []
    if isinstance(costs, list):
        for row in costs:
            if not isinstance(row, dict) or not isinstance(row.get("participant_id"), str):
                continue
            participant_id = row["participant_id"]
            if participant_id not in manifest["participants"]:
                cost_extras.append(participant_id)
            elif participant_id in cost_map:
                cost_duplicates.append(participant_id)
            else:
                cost_map[participant_id] = row
    if cost_duplicates or cost_extras:
        integrity_issues.append({"item_id": "participant_time_costs", "field": "participant_ids",
                                 "duplicate": sorted(cost_duplicates), "extra": sorted(cost_extras)})
    costs_invalid = bool(cost_duplicates or cost_extras)
    for participant_id in manifest["participants"]:
        if participant_id not in cost_map:
            add_missing(missing, f"participant:{participant_id}", "time_cost_inventory")
    costs_unknown = bool(cost_duplicates or cost_extras)
    for item in person_net_savings:
        person_id = item["participant_id"]
        cost_row = cost_map.get(person_id, {})
        if cost_row.get("inventory_complete") != "yes":
            costs_unknown = True
            if cost_row.get("inventory_complete") == "no":
                costs_invalid = True
                integrity_issues.append({"item_id": f"participant:{person_id}", "field": "time_cost_inventory_incomplete"})
            else:
                add_missing(missing, f"participant:{person_id}", "time_cost_inventory_complete")
            item["net_saved_ms"] = None
            per_person[person_id]["net_saved_ms"] = None
            continue
        durations = [cost_row.get(key) for key in ("teach_ms", "reteach_ms", "undo_ms", "already_in_completion_ms")]
        if any(not isinstance(value, int) or isinstance(value, bool) or value < 0 for value in durations):
            costs_unknown = True
            if any(value is not None and (not isinstance(value, int) or isinstance(value, bool) or value < 0) for value in durations):
                costs_invalid = True
                integrity_issues.append({"item_id": f"participant:{person_id}", "field": "time_cost_duration_invalid"})
            else:
                add_missing(missing, f"participant:{person_id}", "time_cost_duration_ms")
            item["net_saved_ms"] = None
            per_person[person_id]["net_saved_ms"] = None
            continue
        total_cost = sum(durations[:3])
        already_counted = durations[3]
        if already_counted > total_cost:
            integrity_issues.append({"item_id": f"participant:{person_id}", "field": "already_in_completion_ms_exceeds_costs"})
            costs_unknown = True
            costs_invalid = True
            item["net_saved_ms"] = None
            per_person[person_id]["net_saved_ms"] = None
            continue
        cost_provenance = verify_artifact_ref(artifact_root, cost_row.get("provenance"), f"participant:{person_id}", "time_costs", missing)
        cost_receipt = read_artifact_document(artifact_root, cost_row.get("provenance"), f"participant:{person_id}",
                                              "time_costs", cost_provenance, integrity_issues)
        expected_cost_fields = {
            "artifact_type": "voiceflow_dictionary_learning_time_cost_receipt",
            "schema_version": 1,
            "batch_id": manifest["batch_id"],
            "manifest_fingerprint": manifest_fingerprint(manifest),
            "phase_id": manifest["phase_id"],
            "participant_id": person_id,
            "inventory_complete": "yes",
            "teach_ms": durations[0],
            "reteach_ms": durations[1],
            "undo_ms": durations[2],
            "already_in_completion_ms": durations[3],
            "local_only_confirmed": True,
        }
        cost_receipt_ok = cost_receipt is not None and all(cost_receipt.get(key) == value for key, value in expected_cost_fields.items())
        if cost_receipt is not None and not cost_receipt_ok:
            integrity_issues.append({"item_id": f"participant:{person_id}", "field": "time_cost_receipt_mismatch"})
            costs_invalid = True
        if not cost_provenance or not cost_receipt_ok:
            costs_unknown = True
            if any(issue.get("item_id") == f"participant:{person_id}"
                   and (issue.get("field") == "time_costs_receipt_invalid"
                        or (issue.get("field", "").startswith("provenance.time_costs.")
                            and (issue.get("field", "").endswith("sha256_mismatch")
                                 or issue.get("field", "").endswith("path_invalid"))))
                   for issue in missing + integrity_issues):
                costs_invalid = True
            item["net_saved_ms"] = None
            per_person[person_id]["net_saved_ms"] = None
            continue
        incremental_cost = total_cost - already_counted
        if item.get("gross_matched_saved_ms") is None:
            costs_unknown = True
            item["net_saved_ms"] = None
            per_person[person_id]["net_saved_ms"] = None
            continue
        item["incremental_cost_ms"] = incremental_cost
        item["net_saved_ms"] = calculate_net_saved_ms(item["gross_matched_saved_ms"], total_cost, already_counted)
        per_person[person_id]["net_saved_ms"] = item["net_saved_ms"]
        per_person[person_id]["incremental_teach_reteach_undo_cost_ms"] = incremental_cost
    net_values = [row["net_saved_ms"] for row in person_net_savings if isinstance(row.get("net_saved_ms"), int)]
    if costs_invalid:
        savings_status = "fail"
        median_net_saved = statistics.median(net_values) if net_values else None
    elif costs_unknown or len(net_values) != len(manifest["participants"]):
        savings_status = "pending"
        median_net_saved = statistics.median(net_values) if net_values else None
    else:
        median_net_saved = statistics.median(net_values)
        savings_status = "pass" if median_net_saved > 0 else "fail"

    harm_gate = safety_harm_gate(stage_data)
    harm_status = harm_gate["status"]
    state_contaminated = any(
        item["state_integrity"] == "contaminated" or item["outcomes"]["state_contamination"] == "yes"
        or (item["state_receipts_complete"] and not item["state_transition_ok"])
        for item in stage_data.values()
    )
    state_unknown = any(
        item["state_integrity"] != "clean" or item["outcomes"]["state_contamination"] == "unknown"
        or not item["state_receipts_complete"] or not item["state_transition_ok"]
        for item in stage_data.values()
    )
    state_status = "fail" if state_contaminated else ("pending" if state_unknown else "pass")
    assignment_missing_fields = {"observed_phase_id", "observed_arm", "observed_participant_id"}
    assignment_missing = [item for item in missing if item["field"] in assignment_missing_fields]
    assignment_issue_fields = {"observed_phase_id", "observed_arm", "observed_participant_id", "task_spec_sha256"}
    assignment_issues = [issue for issue in integrity_issues if issue.get("field") in assignment_issue_fields]
    assignment_status = "fail" if assignment_issues else ("pending" if assignment_missing else "pass")
    provenance_status, provenance_complete, provenance_invalid = provenance_gate_status(stage_data, missing, BENEFIT_PROVENANCE_ROLES, integrity_issues)
    participant_human_denied = any(issue.get("field", "").endswith("_denied")
                                   or issue.get("field", "").startswith("human_attestation.")
                                   for issue in integrity_issues)
    participant_human_ready = (
        evidence.get("evidence_class") == "actual_human"
        and evidence.get("human_data_attested") is True
        and evidence.get("local_only_confirmed") is True
        and evidence.get("research_authorization_confirmed") is True
        and isinstance(evidence.get("participant_authorization"), dict)
        and all(evidence["participant_authorization"].get(participant_id) is True
                and human_attestation_counts[participant_id] == 40
                for participant_id in manifest["participants"])
    )
    participant_human_status = "fail" if participant_human_denied else ("pass" if participant_human_ready else "pending")
    gates = {
        "terminal_evidence_coverage": gate(coverage_status, denominator=total, complete=terminal_complete,
                                            known_failures=terminal_known_failure,
                                            unknown_or_unreadable=terminal_unknown,
                                            required_percentage=95, observed_percentage=round(terminal_coverage * 100, 3) if terminal_coverage is not None else None),
        "positive_and_negative_stage_provenance": gate(provenance_status, denominator=total,
            complete=provenance_complete, required_artifact_roles=list(BENEFIT_PROVENANCE_ROLES),
            invalid_references=provenance_invalid),
        "target_word_correct_delivery_without_manual_edit": gate(effect_status,
            teach={"successes": teach_success_total, "denominator": denominator_target},
            control={"successes": control_success_total, "denominator": denominator_target},
            assigned_target_denominator_per_arm=denominator_target,
            observed_difference_pp=round(observed_overall_delta * 100, 3),
            observed_task_difference={"numerator": difference_tasks, "denominator_per_arm": denominator_target},
            unknown_target_outcomes=unknown_target_total,
            unknowns_count_as_success=False,
            threshold_cross_multiplication={"minimum_pp": 15, "passes": effect_threshold_met},
            required_difference_pp=15,
            participants_with_positive_observed_difference=people_positive,
            required_people_positive=len(manifest["participants"]) - 1),
        "net_time_saved": gate(savings_status, median_net_saved_ms=median_net_saved,
                               participant_results=person_net_savings, required_median_ms_strictly_greater_than=0),
        "critical_cross_scope_unauthorized_contamination_and_resurrection": gate(harm_status,
            **{key: value for key, value in harm_gate.items() if key != "status"}, required_zero=True),
        "safety_negative_evidence": safety_negative_gate(stage_data),
        "participant_state_and_arm_integrity": gate(state_status,
            active_rules_per_participant=manifest["freeze_facts"].get("active_rule_families_by_participant", {}),
            integrity="clean" if state_status == "pass" else state_status),
        "actual_human_local_consent_by_participant": gate(
            participant_human_status,
            evidence_class=evidence.get("evidence_class"),
            per_participant_attested_tasks={participant_id: human_attestation_counts[participant_id]
                                            for participant_id in manifest["participants"]},
            required_tasks_per_participant=40,
            participant_authorization=evidence.get("participant_authorization")),
        "phase_arm_participant_and_task_identity": gate(assignment_status,
            denominator=total, mismatches=assignment_issues, missing=len(assignment_missing)),
        "unique_new_recordings": gate(
            "fail" if duplicate_captures or duplicate_owner_receipts or capture_not_live or capture_invalid else (
                "pass" if sum(bool(item["distinct_recording"]) for item in stage_data.values()) == total else "pending"
            ),
            allocated=total,
            distinct_verified=sum(bool(item["distinct_recording"]) for item in stage_data.values()),
            duplicate_trial_groups=duplicate_captures,
            reused_owner_receipt_groups=duplicate_owner_receipts,
            live_capture_not_confirmed=capture_not_live,
        ),
    }
    if len(row_ids["missing"]) > 0:
        gates["all_allocated_tasks_retained"] = gate("pending", denominator=total, missing_trial_ids=row_ids["missing"])
    else:
        gates["all_allocated_tasks_retained"] = gate("fail" if row_ids["extra"] or row_ids["duplicate"] else "pass",
                                                     denominator=total, missing_trial_ids=[], extra_trial_ids=row_ids["extra"], duplicate_trial_ids=row_ids["duplicate"])
    per_person_delta = {row["participant_id"]: row for row in person_deltas}
    return assemble_report(manifest, evidence, gates, missing, integrity_issues,
                           per_participant=per_person,
                           participant_effects=list(per_person_delta.values()),
                           target_denominator_per_arm_per_person=10,
                           all_assigned_denominator_per_person=40,
                           result_breakdown={
                               "all_assigned": summarize_benefit_rows(list(stage_data.values())),
                               "ordinary": summarize_benefit_rows([item for item in stage_data.values()
                                                                   if item["assignment"]["category"] == "ordinary"]),
                               "negative": summarize_benefit_rows([item for item in stage_data.values()
                                                                   if item["assignment"]["category"].startswith("safety_")]),
                           })


def meets_percentage_point_threshold(difference_tasks: int, denominator: int, minimum_pp: int) -> bool:
    return denominator > 0 and 100 * difference_tasks >= minimum_pp * denominator


def calculate_net_saved_ms(gross_saved_ms: int, total_cost_ms: int, already_counted_ms: int) -> int:
    return gross_saved_ms - (total_cost_ms - already_counted_ms)


def matched_time_savings(target_rows: list[dict]) -> dict:
    by_set_arm = defaultdict(list)
    for item in target_rows:
        assignment = item["assignment"]
        by_set_arm[(assignment["matched_set"], assignment["arm"])].append(item)
    matched_sets = sorted({item["assignment"]["matched_set"] for item in target_rows})
    if len(target_rows) != 20 or len(matched_sets) != 5:
        return {"gross_matched_saved_ms": None, "net_saved_ms": None, "incremental_cost_ms": None,
                "unknown_matched_sets": ["incomplete_target_task_denominator"]}
    for matched_set in matched_sets:
        teach = by_set_arm[(matched_set, "teach")]
        control = by_set_arm[(matched_set, "control")]
        if len(teach) != 2 or len(control) != 2 or any(item.get("completion_ms") is None for item in teach + control):
            return {"gross_matched_saved_ms": None, "net_saved_ms": None, "incremental_cost_ms": None,
                    "unknown_matched_sets": [matched_set]}
    gross_saved = sum(item["completion_ms"] for item in target_rows if item["assignment"]["arm"] == "control") - sum(
        item["completion_ms"] for item in target_rows if item["assignment"]["arm"] == "teach"
    )
    return {"gross_matched_saved_ms": gross_saved, "net_saved_ms": None,
            "incremental_cost_ms": None, "unknown_matched_sets": []}


def analyze(args: argparse.Namespace) -> dict:
    close = bool(getattr(args, "close", False))
    external_path(args.manifest, must_exist=True)
    manifest = read_json(args.manifest)
    manifest_fingerprint(manifest)
    evidence = read_json(args.evidence)
    require(evidence.get("artifact_type") == "voiceflow_dictionary_learning_pilot_evidence", "evidence artifact type mismatch")
    require(evidence.get("schema_version") == 1, "evidence schema_version unsupported")
    evidence_class = evidence.get("evidence_class")
    if evidence_class == "unit_fixture":
        require(evidence.get("fixture_only") is True and Path(args.evidence).resolve().is_relative_to(REPO),
                "unit fixtures must be explicitly marked and stored under the repository")
        require(evidence.get("fixture_only") is True and evidence.get("human_data_attested") is False,
                "unit fixture must be explicitly excluded from human evidence")
        return {
            "artifact_type": "voiceflow_dictionary_learning_pilot_report",
            "schema_version": 1,
            "tool_version": TOOL_VERSION,
            "stage": manifest["stage"],
            "batch_id": manifest["batch_id"],
            "manifest_fingerprint": manifest_fingerprint(manifest),
            "evidence_class": "unit_fixture",
            "status": "fixture_only",
            "human_gate_status": "pending",
            "assigned_trial_count": len(manifest["assignments"]),
            "observed_trial_records": 0,
            "gates": {},
            "missing_evidence": {"items": [], "counts_by_field": {}, "integrity_issues": []},
            "fixture_notice": "Synthetic unit fixture. Excluded from human results and all acceptance gates.",
        }
    external_path(args.evidence, must_exist=True)
    require(evidence.get("stage") == manifest["stage"], "evidence stage mismatch")
    require(evidence.get("batch_id") == manifest["batch_id"], "evidence batch_id mismatch")
    require(evidence.get("manifest_fingerprint") == manifest_fingerprint(manifest), "evidence manifest fingerprint mismatch")
    root = None
    if args.artifact_root:
        raw_root = Path(args.artifact_root)
        require(not raw_root.is_symlink(), "artifact root must not be a symlink")
        root = external_path(raw_root, must_exist=True)
        require(root.is_dir(), "artifact root directory required")
    if manifest.get("freeze_status") != "frozen":
        missing = [{"item_id": "manifest", "field": "sealed_freeze_manifest"}]
        return apply_close_mode(assemble_report(
            manifest, evidence,
            {"frozen_manifest": gate("pending", reason="allocation draft has no frozen system/state/task catalog")},
            missing, [],
        ), close)
    # Revalidate frozen facts and their binding before reading any human outcomes.
    build_owner_public_key = getattr(args, "build_owner_public_key", None)
    validate_freeze_facts(manifest, manifest["freeze_facts"], build_owner_public_key)
    if manifest["stage"] == "technical":
        require(not any(getattr(args, key, None) for key in (
            "technical_report", "technical_manifest", "technical_evidence", "technical_artifact_root",
        )), "technical analysis cannot name upstream technical evidence")
        return apply_close_mode(analyze_technical(manifest, evidence, root), close)
    upstream_values = {
        "report": getattr(args, "technical_report", None),
        "manifest": getattr(args, "technical_manifest", None),
        "evidence": getattr(args, "technical_evidence", None),
        "artifact_root": getattr(args, "technical_artifact_root", None),
    }
    require(all(upstream_values.values()),
            "benefit analysis requires --technical-report, --technical-manifest, --technical-evidence and --technical-artifact-root")
    upstream_report, upstream_binding = validate_upstream_technical(
        upstream_values["report"], upstream_values["manifest"], upstream_values["evidence"],
        upstream_values["artifact_root"], build_owner_public_key,
    )
    require(sha256_json(upstream_report) == manifest.get("upstream_technical_report_sha256"),
            "technical report fingerprint does not match frozen benefit allocation")
    require(upstream_binding == manifest.get("upstream_technical_binding"),
            "technical input bindings changed since benefit allocation")
    validate_benefit_build_identity(manifest["freeze_facts"], upstream_binding)
    return apply_close_mode(analyze_benefit(manifest, evidence, root, close=close), close)


def print_json(value: Any) -> None:
    sys.stdout.write(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    build = commands.add_parser("build-pre-c1", help="build and sign a pinned Installable Test app in a new private owner root")
    build.add_argument("--owner-root", required=True, help="new private absolute output root outside the repository")
    build.add_argument("--owner-private-key", required=True, help="explicit unencrypted local Ed25519 owner signing key")
    build.add_argument("--owner-public-key", required=True, help="matching trusted local Ed25519 public key")

    technical = commands.add_parser("allocate-technical", help="create deterministic 40-trial and auxiliary-check allocation draft")
    technical.add_argument("--batch-id", required=True)
    technical.add_argument("--seed", required=True)
    technical.add_argument("--operator", action="append", required=True, help="repeat exactly twice with pseudonymous operator IDs")
    technical.add_argument("--non-implementer", required=True)
    technical.add_argument("--out", required=True, help="new absolute path outside the repository")

    benefit = commands.add_parser("allocate-benefit", help="create deterministic 4–6 person benefit allocation draft")
    benefit.add_argument("--batch-id", required=True)
    benefit.add_argument("--seed", required=True)
    benefit.add_argument("--participant", action="append", required=True, help="repeat 4–6 times with pseudonymous IDs")
    benefit.add_argument("--family-catalog", required=True, help="pre-draw catalog of ten actual opaque families in five matched sets")
    benefit.add_argument("--technical-report", required=True, help="technical report to compare with a fresh local re-analysis")
    benefit.add_argument("--technical-manifest", required=True)
    benefit.add_argument("--technical-evidence", required=True)
    benefit.add_argument("--artifact-root", required=True)
    benefit.add_argument("--build-owner-public-key", required=True,
                         help="independently trusted key for the frozen Installable Test build")
    benefit.add_argument("--out", required=True, help="new absolute path outside the repository")

    seal = commands.add_parser("seal", help="bind a draft allocation to task catalog and frozen local system/state facts")
    seal.add_argument("--draft", required=True)
    seal.add_argument("--task-catalog", required=True)
    seal.add_argument("--freeze-facts", required=True)
    seal.add_argument("--technical-report", help="benefit stage: report must match re-analysis")
    seal.add_argument("--technical-manifest", help="benefit stage upstream frozen technical manifest")
    seal.add_argument("--technical-evidence", help="benefit stage upstream technical evidence ledger")
    seal.add_argument("--artifact-root", help="benefit stage upstream local artifact root")
    seal.add_argument("--build-owner-public-key", required=True,
                      help="independently trusted key for the frozen Installable Test build")
    seal.add_argument("--out", required=True, help="new absolute path outside the repository")

    empty = commands.add_parser("new-evidence", help="create a blank evidence ledger; it contains no human results")
    empty.add_argument("--manifest", required=True)
    empty.add_argument("--out", required=True, help="new absolute path outside the repository")

    scoring = commands.add_parser("analyze", help="analyze a local evidence ledger without starting any app or model")
    scoring.add_argument("--manifest", required=True)
    scoring.add_argument("--evidence", required=True)
    scoring.add_argument("--artifact-root", help="absolute local directory containing referenced evidence artifacts")
    scoring.add_argument("--build-owner-public-key", help="independently trusted key for the frozen Installable Test build")
    scoring.add_argument("--technical-report", help="benefit stage: frozen upstream technical report")
    scoring.add_argument("--technical-manifest", help="benefit stage: frozen upstream technical manifest")
    scoring.add_argument("--technical-evidence", help="benefit stage: upstream technical evidence ledger")
    scoring.add_argument("--technical-artifact-root", help="benefit stage: upstream technical artifact root")
    scoring.add_argument("--close", action="store_true",
                         help="close collection; unresolved gates become failures and unknown outcomes stay unsuccessful")
    scoring.add_argument("--out", help="optional new absolute report path outside the repository")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "build-pre-c1":
            document = create_pre_c1_build(args)
            print_json(document)
            return 0
        if args.command == "allocate-technical":
            document = make_technical_draft(args)
            write_once(args.out, document)
            print_json({"status": "allocation_draft", "stage": "technical", "trials": len(document["assignments"]),
                        "auxiliary_observations": len(document["auxiliary_observations"]),
                        "allocation_fingerprint": document["allocation_fingerprint"]})
            return 0
        if args.command == "allocate-benefit":
            document = make_benefit_draft(args)
            write_once(args.out, document)
            print_json({"status": "allocation_draft", "stage": "benefit", "participants": len(document["participants"]),
                        "trials": len(document["assignments"]), "allocation_fingerprint": document["allocation_fingerprint"]})
            return 0
        if args.command == "seal":
            manifest = seal_manifest(args)
            print_json({"status": "frozen", "stage": manifest["stage"], "batch_id": manifest["batch_id"],
                        "allocation_fingerprint": manifest["allocation_fingerprint"],
                        "freeze_fingerprint": manifest["freeze_fingerprint"],
                        "task_catalog_sha256": manifest["task_catalog_sha256"]})
            return 0
        if args.command == "new-evidence":
            external_path(args.manifest, must_exist=True)
            manifest = read_json(args.manifest)
            manifest_fingerprint(manifest)
            document = make_evidence_template(manifest)
            write_once(args.out, document)
            print_json({"status": "blank_evidence_ledger", "evidence_class": "unfilled",
                        "human_evidence_rows": 0, "stage": manifest["stage"],
                        "manifest_fingerprint": document["manifest_fingerprint"]})
            return 0
        if args.command == "analyze":
            report = analyze(args)
            if args.out:
                write_once(args.out, report)
            print_json(report)
            return 0
        raise PilotError("unknown command")
    except (PilotError, OSError) as error:
        print(f"dictionary-learning-pilot: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
