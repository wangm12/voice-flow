"""Unit checks for allocation and fail-closed pilot evidence handling.

All data in this module is synthetic fixture data. It is never an acceptance
result and contains no participant observations.
"""
from __future__ import annotations

import importlib.util
import hashlib
import json
import os
import plistlib
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
from contextlib import contextmanager


SCRIPT = Path(__file__).with_name("dictionary-learning-pilot.py")
SPEC = importlib.util.spec_from_file_location("dictionary_learning_pilot", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
pilot = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pilot)
FIXTURE = Path(__file__).with_name("fixtures") / "dictionary-learning-pilot-unit-fixture.json"


def seal_state_snapshot(receipt: dict) -> dict:
    receipt.setdefault("active_rule_inventory_sha256", receipt.get("rule_inventory_sha256"))
    receipt.setdefault("inactive_candidate_inventory_sha256", "f" * 64)
    receipt["state_fingerprint_sha256"] = pilot.state_snapshot_fingerprint(receipt)
    receipt["active_state_fingerprint_sha256"] = pilot.active_state_fingerprint(receipt)
    return receipt


class AllocationTests(unittest.TestCase):
    @staticmethod
    def family_catalog():
        return [
            {"family_id": f"f-{pair}-{side}", "matched_set": f"m-{pair}"}
            for pair in ("91a", "6c2", "e4b", "2d8", "a73")
            for side in ("x", "y")
        ]

    def technical_draft(self):
        args = type("Args", (), {
            "batch_id": "fixture-batch",
            "seed": "fixed-unit-fixture-seed",
            "operator": ["OP-A", "OP-B"],
            "non_implementer": "OP-B",
        })()
        return pilot.make_technical_draft(args)

    @staticmethod
    def write_json(path: Path, value) -> None:
        path.write_text(json.dumps(value), encoding="utf-8")

    def test_technical_allocation_is_deterministic_balanced_and_pre_c1(self):
        first = self.technical_draft()
        second = self.technical_draft()
        self.assertEqual(first, second)
        self.assertEqual(first["c1_boundary"], pilot.C1_BOUNDARY)
        self.assertEqual(len(first["assignments"]), 40)
        counts = {name: sum(row["category"] == name for row in first["assignments"])
                  for name, _ in pilot.TECH_CATEGORIES}
        self.assertEqual(counts, dict(pilot.TECH_CATEGORIES))
        self.assertEqual(sum(row["operator_id"] == "OP-A" for row in first["assignments"]), 20)
        self.assertEqual(sum(row["operator_id"] == "OP-B" for row in first["assignments"]), 20)
        self.assertEqual(sum(row["implementation_role"] == "non_implementer" for row in first["roster"]), 1)
        self.assertEqual(len(first["auxiliary_observations"]), 10)

    def test_technical_allocation_recomputes_seed_and_rejects_reused_aux_ids(self):
        tampered = self.technical_draft()
        same_operator = [row for row in tampered["assignments"]
                         if row["operator_id"] == "OP-A" and row["category"] == "a_positive"][:2]
        same_operator[0]["trial_id"], same_operator[1]["trial_id"] = (
            same_operator[1]["trial_id"], same_operator[0]["trial_id"]
        )
        tampered["allocation_fingerprint"] = pilot.sha256_json({
            key: value for key, value in tampered.items() if key != "allocation_fingerprint"
        })
        with self.assertRaises(pilot.PilotError):
            pilot.validate_allocation(tampered)

        duplicate_aux = self.technical_draft()
        duplicate_aux["auxiliary_observations"][1]["check_id"] = duplicate_aux["auxiliary_observations"][0]["check_id"]
        duplicate_aux["allocation_fingerprint"] = pilot.sha256_json({
            key: value for key, value in duplicate_aux.items() if key != "allocation_fingerprint"
        })
        with self.assertRaises(pilot.PilotError):
            pilot.validate_allocation(duplicate_aux)

    def test_technical_allocation_accepts_multiple_declared_non_implementers(self):
        draft = self.technical_draft()
        for row in draft["roster"]:
            row["implementation_role"] = "non_implementer"
        draft["allocation_fingerprint"] = pilot.sha256_json({
            key: value for key, value in draft.items() if key != "allocation_fingerprint"
        })
        self.assertEqual(pilot.validate_allocation(draft), draft["allocation_fingerprint"])

    def test_benefit_allocation_recomputes_exact_seeded_rows_and_total(self):
        draft = self.benefit_draft()
        same_person_target_rows = [row for row in draft["assignments"]
                                   if row["participant_id"] == "P-A" and row["category"] == "target"][:2]
        same_person_target_rows[0]["trial_id"], same_person_target_rows[1]["trial_id"] = (
            same_person_target_rows[1]["trial_id"], same_person_target_rows[0]["trial_id"]
        )
        draft["allocation_fingerprint"] = pilot.sha256_json({
            key: value for key, value in draft.items() if key != "allocation_fingerprint"
        })
        with self.assertRaises(pilot.PilotError):
            pilot.validate_allocation(draft)

    def benefit_draft(self):
        families = self.family_catalog()
        participants = ["P-A", "P-B", "P-C", "P-D"]
        family_catalog = {"artifact_type": "voiceflow_dictionary_learning_family_catalog",
                          "schema_version": 1, "families": families}
        body = {
            "artifact_type": "voiceflow_dictionary_learning_pilot_allocation",
            "schema_version": 1, "tool_version": pilot.TOOL_VERSION, "stage": "benefit",
            "phase_id": pilot.BENEFIT_PHASE, "c1_boundary": pilot.C1_BOUNDARY,
            "batch_id": "benefit-fixture", "seed": "fixed-family-draw", "freeze_status": "draft",
            "participants": participants, "family_catalog": families,
            "family_catalog_sha256": pilot.sha256_json(family_catalog),
            "assignments": pilot.benefit_assignments("fixed-family-draw", participants, families),
            "auxiliary_observations": [], "freeze_facts": None,
            "task_catalog_sha256": None, "task_specs": None,
            "upstream_technical_report_sha256": "e" * 64,
            "upstream_technical_binding": {
                "manifest_fingerprint": "a" * 64,
                "evidence_ledger_sha256": "b" * 64,
                "artifact_bindings_sha256": "c" * 64,
                "build_sha256": "d" * 64,
                "build_owner_receipt_sha256": "e" * 64,
                "build_owner_signature_sha256": "f" * 64,
                "build_owner_key_fingerprint": "SHA256:unit-fixture-key",
                "asr_model_id": "unit-model",
                "asr_model_revision": "unit-revision",
            },
        }
        body["allocation_fingerprint"] = pilot.sha256_json(body)
        return body

    def test_benefit_freeze_must_match_technical_build_and_asr_model(self):
        draft = self.benefit_draft()
        binding = draft["upstream_technical_binding"]
        facts = {key: binding[key] for key in pilot.TECHNICAL_BUILD_IDENTITY_FIELDS}
        pilot.validate_benefit_build_identity(facts, binding)
        for key in ("build_sha256", "asr_model_id", "asr_model_revision"):
            with self.subTest(field=key):
                mismatched = dict(facts)
                mismatched[key] = "different-build-or-model"
                with self.assertRaises(pilot.PilotError):
                    pilot.validate_benefit_build_identity(mismatched, binding)

    @staticmethod
    def seal_unit_manifest(draft, facts):
        manifest = dict(draft)
        manifest.update({
            "freeze_status": "frozen",
            "freeze_facts": facts,
            "task_catalog_sha256": "f" * 64,
            "task_specs": {row["trial_id"]: "e" * 64 for row in draft["assignments"]},
            "allocation_fingerprint": draft["allocation_fingerprint"],
        })
        manifest["freeze_fingerprint"] = pilot.sha256_json(manifest)
        return manifest

    def frozen_technical(self):
        draft = self.technical_draft()
        effective_hint = hashlib.sha256(b"unit-technical-hint").hexdigest()
        initial = pilot.active_state_fingerprint({
            "global_dictionary_sha256": "a" * 64,
            "rule_inventory_sha256": "b" * 64,
            "active_rule_inventory_sha256": "b" * 64,
            "inactive_candidate_inventory_sha256": "f" * 64,
            "tombstone_inventory_sha256": "c" * 64,
            "active_rule_families": [],
            "effective_hint_sha256": effective_hint,
            "passive_learning_enabled": False,
            "target_rule_state": "not_applicable",
        })
        facts = {
            "artifact_type": "voiceflow_dictionary_learning_freeze_facts", "schema_version": 2,
            "build_sha256": "a" * 64, "asr_route": "on_device", "asr_model_id": "unit-model",
            "asr_model_revision": "unit-revision", "cleanup_mode": "off",
            "term_injection_enabled": False, "fuzzy_matching_enabled": False,
            "permissions_sha256": "b" * 64, "initial_state_sha256": initial,
            "source_revision": pilot.PRE_C1_SOURCE_REVISION,
            "settings_schema_version": pilot.PRE_C1_SETTINGS_SCHEMA_VERSION,
            "history_schema_version": pilot.PRE_C1_HISTORY_SCHEMA_VERSION,
            "test_bundle_identifier": pilot.PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "source_checkout_path": "/tmp/unit-pre-c1-checkout",
            "test_app_path": "/tmp/unit-pre-c1-test.app",
            "build_owner_receipt_path": "/tmp/unit-build-owner-receipt.json",
            "build_owner_receipt_sha256": "c" * 64,
            "build_owner_signature_sha256": "d" * 64,
            "build_owner_key_fingerprint": "SHA256:unit-fixture-key",
        }
        return self.seal_unit_manifest(draft, facts)

    def frozen_benefit(self):
        draft = self.benefit_draft()
        states, hints, active, passive = {}, {}, {}, {}
        for participant_id in draft["participants"]:
            families = sorted({row["family_id"] for row in draft["assignments"]
                              if row["participant_id"] == participant_id
                              and row["category"] == "target" and row["arm"] == "teach"})
            hint = hashlib.sha256(f"hint-{participant_id}".encode()).hexdigest()
            snapshot = {
                "global_dictionary_sha256": "a" * 64,
                "rule_inventory_sha256": "b" * 64,
                "active_rule_inventory_sha256": "b" * 64,
                "inactive_candidate_inventory_sha256": "f" * 64,
                "tombstone_inventory_sha256": "c" * 64,
                "active_rule_families": families,
                "effective_hint_sha256": hint,
                "passive_learning_enabled": False,
                "target_rule_state": "not_applicable",
            }
            states[participant_id] = pilot.state_snapshot_fingerprint(snapshot)
            hints[participant_id] = hint
            active[participant_id] = families
            passive[participant_id] = True
        facts = {
            "artifact_type": "voiceflow_dictionary_learning_freeze_facts", "schema_version": 2,
            "build_sha256": "a" * 64, "asr_route": "on_device", "asr_model_id": "unit-model",
            "asr_model_revision": "unit-revision", "cleanup_mode": "off",
            "term_injection_enabled": False, "fuzzy_matching_enabled": False,
            "permissions_sha256": "b" * 64,
            "source_revision": pilot.PRE_C1_SOURCE_REVISION,
            "settings_schema_version": pilot.PRE_C1_SETTINGS_SCHEMA_VERSION,
            "history_schema_version": pilot.PRE_C1_HISTORY_SCHEMA_VERSION,
            "test_bundle_identifier": pilot.PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "source_checkout_path": "/tmp/unit-pre-c1-checkout",
            "test_app_path": "/tmp/unit-pre-c1-test.app",
            "build_owner_receipt_path": "/tmp/unit-build-owner-receipt.json",
            "build_owner_receipt_sha256": "c" * 64,
            "build_owner_signature_sha256": "d" * 64,
            "build_owner_key_fingerprint": "SHA256:unit-fixture-key",
            "state_sha256_by_participant": states,
            "effective_hint_sha256_by_participant": hints,
            "active_rule_families_by_participant": active,
            "passive_learning_disabled_by_participant": passive,
        }
        return self.seal_unit_manifest(draft, facts)

    def test_benefit_assignment_preserves_per_person_family_controls(self):
        participants = ["P-A", "P-B", "P-C", "P-D"]
        families = self.family_catalog()
        first = pilot.benefit_assignments("fixed-unit-fixture-seed", participants, families)
        second = pilot.benefit_assignments("fixed-unit-fixture-seed", participants, families)
        self.assertEqual(first, second)
        for participant_id in participants:
            rows = [row for row in first if row["participant_id"] == participant_id]
            self.assertEqual(len(rows), 40)
            self.assertEqual({name: sum(row["category"] == name for row in rows)
                              for name, _ in pilot.BENEFIT_CATEGORIES}, dict(pilot.BENEFIT_CATEGORIES))
            family_arms = {}
            for row in rows:
                if row["category"] == "target":
                    family_arms.setdefault(row["family_id"], set()).add(row["arm"])
            self.assertEqual(sum(next(iter(arms)) == "teach" for arms in family_arms.values()), 5)
            self.assertTrue(all(len(arms) == 1 for arms in family_arms.values()))

    def test_benefit_family_placeholders_are_rejected_before_draw(self):
        catalog = {
            "artifact_type": "voiceflow_dictionary_learning_family_catalog",
            "schema_version": 1,
            "families": [
                {"family_id": f"WF{index:02d}", "matched_set": f"m-{(index + 1) // 2}"}
                for index in range(1, 11)
            ],
        }
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-family-unit-") as tmp:
            path = Path(tmp) / "families.json"
            self.write_json(path, catalog)
            with self.assertRaises(pilot.PilotError):
                pilot.load_family_catalog(path)

    def test_freeze_facts_reject_duplicate_teach_families(self):
        manifest = self.frozen_benefit()
        participant_id = manifest["participants"][0]
        manifest["freeze_facts"]["active_rule_families_by_participant"][participant_id][-1] = \
            manifest["freeze_facts"]["active_rule_families_by_participant"][participant_id][0]
        with self.assertRaises(pilot.PilotError):
            pilot.validate_freeze_facts(manifest, manifest["freeze_facts"])

    def test_pre_c1_freeze_facts_reject_current_schema_or_wrong_revision(self):
        manifest = self.frozen_technical()
        facts = dict(manifest["freeze_facts"])
        facts["history_schema_version"] = 20
        with self.assertRaises(pilot.PilotError):
            pilot.validate_freeze_facts(manifest, facts)
        facts = dict(manifest["freeze_facts"])
        facts["source_revision"] = "0" * 40
        with self.assertRaises(pilot.PilotError):
            pilot.validate_freeze_facts(manifest, facts)

    def test_forget_and_restart_substages_are_ordered_per_operator(self):
        manifest = self.technical_draft()
        for operator_id in ("OP-A", "OP-B"):
            rows = sorted((row for row in manifest["assignments"] if row["operator_id"] == operator_id),
                          key=lambda row: row["operator_order"])
            stages = [row["substage"] for row in rows if row["category"] == "forget_restart"]
            self.assertEqual(stages, ["after_forget"] * 3 + ["after_restart"] * 3)

    def test_effect_threshold_uses_exact_integer_percentage_points(self):
        self.assertTrue(pilot.meets_percentage_point_threshold(6, 40, 15))
        self.assertFalse(pilot.meets_percentage_point_threshold(5, 40, 15))

    def test_gross_savings_sum_all_ten_paired_tasks_before_cost(self):
        target_rows = []
        for pair in ("a", "b", "c", "d", "e"):
            for arm, completion in (("teach", 0), ("control", 200)):
                for index in range(2):
                    target_rows.append({
                        "assignment": {"matched_set": f"m-{pair}", "arm": arm},
                        "completion_ms": completion,
                    })
        result = pilot.matched_time_savings(target_rows)
        self.assertEqual(result["gross_matched_saved_ms"], 2000)
        self.assertEqual(pilot.calculate_net_saved_ms(result["gross_matched_saved_ms"], 1500, 0), 500)


class BuildOwnerReceiptTests(unittest.TestCase):
    @staticmethod
    def run_checked(command: list[str], **kwargs) -> subprocess.CompletedProcess:
        completed = subprocess.run(command, capture_output=True, text=True, check=False, **kwargs)
        if completed.returncode != 0:
            raise AssertionError(f"command failed: {command!r}\n{completed.stdout}\n{completed.stderr}")
        return completed

    @staticmethod
    def fingerprint(public_key: Path) -> str:
        result = subprocess.run(
            ["/usr/bin/ssh-keygen", "-lf", str(public_key), "-E", "sha256"],
            capture_output=True, text=True, check=False,
        )
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        return result.stdout.split()[1]

    @staticmethod
    def sign_receipt(receipt_path: Path, private_key: Path) -> bytes:
        completed = subprocess.run(
            ["/usr/bin/ssh-keygen", "-Y", "sign", "-f", str(private_key),
             "-n", pilot.BUILD_OWNER_NAMESPACE, str(receipt_path)],
            capture_output=True, check=False,
        )
        signature_path = receipt_path.with_name(receipt_path.name + ".sig")
        if completed.returncode != 0 or not signature_path.is_file():
            raise AssertionError(completed.stderr.decode("utf-8", errors="replace"))
        return signature_path.read_bytes()

    @staticmethod
    def write_receipt(path: Path, receipt: dict) -> bytes:
        payload = json.dumps(receipt, ensure_ascii=False, indent=2, allow_nan=False).encode("utf-8") + b"\n"
        path.write_bytes(payload)
        return payload

    @contextmanager
    def signed_build_fixture(self):
        """Create a signed protocol fixture, without running or claiming a product build."""
        with tempfile.TemporaryDirectory(prefix="voiceflow-pilot-build-test-", dir="/tmp") as temporary:
            root = Path(temporary).resolve()
            keys = root / "trusted-keys"
            keys.mkdir(mode=0o700)
            private_key = keys / "build-owner"
            public_key = private_key.with_name(private_key.name + ".pub")
            self.run_checked([
                "/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C",
                "unit-test-build-owner", "-f", str(private_key),
            ])
            owner_root = root / "owner-output"
            owner_root.mkdir(mode=0o700)
            source_checkout = owner_root / "source"
            self.run_checked([
                "/usr/bin/git", "clone", "--shared", "--no-checkout", "--no-tags",
                str(pilot.REPO), str(source_checkout),
            ])
            self.run_checked([
                "/usr/bin/git", "-C", str(source_checkout), "checkout", "--detach",
                pilot.PRE_C1_SOURCE_REVISION,
            ])

            source_tree_oid = self.run_checked([
                "/usr/bin/git", "-C", str(source_checkout), "rev-parse", "HEAD^{tree}",
            ]).stdout.strip()
            source_inputs = pilot.pre_c1_source_input_hashes(source_checkout)
            runtime_pin_path = source_checkout / "src-tauri" / "llama-runtime" / "runtime-pin.env"
            pin_values = {}
            for line in runtime_pin_path.read_text(encoding="utf-8").splitlines():
                if "=" in line and not line.lstrip().startswith("#"):
                    key, value = line.split("=", 1)
                    pin_values[key] = value

            app_path = source_checkout / "src-tauri" / "target" / "release" / "bundle" / "macos" / "VoiceFlow.app"
            contents = app_path / "Contents"
            executable = contents / "MacOS" / "VoiceFlow"
            resources = contents / "Resources"
            executable.parent.mkdir(parents=True)
            resources.mkdir(parents=True)
            info = {
                "CFBundleIdentifier": pilot.PRE_C1_TEST_BUNDLE_IDENTIFIER,
                "CFBundleExecutable": "VoiceFlow",
                "CFBundlePackageType": "APPL",
            }
            with (contents / "Info.plist").open("wb") as stream:
                plistlib.dump(info, stream)

            # This small native executable is only a bundle-validation fixture.
            c_source = root / "fixture-main.c"
            c_source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
            self.run_checked([
                "/usr/bin/clang", "-arch", "arm64", "-mmacosx-version-min=13.0",
                str(c_source), "-o", str(executable),
            ])
            os.chmod(executable, 0o755)

            llama_resources = resources / "llama-runtime"
            llama_bin = llama_resources / "bin"
            llama_bin.mkdir(parents=True)
            llama_executable = llama_bin / "llama-server"
            llama_executable.write_bytes(b"synthetic sidecar executable fixture\n")
            identity = {
                "schema_version": 1,
                "status": "ready",
                "runtime": "llama.cpp",
                "source": {
                    "repository": pin_values["LLAMA_CPP_SOURCE_URL"],
                    "release_tag": pin_values["LLAMA_CPP_RELEASE_TAG"],
                    "annotated_tag_object_sha1": pin_values["LLAMA_CPP_TAG_OBJECT_SHA1"],
                    "commit": pin_values["LLAMA_CPP_COMMIT"],
                },
                "artifact": {
                    "path": "bin/llama-server",
                    "architecture": pin_values["LLAMA_RUNTIME_ARCH"],
                    "minimum_macos": pin_values["LLAMA_RUNTIME_MINIMUM_MACOS"],
                    "sha256_signed": pilot.sha256_file(llama_executable),
                },
            }
            (llama_resources / "BUILD-IDENTITY.json").write_text(
                json.dumps(identity, ensure_ascii=False, indent=2) + "\n", encoding="utf-8",
            )

            dependency_manifest = resources / "ThirdPartyLicenses" / "DependencyManifest" / "dependency-identities.txt"
            dependency_manifest.parent.mkdir(parents=True)
            dependency_manifest.write_text(
                "\n".join((
                    f"Sidecar Package.resolved SHA-256: {source_inputs['src-tauri/mlx-sidecar/Package.resolved']}",
                    "Xcode Package.resolved SHA-256: "
                    + source_inputs["src-tauri/mlx-sidecar/xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved"],
                    "mlx-audio-swift source overlay patch SHA-256: "
                    + source_inputs["src-tauri/mlx-sidecar/sdk-overlay/voiceflow-mlx-audio-cancellation-v2.patch"],
                    "mlx-audio-swift source overlay manifest SHA-256: "
                    + source_inputs["src-tauri/mlx-sidecar/sdk-overlay/patch-manifest.json"],
                    "",
                )), encoding="utf-8",
            )

            # No build command runs here. The fake bundle and signed receipt exercise
            # the validator's cryptographic, pin, signature, and full-bundle checks.
            self.run_checked([
                "/usr/bin/codesign", "--force", "--deep", "--sign", "-", str(app_path),
            ])
            signature_facts = pilot.code_signature_facts(app_path)
            sidecars = pilot.validate_pre_c1_sidecars(app_path, source_checkout, {"source_input_hashes": source_inputs})
            receipt_path = owner_root / pilot.PRE_C1_BUILD_RECEIPT_NAME
            toolchain_executables = {
                tool: str(root / "fixture-toolchain" / tool / "bin" / tool)
                for tool in ("npm", "node", "cargo", "rustc", "xcodebuild")
            }
            tool_directories = list(dict.fromkeys(
                Path(toolchain_executables[name]).parent.as_posix()
                for name in ("npm", "node", "cargo", "rustc")
            ))
            fixture_build_path = ":".join(tool_directories + [
                "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin",
            ])
            body = {
                "artifact_type": pilot.PRE_C1_BUILD_RECEIPT_TYPE,
                "schema_version": 1,
                "tool_version": pilot.TOOL_VERSION,
                "owner_tool_sha256": pilot.sha256_file(SCRIPT),
                "owner_signing_key_fingerprint": self.fingerprint(public_key),
                "owner_root": str(owner_root),
                "source_revision": pilot.PRE_C1_SOURCE_REVISION,
                "source_tree_oid": source_tree_oid,
                "source_checkout_path": str(source_checkout),
                "source_input_hashes": source_inputs,
                "toolchain_executables": toolchain_executables,
                "build_path": fixture_build_path,
                "test_app_path": str(app_path),
                "test_bundle_identifier": pilot.PRE_C1_TEST_BUNDLE_IDENTIFIER,
                "build_sha256": pilot.sha256_file(executable),
                "app_bundle_sha256": pilot.bundle_inventory_sha256(app_path),
                "info_plist_sha256": pilot.sha256_file(contents / "Info.plist"),
                "code_signature": signature_facts,
                "sidecar_provenance_sha256": pilot.sha256_json(sidecars),
                # Schema-required command observations are synthetic protocol values;
                # this test intentionally does not invoke npm, Tauri, or human workflows.
                "npm_ci_command": ["npm", "ci"],
                "npm_ci_exit_status": 0,
                "tauri_build_command": pilot.PRE_C1_NPM_BUILD_COMMAND,
                "tauri_build_exit_status": 0,
                "node_version": "unit-fixture-not-run",
                "npm_version": "unit-fixture-not-run",
                "rustc_version": "unit-fixture-not-run",
                "cargo_version": "unit-fixture-not-run",
                "xcode_version": "unit-fixture-not-run",
                "platform": "Darwin",
                "architecture": "arm64",
                "source_date_epoch": 0,
            }
            body["owner_receipt_sha256"] = pilot.sha256_json(body)
            payload = self.write_receipt(receipt_path, body)
            signature_bytes = self.sign_receipt(receipt_path, private_key)
            facts = {
                "artifact_type": "voiceflow_dictionary_learning_freeze_facts",
                "schema_version": 2,
                "build_sha256": body["build_sha256"],
                "asr_route": "on_device",
                "asr_model_id": "unit-model",
                "asr_model_revision": "unit-revision",
                "cleanup_mode": "off",
                "term_injection_enabled": False,
                "fuzzy_matching_enabled": False,
                "permissions_sha256": "b" * 64,
                "source_revision": pilot.PRE_C1_SOURCE_REVISION,
                "settings_schema_version": pilot.PRE_C1_SETTINGS_SCHEMA_VERSION,
                "history_schema_version": pilot.PRE_C1_HISTORY_SCHEMA_VERSION,
                "test_bundle_identifier": pilot.PRE_C1_TEST_BUNDLE_IDENTIFIER,
                "source_checkout_path": str(source_checkout),
                "test_app_path": str(app_path),
                "build_owner_receipt_path": str(receipt_path),
                "build_owner_receipt_sha256": hashlib.sha256(payload).hexdigest(),
                "build_owner_signature_sha256": hashlib.sha256(signature_bytes).hexdigest(),
                "build_owner_key_fingerprint": body["owner_signing_key_fingerprint"],
                "initial_state_sha256": "a" * 64,
            }
            yield {
                "root": root,
                "private_key": private_key,
                "public_key": public_key,
                "owner_root": owner_root,
                "receipt_path": receipt_path,
                "receipt": body,
                "facts": facts,
                "source_checkout": source_checkout,
                "app_path": app_path,
                "executable": executable,
            }

    def test_signed_fixture_validates_pinned_source_and_full_bundle(self):
        manifest = AllocationTests().technical_draft()
        with self.signed_build_fixture() as fixture:
            self.assertEqual(pilot.PRE_C1_SOURCE_REVISION, "f4a75a1866f16c8d52932e7de461c1861e7b493d")
            self.assertEqual(fixture["receipt"]["source_revision"], pilot.PRE_C1_SOURCE_REVISION)
            self.assertEqual(fixture["receipt"]["owner_tool_sha256"], pilot.sha256_file(SCRIPT))
            self.assertEqual(fixture["receipt"]["source_input_hashes"],
                             pilot.pre_c1_source_input_hashes(fixture["source_checkout"]))
            self.assertEqual(
                pilot.validate_freeze_facts(manifest, fixture["facts"], fixture["public_key"]),
                fixture["facts"],
            )

            marker = fixture["app_path"] / "Contents" / "Resources" / "bundle-marker.txt"
            marker.write_text("post-receipt bundle change\n", encoding="utf-8")
            with self.assertRaisesRegex(pilot.PilotError, "app bundle differs"):
                pilot.validate_freeze_facts(manifest, fixture["facts"], fixture["public_key"])

    def test_rehashed_tampered_receipt_fails_sshsig_verification(self):
        manifest = AllocationTests().technical_draft()
        with self.signed_build_fixture() as fixture:
            tampered = dict(fixture["receipt"])
            tampered["build_sha256"] = "0" * 64
            tampered["owner_receipt_sha256"] = pilot.sha256_json({
                key: value for key, value in tampered.items() if key != "owner_receipt_sha256"
            })
            payload = self.write_receipt(fixture["receipt_path"], tampered)
            fixture["facts"]["build_owner_receipt_sha256"] = hashlib.sha256(payload).hexdigest()
            with self.assertRaisesRegex(pilot.PilotError, "signature verification failed"):
                pilot.validate_freeze_facts(manifest, fixture["facts"], fixture["public_key"])

    def test_wrong_trusted_key_is_rejected_by_ssh_keygen(self):
        manifest = AllocationTests().technical_draft()
        with self.signed_build_fixture() as fixture:
            wrong_private = fixture["root"] / "trusted-keys" / "wrong-owner"
            wrong_public = wrong_private.with_name(wrong_private.name + ".pub")
            self.run_checked([
                "/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C",
                "unit-test-wrong-owner", "-f", str(wrong_private),
            ])
            tampered = dict(fixture["receipt"])
            tampered["owner_signing_key_fingerprint"] = self.fingerprint(wrong_public)
            tampered["owner_receipt_sha256"] = pilot.sha256_json({
                key: value for key, value in tampered.items() if key != "owner_receipt_sha256"
            })
            payload = self.write_receipt(fixture["receipt_path"], tampered)
            signature = self.sign_receipt(fixture["receipt_path"], fixture["private_key"])
            fixture["facts"].update({
                "build_owner_receipt_sha256": hashlib.sha256(payload).hexdigest(),
                "build_owner_signature_sha256": hashlib.sha256(signature).hexdigest(),
                "build_owner_key_fingerprint": tampered["owner_signing_key_fingerprint"],
            })
            with self.assertRaisesRegex(pilot.PilotError, "signature verification failed"):
                pilot.validate_freeze_facts(manifest, fixture["facts"], wrong_public)

    def test_unsigned_hash_only_build_facts_are_rejected(self):
        manifest = AllocationTests().technical_draft()
        with self.signed_build_fixture() as fixture:
            with self.assertRaisesRegex(pilot.PilotError, "independently trusted build owner public key"):
                pilot.validate_freeze_facts(manifest, fixture["facts"])
            signature_path = fixture["receipt_path"].with_name(fixture["receipt_path"].name + ".sig")
            signature_path.unlink()
            with self.assertRaisesRegex(pilot.PilotError, "signature is missing"):
                pilot.validate_freeze_facts(manifest, fixture["facts"], fixture["public_key"])

    def test_macho_arm64_executable_and_full_bundle_inventory_include_mode_and_resources(self):
        with tempfile.TemporaryDirectory(prefix="voiceflow-pilot-macho-test-", dir="/tmp") as temporary:
            root = Path(temporary)
            executable = root / "VoiceFlow"
            executable.write_bytes(struct.pack("<IIII", 0xFEEDFACF, 0x0100000C, 0, 2))
            os.chmod(executable, 0o755)
            self.assertTrue(pilot.is_macho_executable(executable))
            os.chmod(executable, 0o644)
            self.assertFalse(pilot.is_macho_executable(executable))
            os.chmod(executable, 0o755)
            executable.write_text("not a Mach-O executable", encoding="utf-8")
            self.assertFalse(pilot.is_macho_executable(executable))

            bundle = root / "Fixture.app"
            resources = bundle / "Contents" / "Resources"
            resources.mkdir(parents=True)
            (resources / "one.txt").write_text("one\n", encoding="utf-8")
            digest_one = pilot.bundle_inventory_sha256(bundle)
            (resources / "one.txt").write_text("two\n", encoding="utf-8")
            digest_two = pilot.bundle_inventory_sha256(bundle)
            self.assertNotEqual(digest_one, digest_two)
            executable = resources / "tool"
            executable.write_bytes(b"same bytes")
            os.chmod(executable, 0o644)
            digest_mode_644 = pilot.bundle_inventory_sha256(bundle)
            os.chmod(executable, 0o755)
            digest_mode_755 = pilot.bundle_inventory_sha256(bundle)
            self.assertNotEqual(digest_mode_644, digest_mode_755)

    def test_fat_macho_finds_arm64_in_later_slice(self):
        arm64_header = struct.pack("<IiiIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 0, 0, 0)
        x86_header = struct.pack("<IiiIIII", 0xFEEDFACF, 0x01000007, 3, 2, 0, 0, 0)
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-fat-macho-", dir="/tmp") as tmp:
            root = Path(tmp)
            for fat64 in (False, True):
                entry_size = 32 if fat64 else 20
                first_offset = 8 + 2 * entry_size
                second_offset = first_offset + 32
                if fat64:
                    magic = b"\xca\xfe\xba\xbf"
                    entries = b"".join((
                        struct.pack(">IIQQII", 0x01000007, 3, first_offset, 32, 0, 0),
                        struct.pack(">IIQQII", 0x0100000C, 0, second_offset, 32, 0, 0),
                    ))
                else:
                    magic = b"\xca\xfe\xba\xbe"
                    entries = b"".join((
                        struct.pack(">IIIII", 0x01000007, 3, first_offset, 32, 0),
                        struct.pack(">IIIII", 0x0100000C, 0, second_offset, 32, 0),
                    ))
                path = root / ("fat64" if fat64 else "fat32")
                path.write_bytes(magic + struct.pack(">I", 2) + entries
                                 + x86_header.ljust(32, b"\0")
                                 + arm64_header.ljust(32, b"\0"))
                path.chmod(0o700)
                self.assertTrue(pilot.is_macho_executable(path), f"arm64 slice must be found in {path.name}")


class EvidenceGateTests(unittest.TestCase):
    def write_json(self, path: Path, value) -> None:
        path.write_text(json.dumps(value), encoding="utf-8")

    def add_live_capture_receipt(self, root: Path, manifest: dict, assignment: dict, row: dict, index: int) -> None:
        fp = pilot.manifest_fingerprint(manifest)
        actor_id = assignment["operator_id"] if manifest["stage"] == "technical" else assignment["participant_id"]
        run_id = f"10000000-0000-4000-8000-{index + 1:012d}"
        case_id = f"20000000-0000-4000-8000-{index + 1:012d}"
        job_id = f"30000000-0000-4000-8000-{index + 1:012d}"
        generation = str(index + 1)
        binding = {
            "batch_id": manifest["batch_id"], "manifest_fingerprint": fp,
            "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"], "actor_id": actor_id,
            "arm": assignment["arm"],
        }
        task_spec_sha256 = (manifest.get("task_specs") or {}).get(assignment["trial_id"])
        if isinstance(task_spec_sha256, str):
            binding["task_spec_sha256"] = task_spec_sha256
        identity = {"run_id": run_id, "case_id": case_id, "job_id": job_id,
                    "recording_generation": generation, "config_generation": "1"}
        event_names = (
            {"event": "session_started", "trigger_source": "explicit_research_stdio"},
            {"event": "input_first_accepted", "source": "capture_worker_16khz_samples", "samples": 1600},
            {"event": "capture_closed", "samples": 1600, "complete": True},
            {"event": "session_terminal", "outcome": "succeeded",
             "side_effect_boundary": "keyboard_may_have_posted", "provider_termination": "unknown"},
        )
        owner = {
            "artifact_type": "voiceflow_dictionary_learning_owner_receipt", "schema_version": 1,
            "source": "voiceflow_research_stdio_v1", "build_sha256": "a" * 64, **binding, **identity,
            "capture_closed": True, "local_terminal": True, "coverage": "owner_hooks_only",
            "dropped": "0", "provider_termination": "unknown",
            "events": [
                {"sequence": str(seq), "monotonic_ns": str(1000 + seq), **identity,
                 "processing_generation": None, **event}
                for seq, event in enumerate(event_names)
            ],
        }
        owner_path = root / "owner" / f"{assignment['trial_id']}.json"
        owner_path.parent.mkdir(parents=True, exist_ok=True)
        self.write_json(owner_path, owner)
        owner_sha = hashlib.sha256(owner_path.read_bytes()).hexdigest()
        raw_hash = hashlib.sha256(f"raw-{index}".encode()).hexdigest()
        inserted_hash = hashlib.sha256(f"inserted-{index}".encode()).hexdigest()
        source = {
            "artifact_type": "voiceflow_dictionary_learning_source_capture_receipt", "schema_version": 1,
            "source": "voiceflow_research_stdio_v1", **binding, **identity,
            "owner_receipt_sha256": owner_sha,
            "raw_asr_sha256": raw_hash, "inserted_text_sha256": inserted_hash,
        }
        source_path = root / "capture" / f"{assignment['trial_id']}.json"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        self.write_json(source_path, source)
        row["source_capture"] = {"capture_id": run_id, "recording_generation": generation,
                                 "live_capture_verified": "yes"}
        row["stage_hashes"] = {"raw_asr_sha256": raw_hash, "inserted_text_sha256": inserted_hash}
        row["provenance"]["owner_receipt"] = {
            "path": owner_path.relative_to(root).as_posix(), "sha256": owner_sha,
        }
        row["provenance"]["source_capture"] = {
            "path": source_path.relative_to(root).as_posix(),
            "sha256": hashlib.sha256(source_path.read_bytes()).hexdigest(),
        }

    def add_installable_test_audit_receipt(
        self, root: Path, manifest: dict, assignment: dict, row: dict, index: int,
        *, audit_fact: dict | None = None, pair_ref: str | None = None, include_pair: bool = True,
        extra_candidate: bool = False, runtime_id: str | None = None,
    ) -> None:
        fp = pilot.manifest_fingerprint(manifest)
        actor_id = assignment["operator_id"] if manifest["stage"] == "technical" else assignment["participant_id"]
        runtime_id = runtime_id or f"40000000-0000-4000-8000-{index + 1:012d}"
        activity_id = f"50000000-0000-4000-8000-{index + 1:012d}"
        if include_pair and pair_ref is None:
            pair_ref = f"60000000-0000-4000-8000-{index + 1:012d}"
        binding = {
            "batch_id": manifest["batch_id"], "manifest_fingerprint": fp,
            "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
            "actor_id": actor_id, "arm": assignment["arm"],
        }
        task_spec_sha256 = (manifest.get("task_specs") or {}).get(assignment["trial_id"])
        if isinstance(task_spec_sha256, str):
            binding["task_spec_sha256"] = task_spec_sha256
        fact = {
            "kind": "post_paste_attempt",
            "registration": {"status": "started", "reason": "registered"},
            "verified_paste": True,
            "outcome": "single_token",
            "classification": {"class": "term", "reason": None},
            "candidate_decision": "pending",
            "candidate_store": {"status": "completed", "reason": "candidate_db_returned"},
            "settings_store": {"status": "not_attempted", "reason": None},
            "attribution": "whole_field_difference_only",
        }
        activity = {
            "activity_id": activity_id, "sequence": "1", "observer_epoch": "1",
            "created_at": "Thu, 22 Oct 2026 00:00:01 GMT", "pair_ref": pair_ref,
            "lifecycle": "closed", "fact": fact,
        }
        def audit_pair(ref: str, key: str, before_surface: str, after_surface: str) -> dict:
            return {
                "pair_ref": ref, "pair_key": key,
                "before_surface": before_surface, "after_surface": after_surface,
                "rule_kind": "word_pair", "record_state": "pending", "candidate_hits": 1,
                "current_classification": {"class": "term", "reason": None},
                "current_promote_threshold": 3,
                "classification_authority": "current_owner_classification",
                "recorded_scope": {"family": None, "mapping_id": None,
                                   "browser_host": None, "native_bundle": None},
                "origin": "unknown_not_persisted", "tombstone_reason": None,
                "latest_record_change_at": "Thu, 22 Oct 2026 00:00:01 GMT",
                "last_output_surface_match_at": None,
                "current_dictionary_contains_after": False,
                "replacement_policy": "context_scoped_promoted_rules",
                "replacement_eligibility": "not_eligible_now",
                "eligibility_authority": "current_lexicon_owner_filter",
                "current_review": {"confirm": True, "ignore": True, "undo": False},
            }
        pair = audit_pair(pair_ref, f"pair-key-{index}-one", "acme", "acmecloud") if pair_ref else None
        if audit_fact is not None:
            fact = audit_fact
            activity["fact"] = fact
        activities = [activity]
        pairs = [pair] if pair_ref is not None else []
        if extra_candidate:
            extra_pair_ref = f"61000000-0000-4000-8000-{index + 1:012d}"
            activities.append({
                **activity,
                "activity_id": f"51000000-0000-4000-8000-{index + 1:012d}",
                "sequence": "2",
                "pair_ref": extra_pair_ref,
                "fact": dict(fact),
            })
            pairs.append(audit_pair(extra_pair_ref, f"pair-key-{index}-two", "voice", "voiceflow"))
        coverage = {
            "kind": "current_process", "capacity": 128, "dropped_entries": "0",
            "dropped_updates": "0", "oldest_sequence": None, "latest_sequence": None,
            "projection_consistency": "stable_revision", "atomicity": "not_cross_store_transaction",
            "consistency_scope": "audit_metadata_revision_only",
            "projection_mutation_interval": "not_observed", "projection_inflight_writers": None,
            "pair_projection": {"kind": "complete_within_budget", "max_rows": 1024,
                                "max_utf8_bytes": 2097152, "max_snapshot_encoded_bytes": 16777216},
        }
        def snapshot(revision: str, activities: list[dict], observer_epoch: str = "1") -> dict:
            current_coverage = {**coverage,
                                "oldest_sequence": activities[0]["sequence"] if activities else None,
                                "latest_sequence": activities[-1]["sequence"] if activities else None}
            return {"schema_version": 1, "runtime_id": runtime_id,
                    "audit_revision": revision, "clear_generation": "0", "observer_epoch": observer_epoch,
                    "coverage": current_coverage,
                    "pairs": pairs if activities else [], "activity": activities}
        owner = {
            "artifact_type": "voiceflow_dictionary_learning_owner_receipt", "schema_version": 1,
            "source": "voiceflow_installable_test_bounded_audit_v1", **binding,
            "build_sha256": "a" * 64,
            "test_bundle_identifier": pilot.PRE_C1_TEST_BUNDLE_IDENTIFIER,
            "capture_closed": True, "local_terminal": True,
            "coverage": "current_process_bounded_audit",
            "operator_attestation": {"live_hotkey_capture": True, "task_id_verified": True,
                                     "capture_closed": True},
            "audit_runtime_id": runtime_id, "activity_id": activity_id, "sequence": "1",
            "audit_before": snapshot("0", [], "0"), "audit_after": snapshot("2", activities),
        }
        owner_path = root / "owner" / f"{assignment['trial_id']}.json"
        owner_path.parent.mkdir(parents=True, exist_ok=True)
        self.write_json(owner_path, owner)
        owner_sha = hashlib.sha256(owner_path.read_bytes()).hexdigest()
        raw_hash = hashlib.sha256(f"raw-{index}".encode()).hexdigest()
        inserted_hash = hashlib.sha256(f"inserted-{index}".encode()).hexdigest()
        source = {
            "artifact_type": "voiceflow_dictionary_learning_source_capture_receipt", "schema_version": 1,
            "source": "voiceflow_installable_test_bounded_audit_v1", **binding,
            "owner_receipt_sha256": owner_sha, "audit_runtime_id": runtime_id,
            "activity_id": activity_id, "sequence": "1",
            "raw_asr_sha256": raw_hash, "inserted_text_sha256": inserted_hash,
        }
        source_path = root / "capture" / f"{assignment['trial_id']}.json"
        source_path.parent.mkdir(parents=True, exist_ok=True)
        self.write_json(source_path, source)
        row["source_capture"] = {"capture_id": activity_id, "recording_generation": "1",
                                 "live_capture_verified": "yes"}
        row["stage_hashes"] = {"raw_asr_sha256": raw_hash, "inserted_text_sha256": inserted_hash}
        for role, path in (("owner_receipt", owner_path), ("source_capture", source_path)):
            row["provenance"][role] = {
                "path": path.relative_to(root).as_posix(),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }

    def test_owner_lifecycle_rejects_renumbered_reverse_terminal_order(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        row = pilot.make_evidence_template(manifest)["trials"][0]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-owner-order-") as tmp:
            root = Path(tmp)
            self.add_live_capture_receipt(root, manifest, assignment, row, 0)
            owner_path = root / row["provenance"]["owner_receipt"]["path"]
            owner = json.loads(owner_path.read_text(encoding="utf-8"))
            owner["events"].reverse()
            for sequence, event in enumerate(owner["events"]):
                event["sequence"] = str(sequence)
            issues = []
            identity, run_id, generation, observer_fact = pilot.verify_owner_capture_receipt(
                owner, assignment["trial_id"], issues,
            )
        self.assertIsNone(identity)
        self.assertIsNone(run_id)
        self.assertIsNone(generation)
        self.assertIsNone(observer_fact)
        self.assertTrue(any(issue["field"] == "owner_receipt.lifecycle_order_invalid" for issue in issues))

    def test_research_owner_receipt_matches_native_zero_based_event_shape(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        row = pilot.make_evidence_template(manifest)["trials"][0]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-native-trace-") as tmp:
            root = Path(tmp)
            self.add_live_capture_receipt(root, manifest, assignment, row, 0)
            owner = json.loads((root / row["provenance"]["owner_receipt"]["path"]).read_text(encoding="utf-8"))
            issues = []
            identity, capture_id, generation, observer_fact = pilot.verify_owner_capture_receipt(
                owner, assignment["trial_id"], issues,
            )
        self.assertEqual([event["sequence"] for event in owner["events"]], ["0", "1", "2", "3"])
        self.assertEqual(owner["events"][0]["event"], "session_started")
        self.assertEqual(owner["events"][-1]["event"], "session_terminal")
        self.assertEqual(owner["events"][-1]["outcome"], "succeeded")
        self.assertIsNotNone(identity)
        self.assertEqual(capture_id, owner["run_id"])
        self.assertEqual(generation, owner["recording_generation"])
        self.assertIsNone(observer_fact["status"])
        self.assertFalse(observer_fact["human_acceptance_authority"])
        self.assertEqual(issues, [])

    def test_installable_test_bounded_audit_receipt_binds_live_hotkey_and_closed_activity(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        row = pilot.make_evidence_template(manifest)["trials"][0]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-test-audit-") as tmp:
            root = Path(tmp)
            self.add_installable_test_audit_receipt(root, manifest, assignment, row, 0)
            verified = {
                role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                for role in ("source_capture", "owner_receipt")
            }
            missing, issues = [], []
            identity, hashes_complete, owner_digest, observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
            expected_pair_ref = json.loads(
                (root / row["provenance"]["owner_receipt"]["path"]).read_text(encoding="utf-8")
            )["audit_after"]["activity"][0]["pair_ref"]
        self.assertTrue(hashes_complete)
        self.assertIsNotNone(identity)
        self.assertEqual(owner_digest, row["provenance"]["owner_receipt"]["sha256"])
        self.assertTrue(observer_fact["human_acceptance_authority"])
        self.assertIs(observer_fact["status"], True)
        self.assertEqual(observer_fact["candidate_refs"], [expected_pair_ref])
        self.assertEqual(missing, [])
        self.assertEqual(issues, [])

    def test_normal_audit_no_grant_is_known_miss_and_incomplete_registration_is_unknown(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        cases = (
            (
                "no_grant",
                {
                    "kind": "post_paste_attempt",
                    "registration": {"status": "skipped", "reason": "no_grant"},
                    "verified_paste": True,
                    "outcome": "not_observed",
                    "classification": None,
                    "candidate_decision": "not_attempted",
                    "candidate_store": {"status": "not_attempted", "reason": None},
                    "settings_store": {"status": "not_attempted", "reason": None},
                    "attribution": "not_observed",
                },
                False,
                "no_grant",
            ),
            (
                "registration_incomplete",
                {
                    "kind": "post_paste_attempt",
                    "registration": {"status": "checking", "reason": None},
                    "verified_paste": True,
                    "outcome": "observing",
                    "classification": None,
                    "candidate_decision": "not_attempted",
                    "candidate_store": {"status": "not_attempted", "reason": None},
                    "settings_store": {"status": "not_attempted", "reason": None},
                    "attribution": "unknown",
                },
                None,
                "registration_incomplete",
            ),
            (
                "style_signal_without_lexical_candidate",
                {
                    "kind": "post_paste_attempt",
                    "registration": {"status": "started", "reason": "registered"},
                    "verified_paste": True,
                    "outcome": "style_signal",
                    "classification": None,
                    "candidate_decision": "not_attempted",
                    "candidate_store": {"status": "completed", "reason": "style_draft_returned"},
                    "settings_store": {"status": "not_attempted", "reason": None},
                    "attribution": "not_observed",
                },
                False,
                "style_candidate_without_lexical_pair",
            ),
            (
                "registered_unreadable",
                {
                    "kind": "post_paste_attempt",
                    "registration": {"status": "started", "reason": "registered"},
                    "verified_paste": True,
                    "outcome": "unreadable",
                    "classification": None,
                    "candidate_decision": "not_attempted",
                    "candidate_store": {"status": "not_attempted", "reason": None},
                    "settings_store": {"status": "not_attempted", "reason": None},
                    "attribution": "not_observed",
                },
                False,
                "unreadable",
            ),
        )
        for index, (name, fact, expected_status, expected_reason) in enumerate(cases):
            with self.subTest(case=name), tempfile.TemporaryDirectory(prefix="dictionary-learning-audit-status-") as tmp:
                root = Path(tmp)
                row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                           if item["trial_id"] == assignment["trial_id"])
                self.add_installable_test_audit_receipt(
                    root, manifest, assignment, row, index, audit_fact=fact, include_pair=False,
                )
                verified = {
                    role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                    for role in ("source_capture", "owner_receipt")
                }
                missing, issues = [], []
                _identity, _hashes_complete, _owner_digest, observer_fact = pilot.verify_source_capture(
                    row, assignment, manifest, verified, root, missing, issues,
                )
                self.assertIsNotNone(observer_fact)
                self.assertIs(observer_fact["status"], expected_status)
                self.assertEqual(observer_fact["reason"], expected_reason)
                self.assertEqual(observer_fact["candidate_refs"], [])
                self.assertEqual(issues, [])

    def add_observer_independent_label(
        self, root: Path, manifest: dict, assignment: dict, row: dict,
        owner_digest: str, observer_fact: dict, outcomes: dict,
        *, candidate_refs: list[str] | None = None,
    ) -> None:
        actor_id = assignment["operator_id"] if manifest["stage"] == "technical" else assignment["participant_id"]
        label = {
            "artifact_type": "voiceflow_dictionary_learning_independent_label",
            "schema_version": 1,
            "batch_id": manifest["batch_id"],
            "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
            "phase_id": manifest["phase_id"],
            "trial_id": assignment["trial_id"],
            "actor_id": actor_id,
            "arm": assignment["arm"],
            "labeler_id": "unit-reviewer",
            "reviewed_before_outcome": True,
            "reference_sha256": (manifest.get("task_specs") or {}).get(assignment["trial_id"]),
            **outcomes,
            "observer_owner_receipt_sha256": owner_digest,
            "observer_capture_id": observer_fact.get("capture_id") or row["source_capture"]["capture_id"],
            "observer_candidate_refs": (
                observer_fact.get("candidate_refs", []) if candidate_refs is None else candidate_refs
            ),
        }
        path = root / "independent_label" / f"{assignment['trial_id']}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        self.write_json(path, label)
        row["provenance"]["independent_label"] = {
            "path": path.relative_to(root).as_posix(),
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        }

    def test_no_grant_owner_receipt_overrules_hashed_positive_capture_claim(self):
        manifest = AllocationTests().technical_draft()
        assignment = next(row for row in manifest["assignments"] if row["category"] == "a_positive")
        row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                   if item["trial_id"] == assignment["trial_id"])
        row["outcomes"].update({
            "alias_exposure": "yes", "alias_eligible": "yes", "alias_applied_correctly": "yes",
            "new_unintended_mutation": "no", "actual_correction": "yes", "eligible_correction": "yes",
            "observer_captured": "yes", "observer_correct": "yes",
        })
        no_grant = {
            "kind": "post_paste_attempt",
            "registration": {"status": "skipped", "reason": "no_grant"},
            "verified_paste": True,
            "outcome": "not_observed",
            "classification": None,
            "candidate_decision": "not_attempted",
            "candidate_store": {"status": "not_attempted", "reason": None},
            "settings_store": {"status": "not_attempted", "reason": None},
            "attribution": "not_observed",
        }
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-no-grant-label-") as tmp:
            root = Path(tmp)
            self.add_installable_test_audit_receipt(
                root, manifest, assignment, row, 0, audit_fact=no_grant, include_pair=False,
            )
            verified = {
                role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                for role in ("source_capture", "owner_receipt")
            }
            missing, issues = [], []
            _identity, _hashes, owner_digest, observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
            self.assertIs(observer_fact["status"], False)
            self.add_observer_independent_label(
                root, manifest, assignment, row, owner_digest, observer_fact, row["outcomes"],
            )
            verified["independent_label"] = pilot.verify_artifact_ref(
                root, row["provenance"]["independent_label"], assignment["trial_id"], "independent_label", [],
            )
            result = pilot.validate_human_and_task_receipts(
                row, assignment, manifest, root, verified, row["outcomes"], observer_fact, owner_digest,
                missing, issues,
            )
        _human_ok, independent_ok, _terminal_ok, _label, _terminal, _human, label_core_ok, binding_ok = result
        # The terminal label remains independently valid; observer disagreement
        # is tracked by the binding/observer gates and integrity report.
        self.assertTrue(independent_ok)
        self.assertTrue(label_core_ok)
        self.assertFalse(binding_ok)
        self.assertIn("independent_label.observer_capture_fact_mismatch", [issue["field"] for issue in issues])
        self.assertTrue(any(issue["field"] == "independent_label.observer_correct_without_bound_candidate"
                            for issue in issues))

    def test_grouped_native_candidates_require_exact_cross_candidate_label_binding(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                   if item["trial_id"] == assignment["trial_id"])
        outcomes = {
            "alias_exposure": "yes", "alias_eligible": "yes", "alias_applied_correctly": "yes",
            "new_unintended_mutation": "no", "actual_correction": "yes", "eligible_correction": "yes",
            "observer_captured": "yes", "observer_correct": "unknown",
        }
        row["outcomes"].update(outcomes)
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-multi-candidate-") as tmp:
            root = Path(tmp)
            self.add_installable_test_audit_receipt(
                root, manifest, assignment, row, 0, extra_candidate=True,
            )
            verified = {
                role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                for role in ("source_capture", "owner_receipt")
            }
            missing, issues = [], []
            _identity, _hashes, owner_digest, observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
            self.assertIs(observer_fact["status"], True)
            self.assertEqual(len(observer_fact["candidate_refs"]), 2)
            self.add_observer_independent_label(
                root, manifest, assignment, row, owner_digest, observer_fact, outcomes,
            )
            verified["independent_label"] = pilot.verify_artifact_ref(
                root, row["provenance"]["independent_label"], assignment["trial_id"], "independent_label", [],
            )
            valid = pilot.validate_human_and_task_receipts(
                row, assignment, manifest, root, verified, outcomes, observer_fact, owner_digest,
                missing, issues,
            )
            correct_label = json.loads(
                (root / row["provenance"]["independent_label"]["path"]).read_text(encoding="utf-8")
            )
            correct_label["observer_candidate_refs"] = observer_fact["candidate_refs"][:-1]
            label_path = root / row["provenance"]["independent_label"]["path"]
            self.write_json(label_path, correct_label)
            row["provenance"]["independent_label"]["sha256"] = hashlib.sha256(label_path.read_bytes()).hexdigest()
            verified["independent_label"] = pilot.verify_artifact_ref(
                root, row["provenance"]["independent_label"], assignment["trial_id"], "independent_label", [],
            )
            missing, mismatch_issues = [], []
            mismatched = pilot.validate_human_and_task_receipts(
                row, assignment, manifest, root, verified, outcomes, observer_fact, owner_digest,
                missing, mismatch_issues,
            )
        self.assertTrue(valid[1])
        self.assertFalse(valid[7])  # A grouped capture cannot receive a single-candidate correctness label.
        self.assertTrue(mismatched[1])
        self.assertFalse(mismatched[7])
        self.assertIn("independent_label.observer_candidate_refs_mismatch",
                      [issue["field"] for issue in mismatch_issues])

    def test_b_conflict_probe_counts_only_task_bound_native_review(self):
        item = {
            "outcomes": {"b_conflict_probe": "yes"},
            "independent_label_ok": True,
            "independent_label": {
                "b_conflict_probe_result": pilot.B_CONFLICT_RESULT_CODE,
                "observer_owner_receipt_sha256": "a" * 64,
                "observer_capture_id": "capture-1",
            },
            "observer_fact": {
                "source": "bounded_audit",
                "human_acceptance_authority": True,
                "b_conflict_probe_review_action": True,
                "capture_id": "capture-1",
            },
            "row": {"source_capture": {"capture_id": "capture-1"}},
            "source_receipt_digest": "a" * 64,
            "distinct_recording": True,
        }
        self.assertTrue(pilot.verified_b_conflict_probe(item))

        for change in (
            {"observer_fact": {**item["observer_fact"], "b_conflict_probe_review_action": False}},
            {"independent_label": {**item["independent_label"],
                                   "b_conflict_probe_result": "store_error"}},
            {"independent_label": {**item["independent_label"],
                                   "observer_owner_receipt_sha256": "b" * 64}},
            {"independent_label_ok": False},
            {"distinct_recording": False},
        ):
            with self.subTest(change=change):
                invalid = {**item, **change}
                self.assertFalse(pilot.verified_b_conflict_probe(invalid))

    def test_duplicate_native_runtime_epoch_across_trials_is_unknown_capture(self):
        manifest = AllocationTests().technical_draft()
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        rows = {row["trial_id"]: row for row in evidence["trials"]}
        assignments = [row for row in manifest["assignments"] if row["category"] == "a_positive"][:2]
        shared_runtime_id = "40000000-0000-4000-8000-999999999999"
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-duplicate-runtime-") as tmp:
            root = Path(tmp)
            for index, assignment in enumerate(assignments):
                row = rows[assignment["trial_id"]]
                row["observed_phase_id"] = manifest["phase_id"]
                row["observed_arm"] = assignment["arm"]
                row["observed_operator_id"] = assignment["operator_id"]
                row["state_integrity"] = "clean"
                row["outcomes"].update({
                    "actual_correction": "yes", "eligible_correction": "yes",
                    "observer_captured": "yes", "observer_correct": "unknown",
                    "alias_exposure": "yes", "alias_eligible": "yes",
                    "alias_applied_correctly": "yes", "new_unintended_mutation": "no",
                })
                self.add_installable_test_audit_receipt(
                    root, manifest, assignment, row, index, runtime_id=shared_runtime_id,
                )
                verified = {
                    role: pilot.verify_artifact_ref(
                        root, row["provenance"][role], assignment["trial_id"], role, [],
                    )
                    for role in ("source_capture", "owner_receipt")
                }
                missing, issues = [], []
                identity, _hashes_complete, owner_digest, observer_fact = pilot.verify_source_capture(
                    row, assignment, manifest, verified, root, missing, issues,
                )
                self.assertEqual(identity, f"test:{shared_runtime_id}:1")
                self.assertTrue(observer_fact["human_acceptance_authority"])
                self.add_observer_independent_label(
                    root, manifest, assignment, row, owner_digest, observer_fact, row["outcomes"],
                )

            report = pilot.analyze_technical(manifest, evidence, root)
        observer = report["gates"]["observer_correction_accounting"]
        self.assertEqual(observer["eligible_corrections"], 2)
        self.assertEqual(observer["eligible_captured_corrections"], 0)
        self.assertEqual(observer["eligible_capture_unknown"], 2)
        self.assertIsNone(observer["eligible_capture_recall"])
        self.assertTrue(set(row["trial_id"] for row in assignments).issubset(observer["unknown_trials"]))

    def test_positive_native_owner_without_source_receipt_is_unknown_capture(self):
        manifest = AllocationTests().technical_draft()
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        assignment = next(row for row in manifest["assignments"] if row["category"] == "a_positive")
        row = next(item for item in evidence["trials"] if item["trial_id"] == assignment["trial_id"])
        row.update({
            "observed_phase_id": manifest["phase_id"],
            "observed_arm": assignment["arm"],
            "observed_operator_id": assignment["operator_id"],
            "state_integrity": "clean",
        })
        row["outcomes"].update({
            "actual_correction": "yes", "eligible_correction": "yes",
            "observer_captured": "yes", "observer_correct": "unknown",
            "alias_exposure": "yes", "alias_eligible": "yes",
            "alias_applied_correctly": "yes", "new_unintended_mutation": "no",
        })
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-owner-without-source-") as tmp:
            root = Path(tmp)
            self.add_installable_test_audit_receipt(root, manifest, assignment, row, 0)
            source_path = root / row["provenance"]["source_capture"]["path"]
            source_path.unlink()
            verified = {"owner_receipt": pilot.verify_artifact_ref(
                root, row["provenance"]["owner_receipt"], assignment["trial_id"], "owner_receipt", [],
            )}
            missing, issues = [], []
            identity, _hashes, owner_digest, observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
            self.assertIsNone(identity)
            self.assertIsNone(observer_fact)
            unbound_fact = {
                "capture_id": row["source_capture"]["capture_id"],
                "candidate_refs": [],
            }
            self.add_observer_independent_label(
                root, manifest, assignment, row, owner_digest, unbound_fact, row["outcomes"],
            )
            report = pilot.analyze_technical(manifest, evidence, root)
        observer = report["gates"]["observer_correction_accounting"]
        self.assertEqual(observer["eligible_corrections"], 1)
        self.assertEqual(observer["eligible_captured_corrections"], 0)
        self.assertEqual(observer["eligible_capture_unknown"], 1)
        self.assertIsNone(observer["eligible_capture_recall"])
        self.assertIn(assignment["trial_id"], observer["unknown_trials"])

    def test_actual_human_evidence_cannot_use_research_stdio_as_observer_authority(self):
        manifest = AllocationTests().technical_draft()
        evidence = pilot.make_evidence_template(manifest)
        evidence.update({
            "evidence_class": "actual_human",
            "human_data_attested": True,
            "local_only_confirmed": True,
            "research_authorization_confirmed": True,
        })
        assignment = next(row for row in manifest["assignments"] if row["category"] == "a_positive")
        row = next(item for item in evidence["trials"] if item["trial_id"] == assignment["trial_id"])
        row.update({
            "observed_phase_id": manifest["phase_id"],
            "observed_arm": assignment["arm"],
            "observed_operator_id": assignment["operator_id"],
            "state_integrity": "clean",
        })
        row["outcomes"].update({
            "actual_correction": "yes", "eligible_correction": "yes",
            "observer_captured": "unknown", "observer_correct": "unknown",
            "alias_exposure": "yes", "alias_eligible": "yes",
            "alias_applied_correctly": "yes", "new_unintended_mutation": "no",
        })
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-human-stdio-") as tmp:
            root = Path(tmp)
            self.add_live_capture_receipt(root, manifest, assignment, row, 0)
            verified = {
                role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                for role in ("source_capture", "owner_receipt")
            }
            missing, issues = [], []
            _identity, _hashes, owner_digest, observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
            self.assertEqual(observer_fact["source"], "research_stdio")
            self.assertIsNone(observer_fact["status"])
            self.assertFalse(observer_fact["human_acceptance_authority"])
            self.add_observer_independent_label(
                root, manifest, assignment, row, owner_digest, observer_fact, row["outcomes"],
            )
            report = pilot.analyze_technical(manifest, evidence, root)
        observer = report["gates"]["observer_correction_accounting"]
        self.assertEqual(observer["eligible_corrections"], 1)
        self.assertEqual(observer["eligible_captured_corrections"], 0)
        self.assertEqual(observer["eligible_capture_unknown"], 1)
        self.assertIsNone(observer["eligible_capture_recall"])
        self.assertIn(assignment["trial_id"], observer["unknown_trials"])

    def test_installable_test_audit_requires_one_epoch_advance_and_activity_on_new_epoch(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        row = pilot.make_evidence_template(manifest)["trials"][0]
        cases = (
            ("two_epoch_jump", "owner_receipt.audit_task_delta_invalid"),
            ("activity_on_old_epoch", "owner_receipt.audit_activity_epoch_mismatch"),
        )
        for case, expected_issue in cases:
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix="dictionary-learning-audit-epoch-") as tmp:
                root = Path(tmp)
                self.add_installable_test_audit_receipt(root, manifest, assignment, row, 0)
                owner_path = root / row["provenance"]["owner_receipt"]["path"]
                owner = json.loads(owner_path.read_text(encoding="utf-8"))
                if case == "two_epoch_jump":
                    owner["audit_after"]["observer_epoch"] = "2"
                else:
                    owner["audit_after"]["activity"][0]["observer_epoch"] = "0"
                issues = []
                identity, capture_id, generation, observer_fact = pilot.verify_owner_capture_receipt(
                    owner, assignment["trial_id"], issues,
                )
                self.assertIsNone(identity)
                self.assertIsNone(capture_id)
                self.assertIsNone(generation)
                self.assertIsNone(observer_fact)
                self.assertTrue(any(issue["field"] == expected_issue for issue in issues), issues)

    def test_cancelled_native_trace_cannot_claim_delivery(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["assignments"][0]
        row = pilot.make_evidence_template(manifest)["trials"][0]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-cancelled-trace-") as tmp:
            root = Path(tmp)
            self.add_live_capture_receipt(root, manifest, assignment, row, 0)
            owner_path = root / row["provenance"]["owner_receipt"]["path"]
            source_path = root / row["provenance"]["source_capture"]["path"]
            owner = json.loads(owner_path.read_text(encoding="utf-8"))
            terminal = owner["events"].pop()
            cancel = {key: terminal[key] for key in (
                "run_id", "case_id", "job_id", "recording_generation", "config_generation",
            )}
            cancel.update({"event": "cancellation_requested", "reason": "explicit_cancel"})
            owner["events"].extend([cancel, terminal])
            terminal["outcome"] = "cancelled"
            terminal["side_effect_boundary"] = "no_delivery_write_observed"
            for sequence, event in enumerate(owner["events"]):
                event["sequence"] = str(sequence)
                event["monotonic_ns"] = str(1000 + sequence)
            self.write_json(owner_path, owner)
            owner_sha = hashlib.sha256(owner_path.read_bytes()).hexdigest()
            source = json.loads(source_path.read_text(encoding="utf-8"))
            source["owner_receipt_sha256"] = owner_sha
            self.write_json(source_path, source)
            row["provenance"]["owner_receipt"]["sha256"] = owner_sha
            row["provenance"]["source_capture"]["sha256"] = hashlib.sha256(source_path.read_bytes()).hexdigest()
            row["outcomes"]["actual_delivery_verified"] = "yes"
            verified = {
                role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                for role in ("source_capture", "owner_receipt")
            }
            missing, issues = [], []
            identity, _, _, _observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
        self.assertIsNone(identity)
        self.assertIn("owner_terminal_delivery_contradiction", [issue["field"] for issue in issues])

    def test_critical_harm_gate_uses_complete_independent_safety_observations(self):
        manifest = AllocationTests().benefit_draft()
        stage_data = {}
        for index, assignment in enumerate(manifest["assignments"]):
            stage_data[assignment["trial_id"]] = {
                "assignment": assignment,
                "outcomes": {name: "no" for name in pilot.SAFETY_HARM_FIELDS},
                "terminal_complete": index < 159,
                "safety_observation_valid": True,
                "safety_observation_complete": True,
                "provenance": {role: True for role in pilot.BENEFIT_PROVENANCE_ROLES},
                "stage_hashes_complete": True,
                "capture_identity": f"capture-{index}",
                "provenance_typed_receipts_complete": True,
            }
        ordinary = next(item for item in stage_data.values()
                        if item["assignment"]["category"] == "ordinary")
        ordinary["outcomes"]["terminal_evidence_verified"] = "unknown"
        self.assertEqual(sum(item["terminal_complete"] for item in stage_data.values()), 159)
        self.assertNotIn("target_terminal", pilot.BENEFIT_PROVENANCE_ROLES)
        self.assertEqual(pilot.provenance_gate_status(
            stage_data, [], pilot.BENEFIT_PROVENANCE_ROLES, [],
        )[0], "pass")
        self.assertEqual(pilot.safety_harm_gate(stage_data)["status"], "pass")

        safety_item = next(item for item in stage_data.values()
                           if item["assignment"]["category"].startswith("safety_"))
        safety_item["outcomes"]["resurrection"] = "unknown"
        safety_item["safety_observation_complete"] = False
        self.assertEqual(pilot.safety_harm_gate(stage_data)["status"], "pending")

        safety_item["outcomes"]["unauthorized_output"] = "yes"
        safety_item["safety_observation_complete"] = False
        safety_item["terminal_complete"] = False
        self.assertEqual(pilot.safety_harm_gate(stage_data)["status"], "fail")

    def test_safety_observation_receipt_is_task_bound_and_independent(self):
        manifest = AllocationTests().frozen_benefit()
        assignment = manifest["assignments"][0]
        row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                   if item["trial_id"] == assignment["trial_id"])
        outcomes = {name: "no" for name in pilot.SAFETY_HARM_FIELDS}
        receipt = {
            "artifact_type": "voiceflow_dictionary_learning_safety_observation", "schema_version": 1,
            "batch_id": manifest["batch_id"], "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
            "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
            "actor_id": assignment["participant_id"], "arm": assignment["arm"],
            "labeler_id": "independent-labeler", "safety_review_complete": True,
            "outcomes": outcomes,
        }
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-safety-observation-") as tmp:
            root = Path(tmp)
            path = root / "safety.json"
            self.write_json(path, receipt)
            row["provenance"]["safety_observation"] = {
                "path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
            missing, issues = [], []
            verified = {"safety_observation": pilot.verify_artifact_ref(
                root, row["provenance"]["safety_observation"], assignment["trial_id"], "safety_observation", missing,
            )}
            valid, complete = pilot.validate_safety_observation(
                row, assignment, manifest, root, verified, outcomes, missing, issues,
            )
            self.assertTrue(valid)
            self.assertTrue(complete)

            receipt["labeler_id"] = assignment["participant_id"]
            self.write_json(path, receipt)
            row["provenance"]["safety_observation"]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            missing, issues = [], []
            verified = {"safety_observation": pilot.verify_artifact_ref(
                root, row["provenance"]["safety_observation"], assignment["trial_id"], "safety_observation", missing,
            )}
            valid, complete = pilot.validate_safety_observation(
                row, assignment, manifest, root, verified, outcomes, missing, issues,
            )
        self.assertFalse(valid)
        self.assertFalse(complete)
        self.assertIn("safety_observation_not_independent_or_incomplete",
                      [issue["field"] for issue in issues])

    def test_analyze_benefit_accepts_159_of_160_terminals_with_complete_safety(self):
        manifest = AllocationTests().frozen_benefit()
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-benefit-e2e-") as tmp:
            root = Path(tmp)
            task_dir = root / "task-specs"
            task_dir.mkdir()
            task_hashes = {}
            for assignment in manifest["assignments"]:
                task_path = task_dir / f"{assignment['trial_id']}.txt"
                task_path.write_text(f"synthetic task spec {assignment['trial_id']}\n", encoding="utf-8")
                task_hashes[assignment["trial_id"]] = hashlib.sha256(task_path.read_bytes()).hexdigest()
            manifest["task_specs"] = task_hashes
            manifest["freeze_fingerprint"] = pilot.sha256_json({
                key: value for key, value in manifest.items() if key != "freeze_fingerprint"
            })
            fp = pilot.manifest_fingerprint(manifest)
            evidence = pilot.make_evidence_template(manifest)
            evidence.update({
                "evidence_class": "unit_fixture",
                "human_data_attested": False,
                "local_only_confirmed": False,
                "research_authorization_confirmed": False,
            })
            evidence["participant_authorization"] = {participant_id: False for participant_id in manifest["participants"]}
            rows = {row["trial_id"]: row for row in evidence["trials"]}
            missing_terminal = next(row for row in manifest["assignments"] if row["category"] == "ordinary")["trial_id"]
            unknown_control_target = next(row for row in manifest["assignments"]
                                          if row["category"] == "target" and row["arm"] == "control")["trial_id"]

            def write_bound(row, assignment, role, artifact_type, extra):
                actor_id = assignment["participant_id"]
                document = {
                    "artifact_type": artifact_type, "schema_version": 1,
                    "batch_id": manifest["batch_id"], "manifest_fingerprint": fp,
                    "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
                    "actor_id": actor_id, "arm": assignment["arm"],
                    **extra,
                }
                path = root / role / f"{assignment['trial_id']}.json"
                path.parent.mkdir(parents=True, exist_ok=True)
                self.write_json(path, document)
                row["provenance"][role] = {
                    "path": path.relative_to(root).as_posix(),
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                }

            for index, assignment in enumerate(manifest["assignments"]):
                trial_id = assignment["trial_id"]
                row = rows[trial_id]
                participant_id = assignment["participant_id"]
                row.update({
                    "observed_phase_id": manifest["phase_id"],
                    "observed_arm": assignment["arm"],
                    "observed_participant_id": participant_id,
                    "state_integrity": "clean",
                })
                target = assignment["category"] == "target"
                success = not target or assignment["arm"] == "teach"
                outcomes = row["outcomes"]
                outcomes.update({
                    "terminal_evidence_verified": "unknown" if trial_id == missing_terminal else "yes",
                    "intended_output_correct": "unknown" if trial_id == unknown_control_target else ("yes" if success else "no"),
                    "actual_delivery_verified": "yes",
                    "manual_edit_required": "no" if success or trial_id == unknown_control_target else "yes",
                    **{field: "no" for field in pilot.SAFETY_HARM_FIELDS},
                })
                if target:
                    row["completion_ms"] = 100 if assignment["arm"] == "teach" else 200

                task_ref = {"path": f"task-specs/{trial_id}.txt", "sha256": task_hashes[trial_id]}
                row["provenance"]["task_spec"] = task_ref
                self.add_installable_test_audit_receipt(root, manifest, assignment, row, index)
                write_bound(row, assignment, "human_attestation",
                            "voiceflow_dictionary_learning_human_attestation", {
                                "subject_role": "participant", "human_data_attested": True,
                                "local_only_confirmed": True, "research_authorization_confirmed": True,
                                "consent_confirmed": True,
                            })
                write_bound(row, assignment, "independent_label",
                            "voiceflow_dictionary_learning_independent_label", {
                                "labeler_id": "unit-reviewer", "reviewed_before_outcome": True,
                                "reference_sha256": task_hashes[trial_id],
                                **{key: outcomes[key] for key in (
                                    "intended_output_correct", "manual_edit_required", *pilot.SAFETY_HARM_FIELDS,
                                )},
                            })
                write_bound(row, assignment, "safety_observation",
                            "voiceflow_dictionary_learning_safety_observation", {
                                "labeler_id": "unit-reviewer", "safety_review_complete": True,
                                "outcomes": {key: outcomes[key] for key in pilot.SAFETY_HARM_FIELDS},
                            })
                if trial_id != missing_terminal:
                    write_bound(row, assignment, "target_terminal",
                                "voiceflow_dictionary_learning_terminal_receipt", {
                                    "terminal_verified": outcomes["terminal_evidence_verified"],
                                    "actual_delivery_verified": outcomes["actual_delivery_verified"],
                                })
                active = manifest["freeze_facts"]["active_rule_families_by_participant"][participant_id]
                hint = manifest["freeze_facts"]["effective_hint_sha256_by_participant"][participant_id]
                for boundary, role in (("before", "state_before"), ("after", "state_after")):
                    state = {
                        "artifact_type": "voiceflow_dictionary_learning_state_snapshot", "schema_version": 1,
                        "batch_id": manifest["batch_id"], "manifest_fingerprint": fp,
                        "phase_id": manifest["phase_id"], "trial_id": trial_id,
                        "actor_id": participant_id, "arm": assignment["arm"],
                        "state_contract": assignment["state_contract"], "boundary": boundary,
                        "global_dictionary_sha256": "a" * 64, "rule_inventory_sha256": "b" * 64,
                        "tombstone_inventory_sha256": "c" * 64, "active_rule_families": active,
                        "effective_hint_sha256": hint, "passive_learning_enabled": False,
                        "target_rule_state": "not_applicable",
                    }
                    seal_state_snapshot(state)
                    path = root / role / f"{trial_id}.json"
                    path.parent.mkdir(parents=True, exist_ok=True)
                    self.write_json(path, state)
                    row["provenance"][role] = {
                        "path": path.relative_to(root).as_posix(),
                        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                    }

            report = pilot.analyze_benefit(manifest, evidence, root)
            self.assertEqual(report["evidence_class"], "unit_fixture")
            self.assertEqual(report["status"], "pending")  # Synthetic input cannot become human acceptance.
            self.assertEqual(report["gates"]["terminal_evidence_coverage"]["status"], "pass")
            self.assertEqual(report["gates"]["terminal_evidence_coverage"]["complete"], 159)
            primary = report["gates"]["target_word_correct_delivery_without_manual_edit"]
            self.assertEqual(primary["teach"]["denominator"], 40)
            self.assertEqual(primary["control"]["denominator"], 40)
            self.assertEqual(primary["control"]["successes"], 0)
            self.assertEqual(primary["unknown_target_outcomes"], 1)
            self.assertFalse(primary["unknowns_count_as_success"])
            self.assertEqual(primary["status"], "pending")
            closed_report = pilot.analyze_benefit(manifest, evidence, root, close=True)
            self.assertEqual(closed_report["gates"]["target_word_correct_delivery_without_manual_edit"]["status"], "pass")
            self.assertEqual(report["gates"]["positive_and_negative_stage_provenance"]["status"], "pass")
            self.assertEqual(report["gates"]["critical_cross_scope_unauthorized_contamination_and_resurrection"]["status"], "pass")

            negative = next(row for row in manifest["assignments"] if row["category"].startswith("safety_"))
            rows[negative["trial_id"]]["provenance"].pop("safety_observation")
            no_safety_report = pilot.analyze_benefit(manifest, evidence, root)
        self.assertEqual(no_safety_report["gates"]["terminal_evidence_coverage"]["status"], "pass")
        self.assertEqual(no_safety_report["gates"]["critical_cross_scope_unauthorized_contamination_and_resurrection"]["status"], "pending")
        self.assertEqual(no_safety_report["gates"]["positive_and_negative_stage_provenance"]["status"], "pending")

    def test_benefit_state_snapshots_bind_unique_family_and_hint_freeze(self):
        manifest = AllocationTests().frozen_benefit()
        pilot.manifest_fingerprint(manifest)
        assignment = next(row for row in manifest["assignments"]
                          if row["category"] == "target" and row["arm"] == "teach")
        row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                   if item["trial_id"] == assignment["trial_id"])
        participant_id = assignment["participant_id"]
        active = manifest["freeze_facts"]["active_rule_families_by_participant"][participant_id]
        hint = manifest["freeze_facts"]["effective_hint_sha256_by_participant"][participant_id]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-frozen-state-") as tmp:
            root = Path(tmp)
            verified = {}
            for boundary, role in (("before", "state_before"), ("after", "state_after")):
                receipt = {
                    "artifact_type": "voiceflow_dictionary_learning_state_snapshot", "schema_version": 1,
                    "batch_id": manifest["batch_id"], "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
                    "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
                    "actor_id": participant_id, "arm": assignment["arm"],
                    "state_contract": assignment["state_contract"], "boundary": boundary,
                    "global_dictionary_sha256": "a" * 64, "rule_inventory_sha256": "b" * 64,
                    "tombstone_inventory_sha256": "c" * 64, "active_rule_families": active,
                    "effective_hint_sha256": hint, "passive_learning_enabled": False,
                    "target_rule_state": "not_applicable",
                }
                seal_state_snapshot(receipt)
                path = root / f"{boundary}.json"
                self.write_json(path, receipt)
                row["provenance"][role] = {"path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                verified[role] = True
            missing, issues = [], []
            complete, transition_ok, _snapshots = pilot.validate_state_receipts(
                row, assignment, manifest, root, verified, missing, issues,
            )
            self.assertTrue(complete)
            self.assertTrue(transition_ok)

            after_path = root / row["provenance"]["state_after"]["path"]
            after = json.loads(after_path.read_text(encoding="utf-8"))
            after["effective_hint_sha256"] = "0" * 64
            seal_state_snapshot(after)
            self.write_json(after_path, after)
            row["provenance"]["state_after"]["sha256"] = hashlib.sha256(after_path.read_bytes()).hexdigest()
            missing, issues = [], []
            complete, transition_ok, _snapshots = pilot.validate_state_receipts(
                row, assignment, manifest, root, verified, missing, issues,
            )
        self.assertTrue(complete)
        self.assertFalse(transition_ok)
        self.assertTrue(any(any("hint_not_bound_to_frozen_hint" in value for value in issue.get("differences", []))
                            for issue in issues))

        with tempfile.TemporaryDirectory(prefix="dictionary-learning-family-inventory-") as tmp:
            root = Path(tmp)
            verified = {}
            for boundary, role in (("before", "state_before"), ("after", "state_after")):
                receipt = {
                    "artifact_type": "voiceflow_dictionary_learning_state_snapshot", "schema_version": 1,
                    "batch_id": manifest["batch_id"], "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
                    "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
                    "actor_id": participant_id, "arm": assignment["arm"],
                    "state_contract": assignment["state_contract"], "boundary": boundary,
                    "global_dictionary_sha256": "a" * 64, "rule_inventory_sha256": "b" * 64,
                    "tombstone_inventory_sha256": "c" * 64, "active_rule_families": active,
                    "effective_hint_sha256": hint, "passive_learning_enabled": False,
                    "target_rule_state": "not_applicable",
                }
                if boundary == "after":
                    receipt["active_rule_families"] = [active[0], active[1], active[2], active[3], active[0]]
                seal_state_snapshot(receipt)
                path = root / f"{boundary}.json"
                self.write_json(path, receipt)
                row["provenance"][role] = {"path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                verified[role] = True
            missing, issues = [], []
            complete, transition_ok, _snapshots = pilot.validate_state_receipts(
                row, assignment, manifest, root, verified, missing, issues,
            )
        self.assertTrue(complete)
        self.assertFalse(transition_ok)
        self.assertTrue(any("benefit_arm_state" in issue.get("differences", []) for issue in issues))

    def test_technical_state_sequence_allows_only_frozen_forget_boundary(self):
        manifest = AllocationTests().frozen_technical()
        initial = manifest["freeze_facts"]["initial_state_sha256"]
        forgotten = "d" * 64
        stage_data = {}
        for assignment in manifest["assignments"]:
            fingerprint = forgotten if assignment["category"] == "forget_restart" else initial
            target_state = "forgotten" if assignment["category"] == "forget_restart" else "not_applicable"
            stage_data[assignment["trial_id"]] = {
                "assignment": assignment,
                "state_receipts_complete": True,
                "state_transition_ok": True,
                "typed_receipts_complete": True,
                "state_snapshots": {
                    boundary: {"fingerprint": fingerprint, "active_fingerprint": fingerprint,
                               "target_rule_state": target_state}
                    for boundary in ("before", "after")
                },
            }
        issues = []
        self.assertEqual(pilot.validate_technical_state_sequence(manifest, stage_data, issues), [])
        self.assertEqual(issues, [])

        ordinary = next(item for item in stage_data.values()
                        if item["assignment"]["category"] != "forget_restart")
        ordinary["state_snapshots"]["before"]["active_fingerprint"] = "9" * 64
        ordinary["state_snapshots"]["after"]["active_fingerprint"] = "9" * 64
        issues = []
        violations = pilot.validate_technical_state_sequence(manifest, stage_data, issues)
        self.assertIn(ordinary["assignment"]["trial_id"], violations)
        self.assertIn(ordinary["assignment"]["trial_id"],
                      [issue["item_id"] for issue in issues])

    def test_zero_human_evidence_stays_pending_with_full_denominator(self):
        draft_args = type("Args", (), {
            "batch_id": "pending-unit",
            "seed": "pending-unit-seed",
            "operator": ["OP-A", "OP-B"],
            "non_implementer": "OP-B",
        })()
        manifest = pilot.make_technical_draft(draft_args)
        evidence = pilot.make_evidence_template(manifest)
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-pilot-unit-") as tmp:
            root = Path(tmp)
            manifest_path = root / "manifest.json"
            evidence_path = root / "evidence.json"
            self.write_json(manifest_path, manifest)
            self.write_json(evidence_path, evidence)
            report = pilot.analyze(type("Args", (), {
                "manifest": str(manifest_path),
                "evidence": str(evidence_path),
                "artifact_root": None,
            })())
        self.assertEqual(report["status"], "pending")
        self.assertEqual(report["human_gate_status"], "pending")
        self.assertEqual(report["evidence_class"], "unfilled")
        self.assertEqual(report["assigned_trial_count"], 40)
        self.assertEqual(report["gates"]["frozen_manifest"]["status"], "pending")

    def test_close_mode_turns_unresolved_human_gates_into_failures(self):
        open_report = {"status": "pending", "evidence_class": "actual_human", "human_gate_status": "pending",
                       "gates": {"evidence": {"status": "pending"}, "harm": {"status": "fail"}}}
        closed = pilot.apply_close_mode(open_report, True)
        self.assertEqual(closed["close_mode"], "closed")
        self.assertEqual(closed["gates"]["evidence"]["status"], "fail")
        self.assertTrue(closed["gates"]["evidence"]["closed_with_missing_evidence"])
        self.assertEqual(closed["status"], "fail")

    def test_unit_fixture_never_enters_human_gates(self):
        draft_args = type("Args", (), {
            "batch_id": "fixture-only",
            "seed": "fixture-only-seed",
            "operator": ["OP-A", "OP-B"],
            "non_implementer": "OP-B",
        })()
        manifest = pilot.make_technical_draft(draft_args)
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-pilot-unit-") as tmp:
            manifest_path = Path(tmp) / "manifest.json"
            self.write_json(manifest_path, manifest)
            report = pilot.analyze(type("Args", (), {
                "manifest": str(manifest_path),
                "evidence": str(FIXTURE),
                "artifact_root": None,
            })())
        self.assertEqual(report["status"], "fixture_only")
        self.assertEqual(report["human_gate_status"], "pending")
        self.assertEqual(report["gates"], {})

    def test_unknown_trials_keep_fixed_denominators_and_provenance_pending(self):
        draft_args = type("Args", (), {
            "batch_id": "denominator-unit",
            "seed": "denominator-unit-seed",
            "operator": ["OP-A", "OP-B"],
            "non_implementer": "OP-B",
        })()
        manifest = pilot.make_technical_draft(draft_args)
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        report = pilot.analyze_technical(manifest, evidence, None)
        self.assertEqual(report["status"], "pending")
        self.assertEqual(report["gates"]["a_positive"]["denominator"], 12)
        self.assertEqual(report["gates"]["b_cross_scope"]["denominator"], 8)
        self.assertEqual(report["gates"]["forget_and_restart"]["denominator"], 12)
        self.assertEqual(report["gates"]["positive_and_negative_stage_provenance"]["status"], "pending")

    def test_a_positive_count_requires_distinct_verified_alias_exposures(self):
        draft_args = type("Args", (), {
            "batch_id": "exposure-unit",
            "seed": "exposure-unit-seed",
            "operator": ["OP-A", "OP-B"],
            "non_implementer": "OP-B",
        })()
        manifest = pilot.make_technical_draft(draft_args)
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        by_id = {row["trial_id"]: row for row in evidence["trials"]}
        positive = [row for row in manifest["assignments"] if row["category"] == "a_positive"]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-pilot-artifacts-") as tmp:
            root = Path(tmp)
            for index, assignment in enumerate(positive):
                row = by_id[assignment["trial_id"]]
                exposed = index < 8
                row["observed_phase_id"] = manifest["phase_id"]
                row["observed_arm"] = "technical"
                row["observed_operator_id"] = assignment["operator_id"]
                row["state_integrity"] = "clean"
                row["outcomes"].update({
                    "actual_delivery_verified": "yes",
                    "independent_terminal_verified": "yes",
                    "alias_exposure": "yes" if exposed else "no",
                    "alias_eligible": "yes" if exposed else "no",
                    "alias_applied_correctly": "yes" if exposed else "no",
                    "new_unintended_mutation": "no",
                })
                if exposed:
                    self.add_installable_test_audit_receipt(root, manifest, assignment, row, index)
            report = pilot.analyze_technical(manifest, evidence, root)
            self.assertEqual(report["gates"]["a_positive"]["status"], "pass")
            self.assertEqual(report["gates"]["a_positive"]["eligible_aliases"], 8)
            self.assertEqual(report["status"], "pending")  # Fixture and missing evidence cannot pass the human gate.

            first = by_id[positive[0]["trial_id"]]["source_capture"]
            duplicate = by_id[positive[7]["trial_id"]]["source_capture"]
            by_id[positive[7]["trial_id"]]["source_capture"] = {
                **duplicate,
                "capture_id": first["capture_id"],
                "recording_generation": first["recording_generation"],
            }
            duplicate_report = pilot.analyze_technical(manifest, evidence, root)
            self.assertNotEqual(first["capture_id"], duplicate["capture_id"])
            self.assertIn(duplicate_report["gates"]["a_positive"]["status"], {"pending", "fail"})
            self.assertEqual(duplicate_report["gates"]["unique_real_recordings"]["status"], "fail")

    def test_a_positive_unreachable_minimum_fails_despite_an_unknown_row(self):
        manifest = AllocationTests().technical_draft()
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        rows = {row["trial_id"]: row for row in evidence["trials"]}
        positives = [row for row in manifest["assignments"] if row["category"] == "a_positive"]
        for assignment in manifest["assignments"]:
            row = rows[assignment["trial_id"]]
            row["observed_phase_id"] = manifest["phase_id"]
            row["observed_arm"] = assignment["arm"]
            row["observed_operator_id"] = assignment["operator_id"]
            row["state_integrity"] = "clean"
            row["outcomes"].update({key: "no" for key in row["outcomes"]})
        for index, assignment in enumerate(positives):
            row = rows[assignment["trial_id"]]
            if index < 3:
                row["outcomes"].update({
                    "alias_exposure": "yes", "alias_eligible": "yes",
                    "alias_applied_correctly": "yes",
                    "actual_delivery_verified": "yes", "independent_terminal_verified": "yes",
                })
            elif index < 11:
                row["outcomes"].update({"alias_exposure": "no", "alias_eligible": "no"})
            else:
                row["outcomes"].update({"alias_exposure": "unknown", "alias_eligible": "unknown"})
        report = pilot.analyze_technical(manifest, evidence, None)
        self.assertEqual(report["gates"]["a_positive"]["eligible_aliases"], 0)
        self.assertEqual(report["gates"]["a_positive"]["maximum_reachable_eligible_aliases"], 4)
        self.assertEqual(report["gates"]["a_positive"]["status"], "fail")

    def test_observer_metrics_use_owner_facts_count_false_positives_and_keep_known_misses(self):
        assignments = AllocationTests().technical_draft()["assignments"][:5]
        pair_refs = [f"70000000-0000-4000-8000-{index + 1:012d}" for index in range(5)]

        def item(actual, eligible, claimed_capture, correct, capture_status, candidate_refs,
                 *, binding_ok=True, independent_label_ok=True):
            return {
                "outcomes": {},
                "independent_label": {
                    "actual_correction": actual,
                    "eligible_correction": eligible,
                    "observer_captured": claimed_capture,
                    "observer_correct": correct,
                },
                "observer_fact": {
                    "status": capture_status,
                    "candidate_refs": candidate_refs,
                    "reason": "fixture",
                    "source": "bounded_audit",
                    "human_acceptance_authority": True,
                },
                "observer_label_core_ok": True,
                "observer_label_binding_ok": binding_ok,
                "independent_label_ok": independent_label_ok,
                "distinct_recording": True,
            }

        stage_data = {
            assignments[0]["trial_id"]: item("yes", "yes", "yes", "yes", True, [pair_refs[0]]),
            # The independently labelled claim says the no-grant task was captured,
            # while the native receipt says no_grant and therefore a known miss.
            assignments[1]["trial_id"]: item(
                "yes", "yes", "yes", "yes", False, [], binding_ok=False, independent_label_ok=False,
            ),
            assignments[2]["trial_id"]: item("yes", "yes", "no", "no", False, []),
            assignments[3]["trial_id"]: item("no", "no", "yes", "no", True, [pair_refs[3]]),
            assignments[4]["trial_id"]: item("yes", "yes", "unknown", "unknown", None, [], binding_ok=False),
        }

        complete = {key: value for key, value in stage_data.items()
                    if key != assignments[4]["trial_id"]}
        issues = []
        summary = pilot.technical_observer_summary(complete, issues)
        self.assertEqual(summary["eligible_corrections"], 3)
        self.assertEqual(summary["eligible_captured_corrections"], 1)
        self.assertEqual(summary["eligible_capture_recall"], 1 / 3)
        self.assertEqual(summary["captured_corrections"], 2)
        self.assertEqual(summary["false_positive_captures"], 1)
        self.assertEqual(summary["correct_captures"], 1)
        self.assertEqual(summary["capture_precision"], 0.5)
        self.assertIn(assignments[1]["trial_id"], summary["violations"])
        self.assertIn("observer_capture_fact_conflict", [issue["field"] for issue in issues])
        self.assertIn("observer_correction_nesting_violation", [issue["field"] for issue in issues])

        issues = []
        with_unknown = pilot.technical_observer_summary(stage_data, issues)
        self.assertEqual(with_unknown["capture_unknown"], 1)
        self.assertEqual(with_unknown["eligible_capture_unknown"], 1)
        self.assertIsNone(with_unknown["eligible_capture_recall"])
        self.assertIsNone(with_unknown["capture_precision"])
        self.assertIn(assignments[4]["trial_id"], with_unknown["unknown_trials"])

        positive = next(iter(stage_data.values()))
        positive["independent_label"]["observer_correct"] = "yes"
        issues = []
        contradictory = pilot.technical_observer_summary({assignments[0]["trial_id"]: positive}, issues)
        self.assertEqual(contradictory["correct_captures"], 1)

    def test_phase_and_arm_mismatch_are_reported_as_contamination(self):
        draft_args = type("Args", (), {
            "batch_id": "phase-unit",
            "seed": "phase-unit-seed",
            "operator": ["OP-A", "OP-B"],
            "non_implementer": "OP-B",
        })()
        manifest = pilot.make_technical_draft(draft_args)
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        assignment = manifest["assignments"][0]
        row = next(item for item in evidence["trials"] if item["trial_id"] == assignment["trial_id"])
        row["observed_phase_id"] = "wrong_phase"
        row["observed_arm"] = "wrong_arm"
        row["observed_operator_id"] = assignment["operator_id"]
        report = pilot.analyze_technical(manifest, evidence, None)
        self.assertEqual(report["gates"]["assignment_phase_arm_and_state_integrity"]["status"], "fail")

    def test_b_wrong_scope_harm_fails_even_without_reported_exposure(self):
        manifest = AllocationTests().technical_draft()
        evidence = pilot.make_evidence_template(manifest)
        evidence["evidence_class"] = "unit_fixture"
        assignment = next(row for row in manifest["assignments"] if row["category"] == "b_cross_scope")
        row = next(item for item in evidence["trials"] if item["trial_id"] == assignment["trial_id"])
        row["outcomes"]["alias_exposure"] = "no"
        row["outcomes"]["wrong_scope_alias_applied"] = "yes"
        report = pilot.analyze_technical(manifest, evidence, None)
        self.assertEqual(report["gates"]["b_cross_scope"]["status"], "fail")
        self.assertIn(assignment["trial_id"], report["gates"]["b_cross_scope"]["wrong_scope_applications"])

    def test_magic_technical_report_cannot_open_benefit_allocation(self):
        manifest = AllocationTests().technical_draft()
        evidence = pilot.make_evidence_template(manifest)
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-upstream-unit-") as tmp:
            root = Path(tmp)
            manifest_path, evidence_path = root / "manifest.json", root / "evidence.json"
            report_path = root / "report.json"
            self.write_json(manifest_path, manifest)
            self.write_json(evidence_path, evidence)
            self.write_json(report_path, {
                "artifact_type": "voiceflow_dictionary_learning_pilot_report", "stage": "technical",
                "status": "pass", "evidence_class": "actual_human", "human_gate_status": "pass",
            })
            artifact_root = root / "artifacts"
            artifact_root.mkdir()
            with self.assertRaises(pilot.PilotError):
                pilot.validate_upstream_technical(report_path, manifest_path, evidence_path, artifact_root)

    def test_state_receipt_change_fails_declared_unchanged_contract(self):
        manifest = AllocationTests().technical_draft()
        assignment = next(row for row in manifest["assignments"] if row["category"] == "a_positive")
        row = pilot.make_evidence_template(manifest)["trials"][int(assignment["trial_id"][1:]) - 1]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-state-unit-") as tmp:
            root = Path(tmp)
            verified = {}
            for boundary, role, dictionary_hash in (("before", "state_before", "a" * 64),
                                                    ("after", "state_after", "b" * 64)):
                receipt = {
                    "artifact_type": "voiceflow_dictionary_learning_state_snapshot", "schema_version": 1,
                    "batch_id": manifest["batch_id"], "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
                    "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
                    "actor_id": assignment["operator_id"], "arm": assignment["arm"],
                    "state_contract": assignment["state_contract"], "boundary": boundary,
                    "global_dictionary_sha256": dictionary_hash,
                    "rule_inventory_sha256": "c" * 64, "tombstone_inventory_sha256": "d" * 64,
                    "active_rule_families": [], "effective_hint_sha256": "e" * 64,
                    "passive_learning_enabled": False,
                    "target_rule_state": "not_applicable",
                }
                seal_state_snapshot(receipt)
                path = root / f"{boundary}.json"
                self.write_json(path, receipt)
                row["provenance"][role] = {
                    "path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                }
                verified[role] = True
            missing, issues = [], []
            complete, transition_ok, _snapshots = pilot.validate_state_receipts(
                row, assignment, manifest, root, verified, missing, issues,
            )
        self.assertTrue(complete)
        self.assertFalse(transition_ok)
        self.assertTrue(any(issue["field"] == "declared_state_transition_violated" for issue in issues))

    def test_pending_candidate_is_allowed_but_active_promotion_is_rejected(self):
        manifest = AllocationTests().technical_draft()
        assignment = next(row for row in manifest["assignments"] if row["category"] == "a_positive")
        row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                   if item["trial_id"] == assignment["trial_id"])

        def validate_transition(root: Path, *, promote: bool = False) -> tuple[bool, list[dict]]:
            self.add_installable_test_audit_receipt(root, manifest, assignment, row, 71)
            verified = {
                role: pilot.verify_artifact_ref(root, row["provenance"][role], assignment["trial_id"], role, [])
                for role in ("owner_receipt", "source_capture")
            }
            missing, issues = [], []
            _identity, _hashes, _owner_digest, observer_fact = pilot.verify_source_capture(
                row, assignment, manifest, verified, root, missing, issues,
            )
            self.assertIsNotNone(observer_fact)
            native = observer_fact["audit_pair_inventories"]
            actor_id = assignment["operator_id"]
            effective_hint = hashlib.sha256(b"pending-state-unit").hexdigest()
            for boundary, role in (("before", "state_before"), ("after", "state_after")):
                inventory = native[boundary]
                active_inventory = inventory["active_rule_inventory_sha256"]
                if promote and boundary == "after":
                    active_inventory = "9" * 64
                receipt = {
                    "artifact_type": "voiceflow_dictionary_learning_state_snapshot", "schema_version": 1,
                    "batch_id": manifest["batch_id"],
                    "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
                    "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
                    "actor_id": actor_id, "arm": assignment["arm"],
                    "state_contract": assignment["state_contract"], "boundary": boundary,
                    "global_dictionary_sha256": "a" * 64,
                    "rule_inventory_sha256": inventory["rule_inventory_sha256"],
                    "active_rule_inventory_sha256": active_inventory,
                    "inactive_candidate_inventory_sha256": inventory["inactive_candidate_inventory_sha256"],
                    "tombstone_inventory_sha256": inventory["tombstone_inventory_sha256"],
                    "active_rule_families": [], "effective_hint_sha256": effective_hint,
                    "passive_learning_enabled": True, "target_rule_state": "not_applicable",
                }
                seal_state_snapshot(receipt)
                path = root / f"{boundary}.json"
                self.write_json(path, receipt)
                row["provenance"][role] = {
                    "path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                }
                verified[role] = True
            complete, transition_ok, _snapshots = pilot.validate_state_receipts(
                row, assignment, manifest, root, verified, missing, issues, observer_fact,
            )
            self.assertTrue(complete)
            return transition_ok, issues

        with tempfile.TemporaryDirectory(prefix="dictionary-learning-pending-state-") as tmp:
            pending_ok, pending_issues = validate_transition(Path(tmp))
        self.assertTrue(pending_ok, pending_issues)

        with tempfile.TemporaryDirectory(prefix="dictionary-learning-promoted-state-") as tmp:
            promoted_ok, promoted_issues = validate_transition(Path(tmp), promote=True)
        self.assertFalse(promoted_ok)
        self.assertTrue(any(issue["field"] == "declared_state_transition_violated"
                            for issue in promoted_issues))

    def test_benefit_control_family_is_not_required_to_be_active(self):
        manifest = AllocationTests().benefit_draft()
        assignment = next(row for row in manifest["assignments"]
                          if row["category"] == "target" and row["arm"] == "control")
        row = next(item for item in pilot.make_evidence_template(manifest)["trials"]
                   if item["trial_id"] == assignment["trial_id"])
        active = sorted({row["family_id"] for row in manifest["assignments"]
                        if row["participant_id"] == assignment["participant_id"]
                        and row["category"] == "target" and row["arm"] == "teach"})
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-control-state-") as tmp:
            root = Path(tmp)
            verified = {}
            for boundary, role in (("before", "state_before"), ("after", "state_after")):
                receipt = {
                    "artifact_type": "voiceflow_dictionary_learning_state_snapshot", "schema_version": 1,
                    "batch_id": manifest["batch_id"], "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
                    "phase_id": manifest["phase_id"], "trial_id": assignment["trial_id"],
                    "actor_id": assignment["participant_id"], "arm": assignment["arm"],
                    "state_contract": assignment["state_contract"], "boundary": boundary,
                    "global_dictionary_sha256": "a" * 64, "rule_inventory_sha256": "b" * 64,
                    "tombstone_inventory_sha256": "c" * 64, "active_rule_families": active,
                    "effective_hint_sha256": "d" * 64,
                    "passive_learning_enabled": False, "target_rule_state": "not_applicable",
                }
                seal_state_snapshot(receipt)
                path = root / f"{boundary}.json"
                self.write_json(path, receipt)
                row["provenance"][role] = {"path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                verified[role] = True
            missing, issues = [], []
            complete, transition_ok, _snapshots = pilot.validate_state_receipts(row, assignment, manifest, root,
                                                                                 verified, missing, issues)
        self.assertTrue(complete)
        self.assertTrue(transition_ok)
        self.assertEqual(issues, [])

    def test_hashed_auxiliary_receipt_with_wrong_phase_is_rejected(self):
        manifest = AllocationTests().technical_draft()
        assignment = manifest["auxiliary_observations"][0]
        row = pilot.make_evidence_template(manifest)["auxiliary_observations"][0]
        with tempfile.TemporaryDirectory(prefix="dictionary-learning-aux-unit-") as tmp:
            root = Path(tmp)
            owner = {
                "artifact_type": "voiceflow_dictionary_learning_auxiliary_owner_receipt",
                "schema_version": 1, "batch_id": manifest["batch_id"],
                "manifest_fingerprint": pilot.manifest_fingerprint(manifest),
                "phase_id": "wrong-phase", "check_id": assignment["check_id"],
                "operator_id": assignment["operator_id"], "check_type": assignment["check_type"],
                "observed_phase_id": manifest["phase_id"], "local_observation_verified": True,
                "outcomes": {"observer_active": "yes", "mid_ime_state_write": "no", "wrong_field_write": "no"},
            }
            path = root / "owner.json"
            self.write_json(path, owner)
            row["provenance"]["owner_receipt"] = {
                "path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
            verified = {"owner_receipt": pilot.verify_artifact_ref(root, row["provenance"]["owner_receipt"],
                                                                     assignment["check_id"], "owner_receipt", [])}
            missing, issues = [], []
            complete, valid = pilot.validate_auxiliary_receipts(
                row, assignment, manifest, root, verified, missing, issues,
            )
        self.assertFalse(complete)
        self.assertFalse(valid)
        self.assertTrue(any(issue["field"] == "owner_receipt.phase_id_mismatch" for issue in issues))


if __name__ == "__main__":
    unittest.main()
