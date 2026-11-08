import importlib.util
import hashlib
import os
from pathlib import Path
import shutil
import stat
import tempfile
import unittest
from unittest.mock import patch

# Canonicalize macOS's /var/folders alias so no fixture path component is a symlink.
tempfile.tempdir = str(Path(tempfile.gettempdir()).resolve(strict=True))

HELPER = Path(__file__).resolve().parents[1] / "normalize-staged-third-party-licenses.py"
spec = importlib.util.spec_from_file_location("license_normalizer", HELPER)
normalizer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(normalizer)


class NormalizeStagedLicensesTests(unittest.TestCase):
    def test_readonly_source_becomes_writable_copy_and_symlink_target_is_untouched(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve(strict=True)
            source = root / "source" / "ThirdPartyLicenses"
            staged = root / "stage" / "ThirdPartyLicenses"
            outside = root / "outside"
            executable = root / "stage" / "voiceflow-mlx-sidecar"
            source.mkdir(parents=True)
            staged.mkdir(parents=True)
            body = b"MIT fixture notice\nCopyright (c) 2026 Example\n"
            src = source / "LICENSE.txt"
            src.write_bytes(body)
            src.chmod(0o444)
            shutil.copy2(src, staged / "LICENSE.txt")
            lock_source = source / "Package.resolved"
            lock_source.write_bytes(b"{\"pins\":[]}\n")
            lock_source.chmod(0o444)
            shutil.copy2(lock_source, staged / "Package.resolved")
            outside.write_bytes(b"untouched foreign file\n")
            outside.chmod(0o444)
            os.symlink(outside, staged / "NOTICE.txt")
            executable.write_bytes(b"executable fixture\n")
            executable.chmod(0o755)

            source_digest = hashlib.sha256(src.read_bytes()).hexdigest()
            foreign_before = (hashlib.sha256(outside.read_bytes()).hexdigest(), outside.stat().st_mode & 0o777)
            first = normalizer.normalize(staged, source)
            copied = staged / "LICENSE.txt"
            self.assertEqual(first["normalized_file_count"], 2)
            self.assertEqual(copied.stat().st_mode & 0o777, 0o644)
            self.assertEqual(copied.read_bytes(), body)
            self.assertEqual((staged / "Package.resolved").stat().st_mode & 0o777, 0o644)
            self.assertEqual((staged / "Package.resolved").read_bytes(), b"{\"pins\":[]}\n")
            self.assertEqual(hashlib.sha256(src.read_bytes()).hexdigest(), source_digest)
            self.assertEqual(src.stat().st_mode & 0o777, 0o444)
            self.assertTrue((staged / "NOTICE.txt").is_symlink())
            self.assertEqual((hashlib.sha256(outside.read_bytes()).hexdigest(), outside.stat().st_mode & 0o777), foreign_before)
            self.assertEqual(executable.stat().st_mode & 0o777, 0o755)

            copied.write_bytes(b"updated generated stage text\n")
            self.assertEqual(copied.read_bytes(), b"updated generated stage text\n")
            second = normalizer.normalize(staged, source)
            self.assertEqual(second["normalized_file_count"], 2)
            self.assertEqual(copied.stat().st_mode & 0o777, 0o644)
            self.assertEqual((staged / "Package.resolved").stat().st_mode & 0o777, 0o644)
            self.assertEqual(copied.read_bytes(), b"updated generated stage text\n")
            self.assertEqual((hashlib.sha256(outside.read_bytes()).hexdigest(), outside.stat().st_mode & 0o777), foreign_before)
            self.assertEqual(executable.stat().st_mode & 0o777, 0o755)

    def test_refuses_overlapping_source_and_stage_roots(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve(strict=True)
            licenses = root / "ThirdPartyLicenses"
            licenses.mkdir()
            with self.assertRaises(ValueError):
                normalizer.normalize(licenses, licenses)

    def test_binary_named_as_license_is_not_chmodded(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve(strict=True)
            source = root / "source" / "ThirdPartyLicenses"
            staged = root / "stage" / "ThirdPartyLicenses"
            source.mkdir(parents=True)
            staged.mkdir(parents=True)
            valid_first = staged / "A.txt"
            valid_first.write_bytes(b"ordinary license text\n")
            valid_first.chmod(0o444)
            binary = staged / "LICENSE.txt"
            binary.write_bytes(b"\x00not text")
            binary.chmod(0o444)
            with self.assertRaisesRegex(ValueError, "NUL byte"):
                normalizer.normalize(staged, source)
            self.assertEqual(valid_first.stat().st_mode & 0o777, 0o444)
            self.assertEqual(binary.stat().st_mode & 0o777, 0o444)


if __name__ == "__main__":
    unittest.main()


class DescriptorRelativeIndependentTests(unittest.TestCase):
    def roots(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        source = root / "source"
        stage = root / "stage"
        source.mkdir()
        stage.mkdir()
        return root, source, stage

    def test_static_directory_symlink_is_skipped(self):
        root, source, stage = self.roots()
        outside = root / "outside"
        outside.mkdir()
        leaf = outside / "LICENSE.txt"
        leaf.write_bytes(b"foreign notice\\n")
        leaf.chmod(0o444)
        (stage / "linked").symlink_to(outside, target_is_directory=True)
        self.assertEqual(normalizer.normalize(stage, source)["normalized_file_count"], 0)
        self.assertEqual(stat.S_IMODE(leaf.stat().st_mode), 0o444)

    def test_foreign_source_hardlink_is_rejected_without_chmod(self):
        root, source, stage = self.roots()
        original = source / "LICENSE.txt"
        original.write_bytes(b"shared inode\\n")
        original.chmod(0o444)
        os.link(original, stage / "LICENSE.txt")
        with self.assertRaisesRegex(ValueError, "aliases"):
            normalizer.normalize(stage, source)
        self.assertEqual(stat.S_IMODE(original.stat().st_mode), 0o444)

    def test_root_symlink_is_rejected(self):
        root, source, stage = self.roots()
        alias = root / "alias"
        alias.symlink_to(stage, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            normalizer.normalize(alias, source)

    def test_nested_roots_are_rejected(self):
        root, source, stage = self.roots()
        nested = source / "nested"
        nested.mkdir()
        with self.assertRaisesRegex(ValueError, "disjoint"):
            normalizer.normalize(nested, source)

    def test_unrecognized_payload_inside_stage_is_untouched(self):
        root, source, stage = self.roots()
        dylib = stage / "example.dylib"
        dylib.write_bytes(b"\\x00binary fixture")
        dylib.chmod(0o755)
        self.assertEqual(normalizer.normalize(stage, source)["normalized_file_count"], 0)
        self.assertEqual(stat.S_IMODE(dylib.stat().st_mode), 0o755)

    def _swap_on_second_leaf_open(self, leaf, swap):
        real_open = os.open
        leaf_name = leaf.name
        parent = leaf.parent
        parent_stat = parent.stat()
        observed = {"leaf_open_count": 0}
        def open_with_swap(path, flags, *args, **kwargs):
            dir_fd = kwargs.get("dir_fd")
            if path == leaf_name and dir_fd is not None:
                current_parent = os.fstat(dir_fd)
                if (current_parent.st_dev, current_parent.st_ino) == (parent_stat.st_dev, parent_stat.st_ino):
                    observed["leaf_open_count"] += 1
                    if observed["leaf_open_count"] == 2:
                        swap()
            return real_open(path, flags, *args, **kwargs)
        return open_with_swap, observed

    def test_different_inode_leaf_replacement_is_rejected(self):
        root, source, stage = self.roots()
        leaf = stage / "LICENSE.txt"
        leaf.write_bytes(b"initial notice\\n")
        leaf.chmod(0o444)
        replacement = root / "replacement.txt"
        replacement.write_bytes(b"replacement\\n")
        replacement.chmod(0o444)
        open_with_swap, observed = self._swap_on_second_leaf_open(leaf, lambda: replacement.replace(leaf))
        with patch.object(normalizer.os, "open", open_with_swap):
            with self.assertRaisesRegex(ValueError, "identity changed|changed while opening"):
                normalizer.normalize(stage, source)
        self.assertEqual(observed["leaf_open_count"], 2)
        self.assertEqual(stat.S_IMODE(leaf.stat().st_mode), 0o444)

    def test_ancestor_directory_symlink_swap_does_not_chmod_outside_stage(self):
        root, source, stage = self.roots()
        child = stage / "nested"
        child.mkdir()
        leaf = child / "LICENSE.txt"
        leaf.write_bytes(b"validated notice\\n")
        leaf.chmod(0o444)
        outside = root / "moved-outside-stage"
        def swap():
            child.rename(outside)
            child.symlink_to(outside, target_is_directory=True)
        open_with_swap, observed = self._swap_on_second_leaf_open(leaf, swap)
        caught = None
        with patch.object(normalizer.os, "open", open_with_swap):
            try:
                normalizer.normalize(stage, source)
            except (OSError, ValueError) as error:
                caught = str(error)
        self.assertEqual(observed["leaf_open_count"], 2)
        self.assertIsNotNone(caught)
        self.assertEqual(stat.S_IMODE((outside / "LICENSE.txt").stat().st_mode), 0o444)
        self.assertTrue(child.is_symlink())

    def test_same_name_directory_replacement_is_rejected(self):
        root, source, stage = self.roots()
        child = stage / "nested"
        child.mkdir()
        original_leaf = child / "LICENSE.txt"
        original_leaf.write_bytes(b"original notice\\n")
        original_leaf.chmod(0o444)
        replacement_dir = root / "replacement-dir"
        replacement_dir.mkdir()
        replacement_leaf = replacement_dir / "LICENSE.txt"
        replacement_leaf.write_bytes(b"replacement notice\\n")
        replacement_leaf.chmod(0o444)
        moved = root / "moved-original-dir"
        def swap():
            child.rename(moved)
            replacement_dir.rename(child)
        open_with_swap, observed = self._swap_on_second_leaf_open(original_leaf, swap)
        with patch.object(normalizer.os, "open", open_with_swap):
            with self.assertRaisesRegex(ValueError, "directory identity changed"):
                normalizer.normalize(stage, source)
        self.assertEqual(observed["leaf_open_count"], 2)
        self.assertEqual(stat.S_IMODE((moved / "LICENSE.txt").stat().st_mode), 0o444)
        self.assertEqual(stat.S_IMODE((child / "LICENSE.txt").stat().st_mode), 0o444)
