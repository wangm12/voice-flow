#!/usr/bin/env python3
"""Explicit, bounded public corpus preparation. Never contacts an ASR provider.

Source annotations are drafts. Rust evaluation/contracts.rs owns evaluation
artifacts; this tool emits unreviewed candidates or imports explicit human labels.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from collections.abc import Mapping
from contextvars import ContextVar
from dataclasses import dataclass, replace
import hashlib
import fcntl
import io
import json
import math
import os
from pathlib import Path
from types import MappingProxyType
import re
import secrets
import stat
import struct
import sys
import time
import urllib.parse
import urllib.request
import wave

# The legacy corpus/admission contracts above remain unchanged. Project role
# facts and reservations below are a separate, metadata-only authority owned by
# this same public-corpus boundary. They do not grant media/rights admission.
PARTITION_KINDS = ("source", "speaker", "recording", "session", "template", "augmentation")
PARTITION_ROLES = ("dev", "test", "train", "quarantine", "unassigned")
PARTITION_MAX_SHARDS = 200
PARTITION_MAX_CASES_PER_SHARD = 200
PARTITION_MAX_CASES = PARTITION_MAX_SHARDS * PARTITION_MAX_CASES_PER_SHARD
PARTITION_BYTES = 16 * 1024 * 1024
PARTITION_MAX_KNOWN_FAMILY_REFERENCES = 65536
RETAINED_PARTITION_WRITERS = []

REPO = Path(__file__).resolve().parents[2]
MIB = 1024 * 1024
RETAINED_OUTPUTS = []
RETAINED_ROOTS = []
OUTPUT_ROOT = ContextVar("public_corpus_output_root", default=None)


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def external(path):
    path = Path(path)
    require(path.is_absolute(), "explicit absolute external path required")
    result = path.resolve()
    require(not result.is_relative_to(REPO), "dataset artifacts must remain outside repository")
    return result


def strict_object(value, keys, name):
    require(isinstance(value, Mapping) and set(value) == set(keys), f"strict {name} fields required")
    return value


def sha(data):
    return hashlib.sha256(data).hexdigest()


@dataclass(frozen=True)
class InputSnapshot:
    path: Path
    raw: bytes
    digest: str
    source_identity: tuple
    parsed: object = None

    @classmethod
    def capture(cls, path, cap):
        require(type(cap) is int and cap > 0, "positive input byte cap")
        with Path(path).open("rb") as stream:
            before = os.fstat(stream.fileno())
            require(stat.S_ISREG(before.st_mode) and before.st_size <= cap, "input size budget exceeded")
            raw = stream.read(cap + 1)
            require(len(raw) <= cap, "input grew beyond byte cap")
            after = os.fstat(stream.fileno())
            identity = lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns, value.st_ctime_ns)
            require(identity(before) == identity(after), "input changed during bounded capture")
        return cls(Path(path), raw, sha(raw), identity(before))

    @classmethod
    def json(cls, path, cap=4 * MIB):
        snapshot = cls.capture(path, cap)
        def pairs(items):
            value = {}
            for key, item in items:
                require(key not in value, "duplicate JSON field")
                value[key] = item
            return value
        def freeze(value):
            if isinstance(value, dict):
                return MappingProxyType({key: freeze(item) for key, item in value.items()})
            if isinstance(value, list):
                return tuple(freeze(item) for item in value)
            return value
        return replace(snapshot, parsed=freeze(json.loads(snapshot.raw, object_pairs_hook=pairs)))

    def parquet(self):
        import pyarrow as pa
        import pyarrow.parquet as pq
        return pq.ParquetFile(pa.BufferReader(self.raw))


class OutputCleanupError(OSError):
    """Ownership remains with the writer until its failed cleanup is resolved."""


class OutputRoot:
    """One held namespace owns a batch and interprets caller-path applicability."""
    def __init__(self, path, requested_path=None):
        require(OUTPUT_ROOT.get() is None, "nested output root scope")
        self.path = Path(path)
        self.requested_path = Path(requested_path) if requested_path is not None else self.path
        self.path.mkdir(parents=True, exist_ok=True)
        require(self.path.resolve() == self.path, "output root must be canonical at open")
        fd = os.open(self.path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        self.parents = {Path(): (fd, self.identity(os.fstat(fd)))}
        self.root_identity = self.parents[Path()][1]
        self.resources = []
        self.applicability = "current"
        self.token = None
        self.closed = False
        self.assert_applicable()

    @staticmethod
    def identity(info):
        return info.st_dev, info.st_ino

    @staticmethod
    def version(info):
        return info.st_size, info.st_mtime_ns, info.st_ctime_ns

    def retain(self, reason):
        if self not in RETAINED_ROOTS:
            RETAINED_ROOTS.append(self)
        resources = [{"destination": str(resource.destination), "destination_identity": resource.destination_identity, "stage_identity": getattr(resource, "stage_identity", None), "committed": resource.committed} for resource in self.resources]
        raise OutputCleanupError(f"retain output root selected={self.requested_path} canonical_at_open={self.path} captured_root_identity={self.root_identity} held_parent_count={len(self.parents)} applicability={self.applicability} owned_resources={len(self.resources)} retained_resources={json.dumps(resources)}: {reason}")

    def assert_applicable(self):
        if self.applicability != "current":
            self.retain("caller-path applicability remains unknown; explicit recovery required")
        try:
            require(self.identity(os.stat(self.requested_path)) == self.parents[Path()][1], "selected output root moved/replaced")
            for relative, (_, identity) in self.parents.items():
                observed = os.stat(self.path / relative, follow_symlinks=False)
                require(stat.S_ISDIR(observed.st_mode) and self.identity(observed) == identity, "output parent moved/replaced")
        except (OSError, ValueError) as error:
            self.applicability = "unknown"
            self.retain(str(error))

    def parent_for(self, destination):
        self.assert_applicable()
        relative = destination.parent.relative_to(self.path)
        require(all(part not in (".", "..", "") for part in relative.parts), "output escapes bound root")
        current = Path()
        for part in relative.parts:
            next_parent = current / part
            if next_parent not in self.parents:
                require(len(self.parents) < 64, "output parent descriptor budget")
                ancestor_fd = self.parents[current][0]
                try:
                    os.mkdir(part, dir_fd=ancestor_fd)
                except FileExistsError:
                    pass
                fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=ancestor_fd)
                self.parents[next_parent] = (fd, self.identity(os.fstat(fd)))
            current = next_parent
        self.assert_applicable()
        return self.parents[relative][0]

    def register(self, resource):
        require(len(self.resources) < 256, "owned output descriptor budget")
        self.resources.append(resource)

    def forget(self, resource):
        if resource in self.resources:
            self.resources.remove(resource)

    def finish(self):
        self.assert_applicable()
        for resource in self.resources:
            if not resource.committed:
                self.retain("uncommitted owned output; batch not applicable")
            try:
                observed = os.stat(resource.destination.name, dir_fd=resource.parent_fd, follow_symlinks=False)
                held = os.fstat(resource.published_stream.fileno())
                require(self.identity(observed) == resource.destination_identity and self.version(held) == resource.destination_version, "published output replaced/edited since commit")
            except (OSError, ValueError) as error:
                self.applicability = "unknown"
                self.retain(str(error))
        self.assert_applicable()
        try:
            self.release_handles()
        except OSError as error:
            self.applicability = "unknown"
            self.retain(f"root handle release incomplete: {error}")

    def release_handles(self):
        for resource in self.resources:
            resource.release_handles()
        self.resources.clear()
        for relative, (fd, _) in list(self.parents.items()):
            os.close(fd)
            del self.parents[relative]
        self.closed = True
        if self in RETAINED_ROOTS:
            RETAINED_ROOTS.remove(self)

    def __enter__(self):
        require(OUTPUT_ROOT.get() is None, "nested output root scope")
        self.token = OUTPUT_ROOT.set(self)
        return self

    def __exit__(self, error_type, error, traceback):
        try:
            if error is None:
                self.finish()
            elif self.applicability != "current" or self.resources:
                self.retain(f"batch incomplete: {error}")
            else:
                self.assert_applicable()
                self.release_handles()
        finally:
            OUTPUT_ROOT.reset(self.token)


class OwnedOutput:
    """Private anonymous staging, then exclusive descriptor-bound publication.

    Only this owner writes its random 0700 namespace. No other producer may use
    that namespace; it is not a security boundary against a same-user process
    deliberately tampering with private descriptors. Public .partial names are
    never linked/unlinked. Publication is exclusive, not whole-file atomic:
    incomplete owned targets are retained on failure, never removed by name.
    """
    def __init__(self, destination):
        self.destination = destination
        self.stream = None
        self.published_stream = None
        self.parent_fd = None
        self.namespace_fd = None
        self.namespace = None
        self.staging_fd = None
        self.publication_fd = None
        self.committed = False
        self.destination_identity = None
        self.root = OUTPUT_ROOT.get()
        self.standalone = self.root is None
        if self.standalone:
            self.root = OutputRoot(destination.parent)
        self.parent_fd = self.root.parent_for(destination)
        self.root.register(self)
        try:
            # Bound random namespace attempts; exclusive mkdir establishes owner.
            for _ in range(16):
                name = ".voiceflow-stage-" + secrets.token_hex(16)
                try:
                    os.mkdir(name, mode=0o700, dir_fd=self.parent_fd)
                    self.namespace = destination.parent / name
                    break
                except FileExistsError:
                    continue
            require(self.namespace is not None, "private staging nonce collision budget")
            self.namespace_fd = os.open(self.namespace.name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=self.parent_fd)
            self.namespace_identity = self.file_identity(os.fstat(self.namespace_fd))
            fd = os.open("payload", os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.namespace_fd)
            self.staging_fd = fd
            self.stage_identity = self.file_identity(os.fstat(fd))
            self.stream = os.fdopen(fd, "w+b")
            self.staging_fd = None
            # Within this newly created exclusive namespace, make the descriptor
            # anonymous before transport or any caller/fault callback runs.
            os.unlink("payload", dir_fd=self.namespace_fd)
        except BaseException as error:
            if self.namespace is None:
                self.root.forget(self)
                self.parent_fd = None
                if self.standalone:
                    self.root.finish()
            else:
                self.retain(f"private staging construction incomplete: {error}")
            raise

    @staticmethod
    def file_identity(info):
        return info.st_dev, info.st_ino

    def retain(self, reason):
        if self not in RETAINED_OUTPUTS:
            RETAINED_OUTPUTS.append(self)
        raise OutputCleanupError(f"retain owned output namespace={self.namespace} stage_identity={getattr(self, 'stage_identity', None)} destination={self.destination} destination_identity={self.destination_identity} committed={self.committed}: {reason}")

    def release_handles(self):
        for attribute in ("stream", "published_stream"):
            stream = getattr(self, attribute)
            if stream is not None:
                try:
                    stream.close()
                finally:
                    if stream.closed:
                        setattr(self, attribute, None)
        for attribute in ("staging_fd", "publication_fd", "namespace_fd"):
            fd = getattr(self, attribute)
            if fd is not None:
                os.close(fd)
                setattr(self, attribute, None)
        self.parent_fd = None

    def cleanup(self):
        try:
            self.root.assert_applicable()
        except OutputCleanupError as error:
            self.retain(f"output root applicability unknown; anchored owned resources preserved: {error}")
        if (self.published_stream is not None or self.publication_fd is not None) and not self.committed:
            self.retain("publication incomplete or identity unresolved; public target never deleted; descriptor retained")
        try:
            if self.namespace is not None:
                # Namespace-only cleanup. No file iteration, recursive removal,
                # shared payload names, or check-then-unlink/link of file names.
                require(self.namespace_fd is not None and hasattr(self, "namespace_identity"), "private namespace identity unavailable; explicit recovery required")
                current = os.stat(self.namespace.name, dir_fd=self.parent_fd, follow_symlinks=False)
                require(self.file_identity(current) == self.namespace_identity, "private namespace moved/replaced; ownership location unresolved")
                os.rmdir(self.namespace.name, dir_fd=self.parent_fd)
                self.namespace = None
            if self.published_stream is None and self.publication_fd is None:
                self.release_handles()
                self.root.forget(self)
            else:
                # Keep publication proof until aggregate receipt/root completion.
                if self.stream is not None:
                    self.stream.close()
                    self.stream = None
                if self.namespace_fd is not None:
                    os.close(self.namespace_fd)
                    self.namespace_fd = None
        except (OSError, ValueError) as error:
            self.retain(f"cleanup incomplete: {error}")
        if self in RETAINED_OUTPUTS:
            RETAINED_OUTPUTS.remove(self)
        if self.standalone:
            self.root.finish()

    def commit(self, expected_sha256, expected_bytes):
        require(type(expected_bytes) is int and 0 <= expected_bytes <= 512 * MIB, "output byte budget")
        self.stream.flush()
        self.stream.seek(0)
        snapshot = self.stream.read(expected_bytes + 1)
        require(len(snapshot) == expected_bytes and sha(snapshot) == expected_sha256, "owned staged bytes changed before publication")
        self.root.assert_applicable()
        # Capture validated bytes once; a later edit to the anonymous descriptor
        # cannot become the publication source. No staging pathname is reopened.
        fd = os.open(self.destination.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.parent_fd)
        self.publication_fd = fd
        self.destination_identity = self.file_identity(os.fstat(fd))
        self.published_stream = os.fdopen(fd, "wb")
        self.publication_fd = None
        self.published_stream.write(snapshot)
        self.published_stream.flush()
        os.fsync(fd)
        self.root.assert_applicable()
        observed = os.stat(self.destination.name, dir_fd=self.parent_fd, follow_symlinks=False)
        require(self.file_identity(observed) == self.destination_identity, "publication path replaced; retain owned descriptor and preserve foreign path")
        self.destination_version = self.root.version(os.fstat(fd))
        self.committed = True
        self.cleanup()

    def abort(self, error):
        if self.parent_fd is not None and self not in RETAINED_OUTPUTS:
            try:
                self.cleanup()
            except OutputCleanupError as cleanup_error:
                raise cleanup_error from error
        raise error


def write_once(path, data):
    owned = OwnedOutput(path)
    try:
        owned.stream.write(data)
        owned.commit(sha(data), len(data))
    except BaseException as error:
        owned.abort(error)
    return sha(data)


def public_url(value):
    require(isinstance(value, str) and not any(ord(c) <= 32 for c in value), "public HTTPS source URL required")
    url = urllib.parse.urlsplit(value)
    require(url.scheme == "https" and url.hostname and not url.username and not url.password and not url.query and not url.fragment and url.port in (None, 443), "public HTTPS source URL required")
    parts = []
    for encoded in url.path.split("/")[1:]:
        # Decode once at the URI owner; forbid escaped separators, traversal and
        # double-encoding ambiguity. Preserve the requested URL in the receipt.
        require(re.search(r"%(?![0-9a-fA-F]{2})", encoded) is None, "invalid URL escape")
        part = urllib.parse.unquote(encoded, errors="strict")
        require(part and part not in (".", "..") and not any(c in part for c in ("/", "\\", "%")) and not any(ord(c) <= 32 for c in part), "unsafe URL path segment")
        parts.append(part)
    return url, parts


def bind_source_identity(plan, entry, parsed_source=None):
    source_url, source_parts = parsed_source if parsed_source is not None else public_url(plan["source"])
    url, parts = public_url(entry["url"])
    if source_url.hostname != "huggingface.co":
        require(url.hostname != "huggingface.co", "HF request requires a matching HF source")
        return {"kind": "observed_http_resource", "requested_url": entry["url"], "revision": None, "declared_observation_label": plan["revision"], "byte_identity": "actual_sha256_after_acquisition"}
    require(len(source_parts) == 3 and source_parts[0] == "datasets", "HF source must name a dataset repository")
    repo = source_parts[1:3]
    require(all(re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in repo), "HF repository grammar")
    revision = plan["revision"]
    require(isinstance(revision, str) and re.fullmatch(r"[0-9a-f]{40}", revision), "HF source requires immutable commit revision")
    require(url.hostname == "huggingface.co", "HF source/request authority mismatch")
    if len(parts) >= 6 and parts[:1] == ["datasets"] and parts[3] == "resolve":
        request_repo, request_revision, remote_path = parts[1:3], parts[4], parts[5:]
        kind = "hf_resolve"
        require("/".join(remote_path) == entry["path"], "HF resolve source file/path mismatch")
    elif len(parts) >= 6 and parts[:2] == ["api", "datasets"] and parts[4] == "tree":
        request_repo, request_revision, remote_path = parts[2:4], parts[5], parts[6:]
        kind = "hf_tree"
    else:
        raise ValueError("unsupported HF source request grammar")
    require(request_repo == repo and request_revision == revision, "HF source/request repository or revision mismatch")
    return {"kind": kind, "repository": "/".join(repo), "revision": revision, "remote_path": "/".join(remote_path), "local_path": entry["path"], "requested_url": entry["url"]}


def json_bytes(value):
    def serialize(item):
        if isinstance(item, Mapping):
            return dict(item)
        raise TypeError("unsupported artifact value")
    return (json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False, default=serialize) + "\n").encode()


def fetch(args):
    plan_path, root = external(args.plan), external(args.out_dir)
    plan_snapshot = InputSnapshot.json(plan_path)
    plan = plan_snapshot.parsed
    strict_object(plan, ["artifact_type", "schema_version", "source", "revision", "license", "attribution", "files"], "acquisition plan")
    require(plan["artifact_type"] == "public_acquisition_plan" and type(plan["schema_version"]) is int and plan["schema_version"] == 1, "plan version")
    for field in ("license", "attribution"):
        require(isinstance(plan[field], str) and plan[field].strip() and len(plan[field].encode()) <= 4096, "bounded source license/attribution required")
    require(plan["revision"] is None or isinstance(plan["revision"], str) and plan["revision"].strip() and len(plan["revision"].encode()) <= 256, "bounded revision or explicit unknown required")
    parsed_source = public_url(plan["source"])
    require(isinstance(plan["files"], (list, tuple)) and 0 < len(plan["files"]) <= 20, "select 1–20 files")
    seen = set()
    total_expected = 0
    bindings = []
    for entry in plan["files"]:
        strict_object(entry, ["path", "url", "publisher_sha256", "max_bytes"], "source file")
        require(isinstance(entry["path"], str) and entry["path"] and "\\" not in entry["path"] and all(p not in ("", "..", ".") for p in entry["path"].split("/")), "source path traversal")
        require(entry["path"] not in seen, "duplicate source path")
        seen.add(entry["path"])
        bindings.append(bind_source_identity(plan, entry, parsed_source))
        require(type(entry["max_bytes"]) is int and 0 < entry["max_bytes"] <= args.max_file_bytes, "source file exceeds byte cap")
        require(entry["publisher_sha256"] is None or re.fullmatch("[0-9a-f]{64}", entry["publisher_sha256"]), "publisher digest")
        total_expected += entry["max_bytes"]
    require(total_expected <= args.max_total_bytes, "plan exceeds total byte cap")
    require(not (root / "acquisition.json").exists(), "immutable acquisition already exists")
    if args.dry_run:
        print(json.dumps({"dry_run": True, "files": [e["path"] for e in plan["files"]], "source_bindings": bindings, "maximum_bytes": total_expected, "network_requests": 0}))
        return
    with OutputRoot(root, requested_path=args.out_dir):
        start = time.monotonic()
        total = 0
        receipts = []
        for entry, binding in zip(plan["files"], bindings):
            destination = root / entry["path"]
            require(not destination.exists() and not destination.is_symlink(), "download output collision")
            size = 0
            digest = hashlib.sha256()
            elapsed = time.monotonic() - start
            require(elapsed < args.deadline_seconds, "download deadline")
            owned = OwnedOutput(destination)
            try:
                request = urllib.request.Request(entry["url"], headers={"User-Agent": "VoiceFlow-public-corpus/1"})
                with urllib.request.urlopen(request, timeout=min(30, args.deadline_seconds - elapsed)) as response:
                    content_length = response.headers.get("Content-Length")
                    require(content_length is None or int(content_length) <= entry["max_bytes"], "declared source exceeds byte cap")
                    while True:
                        require(time.monotonic() - start < args.deadline_seconds, "download deadline")
                        block = response.read(min(MIB, entry["max_bytes"] - size + 1))
                        if not block:
                            break
                        size += len(block)
                        total += len(block)
                        require(size <= entry["max_bytes"] and total <= args.max_total_bytes, "download byte cap exceeded")
                        owned.stream.write(block)
                        digest.update(block)
                actual = digest.hexdigest()
                require(entry["publisher_sha256"] is None or actual == entry["publisher_sha256"], "publisher checksum mismatch")
                owned.commit(actual, size)
            except BaseException as error:
                owned.abort(error)
            receipts.append({**entry, "source_identity": binding, "actual_bytes": size, "actual_sha256": actual, "publisher_digest_matched": None if entry["publisher_sha256"] is None else True})
            print(f"acquired {entry['path']}: {size} bytes {actual}", flush=True)
        immutable_revision = plan["revision"] if all(b["kind"].startswith("hf_") for b in bindings) else None
        write_once(root / "acquisition.json", json_bytes({"artifact_type": "public_acquisition_receipt", "schema_version": 1, "plan_sha256": plan_snapshot.digest, "source": plan["source"], "revision": immutable_revision, "declared_observation_label": None if immutable_revision else plan["revision"], "license": plan["license"], "attribution": plan["attribution"], "files": receipts, "elapsed_seconds": time.monotonic() - start, "download_bytes": total, "caps": {"file_bytes": args.max_file_bytes, "total_bytes": args.max_total_bytes, "deadline_seconds": args.deadline_seconds}, "review_status": "source_only_project_labels_pending"}))


def parquet_rows(snapshot, args):
    require(len(snapshot.raw) <= args.max_file_bytes, "parquet file budget")
    parquet = snapshot.parquet()
    require(parquet.metadata.num_rows <= args.max_rows, "parquet row budget")
    for index in range(parquet.metadata.num_row_groups):
        require(parquet.metadata.row_group(index).total_byte_size <= args.max_row_group_bytes, "parquet decoded row-group budget")
    # Bound decoded rows; parquet may internally decode one bounded row group.
    for batch in parquet.iter_batches(batch_size=1, use_threads=False):
        yield batch.to_pylist()[0]


def wav_identity(data):
    require(44 < len(data) <= 25 * MIB, "embedded WAV byte budget")
    require(data[:4] == b"RIFF" and data[8:12] == b"WAVE" and struct.unpack_from("<I", data, 4)[0] + 8 == len(data), "complete RIFF WAV required")
    cursor, counts = 12, Counter()
    while cursor < len(data):
        require(cursor + 8 <= len(data), "truncated WAV chunk")
        size = struct.unpack_from("<I", data, cursor + 4)[0]
        counts[data[cursor:cursor + 4]] += 1
        cursor += 8 + size + size % 2
        require(cursor <= len(data), "truncated WAV chunk body")
    require(counts[b"fmt "] == counts[b"data"] == 1, "single WAV format/data required")
    with wave.open(io.BytesIO(data), "rb") as reader:
        require(reader.getnchannels() == 1 and reader.getsampwidth() == 2 and reader.getframerate() == 16000 and reader.getcomptype() == "NONE", "source must already be 16k mono PCM16; unsupported conversion requires separate explicit recipe")
        count = reader.getnframes()
        require(0 < count <= 16000 * 600, "WAV duration budget")
        pcm = reader.readframes(count)
        require(len(pcm) == count * 2, "complete WAV samples required")
    return {"audio_sha256": sha(data), "pcm_sha256": sha(pcm), "sample_count": count, "sample_rate": 16000, "channels": 1, "bits_per_sample": 16, "duration_seconds": count / 16000}


ASCEND_FIELDS = {"id", "path", "audio", "transcription", "duration", "language", "original_speaker_id", "session_id", "topic"}
ASCEND_NAME = re.compile(r"ses(?P<session>\d+)_spk(?P<speaker>\d+)(?:_(?P<recording_variant>\d+))?_L(?P<line>\d+)_(?P<start>\d+(?:\.\d+)?)_(?P<duration>\d+(?:\.\d+)?)\.wav")


def ascend_row(row, split, ordinal, receipt_entry):
    strict_object(row, ASCEND_FIELDS, "ASCEND parquet row")
    strict_object(row["audio"], ["bytes", "path"], "embedded audio")
    require(row["language"] in ("zh", "en", "mixed"), "unknown source language")
    require(type(row["original_speaker_id"]) is int and row["original_speaker_id"] > 0 and type(row["session_id"]) is int and row["session_id"] > 0, "missing publisher speaker/session")
    require(isinstance(row["transcription"], str) and len(row["transcription"].encode()) <= 16384, "source transcript byte bound")
    name = row["audio"]["path"]
    require(isinstance(name, str) and Path(name).name == name and Path(row["path"]).name == name, "source paths disagree")
    match = ASCEND_NAME.fullmatch(name)
    require(match is not None, "unknown ASCEND path grammar")
    parsed = match.groupdict()
    require(int(parsed["speaker"]) == row["original_speaker_id"] and int(parsed["session"]) == row["session_id"], "source filename/metadata identity disagreement")
    data = row["audio"]["bytes"]
    require(isinstance(data, bytes), "embedded WAV missing")
    audio = wav_identity(data)
    require(math.isfinite(row["duration"]) and abs(row["duration"] - audio["duration_seconds"]) <= 0.001 and abs(float(parsed["duration"]) - audio["duration_seconds"]) <= 0.001, "source duration/sample mismatch")
    record = {"case_id": f"ascend-{split}-{row['id']}", "source_split": split, "project_split": "reserved_source_test_unassigned" if split == "test" else "development_candidate_pool", "source_parquet": receipt_entry["path"], "source_parquet_sha256": receipt_entry["actual_sha256"], "row_ordinal": ordinal, "source_id": row["id"], "source_path": row["path"], "embedded_audio_path": name, "language": row["language"], "original_speaker_id": row["original_speaker_id"], "session_id": row["session_id"], "topic": row["topic"], "publisher_duration_seconds": row["duration"], "publisher_duration_minus_pcm_seconds": row["duration"] - audio["duration_seconds"], "filename_components_observed": parsed, "raw_recording_group": f"ascend:spk{row['original_speaker_id']}:ses{row['session_id']}:variant{parsed['recording_variant'] or 'omitted'}", "conversation_partner_id": None, "conversation_pair_id": None, "global_conversation_session_id": None, "template_family_id": None, "source_annotation": {"text": row["transcription"], "text_sha256": sha(row["transcription"].encode()), "origin": "publisher_manual_asr_annotation", "project_review_status": "pending", "contains_unk": "[UNK]" in row["transcription"]}, "audio": audio, "conversion": {"recipe": "identity-preserve-v1", "changes": [], "reason": "source already 16000 Hz mono PCM16 WAV; preserve complete source container and PCM", "source_sample_interval": [0, audio["sample_count"]]}, "conditions": {"microphone": "publisher paper: RODE SmartLav+; per-clip device not independently observed", "noise": "publisher controlled classroom; per-clip acoustic review pending", "speaking_style": "publisher spontaneous conversation"}}
    return record, data


def prepare_ascend(args):
    root, out = external(args.source_dir), external(args.out_dir)
    require(not out.exists(), "choose a fresh immutable preparation directory")
    receipt_snapshot = InputSnapshot.json(root / "acquisition.json")
    receipt = receipt_snapshot.parsed
    tool_snapshot = InputSnapshot.capture(Path(__file__), MIB)
    require(receipt["source"] == "https://huggingface.co/datasets/CAiRE/ASCEND" and receipt["revision"] == "737e9800ae31be9932ba8464c80366559bd28424", "supported pinned ASCEND source required")
    entries = {e["path"]: e for e in receipt["files"]}
    rows, selected_bytes, schemas, parquet_snapshots = [], {}, {}, {}
    source_bytes = 0
    start = time.monotonic()
    total_audio_bytes = 0
    seen_ids = set()
    for split in ("validation", "test"):
        name = f"main/{split}-00000-of-00001.parquet"
        entry = entries[name]
        path = root / name
        snapshot = InputSnapshot.capture(path, min(args.max_file_bytes, args.max_total_bytes - source_bytes))
        source_bytes += len(snapshot.raw)
        require(snapshot.digest == entry["actual_sha256"] == entry["publisher_sha256"], "acquired parquet identity changed")
        parquet_snapshots[split] = snapshot
        schemas[split] = str(snapshot.parquet().schema_arrow)
        for ordinal, row in enumerate(parquet_rows(snapshot, args)):
            require(time.monotonic() - start <= args.deadline_seconds, "preparation deadline")
            record, data = ascend_row(row, split, ordinal, entry)
            require(record["case_id"] not in seen_ids, "duplicate source case")
            seen_ids.add(record["case_id"])
            total_audio_bytes += len(data)
            require(total_audio_bytes <= args.max_total_bytes, "decoded audio total budget")
            rows.append(record)
    require(len(rows) <= args.max_rows, "combined source row budget")
    pools = defaultdict(list)
    for record in rows:
        if record["source_split"] == "validation" and 2 <= record["audio"]["duration_seconds"] <= 15 and record["source_annotation"]["text"].strip() and not record["source_annotation"]["contains_unk"]:
            pools[record["language"]].append(record)
    selected = []
    for language in ("zh", "en", "mixed"):
        speakers = defaultdict(list)
        for record in sorted(pools[language], key=lambda r: sha(r["case_id"].encode())):
            speakers[record["original_speaker_id"]].append(record)
        require(sum(map(len, speakers.values())) >= 20, f"insufficient {language} eligible validation speech")
        picked = []
        while len(picked) < 20:
            for speaker in sorted(speakers):
                if speakers[speaker] and len(picked) < 20:
                    picked.append(speakers[speaker].pop(0))
        selected.extend(picked)
    chosen = {r["case_id"] for r in selected}
    require(len(chosen) == 60 and len({r["audio"]["pcm_sha256"] for r in selected}) == 60, "candidate duplicate PCM")
    # Replay the same bounded raw snapshot, never the mutable source path.
    selected_records = {r["case_id"]: r for r in selected}
    for ordinal, row in enumerate(parquet_rows(parquet_snapshots["validation"], args)):
        identity = f"ascend-validation-{row['id']}"
        if identity in chosen:
            record, data = ascend_row(row, "validation", ordinal, entries["main/validation-00000-of-00001.parquet"])
            require(record == selected_records[identity], "selected replay source/audio identity mismatch")
            selected_bytes[identity] = data
    require(set(selected_bytes) == chosen, "selected replay coverage mismatch")
    groups = {}
    for key in ("original_speaker_id", "raw_recording_group"):
        by_split = {split: {r[key] for r in rows if r["source_split"] == split} for split in ("validation", "test")}
        groups[key] = {"validation": sorted(by_split["validation"]), "test": sorted(by_split["test"]), "intersection": sorted(by_split["validation"] & by_split["test"])}
    duplicates = {}
    for key in ("pcm_sha256", "audio_sha256"):
        mapping = defaultdict(list)
        for record in rows:
            mapping[record["audio"][key]].append(record["case_id"])
        duplicates[key] = {digest: ids for digest, ids in mapping.items() if len(ids) > 1}
    audit = {"artifact_type": "public_source_preparation_audit", "schema_version": 1, "source_revision": receipt["revision"], "acquisition_sha256": receipt_snapshot.digest, "schemas": schemas, "source_row_counts": dict(Counter(r["source_split"] for r in rows)), "language_counts": {s: dict(Counter(r["language"] for r in rows if r["source_split"] == s)) for s in ("validation", "test")}, "source_group_intersections": groups, "duplicate_audio": duplicates, "unk_counts": {s: sum(r["source_annotation"]["contains_unk"] for r in rows if r["source_split"] == s) for s in ("validation", "test")}, "selected": {"counts": dict(Counter(r["language"] for r in selected)), "speaker_counts": dict(Counter(r["original_speaker_id"] for r in selected)), "duration_seconds": sum(r["audio"]["duration_seconds"] for r in selected), "source_split": "validation", "project_split": "dev", "selection": "20/language, duration 2–15 seconds, nonempty/no UNK source draft, case-ID SHA order round-robin publisher speaker", "selected_before_model_outputs": True}, "reserved_source_test": {"rows": sum(r["source_split"] == "test" for r in rows), "project_split": "unassigned_reserved", "audio_extracted": False}, "unknowns": ["conversation partners and globally unique conversation sessions unavailable in acquired fields", "template families not annotated; spontaneous topics do not certify independent templates", "six-speaker coverage: acquired subset has five distinct publisher speakers; candidates have three", "two test-only speaker IDs observed, not certified conversation-partner holdout", "independent human verbatim/final/protected/variants and intent review pending", "music/real ambient negative coverage missing", "foundation-model pretraining exposure unknown"], "conversion": "all audited WAV/PCM kept source-compatible; selected WAV bytes preserved exactly", "elapsed_seconds": time.monotonic() - start, "decoded_source_audio_bytes": total_audio_bytes, "costs": {"whole_task_tokens": None, "process_family_cpu_seconds": None, "process_family_peak_memory_bytes": None, "paid_spend": 0}, "independent_verification": "pending"}
    if args.dry_run:
        print(json.dumps({"dry_run": True, "source_rows": len(rows), "candidate_counts": audit["selected"]["counts"], "output_files_written": 0}))
        return
    out.mkdir(parents=True)
    with OutputRoot(out, requested_path=args.out_dir):
        write_once(out / "preparation-tool.py", tool_snapshot.raw)
        write_once(out / "source-acquisition.json", receipt_snapshot.raw)
        audit["input_snapshots"] = {key: {"path": str(value.path), "sha256": value.digest, "bytes": len(value.raw), "captured_source_identity": value.source_identity} for key, value in parquet_snapshots.items()}
        audit["preparation_tool_sha256"] = tool_snapshot.digest
        write_once(out / "source-metadata.jsonl", b"".join(json_bytes(r).replace(b"\n", b" ").rstrip() + b"\n" for r in rows))
        for record in selected:
            record["project_split"] = "dev"
            record["adjacent_wav"] = record["case_id"] + ".wav"
            write_once(out / record["adjacent_wav"], selected_bytes[record["case_id"]])
        candidates_digest = write_once(out / "candidates.json", json_bytes({"artifact_type": "public_speech_candidates", "schema_version": 1, "source_receipt": str(root / "acquisition.json"), "source_receipt_sha256": receipt_snapshot.digest, "source": receipt["source"], "revision": receipt["revision"], "license": receipt["license"], "attribution": receipt["attribution"], "review_status": "pending", "cases": selected}))
        draft = {"schema_version": 1, "provenance": {"kind": "human_recorded", "note": "ASCEND licensed human speech. Source ASR annotations only; expected_final below is a draft copy, not reviewed cleanup gold. Project listening/intent/entity review pending. Do not score or train.", "reviewed": False}, "cases": [{"id": r["case_id"], "audio": r["adjacent_wav"], "reference": r["source_annotation"]["text"], "expected_final": r["source_annotation"]["text"], "scenario": "publisher spontaneous conversation source draft; project dictation intent pending", "family": "general", "focus_kind": "unknown", "cleanup": "off", "protected": [], "reference_variants": []} for r in selected]}
        write_once(out / "speech-draft.json", json_bytes(draft))
        queue = {"artifact_type": "public_label_review_queue", "schema_version": 1, "candidates_sha256": candidates_digest, "cases": [{"case_id": r["case_id"], "audio_sha256": r["audio"]["audio_sha256"], "pcm_sha256": r["audio"]["pcm_sha256"], "status": "pending", "source_annotation_text": r["source_annotation"]["text"], "transcript_verbatim": None, "expected_final": None, "protected": None, "reference_variants": None, "authorized_changes": None, "intent_evidence": None, "reviewer": None, "reviewed_at": None, "annotation_revision": None, "adjudication": None, "speaker_id": f"ascend:spk{r['original_speaker_id']}", "source_session_id": None, "template_family_id": None, "split": "dev", "rights_review_id": None} for r in selected]}
        write_once(out / "review-queue.json", json_bytes(queue))
        negative_index, generators = {"artifact_type": "non_speech_index", "schema_version": 1, "provenance": {"kind": "synthetic", "note": "Deterministic generated silence/xorshift noise fixtures, never ambient/music recordings. Listening/source review pending.", "reviewed": False}, "cases": []}, []
        for index, (category, seconds) in enumerate([(kind, seconds) for kind in ("silence", "noise") for seconds in (3, 5, 8)]):
            name = f"negative-{category}-{seconds}s.wav"
            pcm = bytearray()
            state = 17 + index
            for _ in range(16000 * seconds):
                if category == "noise":
                    state ^= (state << 13) & 0xffffffff
                    state ^= state >> 17
                    state ^= (state << 5) & 0xffffffff
                    state &= 0xffffffff
                    sample = (state % 4097) - 2048
                else:
                    sample = 0
                pcm.extend(struct.pack("<h", sample))
            output = io.BytesIO()
            with wave.open(output, "wb") as writer:
                writer.setnchannels(1)
                writer.setsampwidth(2)
                writer.setframerate(16000)
                writer.writeframes(pcm)
            data = output.getvalue()
            write_once(out / name, data)
            negative_index["cases"].append({"case_id": name[:-4], "adjacent_wav": name, "audio_sha256": sha(data), "category": category, "source_review_id": "generated-fixture-review-pending"})
            generators.append({"case_id": name[:-4], "source_kind": "synthetic", "category": category, "generator": "zero PCM" if category == "silence" else "xorshift32, shifts 13/17/5, modulo 4097 minus 2048", "seed": None if category == "silence" else 17 + index, "audio": wav_identity(data), "project_review_status": "pending"})
        write_once(out / "negative-draft.json", json_bytes(negative_index))
        write_once(out / "negative-source.json", json_bytes({"artifact_type": "synthetic_negative_sources", "schema_version": 1, "generator_source_sha256": tool_snapshot.digest, "cases": generators, "music_coverage": "missing; generated noise and silence are not music"}))
        write_once(out / "preparation-audit.json", json_bytes(audit))
        write_once(out / "sidecar-readiness.json", json_bytes({"artifact_type": "sidecar_readiness", "ready": False, "reason": "source_session/template/conversation grouping and independent human labels unresolved; no fabricated string identities emitted", "candidate_count": 60, "next_step": "fill explicit review labels after listening and resolve group authority, then import into fresh external directory"}))
    print(json.dumps({"prepared": str(out), "speech_candidates": 60, "synthetic_negatives": 6, "reviewed": False, "provider_attempts": 0}))


def inspect_yodas(args):
    root, out = external(args.source_dir), external(args.out_dir)
    require(not out.exists(), "fresh metadata output directory required")
    receipt_snapshot = InputSnapshot.json(root / "acquisition.json")
    receipt = receipt_snapshot.parsed
    tool_snapshot = InputSnapshot.capture(Path(__file__), MIB)
    require(receipt["source"] == "https://huggingface.co/datasets/espnet/yodas3" and receipt["revision"] == "81d802f45a1c1e1f5331260484dacf1fbea81981", "supported pinned YODAS metadata required")
    summary, excerpts = [], []
    start = time.monotonic()
    for entry in receipt["files"]:
        require(re.fullmatch(r"data/(en|zh)/metadata/\d{4}\.parquet", entry["path"]), "only explicit en/zh metadata files allowed")
        path = root / entry["path"]
        snapshot = InputSnapshot.capture(path, args.max_file_bytes)
        require(snapshot.digest == entry["actual_sha256"] == entry["publisher_sha256"], "metadata digest changed")
        count, languages, locales, channels, statuses = 0, Counter(), Counter(), Counter(), Counter()
        for row in parquet_rows(snapshot, args):
            require(time.monotonic() - start <= args.deadline_seconds, "metadata deadline")
            strict_object(row, ["id", "shard", "transcript", "translation_en", "transcript_lang", "locale_lang", "length", "audio_bitrate", "max_freq_hz", "estimated_bandwidth_hz", "channels", "n_distinct_channels"], "YODAS metadata row")
            require(re.fullmatch(r"[0-9a-f]{32}", row["id"]) and row["shard"] == Path(entry["path"]).stem, "YODAS shard/id grammar")
            require(type(row["length"]) is float and math.isfinite(row["length"]) and row["length"] > 0, "duration must be known finite")
            count += 1
            languages[str(row["transcript_lang"])] += 1
            locales[str(row["locale_lang"])] += 1
            channels[str(row["channels"])] += 1
            status = "absent" if row["transcript"] is None else "present_unreviewed"
            statuses[status] += 1
            segment_count, text_chars, words_present, invalid_segments = 0, 0, 0, 0
            caption_parse_status = "absent" if row["transcript"] is None else "parsed"
            if row["transcript"] is not None and len(row["transcript"].encode()) > 2 * MIB:
                caption_parse_status = "not_parsed_caption_exceeds_2MiB_budget"
                segment_count = text_chars = words_present = invalid_segments = None
            if row["transcript"] is not None and caption_parse_status == "parsed":
                require(isinstance(row["transcript"], str), "caption string required")
                segments = json.loads(row["transcript"])
                require(isinstance(segments, list) and len(segments) <= 50000, "caption segment budget")
                for segment in segments:
                    require(isinstance(segment, dict) and set(segment) <= {"start", "duration", "text", "words"} and {"start", "duration", "text"} <= set(segment), "caption schema")
                    require(isinstance(segment["text"], str), "caption text")
                    segment_count += 1
                    text_chars += len(segment["text"])
                    words_present += "words" in segment
                    if not (type(segment["start"]) is int and type(segment["duration"]) is int and segment["start"] >= 0 and segment["duration"] >= 0 and segment["start"] + segment["duration"] <= row["length"] * 1000 + 1000):
                        invalid_segments += 1
            summary_row = {"source_path": entry["path"], "source_sha256": entry["actual_sha256"], "row_ordinal": count - 1, "encrypted_recording_id": row["id"], "speaker_id": None, "project_split": "unassigned_exploratory", "shard": row["shard"], "transcript_lang": row["transcript_lang"], "locale_lang": row["locale_lang"], "length": row["length"], "channels": row["channels"], "n_distinct_channels": row["n_distinct_channels"], "max_freq_hz": row["max_freq_hz"], "estimated_bandwidth_hz": row["estimated_bandwidth_hz"], "source_caption_status": status, "caption_parse_status": caption_parse_status, "source_caption_sha256": None if row["transcript"] is None else sha(row["transcript"].encode()), "source_translation_en_sha256": None if row["translation_en"] is None else sha(row["translation_en"].encode()), "segment_count": segment_count, "text_characters": text_chars, "segments_with_words": words_present, "flagged_segment_count": invalid_segments, "audio_downloaded": False, "project_label_review": "pending"}
            # Metadata facts, not raw captions or unbounded media. Raw selected parquets remain retained.
            nonfinite = [key for key, value in summary_row.items() if isinstance(value, float) and not math.isfinite(value)]
            for key in nonfinite:
                summary_row[key] = None
            summary_row["nonfinite_source_fields"] = nonfinite
            excerpts.append(summary_row)
        summary.append({"path": entry["path"], "rows_examined": count, "transcript_languages": dict(languages), "locale_languages": dict(locales), "channel_counts": dict(channels), "caption_status": dict(statuses)})
    require(len(excerpts) <= args.max_rows, "combined metadata row budget")
    timing = {"flagged_segment_count": sum(row["flagged_segment_count"] or 0 for row in excerpts), "rows_with_flagged_segments": sum((row["flagged_segment_count"] or 0) > 0 for row in excerpts), "rows_not_examined": sum(row["flagged_segment_count"] is None for row in excerpts), "rule": "invalid/noninteger/negative start or duration, or segment end exceeds declared recording length + 1000 ms; metadata observation, not audio alignment truth"}
    report = {"artifact_type": "yodas_bounded_metadata_observation", "schema_version": 2, "revision": receipt["revision"], "license": receipt["license"], "attribution": receipt["attribution"], "acquisition_sha256": receipt_snapshot.digest, "files": summary, "timing_observations": timing, "audio_acquisition": {"state": "not_downloaded", "reason": "metadata-only declared scope; paired audio tars approximately 8GB exceed first-stage file cap. Select explicit members/ranges and a new byte/time cap before audio acquisition."}, "speaker_holdout": "uncertified: encrypted recording IDs/channels do not identify speakers", "captions": "weak source drafts; English translations are separate from original-language transcripts; word timestamps are not human-review certification", "independent_verification": "pending"}
    if args.dry_run:
        print(json.dumps({"dry_run": True, "rows": len(excerpts), "audio_downloaded": False, "output_files_written": 0}))
        return
    out.mkdir(parents=True)
    with OutputRoot(out, requested_path=args.out_dir):
        write_once(out / "metadata-observations.jsonl", b"".join(json.dumps(r, ensure_ascii=False, allow_nan=False).encode() + b"\n" for r in excerpts))
        write_once(out / "preparation-tool.py", tool_snapshot.raw)
        report["preparation_tool_sha256"] = tool_snapshot.digest
        write_once(out / "metadata-summary.json", json_bytes(report))
    print(json.dumps({"metadata_rows": len(excerpts), "audio_downloaded": False, "reviewed_labels": 0}))


def import_review(args):
    candidate_path, label_path, out = external(args.candidates), external(args.labels), external(args.out_dir)
    require(not out.exists(), "fresh immutable import directory required")
    candidate_snapshot = InputSnapshot.json(candidate_path)
    candidates = candidate_snapshot.parsed
    require(candidates.get("artifact_type") == "public_speech_candidates" and candidates.get("schema_version") == 1 and candidates.get("source") == "https://huggingface.co/datasets/CAiRE/ASCEND" and candidates.get("revision") == "737e9800ae31be9932ba8464c80366559bd28424", "supported candidate source/version required")
    require(isinstance(candidates.get("cases"), (list, tuple)) and 0 < len(candidates["cases"]) <= 200, "bounded candidate list required")
    label_snapshot = InputSnapshot.json(label_path)
    labels = label_snapshot.parsed
    strict_object(labels, ["artifact_type", "schema_version", "candidates_sha256", "rights_review_id", "group_review_id", "cases"], "review import")
    require(labels["artifact_type"] == "reviewed_public_speech_labels" and labels["schema_version"] == 1 and labels["candidates_sha256"] == candidate_snapshot.digest, "review/candidate identity mismatch")
    require(isinstance(labels["cases"], (list, tuple)) and 0 < len(labels["cases"]) <= 200, "reviewed import 1–200 cases")
    source_cases = {r["case_id"]: r for r in candidates["cases"]}
    require(len(source_cases) == len(candidates["cases"]), "duplicate candidate identity")
    partition_decision = partition_for_review_import(args, candidate_snapshot, [row["case_id"] for row in labels["cases"]])
    prepared, sidecars, bytes_to_copy, seen = [], [], {}, set()
    for value in (labels["rights_review_id"], labels["group_review_id"]):
        require(isinstance(value, str) and 0 < len(value.encode()) <= 256 and "pending" not in value.lower() and "unknown" not in value.lower(), "explicit rights/group review identity required")
    for label in labels["cases"]:
        strict_object(label, ["case_id", "audio_sha256", "pcm_sha256", "status", "transcript_verbatim", "expected_final", "protected", "reference_variants", "authorized_changes", "intent_evidence", "reviewer", "reviewed_at", "annotation_revision", "adjudication", "speaker_id", "source_session_id", "template_family_id", "split"], "reviewed label")
        identity = label["case_id"]
        require(identity in source_cases and identity not in seen, "unknown/duplicate reviewed case")
        seen.add(identity)
        source = source_cases[identity]
        require(label["status"] == "reviewed" and label["adjudication"] == "resolved" and label["split"] == "dev", "pilot requires explicitly reviewed/resolved development labels")
        for field in ("transcript_verbatim", "expected_final", "intent_evidence", "reviewer", "reviewed_at", "annotation_revision", "speaker_id", "source_session_id", "template_family_id"):
            limit = 16384 if field in ("transcript_verbatim", "expected_final", "intent_evidence") else 256
            value = label[field]
            require(isinstance(value, str) and value.strip() and len(value.encode()) <= limit, f"reviewed {field} required")
        require(label["speaker_id"] == f"ascend:spk{source['original_speaker_id']}", "speaker must preserve publisher identity")
        for field in ("source_session_id", "template_family_id"):
            require("unknown" not in label[field].lower() and "pending" not in label[field].lower(), "unresolved grouping cannot produce a strict sidecar")
        for field, cap in (("protected", 128), ("reference_variants", 16), ("authorized_changes", 128)):
            require(isinstance(label[field], (list, tuple)) and len(label[field]) <= cap and all(isinstance(v, str) and v.strip() and len(v.encode()) <= 16384 for v in label[field]), f"bounded reviewed {field} required")
        audio_path = candidate_path.parent / source["adjacent_wav"]
        require(audio_path.resolve().parent == candidate_path.parent, "adjacent selected WAV required")
        remaining = 512 * MIB - sum(map(len, bytes_to_copy.values()))
        audio_snapshot = InputSnapshot.capture(audio_path, min(25 * MIB, remaining))
        data = audio_snapshot.raw
        audio = wav_identity(data)
        require(audio["audio_sha256"] == label["audio_sha256"] == source["audio"]["audio_sha256"] and audio["pcm_sha256"] == label["pcm_sha256"] == source["audio"]["pcm_sha256"], "reviewed WAV/PCM changed")
        bytes_to_copy[source["adjacent_wav"]] = data
        require(sum(map(len, bytes_to_copy.values())) <= 512 * MIB, "import audio total budget")
        prepared.append({"id": identity, "audio": source["adjacent_wav"], "reference": label["transcript_verbatim"], "expected_final": label["expected_final"], "scenario": "explicit reviewed public conversation; dictation intent in retained labels", "family": "general", "focus_kind": "unknown", "cleanup": "off", "protected": label["protected"], "reference_variants": label["reference_variants"]})
        sidecars.append({"case_id": identity, "manifest_sha256": None, "audio_sha256": audio["audio_sha256"], "pcm_sha256": audio["pcm_sha256"], "source_kind": "human_recorded", "rights_review_id": labels["rights_review_id"], "speaker_id": label["speaker_id"], "source_session_id": label["source_session_id"], "template_family_id": label["template_family_id"], "split": "dev", "language": source["language"], "conditions": source["conditions"], "reference_sha256": sha(label["transcript_verbatim"].encode()), "annotation_revision": label["annotation_revision"]})
    manifest = {"schema_version": 1, "provenance": {"kind": "human_recorded", "note": f"Explicit public labels import. Rights review {labels['rights_review_id']}; grouping review {labels['group_review_id']}. ASCEND source attribution retained in source-provenance.json. File replay only.", "reviewed": True}, "cases": prepared}
    data = json_bytes(manifest)
    for row in sidecars:
        row["manifest_sha256"] = sha(data)
    side = {"artifact_type": "corpus_sidecar", "schema_version": 1, "corpus_id": "ascend-reviewed-development-v1", "manifests": [{"relative_path": "speech.json", "sha256": sha(data)}], "cases": sidecars}
    require(len(data) <= MIB and len(json_bytes(side)) <= MIB, "Rust JSON input budget")
    if args.dry_run:
        print(json.dumps({"dry_run": True, "reviewed_import_cases": len(prepared), "output_files_written": 0, "provider_attempts": 0,
            "project_partition": partition_decision if partition_decision is not None else {"certainty": "unknown", "applicability": "not_requested", "admission": "not_granted"}}))
        return
    out.mkdir(parents=True)
    with OutputRoot(out, requested_path=args.out_dir):
        for name, content in bytes_to_copy.items():
            write_once(out / name, content)
        write_once(out / "speech.json", data)
        write_once(out / "corpus-sidecar.json", json_bytes(side))
        write_once(out / "reviewed-labels.json", label_snapshot.raw)
        write_once(out / "candidate-source.json", candidate_snapshot.raw)
        if partition_decision is not None:
            write_once(out / "project-partition-decision.json", partition_encode(partition_decision))
        write_once(out / "source-provenance.json", json_bytes({"candidates_path": str(candidate_snapshot.path), "candidates_sha256": candidate_snapshot.digest, "candidates_source_identity": candidate_snapshot.source_identity, "labels_path": str(label_snapshot.path), "labels_sha256": label_snapshot.digest, "labels_source_identity": label_snapshot.source_identity, "source": candidates["source"], "revision": candidates["revision"], "license": candidates["license"], "attribution": candidates["attribution"], "conversion": "source-compatible WAV bytes preserved", "rust_validation": "pending: sole contract owner must validate import before provider/model runs"}))
    print(json.dumps({"imported": len(prepared), "rust_validation": "pending", "provider_attempts": 0}))


class PartitionError(ValueError):
    """Expected boundary failure with retained partial facts, never 'clean'."""
    def __init__(self, code, detail, partial=None):
        self.code, self.detail, self.partial = code, detail, partial or {}
        super().__init__(detail)

    def fact(self):
        return {"kind": "error", "code": self.code, "detail": self.detail,
                "partial": self.partial}


def partition_need(condition, code, detail, partial=None):
    if not condition:
        raise PartitionError(code, detail, partial)


def partition_plain(value):
    # InputSnapshot owns JSON parsing/freezing. Normalize its tuple/Mapping
    # representation once here, before any partition consumer inspects facts.
    if isinstance(value, Mapping):
        return {key: partition_plain(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [partition_plain(item) for item in value]
    return value


def partition_fields(value, fields, label):
    partition_need(isinstance(value, dict) and set(value) == set(fields),
                   "schema", "strict " + label + " fields required")


def partition_text(value, label, limit=4096):
    partition_need(isinstance(value, str) and bool(value.strip())
                   and len(value.encode()) <= limit, "schema", "bounded " + label + " required")
    return value


def partition_evidence(value):
    partition_fields(value, ("path", "sha256", "locator"), "partition evidence")
    for field in ("path", "locator"):
        partition_text(value[field], field)
    partition_need(isinstance(value["sha256"], str)
                   and re.fullmatch(r"[0-9a-f]{64}", value["sha256"]),
                   "identity", "exact evidence SHA256 required")
    return value


def partition_family(value):
    partition_fields(value, ("state", "namespace", "value", "basis", "reason", "evidence"), "family fact")
    partition_need(value["state"] in ("known", "unknown", "not_applicable"), "schema", "family certainty required")
    partition_need(value["basis"] in ("publisher", "producer_derived", "explicit_declaration"), "schema", "family basis required")
    partition_text(value["reason"], "family reason")
    partition_evidence(value["evidence"])
    if value["state"] == "known":
        partition_text(value["namespace"], "family namespace")
        partition_text(value["value"], "family identity")
    else:
        partition_need(value["namespace"] is None and value["value"] is None,
                       "schema", "unknown/inapplicable family has no fabricated identity")
    return value


def partition_source(value):
    partition_fields(value, ("dataset_id", "revision", "publisher_split", "source_id"), "source identity")
    for field in value:
        partition_text(value[field], "source " + field)
    return value


def partition_identity(row):
    source = row["source"]
    return tuple(source[field] for field in ("dataset_id", "revision", "publisher_split", "source_id"))


def partition_case(value):
    partition_fields(value, ("case_id", "source", "families", "project", "evidence"), "partition case")
    partition_text(value["case_id"], "case ID", 256)
    partition_source(value["source"])
    partition_fields(value["families"], PARTITION_KINDS, "family coverage")
    for kind in PARTITION_KINDS:
        partition_family(value["families"][kind])
    speaker = value["families"]["speaker"]
    if speaker["state"] == "known":
        partition_need(speaker["namespace"] == value["source"]["dataset_id"] + "@" + value["source"]["revision"],
                       "revision_mismatch", "publisher speaker namespace must bind the exact selected dataset revision")
    partition_fields(value["project"], ("role", "state", "reason"), "project role observation")
    partition_need(value["project"]["role"] in PARTITION_ROLES, "schema", "explicit project role required")
    partition_need(value["project"]["state"] in ("declared", "observed_exposure", "proposed_only"),
                   "schema", "declared/observed/proposed role state required")
    partition_text(value["project"]["reason"], "role provenance reason")
    partition_evidence(value["evidence"])
    return value


@dataclass(frozen=True)
class PartitionShard:
    manifest_id: str
    evidence: dict
    cases: tuple


def parse_partition_shard(value):
    value = partition_plain(value)
    partition_fields(value, ("artifact_type", "schema_version", "manifest_id", "evidence", "cases"), "partition fact shard")
    partition_need(value["artifact_type"] == "project_partition_fact_shard" and type(value["schema_version"]) is int
                   and value["schema_version"] == 1, "schema", "partition fact shard version")
    partition_text(value["manifest_id"], "manifest ID", 256)
    partition_evidence(value["evidence"])
    partition_need(isinstance(value["cases"], list) and 0 < len(value["cases"]) <= PARTITION_MAX_CASES_PER_SHARD,
                   "capacity", "one shard has1..200 cases; retain multiple shard identities")
    rows, case_ids, source_ids = [], set(), set()
    for item in value["cases"]:
        row = partition_case(item)
        case_id, source_id = row["case_id"], partition_identity(row)
        partial = {"examined_cases": len(rows), "case_id": case_id, "source_identity": list(source_id)}
        partition_need(case_id not in case_ids, "duplicate_case", "duplicate case ID in complete input shard", partial)
        partition_need(source_id not in source_ids, "duplicate_source", "duplicate source identity in complete input shard", partial)
        case_ids.add(case_id); source_ids.add(source_id); rows.append(row)
    return PartitionShard(value["manifest_id"], value["evidence"], tuple(rows))


def partition_shard_from_source_packet(packet, evidence, manifest_id):
    """Consume v2 producer facts. Never inspect reference text or parse a path/ID."""
    packet = partition_plain(packet)
    return _partition_shard_from_normalized_packet(packet, evidence, manifest_id)


def _partition_shard_from_normalized_packet(packet, evidence, manifest_id):
    """Internal normalized-packet boundary shared by retained and legacy inputs."""
    partition_need(isinstance(packet, dict) and packet.get("artifact_type") == "asr_speech_source_packet"
                   and type(packet.get("schema_version")) is int and packet["schema_version"] == 1,
                   "unsupported_source", "parsed ASR source packet required")
    allowed = {"artifact_type", "schema_version", "profile", "state", "purposes", "source", "source_authority", "evidence",
               "metadata_snapshot_output_owner_sha256", "case_count", "cases", "coverage", "producer_source_observation"}
    partition_need(set(packet) <= allowed, "unsupported_source", "unknown parsed packet fields require source-owner support")
    source = packet.get("source", {})
    partition_need(isinstance(source, dict) and source.get("dataset_id") == "CAiRE/ASCEND"
                   and source.get("revision") == "737e9800ae31be9932ba8464c80366559bd28424",
                   "revision_mismatch", "only the existing parsed ASCEND revision is supported")
    partition_need(isinstance(packet.get("cases"), list) and 0 < len(packet["cases"]) <= 200,
                   "capacity", "bounded parsed source packet required")
    namespace = source["dataset_id"] + "@" + source["revision"]
    cases = []
    for row in packet["cases"]:
        partition_need(isinstance(row, dict) and set(row) <= {"case_id", "source_locator", "publisher_record", "source_observations",
            "audio_identity_declared_not_revalidated", "audio_locator", "conversion_declared", "grouping", "project_partition",
            "project_content_review", "references_by_purpose"}, "unsupported_source", "known parsed source-case fields required")
        locator, grouping = row.get("source_locator", {}), row.get("grouping", {})
        partition_need(isinstance(locator, dict) and isinstance(grouping, dict), "schema", "parsed source locator/group objects required")
        partition_text(row.get("case_id"),"parsed global case ID",256)
        for field in ("source_split", "source_id"): partition_text(locator.get(field), "parsed " + field)
        partition_need(row.get("project_partition") == "development_observation",
                       "unsupported_role", "this adapter preserves existing development observations only")
        partition_need(set(grouping) == {"speaker", "recording", "conversation", "template"},
                       "unsupported_source", "exact parsed grouping owner contract required")
        speaker, recording = grouping["speaker"], grouping["recording"]
        partition_need(all(isinstance(item, dict) for item in grouping.values()), "schema", "parsed grouping facts required")
        partition_need(speaker.get("kind") == "publisher_id" and speaker.get("namespace") == namespace
                       and speaker.get("source_field") == "original_speaker_id",
                       "revision_mismatch", "parsed publisher speaker identity/revision required")
        partition_need(recording.get("kind") == "derived" and recording.get("independence") == "unproven"
                       and grouping["conversation"].get("kind") == grouping["template"].get("kind") == "unknown",
                       "unsupported_source", "new grouping claims require a separately supported source-owner adapter")
        def fact(state, value=None, basis="producer_derived", reason="existing parsed metadata fact", ns=namespace):
            return {"state": state, "namespace": ns if state == "known" else None,
                    "value": value, "basis": basis, "reason": reason, "evidence": evidence}
        families = {kind: fact("unknown", reason="global " + kind + " mapping not provided by parsed source owner")
                    for kind in PARTITION_KINDS}
        families["speaker"] = fact("known", speaker["value"], "publisher", "publisher participant ID, not exclusive audible voice")
        families["recording"] = fact("known", recording["value"], reason="producer lexical recording grouping; global independence unproven")
        # A complete identity-preserving interval is already a parsed producer
        # fact. It is not an audio observation or an augmentation independence proof.
        conversion = row.get("conversion_declared", {})
        partition_need(isinstance(conversion, dict), "schema", "parsed conversion facts required")
        if conversion.get("recipe") == "identity-preserve-v1" and conversion.get("changes") == []:
            families["augmentation"] = fact("known", locator.get("source_split", "") + ":" + locator.get("source_id", ""),
                reason="declared identity-preserve source lineage only")
        cases.append({"case_id": row["case_id"], "source": {"dataset_id": source["dataset_id"], "revision": source["revision"],
            "publisher_split": locator.get("source_split"), "source_id": locator.get("source_id")}, "families": families,
            "project": {"role": "dev", "state": "declared", "reason": "retained existing development_observation; no new exposure asserted"},
            "evidence": evidence})
    return parse_partition_shard({"artifact_type": "project_partition_fact_shard", "schema_version": 1,
        "manifest_id": manifest_id, "evidence": evidence, "cases": cases})


# An input pin is not permission to invent another source envelope. This current
# route only selects an already-produced, separately retained metadata artifact.
# No module/executable path is taken from that artifact or its producer evidence.
RETAINED_PROJECT_PACKETS = {
    "8c7d944330a54676bb44cc4097f3565d0bf894604c3c84737051fe07ea50da6c": {
        "profile": "ascend-audited-pilot60-metadata-v1", "case_count": 60,
        "producer_sha256": "09dd710de241e5f3869c2f5a6dc51c146f15a7108c8ee599eb3dd4ee5d73fde1",
        "historical_owner_sha256": "dfca44713f27ebd1ba138ad5ac652115985969db43fd92badbd031af295c6c88"}}


@dataclass(frozen=True)
class RetainedProjectPacket:
    snapshot: InputSnapshot
    packet: dict
    shard: PartitionShard


def read_retained_project_packet(path, expected_sha, manifest_id):
    """One strict JSON capture/normalization; no historical code or media IO.

    Digest support is code-owned and independent of the caller-supplied pin.
    All source semantics still come from the same parsed-packet adapter.
    """
    partition_need(isinstance(expected_sha, str) and re.fullmatch(r"[0-9a-f]{64}", expected_sha),
                   "identity", "explicit retained packet SHA256 required")
    selected = Path(path)
    partition_need(selected.suffix == ".json", "source_boundary", "retained packet requires explicit JSON input")
    try:
        snapshot = InputSnapshot.json(external(selected), MIB)
    except (ValueError, UnicodeError, OSError) as error:
        raise PartitionError("malformed_source_packet", "bounded retained JSON capture rejected", {
            "selected_path": str(selected), "expected_sha256": expected_sha, "reason": str(error)}) from error
    partition_need(snapshot.digest == expected_sha, "source_identity_conflict", "held retained packet differs from caller-selected SHA", {
        "selected_path": str(selected), "expected_sha256": expected_sha, "observed_sha256": snapshot.digest})
    contract = RETAINED_PROJECT_PACKETS.get(snapshot.digest)
    partition_need(contract is not None, "unsupported_source", "packet pin is not a supported retained artifact; no arbitrary packet admission")
    packet = partition_plain(snapshot.parsed)
    partition_fields(packet, ("artifact_type", "schema_version", "profile", "state", "purposes", "source", "source_authority", "evidence",
        "metadata_snapshot_output_owner_sha256", "case_count", "cases", "coverage", "producer_source_observation"), "retained packet")
    partition_need(packet["profile"] == contract["profile"] and type(packet["case_count"]) is int
        and packet["case_count"] == contract["case_count"] and isinstance(packet["cases"], list)
        and len(packet["cases"]) == packet["case_count"], "source_identity_conflict", "complete retained profile/case association required")
    partition_need(packet["state"] == "metadata_projection_only_native_admission_not_run"
        and packet["purposes"] == ["functional_unscored", "publisher_reference_asr"], "unsupported_source", "retained metadata scope/purposes required")
    origin = packet["producer_source_observation"]
    partition_fields(origin, ("path", "sha256", "scope"), "historical packet producer observation")
    partition_need(origin["sha256"] == contract["producer_sha256"]
        and packet["metadata_snapshot_output_owner_sha256"] == contract["historical_owner_sha256"],
        "source_identity_conflict", "historical packet producer/owner identities must remain historical")
    evidence = {"path": str(snapshot.path), "sha256": snapshot.digest, "locator": "held unchanged preproduced source packet"}
    shard = _partition_shard_from_normalized_packet(packet, evidence, manifest_id)
    # The shared shard parser already validated every case/source identity before
    # any consumer can filter registered rows; no synthetic empty-registry pass.
    return RetainedProjectPacket(snapshot, packet, shard)


def parse_partition_reservation(value):
    value = partition_plain(value)
    partition_fields(value, ("reservation_id", "target_role", "state", "source_scope", "resolves_proposals", "families", "evidence", "reason"), "reservation")
    partition_text(value["reservation_id"], "reservation ID", 256)
    partition_need(value["target_role"] in ("dev", "test", "train") and value["state"] in ("proposed_only", "accepted_reservation"),
                   "schema", "reservation target/state explicit; not an assigned/admitted role")
    partition_need(isinstance(value["families"], list) and 0 < len(value["families"]) <= 8192,
                   "capacity", "bounded complete known reservation component required")
    scope=value["source_scope"]
    partition_need(isinstance(scope,dict),"schema","explicit reservation source applicability required")
    if scope.get("kind")=="known_sources":
        partition_fields(scope,("kind","sources"),"reservation source scope")
        partition_need(isinstance(scope["sources"],list) and 0<len(scope["sources"])<=200,"capacity","bounded source applicability")
        for source in scope["sources"]:
            partition_fields(source,("dataset_id","revision"),"reservation source revision")
            for field in source:partition_text(source[field],"reservation "+field)
        partition_need(len({(s["dataset_id"],s["revision"]) for s in scope["sources"]})==len(scope["sources"]),"duplicate_source","duplicate reservation source scope")
    else:
        partition_fields(scope,("kind","namespace","reason"),"unresolved reservation applicability")
        partition_need(scope["kind"]=="unresolved_external_component","schema","supported reservation applicability kind required")
        partition_text(scope["namespace"],"component namespace");partition_text(scope["reason"],"applicability reason")
    resolutions=value["resolves_proposals"]
    partition_need(isinstance(resolutions,list) and len(resolutions)<=200 and all(isinstance(key,str) and key.strip() for key in resolutions)
                   and len(set(resolutions))==len(resolutions),"schema","distinct explicit proposal resolutions required")
    partition_need(not resolutions or value["state"]=="accepted_reservation","schema","only an explicit accepted reservation can resolve prior proposals")
    seen = set()
    for item in value["families"]:
        partition_fields(item, ("kind", "fact"), "reservation family")
        partition_need(item["kind"] in PARTITION_KINDS, "schema", "explicit family kind required")
        fact = partition_family(item["fact"])
        partition_need(fact["state"] == "known", "schema", "reserve known identities, keep unknown closure in coverage")
        key = (item["kind"], fact["namespace"], fact["value"])
        partition_need(key not in seen, "duplicate_family", "duplicate reservation family")
        seen.add(key)
        if item["kind"]=="speaker" and scope["kind"]=="known_sources":
            partition_need(any(fact["namespace"]==s["dataset_id"]+"@"+s["revision"] for s in scope["sources"]),
                           "revision_mismatch","speaker reservation must preserve its explicit source revision")
    partition_evidence(value["evidence"])
    partition_text(value["reason"], "reservation reason")
    return value


def partition_reservations_from_annotation05(parsed, evidence):
    """Consume its existing component IDs/known group IDs, never label bodies."""
    value = partition_plain(parsed)
    namespace = "public_disflqa_train_only_annotation_probe_05"
    partition_fields(value, ("schema_version", "cohort", "prospective_only", "current_PROJECT_roles_unchanged", "actual_PROJECT_overlap", "semantic_template_separation", "components"), "annotation05 reservation facts")
    partition_need(type(value.get("schema_version")) is int and value["schema_version"] == 1 and value.get("cohort") == namespace
                   and value.get("prospective_only") is True and value.get("current_PROJECT_roles_unchanged") is True
                   and value.get("actual_PROJECT_overlap") == "NOT_ESTABLISHED"
                   and isinstance(value.get("components"), list) and len(value["components"]) == 2,
                   "unsupported_source", "existing prospective annotation05 component contract required")
    result = []
    for component in value["components"]:
        partition_fields(component, ("component_id", "role", "known_question_ids", "TRAIN_source_ids", "outside_TRAIN_grouping_only_ids",
                                     "all_known_group_keys", "source_titles", "prohibition"), "existing annotation component")
        partition_need(component.get("role") == "PROSPECTIVE_PROJECT_TRAINING_EXPOSURE_ONLY",
                       "unsupported_role", "annotation05 remains prospective/quarantined, not actual training")
        families = []
        for suffix, values in [("component", [component.get("component_id")]),
                               ("question", component.get("known_question_ids")),
                               ("known-group", component.get("all_known_group_keys"))]:
            partition_need(isinstance(values, list), "schema", "parsed known ID list required")
            for item in values:
                families.append({"kind": "source", "fact": {"state": "known", "namespace": namespace + "/" + suffix,
                    "value": item, "basis": "producer_derived", "reason": "exact existing connected-component membership; semantic closure unknown",
                    "evidence": evidence}})
        result.append(parse_partition_reservation({"reservation_id": component["component_id"], "target_role": "train",
            "state": "proposed_only", "source_scope":{"kind":"unresolved_external_component","namespace":namespace,
                "reason":"known source-group IDs only; actual PROJECT source/revision applicability not established"},
            "resolves_proposals":[], "families": families, "evidence": evidence,
            "reason": "quarantined prospective annotation05 exposure restriction; zero project human labels; no role assignment"}))
    return tuple(result)


def partition_reservation_from_speaker_proposal(parsed, evidence):
    """Retain proposal01's whole-speaker restriction without assigning test cases."""
    value = partition_plain(parsed)
    partition_need(isinstance(value, dict) and value.get("artifact_type") == "full_corpus_selection_feasibility_proposal"
                   and type(value.get("schema_version")) is int and value["schema_version"] == 1
                   and value.get("state") == "PROVISIONAL_CAPACITY_ONLY_NOT_FROZEN_SELECTION_OR_ADMISSION",
                   "unsupported_source", "existing prospective capacity proposal01 required")
    source = value.get("source", {})
    partition_need(isinstance(source, dict) and source.get("dataset") == "CAiRE/ASCEND"
                   and source.get("metadata_revision") == "b966160952fc5fe9cb036f098dd44f135d5c6f20"
                   and source.get("converted_revision") == "737e9800ae31be9932ba8464c80366559bd28424",
                   "revision_mismatch", "proposal source revisions changed")
    entries = value.get("proposed_family_reservations")
    partition_need(isinstance(entries, list) and len(entries) == 6 and all(isinstance(item,dict) for item in entries), "unsupported_source", "original six prospective quota rows required")
    selected = [item for item in entries if item.get("publisher_split") == "train" and item.get("speaker_id") == 2
                and item.get("project_role") == "locked_test_candidate_family"]
    limits=value.get("capacity_limits")
    partition_need(isinstance(limits,dict) and len(selected) == 1 and limits.get("selected_new_member_records") == 0
                   and limits.get("admitted_new_wavs") == 0,
                   "unsupported_source", "proposal remains no-members/no-admission")
    fact = {"state": "known", "namespace": "CAiRE/ASCEND@" + source["converted_revision"], "value": "2",
            "basis": "explicit_declaration", "reason": "prospective whole-speaker selector; parent820 rows, conversion/global partner closure unproved", "evidence": evidence}
    return parse_partition_reservation({"reservation_id": "ascend-whole-speaker2-proposal01", "target_role": "test", "state": "proposed_only",
        "source_scope":{"kind":"known_sources","sources":[{"dataset_id":"CAiRE/ASCEND","revision":source["converted_revision"]}]},"resolves_proposals":[],
        "families": [{"kind": "speaker", "fact": fact}], "evidence": evidence,
        "reason": "all speaker2 sessions/variants/topics prospectively excluded from future dev/training; no480 test cases assigned"})


def partition_coverage(value):
    partition_fields(value, ("state", "reason", "evidence"), "project comparison coverage")
    partition_need(value["state"] in ("unknown", "complete_declared"), "schema", "explicit comparison coverage required")
    partition_text(value["reason"], "coverage reason")
    partition_need(isinstance(value["evidence"], list) and len(value["evidence"]) <= 200, "capacity", "bounded coverage evidence")
    for item in value["evidence"]:
        partition_evidence(item)
    partition_need(value["state"] != "complete_declared" or bool(value["evidence"]), "schema", "complete coverage needs retained declaration evidence")
    return value


@dataclass(frozen=True)
class ProjectPartition:
    project_id: str
    generation: int
    predecessor_sha256: object
    coverage: dict
    cases: tuple
    manifests: tuple
    reservations: tuple
    creator_source: object


def empty_project_partition(project_id):
    partition_text(project_id, "project ID", 256)
    return ProjectPartition(project_id, 0, None, {"state": "unknown", "reason": "project-wide role/exposure coverage not established", "evidence": []}, (), (), (), None)


def merge_project_partition(current, shards=(), reservations=(), coverage=None):
    """Validate complete incoming batch before effects; repeated exact import is inert."""
    partition_need(len(shards) <= PARTITION_MAX_SHARDS, "capacity", "at most200 manifest/shard inputs")
    cases = {row["case_id"]: row for row in current.cases}
    sources = {partition_identity(row): row["case_id"] for row in current.cases}
    manifests = {item["manifest_id"]: item for item in current.manifests}
    registered_manifest = {case_id: item["manifest_id"] for item in current.manifests for case_id in item["case_ids"]}
    incoming_ids, incoming_sources, incoming_manifests = set(), set(), set()
    for shard in shards:
        partition_need(isinstance(shard, PartitionShard), "source_boundary", "consume owner-parsed PartitionShard facts")
        partition_need(shard.manifest_id not in incoming_manifests, "duplicate_manifest", "duplicate shard in selected input batch")
        incoming_manifests.add(shard.manifest_id)
        meta = {"manifest_id": shard.manifest_id, "evidence": shard.evidence, "case_ids": [row["case_id"] for row in shard.cases]}
        if shard.manifest_id in manifests:
            partition_need(manifests[shard.manifest_id] == meta, "manifest_identity_conflict", "manifest ID changed under existing registration")
        for row in shard.cases:
            case_id, source = row["case_id"], partition_identity(row)
            partial = {"examined_cases": len(incoming_ids), "case_id": case_id, "source_identity": list(source)}
            partition_need(case_id not in incoming_ids, "duplicate_case", "global case collision across selected shards", partial)
            partition_need(source not in incoming_sources, "duplicate_source", "same source row selected twice across shards", partial)
            incoming_ids.add(case_id); incoming_sources.add(source)
            if case_id in cases:
                partition_need(cases[case_id] == row, "case_identity_conflict", "existing case/source/family/role facts cannot be overwritten", partial)
                partition_need(registered_manifest[case_id] == shard.manifest_id,
                               "duplicate_case", "existing global case cannot be duplicated in a different manifest", partial)
            partition_need(source not in sources or sources[source] == case_id, "source_identity_conflict", "source row has a different global case ID", partial)
            cases[case_id] = row; sources[source] = case_id
        manifests[shard.manifest_id] = meta
    partition_need(len(manifests) <= PARTITION_MAX_SHARDS and len(cases) <= PARTITION_MAX_CASES,
                   "capacity", "project manifest/case bound exceeded")
    known = {row["reservation_id"]: row for row in current.reservations}
    seen = set()
    for row in reservations:
        row = parse_partition_reservation(row)
        key = row["reservation_id"]
        partition_need(key not in seen, "duplicate_reservation", "duplicate reservation in input batch")
        seen.add(key)
        partition_need(key not in known or known[key] == row, "reservation_identity_conflict", "existing reservation cannot be silently reassigned")
        known[key] = row
    partition_need(len(known) <= 200, "capacity", "at most200 reservation components")
    resolved={}
    for row in known.values():
        keys={(f["kind"],f["fact"]["namespace"],f["fact"]["value"]) for f in row["families"]}
        for old_id in row["resolves_proposals"]:
            old=known.get(old_id)
            partition_need(old is not None and old["state"]=="proposed_only" and old["target_role"]==row["target_role"],
                           "reservation_resolution_conflict","resolution must retain a known proposal's target; no silent reassignment")
            partition_need(old_id not in resolved,"reservation_resolution_conflict","proposal has multiple competing resolution owners")
            partition_need({(f["kind"],f["fact"]["namespace"],f["fact"]["value"]) for f in old["families"]}<=keys,
                           "reservation_resolution_conflict","accepted resolution must retain every prior known reserved family")
            resolved[old_id]=row["reservation_id"]
    references = sum(fact["state"] == "known" for row in cases.values() for fact in row["families"].values()) + sum(len(row["families"]) for row in known.values())
    partition_need(references <= PARTITION_MAX_KNOWN_FAMILY_REFERENCES,
                   "capacity", "known family graph reference budget65536; no truncated coverage")
    next_coverage = current.coverage if coverage is None else partition_coverage(partition_plain(coverage))
    return replace(current, cases=tuple(cases[key] for key in sorted(cases)),
        manifests=tuple(manifests[key] for key in sorted(manifests)),
        reservations=tuple(known[key] for key in sorted(known)), coverage=next_coverage)


def parse_project_partition(value):
    value = partition_plain(value)
    partition_fields(value, ("artifact_type", "schema_version", "project_id", "generation", "predecessor_sha256", "coverage", "cases", "manifests", "reservations", "creator_source"), "project partition")
    partition_need(value["artifact_type"] == "project_corpus_partition" and type(value["schema_version"]) is int and value["schema_version"] == 1,
                   "schema", "project partition version")
    partition_text(value["project_id"], "project ID", 256)
    partition_need(type(value["generation"]) is int and value["generation"] >= 1, "schema", "positive registry generation")
    partition_need(value["predecessor_sha256"] is None or (isinstance(value["predecessor_sha256"], str)
                   and re.fullmatch(r"[0-9a-f]{64}", value["predecessor_sha256"])), "identity", "registry predecessor SHA")
    partition_need(isinstance(value["cases"], list) and len(value["cases"]) <= PARTITION_MAX_CASES
                   and isinstance(value["manifests"], list) and len(value["manifests"]) <= PARTITION_MAX_SHARDS
                   and isinstance(value["reservations"], list) and len(value["reservations"]) <= 200,
                   "capacity", "registry collections bounded")
    # Use the same case parser, uniqueness and shard interpretation as ingestion.
    parsed_rows = [partition_case(row) for row in value["cases"]]
    rows = {row["case_id"]: row for row in parsed_rows}
    partition_need(len(rows) == len(value["cases"]), "duplicate_case", "duplicate stored case ID")
    shards = []
    referenced = []
    for meta in value["manifests"]:
        partition_fields(meta, ("manifest_id", "evidence", "case_ids"), "stored manifest")
        partition_need(isinstance(meta["case_ids"], list) and all(case_id in rows for case_id in meta["case_ids"]), "identity", "stored manifest member missing")
        referenced.extend(meta["case_ids"])
        shards.append(parse_partition_shard({"artifact_type": "project_partition_fact_shard", "schema_version": 1,
            "manifest_id": meta["manifest_id"], "evidence": meta["evidence"], "cases": [rows[key] for key in meta["case_ids"]]}))
    partition_need(len(referenced) == len(rows), "identity", "stored manifests must cover every case exactly once")
    parsed = merge_project_partition(empty_project_partition(value["project_id"]), shards, value["reservations"], value["coverage"])
    creator = partition_evidence(value["creator_source"])
    return replace(parsed, generation=value["generation"], predecessor_sha256=value["predecessor_sha256"], creator_source=creator)


def project_partition_decision(view, selected_ids, purpose):
    """Known connected-family conflict and explicit unknown coverage; no name heuristics."""
    partition_need(purpose in ("existing_development", "certified_lock", "certified_training"), "purpose", "partition purpose required")
    selected_ids = partition_plain(selected_ids)
    partition_need(isinstance(selected_ids, list) and 0 < len(selected_ids) <= PARTITION_MAX_CASES
                   and all(isinstance(key,str) and key.strip() and len(key.encode())<=256 for key in selected_ids)
                   and len(set(selected_ids)) == len(selected_ids), "selection", "distinct selected global case IDs required")
    rows = {row["case_id"]: row for row in view.cases}
    partition_need(all(key in rows for key in selected_ids), "selection", "selected case missing from project facts")
    role = {"existing_development": "dev", "certified_lock": "test", "certified_training": "train"}[purpose]
    parent = {}
    def find(key):
        parent.setdefault(key, key)
        trail = []
        while parent[key] != key:
            trail.append(key); key = parent[key]
        for item in trail:
            parent[item] = key
        return key
    def connect(keys):
        keys = list(keys)
        if keys:
            root = find(keys[0])
            for key in keys[1:]: parent[find(key)] = root
    def keys_for(row):
        return [(kind, fact["namespace"], fact["value"]) for kind, fact in row["families"].items() if fact["state"] == "known"]
    for row in view.cases: connect(keys_for(row))
    resolved={key for row in view.reservations for key in row["resolves_proposals"]}
    active_reservations=[row for row in view.reservations if row["reservation_id"] not in resolved]
    for row in active_reservations:
        connect((item["kind"], item["fact"]["namespace"], item["fact"]["value"]) for item in row["families"])
    by_component = defaultdict(list)
    for row in view.cases:
        for root in {find(key) for key in keys_for(row)}: by_component[root].append(row)
    reservation_components = defaultdict(list)
    for row in active_reservations:
        for root in {find((item["kind"], item["fact"]["namespace"], item["fact"]["value"])) for item in row["families"]}:
            reservation_components[root].append(row)
    conflicts, unknowns, advisories = [], [], []
    revisions = defaultdict(set)
    for row in view.cases: revisions[row["source"]["dataset_id"]].add(row["source"]["revision"])
    for reservation in active_reservations:
        scope=reservation["source_scope"]
        if scope["kind"]=="known_sources":
            for source in scope["sources"]:revisions[source["dataset_id"]].add(source["revision"])
        else:
            unknowns.append({"code":"unresolved_reservation_applicability","reservation_id":reservation["reservation_id"],
                             "source_scope":scope,"evidence":reservation["evidence"]})
    selected_components = defaultdict(list)
    unresolved = {}
    role_mismatches = defaultdict(list)
    selected_set = set(selected_ids)
    incompatible_rows = [row for row in view.cases if row["project"]["state"] != "proposed_only"
                         and row["project"]["role"] in ("dev", "test", "train") and row["project"]["role"] != role]
    incompatible_ids = {row["case_id"] for row in incompatible_rows}
    # Missing edges are not disjointness evidence. With any incompatible role
    # or reservation, every inventory row can be a partly mapped connecting
    # row, including same-role/quarantined rows outside the known components.
    # Role-inventory completeness never resolves their grouping applicability.
    comparison_required = bool(incompatible_rows or any(row["target_role"] != role for row in active_reservations))
    comparison_rows = [row for row in view.cases if row["case_id"] not in selected_set] if comparison_required else []
    def retain_reason(code, row, scope, fact):
        # Aggregate complete denominators while retaining only eight examples;
        # do not expand every comparison pair or copy revision sets per case.
        reason = unresolved.setdefault(code, {"code": code.split(":", 1)[0],
            "dimension": code.split(":", 1)[1] if ":" in code else None,
            "case_count": 0, "case_examples": [], "scope_counts": Counter(), "role_counts": Counter(), "fact_examples": []})
        reason["case_count"] += 1
        reason["scope_counts"][scope] += 1
        reason["role_counts"][row["project"]["role"]] += 1
        if len(reason["case_examples"]) < 8:
            reason["case_examples"].append(row["case_id"])
            reason["fact_examples"].append({"case_id": row["case_id"], "scope": scope, "project": row["project"],
                "source": row["source"], "fact": fact, "evidence": row["evidence"]})
    revision_examples = {dataset: sorted(values)[:8] for dataset, values in revisions.items() if len(values) > 1}
    def retain_uncertainty(row, scope):
        for kind, fact in row["families"].items():
            if fact["state"] == "unknown":
                retain_reason("unknown_family:" + kind, row, scope, fact)
        if len(revisions[row["source"]["dataset_id"]]) > 1:
            retain_reason("unmapped_dataset_revisions:" + row["source"]["dataset_id"], row, scope, {
                "source": row["source"], "revision_count": len(revisions[row["source"]["dataset_id"]]),
                "revision_examples": revision_examples[row["source"]["dataset_id"]],
                "reason": "revision identities have no authoritative cross-revision family mapping", "evidence": row["evidence"]})
    for case_id in selected_ids:
        row = rows[case_id]
        if row["project"]["state"] == "proposed_only" or row["project"]["role"] in ("quarantine", "unassigned"):
            retain_reason("unassigned_or_proposed_role", row, "selected", row["project"])
        elif row["project"]["role"] != role:
            role_mismatches[row["project"]["role"]].append(case_id)
        retain_uncertainty(row, "selected")
        for root in {find(key) for key in keys_for(row)}:
            selected_components[root].append(case_id)
    for row in comparison_rows:
        retain_uncertainty(row, "comparison_graph")
    # Summarize complete examined components rather than allocating N*N pairs.
    # Exact complete facts remain in the SHA-bound registry/source shard.
    for root, selected in sorted(selected_components.items()):
        incompatible = [other for other in by_component[root] if other["case_id"] in incompatible_ids]
        if incompatible:
            conflicts.append({"code": "connected_family_role_conflict", "family": list(root), "requested_role": role,
                "selected_count": len(selected), "selected_examples": selected[:8], "incompatible_count": len(incompatible),
                "incompatible_role_counts": dict(Counter(other["project"]["role"] for other in incompatible)),
                "incompatible_examples": [{"case_id": other["case_id"], "evidence": other["evidence"]} for other in incompatible[:8]]})
        for reservation in reservation_components[root]:
            detail = {"code": "reservation_requires_decision" if reservation["state"] == "proposed_only" else "reserved_family_role_conflict",
                "selected_count": len(selected), "selected_examples": selected[:8], "reservation_id": reservation["reservation_id"], "target_role": reservation["target_role"],
                "reservation_state": reservation["state"], "evidence": reservation["evidence"]}
            if reservation["state"] == "proposed_only": advisories.append(detail)
            elif reservation["target_role"] != role: conflicts.append(detail)
    unknowns.extend(reason for code, reason in sorted(unresolved.items()))
    for existing_role, cases in sorted(role_mismatches.items()):
        conflicts.append({"code": "existing_role_conflict", "existing_role": existing_role, "requested_role": role,
                          "case_count": len(cases), "case_examples": cases[:8]})
    if view.coverage["state"] != "complete_declared": unknowns.append({"code": "project_comparison_incomplete", "coverage": view.coverage})
    def unique(values):
        return [json.loads(key) for key in sorted({json.dumps(value, sort_keys=True) for value in values})]
    conflicts, unknowns, advisories = unique(conflicts), unique(unknowns), unique(advisories)
    certainty = "conflict" if conflicts else "unknown" if unknowns or advisories else "allowed"
    return {"artifact_type": "project_partition_decision", "schema_version": 1, "project_id": view.project_id,
        "purpose": purpose, "requested_role": role, "selected_case_ids": selected_ids, "certainty": certainty,
        "applicability": "existing_development_facts_only" if purpose == "existing_development" else "certified_partition_separation_only",
        "ingest_truthful_facts": "allowed", "consumer_action": "block" if conflicts or (purpose != "existing_development" and certainty != "allowed") else "continue_existing_development_only" if purpose == "existing_development" else "partition_separation_satisfied",
        "conflicts": conflicts, "unknowns": unknowns, "proposed_reservation_advisories": advisories,
        "examined": {"cases": len(view.cases), "manifests": len(view.manifests), "reservations": len(view.reservations)},
        "comparison_scope": {"selected_cases": len(selected_ids), "nonselected_grouping_cases": len(comparison_rows),
            "incompatible_role_cases": len(incompatible_rows), "incompatible_role_counts": dict(Counter(row["project"]["role"] for row in incompatible_rows)),
            "rule": "all selected rows; all inventory rows as potential connectors when an incompatible role or active reservation exists; no source-name exclusion"},
        "resolved_proposals_retained":sorted(resolved),
        "coverage": view.coverage, "detail_policy": "complete examined component counts; at most8 case/evidence examples per reason; full facts remain in bound inputs",
        "admission": "not_granted_media_rights_content_and_training_authority_are_separate"}


def partition_encode(value):
    # Bound expansion during encoding, before allocating one unbounded JSON copy.
    buffer = io.BytesIO()
    for part in json.JSONEncoder(ensure_ascii=False, sort_keys=True, allow_nan=False, separators=(",", ":")).iterencode(value):
        raw = part.encode()
        partition_need(buffer.tell() + len(raw) + 1 <= PARTITION_BYTES, "capacity", "partition JSON expansion exceeds16MiB")
        buffer.write(raw)
    buffer.write(b"\n")
    return buffer.getvalue()


def partition_document(view):
    return {"artifact_type": "project_corpus_partition", "schema_version": 1,
        "project_id": view.project_id, "generation": view.generation, "predecessor_sha256": view.predecessor_sha256,
        "coverage": view.coverage, "cases": list(view.cases), "manifests": list(view.manifests),
        "reservations": list(view.reservations), "creator_source": view.creator_source}


def partition_owner_identity():
    snapshot = InputSnapshot.capture(Path(__file__), MIB)
    return {"path": str(snapshot.path), "sha256": snapshot.digest,
            "locator": "active extended public-corpus source file observation, not loaded-bytecode attestation"}


def partition_read_registry(path, project_id):
    selected = Path(path)
    path = external(selected)
    partition_need(selected == path, "registry_namespace_unknown", "registry path must be absolute/canonical with no symlink aliases")
    partition_need(path.suffix == ".json", "schema", "explicit external registry JSON required")
    try:
        snapshot = InputSnapshot.json(path, PARTITION_BYTES)
    except FileNotFoundError:
        return empty_project_partition(project_id), None
    except (ValueError, UnicodeError, TypeError) as error:
        raise PartitionError("malformed_registry", "existing registry rejected; no replacement permitted", {"path": str(path), "reason": str(error)}) from error
    try:
        value = parse_project_partition(snapshot.parsed)
    except (KeyError, TypeError, ValueError) as error:
        if isinstance(error, PartitionError): raise
        raise PartitionError("malformed_registry", "existing registry structure rejected; no replacement permitted") from error
    partition_need(value.project_id == project_id, "project_identity_conflict", "registry belongs to a different project")
    return value, snapshot


def partition_selection(registry_path, registry_sha, project_id, shard, selected_ids, purpose):
    """Read-only direct-consumer seam; missing registry is explicit unknown."""
    current, snapshot = partition_read_registry(registry_path, project_id)
    partition_need((snapshot.digest if snapshot else None) == registry_sha, "registry_identity_conflict", "caller-selected registry snapshot changed")
    # Compare registered logical facts while preserving both provenance snapshots.
    # A fresh packet container is not a new source case or overwrite authority.
    existing = {row["case_id"]: row for row in current.cases}
    def logical(row):
        value = partition_plain(row)
        del value["evidence"]
        for fact in value["families"].values(): del fact["evidence"]
        return value
    fresh = []
    for row in shard.cases:
        if row["case_id"] in existing:
            partition_need(logical(row) == logical(existing[row["case_id"]]),
                           "case_identity_conflict", "selected parsed facts differ from registered case/source/family/role facts")
        else: fresh.append(row)
    proposed = merge_project_partition(current, (replace(shard, cases=tuple(fresh)),)) if fresh else current
    result = project_partition_decision(proposed, selected_ids, purpose)
    result["registry_source"] = None if snapshot is None else {"path": str(snapshot.path), "sha256": snapshot.digest,
        "locator": "held project registry snapshot; no mutation"}
    result["active_owner_source"] = partition_owner_identity()
    result["source_manifest"] = {"manifest_id": shard.manifest_id, "evidence": shard.evidence}
    unregistered_selected = [case_id for case_id in result["selected_case_ids"] if case_id not in existing]
    result["registered_selected_cases"] = len(result["selected_case_ids"]) - len(unregistered_selected)
    if unregistered_selected and purpose != "existing_development":
        result["unknowns"].append({"code": "selected_cases_not_registered", "case_ids": unregistered_selected})
        if not result["conflicts"]: result["certainty"] = "unknown"
        result["consumer_action"] = "block"
    return result


class PartitionWriter:
    """Cooperative private-directory transaction; retain uncertain cleanup ownership.

    A stable flock file serializes compliant writers. Descriptor/version checks
    detect replacement; this is not protection against a malicious same-UID
    actor racing the final rename. No PID/path-only cleanup ownership claims.
    """
    def __init__(self, registry):
        self.registry = Path(registry)
        self.root = None
        self.lock_fd = self.stage_fd = None
        self.stage_name = None
        self.stage_identity = None
        self.replaced = False

    def retain(self, reason):
        if self not in RETAINED_PARTITION_WRITERS: RETAINED_PARTITION_WRITERS.append(self)
        raise PartitionError("retained_transaction", reason, {"registry": str(self.registry), "stage_name": self.stage_name,
            "stage_identity": self.stage_identity, "replacement_observed": self.replaced, "ownership": "retained_by_live_PartitionWriter"})

    def __enter__(self):
        info = self.registry.parent.stat()
        partition_need(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700,
                       "namespace_authority", "registry parent must be an explicitly initialized owned0700 directory")
        self.root = OutputRoot(self.registry.parent)
        self.parent_fd = self.root.parents[Path()][0]
        try:
            self.lock_fd = os.open(self.registry.name + ".lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600, dir_fd=self.parent_fd)
            lock = os.fstat(self.lock_fd)
            partition_need(stat.S_ISREG(lock.st_mode) and lock.st_uid == os.getuid() and stat.S_IMODE(lock.st_mode) == 0o600,
                           "namespace_authority", "owned regular0600 stable lock required")
            fcntl.flock(self.lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.lock_identity = OutputRoot.identity(lock)
            self.root.__enter__()
            return self
        except Exception:
            if self.lock_fd is not None: os.close(self.lock_fd); self.lock_fd = None
            self.root.release_handles()
            raise

    def assert_current(self, expected):
        self.root.assert_applicable()
        lock = os.stat(self.registry.name + ".lock", dir_fd=self.parent_fd, follow_symlinks=False)
        if OutputRoot.identity(lock) != self.lock_identity: self.retain("lock namespace changed; cooperative authority unknown")
        try:
            info = os.stat(self.registry.name, dir_fd=self.parent_fd, follow_symlinks=False)
        except FileNotFoundError:
            partition_need(expected is None, "registry_identity_conflict", "registry disappeared before replacement")
            return
        partition_need(expected is not None and stat.S_ISREG(info.st_mode)
                       and (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns) == expected.source_identity,
                       "registry_identity_conflict", "registry identity/version changed; original retained")

    def publish(self, raw, previous):
        self.assert_current(previous)
        if previous is not None:
            backup = self.registry.parent / (self.registry.name + ".backup-" + previous.digest + ".json")
            try:
                info = os.stat(backup, follow_symlinks=False)
                partition_need(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid(), "backup_identity_conflict", "backup is not an owned regular file")
                held = InputSnapshot.capture(backup, PARTITION_BYTES)
                partition_need(held.raw == previous.raw and held.source_identity[:2] == (info.st_dev, info.st_ino), "backup_identity_conflict", "existing immutable backup differs or moved")
            except FileNotFoundError:
                write_once(backup, previous.raw)
        self.stage_name = "." + self.registry.name + ".stage-" + secrets.token_hex(16)
        self.stage_fd = os.open(self.stage_name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.parent_fd)
        self.stage_identity = OutputRoot.identity(os.fstat(self.stage_fd))
        with os.fdopen(os.dup(self.stage_fd), "wb") as stream:
            stream.write(raw); stream.flush(); os.fsync(stream.fileno())
        self.assert_current(previous)
        held = os.fstat(self.stage_fd)
        visible = os.stat(self.stage_name, dir_fd=self.parent_fd, follow_symlinks=False)
        if OutputRoot.identity(visible) != self.stage_identity or held.st_size != len(raw): self.retain("staged identity changed")
        os.replace(self.stage_name, self.registry.name, src_dir_fd=self.parent_fd, dst_dir_fd=self.parent_fd)
        self.stage_name = None; self.replaced = True
        observed = os.stat(self.registry.name, dir_fd=self.parent_fd, follow_symlinks=False)
        if OutputRoot.identity(observed) != self.stage_identity: self.retain("replacement identity changed; outcome uncertain")
        try:
            os.fsync(self.parent_fd)
        except OSError as error:
            # The new inode is already visible. Preserve it and its backup;
            # never report a pre-replacement failure or roll back by pathname.
            raise PartitionError("durability_unknown", "replacement observed; directory fsync failed", {
                "registry": str(self.registry), "replacement_observed": True, "registry_sha256": sha(raw),
                "backup_of_predecessor": previous.digest if previous else None}) from error

    def __exit__(self, error_type, error, traceback):
        try:
            if self.stage_name is not None:
                try:
                    info = os.stat(self.stage_name, dir_fd=self.parent_fd, follow_symlinks=False)
                    if OutputRoot.identity(info) != self.stage_identity: self.retain("stage name no longer proves ownership")
                    os.unlink(self.stage_name, dir_fd=self.parent_fd); self.stage_name = None
                except OSError as cleanup_error: self.retain("owned stage cleanup failed: " + str(cleanup_error))
            self.root.assert_applicable()
            self.root.finish()
            if self.stage_fd is not None: os.close(self.stage_fd); self.stage_fd = None
            if self.lock_fd is not None: os.close(self.lock_fd); self.lock_fd = None
        except Exception as cleanup_error:
            if self not in RETAINED_PARTITION_WRITERS:
                self.retain("transaction closure incomplete: " + str(cleanup_error))
            raise
        finally:
            if self.root.token is not None:
                OUTPUT_ROOT.reset(self.root.token); self.root.token = None


def update_project_partition(registry_path, expected_sha, project_id, shards=(), reservations=(), coverage=None):
    registry = external(registry_path)
    partition_need(Path(registry_path) == registry, "registry_namespace_unknown", "canonical selected registry path required")
    partition_need(expected_sha is None or (isinstance(expected_sha, str) and re.fullmatch(r"[0-9a-f]{64}", expected_sha)),
                   "identity", "explicit predecessor SHA256 or absent required")
    active = partition_owner_identity()
    with PartitionWriter(registry) as transaction:
        current, snapshot = partition_read_registry(registry, project_id)
        partition_need((snapshot.digest if snapshot else None) == expected_sha,
                       "registry_identity_conflict", "explicit predecessor SHA required; stale update not applied")
        next_view = merge_project_partition(current, shards, reservations, coverage)
        if partition_document(current) == partition_document(next_view):
            return {"kind": "unchanged", "registry_sha256": snapshot.digest if snapshot else None,
                    "generation": current.generation, "active_owner_source": active, "admission": "not_granted"}
        next_view = replace(next_view, generation=current.generation + 1,
            predecessor_sha256=snapshot.digest if snapshot else None, creator_source=active)
        raw = partition_encode(partition_document(next_view))
        transaction.publish(raw, snapshot)
        return {"kind": "updated", "registry_sha256": sha(raw), "generation": next_view.generation,
                "active_owner_source": active, "admission": "not_granted", "facts_ingested_not_roles_assigned": True}


def partition_for_review_import(args, candidates, selected_ids):
    """Direct reviewed-import consumer of the same parsed producer packet/API.

    Candidate bytes are bound by existing packet evidence, not reinterpreted as
    grouping facts. This check completes before any importer WAV reads/copies.
    """
    names = ("project_source_packet", "project_source_packet_sha256", "project_registry",
             "project_registry_sha256", "project_id", "project_manifest_id")
    values = [getattr(args, name, None) for name in names]
    if not any(value is not None for value in values): return None
    partition_need(all(value is not None for value in values), "source_boundary", "review import partition arguments must be supplied together")
    snapshot = InputSnapshot.json(external(args.project_source_packet), MIB)
    partition_need(snapshot.digest == args.project_source_packet_sha256, "source_identity_conflict", "selected parsed packet SHA changed")
    packet = partition_plain(snapshot.parsed)
    partition_need(isinstance(packet,dict) and isinstance(packet.get("evidence"),list)
                   and len(packet["evidence"]) <= 200, "source_boundary", "parsed packet evidence required")
    matches = [item for item in packet["evidence"] if isinstance(item,dict) and item.get("role") == "candidates"]
    partition_need(len(matches) == 1 and matches[0].get("sha256") == candidates.digest,
                   "source_identity_conflict", "review candidate bytes must match the source producer's parsed packet evidence")
    partition_need(args.project_registry_sha256 == "absent" or (isinstance(args.project_registry_sha256,str)
                   and re.fullmatch(r"[0-9a-f]{64}",args.project_registry_sha256)), "identity", "selected registry SHA256 or absent required")
    evidence = {"path": str(snapshot.path), "sha256": snapshot.digest, "locator": "existing parsed source packet for reviewed import"}
    shard = partition_shard_from_source_packet(packet,evidence,args.project_manifest_id)
    result = partition_selection(args.project_registry,None if args.project_registry_sha256 == "absent" else args.project_registry_sha256,
        args.project_id,shard,selected_ids,"existing_development")
    partition_need(result["consumer_action"] != "block", "project_partition_conflict", "reviewed development import conflicts with known project role/reservation", result)
    return result


def partition_read_selected_input(item, remaining_bytes):
    """One captured/parsed metadata boundary shared by update and read-only check."""
    partition_fields(item, ("kind", "path", "sha256", "manifest_id"), "selected partition input")
    partition_need(item["kind"] in ("asr_source_packet", "annotation05_reservations", "speaker_proposal01", "partition_fact_shard"),
                   "unsupported_source", "unsupported selected metadata adapter; no input opened")
    partition_need(isinstance(item["path"], str) and Path(item["path"]).suffix == ".json",
                   "source_boundary", "selected metadata must have its explicit JSON suffix")
    partition_need(type(remaining_bytes) is int and remaining_bytes > 0, "capacity", "selected metadata byte budget exhausted")
    snap = InputSnapshot.json(external(item["path"]), min(2 * MIB, remaining_bytes))
    partition_need(snap.digest == item["sha256"], "source_identity_conflict", "selected metadata identity changed")
    evidence = {"path": item["path"], "sha256": snap.digest, "locator": item["manifest_id"]}
    if item["kind"] == "asr_source_packet":
        shards, reservations = (partition_shard_from_source_packet(snap.parsed, evidence, item["manifest_id"]),), ()
    elif item["kind"] == "annotation05_reservations":
        shards, reservations = (), partition_reservations_from_annotation05(snap.parsed, evidence)
    elif item["kind"] == "speaker_proposal01":
        shards, reservations = (), (partition_reservation_from_speaker_proposal(snap.parsed, evidence),)
    else:
        shard = parse_partition_shard(snap.parsed)
        partition_need(shard.manifest_id == item["manifest_id"], "manifest_identity_conflict", "selected shard manifest ID differs")
        shards, reservations = (replace(shard, evidence=evidence),), ()
    return shards, reservations, len(snap.raw)


def project_partition_check_command(args):
    """Read-only public consumer of the same PartitionShard/ProjectPartition decision."""
    try:
        snapshot = InputSnapshot.json(external(args.plan), 128 * 1024)
        plan = partition_plain(snapshot.parsed)
        partition_fields(plan, ("artifact_type", "schema_version", "project_id", "registry_path", "expected_registry_sha256",
                               "input", "selected_case_ids", "purpose"), "partition check plan")
        partition_need(plan["artifact_type"] == "project_partition_check_plan" and type(plan["schema_version"]) is int
                       and plan["schema_version"] == 1, "schema", "partition check plan version")
        shards, reservations, _ = partition_read_selected_input(plan["input"], PARTITION_BYTES - len(snapshot.raw))
        partition_need(len(shards) == 1 and not reservations, "source_boundary", "partition check selects one parsed case shard")
        result = partition_selection(plan["registry_path"], plan["expected_registry_sha256"], plan["project_id"],
                                     shards[0], plan["selected_case_ids"], plan["purpose"])
        result["check_plan_source"] = {"path": str(snapshot.path), "sha256": snapshot.digest,
                                       "locator": "caller-selected read-only partition check"}
        # Encoding uses the existing16MiB expansion bound. A blocked decision is
        # still returned losslessly; exit2 is not a rights/content admission error.
        sys.stdout.buffer.write(partition_encode(result))
        return 2 if result["consumer_action"] == "block" else 0
    except (PartitionError, ValueError, OSError, KeyError, TypeError) as error:
        failure = error.fact() if isinstance(error, PartitionError) else {"kind": "error", "code": "metadata_input_or_io", "detail": str(error), "partial": {}}
        print(json.dumps(failure, ensure_ascii=False), file=sys.stderr); return 2


def project_partition_command(args):
    """Explicit metadata transaction plan; no implicit current registry discovery."""
    try:
        plan_snapshot = InputSnapshot.json(external(args.plan), 128 * 1024)
        plan = partition_plain(plan_snapshot.parsed)
        partition_fields(plan, ("artifact_type", "schema_version", "project_id", "registry_path", "expected_registry_sha256", "inputs", "coverage"), "partition update plan")
        partition_need(plan["artifact_type"] == "project_partition_update_plan" and type(plan["schema_version"]) is int and plan["schema_version"] == 1,
                       "schema", "partition update plan version")
        partition_need(isinstance(plan["inputs"], list) and 0 < len(plan["inputs"]) <= PARTITION_MAX_SHARDS,
                       "capacity", "explicit1..200 selected metadata inputs")
        shards, reservations, total = [], [], len(plan_snapshot.raw)
        for item in plan["inputs"]:
            selected_shards, selected_reservations, captured_bytes = partition_read_selected_input(item, PARTITION_BYTES - total)
            shards.extend(selected_shards); reservations.extend(selected_reservations)
            total += captured_bytes
        current, snapshot = partition_read_registry(plan["registry_path"], plan["project_id"])
        partition_need((snapshot.digest if snapshot else None) == plan["expected_registry_sha256"], "registry_identity_conflict", "plan predecessor changed")
        proposed = merge_project_partition(current, shards, reservations, plan["coverage"])
        decision = {"kind": "planned_metadata_ingest", "cases": len(proposed.cases), "manifests": len(proposed.manifests),
            "reservations": len(proposed.reservations), "coverage": proposed.coverage, "roles_assigned": False,
            "admission": "not_granted", "active_owner_source": partition_owner_identity(), "plan_sha256": plan_snapshot.digest}
        if not args.dry_run:
            decision["write"] = update_project_partition(plan["registry_path"], plan["expected_registry_sha256"], plan["project_id"], shards, reservations, plan["coverage"])
        print(json.dumps(decision, ensure_ascii=False)); return 0
    except (PartitionError, ValueError, OSError, KeyError, TypeError) as error:
        failure = error.fact() if isinstance(error, PartitionError) else {"kind": "error", "code": "metadata_input_or_io", "detail": str(error), "partial": {}}
        print(json.dumps(failure, ensure_ascii=False), file=sys.stderr); return 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="command", required=True)
    partition = subs.add_parser("project-reservations", help="plan/ingest selected project role facts; never grants admission")
    partition.add_argument("--plan", required=True)
    partition.add_argument("--dry-run", action="store_true")
    partition.set_defaults(action=project_partition_command)
    check = subs.add_parser("project-partition-check", help="read-only same-owner partition decision; never grants training admission")
    check.add_argument("--plan", required=True)
    check.set_defaults(action=project_partition_check_command)
    download = subs.add_parser("fetch", help="fetch only pinned files in an explicit plan")
    download.add_argument("--plan", required=True)
    download.add_argument("--out-dir", required=True)
    download.add_argument("--max-file-bytes", type=int, default=120 * MIB)
    download.add_argument("--max-total-bytes", type=int, default=250 * MIB)
    download.add_argument("--deadline-seconds", type=int, default=300)
    download.add_argument("--dry-run", action="store_true")
    download.set_defaults(action=fetch)
    prepare = subs.add_parser("prepare-ascend", help="audit pinned source and prepare 60 unreviewed development candidates plus six synthetic negatives")
    prepare.add_argument("--source-dir", required=True)
    prepare.add_argument("--out-dir", required=True)
    prepare.add_argument("--max-file-bytes", type=int, default=120 * MIB)
    prepare.add_argument("--max-total-bytes", type=int, default=250 * MIB)
    prepare.add_argument("--max-rows", type=int, default=3000)
    prepare.add_argument("--max-row-group-bytes", type=int, default=64 * MIB)
    prepare.add_argument("--deadline-seconds", type=int, default=120)
    prepare.add_argument("--dry-run", action="store_true")
    prepare.set_defaults(action=prepare_ascend)
    metadata = subs.add_parser("inspect-yodas", help="bounded metadata only; never downloads audio")
    metadata.add_argument("--source-dir", required=True)
    metadata.add_argument("--out-dir", required=True)
    metadata.add_argument("--max-file-bytes", type=int, default=12 * MIB)
    metadata.add_argument("--max-rows", type=int, default=1000)
    metadata.add_argument("--max-row-group-bytes", type=int, default=64 * MIB)
    metadata.add_argument("--deadline-seconds", type=int, default=120)
    metadata.add_argument("--dry-run", action="store_true")
    metadata.set_defaults(action=inspect_yodas)
    review = subs.add_parser("import-reviewed", help="import explicit human labels; pending labels fail closed")
    review.add_argument("--candidates", required=True)
    review.add_argument("--labels", required=True)
    review.add_argument("--out-dir", required=True)
    review.add_argument("--dry-run", action="store_true")
    review.add_argument("--project-source-packet", help="Optional complete parsed producer packet; no raw-source grouping parser")
    review.add_argument("--project-source-packet-sha256")
    review.add_argument("--project-registry")
    review.add_argument("--project-registry-sha256", help="Exact snapshot SHA256 or literal absent")
    review.add_argument("--project-id")
    review.add_argument("--project-manifest-id")
    review.set_defaults(action=import_review)
    args = parser.parse_args()
    require(getattr(args, "deadline_seconds", 1) > 0, "positive deadline required")
    return args.action(args)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except PartitionError as error:
        print(json.dumps(error.fact(),ensure_ascii=False),file=sys.stderr)
        sys.exit(2)
    except (ValueError, OSError) as error:
        print(f"public-corpus preparation rejected: {error}", file=sys.stderr)
        sys.exit(2)
