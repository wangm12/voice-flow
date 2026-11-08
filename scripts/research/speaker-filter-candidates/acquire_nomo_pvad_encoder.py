#!/usr/bin/env python3
"""Acquire the pinned ERes2NetV2 enrollment checkpoint into a private folder.

Only downloads the one checkpoint used by Nomo-PVAD enrollment; it does not
use ModelScope's broad runtime downloader or write to a global model cache.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time
from urllib.request import Request, urlopen

MODEL_ID = "iic/speech_eres2netv2_sv_zh-cn_16k-common"
MODEL_REVISION = "v1.0.2"
FILE_NAME = "pretrained_eres2netv2.ckpt"
EXPECTED_BYTES = 71_768_231
EXPECTED_SHA256 = "0eb4057106b2573dd7b132cf0c36273ab29afd192c1610f80baa9c556dbb963c"
SOURCE = (
    "https://www.modelscope.cn/api/v1/models/"
    "iic/speech_eres2netv2_sv_zh-cn_16k-common/repo"
    "?Revision=v1.0.2&FilePath=pretrained_eres2netv2.ckpt"
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True,
                        help="Private destination folder for the checkpoint")
    args = parser.parse_args()
    destination = args.destination.expanduser().resolve()
    destination.mkdir(parents=True, exist_ok=True, mode=0o700)
    output = destination / FILE_NAME

    if output.exists():
        if output.is_symlink() or output.stat().st_size != EXPECTED_BYTES or sha256(output) != EXPECTED_SHA256:
            raise SystemExit(f"existing checkpoint failed pinned identity: {output}")
        print(json.dumps({"status": "already_verified", "path": str(output),
                          "bytes": EXPECTED_BYTES, "sha256": EXPECTED_SHA256}))
        return

    started = time.monotonic()
    request = Request(SOURCE, headers={"User-Agent": "VoiceFlow-private-filter-candidate/1"})
    fd, temp_name = tempfile.mkstemp(prefix=".eres2netv2-", suffix=".part", dir=destination)
    temp_path = Path(temp_name)
    os.chmod(temp_path, 0o600)
    try:
        digest = hashlib.sha256()
        size = 0
        with os.fdopen(fd, "wb") as sink, urlopen(request, timeout=60) as response:
            while block := response.read(1024 * 1024):
                size += len(block)
                if size > EXPECTED_BYTES:
                    raise RuntimeError("download exceeded the pinned file size")
                digest.update(block)
                sink.write(block)
            sink.flush()
            os.fsync(sink.fileno())
        if size != EXPECTED_BYTES or digest.hexdigest() != EXPECTED_SHA256:
            raise RuntimeError("download did not match the pinned file size and SHA-256")
        os.replace(temp_path, output)
        os.chmod(output, 0o600)
    except Exception:
        temp_path.unlink(missing_ok=True)
        raise

    receipt = {
        "artifact_type": "voiceflow_filter_candidate_encoder_acquisition",
        "schema_version": 1,
        "model_id": MODEL_ID,
        "revision": MODEL_REVISION,
        "file": FILE_NAME,
        "source": SOURCE,
        "license": "Apache-2.0 (official ModelScope model card)",
        "bytes": size,
        "sha256": digest.hexdigest(),
        "elapsed_seconds": round(time.monotonic() - started, 3),
        "downloaded_from_network": True,
    }
    receipt_path = destination / "acquisition.json"
    receipt_fd = os.open(receipt_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(receipt_fd, "w", encoding="utf-8") as stream:
        json.dump(receipt, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    print(json.dumps({"status": "verified", "path": str(output), "bytes": size,
                      "sha256": digest.hexdigest(),
                      "elapsed_seconds": receipt["elapsed_seconds"]}))


if __name__ == "__main__":
    main()
