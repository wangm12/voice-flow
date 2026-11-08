from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[4]
LICENSE_INDEX_PATH = REPO_ROOT / "ThirdPartyLicenses/DependencyTexts/license-index.json"


def repository_license_index():
    return json.loads(LICENSE_INDEX_PATH.read_text(encoding="utf-8"))


def repository_notice_text(record: dict) -> bytes:
    return (REPO_ROOT / record["output_path"]).read_bytes()


SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "generate_notices.py"
SPEC = importlib.util.spec_from_file_location("generate_notices", SCRIPT)
assert SPEC and SPEC.loader
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class NoticeGeneratorTests(unittest.TestCase):
    def test_case_insensitive_and_underscore_license_names(self) -> None:
        self.assertTrue(module.is_license_filename("license.txt"))
        self.assertTrue(module.is_license_filename("LICENSE_APACHE-2.0"))
        self.assertTrue(module.is_license_filename("Licence-MIT.md"))
        self.assertTrue(module.is_license_filename("COPYRIGHT"))
        self.assertFalse(module.is_license_filename("copying.rs"))
        self.assertFalse(module.is_license_filename("copyright.mjs"))
        self.assertFalse(module.is_license_filename("copyright.mjs.map"))
        self.assertFalse(module.is_license_filename("licensee-notes.md"))

    def test_notice_name_is_collected(self) -> None:
        self.assertTrue(module.is_license_filename("NOTICE"))
        self.assertTrue(module.is_license_filename("notices.md"))
        self.assertTrue(module.is_license_path(Path("LICENSES/MIT")))

    def test_package_root_symlink_escape_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            package = base / "package"
            elsewhere = base / "elsewhere"
            package.mkdir()
            elsewhere.mkdir()
            external_file = elsewhere / "LICENSE"
            external_file.write_text("external", encoding="utf-8")
            link = package / "LICENSE"
            link.symlink_to(external_file)
            with self.assertRaises(ValueError):
                module.contained_path(package, link)

    def test_locked_identity_mismatch_fails_closed(self) -> None:
        module.require_exact_identity(("crate", "1.2.3", "registry"), ("crate", "1.2.3", "registry"), "fixture")
        with self.assertRaises(ValueError):
            module.require_exact_identity(("crate", "1.2.3", "registry"), ("crate", "1.2.4", "registry"), "fixture")

    def test_missing_text_remains_unresolved(self) -> None:
        self.assertEqual(module.text_status([]), "unresolved_no_full_text_candidate")
        self.assertEqual(module.text_status([{"kind": "license_text"}]), "license_text_found")

    def test_spdx_tag_value_is_not_counted_as_license_text(self) -> None:
        kind = module.classify_text(Path("LICENSE.spdx"), b"SPDXVersion: SPDX-2.1\nPackageLicenseDeclared: MIT\n")
        record = {"texts": [{"kind": kind}]}
        self.assertEqual(kind, "spdx_metadata")
        self.assertFalse(module.has_license_text(record))
        self.assertEqual(module.text_status(record["texts"]), "metadata_only_no_license_text")

    def test_license_directory_body_is_classified_as_license_text(self) -> None:
        body = (b"MIT License\n\nPermission is hereby granted, free of charge to any person obtaining a copy "
                b"of this software and associated documentation files (the Software), to deal in the Software "
                b"without restriction, including without limitation the rights to use, copy, modify, merge, "
                b"publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons "
                b"to whom the Software is furnished to do so, subject to the following conditions:\n\n"
                b"The above copyright notice and this permission notice shall be included in all copies or "
                b"substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY "
                b"OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, "
                b"FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT "
                b"HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, "
                b"TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER "
                b"DEALINGS IN THE SOFTWARE.\n")
        self.assertEqual(module.classify_text(Path("LICENSES/MIT"), body), "license_text")

    def test_objc2_guide_and_dasp_apache_header_are_not_full_terms(self) -> None:
        index = repository_license_index()
        objc = next(p for p in index["packages"] if p["name"] == "objc2")
        objc_text = next(t for t in objc["texts"] if t["source_basename"] == "LICENSE.md")
        guide = repository_notice_text(objc_text)
        self.assertEqual(module.classify_text(Path("LICENSE.md"), guide), "license_reference_metadata")
        dasp = next(p for p in index["packages"] if p["name"] == "dasp_sample")
        header = next(t for t in dasp["texts"] if t["source_basename"] == "LICENSE-APACHE")
        self.assertEqual(header["bytes"], 561)
        self.assertEqual(module.classify_text(Path("LICENSE-APACHE"), repository_notice_text(header)), "license_reference_metadata")

    def test_complete_apache_boilerplate_remains_full_license_text(self) -> None:
        index = repository_license_index()
        cpal = next(p for p in index["packages"] if p["name"] == "cpal")
        text = next(t for t in cpal["texts"] if t["source_basename"] == "LICENSE")
        body = repository_notice_text(text)
        self.assertIn("Apache-2.0", module.license_body_families(body))
        self.assertEqual(module.classify_text(Path("LICENSE"), body), "license_text")

    def test_atomic_waker_mixed_file_retains_complete_mit_body(self) -> None:
        index = repository_license_index()
        atomic = next(p for p in index["packages"] if p["name"] == "atomic-waker")
        text = next(t for t in atomic["texts"] if t["source_basename"] == "LICENSE-THIRD-PARTY")
        body = repository_notice_text(text)
        kind = module.classify_text(Path("LICENSE-THIRD-PARTY"), body)
        self.assertEqual(kind, "mixed_license_text_and_reference")
        self.assertTrue(module.has_license_text({"texts": [{"kind": kind}]}))
        self.assertEqual(module.license_body_families(body), ["MIT"])
        self.assertEqual(module.license_reference_families(body), ["Apache-2.0"])

    def test_short_license_pointers_and_apache_headers_are_metadata(self) -> None:
        ring_pointer = b"*ring* uses an ISC license. See LICENSE-other-bits for its text.\n" + b"See LICENSE-BoringSSL for the BoringSSL license.\n" * 4
        apache_header = (b"Copyright 2015 Author.\nLicensed under the Apache License, Version 2.0.\n"
                         b"You may obtain a copy of the License at\nhttps://www.apache.org/licenses/LICENSE-2.0\n"
                         b"Unless required by applicable law or agreed to in writing, software is distributed on an AS IS basis.\n")
        self.assertEqual(module.classify_text(Path("LICENSE"), ring_pointer), "license_reference_metadata")
        self.assertEqual(module.classify_text(Path("LICENSE-APACHE"), apache_header), "license_reference_metadata")

    def test_license_headers_are_not_accepted_as_full_text_when_section_body_is_absent(self) -> None:
        body = (b"Copyright 2015 Author.\nThe Apache License, Version 2.0\n"
                b"You may obtain a copy of the License at https://www.apache.org/licenses/LICENSE-2.0\n"
                b"Unless required by applicable law or agreed to in writing, software distributed under the License is distributed on an AS IS BASIS.\n")
        self.assertEqual(module.classify_text(Path("LICENSE-APACHE"), body), "license_reference_metadata")

    def test_repository_reference_does_not_invent_a_git_tag(self) -> None:
        self.assertEqual(
            module.public_repository_reference("https://github.com/example/project.git"),
            "https://github.com/example/project",
        )

    def test_generated_program_files_named_copyright_are_not_notice_text(self) -> None:
        self.assertEqual(module.classify_text(Path("copyright.mjs"), b"// generated icon module"), "other_metadata")
        self.assertEqual(module.classify_text(Path("copyright.mjs.map"), b"{\"version\":3}"), "other_metadata")
        self.assertFalse(module.is_license_path(Path("LICENSES/copyright.mjs")))
        self.assertFalse(module.is_license_path(Path("LICENSES/copyright.mjs.map")))

    def test_create_new_output_paths_must_be_external_and_disjoint(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base=Path(temporary).resolve(); repo=base/"repo"; repo.mkdir()
            meta=base/"metadata.json"; meta.write_text("{}")
            obs=base/"observations.json"; obs.write_text("{}")
            out_parent=base/"out"; out_parent.mkdir()
            evidence_parent=base/"evidence"; evidence_parent.mkdir()
            output=out_parent/"candidate"; evidence=evidence_parent/"run"
            resolved=module.validate_new_external_outputs(repo,meta,obs,output,evidence)
            self.assertEqual(resolved,(output,evidence))
            with self.assertRaises(ValueError):
                module.validate_new_external_outputs(repo,meta,obs,repo/"candidate",evidence_parent/"run2")
            with self.assertRaises(ValueError):
                module.validate_new_external_outputs(repo,meta,obs,output,output)
            with self.assertRaises(ValueError):
                module.validate_new_external_outputs(repo,meta,obs,meta,evidence_parent/"run3")

    def test_create_new_output_rejects_symlink_and_write_refuses_symlink_target(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base=Path(temporary).resolve(); repo=base/"repo"; repo.mkdir(); meta=base/"meta"; meta.write_text("x"); obs=base/"obs"; obs.write_text("x")
            outside=base/"outside"; outside.mkdir(); parent=base/"parent"; parent.mkdir(); link=parent/"candidate"; link.symlink_to(outside, target_is_directory=True)
            evidence_parent=base/"evidence"; evidence_parent.mkdir()
            with self.assertRaises(ValueError):
                module.validate_new_external_outputs(repo,meta,obs,link,evidence_parent/"run")
            target=parent/"file"; target.symlink_to(outside/"escaped")
            with self.assertRaises(ValueError):
                module.write_bytes(target,b"do not follow",root=parent)
            self.assertFalse((outside/"escaped").exists())
            child_link=parent/"nested"; child_link.symlink_to(outside,target_is_directory=True)
            with self.assertRaises(ValueError):
                module.write_bytes(child_link/"escaped",b"do not follow",root=parent)
            self.assertFalse((outside/"escaped").exists())

    def test_cargo_manifest_source_root_is_protected_before_any_output_is_created(self) -> None:
        import json
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            repo = base / "repo"
            repo.mkdir()
            crate = base / "cargo" / "registry" / "src" / "crate-1.0.0"
            crate.mkdir(parents=True)
            manifest = crate / "Cargo.toml"
            manifest.write_text("[package]\nname='crate'\nversion='1.0.0'\n", encoding="utf-8")
            metadata = base / "metadata.json"
            metadata.write_text(json.dumps({"packages": [{"manifest_path": str(manifest)}]}), encoding="utf-8")
            observations = base / "observations.json"
            observations.write_text("{}", encoding="utf-8")
            output = crate / "generated-output"
            evidence_parent = base / "evidence"
            evidence_parent.mkdir()
            evidence = evidence_parent / "run"
            with self.assertRaises(ValueError):
                module.generate(repo, metadata, observations, output, evidence)
            self.assertFalse(output.exists())
            self.assertFalse(evidence.exists())

    def test_declared_external_license_file_is_protected(self) -> None:
        import json
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve()
            crate = base / "crate"
            crate.mkdir()
            manifest = crate / "Cargo.toml"
            manifest.write_text("manifest", encoding="utf-8")
            license_root = base / "external-license"
            license_root.mkdir()
            license_file = license_root / "LICENSE"
            license_file.write_text("license", encoding="utf-8")
            metadata = {"packages": [{"manifest_path": str(manifest), "license_file": str(license_file)}]}
            protected = module.cargo_metadata_source_paths(metadata)
            repo = base / "repo"
            repo.mkdir()
            meta_path = base / "cargo.json"
            meta_path.write_text("{}", encoding="utf-8")
            obs = base / "observations.json"
            obs.write_text("{}", encoding="utf-8")
            evidence_parent = base / "evidence"
            evidence_parent.mkdir()
            with self.assertRaises(ValueError):
                module.validate_new_external_outputs(repo, meta_path, obs, license_root / "generated", evidence_parent / "run", protected_source_paths=protected)
            self.assertFalse((license_root / "generated").exists())

    def test_writer_refuses_destinations_outside_the_declared_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            base=Path(temporary).resolve()
            output=base/"output"; output.mkdir()
            outside=base/"outside.txt"
            with self.assertRaises(ValueError):
                module.write_bytes(outside,b"must not be written",root=output)
            self.assertFalse(outside.exists())


if __name__ == "__main__":
    unittest.main()
