# Dictionary-learning human pilot tool

`dictionary-learning-pilot.py` freezes the task assignment and summarizes a
locally recorded evidence ledger for the technical acceptance and benefit
pilot in `docs/design/voiceflow-dictionary-learning-harness-plan.md` §§4–5.
The protocol is explicitly **pre-C1**: its App A/App B conflict cases test the
legacy rule identity behavior. It does not migrate C1 storage or evaluate the
later preference work in §7.5.

The script allocates rows, builds and verifies a pinned pre-C1 Installable Test
app, checks local evidence and computes gates. It does not launch VoiceFlow,
dictate, observe a target app, read Keychain, call a model, or contact
participants. Use the normal VoiceFlow UI for human tasks. The frozen binary
does not include `research-harness`, so `--research-stdio` is not human pilot
evidence. Keep keys, allocations, freeze facts, the build, receipts, evidence
artifacts and reports outside the repository.

No participants are available yet. The checked-in fixture and blank evidence
ledger are explicitly synthetic or empty; the current human result is pending.
Do not change an empty ledger to `actual_human` or treat the unit tests as pilot
evidence. This batch is the pre-C1 legacy App A/App B conflict protocol, before
the fresh-database C1 and preference-settings work.

## Technical acceptance

Create a deterministic draft. It assigns 20 of the 40 trials to each operator,
splits each category evenly, and assigns five IME and five focus-switch checks.
At least one operator must be declared a non-implementer.

```sh
python3 scripts/research/dictionary-learning-pilot.py allocate-technical \
  --batch-id tech-001 --seed '<frozen-seed>' \
  --operator OP01 --operator OP02 --non-implementer OP02 \
  --out /absolute/private/pilot/tech-draft.json
```

This batch is frozen to the legacy pre-C1 contract at commit
`f4a75a1866f16c8d52932e7de461c1861e7b493d` (Settings schema 27, History schema
19). The build owner creates a separate clean pinned checkout and runs the
locked arm64 Tauri build. Generate an unencrypted Ed25519 signing key outside
the repository, then provide a separately pinned copy of its public key through
the agreed trusted-key channel. Verify that public-key fingerprint
independently before using it. The private key must have owner-only file
permissions; do not put either key in the repository.

```sh
mkdir -m 700 -p /absolute/private/build-owner-keys /absolute/private/trusted-keys /absolute/private/pilot
ssh-keygen -q -t ed25519 -N '' -C 'voiceflow-pre-c1-build-owner' \
  -f /absolute/private/build-owner-keys/voiceflow-pre-c1
ssh-keygen -lf /absolute/private/trusted-keys/voiceflow-pre-c1.pub -E sha256
python3 scripts/research/dictionary-learning-pilot.py build-pre-c1 \
  --owner-root /absolute/private/pilot/build-owner-001 \
  --owner-private-key /absolute/private/build-owner-keys/voiceflow-pre-c1 \
  --owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  > /absolute/private/pilot/build-owner-output.json
```

The command writes the pinned source checkout, app, logs, and signed
`voiceflow_dictionary_learning_pre_c1_build_owner_receipt` as
`build-owner-receipt.json` plus `build-owner-receipt.json.sig` under the new
private `owner-root`. It returns `freeze_facts_base` with the absolute checkout
and app paths, executable SHA-256, receipt and signature SHA-256 values, and
the signing key fingerprint. The build validates the source revision, clean
checkout, Settings 27 / History 19, Test bundle identifier, locked dependency
inputs, whole bundle inventory, executable, and ad-hoc app signature before it
writes the signed receipt. A failed build emits no valid receipt. Copy
`freeze_facts_base` into a private freeze-facts file, add the frozen route,
model, permissions and initial state fields below, and use this exact app for
the whole batch. Do not build a replacement after freezing. At seal and
analysis, the tool requires the independently trusted public key and verifies
the receipt signature, receipt and signature hashes, key fingerprint, whole
bundle, executable hash and app signature against the frozen facts. Keep the
private key separate from the build output root and trusted public key; never
copy either key or the receipt into the repository. Do not reinterpret the
frozen §4 A/B conflict gate using C1's separate scoped rows.

Prepare a task catalog with one row for every assigned trial ID. Hash each
frozen task specification without copying its text into this repository:

```json
{
  "artifact_type": "voiceflow_dictionary_learning_task_catalog",
  "schema_version": 1,
  "stage": "technical",
  "tasks": [
    {"trial_id": "T001", "category": "a_positive", "task_spec_sha256": "<64 lowercase hex>"}
  ]
}
```

The catalog must contain exactly the assigned IDs and categories. Create freeze
facts from the actual Test bundle before starting the batch:

```json
{
  "artifact_type": "voiceflow_dictionary_learning_freeze_facts",
  "schema_version": 2,
  "source_revision": "f4a75a1866f16c8d52932e7de461c1861e7b493d",
  "settings_schema_version": 27,
  "history_schema_version": 19,
  "test_bundle_identifier": "com.voiceflow.installable-test",
  "source_checkout_path": "/absolute/private/pilot/build-owner-001/source",
  "test_app_path": "/absolute/private/pilot/build-owner-001/source/src-tauri/target/release/bundle/macos/VoiceFlow.app",
  "build_sha256": "<64 lowercase hex>",
  "build_owner_receipt_path": "/absolute/private/pilot/build-owner-001/build-owner-receipt.json",
  "build_owner_receipt_sha256": "<64 lowercase hex>",
  "build_owner_signature_sha256": "<64 lowercase hex>",
  "build_owner_key_fingerprint": "SHA256:<pinned key fingerprint>",
  "asr_route": "on_device",
  "asr_model_id": "<selected model>",
  "asr_model_revision": "<selected revision>",
  "cleanup_mode": "off",
  "term_injection_enabled": false,
  "fuzzy_matching_enabled": false,
  "permissions_sha256": "<64 lowercase hex>",
  "initial_state_sha256": "<64 lowercase hex>"
}
```

Seal the assignment before collecting evidence, then create the blank ledger:

```sh
python3 scripts/research/dictionary-learning-pilot.py seal \
  --draft /absolute/private/pilot/tech-draft.json \
  --task-catalog /absolute/private/pilot/tech-task-catalog.json \
  --freeze-facts /absolute/private/pilot/tech-freeze-facts.json \
  --build-owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  --out /absolute/private/pilot/tech-frozen.json

python3 scripts/research/dictionary-learning-pilot.py new-evidence \
  --manifest /absolute/private/pilot/tech-frozen.json \
  --out /absolute/private/pilot/tech-evidence.json
```

The manifest fingerprint binds the seed, operators, task assignments, task
catalog, build, route, model, permissions and starting state. Technical
`initial_state_sha256` is the SHA-256 of canonical JSON over the active/effective
state fields: `global_dictionary_sha256`, `active_rule_inventory_sha256`,
`tombstone_inventory_sha256`, `active_rule_families`, `effective_hint_sha256`,
`passive_learning_enabled` and `target_rule_state`. Every snapshot's
`active_state_fingerprint_sha256` uses that same field set. Its separate
`state_fingerprint_sha256` includes the full `rule_inventory_sha256` and
`inactive_candidate_inventory_sha256` as well.

The four rule-inventory digests come from the native bounded-audit `pairs`
projection. Its stable fields are `pair_key`, `before_surface`, `after_surface`,
`rule_kind`, `record_state`, `current_classification`,
`current_promote_threshold`, `classification_authority`, `recorded_scope`,
`origin`, `tombstone_reason`, `current_dictionary_contains_after`,
`replacement_policy`, `replacement_eligibility`, `eligibility_authority` and
`current_review`. Sort each row set by persisted `pair_key`, then hash canonical
JSON. `rule_inventory_sha256` covers all states and includes `candidate_hits`
on every row; `active_rule_inventory_sha256` covers only `db_promoted` rows
without hits; `inactive_candidate_inventory_sha256` covers `pending` rows with
hits; `tombstone_inventory_sha256` covers `tombstoned` rows with hits. Time and
last-use fields are omitted. The process-local salted `pair_ref` is excluded
from cross-run hashes and is used only for same-runtime audit activity,
candidate-label and before/after pending-write links.

Ordinary technical tasks retain the frozen active/effective fingerprint.
Forget/Restart tasks use one stable post-Forget active fingerprint that differs
from the initial fingerprint, only in the assigned post-Forget substages. A
pending-only inventory change is allowed only in a technical A/B passive
observer task. The owner receipt's before/after native inventory hashes must
match the state snapshots; active, tombstone, dictionary, hint and other
effective state must remain unchanged. The full `rule_inventory` and inactive
candidate projections must move together. `pending_candidate_writes` must
exactly equal the changed inactive-candidate `pair_ref` list, and each write
must report `completed` / `candidate_db_returned` and remain pending afterward.
Pair refs are compared only within the same runtime. Because full inventory
hashes `candidate_hits` on every row, unrelated or active-row changes fail this
narrow exception. Editing a frozen manifest invalidates its fingerprint. The
evaluator also requires every row to report the assigned phase, arm and
operator. A missing row remains in the allocated denominator and appears in
`missing_evidence`.

Each operator works from an isolated copy of the same frozen initial state.
Within each operator's frozen order, ordinary A/B/boundary tasks precede the
Forget boundary, all `after_forget` tasks precede Restart, and all
`after_restart` tasks follow it. The analyzer rejects a reordered allocation.

## Benefit pilot

Before the random draw, freeze ten actual opaque family IDs in five matched
sets. The catalog contains IDs only, not task text; do not use generated
placeholder `WF` names:

```json
{
  "artifact_type": "voiceflow_dictionary_learning_family_catalog",
  "schema_version": 1,
  "families": [
    {"family_id": "f-<opaque-id-1>", "matched_set": "m-<opaque-set-1>"},
    {"family_id": "f-<opaque-id-2>", "matched_set": "m-<opaque-set-1>"}
  ]
}
```

The example shows one pair; include exactly ten unique family IDs in five
matched sets of two. The same catalog fingerprint is stored in the allocation
before randomization.

The benefit stage regenerates technical analysis from the frozen manifest,
evidence ledger and local artifact files. A report with manually typed status
fields cannot open the gate. Supply the same report and upstream inputs at
allocation and sealing:

```sh
python3 scripts/research/dictionary-learning-pilot.py allocate-benefit \
  --batch-id benefit-001 --seed '<frozen-seed>' \
  --participant P01 --participant P02 --participant P03 --participant P04 \
  --family-catalog /absolute/private/pilot/family-catalog.json \
  --build-owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  --technical-report /absolute/private/pilot/tech-report.json \
  --technical-manifest /absolute/private/pilot/tech-frozen.json \
  --technical-evidence /absolute/private/pilot/tech-evidence.json \
  --artifact-root /absolute/private/pilot/tech-artifacts \
  --out /absolute/private/pilot/benefit-draft.json
```

At `seal`, pass the same trusted public key and the same four technical inputs
again. If an upstream ledger or artifact changes, rerun technical analysis with
that key and create a new benefit draft. The report binds its manifest
fingerprint, canonical evidence-ledger hash and verified artifact-reference
digest.

```sh
python3 scripts/research/dictionary-learning-pilot.py seal \
  --draft /absolute/private/pilot/benefit-draft.json \
  --task-catalog /absolute/private/pilot/benefit-task-catalog.json \
  --freeze-facts /absolute/private/pilot/benefit-freeze-facts.json \
  --build-owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  --technical-report /absolute/private/pilot/tech-report.json \
  --technical-manifest /absolute/private/pilot/tech-frozen.json \
  --technical-evidence /absolute/private/pilot/tech-evidence.json \
  --artifact-root /absolute/private/pilot/tech-artifacts \
  --out /absolute/private/pilot/benefit-frozen.json
```

Each person gets 40 tasks: 20 target tasks, 10 safety negatives and 10 ordinary
tasks. The ten target families form five matched sets. Within each person, one
family in each set is randomized to Teach and the other to control; each family
has two tasks and stays in one arm. Safety negatives include three CJK
homophone cases and two Latin capitalization cases.

The benefit task catalog uses the allocated trial IDs and carries each actual
task's `family_id` and `matched_set`. Benefit freeze facts use the same common
fields as the technical stage plus:

```json
{
  "state_sha256_by_participant": {"P01": "<64 lowercase hex>"},
  "effective_hint_sha256_by_participant": {"P01": "<64 lowercase hex>"},
  "active_rule_families_by_participant": {"P01": ["f-opaque-a", "f-opaque-b", "f-opaque-c", "f-opaque-d", "f-opaque-e"]},
  "passive_learning_disabled_by_participant": {"P01": true}
}
```

Include every assigned participant in each map. `state_sha256_by_participant`
is the full `state_fingerprint_sha256`, including active rules, pending
candidates and tombstones; `effective_hint_sha256_by_participant` binds the
actual hint projection separately. Every before/after receipt for that person
must match both frozen values for every assigned task, with no pending-only
exception in the benefit stage. The active-family inventory contains exactly
five unique families and must match that person's randomized Teach allocation;
control families must not be active. Benefit freeze facts must also exactly
reuse the technical batch's build hash, signed receipt and signature hashes,
trusted-key fingerprint, and ASR model ID/revision. This links every assigned
task to the same per-person full state and signed build. Seal against the same
technical report and upstream inputs, then create the blank evidence ledger as
above.

## Evidence ledger and analysis

Change the blank ledger's `evidence_class` to `actual_human` only when it
contains actual, locally authorized evidence. Set `human_data_attested`,
`local_only_confirmed` and `research_authorization_confirmed` to `true`; the
benefit ledger also needs `participant_authorization` set to `true` for every
pseudonymous participant. A false authorization is a failure; missing
authorization remains pending. Each operator or participant must have a
content-free `human_attestation` receipt bound to that actor, batch, phase and
manifest.

For every trial, record the observed phase and arm, the assigned operator or
participant, `state_integrity` (`clean`, `contaminated` or `unknown`), and the
category-specific outcome codes (`yes`, `no` or `unknown`). Each `provenance`
role is `{ "path": "relative/file", "sha256": "..." }`; paths are relative to
the external artifact root. Task-bound receipts bind `batch_id`,
`manifest_fingerprint`, `phase_id`, `trial_id`, `actor_id` and assigned `arm`.

The physical hotkey path is the only accepted human collection path for this
frozen build. The binary does not include `research-harness`; `--research-stdio`
receipts and synthetic `LearningRecord` fixtures cannot establish actual human
capture or delivery. Use the normal bounded `get_dictionary_learning_audit`
read in the exact signed Test app.

The content-free owner receipt is bound to the exact Test bundle and carries
before/after audit snapshots. It requires the same runtime and audit generation,
a single observer-epoch advance, stable projection reads, zero dropped
entries/updates, and a new closed or superseded `post_paste_attempt` activity
with the existing `verified_paste`, registration/outcome and candidate/Settings
store statuses. Record the task's native `activity_id` as `capture_id` and
native audit `sequence` as `recording_generation`; bind the source receipt to
these values, `runtime_id`, the owner receipt hash, task, actor and manifest.
The operator confirms that the task matches a new live hotkey capture and that
capture closed. The separate task-bound `target_terminal` receipt and
independent label prove actual target delivery; bounded audit alone is not
terminal proof. Reusing the same owner file or canonical capture identity under
a new ledger ID fails. No raw audio or text body is required; record only
`raw_asr_sha256` and `inserted_text_sha256`, matching the source receipt.

Same-task bounded-audit activities from one runtime and observer epoch may be
aggregated as one physical capture; they do not create extra eligible
corrections or captures. Multiple-token candidate activities in that group
produce one sorted `observer_candidate_refs` list. A capture counts when the
owner receipt binds the task to the normal audit and the audit confirms an
observed lexical candidate; candidate persistence may fail without erasing an
observed capture. A verified `no_grant`, timeout, unreadable result, no
eligible candidate, or superseded/stale activity with no write is a known miss
(`observer_captured: no`). Unsupported multiple-token outcomes, conflicting
facts, or missing/unlinked activity or receipt are `unknown`.

The independent label records `observer_owner_receipt_sha256`,
`observer_capture_id` and the exact sorted `observer_candidate_refs` list from
the owner receipt. Capture binding must match even when correctness is
unknown. `observer_correct: yes` counts only when the label is valid,
`eligible_correction` is yes, the normal audit marks a positive capture, the
owner-receipt hash and capture ID match, and exactly one candidate ref matches.
`observer_correct: no` can be a known negative for a valid positive capture
with one independently bound candidate ref, including when
`eligible_correction` is no; it never increments `correct_captures`, and an
ineligible capture remains in the false-positive/captured denominator. A
grouped capture with multiple refs remains captured but its correctness is
`unknown`; a missing or ambiguous candidate binding also keeps correctness
unknown.

For a B conflict probe, the normal audit delta may include one closed
`review_action` for `add_manual_scoped_pair`: `source: dictionary_command`,
`history_reference: not_provided`, `outcome: failed`,
`candidate_store: {status: failed, reason: store_error}`,
`settings_store: {status: not_attempted, reason: null}`, `pair_ref: null` and
`observer_epoch: null`. This attempted command itself writes no rule and must
not change the active A rule, active-rule inventory, tombstones or dictionary.
A separate pending observer candidate may appear in the same A/B task only
under the pending-only exception above, with its exact `pair_ref` and
`completed` / `candidate_db_returned` evidence; do not attribute it to this
failed review action. The independent B label carries
`b_conflict_probe_result: manual_pair_scope_conflict` and must
agree with the exact conflict response plus task outcomes
`b_conflict_probe=yes`, `explicit_conflict=yes` and `a_rule_unchanged=yes`.
It counts as a verified probe only when the label's
`observer_owner_receipt_sha256` and `observer_capture_id` match the owner
receipt, source receipt and source-capture ID, and the recording is distinct;
the owner-derived observer fact must be `bounded_audit` with
`human_acceptance_authority: true` and a validated review action. Missing or
unverified action/bindings remain pending; contradictions fail the probe. The
report separates claimed, verified (`conflict_probes`) and unverified probes.
The generic `store_error` alone does not prove that a scope conflict occurred.

Typed receipts use `schema_version: 1` and these artifact types:

- `voiceflow_dictionary_learning_source_capture_receipt` and
  `voiceflow_dictionary_learning_owner_receipt` for each live task.
- `voiceflow_dictionary_learning_independent_label`, with a different
  `labeler_id`, the frozen task reference hash and
  `reviewed_before_outcome: true`. Its outcome codes must match the ledger;
  technical observer labels also bind the owner receipt SHA, capture ID and
  exact candidate-ref list described above.
- `voiceflow_dictionary_learning_safety_observation`, with a labeler different
  from the participant, `safety_review_complete: true`, and task-bound outcomes
  for critical misedit, cross-scope application, unauthorized output, state
  contamination and resurrection. Its outcomes must match the ledger. This
  separate independent observation proves the zero-harm gate; terminal
  readback does not stand in for it.
- `voiceflow_dictionary_learning_terminal_receipt` for independent actual
  delivery and terminal readback. Benefit terminal coverage is evaluated over
  all assigned tasks at the 95% gate. Ordinary tasks may have an unknown or
  unreadable terminal and still meet that denominator threshold; all safety
  negative tasks still need complete terminal and safety evidence.
- `voiceflow_dictionary_learning_auxiliary_owner_receipt` and
  `voiceflow_dictionary_learning_auxiliary_label` for each IME/focus check.
  Both bind the check ID, check type, operator and phase; the second names a
  different labeler and matches the three observer/write outcomes.
- `voiceflow_dictionary_learning_human_attestation` per operator or
  participant. It confirms human data, local-only handling, research
  authorization and consent; operator receipts also declare the frozen
  `implementation_role`.
- `voiceflow_dictionary_learning_state_snapshot` for `state_before` and
  `state_after`, including arm, declared state contract, boundary, all four
  native pair inventory hashes (`rule_inventory_sha256`,
  `active_rule_inventory_sha256`, `inactive_candidate_inventory_sha256`,
  `tombstone_inventory_sha256`), global dictionary hash, active families,
  effective-hint digest, passive-learning state, target-rule state, full
  `state_fingerprint_sha256` and active `active_state_fingerprint_sha256`.
  Auxiliary snapshots use the `no_dictionary_write_during_auxiliary` contract
  and require the full state to remain unchanged.
- `voiceflow_dictionary_learning_time_cost_receipt` for the per-person cost
  inventory and its duration values.

The analyzer parses these files and checks their hashes, bindings and state
transitions; it does not print evidence paths or contents. Hashes prove byte
identity, not that a human label is truthful. Keep terminal readback and intent
labels independent of product audit facts. Manual attestations are allowed when
bound to the matching real owner receipts; the unit fixture remains excluded
from actual-human gates. Raw audio and text bodies are not required by this
tool; retain them only when the task's separate authorization allows it. For
participant time costs, record Teach, re-teach and undo milliseconds and the
part already included in task completion time.

Run analysis with the root directory containing the referenced files:

```sh
python3 scripts/research/dictionary-learning-pilot.py analyze \
  --manifest /absolute/private/pilot/tech-frozen.json \
  --evidence /absolute/private/pilot/tech-evidence.json \
  --build-owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  --artifact-root /absolute/private/pilot/artifacts \
  --out /absolute/private/pilot/tech-report.json
```

An unsealed allocation or an empty ledger is `pending`; a unit fixture is
reported as `fixture_only` and excluded from human gates. Reports use fixed
allocation denominators, keep missing/unknown outcomes visible, and cannot pass
on an empty or incomplete evidence set. While collection remains open, unresolved
gates stay `pending`. Once the frozen batch is complete or stopped, rerun with
`--close`; remaining pending gates become failures, and unknown target outcomes
remain unsuccessful in the full assigned denominator.

```sh
python3 scripts/research/dictionary-learning-pilot.py analyze \
  --manifest /absolute/private/pilot/tech-frozen.json \
  --evidence /absolute/private/pilot/tech-evidence.json \
  --build-owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  --artifact-root /absolute/private/pilot/artifacts \
  --close \
  --out /absolute/private/pilot/tech-final-report.json
```

Benefit analysis also requires a fresh technical re-analysis against the same
trusted build-owner key and technical inputs. Keep its upstream artifact root
separate from the benefit ledger's own `--artifact-root`:

```sh
python3 scripts/research/dictionary-learning-pilot.py analyze \
  --manifest /absolute/private/pilot/benefit-frozen.json \
  --evidence /absolute/private/pilot/benefit-evidence.json \
  --build-owner-public-key /absolute/private/trusted-keys/voiceflow-pre-c1.pub \
  --artifact-root /absolute/private/pilot/benefit-artifacts \
  --technical-report /absolute/private/pilot/tech-report.json \
  --technical-manifest /absolute/private/pilot/tech-frozen.json \
  --technical-evidence /absolute/private/pilot/tech-evidence.json \
  --technical-artifact-root /absolute/private/pilot/tech-artifacts \
  --out /absolute/private/pilot/benefit-report.json
```

Technical gates include the 8/12 eligible A exposures, 4/8 distinct B alias
exposures, zero B misapplications, an explicit B conflict with the A row
unchanged, zero boundary misedits, 12 distinct post-Forget / post-restart alias
encounters with no reapplication or resurrection, 40 actual deliveries with
independent terminal evidence, a declared non-implementer, and five clean IME
plus five clean focus-switch observations. The report separately counts actual
corrections, eligible corrections, observer captures, correct captures and
false-positive captures. Recall is
`eligible_captured_corrections / eligible_corrections`; precision is
`correct_captures / captured_corrections`. A missing eligible/capture fact
keeps recall unavailable; missing capture or correctness evidence keeps
precision unavailable. Verified `no_grant`, timeout, unreadable or
no-candidate outcomes are misses; missing or conflicting activity/receipt
evidence is `unknown`. False-positive captures stay in the captured
denominator and are separately reported; unknown captured correctness remains
visible and makes precision unavailable. Any B wrong-scope application fails even when the task did not
report an alias exposure. An A or B exposure gate fails as soon as remaining
assigned rows cannot possibly reach its minimum. At close, unresolved exposure,
delivery and observer evidence fails.

Benefit gates include at least 95% terminal evidence over every assigned task,
the Teach/control primary difference of at least 15 percentage points, a
positive observed difference for at least N−1 people, median per-person net
time saved above zero, and zero critical misedit, cross-scope application,
unauthorized output, contamination or resurrection. The primary numerator is
successful target tasks in each arm; every assigned target task remains in its
arm denominator, and unknown/failed/abstained results count as unsuccessful.
The 15 percentage point gate uses integer cross multiplication.
Time savings sum all ten paired Teach/control task differences before
subtracting incremental Teach, re-teach and undo cost. Per-person arm results
are always reported. All-task, ordinary and negative results are separate;
safety-negative evidence has its own gate, so 95% terminal coverage cannot
stand in for missing safety evidence.

## Unit fixture

`fixtures/dictionary-learning-pilot-unit-fixture.json` is synthetic and marked
`fixture_only`. It contains no human observation. `test_dictionary_learning_pilot.py`
checks deterministic assignment, per-person family controls, ordered
Forget/Restart stages, exact denominator arithmetic, reused or mismatched
capture receipts, state-transition mismatches, the B wrong-scope failure, a
forged technical report rejection, an auxiliary receipt phase mismatch,
empty-evidence pending status and fixture exclusion. Synthetic receipt tests
exercise parser behavior only and cannot produce an actual-human result.
