#!/usr/bin/env python3
"""Build an offline, lock-bound third-party text inventory.

This records attribution inputs. It does not decide whether any binary ships a
package or whether its license obligations are satisfied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import tomllib
from pathlib import Path
from typing import Any, Iterable
from urllib.parse import urlsplit


SCHEMA_VERSION = 1
MAX_TEXT_BYTES = 10_000_000
SKIP_DIRS = {".git", "node_modules", "target", ".cache", ".turbo"}
LICENSE_BASENAME = re.compile(
    r"^(?:licen[cs]es?|copying|notices?|copyright)(?:[._-].*)?$", re.IGNORECASE
)
CODE_SUFFIXES = {
    ".rs", ".c", ".h", ".cc", ".cpp", ".m", ".mm", ".swift", ".go",
    ".js", ".mjs", ".cjs", ".jsx", ".ts", ".tsx", ".py", ".map",
    ".json", ".wasm",
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def hash_file(path: Path) -> tuple[str, int]:
    data = path.read_bytes()
    return sha256(data), len(data)


def package_id(name: str, version: str, source: str | None) -> tuple[str, str, str | None]:
    return name, version, source


def require_exact_identity(expected: tuple[Any, ...], actual: tuple[Any, ...], label: str) -> None:
    if expected != actual:
        raise ValueError(f"{label} identity mismatch: expected {expected!r}, got {actual!r}")


def contained_path(root: Path, candidate: Path, *, must_exist: bool = True) -> Path:
    root_real = root.resolve(strict=True)
    candidate_real = candidate.resolve(strict=must_exist)
    if not candidate_real.is_relative_to(root_real):
        raise ValueError(f"path escapes package root: {candidate.name}")
    return candidate_real


def is_license_filename(name: str) -> bool:
    """Match conventional license, copying, notice, and copyright text names."""
    folded = name.casefold().replace("_", "-")
    if Path(folded).suffix in CODE_SUFFIXES:
        return False
    return bool(LICENSE_BASENAME.match(folded))


def is_license_path(path: Path) -> bool:
    if is_license_filename(path.name):
        return True
    if any(part.casefold() in {"licenses", "licences"} for part in path.parts[:-1]):
        return Path(path.name).suffix.casefold() not in CODE_SUFFIXES
    return False


def license_body_families(data: bytes) -> list[str]:
    # Normalize punctuation as well as whitespace so valid variants such as
    # `provided "AS IS"` still match, while each family requires its actual
    # grant, conditions, and disclaimer clauses.
    text = re.sub(r"[^a-z0-9]+", " ", data.decode("utf-8", errors="replace").casefold())
    def contains(phrase: str) -> bool:
        normalized = re.sub(r"[^a-z0-9]+", " ", phrase.casefold()).strip()
        return normalized in text
    families: list[str] = []
    if (
        contains("permission is hereby granted, free of charge")
        and contains("shall be included in all copies")
        and contains("the software is provided as is")
        and (contains("shall not be liable") or (contains("in no event shall") and contains("be liable")))
    ):
        families.append("MIT")
    if (
        contains("1. Definitions")
        and contains("2. Grant of Copyright License")
        and contains("Disclaimer of Warranty")
        and contains("Limitation of Liability")
        and contains("End of Terms and Conditions")
    ):
        families.append("Apache-2.0")
    if (
        contains("permission is granted to anyone to use this software for any purpose")
        and contains("altered source versions must be plainly marked as such")
        and contains("this notice may not be removed or altered from any source distribution")
    ):
        families.append("Zlib")
    if (
        contains("redistribution and use in source and binary forms")
        and contains("neither the name")
        and (contains("as is") or contains("without warranty"))
    ):
        families.append("BSD")
    if (
        contains("permission to use, copy, modify, and/or distribute this software")
        and (contains("the above copyright notice and this permission notice") or contains("with or without fee is hereby granted"))
        and (contains("as is") or contains("without warranty"))
        and (contains("shall not be liable") or (contains("in no event shall") and contains("be liable")))
    ):
        families.append("ISC-or-0BSD")
    if (
        contains("this is free and unencumbered software released into the public domain")
        and contains("anyone is free to copy, modify, publish")
        and contains("as is")
    ):
        families.append("Unlicense")
    if (
        contains("unicode data files include all data files")
        and contains("permission is hereby granted")
        and contains("copyright and permission notice")
    ):
        families.append("Unicode-3.0")
    if (
        contains("mozilla public license version 2.0")
        and contains("1. definitions")
        and contains("2. license grants")
        and contains("3. responsibilities")
    ):
        families.append("MPL-2.0")
    if (
        contains("boost software license - version 1.0")
        and contains("permission is hereby granted, free of charge")
        and contains("software is provided as is")
    ):
        families.append("BSL-1.0")
    if (
        contains("cc0 1.0 universal")
        and contains("creative commons legal code")
        and contains("statement of purpose")
        and contains("permanently relinquish those rights")
    ):
        families.append("CC0-1.0")
    return families


def license_reference_families(data: bytes) -> list[str]:
    text = data.decode("utf-8", errors="replace").casefold()
    families=[]
    if "apache" in text: families.append("Apache-2.0")
    if "zlib" in text or "zlib.net/zlib_license" in text: families.append("Zlib")
    if "mit license" in text or "[mit]" in text or "opensource.org/license/mit" in text: families.append("MIT")
    if "mpl-2.0" in text or "mozilla public license" in text: families.append("MPL-2.0")
    if "boost software license" in text: families.append("BSL-1.0")
    return sorted(set(families))


def license_classification_evidence(path: Path, data: bytes, kind: str | None = None) -> dict[str, Any] | None:
    bodies=license_body_families(data)
    references=license_reference_families(data)
    if not bodies and not references:
        return None
    evidence: dict[str, Any] = {}
    if bodies: evidence['complete_license_families']=bodies
    if references:
        evidence['referenced_license_families']=[family for family in references if family not in bodies]
    if kind == 'mixed_license_text_and_reference':
        evidence['classification_basis']='At least one complete license body is present; referenced license headers/links are not treated as full terms.'
    elif kind == 'license_reference_metadata' and not bodies:
        evidence['classification_basis']='License names or canonical links are present without a recognized complete terms body.'
    return evidence


def classify_text(path: Path, data: bytes) -> str:
    head = data[:2048].decode("utf-8", errors="replace")
    if path.suffix.casefold() == ".spdx" or re.search(r"(?im)^(?:SPDXVersion|PackageLicenseDeclared):", head):
        return "spdx_metadata"
    folded = path.name.casefold().replace("_", "-")
    if path.suffix.casefold() in CODE_SUFFIXES:
        return "other_metadata"
    if folded.startswith(("notice", "copyright")):
        return "notice_text"
    in_license_directory = any(part.casefold() in {"licenses", "licences"} for part in path.parts[:-1])
    if folded.startswith(("license", "licence", "copying")) or in_license_directory:
        families=license_body_families(data)
        compact = "\n".join(line.strip() for line in data[:4096].decode("utf-8", errors="replace").splitlines() if line.strip())
        lower = data.decode("utf-8", errors="replace").casefold()
        is_objc2_guide = (
            "licensing of these crates is a bit complicated" in lower
            and "trio-licensed" in lower
            and "apple sdks" in lower
        )
        pointer = bool(
            re.search(r"(?is)\bsee\s+(?:the\s+)?(?:full\s+)?(?:license|licence|copying)\s+(?:file|text|document|at|here|in|other|additional|available)\b", lower)
            or re.search(r"(?is)\bsee\s+LICENSE[\w.-]+", lower)
        )
        apache_header = "you may obtain a copy of the license" in lower and "apache" in lower and "apache-2.0" not in families
        spdx_only = bool(re.fullmatch(r"(?is)(?:spdx-license-identifier:.*|see\s+.*license.*|https?://\S+)", compact))
        if families:
            references=license_reference_families(data)
            if any(ref not in families for ref in references) or (pointer and families):
                return "mixed_license_text_and_reference"
            return "license_text"
        if is_objc2_guide or len(data) < 400 or pointer or apache_header or spdx_only:
            return "license_reference_metadata"
        # A long license-named file counts as terms only when it has clauses
        # showing a grant, conditions, and a warranty/disclaimer section.
        has_grant = any(term in lower for term in ("permission is granted", "permission is hereby granted", "redistribution and use in"))
        has_conditions = any(term in lower for term in ("provided that", "subject to the following", "shall be included", "provided, however"))
        has_disclaimer = any(term in lower for term in ("as is", "without warranty", "disclaims all warranties", "shall not be liable"))
        if has_grant and has_conditions and has_disclaimer:
            return "license_text"
        return "license_reference_metadata"
    return "other_metadata"


def has_license_text(record: dict[str, Any]) -> bool:
    return any(text.get("kind") in {"license_text", "mixed_license_text_and_reference"} for text in record.get("texts", []))


def text_status(texts: list[dict[str, Any]]) -> str:
    kinds = {text.get("kind") for text in texts}
    if kinds & {"license_text", "mixed_license_text_and_reference"}:
        return "license_text_found"
    if "notice_text" in kinds:
        return "notice_text_only_no_license_text"
    if kinds & {"spdx_metadata", "license_reference_metadata", "other_metadata"}:
        return "metadata_only_no_license_text"
    return "unresolved_no_full_text_candidate"


def reclassify_saved_text_records(index: dict[str, Any], overlay_root: Path) -> dict[str, Any]:
    """Recheck saved text bytes without fetching or touching package sources."""
    for row in index.get("packages", []):
        for text in row.get("texts", []):
            path = overlay_root / text["output_path"]
            data = path.read_bytes()
            if len(data) != text.get("bytes") or sha256(data) != text.get("sha256"):
                raise ValueError(f"saved license text hash/size mismatch: {text.get('source_path', 'unknown')}")
            source_path = text.get("source_path", "")
            basename = text.get("source_basename") or Path(source_path).name or "LICENSE"
            classification_path = Path("LICENSE-" + Path(source_path).name) if source_path.startswith("canonical-license-texts/") else Path(basename)
            kind = classify_text(classification_path, data)
            text["kind"] = kind
            text.pop("classification_evidence", None)
            evidence = license_classification_evidence(classification_path, data, kind)
            if evidence:
                text["classification_evidence"] = evidence
        if row.get("text_source_relationship") and has_license_text(row):
            row["text_status"] = "license_text_found_from_same_repo_release_peer"
            row["unresolved_reason"] = None
        elif has_license_text(row):
            row["text_status"] = "license_text_found"
            row["unresolved_reason"] = None
        elif row.get("first_party") and not row.get("license_expression"):
            row["text_status"] = "first_party_license_undeclared"
            row["unresolved_reason"] = "First-party workspace package; project-wide license declaration was not found. This is not a third-party dependency text gap."
        else:
            row["text_status"] = text_status(row.get("texts", []))
    packages = index.get("packages", [])
    index.setdefault("summary", {}).update({
        "records_with_full_license_text": sum(has_license_text(row) for row in packages),
        "records_without_full_license_text": sum(not has_license_text(row) for row in packages),
        "third_party_records_without_full_license_text": sum(not row.get("first_party") and not has_license_text(row) for row in packages),
        "first_party_undeclared_license_records": sum(bool(row.get("first_party") and not row.get("license_expression")) for row in packages),
        "unique_dependency_files": len(list((overlay_root / "ThirdPartyLicenses/DependencyTexts/by-sha256").glob("*.txt"))),
    })
    return index


def scan_license_files(package_root: Path, *, recursive: bool = True) -> list[tuple[Path, bytes, str]]:
    root = package_root.resolve(strict=True)
    found: list[tuple[Path, bytes, str]] = []
    if not recursive:
        paths: Iterable[Path] = root.iterdir()
    else:
        walked: list[Path] = []
        for current, dirs, files in os.walk(root, followlinks=False):
            dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
            base = Path(current)
            walked.extend(base / name for name in files)
        paths = walked

    for path in sorted(paths, key=lambda item: item.as_posix().casefold()):
        if not is_license_path(path):
            continue
        try:
            safe = contained_path(root, path)
            if not safe.is_file():
                continue
            data = safe.read_bytes()
        except (OSError, ValueError):
            continue
        if data and len(data) <= MAX_TEXT_BYTES:
            found.append((safe, data, classify_text(safe, data)))
    return found


def safe_source_label(source: str | None, repo: Path, package_root: Path) -> tuple[str, str | None]:
    if source is None:
        try:
            rel = package_root.resolve().relative_to(repo.resolve()).as_posix()
        except ValueError:
            rel = "workspace-package"
        return f"workspace:{rel}", None
    if source.startswith("registry+"):
        if "crates.io-index" in source:
            return "registry:crates.io", None
        return "registry:external", None
    if source.startswith("git+"):
        raw = source[4:]
        before_fragment, _, revision = raw.partition("#")
        parsed = urlsplit(before_fragment.split("?", 1)[0])
        if parsed.scheme in {"http", "https"} and parsed.hostname:
            host = parsed.hostname.casefold()
            path = parsed.path.strip("/")
            if host == "github.com":
                origin = f"git:{host}/{path}"
            else:
                origin = f"git:{host}/external"
            return origin, revision or None
        return "git:external", revision or None
    return "source:other", None


def public_repository_reference(repository: str | None) -> str | None:
    if not repository:
        return None
    value = repository.removeprefix("git+").removesuffix(".git")
    parsed = urlsplit(value)
    if parsed.scheme not in {"http", "https"} or parsed.hostname != "github.com":
        return None
    path = parsed.path.strip("/")
    if not path or "@" in parsed.netloc:
        return None
    return f"https://github.com/{path}"


def source_relative(root: Path, file_path: Path, ecosystem: str, name: str, version: str, repo: Path) -> str:
    rel = file_path.resolve().relative_to(root.resolve()).as_posix()
    if ecosystem == "rust" and root.resolve().is_relative_to(repo.resolve()):
        package_root_rel = root.resolve().relative_to(repo.resolve()).as_posix()
        return f"{package_root_rel}/{rel}"
    if ecosystem == "rust":
        return f"cargo-source/{name}-{version}/{rel}"
    try:
        package_path = root.resolve().relative_to(repo.resolve()).as_posix()
    except ValueError:
        package_path = f"node_modules/{name}"
    return f"{package_path}/{rel}"


def reject_symlink_components(path: Path) -> None:
    absolute = Path(os.path.abspath(path))
    for candidate in (absolute, *absolute.parents):
        if candidate.is_symlink():
            raise ValueError(f"symlink path is not allowed: {candidate.name}")


def path_overlaps(left: Path, right: Path) -> bool:
    return left == right or left.is_relative_to(right) or right.is_relative_to(left)


def cargo_metadata_source_paths(metadata: dict[str, Any]) -> list[Path]:
    """Return all Cargo source roots and declared external license inputs."""
    protected: list[Path] = []
    for package in metadata.get("packages", []):
        manifest = Path(package["manifest_path"])
        reject_symlink_components(manifest)
        manifest_real = manifest.resolve(strict=True)
        if not manifest_real.is_file():
            raise ValueError("Cargo metadata manifest_path is not a file")
        package_root = manifest_real.parent
        protected.append(package_root)
        declared = package.get("license_file")
        if not declared:
            continue
        candidate = Path(declared)
        if not candidate.is_absolute():
            candidate = package_root / candidate
        reject_symlink_components(candidate)
        if candidate.exists():
            resolved = candidate.resolve(strict=True)
            if not resolved.is_file():
                raise ValueError("Cargo metadata license_file is not a file")
            protected.append(resolved)
            protected.append(resolved.parent)
        else:
            # Preserve a declared but missing input as a protected path too.
            # It cannot be silently created by the output writer.
            protected.append(Path(os.path.abspath(candidate)))
            protected.append(Path(os.path.abspath(candidate)).parent)
    return protected


def validate_new_external_outputs(
    repo: Path,
    metadata_path: Path,
    observations_path: Path,
    output_root: Path,
    evidence_root: Path,
    *,
    protected_source_paths: Iterable[Path] = (),
) -> tuple[Path, Path]:
    protected = [repo.resolve(strict=True), metadata_path.resolve(strict=True), observations_path.resolve(strict=True)]
    protected.extend(Path(os.path.abspath(path)) for path in protected_source_paths)
    outputs = []
    for label, raw in (("output root", output_root), ("evidence root", evidence_root)):
        absolute = Path(os.path.abspath(raw))
        reject_symlink_components(absolute)
        if absolute.exists():
            raise ValueError(f"{label} must be a new, absent path")
        if not absolute.parent.is_dir():
            raise ValueError(f"{label} parent must already exist")
        resolved_parent = absolute.parent.resolve(strict=True)
        absolute = resolved_parent / absolute.name
        if any(path_overlaps(absolute, source) for source in protected):
            raise ValueError(f"{label} overlaps a read-only source input")
        outputs.append(absolute)
    if path_overlaps(outputs[0], outputs[1]):
        raise ValueError("output and evidence roots must be disjoint")
    return outputs[0], outputs[1]


def write_bytes(path: Path, data: bytes, *, root: Path) -> None:
    """Write one file beneath an already-created output/evidence root."""
    root_abs = Path(os.path.abspath(root))
    path_abs = Path(os.path.abspath(path))
    reject_symlink_components(root_abs)
    reject_symlink_components(path_abs)
    if not root_abs.is_dir():
        raise ValueError("output containment root must be an existing directory")
    if not path_abs.is_relative_to(root_abs):
        raise ValueError("output destination escapes its declared root")
    path_abs.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    reject_symlink_components(path_abs)
    root_real = root_abs.resolve(strict=True)
    if not path_abs.parent.resolve(strict=True).is_relative_to(root_real):
        raise ValueError("output destination resolves outside its declared root")
    path = path_abs
    if path.exists() and not path.is_file():
        raise ValueError("output target exists and is not a regular file")
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(path, flags, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
    path.chmod(0o600)
    path.parent.chmod(0o700)


def harden_output_tree(root: Path) -> None:
    reject_symlink_components(root)
    for path in sorted(root.rglob("*"), key=lambda item: len(item.parts), reverse=True):
        if path.is_symlink():
            raise ValueError("generated output tree contains a symlink")
        path.chmod(0o700 if path.is_dir() else 0o600)
    root.chmod(0o700)


def record_existing_tree(src: Path) -> list[dict[str, Any]]:
    """Record unchanged Swift/MLX texts without copying or editing them."""
    records: list[dict[str, Any]] = []
    for path in sorted(p for p in src.rglob("*") if p.is_file()):
        rel = path.relative_to(src).as_posix()
        digest, size = hash_file(path)
        records.append({"path": f"ThirdPartyLicenses/{rel}", "sha256": digest, "bytes": size})
    return records


def load_observations(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("artifact_type") != "local_distribution_dependency_license_observations_v2":
        raise ValueError("unexpected dependency observation artifact type")
    return data


def cargo_records(repo: Path, metadata_path: Path, observations: dict[str, Any], text_root: Path) -> list[dict[str, Any]]:
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    lock = tomllib.loads((repo / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))
    lock_rows = lock.get("package", [])
    lock_by_id: dict[tuple[str, str, str | None], list[dict[str, Any]]] = {}
    for row in lock_rows:
        key = package_id(row["name"], row["version"], row.get("source"))
        lock_by_id.setdefault(key, []).append(row)

    obs_by_id = {row["id"]: row for row in observations["rust"]}
    result: list[dict[str, Any]] = []
    record_by_id: dict[str, dict[str, Any]] = {}
    metadata_by_id = {package["id"]: package for package in metadata["packages"]}
    seen_ids: set[str] = set()
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"], p.get("source") or "")):
        key = package_id(package["name"], package["version"], package.get("source"))
        matches = lock_by_id.get(key, [])
        if len(matches) != 1:
            raise ValueError(f"Cargo.lock must contain exactly one entry for {package['name']}@{package['version']}")
        lock_row = matches[0]
        require_exact_identity(key, package_id(lock_row["name"], lock_row["version"], lock_row.get("source")), "Cargo.lock")
        observation = obs_by_id.get(package["id"])
        if observation is None:
            raise ValueError(f"metadata package lacks prior graph-role observation: {package['name']}@{package['version']}")
        seen_ids.add(package["id"])
        package_root = Path(package["manifest_path"]).parent.resolve(strict=True)
        license_root = repo.resolve() if package.get("source") is None else package_root
        if package.get("source") is None:
            files = scan_license_files(package_root, recursive=False)
            if package_root != repo.resolve():
                files.extend(scan_license_files(repo, recursive=False))
        else:
            files = scan_license_files(package_root, recursive=True)
        declared_file = package.get("license_file")
        if declared_file:
            raw = Path(declared_file)
            candidate = raw if raw.is_absolute() else package_root / raw
            try:
                safe = contained_path(license_root, candidate)
                if safe.is_file() and all(safe != old[0] for old in files):
                    data = safe.read_bytes()
                    if data and len(data) <= MAX_TEXT_BYTES:
                        files.append((safe, data, classify_text(safe, data)))
            except (OSError, ValueError):
                pass

        source_label, revision = safe_source_label(package.get("source"), repo, package_root)
        expression = package.get("license")
        first_party = package.get("source") is None
        package_records = []
        for file_path, data, kind in sorted(files, key=lambda item: item[0].as_posix().casefold()):
            digest = sha256(data)
            out_rel = f"ThirdPartyLicenses/DependencyTexts/by-sha256/{digest}.txt"
            write_bytes(text_root / "by-sha256" / f"{digest}.txt", data, root=text_root)
            text_record = {
                "source_path": source_relative(license_root, file_path, "rust", package["name"], package["version"], repo),
                "output_path": out_rel,
                "sha256": digest,
                "bytes": len(data),
                "source_basename": file_path.name,
                "kind": kind,
            }
            evidence = license_classification_evidence(file_path, data, kind)
            if evidence:
                text_record["classification_evidence"] = evidence
            package_records.append(text_record)
        status = text_status(package_records)
        if first_party and not expression and not has_license_text({"texts": package_records}):
            status = "first_party_license_undeclared"
        record = {
            "ecosystem": "rust",
            "name": package["name"],
            "version": package["version"],
            "source": source_label,
            "source_revision": revision,
            "lock_identity": {
                "name": lock_row["name"],
                "version": lock_row["version"],
                "source": source_label,
                "checksum": lock_row.get("checksum"),
            },
            "license_expression": expression,
            "license_file_declared": bool(declared_file),
            "graph_role": observation["role"],
            "workspace_package": first_party,
            "first_party": first_party,
            "first_party_license_expression_absent": first_party and not expression,
            "text_status": status,
            "texts": package_records,
            "unresolved_reason": None if status == "license_text_found" else (
                "First-party workspace package; project-wide license declaration was not found. This is not a third-party dependency text gap."
                if first_party and not expression else
                "No full license text file was found inside the package distribution or declared project license file; "
                "the declared expression alone is not a text, and no sibling-package text was borrowed."
            ),
            "upstream_repository": public_repository_reference(package.get("repository")),
        }
        result.append(record)
        record_by_id[package["id"]] = record

    # Some published crate archives omit a repository-level license file even
    # though another crate from the exact same repository and release includes
    # it. Reuse only when repository, version, registry source, and expression
    # all agree, and preserve the actual peer package-relative origin in `texts`.
    for package in metadata["packages"]:
        record = record_by_id[package["id"]]
        if record["texts"] or not package.get("repository") or not package.get("license"):
            continue
        peer = next((
            candidate for candidate in metadata["packages"]
            if candidate["id"] != package["id"]
            and candidate.get("repository") == package.get("repository")
            and candidate["version"] == package["version"]
            and candidate.get("source") == package.get("source")
            and candidate.get("license") == package.get("license")
            and has_license_text(record_by_id[candidate["id"]])
        ), None)
        if peer is None:
            continue
        peer_record = record_by_id[peer["id"]]
        record["texts"] = [dict(text) for text in peer_record["texts"]]
        record["text_status"] = "license_text_found_from_same_repo_release_peer"
        record["unresolved_reason"] = None
        record["text_source_relationship"] = {
            "type": "same_repository_same_version_same_expression_peer",
            "source_package": f"{peer['name']}@{peer['version']}",
            "repository": public_repository_reference(package.get("repository")),
            "limitation": "Text bytes originate from the peer package distribution and are not present in this package archive.",
        }
    if seen_ids != set(obs_by_id):
        raise ValueError("Cargo metadata package set differs from reviewed graph-role observation set")
    return result


def npm_records(repo: Path, observations: dict[str, Any], text_root: Path) -> list[dict[str, Any]]:
    lock = json.loads((repo / "package-lock.json").read_text(encoding="utf-8"))
    packages = lock.get("packages", {})
    expected = {row["lock_path"]: row for row in observations["npm"]}
    nondev_paths = {
        path for path, row in packages.items()
        if path and not row.get("dev", False)
    }
    if nondev_paths != set(expected):
        raise ValueError(f"package-lock nondev identity set mismatch: lock={len(nondev_paths)} observations={len(expected)}")

    result: list[dict[str, Any]] = []
    for lock_path in sorted(nondev_paths):
        lock_row = packages[lock_path]
        observation = expected[lock_path]
        package_root = (repo / lock_path).resolve(strict=True)
        package_json = contained_path(repo, package_root / "package.json")
        package_meta_bytes = package_json.read_bytes()
        package_meta = json.loads(package_meta_bytes)
        require_exact_identity(
            (observation["name"], observation["version"]),
            (package_meta.get("name"), package_meta.get("version")),
            "installed npm package",
        )
        require_exact_identity(
            (observation["name"], observation["version"]),
            (package_meta.get("name"), lock_row.get("version")),
            "package-lock entry",
        )
        if sha256(package_meta_bytes) != observation["package_metadata_sha256"]:
            raise ValueError(f"installed package.json hash changed for {observation['name']}@{observation['version']}")
        files = scan_license_files(package_root, recursive=True)
        package_records = []
        for file_path, data, kind in sorted(files, key=lambda item: item[0].as_posix().casefold()):
            digest = sha256(data)
            out_rel = f"ThirdPartyLicenses/DependencyTexts/by-sha256/{digest}.txt"
            write_bytes(text_root / "by-sha256" / f"{digest}.txt", data, root=text_root)
            text_record = {
                "source_path": source_relative(package_root, file_path, "npm", package_meta["name"], package_meta["version"], repo),
                "output_path": out_rel,
                "sha256": digest,
                "bytes": len(data),
                "source_basename": file_path.name,
                "kind": kind,
            }
            evidence = license_classification_evidence(file_path, data, kind)
            if evidence:
                text_record["classification_evidence"] = evidence
            package_records.append(text_record)
        expression = package_meta.get("license") or lock_row.get("license")
        status = text_status(package_records)
        resolved = lock_row.get("resolved") or ""
        result.append({
            "ecosystem": "npm",
            "name": package_meta["name"],
            "version": package_meta["version"],
            "source": "npm:registry.npmjs.org" if resolved.startswith("https://registry.npmjs.org/") else "npm:lock-source",
            "lock_identity": {
                "lock_path": lock_path,
                "version": lock_row["version"],
                "integrity": lock_row.get("integrity"),
            },
            "license_expression": expression,
            "installed_license_expression": package_meta.get("license"),
            "graph_role": "production_or_optional_nondev_lock_entry",
            "text_status": status,
            "texts": package_records,
            "unresolved_reason": None if status == "license_text_found" else (
                "The locked installed package has a license expression but no full license text candidate; "
                "no text from a similarly named package was borrowed."
            ),
            "package_metadata_sha256": sha256(package_meta_bytes),
            "upstream_repository": public_repository_reference(package_meta.get("repository", {}).get("url") if isinstance(package_meta.get("repository"), dict) else package_meta.get("repository")),
        })
    return result


def markdown_escape(value: Any) -> str:
    return str(value if value is not None else "—").replace("|", "\\|").replace("\n", " ")


def render_dependency_markdown(records: list[dict[str, Any]]) -> str:
    rust = [r for r in records if r["ecosystem"] == "rust"]
    npm = [r for r in records if r["ecosystem"] == "npm"]
    included = sum(has_license_text(r) for r in records)
    unresolved = [r for r in records if not has_license_text(r) and not r.get("first_party")]
    first_party_undeclared = [r for r in records if r.get("first_party") and not has_license_text(r)]
    canonical_choices = [r for r in records if r.get("canonical_license_choice")]
    lines = [
        "# Locked dependency license-text inventory",
        "",
        "Generated from the application Cargo.lock, platform-filtered Cargo metadata, root package-lock.json, and matching installed npm package metadata. Text candidates are copied byte-for-byte from package distributions, exact source revisions, or official canonical license sources explicitly identified by a package declaration/pointer. Same-repository peer text is explicitly marked and retains the peer's source path.",
        "",
        "This is an attribution inventory, not a legal license certification, a linker map, or proof that every locked dependency is included in the shipped binary. Rust rows include the metadata graph and mark build-only nodes separately. Npm rows include non-development lock entries. A declared SPDX expression without a full text remains unresolved. A mixed text includes at least one complete license body plus a separate reference or incomplete header; the reference is not counted as another complete body.",
        "",
        f"Package records: {len(rust)} Rust ({sum(r['graph_role'] == 'normal_graph' for r in rust)} normal graph; {sum(r['graph_role'] == 'build_only_graph' for r in rust)} build-only graph) and {len(npm)} npm. Full license texts are present for {included} records; {len(unresolved)} third-party records remain without a full license text.",
        "",
        "Machine-readable source/version/license/text hashes: [license-index.json](license-index.json). Text files are stored once under [by-sha256/](by-sha256/).",
        "",
        "## Rust package graph",
        "",
        "| Package | Version | Declared expression | Graph role | Text candidates |",
        "|---|---:|---|---|---|",
    ]
    for row in rust:
        text_links = [f"[{markdown_escape(t['source_basename'])} ({t['kind']})](by-sha256/{Path(t['output_path']).name})" for t in row["texts"]]
        text_label = ", ".join(text_links) if text_links else "unresolved"
        if row.get("first_party"):
            text_label = "first-party; project license undeclared"
        if row.get("text_source_relationship"):
            text_label += f" (peer: {row['text_source_relationship']['source_package']})"
        if row.get("canonical_license_choice"):
            text_label += " (canonical MIT option; not package-archive bytes)"
        lines.append(f"| `{row['name']}` | `{row['version']}` | {markdown_escape(row['license_expression'])} | {row['graph_role']} | {text_label} |")
    lines += ["", "## Npm production lock entries", "", "| Package | Version | Declared expression | Text candidates |", "|---|---:|---|---|"]
    for row in npm:
        text_links = [f"[{markdown_escape(t['source_basename'])} ({t['kind']})](by-sha256/{Path(t['output_path']).name})" for t in row["texts"]]
        text_label = ", ".join(text_links) if text_links else "unresolved"
        lines.append(f"| `{row['name']}` | `{row['version']}` | {markdown_escape(row['license_expression'])} | {text_label} |")
    lines += ["", "## Unresolved records", ""]
    if unresolved:
        for row in unresolved:
            extra = " (SPDX metadata or notice files are separately indexed)" if row["texts"] else ""
            lines.append(f"- `{row['ecosystem']} {row['name']}@{row['version']}` — declared expression: {markdown_escape(row['license_expression'])}; no full license text found{extra}.")
    else:
        lines.append("None.")
    if first_party_undeclared:
        lines += ["", "## First-party package license declaration", ""]
        for row in first_party_undeclared:
            lines.append(f"- `{row['name']}@{row['version']}` is a first-party workspace package. The project license declaration is absent; this is not a third-party dependency text gap.")
    if canonical_choices:
        lines += [
            "",
            "## Canonical license choices",
            "",
            "Canonical license bodies are separate from package-archive bytes. The objc2 family uses the canonical SPDX MIT body as one option identified by each exact pinned source guide. The guide also notes separate licensing uncertainty for Apple SDK-derived code, which this attribution inventory does not resolve.",
            "",
        ]
        for row in canonical_choices:
            choice = row["canonical_license_choice"]
            lines.append(f"- `{row['name']}@{row['version']}`: {choice['license_id']} canonical text [{choice['text_sha256'][:12]}…] cited by the exact source guide at revision `{choice['guide_source_revision']}`; not package-archive bytes.")
    lines += [
        "",
        "No license text is synthesized. Any same-repository peer text is clearly marked with its source package and is not represented as present in the target package archive. Repository links identify the upstream project; exact remote text imports carry a separate registry gitHead or Cargo .cargo_vcs_info revision binding in the JSON index. Canonical standard-license bodies are clearly distinguished from files shipped inside a package archive.",
        "For `siphasher@1.0.3`, the canonical SPDX MIT text retains its literal `<year> <copyright holders>` placeholder. The exact pinned package `COPYING` file separately preserves its copyright lines and points to the official Apache/MIT terms; no project-specific placeholder text was filled in.",
        "",
    ]
    return "\n".join(lines)


def render_notice_appendix(rust_count: int, normal_count: int, build_count: int, npm_count: int, unresolved_count: int, first_party_count: int) -> str:
    return f"""

## Locked Rust and frontend dependency texts

The current candidate adds a lock-bound attribution inventory for {rust_count} Cargo metadata packages ({normal_count} normal dependency graph nodes and {build_count} build-only nodes) and {npm_count} non-development root npm lock entries. These are lock/metadata populations, not a claim that every record contributes linked runtime code. Package-distributed license/notice text candidates and explicitly identified same-repository, same-version peer texts are included under [`ThirdPartyLicenses/DependencyTexts/`](ThirdPartyLicenses/DependencyTexts/DEPENDENCY_LICENSES.md); package name, version, source class, declared expression, lock identity, per-text source-relative path, byte count, and SHA-256 are in [`license-index.json`](ThirdPartyLicenses/DependencyTexts/license-index.json).

The inventory preserves declared expressions when text is absent and lists {unresolved_count} third-party records without a full license body. It separately records {first_party_count} first-party workspace package(s) without a declared project license. A missing text is not treated as permission, and the inventory is not legal clearance. It does not include model weights or certify exact runtime linkage. Existing Swift/MLX and other source notices are preserved.
"""


def render_license_readme_appendix(unresolved_count: int, first_party_count: int) -> str:
    return f"""

## Locked Rust and frontend dependency texts

`DependencyTexts/` holds the lock-bound package/version/source/license index, a human-readable package table, and deduplicated copies of discovered license and notice files. SPDX tag-value metadata and short license pointers are labeled separately; they are not counted as full license bodies. The current inventory has {unresolved_count} third-party lock records without a full license text and {first_party_count} first-party workspace package(s) without a declared project license. It is an attribution inventory, not legal clearance or proof that each lock entry is linked into the binary. Model weights are outside this inventory.

The existing `package.sh` stage copy carries this directory into the staged resources. Its byte-exact checkout verification remains scoped to the pinned Swift/MLX `Upstream/` texts and the compatibility-runtime reference; package text provenance and SHA-256 values are recorded in `DependencyTexts/license-index.json`.
"""


def generate(repo: Path, metadata_path: Path, observations_path: Path, output_root: Path, evidence_root: Path) -> dict[str, Any]:
    for label, source in (("repository", repo), ("Cargo metadata", metadata_path), ("observations", observations_path)):
        reject_symlink_components(source)
        if not source.exists():
            raise ValueError(f"{label} input does not exist")
    repo = repo.resolve(strict=True)
    metadata_path = metadata_path.resolve(strict=True)
    observations_path = observations_path.resolve(strict=True)
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    protected_sources = cargo_metadata_source_paths(metadata)
    output_root, evidence_root = validate_new_external_outputs(
        repo,
        metadata_path,
        observations_path,
        output_root,
        evidence_root,
        protected_source_paths=protected_sources,
    )
    output_root.mkdir(mode=0o700)
    evidence_root.mkdir(mode=0o700)
    output_root.chmod(0o700)
    evidence_root.chmod(0o700)
    text_root = output_root / "ThirdPartyLicenses" / "DependencyTexts"
    text_root.mkdir(parents=True, exist_ok=True, mode=0o700)
    observations = load_observations(observations_path)
    rust = cargo_records(repo, metadata_path, observations, text_root)
    npm = npm_records(repo, observations, text_root)
    records = rust + npm
    records.sort(key=lambda r: (r["ecosystem"], r["name"].casefold(), r["version"], r.get("source", "")))

    source_licenses = record_existing_tree(repo / "ThirdPartyLicenses")
    lock_cargo = repo / "src-tauri/Cargo.lock"
    lock_npm = repo / "package-lock.json"
    notice_baseline = repo / "THIRD_PARTY_NOTICES.md"
    license_readme_baseline = repo / "ThirdPartyLicenses/README.md"
    tauri_config = repo / "src-tauri/tauri.conf.json"
    sidecar_packager = repo / "src-tauri/mlx-sidecar/scripts/package.sh"
    index = {
        "schema_version": SCHEMA_VERSION,
        "artifact_type": "voiceflow-locked-dependency-license-text-index",
        "generated_date": "2026-10-26",
        "statement": "Attribution inventory only; not legal clearance or proof of exact binary linkage.",
        "text_classification": "License-named files are treated as full license text only when they contain a nontrivial body; SPDX tag-value files are metadata, and NOTICE/COPYRIGHT files remain notice texts rather than substitutes for a license body.",
        "inputs": {
            "cargo_lock": {"path": "src-tauri/Cargo.lock", "sha256": hash_file(lock_cargo)[0], "bytes": lock_cargo.stat().st_size},
            "package_lock": {"path": "package-lock.json", "sha256": hash_file(lock_npm)[0], "bytes": lock_npm.stat().st_size},
            "cargo_metadata": {"label": "offline Cargo metadata, filtered for aarch64-apple-darwin", "sha256": hash_file(metadata_path)[0], "bytes": metadata_path.stat().st_size, "package_count": len(rust)},
            "prior_observation_set": {"label": "prior offline dependency inventory used only to bind the metadata graph roles and npm lock paths", "sha256": hash_file(observations_path)[0]},
            "notice_baseline": {"path": "THIRD_PARTY_NOTICES.md", "sha256": hash_file(notice_baseline)[0], "bytes": notice_baseline.stat().st_size},
            "license_readme_baseline": {"path": "ThirdPartyLicenses/README.md", "sha256": hash_file(license_readme_baseline)[0], "bytes": license_readme_baseline.stat().st_size},
            "existing_packaging_copy_path": {
                "tauri_config": {"path": "src-tauri/tauri.conf.json", "sha256": hash_file(tauri_config)[0]},
                "sidecar_packager": {"path": "src-tauri/mlx-sidecar/scripts/package.sh", "sha256": hash_file(sidecar_packager)[0]},
                "existing_route": "package.sh copies ThirdPartyLicenses into target/mlx-sidecar/stage/ThirdPartyLicenses; tauri.conf.json maps that stage directory to app resources.",
            },
        },
        "preserved_existing_swift_mlx_tree_files": source_licenses,
        "summary": {
            "rust_package_records": len(rust),
            "rust_normal_graph_records": sum(r["graph_role"] == "normal_graph" for r in rust),
            "rust_build_only_graph_records": sum(r["graph_role"] == "build_only_graph" for r in rust),
            "npm_nondev_records": len(npm),
            "records_with_full_license_text": sum(has_license_text(r) for r in records),
            "records_without_full_license_text": sum(not has_license_text(r) for r in records),
            "third_party_records_without_full_license_text": sum(not r.get("first_party") and not has_license_text(r) for r in records),
            "first_party_undeclared_license_records": sum(r.get("first_party_license_expression_absent", False) for r in records),
            "unique_dependency_files": len(list((text_root / "by-sha256").glob("*.txt"))),
        },
        "packages": records,
    }
    index_bytes = (json.dumps(index, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode()
    write_bytes(text_root / "license-index.json", index_bytes, root=text_root)
    md = render_dependency_markdown(records).encode()
    write_bytes(text_root / "DEPENDENCY_LICENSES.md", md, root=text_root)
    notice_path = repo / "THIRD_PARTY_NOTICES.md"
    existing_notice = notice_path.read_bytes()
    appendix = render_notice_appendix(
        len(rust),
        index["summary"]["rust_normal_graph_records"],
        index["summary"]["rust_build_only_graph_records"],
        len(npm),
        index["summary"]["third_party_records_without_full_license_text"],
        index["summary"]["first_party_undeclared_license_records"],
    ).encode()
    write_bytes(output_root / "THIRD_PARTY_NOTICES.md", existing_notice.rstrip() + appendix + b"\n", root=output_root)
    existing_license_readme = license_readme_baseline.read_bytes()
    readme_appendix = render_license_readme_appendix(
        index["summary"]["third_party_records_without_full_license_text"],
        index["summary"]["first_party_undeclared_license_records"],
    ).encode()
    write_bytes(output_root / "ThirdPartyLicenses/README.md", existing_license_readme.rstrip() + readme_appendix + b"\n", root=output_root)

    input_manifest = {
        "artifact_type": "voiceflow-dependency-notices-source-manifest",
        "schema_version": 1,
        "inputs": index["inputs"],
        "preserved_swift_mlx_tree_files": source_licenses,
        "output_files": [],
        "path_disclosure": "All recorded paths are project-relative or package-root-relative; machine-local cache roots are omitted.",
    }
    output_parent = output_root.parent.resolve()
    for path in sorted(p for p in output_root.rglob("*") if p.is_file()):
        reject_symlink_components(path)
        rel = path.relative_to(output_parent).as_posix()
        digest, size = hash_file(path)
        input_manifest["output_files"].append({"path": rel, "sha256": digest, "bytes": size})
    write_bytes(evidence_root / "source-and-output-manifest.json", (json.dumps(input_manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode(), root=evidence_root)
    harden_output_tree(output_root)
    harden_output_tree(evidence_root)
    return index


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=Path, help="read-only VoiceFlow repository root")
    parser.add_argument("--cargo-metadata", required=True, type=Path, help="offline Cargo metadata JSON for the locked macOS dependency graph")
    parser.add_argument("--observations", required=True, type=Path, help="prior offline observation JSON used to bind graph roles and npm package paths")
    parser.add_argument("--output-root", required=True, type=Path, help="new, absent project-overlay staging directory outside the repository")
    parser.add_argument("--evidence-root", required=True, type=Path, help="new, absent evidence directory outside the repository, disjoint from output")
    args = parser.parse_args()
    index = generate(args.repo, args.cargo_metadata, args.observations, args.output_root, args.evidence_root)
    print(json.dumps(index["summary"], sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
