"""No-network production-boundary faults. Execute only in independent verification.

Fixtures are synthetic tool inputs and byte streams, never human-review labels.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import sys
import struct
from pathlib import Path
import tempfile
import wave
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("public_corpus", Path(__file__).with_name("prepare-public-corpus.py"))
TOOL = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = TOOL
SPEC.loader.exec_module(TOOL)
PIN = "737e9800ae31be9932ba8464c80366559bd28424"
SOURCE = "https://huggingface.co/datasets/CAiRE/ASCEND"


class Response(io.BytesIO):
    def __init__(self, data=b"fixture bytes", on_read=None):
        super().__init__(data)
        self.headers = {"Content-Length": str(len(data))}
        self.on_read = on_read

    def read(self, size=-1):
        if self.on_read is not None:
            callback, self.on_read = self.on_read, None
            callback()
        return super().read(size)


class PublicCorpusFaults(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="voiceflow-public-tool-")
        self.root = Path(self.directory.name).resolve()
        self.output = self.root / "out"
        self.output.mkdir()
        self.plan = {"artifact_type": "public_acquisition_plan", "schema_version": 1, "source": SOURCE, "revision": PIN, "license": "CC-BY-SA-4.0", "attribution": "synthetic acquisition boundary fixture", "files": [{"path": "README.md", "url": f"{SOURCE}/resolve/{PIN}/README.md", "publisher_sha256": None, "max_bytes": 1024}]}

    def tearDown(self):
        # Explicitly release only handles/resources this fixture created. Do not
        # let cleanup-failure cases leave open descriptors in the verifier.
        for owned in list(TOOL.RETAINED_OUTPUTS):
            owned.release_handles()
            TOOL.RETAINED_OUTPUTS.remove(owned)
            if not owned.root.closed:
                owned.root.release_handles()
        for root in list(TOOL.RETAINED_ROOTS):
            root.release_handles()
        self.directory.cleanup()

    def args(self, dry_run=False):
        path = self.root / "plan.json"
        path.write_text(json.dumps(self.plan))
        return argparse.Namespace(plan=str(path), out_dir=str(self.output), max_file_bytes=1024, max_total_bytes=4096, deadline_seconds=10, dry_run=dry_run)

    def fetch(self, response=None, error=None, dry_run=False):
        with patch.object(TOOL.urllib.request, "urlopen", side_effect=error, return_value=response) as transport, contextlib.redirect_stdout(io.StringIO()):
            TOOL.fetch(self.args(dry_run=dry_run))
        return transport.call_count

    def test_foreign_partial_preserved_on_transport_failure(self):
        partial = self.output / "README.md.partial"
        partial.write_bytes(b"foreign partial")
        with self.assertRaisesRegex(OSError, "transport failure"):
            self.fetch(error=OSError("no-network transport failure"))
        self.assertEqual(partial.read_bytes(), b"foreign partial")
        self.assertEqual(list(self.output.iterdir()), [partial])

    def test_owned_anonymous_stage_removed_on_transport_failure(self):
        with self.assertRaisesRegex(OSError, "transport failure"):
            self.fetch(error=OSError("transport failure"))
        self.assertEqual(list(self.output.iterdir()), [])

    def test_existing_destination_preserved_without_network(self):
        target = self.output / "README.md"
        target.write_bytes(b"caller target")
        with patch.object(TOOL.urllib.request, "urlopen") as transport:
            with self.assertRaisesRegex(ValueError, "collision"):
                TOOL.fetch(self.args())
        self.assertEqual(transport.call_count, 0)
        self.assertEqual(target.read_bytes(), b"caller target")

    def test_concurrent_destination_publish_preserved(self):
        target = self.output / "README.md"
        with self.assertRaises(FileExistsError):
            self.fetch(Response(on_read=lambda: target.write_bytes(b"concurrent target")))
        self.assertEqual(target.read_bytes(), b"concurrent target")
        self.assertFalse((self.output / "acquisition.json").exists())

    def test_old_check_to_mutation_names_are_never_used(self):
        target = self.output / "writer.json"
        partial = self.output / "writer.json.partial"
        owned = TOOL.OwnedOutput(target)
        owned.stream.write(b"owned bytes")
        # Interpose exactly where old name mutations could delete/link a newly
        # substituted public stage. New commit/cleanup must never call either.
        partial.write_bytes(b"foreign substitute")
        with patch.object(TOOL.os, "link", side_effect=AssertionError("name link forbidden")) as link, patch.object(TOOL.os, "unlink", side_effect=AssertionError("name unlink forbidden")) as unlink:
            owned.commit(hashlib.sha256(b"owned bytes").hexdigest(), len(b"owned bytes"))
        self.assertEqual(link.call_count, 0)
        self.assertEqual(unlink.call_count, 0)
        self.assertEqual(partial.read_bytes(), b"foreign substitute")
        self.assertEqual(target.read_bytes(), b"owned bytes")

    def test_namespace_move_retains_owner_and_foreign_contents(self):
        def move():
            namespace = next(self.output.glob(".voiceflow-stage-*"))
            namespace.rename(self.output / "moved-owned-namespace")
            namespace.mkdir()
            (namespace / "foreign").write_bytes(b"foreign replacement")
        with self.assertRaisesRegex(TOOL.OutputCleanupError, "retain"):
            self.fetch(Response(on_read=move))
        self.assertEqual((self.output / "README.md").read_bytes(), b"fixture bytes")
        self.assertEqual(len(TOOL.RETAINED_OUTPUTS), 1)
        self.assertIsNotNone(TOOL.RETAINED_OUTPUTS[0].namespace_fd)
        self.assertIsNotNone(TOOL.RETAINED_OUTPUTS[0].stream)
        self.assertTrue(any((directory / "foreign").exists() for directory in self.output.glob(".voiceflow-stage-*")))

    def test_cleanup_failure_retains_handles_until_explicit_recovery(self):
        owned = TOOL.OwnedOutput(self.output / "out.bin")
        with patch.object(TOOL.os, "rmdir", side_effect=PermissionError("fixture cleanup denied")):
            with self.assertRaisesRegex(TOOL.OutputCleanupError, "cleanup denied"):
                owned.cleanup()
        self.assertEqual(len(TOOL.RETAINED_OUTPUTS), 1)
        self.assertIsNotNone(owned.namespace_fd)
        self.assertIsNotNone(owned.stream)
        owned.cleanup()
        self.assertEqual(TOOL.RETAINED_OUTPUTS, [])

    def test_successful_stub_commit_records_bound_source_and_bytes(self):
        data = b"fixture bytes"
        self.plan["files"][0]["publisher_sha256"] = hashlib.sha256(data).hexdigest()
        self.assertEqual(self.fetch(Response(data)), 1)
        receipt = json.loads((self.output / "acquisition.json").read_text())
        self.assertEqual(receipt["files"][0]["source_identity"]["revision"], PIN)
        self.assertEqual(receipt["files"][0]["actual_sha256"], hashlib.sha256(data).hexdigest())
        self.assertEqual((self.output / "README.md").read_bytes(), data)

    def test_failed_publication_keeps_owned_descriptor_without_unlink(self):
        target = self.output / "writer.json"
        with patch.object(TOOL.os, "fsync", side_effect=OSError("fixture fsync failure")):
            with self.assertRaisesRegex(TOOL.OutputCleanupError, "publication incomplete"):
                TOOL.write_once(target, b"content")
        self.assertTrue(target.exists())
        self.assertEqual(len(TOOL.RETAINED_OUTPUTS), 1)
        self.assertIsNotNone(TOOL.RETAINED_OUTPUTS[0].published_stream)

    def test_edited_owned_stage_is_not_foreign_identity(self):
        target = self.output / "writer.json"
        owned = TOOL.OwnedOutput(target)
        owned.stream.write(b"changed")
        with self.assertRaisesRegex(ValueError, "staged bytes changed") as raised:
            owned.commit(hashlib.sha256(b"intended").hexdigest(), len(b"intended"))
        with self.assertRaises(ValueError):
            owned.abort(raised.exception)
        self.assertFalse(target.exists())
        self.assertEqual(TOOL.RETAINED_OUTPUTS, [])

    def test_publication_uses_validated_bytes_not_later_descriptor_edit(self):
        target = self.output / "stable.bin"
        owned = TOOL.OwnedOutput(target)
        intended = b"intended"
        owned.stream.write(intended)
        actual_open = TOOL.os.open
        def edit_before_destination_open(name, *args, **kwargs):
            if name == target.name:
                owned.stream.seek(0)
                owned.stream.write(b"edited!!")
                owned.stream.flush()
            return actual_open(name, *args, **kwargs)
        with patch.object(TOOL.os, "open", side_effect=edit_before_destination_open):
            owned.commit(hashlib.sha256(intended).hexdigest(), len(intended))
        self.assertEqual(target.read_bytes(), intended)

    def test_writer_ignores_foreign_legacy_stage(self):
        target = self.output / "writer.json"
        partial = self.output / "writer.json.partial"
        partial.write_bytes(b"foreign")
        TOOL.write_once(target, b"new")
        self.assertEqual(partial.read_bytes(), b"foreign")
        self.assertEqual(target.read_bytes(), b"new")

    def test_interrupt_cleanup_keeps_explicit_failure(self):
        def interrupt():
            raise KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt):
            self.fetch(Response(on_read=interrupt))
        self.assertEqual(list(self.output.iterdir()), [])

    def test_plan_replacement_does_not_change_receipt_authority(self):
        args = self.args()
        original = Path(args.plan).read_bytes()
        with patch.object(TOOL.urllib.request, "urlopen", return_value=Response(on_read=lambda: Path(args.plan).write_bytes(b"pending replacement"))), contextlib.redirect_stdout(io.StringIO()):
            TOOL.fetch(args)
        receipt = json.loads((self.output / "acquisition.json").read_text())
        self.assertEqual(receipt["plan_sha256"], hashlib.sha256(original).hexdigest())

    def test_response_move_of_selected_root_rejects_success_and_retains_stage(self):
        moved = self.root / "moved-root"
        def replace_root():
            self.output.rename(moved)
            self.output.mkdir()
            (self.output / "README.md").write_bytes(b"foreign visible target")
        with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
            self.fetch(Response(on_read=replace_root))
        self.assertEqual((self.output / "README.md").read_bytes(), b"foreign visible target")
        self.assertFalse((self.output / "acquisition.json").exists())
        self.assertFalse((moved / "acquisition.json").exists())
        self.assertEqual(len(TOOL.RETAINED_ROOTS), 1)
        self.assertEqual(TOOL.RETAINED_ROOTS[0].applicability, "unknown")
        retained = TOOL.RETAINED_OUTPUTS[0]
        retained.stream.seek(0)
        self.assertEqual(retained.stream.read(), b"fixture bytes")
        self.assertIsNone(retained.destination_identity)

    def test_nested_parent_move_uses_same_applicability_owner(self):
        self.plan["files"][0].update(path="main/README.md", url=f"{SOURCE}/resolve/{PIN}/main/README.md")
        def replace_nested():
            (self.output / "main").rename(self.output / "moved-main")
            (self.output / "main").mkdir()
            (self.output / "main/README.md").write_bytes(b"foreign nested target")
        with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
            self.fetch(Response(on_read=replace_nested))
        self.assertEqual((self.output / "main/README.md").read_bytes(), b"foreign nested target")
        self.assertFalse((self.output / "acquisition.json").exists())
        self.assertEqual(TOOL.RETAINED_ROOTS[0].applicability, "unknown")

    def test_batch_retains_prior_publication_and_blocks_fresh_receipt_root(self):
        self.plan["files"].append({"path": "LICENSE", "url": f"{SOURCE}/resolve/{PIN}/LICENSE", "publisher_sha256": None, "max_bytes": 1024})
        moved = self.root / "moved-batch"
        def replace_root():
            self.output.rename(moved)
            self.output.mkdir()
            (self.output / "README.md").write_bytes(b"foreign first")
        responses = [Response(b"owned first"), Response(b"owned second", on_read=replace_root)]
        with patch.object(TOOL.urllib.request, "urlopen", side_effect=responses), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
                TOOL.fetch(self.args())
        self.assertEqual((moved / "README.md").read_bytes(), b"owned first")
        self.assertEqual((self.output / "README.md").read_bytes(), b"foreign first")
        self.assertFalse((self.output / "acquisition.json").exists())
        self.assertFalse((moved / "acquisition.json").exists())
        root = TOOL.RETAINED_ROOTS[0]
        self.assertEqual(len(root.resources), 2)
        self.assertIsNotNone(root.resources[0].published_stream)

    def test_write_once_batch_refuses_root_rebind_between_outputs(self):
        moved = self.root / "moved-writer-batch"
        with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
            with TOOL.OutputRoot(self.output):
                TOOL.write_once(self.output / "first.json", b"owned first")
                self.output.rename(moved)
                self.output.mkdir()
                (self.output / "first.json").write_bytes(b"foreign")
                TOOL.write_once(self.output / "receipt.json", b"must not publish")
        self.assertEqual((moved / "first.json").read_bytes(), b"owned first")
        self.assertEqual((self.output / "first.json").read_bytes(), b"foreign")
        self.assertFalse((self.output / "receipt.json").exists())

    def test_root_move_after_receipt_write_still_rejects_batch_completion(self):
        moved = self.root / "moved-after-receipt"
        original_write = TOOL.write_once
        def move_after_receipt(path, data):
            result = original_write(path, data)
            if path.name == "acquisition.json":
                self.output.rename(moved)
                self.output.mkdir()
                (self.output / "README.md").write_bytes(b"foreign after receipt")
            return result
        with patch.object(TOOL, "write_once", side_effect=move_after_receipt):
            with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
                self.fetch(Response())
        self.assertEqual((self.output / "README.md").read_bytes(), b"foreign after receipt")
        self.assertFalse((self.output / "acquisition.json").exists())
        # The retained old receipt is an incomplete batch artifact, not an
        # acknowledgement at the selected root; both descriptor owners survive.
        self.assertTrue((moved / "acquisition.json").exists())
        self.assertEqual(len(TOOL.RETAINED_ROOTS[0].resources), 2)

    def test_import_batch_cannot_publish_metadata_into_replacement_root(self):
        args = self.review_fixture()
        output = Path(args.out_dir)
        moved = self.root / "moved-import"
        original_write = TOOL.write_once
        def move_after_first(path, data):
            result = original_write(path, data)
            if path.name == "case.wav":
                output.rename(moved)
                output.mkdir()
                (output / "case.wav").write_bytes(b"foreign import audio")
            return result
        with patch.object(TOOL, "write_once", side_effect=move_after_first):
            with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
                TOOL.import_review(args)
        self.assertEqual((output / "case.wav").read_bytes(), b"foreign import audio")
        self.assertFalse((output / "speech.json").exists())
        self.assertFalse((output / "source-provenance.json").exists())
        self.assertTrue((moved / "case.wav").exists())

    def test_transport_failure_after_root_move_retains_owned_stage(self):
        moved = self.root / "moved-failed-transport"
        def failing_read():
            self.output.rename(moved)
            self.output.mkdir()
            (self.output / "README.md.partial").write_bytes(b"foreign stage")
            raise OSError("no-network response failure after move")
        with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
            self.fetch(Response(on_read=failing_read))
        self.assertEqual((self.output / "README.md.partial").read_bytes(), b"foreign stage")
        self.assertEqual(len(TOOL.RETAINED_OUTPUTS), 1)
        self.assertIsNotNone(TOOL.RETAINED_OUTPUTS[0].stream)
        self.assertFalse((self.output / "acquisition.json").exists())

    def test_prepare_batch_cannot_publish_candidates_into_replacement_root(self):
        args = self.ascend_fixture()
        output = Path(args.out_dir)
        moved = self.root / "moved-preparation"
        original_write = TOOL.write_once
        def move_after_creator(path, data):
            result = original_write(path, data)
            if path.name == "preparation-tool.py":
                output.rename(moved)
                output.mkdir()
                (output / "preparation-tool.py").write_bytes(b"foreign creator")
            return result
        with patch.object(TOOL, "write_once", side_effect=move_after_creator):
            with self.assertRaisesRegex(TOOL.OutputCleanupError, "applicability=unknown"):
                TOOL.prepare_ascend(args)
        self.assertEqual((output / "preparation-tool.py").read_bytes(), b"foreign creator")
        self.assertFalse((output / "candidates.json").exists())
        self.assertFalse((moved / "candidates.json").exists())
        self.assertIsNotNone(TOOL.RETAINED_ROOTS[0].resources[0].published_stream)

    def test_snapshot_retains_raw_parse_and_hash_after_path_replacement(self):
        path = self.root / "snapshot.json"
        raw = b'{"value":"first"}'
        path.write_bytes(raw)
        snapshot = TOOL.InputSnapshot.json(path)
        path.write_bytes(b'{"value":"second"}')
        self.assertEqual(snapshot.raw, raw)
        self.assertEqual(snapshot.parsed, {"value": "first"})
        self.assertEqual(snapshot.digest, hashlib.sha256(raw).hexdigest())
        with self.assertRaises(TypeError):
            snapshot.parsed["value"] = "mutated parsed authority"

    def test_growing_input_reads_only_cap_plus_one(self):
        path = self.root / "growing.json"
        path.write_bytes(b"small")
        actual_open = Path.open
        reads = []
        class GrowingStream:
            def __init__(self, stream): self.stream = stream
            def __enter__(self): return self
            def __exit__(self, *args): self.stream.close()
            def fileno(self): return self.stream.fileno()
            def read(self, size):
                reads.append(size)
                with actual_open(path, "ab") as append: append.write(b"x" * 1024)
                return self.stream.read(size)
        def selected_open(selected, *args, **kwargs):
            stream = actual_open(selected, *args, **kwargs)
            return GrowingStream(stream) if selected == path and args == ("rb",) else stream
        with patch.object(Path, "open", new=selected_open):
            with self.assertRaisesRegex(ValueError, "byte cap"):
                TOOL.InputSnapshot.capture(path, 32)
        self.assertEqual(reads, [33])

    def review_fixture(self):
        # Synthetic contract stand-in only, never a project human-reviewed label.
        buffer = io.BytesIO()
        with wave.open(buffer, "wb") as writer:
            writer.setnchannels(1); writer.setsampwidth(2); writer.setframerate(16000)
            writer.writeframes(b"\0\0" * 320)
        audio = buffer.getvalue()
        (self.root / "case.wav").write_bytes(audio)
        identity = TOOL.wav_identity(audio)
        candidate = {"artifact_type": "public_speech_candidates", "schema_version": 1, "source": SOURCE, "revision": PIN, "license": "CC-BY-SA-4.0", "attribution": "synthetic contract stand-in, not public human source", "cases": [{"case_id": "case", "adjacent_wav": "case.wav", "original_speaker_id": 7, "language": "en", "audio": identity, "conditions": {"noise": "synthetic fixture", "microphone": "generated", "speaking_style": "synthetic fixture"}}]}
        candidates = self.root / "candidates.json"
        candidates.write_bytes(TOOL.json_bytes(candidate))
        row = {"case_id": "case", "audio_sha256": identity["audio_sha256"], "pcm_sha256": identity["pcm_sha256"], "status": "reviewed", "transcript_verbatim": "fixture", "expected_final": "fixture", "protected": [], "reference_variants": [], "authorized_changes": [], "intent_evidence": "synthetic fixture only", "reviewer": "test-stand-in-not-human-review", "reviewed_at": "2026-10-14T00:00:00Z", "annotation_revision": "fixture-v1", "adjudication": "resolved", "speaker_id": "ascend:spk7", "source_session_id": "fixture-session", "template_family_id": "fixture-template", "split": "dev"}
        label = {"artifact_type": "reviewed_public_speech_labels", "schema_version": 1, "candidates_sha256": hashlib.sha256(candidates.read_bytes()).hexdigest(), "rights_review_id": "fixture-rights", "group_review_id": "fixture-groups", "cases": [row]}
        labels = self.root / "labels.json"
        labels.write_bytes(TOOL.json_bytes(label))
        return argparse.Namespace(candidates=str(candidates), labels=str(labels), out_dir=str(self.root / "import"), dry_run=False)

    def test_import_retains_L1_when_labels_and_audio_path_become_L2(self):
        args = self.review_fixture()
        original = Path(args.labels).read_bytes()
        original_audio = (self.root / "case.wav").read_bytes()
        actual_identity = TOOL.wav_identity
        def change_after_capture(data):
            Path(args.labels).write_bytes(b'{"status":"pending"}')
            (self.root / "case.wav").write_bytes(b"replacement audio")
            return actual_identity(data)
        with patch.object(TOOL, "wav_identity", side_effect=change_after_capture), contextlib.redirect_stdout(io.StringIO()):
            TOOL.import_review(args)
        output = Path(args.out_dir)
        self.assertEqual((output / "reviewed-labels.json").read_bytes(), original)
        self.assertEqual((output / "case.wav").read_bytes(), original_audio)
        provenance = json.loads((output / "source-provenance.json").read_text())
        self.assertEqual(provenance["labels_sha256"], hashlib.sha256(original).hexdigest())

    def test_candidate_C1_cannot_validate_under_hash_C2(self):
        args = self.review_fixture()
        candidate_path = Path(args.candidates)
        changed = candidate_path.read_bytes() + b" "
        labels = json.loads(Path(args.labels).read_bytes())
        labels["candidates_sha256"] = hashlib.sha256(changed).hexdigest()
        Path(args.labels).write_bytes(TOOL.json_bytes(labels))
        original_json = TOOL.InputSnapshot.json
        def capture_and_change(path, cap=4 * TOOL.MIB):
            result = original_json(path, cap)
            if path == candidate_path: candidate_path.write_bytes(changed)
            return result
        with patch.object(TOOL.InputSnapshot, "json", side_effect=capture_and_change):
            with self.assertRaisesRegex(ValueError, "identity mismatch"):
                TOOL.import_review(args)
        self.assertFalse(Path(args.out_dir).exists())

    def ascend_fixture(self):
        import pyarrow as pa
        import pyarrow.parquet as pq
        root = self.root / "synthetic-ascend-shape"
        (root / "main").mkdir(parents=True)
        entries = []
        for split, count, offset in (("validation", 60, 0), ("test", 1, 100)):
            rows = []
            for index in range(count):
                audio = io.BytesIO()
                with wave.open(audio, "wb") as writer:
                    writer.setnchannels(1)
                    writer.setsampwidth(2)
                    writer.setframerate(16000)
                    writer.writeframes(struct.pack("<h", offset + index + 1) * 32000)
                name = f"ses1_spk7_L{offset + index}_{index * 2}.0000_2.0000.wav"
                rows.append({"id": str(index).zfill(5), "path": f"/synthetic-fixture/{name}", "audio": {"bytes": audio.getvalue(), "path": name}, "transcription": "synthetic fixture only", "duration": 2.0, "language": ("zh", "en", "mixed")[index % 3], "original_speaker_id": 7, "session_id": 1, "topic": "synthetic shape fixture"})
            relative = f"main/{split}-00000-of-00001.parquet"
            pq.write_table(pa.Table.from_pylist(rows), root / relative)
            digest = hashlib.sha256((root / relative).read_bytes()).hexdigest()
            entries.append({"path": relative, "actual_sha256": digest, "publisher_sha256": digest})
        receipt = {"source": SOURCE, "revision": PIN, "license": "synthetic contract stand-in", "attribution": "not actual human/public dataset acquisition", "files": entries}
        (root / "acquisition.json").write_bytes(TOOL.json_bytes(receipt))
        return argparse.Namespace(source_dir=str(root), out_dir=str(self.root / "prepared-synthetic"), max_file_bytes=12 * TOOL.MIB, max_total_bytes=24 * TOOL.MIB, max_rows=100, max_row_group_bytes=12 * TOOL.MIB, deadline_seconds=120, dry_run=False)

    def test_replayed_selected_audio_mutation_rejects_before_any_output(self):
        args = self.ascend_fixture()
        rows = TOOL.parquet_rows
        validation_passes = 0
        def changed_replay(snapshot, controls):
            nonlocal validation_passes
            if snapshot.path.name.startswith("validation"):
                validation_passes += 1
            for ordinal, row in enumerate(rows(snapshot, controls)):
                if validation_passes == 2 and snapshot.path.name.startswith("validation") and ordinal == 0:
                    row["audio"]["bytes"] = row["audio"]["bytes"][:-2] + b"\x7f\x00"
                yield row
        with patch.object(TOOL, "parquet_rows", side_effect=changed_replay):
            with self.assertRaisesRegex(ValueError, "replay source/audio identity mismatch"):
                TOOL.prepare_ascend(args)
        self.assertFalse(Path(args.out_dir).exists())

    def test_parquet_path_replacement_does_not_replace_captured_W1(self):
        args = self.ascend_fixture()
        rows = TOOL.parquet_rows
        validation_passes = 0
        original_source_sha = hashlib.sha256((Path(args.source_dir) / "main/validation-00000-of-00001.parquet").read_bytes()).hexdigest()
        def replace_path(snapshot, controls):
            nonlocal validation_passes
            if snapshot.path.name.startswith("validation"):
                validation_passes += 1
                if validation_passes == 2:
                    snapshot.path.write_bytes(b"replaced source path after capture")
            yield from rows(snapshot, controls)
        with patch.object(TOOL, "parquet_rows", side_effect=replace_path), contextlib.redirect_stdout(io.StringIO()):
            TOOL.prepare_ascend(args)
        output = Path(args.out_dir)
        candidates = json.loads((output / "candidates.json").read_text())
        self.assertEqual(len(candidates["cases"]), 60)
        for row in candidates["cases"]:
            self.assertEqual(row["source_parquet_sha256"], original_source_sha)
            self.assertEqual(hashlib.sha256((output / row["adjacent_wav"]).read_bytes()).hexdigest(), row["audio"]["audio_sha256"])

    def test_pin_mismatch_and_escaped_paths_reject_before_io(self):
        invalid = [f"{SOURCE}/resolve/main/README.md", f"{SOURCE}/resolve/{'1' * 40}/README.md", f"https://huggingface.co/datasets/other/ASCEND/resolve/{PIN}/README.md", f"{SOURCE}/resolve/{PIN}/a%2fREADME.md", f"{SOURCE}/resolve/{PIN}/%2e%2e/README.md", f"{SOURCE}/resolve/{PIN}/%252e%252e/README.md", f"{SOURCE}/resolve/{PIN}/README.md%5c", f"{SOURCE}/resolve/{PIN}/%zz"]
        for url in invalid:
            with self.subTest(url=url), patch.object(TOOL.urllib.request, "urlopen") as transport:
                self.plan["files"][0]["url"] = url
                with self.assertRaises(ValueError):
                    TOOL.fetch(self.args(dry_run=True))
                self.assertEqual(transport.call_count, 0)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_valid_escaped_filename_preserves_requested_url(self):
        self.plan["files"][0]["url"] = f"{SOURCE}/resolve/{PIN}/%52EADME.md"
        binding = TOOL.bind_source_identity(self.plan, self.plan["files"][0])
        self.assertEqual(binding["remote_path"], "README.md")
        self.assertEqual(binding["requested_url"], self.plan["files"][0]["url"])
        self.assertEqual(self.fetch(dry_run=True), 0)

    def test_tree_binds_remote_directory_and_preserves_local_alias(self):
        self.plan["files"][0].update(path="tree.json", url=f"https://huggingface.co/api/datasets/CAiRE/ASCEND/tree/{PIN}/main")
        binding = TOOL.bind_source_identity(self.plan, self.plan["files"][0])
        self.assertEqual(binding["remote_path"], "main")
        self.assertEqual(binding["local_path"], "tree.json")
        self.assertEqual(self.fetch(dry_run=True), 0)
        self.plan["files"][0]["url"] = "https://huggingface.co/api/datasets/CAiRE/ASCEND/tree/main/main"
        with self.assertRaisesRegex(ValueError, "revision mismatch"):
            self.fetch(dry_run=True)

    def test_other_official_observation_has_unknown_repo_revision(self):
        self.plan.update(source="https://creativecommons.org/licenses/by-sa/4.0/legalcode.en", revision="observed text date label")
        self.plan["files"][0]["url"] = self.plan["source"]
        self.fetch(Response())
        receipt = json.loads((self.output / "acquisition.json").read_text())
        self.assertIsNone(receipt["revision"])
        self.assertEqual(receipt["declared_observation_label"], "observed text date label")
        self.assertIsNone(receipt["files"][0]["source_identity"]["revision"])


if __name__ == "__main__":
    unittest.main()
