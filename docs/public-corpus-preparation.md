# Public speech preparation

`scripts/research/prepare-public-corpus.py` prepares explicitly selected public speech outside the repository. It never reads History, Keychain, clipboard or microphone, uploads audio, calls providers, trains models, or changes application defaults. Python 3.12 and the research-only pinned PyArrow dependency are sufficient; install the adjacent requirements file into a private virtual environment.

The Rust types and validator in `src-tauri/src/evaluation/` own evaluator input semantics. This preparation tool records source facts and emits drafts; it does not independently certify evaluator validity, labels, quality or speaker holdouts.

Text-only cleanup pairs are not audio/speech candidates or reviewed public-speech labels; the metadata path below may describe text-source role proposals but does not promote them or grant admission. The only source snapshot prepared from this research is the pinned Disfl-QA `dev.json`, kept outside the repository: its 1,000 original/disfluent text pairs were independently checked for nonempty fields, unique originals and no identical pairs. It is text-only capability material, not captured speech, user corrections, a held-out set or quality evidence; it has not been imported into the repository or an experiment. Other dataset/model/rules pages were inspected only; no other data or code was downloaded or copied. Synthetic TTS remains synthetic and requires per-record source-license review, so it cannot stand in for real-speaker evidence. See the [cleanup-learning harness plan](cleanup-learning-harness-plan-2026-11-05.md#31-text-cleanup-data-and-implementation-references-checked-2026-11-06) for the pinned source and exact snapshot identity.

## Explicit acquisition

Create an external JSON plan with exactly these fields:

```json
{
  "artifact_type": "public_acquisition_plan",
  "schema_version": 1,
  "source": "https://huggingface.co/datasets/CAiRE/ASCEND",
  "revision": "737e9800ae31be9932ba8464c80366559bd28424",
  "license": "CC-BY-SA-4.0",
  "attribution": "ASCEND authors, citation, original URL and license URL",
  "files": [{
    "path": "README.md",
    "url": "https://huggingface.co/datasets/CAiRE/ASCEND/resolve/737e9800ae31be9932ba8464c80366559bd28424/README.md",
    "publisher_sha256": null,
    "max_bytes": 1048576
  }]
}
```

Use the publisher's full SHA-256 for each parquet. The example README has no supplied publisher SHA; the receipt still records the actual digest. Plans reject unknown/duplicate fields, duplicate files, traversal and credential-bearing URLs. Before IO, HF dataset resolve/tree URLs must match the declared repository and full immutable commit. Resolve paths must also match the selected file; tree requests preserve the remote directory separately from the local JSON filename. Mutable branches, mismatched commits/repositories, escaped separators/traversal and ambiguous double escapes are rejected. Requested URLs are retained unchanged. Other public paper/legal URLs have observed byte identities and an explicit observation label; their receipt repository revision is null, not a claimed HF commit. All inputs/outputs must be explicit absolute paths outside this repository. Fresh outputs are immutable; choose a new directory after changing source/labels.

```bash
python scripts/research/prepare-public-corpus.py fetch \
  --plan /external/plan.json --out-dir /external/source-v1 --dry-run
python scripts/research/prepare-public-corpus.py fetch \
  --plan /external/plan.json --out-dir /external/source-v1 \
  --max-file-bytes 125829120 --max-total-bytes 262144000 --deadline-seconds 300
```

Dry-run performs no network writes or downloads. Fetch accepts at most 20 selected files, checks content length and streamed bytes, verifies supplied digests, and records parsed source identity, unchanged requested URLs, license, attribution, bytes, digests and elapsed time. The shared writer creates an exclusive mode-0700 private namespace using a 128-bit nonce (at most 16 attempts), makes its staging descriptor anonymous before transport/callbacks, and captures validated stage bytes before publication. It never unlinks or links a public `.partial` filename. Only this owner may write the private namespace; this is a producer ownership boundary, not protection against a same-user process deliberately tampering with private descriptors.

Publication creates the destination exclusively and writes retained bytes through that new descriptor. Existing/concurrent target collisions are preserved. **Publication is exclusive, not whole-file atomic.** During writing, a destination may be incomplete; failure preserves that owned target and its descriptor/identity with an explicit recovery reason. A parseable file or its existence alone is not a successful tool completion. This differs from the evaluator's atomic checkpoint requirement. Cleanup closes anonymous stages and removes only the empty private namespace, without scanning or recursively deleting files. Moved/replaced namespace identity or failed cleanup retains ownership/handles and a reason. Complete outputs remain after a later failure. Use a fresh destination for retries and explicitly recover reported retained resources. The same owner handles fetch, preparation and import writers; it never deletes public targets or overwrites collisions.

`OutputRoot` binds the selected path, canonical path at open and held directory identity once for the entire batch, including its aggregate receipt. Nested parents are opened beneath that owner; writers borrow its descriptors rather than opening a new root for each file. It checks selected/canonical parent applicability before and after publication, before cleanup and at batch completion. Completed publication descriptors remain held until the whole batch succeeds, with identity and captured file version checked at completion. A moved/replaced root or nested parent latches applicability to unknown, rejects successful completion and retains anchored resources with selected/captured identities and destination facts. The replacement directory and foreign bytes remain untouched. This distinguishes owning a descriptor in a moved directory from certifying the caller's selected path. It detects changes at these meaningful boundaries; it does not claim atomic pathname checks or perpetual freshness after return.

The namespace owner permits at most 64 held parent directories and 256 output resources. A receipt written before a later batch failure may remain among retained incomplete artifacts; its existence is not successful acknowledgement. Error diagnostics identify retained resources and applicability, and a new receipt writer cannot silently rebind the batch to a replacement root. Fetch, ASCEND preparation, YODAS inspection, reviewed import and standalone `write_once` share this boundary. Retained descriptors last for the process lifetime or explicit recovery; they do not imply a cross-process recovery service. The extra retained descriptors and applicability checks have not yet been measured for completed-task costs.

`InputSnapshot` captures bytes from one opened regular-file descriptor with a cap-plus-one read, rejects over-cap/growing inputs, and binds parsed JSON and SHA to those same retained bytes. Input path replacements after capture cannot rewrite its facts or provenance. Plans, receipts, candidates, labels and WAV imports share this owner. Imports retain exact label/candidate bytes and captured source identities; they do not reopen those paths to copy or hash them later.

## ASCEND candidates and review

`prepare-ascend --source-dir /external/source-v1 --out-dir /external/candidates-v1` supports the pinned ASCEND validation/test parquets. It retains bounded raw parquet snapshots (120 MiB per file, 250 MiB combined by default) and audits their actual columns, embedded audio and source paths in one-row batches, with a 64 MiB decoded row-group cap, 3,000 combined row cap, 250 MiB decoded audio cap and 120-second default deadline. A second pass replays the same captured bytes and checks selected source/WAV/PCM facts against the first pass before any output is created. It never reopens a mutable parquet path for that replay. `--dry-run` reads/audits the already downloaded source but writes nothing. These bounds trade additional retained raw input memory for consistent source authority; completed-task costs still need measurement.

The parser retains publisher speaker/session/topic, full source path, optional filename recording variant, transcription and duration separately from sample-derived facts. Duration differences up to 1 ms are retained, not trimmed. The speaker/session/variant key is a raw-recording grouping observation; it does not establish the conversation partner or a globally unique conversation session.

Selection uses only publisher validation: 20 cases per zh/en/mixed language, 2–15 seconds, nonempty source draft without `[UNK]`, ordered by case-ID SHA and round-robin publisher speaker. Publisher test is reserved and remains unassigned to a project split. Source metadata and duplicate WAV/PCM audits remain accessible. The source WAV is already 16 kHz mono PCM16; selected bytes and all samples are preserved. Unsupported audio formats fail closed rather than receiving an undeclared conversion.

Outputs include `candidates.json`, `source-metadata.jsonl`, `preparation-audit.json`, `source-acquisition.json`, `speech-draft.json`, `review-queue.json`, `negative-draft.json`, `negative-source.json` and WAVs. Draft manifests have `provenance.reviewed=false`; their `expected_final` is explicitly a source-text draft, and Rust validation should reject their review gate. A separate queue starts project verbatim/final/protected/variants/intent fields at null. No strict sidecar is fabricated for missing session/template facts; `sidecar-readiness.json` reports the gap.

Six generated negatives are three silence and three xorshift noise fixtures. Seeds, recipe, source-code digest, duration, WAV and PCM hashes are recorded. They are synthetic, not real ambient or music recordings. Their review gate also starts false; music coverage remains missing.

Listen to each selected clip before model outputs. Record verbatim speech, language, uncertainty and allowed variants separately from the intended final text. Capture why the final target preserves or removes fillers, repetitions and self-corrections; public conversation often lacks the speaker's dictation intent. Leave ambiguous intent pending or explicitly restrict that case to verbatim. Review protected entities and literals, record reviewer/time/revision, resolve disagreements, and resolve conversation/session/template grouping conservatively before importing a strict sidecar. Neither source human annotation nor ordinary edit feedback is project cleanup gold.

`import-reviewed --candidates /external/candidates-v1/candidates.json --labels /external/labels.json --out-dir /external/reviewed-v1` consumes explicit human declarations. Add `--dry-run` to check without writing. The labels artifact has exactly `artifact_type: reviewed_public_speech_labels`, `schema_version: 1`, `candidates_sha256`, `rights_review_id`, `group_review_id`, and `cases`. Each case has exactly:

```text
case_id, audio_sha256, pcm_sha256, status, transcript_verbatim,
expected_final, protected, reference_variants, authorized_changes,
intent_evidence, reviewer, reviewed_at, annotation_revision, adjudication,
speaker_id, source_session_id, template_family_id, split
```

Require `status=reviewed`, `adjudication=resolved`, `split=dev`, nonempty bounded labels/identity/reviewer fields, exact publisher speaker identity, and matching candidate/WAV/PCM digests. Pending/null/unresolved groups cannot import. The importer preserves original candidates and labels, copies compatible WAV bytes and emits `speech.json` plus `corpus-sidecar.json`; Rust `--validate` remains required before scoring or any model/provider run. It does not export training formats or declare a locked acceptance set. Negative review must retain the generator source and category in an explicit review artifact before creating a reviewed NonSpeechIndex; simply flipping the draft boolean is insufficient evidence.

## YODAS metadata

`inspect-yodas --source-dir /external/yodas-source --out-dir /external/yodas-observations` accepts only explicit en/zh metadata parquets at revision `81d802f45a1c1e1f5331260484dacf1fbea81981`. Defaults limit files to 12 MiB, decoded row groups to 64 MiB, combined rows to 1,000 and elapsed processing to 120 seconds. It preserves encrypted recording identity, shard, language, duration and channel facts, original-caption and translation hashes, bounded segment observations and unknown speaker identity. Caption strings over 2 MiB remain unparsed with a reason. Nonfinite source fields become null with an explicit field list; original values remain in the pinned parquet.

New metadata summaries use schema version 2: each row has `flagged_segment_count`, and `timing_observations` separately counts total flagged segments, rows with one or more flags, and rows whose captions were not examined. A flag is a noninteger/negative timestamp or a segment end beyond declared duration plus 1 second; it does not establish alignment truth. Earlier schema version 1 artifacts remain immutable, with their original creator snapshots. A newer utility hash does not invalidate historical creator provenance; regenerated outputs require a new destination/version.

No audio or tar member is acquired by this command. Large paired audio shards require a separately declared member/range acquisition design and concrete cap. Preserve original WebM/Opus before any future crop/channel mix/resample. Weak captions, timestamp words and English translations are not reviewed gold or speaker identities. See the external research source decision for the source eligibility and planned audio experiments; this page documents implemented preparation behavior only.

## PROJECT reservation facts and retained-packet selection

The public-corpus owner also supports `project-reservations --plan /absolute/external/plan.json --dry-run`. Inputs are explicit SHA-bound parsed ASR source packets and the supported prospective reservation adapters. The one owner retains global case/source identity, family certainty, observed versus proposed roles, complete comparison counts and provenance. Complete role inventory does not fill missing source/session/template grouping. Unknowns block certified lock/training separation while truthful existing development facts can remain exploratory. Even satisfied partition separation grants no rights, media, human labels or training admission.

The `partition_fact_shard` input kind exposes the existing strict `project_partition_fact_shard` parser (1–200 cases). Its manifest ID must equal the plan-selected ID. Container SHA evidence and unchanged upstream row/family evidence remain distinct. This metadata path can describe speech negatives or text-source role proposals; it does not parse labels, promote proposed roles, repair targets, or grant rights, review, grouping completeness or training eligibility.

`project-partition-check --plan /absolute/external/check.json` is read-only and reuses the same selected-input capture, `PartitionShard`, `ProjectPartition` and connected-family decision. The strict plan has `artifact_type: project_partition_check_plan`, `schema_version: 1`, `project_id`, `registry_path`, `expected_registry_sha256` (actual SHA or explicit null for verified absence), one `input` (kind/path/sha256/manifest_id), caller-ordered distinct `selected_case_ids`, and `purpose` (`existing_development`, `certified_lock`, or `certified_training`). Complete per-shard case/source uniqueness is validated before any registered-row filtering. Registration counts and certified-use unregistered reasons follow only the normalized caller-selected IDs in caller order; unselected rows remain in the complete comparison graph. It never creates a registry, lock, stage, output directory or label artifact. The complete bounded decision is stdout JSON; exit0 means its consumer action can continue, exit2 means blocked or a tagged input failure. Blocked decisions retain conflicts, unknowns, proposed reservations, captured registry/owner/input/plan identities and examined counts. Existing development may continue with explicit unknowns; certified training/lock cannot. Fresh unregistered cases remain blocked for certified uses even if their proposed facts are otherwise aligned. No result grants training admission.

Decision reuse must bind the current owner source, actual registry SHA, captured selected-input SHA/manifest, exact selected ID order, purpose and check-plan SHA. There is no cache or automatic current-registry discovery. A changed source/registry/plan or unknown applicability requires a new decision. A dry-run metadata ingest proves no actual registration; no empty registry or merely source-TRAIN label certifies project decontamination.

A real registry lives only at an explicitly selected external research path, with project ID and predecessor SHA (or independently checked absence). Initialization and bootstrap require separate authorization. Non-dry-run updates are additive/idempotent, with owned private parent/lock/stage, immutable backups and atomic replacement; failures preserve partial ownership/durability reasons. No empty replacement or role reassignment resolves grouping gaps.

The separately versioned research consumer `/Users/mingjie/Documents/voice-dictation-research/tools/asr-speech-source/v3-project-partitions/project_source.py` has two explicit routes. `--route current-packet` requires `--public-tool`, `--public-tool-sha256`, `--source-packet`, `--source-packet-sha256`, `--project-registry`, `--project-registry-sha256`, `--project-id`, `--project-manifest-id`, `--purpose`, `--job`, and `--out-directory`; repeat `--case-id` for caller order. It loads only the selected current public owner. The packet cannot select executable code. The output must be an explicitly preinitialized empty canonical mode0700 external directory; the same owner binds it and exclusively publishes metadata files, retaining failed cleanup ownership.

The current input contract supports only the unchanged preproduced pilot60 metadata packet SHA256 `8c7d944330a54676bb44cc4097f3565d0bf894604c3c84737051fe07ea50da6c`. An arbitrary caller pin cannot add a new supported packet. It is captured/normalized once; its bytes, historical producer observation and historical dfca owner field remain unchanged. The same parsed facts feed native selection and the shared partition owner. Current owner/consumer observations appear separately in `project-partition-decision.json`. No audio, raw parquet, publisher label body, new corpus member or human intent is acquired or created. Native Rust still validates JobV1, evidence, packet/index/WAV applicability before any separately authorized runtime.

`--route legacy-v2` explicitly retains the earlier ten-evidence projection and historical loader/namespace-owner requirements. Its `--public-tool` must select retained historical dfca bytes, not a mutable current source file. Original v2 and old audits/native wire stay immutable. Current-packet mode never imports historical dfca code or restamps the new owner as old. Pin presence grants no admission. The private research implementation/tests remain separately reviewed; no public corpus claim follows from metadata preparation.
