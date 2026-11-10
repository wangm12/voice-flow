#!/usr/bin/env python3
"""Apply or verify VoiceFlow's exact pinned mlx-audio-swift source overlay."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import tempfile


LEGACY_PATCH_SHA256 = "888d17cfe319c05191d2ebd1276000da06906bb6764002e147527740a00883a2"
LEGACY_OVERLAY_AFTER_SHA256 = {
    "Sources/MLXAudioCore/DSP.swift": "b4830558aa62ac6ff9b3291589ee5a3879eac477333f2dcc4293e57fb094e174",
    "Sources/MLXAudioSTT/Models/Qwen3ASR/Qwen3ASR.swift": "9983a6925350f8a4f60807f7c496dff441622a523c3991cf37067722b136f45a",
    "Sources/MLXAudioSTT/Models/CohereTranscribe/CohereTranscribe.swift": "4af6aebc56911f13469cdf6b8ddb84ff1f5a3a995a0b7ce5a1bf941640882157",
}


def fail(message):
    raise SystemExit(message)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def checked_file(root, relative):
    rel = PurePosixPath(relative)
    if rel.is_absolute() or not rel.parts or any(part in ("", ".", "..") for part in rel.parts):
        fail(f"manifest path is not a safe relative path: {relative}")
    current = root
    for part in rel.parts[:-1]:
        current = current / part
        try:
            mode = current.lstat().st_mode
        except OSError as error:
            fail(f"overlay source path is unavailable: {relative}: {error}")
        if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
            fail(f"overlay source path traverses a non-directory or symlink: {relative}")
    path = current / rel.parts[-1]
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"overlay source file is unavailable: {relative}: {error}")
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        fail(f"overlay source is not an unaliased regular file: {relative}")
    return path, metadata


def optional_checked_file(root, relative):
    rel = PurePosixPath(relative)
    if rel.is_absolute() or not rel.parts or any(part in ("", ".", "..") for part in rel.parts):
        fail(f"manifest path is not a safe relative path: {relative}")
    current = root
    for part in rel.parts[:-1]:
        current = current / part
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            return None, None
        except OSError as error:
            fail(f"overlay target path is unavailable: {relative}: {error}")
        if stat.S_ISLNK(mode) or not stat.S_ISDIR(mode):
            fail(f"overlay target path traverses a non-directory or symlink: {relative}")
    path = current / rel.parts[-1]
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return None, None
    except OSError as error:
        fail(f"overlay target file is unavailable: {relative}: {error}")
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        fail(f"overlay target is not an unaliased regular file: {relative}")
    return path, metadata


def ensure_parent_directories(root, relative):
    rel = PurePosixPath(relative)
    current = root
    created = []
    for part in rel.parts[:-1]:
        current = current / part
        try:
            metadata = current.lstat()
        except FileNotFoundError:
            os.mkdir(current, 0o755)
            created.append(current)
            continue
        except OSError as error:
            fail(f"overlay target directory is unavailable: {relative}: {error}")
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
            fail(f"overlay target path traverses a non-directory or symlink: {relative}")
    return created


def replace_atomically(target, contents, temporary_suffix, mode=None):
    temporary_file = target.with_name(f".{target.name}.{temporary_suffix}")
    fd = os.open(temporary_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(contents)
            output.flush()
            os.fsync(output.fileno())
            if mode is not None:
                os.fchmod(output.fileno(), mode)
        os.replace(temporary_file, target)
    finally:
        temporary_file.unlink(missing_ok=True)


def replace_from_overlay(target, staged_source):
    replace_atomically(
        target,
        staged_source.read_bytes(),
        "voiceflow-overlay-tmp",
    )


def run_git(root, *args):
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        fail(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    # Preserve porcelain status columns; the first column is intentionally a
    # leading space for ordinary unstaged changes.
    return result.stdout.rstrip("\n")


def git_blob_bytes(root, revision, item):
    relative = item["path"]
    blob_id = item.get("base_git_blob")
    committed_blob = run_git(root, "rev-parse", f"{revision}:{relative}")
    if committed_blob != blob_id:
        fail(f"SDK pinned Git blob mismatch: {relative}")
    result = subprocess.run(
        ["git", "-C", str(root), "cat-file", "blob", blob_id],
        capture_output=True,
        check=False,
    )
    if result.returncode:
        fail(f"SDK pinned Git blob is unavailable: {relative}")
    if hashlib.sha256(result.stdout).hexdigest() != item.get("base_sha256"):
        fail(f"SDK pinned Git blob SHA-256 mismatch: {relative}")
    return result.stdout


def prior_overlay_after_hashes(manifest, current_files):
    provenance = manifest.get("legacy_prior_overlay")
    if not isinstance(provenance, dict):
        return None
    if provenance.get("patch_sha256") != LEGACY_PATCH_SHA256 or provenance.get("after_sha256") != LEGACY_OVERLAY_AFTER_SHA256:
        return None
    current_by_path = {item["path"]: item for item in current_files}
    for path, old_after_hash in LEGACY_OVERLAY_AFTER_SHA256.items():
        if current_by_path.get(path, {}).get("after_sha256") != old_after_hash:
            return None
    return dict(LEGACY_OVERLAY_AFTER_SHA256)


def status_records(status):
    records = []
    for line in status.splitlines():
        relative = line[3:]
        if " -> " in relative:
            relative = relative.split(" -> ", 1)[1]
        records.append((line[:2], relative))
    return records


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--require-after", action="store_true")
    args = parser.parse_args()

    source = args.source_root.resolve(strict=True)
    if not source.is_dir():
        fail("SDK source root must be a directory")
    manifest_path = Path(__file__).resolve().parent.parent / "sdk-overlay" / "patch-manifest.json"
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("schema") != "voiceflow-sdk-source-patch-manifest/v2":
        fail("SDK overlay manifest schema mismatch")
    upstream = manifest.get("upstream", {})
    revision = upstream.get("revision")
    if upstream.get("repository") != "https://github.com/Blaizzy/mlx-audio-swift" or revision != "01dec7c9bdce3088a6b6b7ab9f2e403458195efb":
        fail("SDK overlay upstream revision does not match the approved pin")
    license_file = Path(__file__).resolve().parents[3] / "ThirdPartyLicenses/Upstream/mlx-audio-swift/LICENSE"
    if digest(license_file) != upstream.get("license_sha256"):
        fail("preserved mlx-audio-swift MIT license hash mismatch")
    license_blob = subprocess.run(
        ["git", "hash-object", str(license_file)], text=True, capture_output=True, check=False
    )
    if license_blob.returncode or license_blob.stdout.strip() != upstream.get("license_blob"):
        fail("preserved mlx-audio-swift MIT license Git blob mismatch")

    patch_relative = manifest.get("patch")
    patch_path = (manifest_path.parent / patch_relative).resolve(strict=True)
    if patch_path.parent != manifest_path.parent or digest(patch_path) != manifest.get("patch_sha256"):
        fail("SDK overlay patch identity mismatch")
    files = manifest.get("files")
    base_paths = {
        "Sources/MLXAudioCore/DSP.swift",
        "Sources/MLXAudioSTT/Models/Qwen3ASR/Qwen3ASR.swift",
        "Sources/MLXAudioSTT/Models/CohereTranscribe/CohereTranscribe.swift",
        "Sources/MLXAudioSTT/Models/Parakeet/ParakeetModel.swift",
    }
    added_files = manifest.get("added_files")
    expected_added_paths = {
        "Sources/MLXAudioSTT/Models/FunASRNano/FunASRNanoAudio.swift",
        "Sources/MLXAudioSTT/Models/FunASRNano/FunASRNanoConfig.swift",
        "Sources/MLXAudioSTT/Models/FunASRNano/FunASRNanoModel.swift",
    }
    if not isinstance(files, list) or {item.get("path") for item in files} != base_paths:
        fail("SDK overlay file allowlist mismatch")
    if len(files) != len(base_paths):
        fail("SDK overlay file allowlist contains duplicates")
    if not isinstance(added_files, list) or {item.get("path") for item in added_files} != expected_added_paths:
        fail("SDK added source allowlist mismatch")
    if len(added_files) != len(expected_added_paths):
        fail("SDK added source allowlist contains duplicates")

    repo_root = Path(__file__).resolve().parents[3]
    actual_paths = {}
    actual_hashes = []
    base_hashes = []
    after_hashes = []
    for item in files:
        path, metadata = checked_file(source, item["path"])
        actual_paths[item["path"]] = (path, metadata)
        actual_hashes.append(digest(path))
        base_hashes.append(item["base_sha256"])
        after_hashes.append(item["after_sha256"])
        if item.get("source_url") != (
            f"https://github.com/Blaizzy/mlx-audio-swift/blob/{revision}/{item['path']}"
        ):
            fail(f"SDK overlay source URL mismatch: {item['path']}")

    added_actual_paths = {}
    added_actual_hashes = []
    added_after_hashes = []
    added_source_paths = {}
    for item in added_files:
        source_path, _ = checked_file(manifest_path.parent, item["source_path"])
        added_source_paths[item["path"]] = source_path
        if item.get("source_revision") != "262a391a6a63c2e6923e826bcc7122caee0f18f6":
            fail(f"SDK added source revision mismatch: {item['path']}")
        if item.get("source_url") != (
            f"https://github.com/Blaizzy/mlx-audio-swift/blob/{item['source_revision']}/"
            f"Sources/MLXAudioSTT/Models/FunASRNano/{PurePosixPath(item['path']).name}"
        ):
            fail(f"SDK added source URL mismatch: {item['path']}")
        source_hash = digest(source_path)
        if source_hash != item.get("source_sha256") or source_hash != item.get("after_sha256"):
            fail(f"SDK added source hash mismatch: {item['path']}")
        source_bytes = source_path.read_bytes()
        if item.get("compatibility_change") == "Added direct MLXLMCommon import and qualified Tokenizers.Tokenizer for pinned target dependencies.":
            original_bytes = source_bytes.replace(b"import MLXLMCommon\n", b"", 1)
            original_bytes = original_bytes.replace(
                b"public var tokenizer: (any Tokenizers.Tokenizer)?",
                b"public var tokenizer: (any Tokenizer)?",
                1,
            )
            if original_bytes == source_bytes or hashlib.sha256(original_bytes).hexdigest() != item.get("upstream_sha256"):
                fail(f"SDK added source compatibility transform mismatch: {item['path']}")
        elif source_hash != item.get("upstream_sha256"):
            fail(f"SDK added source differs from its pinned upstream hash: {item['path']}")
        target, _ = optional_checked_file(source, item["path"])
        added_actual_paths[item["path"]] = target
        added_actual_hashes.append(digest(target) if target else None)
        added_after_hashes.append(item["after_sha256"])

    head = run_git(source, "rev-parse", "HEAD")
    if head != revision:
        fail(f"SDK checkout HEAD mismatch: expected {revision}, found {head}")
    status = run_git(source, "status", "--porcelain", "--untracked-files=all")
    records = status_records(status)

    if actual_hashes == after_hashes and added_actual_hashes == added_after_hashes:
        expected_after_status = {(" M", item["path"]) for item in files}
        expected_after_status |= {("??", item["path"]) for item in added_files}
        if set(records) != expected_after_status or len(records) != len(expected_after_status):
            fail("SDK checkout dirty-path set does not exactly match the declared overlay paths")
        post_sha256 = {item["path"]: item["after_sha256"] for item in files + added_files}
        print(json.dumps({"state": "verified_after", "head": head, "patch_sha256": manifest["patch_sha256"], "post_sha256": post_sha256}, sort_keys=True))
        return
    if args.require_after:
        fail("SDK post-build source is not in the exact declared after-state")
    legacy_after = prior_overlay_after_hashes(manifest, files)
    is_legacy = (
        legacy_after is not None
        and actual_hashes == [legacy_after.get(item["path"], item["base_sha256"]) for item in files]
        and all(value is None for value in added_actual_hashes)
        and set(records) == {(" M", path) for path in LEGACY_OVERLAY_AFTER_SHA256}
        and len(records) == len(LEGACY_OVERLAY_AFTER_SHA256)
    )
    is_base = actual_hashes == base_hashes and all(value is None for value in added_actual_hashes) and not records
    if not is_legacy and not is_base:
        fail("SDK source files are neither the complete pinned base nor the complete declared after-state")
    base_blob_contents = {}
    for item in files:
        base_blob_contents[item["path"]] = git_blob_bytes(source, head, item)

    source_file_hashes = {item["path"]: actual_hashes[index] for index, item in enumerate(files)}
    publish_files = [item for item in files if source_file_hashes[item["path"]] != item["after_sha256"]]

    with tempfile.TemporaryDirectory(prefix="voiceflow-sdk-overlay-", dir=source.parent) as temporary:
        stage = Path(temporary)
        os.chmod(stage, 0o700)
        for item in files:
            relative = PurePosixPath(item["path"])
            destination = stage.joinpath(*relative.parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            destination.write_bytes(base_blob_contents[item["path"]])
        check = subprocess.run(["git", "apply", "--check", str(patch_path)], cwd=stage, text=True, capture_output=True)
        if check.returncode:
            fail(f"SDK overlay exact-context check failed: {check.stderr.strip()}")
        applied = subprocess.run(["git", "apply", str(patch_path)], cwd=stage, text=True, capture_output=True)
        if applied.returncode:
            fail(f"SDK overlay application failed: {applied.stderr.strip()}")
        for item in added_files:
            destination = stage.joinpath(*PurePosixPath(item["path"]).parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            shutil.copyfile(added_source_paths[item["path"]], destination)
        for item in files + added_files:
            if digest(stage.joinpath(*PurePosixPath(item["path"]).parts)) != item["after_sha256"]:
                fail(f"SDK overlay post-image hash mismatch: {item['path']}")

        originals = {
            item["path"]: (
                actual_paths[item["path"]][0].read_bytes(),
                stat.S_IMODE(actual_paths[item["path"]][1].st_mode),
            )
            for item in publish_files
        }
        replaced = []
        created_directories = []
        try:
            current_status = run_git(source, "status", "--porcelain", "--untracked-files=all")
            if current_status != status:
                fail("SDK checkout status changed immediately before overlay publish")
            for item in files:
                relative = item["path"]
                target, before = checked_file(source, relative)
                initial = actual_paths[relative][1]
                if (
                    (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
                    != (initial.st_dev, initial.st_ino, initial.st_size, initial.st_mtime_ns)
                    or digest(target) != source_file_hashes[relative]
                ):
                    fail(f"SDK source changed immediately before publish: {relative}")
            for item in publish_files:
                relative = item["path"]
                target, before = checked_file(source, relative)
                if digest(target) != source_file_hashes[relative]:
                    fail(f"SDK source changed immediately before publish: {relative}")
                replace_from_overlay(target, stage.joinpath(*PurePosixPath(relative).parts))
                replaced.append(("base", relative))
            for item in added_files:
                relative = item["path"]
                target = source.joinpath(*PurePosixPath(relative).parts)
                created_directories.extend(ensure_parent_directories(source, relative))
                if optional_checked_file(source, relative)[0] is not None:
                    fail(f"SDK added source target appeared immediately before publish: {relative}")
                replace_from_overlay(target, stage.joinpath(*PurePosixPath(relative).parts))
                replaced.append(("added", relative))
            post_hashes = [digest(actual_paths[item["path"]][0]) for item in files]
            post_added_hashes = [digest(source.joinpath(*PurePosixPath(item["path"]).parts)) for item in added_files]
            if post_hashes != after_hashes or post_added_hashes != added_after_hashes:
                fail("SDK overlay post-publish hashes do not match the declared after-state")
            status = run_git(source, "status", "--porcelain", "--untracked-files=all")
            expected_status = {(" M", item["path"]) for item in files}
            expected_status |= {("??", item["path"]) for item in added_files}
            if set(status_records(status)) != expected_status:
                fail("SDK checkout dirty-path set changed unexpectedly after overlay publish")
        except BaseException:
            for kind, relative in reversed(replaced):
                target = source.joinpath(*PurePosixPath(relative).parts)
                if kind == "added":
                    target.unlink(missing_ok=True)
                else:
                    original_bytes, original_mode = originals[relative]
                    replace_atomically(
                        target,
                        original_bytes,
                        "voiceflow-overlay-rollback",
                        original_mode,
                    )
            for directory in reversed(created_directories):
                try:
                    directory.rmdir()
                except OSError:
                    pass
            raise

    post_sha256 = {item["path"]: item["after_sha256"] for item in files + added_files}
    print(json.dumps({"state": "applied_and_verified", "head": head, "patch_sha256": manifest["patch_sha256"], "post_sha256": post_sha256}, sort_keys=True))


if __name__ == "__main__":
    main()
