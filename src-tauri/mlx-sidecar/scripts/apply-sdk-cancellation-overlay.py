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
    expected_paths = {
        "Sources/MLXAudioCore/DSP.swift",
        "Sources/MLXAudioSTT/Models/Qwen3ASR/Qwen3ASR.swift",
        "Sources/MLXAudioSTT/Models/CohereTranscribe/CohereTranscribe.swift",
    }
    if not isinstance(files, list) or {item.get("path") for item in files} != expected_paths:
        fail("SDK overlay file allowlist mismatch")
    if len(files) != len(expected_paths):
        fail("SDK overlay file allowlist contains duplicates")

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

    head = run_git(source, "rev-parse", "HEAD")
    if head != revision:
        fail(f"SDK checkout HEAD mismatch: expected {revision}, found {head}")
    status = run_git(source, "status", "--porcelain", "--untracked-files=all")
    changed_paths = set()
    for line in status.splitlines():
        relative = line[3:]
        if " -> " in relative:
            relative = relative.split(" -> ", 1)[1]
        changed_paths.add(relative)
    allowed = expected_paths

    if actual_hashes == after_hashes:
        if changed_paths != allowed:
            fail("SDK checkout dirty-path set does not exactly match the three overlay files")
        print(json.dumps({"state": "verified_after", "head": head, "patch_sha256": manifest["patch_sha256"], "post_sha256": {item["path"]: item["after_sha256"] for item in files}}, sort_keys=True))
        return
    if args.require_after:
        fail("SDK post-build source is not in the exact declared after-state")
    if actual_hashes != base_hashes:
        fail("SDK source files are neither the complete pinned base nor the complete declared after-state")
    if changed_paths:
        fail("pinned SDK checkout contains an unexpected tracked or untracked change")
    for item in files:
        blob = subprocess.run(
            ["git", "-C", str(source), "hash-object", str(actual_paths[item["path"]][0])],
            text=True,
            capture_output=True,
            check=False,
        )
        if blob.returncode or blob.stdout.strip() != item.get("base_git_blob"):
            fail(f"SDK base Git blob mismatch: {item['path']}")

    with tempfile.TemporaryDirectory(prefix="voiceflow-sdk-overlay-", dir=source.parent) as temporary:
        stage = Path(temporary)
        os.chmod(stage, 0o700)
        for item in files:
            relative = PurePosixPath(item["path"])
            destination = stage.joinpath(*relative.parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            shutil.copyfile(actual_paths[item["path"]][0], destination)
        check = subprocess.run(["git", "apply", "--check", str(patch_path)], cwd=stage, text=True, capture_output=True)
        if check.returncode:
            fail(f"SDK overlay exact-context check failed: {check.stderr.strip()}")
        applied = subprocess.run(["git", "apply", str(patch_path)], cwd=stage, text=True, capture_output=True)
        if applied.returncode:
            fail(f"SDK overlay application failed: {applied.stderr.strip()}")
        for item in files:
            if digest(stage.joinpath(*PurePosixPath(item["path"]).parts)) != item["after_sha256"]:
                fail(f"SDK overlay post-image hash mismatch: {item['path']}")

        originals = {path: data[0].read_bytes() for path, data in actual_paths.items()}
        replaced = []
        try:
            for item in files:
                relative = item["path"]
                target, before = checked_file(source, relative)
                if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
                    actual_paths[relative][1].st_dev,
                    actual_paths[relative][1].st_ino,
                    actual_paths[relative][1].st_size,
                    actual_paths[relative][1].st_mtime_ns,
                ) or digest(target) != item["base_sha256"]:
                    fail(f"SDK source changed immediately before publish: {relative}")
                temporary_file = target.with_name(f".{target.name}.voiceflow-overlay-tmp")
                fd = os.open(temporary_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(fd, "wb") as output:
                    output.write(stage.joinpath(*PurePosixPath(relative).parts).read_bytes())
                    output.flush()
                    os.fsync(output.fileno())
                os.replace(temporary_file, target)
                replaced.append(relative)
        except BaseException:
            for relative in reversed(replaced):
                target = source.joinpath(*PurePosixPath(relative).parts)
                temporary_file = target.with_name(f".{target.name}.voiceflow-overlay-rollback")
                fd = os.open(temporary_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                with os.fdopen(fd, "wb") as output:
                    output.write(originals[relative])
                    output.flush()
                    os.fsync(output.fileno())
                os.replace(temporary_file, target)
            raise

    post_hashes = [digest(actual_paths[item["path"]][0]) for item in files]
    if post_hashes != after_hashes:
        fail("SDK source post-publish hashes do not match the declared after-state")
    status = run_git(source, "status", "--porcelain", "--untracked-files=all")
    if {line[3:].split(" -> ", 1)[-1] for line in status.splitlines()} != allowed:
        fail("SDK checkout dirty-path set changed unexpectedly after overlay publish")
    print(json.dumps({"state": "applied_and_verified", "head": head, "patch_sha256": manifest["patch_sha256"], "post_sha256": {item["path"]: item["after_sha256"] for item in files}}, sort_keys=True))


if __name__ == "__main__":
    main()
