from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


REPO_ROOT = Path(__file__).resolve().parents[4]
LICENSE_INDEX_PATH = REPO_ROOT / "ThirdPartyLicenses/DependencyTexts/license-index.json"


def repository_license_index():
    return json.loads(LICENSE_INDEX_PATH.read_text(encoding="utf-8"))


def repository_notice_text(record: dict) -> bytes:
    return (REPO_ROOT / record["output_path"]).read_bytes()


SCRIPT = Path(__file__).resolve().parents[1] / "tools" / "fetch_verified_sources.py"
SPEC = importlib.util.spec_from_file_location("fetch_verified_sources", SCRIPT)
assert SPEC and SPEC.loader
fetch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fetch)

MIT_BODY = b'''MIT License\n\nCopyright (c) Fixture Authors\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the Software), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n'''


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class FetchCliEndToEndTests(unittest.TestCase):
    def test_frozen_reclassification_and_canonical_objc2_terms_preserve_source_boundaries(self):
        package_revision = "a" * 40
        other_revision = "b" * 40
        guide_body = (
            b"Licensing of these crates is a bit complicated. The crates `objc2`, `block2`, "
            b"`objc2-foundation` and `objc2-encode` are currently available under [the MIT License][MIT]. "
            b"All other crates are Trio-licensed; see [Zlib], [Apache-2.0], and [MIT]. "
            b"Apple SDKs and derived-work terms are a separate discussion.\n"
        )
        guide_hash = digest(guide_body)
        canonical_hash = digest(MIT_BODY)
        guide_output = f"ThirdPartyLicenses/DependencyTexts/by-sha256/{guide_hash}.txt"
        canonical_output = f"ThirdPartyLicenses/DependencyTexts/by-sha256/{canonical_hash}.txt"
        guide_url = f"https://raw.githubusercontent.com/madsmtm/objc2/{package_revision}/LICENSE.md"
        canonical_source_url = "https://spdx.org/licenses/MIT.txt"

        with tempfile.TemporaryDirectory() as temporary:
            overlay = Path(temporary) / "overlay"
            guide_path = overlay / guide_output
            canonical_path = overlay / canonical_output
            guide_path.parent.mkdir(parents=True)
            guide_path.write_bytes(guide_body)
            canonical_path.write_bytes(MIT_BODY)

            guide_item = {
                "source_path": "LICENSE.md", "output_path": guide_output,
                "sha256": guide_hash, "bytes": len(guide_body),
                "kind": "license_reference_metadata", "source_url": guide_url,
                "source_revision": package_revision,
            }
            canonical_item = {
                "source_path": "canonical-license-texts/MIT.txt", "output_path": canonical_output,
                "sha256": canonical_hash, "bytes": len(MIT_BODY),
                "kind": "license_text", "source_url": canonical_source_url,
                "source_revision": None, "package_source_revision": package_revision,
                "license_expression_id": "MIT", "guide_source_url": guide_url,
                "guide_sha256": guide_hash,
                "source_relationship": (
                    "Canonical MIT terms, not bytes from this package archive. The pinned upstream LICENSE.md "
                    "names MIT as an applicable option for this exact package source; SDK-derived-work terms "
                    "remain outside this text attribution record."
                ),
            }
            packages = []
            bindings = {
                "canonical-source": {"files": [canonical_item]},
            }
            for name in sorted(fetch.OBJC2_MIT_ONLY | fetch.OBJC2_TRIO_LICENSED):
                version = "fixture-1.0.0"
                expression = "MIT" if name in fetch.OBJC2_MIT_ONLY else "Zlib OR Apache-2.0 OR MIT"
                row = {
                    "ecosystem": "rust", "name": name, "version": version,
                    "source": "registry:crates.io", "source_revision": None,
                    "license_expression": expression, "texts": [],
                }
                if name == "objc2":
                    row["texts"].append({
                        "source_path": "objc2/LICENSE.md", "source_basename": "LICENSE.md",
                        "output_path": guide_output, "sha256": guide_hash, "bytes": len(guide_body),
                        "kind": "license_text", "source_url": guide_url,
                    })
                packages.append(row)
                bindings[f"{name}@{version}"] = {
                    "status": "exact_revision_verified",
                    "repository_url": "https://github.com/madsmtm/objc2",
                    "source_revision": package_revision,
                    "files": [dict(guide_item)],
                }

            packages.append({
                "ecosystem": "rust", "name": "dasp_sample", "version": "fixture-1.0.0",
                "source": "registry:crates.io", "license_expression": "MIT",
                "texts": [{
                    "source_path": "dasp_sample/LICENSE-MIT", "source_basename": "LICENSE-MIT",
                    "output_path": canonical_output, "sha256": canonical_hash, "bytes": len(MIT_BODY),
                    "kind": "license_reference_metadata",
                }],
            })
            packages.append({
                "ecosystem": "rust", "name": "voiceflow-fixture", "version": "0.0.0",
                "source": "workspace:src-tauri/voiceflow-fixture", "workspace_package": True,
                "first_party": True, "license_expression": None, "texts": [],
            })
            index = {"packages": packages, "summary": {}}

            index = fetch._CLASSIFIER.reclassify_saved_text_records(index, overlay)
            self.assertEqual(index["summary"]["records_with_full_license_text"], 1)
            self.assertEqual(index["summary"]["third_party_records_without_full_license_text"], 15)
            self.assertEqual(index["summary"]["first_party_undeclared_license_records"], 1)
            objc = next(row for row in index["packages"] if row["name"] == "objc2")
            reclassified_guide = next(text for text in objc["texts"] if text["source_basename"] == "LICENSE.md")
            self.assertEqual(reclassified_guide["kind"], "license_reference_metadata")

            attached = fetch.attach_objc2_canonical_mit_texts(index, bindings, overlay)
            self.assertEqual(attached, 15)
            index = fetch.merge_pinned_text_bindings(index, bindings, overlay)
            self.assertEqual(index["summary"]["records_with_full_license_text"], 16)
            self.assertEqual(index["summary"]["third_party_records_without_full_license_text"], 0)
            self.assertEqual(index["summary"]["first_party_undeclared_license_records"], 1)
            for name in fetch.OBJC2_MIT_ONLY | fetch.OBJC2_TRIO_LICENSED:
                row = next(item for item in index["packages"] if item["name"] == name)
                self.assertEqual(row["canonical_license_choice"]["license_id"], "MIT")
                self.assertEqual(row["canonical_license_choice"]["canonical_source_url"], canonical_source_url)
                canonical = next(text for text in row["texts"] if text.get("source_relationship", "").startswith("Canonical MIT terms"))
                self.assertIsNone(canonical["source_revision"])
                self.assertEqual(canonical["package_source_revision"], row["source_revision"])
                self.assertEqual(row["source_revision"], package_revision)
                self.assertEqual(canonical["source_url"], canonical_source_url)
                self.assertEqual(canonical["sha256"], canonical_hash)

            unknown_row = {
                "ecosystem": "rust", "name": "objc2", "version": "fixture-unknown-revision",
                "license_expression": "MIT", "texts": [],
            }
            unknown_binding = {
                "status": "exact_revision_verified", "repository_url": "https://github.com/madsmtm/objc2",
                "source_revision": "unknown", "files": [dict(guide_item)],
            }
            unknown_index = {"packages": [unknown_row]}
            self.assertEqual(fetch.attach_objc2_canonical_mit_texts(
                unknown_index, {**bindings, "objc2@fixture-unknown-revision": unknown_binding}, overlay,
            ), 0)
            self.assertEqual(unknown_binding["files"], [guide_item])
            self.assertEqual(unknown_row["texts"], [])

            mismatched_row = {
                "ecosystem": "rust", "name": "block2", "version": "fixture-mismatched-guide-revision",
                "license_expression": "MIT", "texts": [],
            }
            mismatched_guide = dict(guide_item)
            mismatched_guide.update({
                "source_revision": other_revision,
                "source_url": f"https://raw.githubusercontent.com/madsmtm/objc2/{other_revision}/LICENSE.md",
            })
            mismatched_binding = {
                "status": "exact_revision_verified", "repository_url": "https://github.com/madsmtm/objc2",
                "source_revision": package_revision, "files": [mismatched_guide],
            }
            self.assertEqual(fetch.attach_objc2_canonical_mit_texts(
                {"packages": [mismatched_row]},
                {"canonical-source": bindings["canonical-source"], "block2@fixture-mismatched-guide-revision": mismatched_binding},
                overlay,
            ), 0)
            self.assertEqual(mismatched_binding["files"], [mismatched_guide])
            self.assertEqual(mismatched_row["texts"], [])

    def test_fetch_classification_uses_same_body_and_reference_rules(self):
        index = repository_license_index()
        objc = next(p for p in index["packages"] if p["name"] == "objc2")
        guide = next(t for t in objc["texts"] if t["source_basename"] == "LICENSE.md")
        self.assertEqual(fetch.text_kind("LICENSE.md", repository_notice_text(guide)), "license_reference_metadata")
        atomic = next(p for p in index["packages"] if p["name"] == "atomic-waker")
        mixed = next(t for t in atomic["texts"] if t["source_basename"] == "LICENSE-THIRD-PARTY")
        self.assertEqual(fetch.text_kind("LICENSE-THIRD-PARTY", repository_notice_text(mixed)), "mixed_license_text_and_reference")

    def make_fixture(self, base: Path, registry_integrity: str = "sha512-lock"):
        repo = base / "repo"
        package_dir = repo / "node_modules/react-remove-scroll-bar"
        package_dir.mkdir(parents=True)
        (repo / "src-tauri").mkdir()
        cargo_lock = repo / "src-tauri/Cargo.lock"
        cargo_lock.write_text("version = 4\n", encoding="utf-8")
        package_lock = {
            "name": "fixture",
            "lockfileVersion": 3,
            "packages": {
                "": {"name": "fixture"},
                "node_modules/react-remove-scroll-bar": {
                    "version": "2.3.8",
                    "integrity": "sha512-lock",
                    "resolved": "https://registry.npmjs.org/react-remove-scroll-bar/-/react-remove-scroll-bar-2.3.8.tgz",
                },
            },
        }
        lock_bytes = (json.dumps(package_lock, sort_keys=True) + "\n").encode()
        (repo / "package-lock.json").write_bytes(lock_bytes)
        (package_dir / "package.json").write_text(
            json.dumps({"name": "react-remove-scroll-bar", "version": "2.3.8"}),
            encoding="utf-8",
        )
        inventory = {
            "inputs": {
                "cargo_lock": {"sha256": digest(cargo_lock.read_bytes())},
                "package_lock": {"sha256": digest(lock_bytes)},
            },
            "packages": [
                {
                    "ecosystem": "npm",
                    "name": "react-remove-scroll-bar",
                    "version": "2.3.8",
                    "text_status": "unresolved_no_full_text_candidate",
                    "upstream_repository": "https://github.com/theKashey/react-remove-scroll-bar",
                    "lock_identity": {
                        "lock_path": "node_modules/react-remove-scroll-bar",
                        "version": "2.3.8",
                        "integrity": "sha512-lock",
                    },
                    "texts": [],
                }
            ],
            "summary": {},
        }
        index_path = base / "inventory.json"
        index_path.write_text(json.dumps(inventory), encoding="utf-8")
        cargo_meta = base / "cargo-metadata.json"
        cargo_meta.write_text(json.dumps({"packages": []}), encoding="utf-8")
        overlay = base / "overlay"
        evidence = base / "evidence"
        metadata = {
            "name": "react-remove-scroll-bar",
            "version": "2.3.8",
            "dist": {"integrity": registry_integrity},
            "repository": {"url": "git+https://github.com/theKashey/react-remove-scroll-bar.git"},
            "gitHead": "a" * 40,
        }
        return repo, index_path, cargo_meta, overlay, evidence, metadata

    def run_mocked(self, base: Path, *, registry_integrity: str = "sha512-lock"):
        repo, index_path, cargo_meta, overlay, evidence, metadata = self.make_fixture(base, registry_integrity)
        observed_urls: list[str] = []

        def mocked_http_get(url: str, budget: dict, *, purpose: str):
            observed_urls.append(url)
            if url.startswith("https://registry.npmjs.org/"):
                body = json.dumps(metadata).encode()
                response = {
                    "url": url,
                    "final_url": url,
                    "status": 200,
                    "content_type": "application/json",
                    "bytes": len(body),
                    "sha256": digest(body),
                    "purpose": purpose,
                }
            else:
                body = MIT_BODY
                response = {
                    "url": url,
                    "final_url": url,
                    "status": 200,
                    "content_type": "text/plain; charset=utf-8",
                    "bytes": len(body),
                    "sha256": digest(body),
                    "purpose": purpose,
                }
            budget["bytes"] += len(body)
            return body, response

        with patch.object(fetch, "http_get", side_effect=mocked_http_get):
            result = fetch.main([
                "--repo", str(repo),
                "--inventory-index", str(index_path),
                "--cargo-metadata", str(cargo_meta),
                "--overlay-root", str(overlay),
                "--evidence-root", str(evidence),
            ])
        self.assertEqual(result, 0)
        return observed_urls, overlay, evidence

    def test_cli_uses_integrity_bound_metadata_and_full_repository_url_for_raw_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            urls, overlay, evidence = self.run_mocked(Path(temporary))
            raw = [url for url in urls if url.startswith("https://raw.githubusercontent.com/")]
            self.assertEqual(raw, ["https://raw.githubusercontent.com/theKashey/react-remove-scroll-bar/" + "a" * 40 + "/LICENSE"])
            index = json.loads((overlay / "ThirdPartyLicenses/DependencyTexts/license-index.json").read_text())
            row = index["packages"][0]
            self.assertEqual(row["text_status"], "license_text_found")
            self.assertEqual(row["source_revision"], "a" * 40)
            receipt = json.loads((evidence / "fetch-receipt.json").read_text())
            self.assertTrue(receipt["no_crate_archives_or_model_weights_requested"])

    def test_registry_integrity_mismatch_never_requests_raw_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            urls, overlay, evidence = self.run_mocked(Path(temporary), registry_integrity="sha512-not-lock")
            self.assertFalse(any(url.startswith("https://raw.githubusercontent.com/") for url in urls))
            index = json.loads((overlay / "ThirdPartyLicenses/DependencyTexts/license-index.json").read_text())
            row = index["packages"][0]
            self.assertEqual(row["text_status"], "unresolved_no_full_text_candidate")
            receipt = json.loads((evidence / "fetch-receipt.json").read_text())
            self.assertTrue(any(r["url"].startswith("https://registry.npmjs.org/") for r in receipt["requests"]))
            self.assertFalse(any(r["url"].startswith("https://raw.githubusercontent.com/") for r in receipt["requests"]))


if __name__ == "__main__":
    unittest.main()
