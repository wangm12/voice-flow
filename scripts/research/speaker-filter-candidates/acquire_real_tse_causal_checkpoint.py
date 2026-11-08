#!/usr/bin/env python3
"""Range-fetch only the public REAL-TSE causal speaker-embedding checkpoint.

The official Drive URL is a ZIP containing four models. This script parses a
previously captured 64 KiB ZIP tail and retrieves only the selected member and
its YAML configuration with bounded HTTP Range requests; it never downloads
the complete archive or the other three models.

The checkpoint has no declared license in the official README or archive.
This script is for private, local research acquisition only.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import tempfile
import time
from urllib.request import Request, urlopen
import zlib

DRIVE_FILE_ID = "1M4UqK2A2EeHmQ0pCevYqBgaYn3RvklgC"
ARCHIVE_NAME = "real-tse-ckpt.zip"
ARCHIVE_BYTES = 1_048_798_335
SOURCE_URL = f"https://drive.google.com/file/d/{DRIVE_FILE_ID}/view"
DOWNLOAD_BASE = f"https://drive.usercontent.google.com/download?id={DRIVE_FILE_ID}&export=download"
REPO_COMMIT = "2a540977a348fbaa92e623210505430e2cec608d"
WESPEAKER_COMMIT = "8f53b6485d9f88a207bd17e7f8dba899495ec794"
SELECTED = {
    "spk_emb_causal_100/config.yaml": "config.yaml",
    "spk_emb_causal_100/avg_model.pt": "avg_model.pt",
}
MAX_COMPRESSED_BYTES = 237_000_000
READ_BLOCK = 1024 * 1024


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_path(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(READ_BLOCK), b""):
            digest.update(block)
    return digest.hexdigest()


def read_selected_entries(tail_path: Path) -> tuple[dict, dict[str, dict]]:
    if tail_path.is_symlink() or tail_path.stat().st_size != 65_536:
        raise ValueError("ZIP metadata tail must be a regular 65,536-byte file")
    tail = tail_path.read_bytes()
    eocd = tail.rfind(b"PK\x05\x06")
    if eocd < 0 or len(tail) - eocd < 22:
        raise ValueError("ZIP end-of-central-directory not found in bounded tail")
    sig, disk, cd_disk, n_disk, n_total, cd_size, cd_abs, comment_len = struct.unpack_from(
        "<4s4H2LH", tail, eocd
    )
    if sig != b"PK\x05\x06" or disk != 0 or cd_disk != 0 or n_disk != n_total:
        raise ValueError("unexpected multi-disk/ZIP64 archive structure")
    if ARCHIVE_BYTES - len(tail) + eocd + 22 + comment_len != ARCHIVE_BYTES:
        raise ValueError("ZIP tail is not aligned to the pinned Drive archive size")
    cd_start = eocd - cd_size
    if cd_start < 0 or cd_start + cd_size != eocd or cd_abs != ARCHIVE_BYTES - len(tail) + cd_start:
        raise ValueError("central directory offsets do not match the captured tail")

    all_entries: dict[str, dict] = {}
    pos, stop = cd_start, cd_start + cd_size
    while pos < stop:
        fields = struct.unpack_from("<4s6H3L5H2L", tail, pos)
        sig, made, needed, flag, method, mt, md, crc, csize, usize, nlen, elen, clen, disk_no, int_attr, ext_attr, local = fields
        if sig != b"PK\x01\x02" or disk_no != 0:
            raise ValueError("unexpected central-directory record")
        name_start = pos + 46
        name = tail[name_start:name_start + nlen].decode("utf-8")
        if name in all_entries:
            raise ValueError(f"duplicate ZIP member: {name}")
        all_entries[name] = {
            "name": name,
            "compressed_bytes": csize,
            "uncompressed_bytes": usize,
            "compression_method": method,
            "crc32": crc,
            "local_header_offset": local,
            "general_purpose_flags": flag,
        }
        pos = name_start + nlen + elen + clen
    if pos != stop or n_total != len(all_entries):
        raise ValueError("central-directory entry count/size mismatch")
    selected = {}
    for name in SELECTED:
        entry = all_entries.get(name)
        if not entry:
            raise ValueError(f"pinned archive lacks selected member {name}")
        if entry["compression_method"] != 8 or entry["general_purpose_flags"] & 1:
            raise ValueError(f"unsupported compression/encryption on {name}")
        if entry["compressed_bytes"] > MAX_COMPRESSED_BYTES:
            raise ValueError(f"selected member exceeds the bounded acquisition size: {name}")
        selected[name] = entry
    meta = {
        "drive_file_id": DRIVE_FILE_ID,
        "archive_name": ARCHIVE_NAME,
        "archive_bytes": ARCHIVE_BYTES,
        "archive_sha256": None,
        "archive_sha256_note": "not downloaded or published; only selected member ranges were fetched",
        "tail_bytes": len(tail),
        "tail_sha256": sha256_bytes(tail),
        "zip_entry_count": n_total,
        "central_directory_bytes": cd_size,
        "all_entries": list(all_entries.values()),
    }
    return meta, selected


def get_confirmation_url() -> str:
    request = Request(DOWNLOAD_BASE, headers={"Range": "bytes=0-0", "User-Agent": "VoiceFlow-private-filter-research/1"})
    with urlopen(request, timeout=30) as response:
        content_type = response.headers.get("Content-Type", "")
        length = int(response.headers.get("Content-Length", "0") or 0)
        if "text/html" not in content_type or length > 8_192:
            raise RuntimeError("Google Drive did not return the expected small confirmation page")
        page = response.read(8_192).decode("utf-8", "replace")
    uuid = re.search(r'name="uuid" value="([^"]+)"', page)
    if not uuid:
        raise RuntimeError("Google Drive confirmation UUID not found")
    return f"{DOWNLOAD_BASE}&confirm=t&uuid={uuid.group(1)}"


def range_response(url: str, start: int, end: int):
    request = Request(url, headers={
        "Range": f"bytes={start}-{end}",
        "User-Agent": "VoiceFlow-private-filter-research/1",
    })
    response = urlopen(request, timeout=90)
    expected_range = f"bytes {start}-{end}/{ARCHIVE_BYTES}"
    if response.status != 206 or response.headers.get("Content-Range") != expected_range:
        response.close()
        raise RuntimeError(f"Drive did not honor bounded Range {expected_range}")
    if int(response.headers.get("Content-Length", "-1")) != end - start + 1:
        response.close()
        raise RuntimeError("Drive range Content-Length mismatch")
    return response


def fetch_entry(url: str, entry: dict, destination: Path) -> dict:
    local_offset = entry["local_header_offset"]
    with range_response(url, local_offset, local_offset + 29) as header_response:
        header = header_response.read(30)
    sig, version, flag, method, mt, md, crc, csize, usize, nlen, elen = struct.unpack(
        "<4s5H3L2H", header
    )
    if sig != b"PK\x03\x04" or flag & 1 or method != 8:
        raise ValueError(f"unexpected local ZIP header for {entry['name']}")
    if (crc, csize, usize) != (entry["crc32"], entry["compressed_bytes"], entry["uncompressed_bytes"]):
        raise ValueError(f"local/central ZIP metadata mismatch for {entry['name']}")
    name_end = local_offset + 30 + nlen
    with range_response(url, local_offset + 30, name_end - 1) as name_response:
        local_name = name_response.read(nlen)
    if local_name.decode("utf-8") != entry["name"]:
        raise ValueError(f"local ZIP member name mismatch for {entry['name']}")
    payload_start = local_offset + 30 + nlen + elen
    payload_end = payload_start + csize - 1
    started = time.monotonic()
    fd, temporary_name = tempfile.mkstemp(prefix=f".{destination.name}-", suffix=".part", dir=destination.parent)
    temp = Path(temporary_name)
    os.chmod(temp, 0o600)
    received = expanded = 0
    digest = hashlib.sha256()
    crc_value = 0
    decompressor = zlib.decompressobj(-15)
    try:
        with os.fdopen(fd, "wb") as sink, range_response(url, payload_start, payload_end) as response:
            while block := response.read(READ_BLOCK):
                received += len(block)
                if received > csize:
                    raise RuntimeError("Drive range returned too many compressed bytes")
                output = decompressor.decompress(block)
                if output:
                    expanded += len(output)
                    if expanded > usize:
                        raise RuntimeError("expanded ZIP member exceeds central-directory size")
                    sink.write(output)
                    digest.update(output)
                    crc_value = zlib.crc32(output, crc_value)
            tail = decompressor.flush()
            if tail:
                expanded += len(tail)
                sink.write(tail)
                digest.update(tail)
                crc_value = zlib.crc32(tail, crc_value)
            sink.flush()
            os.fsync(sink.fileno())
        if received != csize or not decompressor.eof or decompressor.unused_data:
            raise RuntimeError("selected ZIP member compressed stream is incomplete or has trailing data")
        if expanded != usize or (crc_value & 0xFFFFFFFF) != entry["crc32"]:
            raise RuntimeError("selected ZIP member expanded size or CRC32 mismatch")
        if destination.exists() or destination.is_symlink():
            raise FileExistsError(destination)
        os.replace(temp, destination)
        os.chmod(destination, 0o600)
    except Exception:
        temp.unlink(missing_ok=True)
        raise
    return {
        "source_entry": entry["name"],
        "destination": str(destination),
        "compressed_bytes_range_fetched": received,
        "expanded_bytes": expanded,
        "crc32": f"{crc_value & 0xFFFFFFFF:08x}",
        "sha256": digest.hexdigest(),
        "elapsed_seconds": round(time.monotonic() - started, 3),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata-tail", type=Path, required=True,
                        help="previously captured final 64 KiB of the official ZIP")
    parser.add_argument("--repo-dir", type=Path, required=True,
                        help="private WeSep checkout at the pinned commit")
    parser.add_argument("--wespeaker-dir", type=Path, required=True,
                        help="private WeSpeaker checkout at the pinned commit")
    parser.add_argument("--destination", type=Path, required=True,
                        help="new private output directory")
    args = parser.parse_args()
    import subprocess
    repo_dir = args.repo_dir.resolve(strict=True)
    wespeaker_dir = args.wespeaker_dir.resolve(strict=True)
    for path, expected, label in (
        (repo_dir, REPO_COMMIT, "WeSep"),
        (wespeaker_dir, WESPEAKER_COMMIT, "WeSpeaker"),
    ):
        actual = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"],
                                check=True, text=True, capture_output=True).stdout.strip()
        if actual != expected:
            raise ValueError(f"{label} pin mismatch: {actual}")
    destination = args.destination.expanduser().resolve()
    if destination.exists():
        raise FileExistsError(destination)
    destination.mkdir(parents=True, mode=0o700)
    os.chmod(destination, 0o700)
    metadata, selected = read_selected_entries(args.metadata_tail.resolve(strict=True))
    bundle = destination / "spk_emb_causal_100"
    bundle.mkdir(mode=0o700)
    confirmed_url = get_confirmation_url()
    files = []
    for name, filename in SELECTED.items():
        files.append(fetch_entry(confirmed_url, selected[name], bundle / filename))
    receipt = {
        "artifact_type": "voiceflow_real_tse_causal_private_checkpoint_acquisition",
        "schema_version": 1,
        "status": "complete",
        "source_repository": "https://github.com/REAL-TSE/wesep-real-tse",
        "source_repository_commit": REPO_COMMIT,
        "source_repository_license": "No root LICENSE file or GitHub license record found; some source files declare Apache-2.0 headers",
        "speaker_encoder_repository": "https://github.com/wenet-e2e/wespeaker",
        "speaker_encoder_commit": WESPEAKER_COMMIT,
        "model_name": "spk_emb_causal_100",
        "model_license": "not stated in linked README or checkpoint archive; private research use only, redistribution rights unverified",
        "source_archive_url": SOURCE_URL,
        "source_archive_name": ARCHIVE_NAME,
        "source_archive_bytes": ARCHIVE_BYTES,
        "source_archive_sha256": None,
        "source_archive_metadata": metadata,
        "selected_members": files,
        "total_compressed_bytes_fetched": sum(item["compressed_bytes_range_fetched"] for item in files),
        "other_checkpoint_members_fetched": 0,
        "whole_zip_downloaded": False,
        "model_license_warning": "Do not redistribute or integrate into a marketed build until model-weight rights are resolved.",
    }
    receipt_path = destination / "acquisition.json"
    fd = os.open(receipt_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        json.dump(receipt, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    print(json.dumps({"status": "complete", "receipt": str(receipt_path),
                      "downloaded_bytes": receipt["total_compressed_bytes_fetched"],
                      "members": files}, ensure_ascii=False))


if __name__ == "__main__":
    main()
