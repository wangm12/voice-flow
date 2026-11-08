#!/usr/bin/env python3
"""Verify that DependencyTexts is copied byte-for-byte into a package boundary."""
import hashlib
import json
import os
import stat
import sys
from pathlib import Path


def fail(message):
    raise ValueError(message)


def canonical_physical_root(value):
    """Require callers to resolve trusted aliases before giving us a root."""
    path = Path(value)
    if not path.is_absolute() or Path(os.path.normpath(str(path))) != path:
        fail("verification roots must be absolute canonical physical paths")
    current = Path(path.anchor)
    for component in path.parts[1:]:
        current = current / component
        info = os.lstat(current)
        if stat.S_ISLNK(info.st_mode):
            fail("verification root path contains a symlink component; canonicalize trusted aliases before invoking")
    resolved = path.resolve(strict=True)
    if resolved != path:
        fail("verification root path is not canonical; canonicalize trusted aliases before invoking")
    return path


def manifest(root):
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        fail("dependency text root is missing or not a real directory")
    for name in ("license-index.json", "DEPENDENCY_LICENSES.md"):
        item = root / name
        if item.is_symlink() or not item.is_file() or item.stat().st_size == 0:
            fail("required dependency text inventory file is missing")
    by_sha = root / "by-sha256"
    if by_sha.is_symlink() or not by_sha.is_dir():
        fail("dependency text body directory is missing")

    entries = []
    def raise_walk_error(error):
        raise error

    for base, dirs, files in os.walk(root, followlinks=False, onerror=raise_walk_error):
        base_path = Path(base)
        for name in dirs:
            if (base_path / name).is_symlink():
                fail("dependency text tree contains a symlink")
        for name in files:
            path = base_path / name
            if path.is_symlink():
                fail("dependency text tree contains a symlink")
            before = os.lstat(path)
            if not stat.S_ISREG(before.st_mode):
                fail("dependency text tree contains a non-regular file")
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                for block in iter(lambda: stream.read(128 * 1024), b""):
                    digest.update(block)
            after = os.lstat(path)
            if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
                after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns
            ):
                fail("dependency text changed while being checked")
            entries.append({
                "path": path.relative_to(root).as_posix(),
                "bytes": before.st_size,
                "sha256": digest.hexdigest(),
            })
    entries.sort(key=lambda row: row["path"])
    if not any(row["path"].startswith("by-sha256/") for row in entries):
        fail("dependency text body directory is empty")
    canonical = json.dumps(entries, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return {
        "entries": entries,
        "file_count": len(entries),
        "total_bytes": sum(row["bytes"] for row in entries),
        "tree_sha256": hashlib.sha256(canonical).hexdigest(),
    }


def main(argv):
    if len(argv) != 3:
        print("usage: verify-dependency-texts.py SOURCE_ROOT DESTINATION_ROOT", file=sys.stderr)
        return 2
    try:
        source_real = canonical_physical_root(argv[1])
        destination_real = canonical_physical_root(argv[2])
        if (source_real == destination_real
                or source_real in destination_real.parents
                or destination_real in source_real.parents):
            fail("source and destination trees must be disjoint")
        source_manifest = manifest(source_real)
        destination_manifest = manifest(destination_real)
        if source_manifest["entries"] != destination_manifest["entries"]:
            fail("packaged dependency text tree differs from the current source tree")
    except (OSError, ValueError) as error:
        print("dependency text package verification failed: " + str(error), file=sys.stderr)
        return 1
    print(json.dumps({
        "status": "match",
        "file_count": source_manifest["file_count"],
        "total_bytes": source_manifest["total_bytes"],
        "tree_sha256": source_manifest["tree_sha256"],
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
