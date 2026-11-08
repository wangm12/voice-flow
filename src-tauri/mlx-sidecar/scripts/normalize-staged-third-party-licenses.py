#!/usr/bin/env python3
"""Normalize staged public license text modes using descriptor-relative traversal."""
import json
import os
import stat
import sys
from pathlib import Path

TEXT_SUFFIXES = {".json", ".md", ".mit", ".resolved", ".txt"}
TEXT_BASENAMES = {"license", "notice", "copying", "copyright", "patents"}


def fail(message):
    raise ValueError(message)


def absolute_canonical_path(value, label):
    path = Path(value)
    if not path.is_absolute() or Path(os.path.normpath(os.fspath(value))) != path:
        fail(f"{label} must be an absolute normalized path")
    if path == Path(path.anchor):
        fail(f"{label} cannot be a filesystem root")
    return path


def disjoint(left, right):
    return left != right and left not in right.parents and right not in left.parents


def text_name(name):
    path = Path(name)
    return path.suffix.lower() in TEXT_SUFFIXES or path.name.lower() in TEXT_BASENAMES


def directory_signature(info):
    return (info.st_dev, info.st_ino, stat.S_IFMT(info.st_mode))


def file_signature(info):
    return (
        info.st_dev,
        info.st_ino,
        stat.S_IFMT(info.st_mode),
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
        info.st_nlink,
    )


def require_descriptor_flags():
    if not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY"):
        fail("descriptor-relative no-follow directory traversal is unavailable")


def open_absolute_directory(path, label):
    """Open each absolute path component without following a symlink."""
    require_descriptor_flags()
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    fd = os.open(path.anchor, flags)
    try:
        for name in path.parts[1:]:
            before = os.stat(name, dir_fd=fd, follow_symlinks=False)
            if stat.S_ISLNK(before.st_mode):
                fail(f"{label} contains a symlink path component")
            if not stat.S_ISDIR(before.st_mode):
                fail(f"{label} contains a non-directory path component")
            child_fd = os.open(name, flags, dir_fd=fd)
            actual = os.fstat(child_fd)
            if not stat.S_ISDIR(actual.st_mode) or directory_signature(actual) != directory_signature(before):
                os.close(child_fd)
                fail(f"{label} changed while opening a path component")
            os.close(fd)
            fd = child_fd
        return fd
    except BaseException:
        os.close(fd)
        raise


def open_child_directory(parent_fd, name, expected_signature=None):
    before = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISLNK(before.st_mode):
        fail("staged license directory path changed to a symlink")
    if not stat.S_ISDIR(before.st_mode):
        fail("staged license directory changed type")
    before_signature = directory_signature(before)
    if expected_signature is not None and before_signature != expected_signature:
        fail("staged license directory identity changed")
    fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent_fd)
    actual = os.fstat(fd)
    if not stat.S_ISDIR(actual.st_mode) or directory_signature(actual) != before_signature:
        os.close(fd)
        fail("staged license directory changed while opening")
    return fd, before_signature


def open_directory_chain(root_fd, components):
    """Reopen a validated relative directory chain from the held stage root."""
    current_fd = os.dup(root_fd)
    try:
        for name, expected_signature in components:
            child_fd, _ = open_child_directory(current_fd, name, expected_signature)
            os.close(current_fd)
            current_fd = child_fd
        return current_fd
    except BaseException:
        os.close(current_fd)
        raise


def open_text_leaf(parent_fd, name, expected_signature=None):
    before = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    if stat.S_ISLNK(before.st_mode):
        fail("staged license text changed to a symlink")
    if not stat.S_ISREG(before.st_mode):
        fail("staged license text is not a regular file")
    signature = file_signature(before)
    if before.st_nlink != 1:
        fail("staged license text has aliases; refusing permission change")
    if expected_signature is not None and signature != expected_signature:
        fail("staged license text identity changed")
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=parent_fd)
    actual = os.fstat(fd)
    if not stat.S_ISREG(actual.st_mode) or file_signature(actual) != signature or actual.st_nlink != 1:
        os.close(fd)
        fail("staged license text changed while opening")
    return fd, signature


def validate_text_contents(fd):
    before = os.fstat(fd)
    while True:
        chunk = os.read(fd, 128 * 1024)
        if not chunk:
            break
        if b"\x00" in chunk:
            fail("staged license text contains a NUL byte")
    after = os.fstat(fd)
    if file_signature(before) != file_signature(after):
        fail("staged license text changed while being checked")


def collect_candidates(root_fd):
    candidates = []

    def walk(directory_fd, relative_dirs, directory_identities):
        for name in sorted(os.listdir(directory_fd)):
            before = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            if stat.S_ISLNK(before.st_mode):
                continue
            if stat.S_ISDIR(before.st_mode):
                child_fd, signature = open_child_directory(directory_fd, name)
                try:
                    walk(child_fd, relative_dirs + (name,), directory_identities + ((name, signature),))
                finally:
                    os.close(child_fd)
                continue
            if not text_name(name):
                continue
            if not stat.S_ISREG(before.st_mode):
                fail("staged license text is not a regular file")
            if before.st_nlink != 1:
                fail("staged license text has aliases; refusing permission change")
            fd, signature = open_text_leaf(directory_fd, name, file_signature(before))
            try:
                validate_text_contents(fd)
            finally:
                os.close(fd)
            candidates.append({
                "relative_dirs": relative_dirs,
                "directory_identities": directory_identities,
                "leaf_name": name,
                "file_signature": signature,
            })

    walk(root_fd, (), ())
    return candidates


def normalize(staged_root, source_root):
    stage = absolute_canonical_path(staged_root, "staged license root")
    source = absolute_canonical_path(source_root, "license source root")
    if not disjoint(stage, source):
        fail("staged and source license roots must be disjoint")

    source_fd = open_absolute_directory(source, "license source root")
    os.close(source_fd)
    stage_fd = open_absolute_directory(stage, "staged license root")
    try:
        root_signature = directory_signature(os.fstat(stage_fd))
        candidates = collect_candidates(stage_fd)
        normalized = 0
        for candidate in candidates:
            chain = candidate["directory_identities"]
            parent_fd = open_directory_chain(stage_fd, chain)
            leaf_fd = None
            rechecked_parent_fd = None
            try:
                leaf_fd, signature = open_text_leaf(
                    parent_fd,
                    candidate["leaf_name"],
                    candidate["file_signature"],
                )
                if signature != candidate["file_signature"]:
                    fail("staged license text identity changed before commit")

                # Rewalk from the held root after opening the leaf. This confirms
                # every ancestor still occupies the validated path immediately
                # before chmod; no absolute leaf open can follow a replaced parent.
                rechecked_parent_fd = open_directory_chain(stage_fd, chain)
                if directory_signature(os.fstat(rechecked_parent_fd)) != directory_signature(os.fstat(parent_fd)):
                    fail("staged license directory moved before commit")
                current = os.stat(candidate["leaf_name"], dir_fd=rechecked_parent_fd, follow_symlinks=False)
                if not stat.S_ISREG(current.st_mode) or file_signature(current) != candidate["file_signature"] or current.st_nlink != 1:
                    fail("staged license text moved or changed before commit")
                if directory_signature(os.fstat(stage_fd)) != root_signature:
                    fail("staged license root identity changed during normalization")
                os.fchmod(leaf_fd, 0o644)
                normalized += 1
            finally:
                if leaf_fd is not None:
                    os.close(leaf_fd)
                if rechecked_parent_fd is not None:
                    os.close(rechecked_parent_fd)
                os.close(parent_fd)
        return {"normalized_file_count": normalized, "mode": "0644"}
    finally:
        os.close(stage_fd)


def main(argv):
    if len(argv) != 3:
        print("usage: normalize-staged-third-party-licenses.py STAGED_ROOT SOURCE_ROOT", file=sys.stderr)
        return 2
    try:
        print(json.dumps(normalize(argv[1], argv[2]), sort_keys=True))
    except (OSError, ValueError) as error:
        print("license stage permission normalization failed: " + str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
