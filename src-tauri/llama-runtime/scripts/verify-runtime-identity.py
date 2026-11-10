#!/usr/bin/env python3
"""Verify a staged or bundled llama.cpp helper against its signed identity."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path


RELEASE_TAG = "v0.9.0"
TAG_OBJECT_SHA1 = "c4261a74a59c5a719e8f73038e28633b08f2bea4"
SOURCE_COMMIT = "cd959bae99ff4bed22bef2604f45241c09bbb8c8"
MINIMUM_MACOS = "13.3"


def fail(message: str) -> None:
    raise SystemExit(f"llama.cpp runtime verification failed: {message}")


def run(*args: str, timeout: int = 10) -> str:
    try:
        result = subprocess.run(args, check=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, text=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError) as error:
        fail(f"could not run {Path(args[0]).name}: {error}")
    return result.stdout


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime-dir", required=True, type=Path)
    parser.add_argument("--expected-signing-mode", choices=("ad_hoc", "signed", "developer_id"), required=True)
    parser.add_argument("--expected-authority")
    parser.add_argument("--expected-team-id")
    args = parser.parse_args()

    runtime_dir = args.runtime_dir.resolve(strict=True)
    identity_path = runtime_dir / "BUILD-IDENTITY.json"
    binary_path = runtime_dir / "bin/llama-server"
    try:
        identity = json.loads(identity_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"build identity is missing or invalid: {error}")

    source = identity.get("source")
    artifact = identity.get("artifact")
    build = identity.get("build")
    signing = identity.get("signing")
    if (identity.get("schema_version") != 1 or identity.get("status") != "ready"
            or identity.get("runtime") != "llama.cpp"):
        fail("runtime status or schema is not supported")
    if (not isinstance(source, dict) or source.get("repository") != "https://github.com/ggml-org/llama.cpp"
            or source.get("release_tag") != RELEASE_TAG
            or source.get("annotated_tag_object_sha1") != TAG_OBJECT_SHA1
            or source.get("commit") != SOURCE_COMMIT or source.get("license") != "MIT"):
        fail("source identity does not match the pinned llama.cpp revision")
    if (not isinstance(artifact, dict) or artifact.get("path") != "bin/llama-server"
            or artifact.get("architecture") != "arm64"
            or artifact.get("minimum_macos") != MINIMUM_MACOS):
        fail("artifact architecture or minimum macOS version does not match the pin")
    if (not isinstance(build, dict) or build.get("configuration") != "Release"
            or build.get("metal") is not True or build.get("metal_embedded") is not True
            or build.get("static_native_libraries") is not True
            or build.get("openmp") is not False or build.get("openssl") is not False
            or build.get("web_ui") is not False):
        fail("build configuration does not match the reviewed Metal-only runtime")
    if not isinstance(signing, dict) or signing.get("mode") != args.expected_signing_mode:
        fail("signature mode does not match the requested package path")
    if not binary_path.is_file() or not binary_path.stat().st_size:
        fail("llama-server is missing or empty")

    signed_hash = hashlib.sha256(binary_path.read_bytes()).hexdigest()
    if artifact.get("sha256_signed") != signed_hash or artifact.get("size_bytes") != binary_path.stat().st_size:
        fail("signed helper bytes do not match BUILD-IDENTITY.json")
    if not re.fullmatch(r"[0-9a-f]{64}", str(artifact.get("sha256_pre_signing", ""))) \
            or not isinstance(artifact.get("size_bytes_pre_signing"), int):
        fail("pre-signing artifact identity is missing")

    run("codesign", "--verify", "--strict", str(binary_path))
    signature_info = run("codesign", "-dv", "--verbose=4", str(binary_path))
    signature_lines = signature_info.splitlines()
    authority = next((line.removeprefix("Authority=") for line in signature_lines
                      if line.startswith("Authority=")), None)
    team_identifier = next((line.removeprefix("TeamIdentifier=") for line in signature_lines
                            if line.startswith("TeamIdentifier=")), None)
    code_directory = next((line for line in signature_lines if "CodeDirectory" in line), "")
    hardened_runtime = "(runtime)" in code_directory
    ad_hoc_signature = "Signature=adhoc" in signature_lines or "(adhoc)" in code_directory
    if (signing.get("authority") != authority or signing.get("team_identifier") != team_identifier
            or signing.get("hardened_runtime") is not hardened_runtime):
        fail("BUILD-IDENTITY.json signature details do not match the code signature")
    if args.expected_signing_mode == "ad_hoc":
        if not ad_hoc_signature or authority is not None:
            fail("expected an ad-hoc code signature")
    elif ad_hoc_signature or not authority:
        fail("expected a certificate-signed helper")
    if args.expected_signing_mode == "developer_id":
        if not args.expected_authority or not args.expected_team_id:
            fail("Developer ID verification requires expected authority and team ID")
        if authority != args.expected_authority or team_identifier != args.expected_team_id:
            fail("helper signer authority or team ID does not match the notarization identity")
        if not authority.startswith("Developer ID Application:") or not hardened_runtime:
            fail("notarized helper must have a hardened Developer ID signature")
    elif args.expected_authority or args.expected_team_id:
        if authority != args.expected_authority or team_identifier != args.expected_team_id:
            fail("helper signer authority or team ID does not match the expected identity")
    strings = run("strings", "-a", str(binary_path), timeout=30)
    if "/Users/" in strings:
        fail("helper contains a developer-home path")
    architectures = run("lipo", "-archs", str(binary_path)).strip().split()
    if architectures != ["arm64"]:
        fail(f"unexpected architecture slices: {architectures}")
    build_info = run("vtool", "-arch", "arm64", "-show-build", str(binary_path))
    if not re.search(rf"^\s*minos {re.escape(MINIMUM_MACOS)}$", build_info, re.MULTILINE):
        fail("Mach-O minimum macOS version differs from the pin")
    version = run(str(binary_path), "--version")
    if f"commit {SOURCE_COMMIT[:7]}" not in version:
        fail("llama-server reports an unexpected source commit")

    print(f"Verified llama.cpp {RELEASE_TAG} ({SOURCE_COMMIT[:7]}), arm64 macOS {MINIMUM_MACOS}+; signing={args.expected_signing_mode}.")


if __name__ == "__main__":
    main()
