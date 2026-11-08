import type { DictionaryLearningAudit, LearningAuditActivity, LearningAuditPair } from "../lib/dictionaryLearningAudit";

export const auditRuntime = "11111111-1111-4111-8111-111111111111";
export const auditPairRef = "22222222-2222-4222-8222-222222222222";
export function auditPair(overrides: Partial<LearningAuditPair> = {}): LearningAuditPair {
  return {
    pair_ref: auditPairRef, pair_key: "fixture-star-protocol", before_surface: "星蓝协议", after_surface: "星澜协议",
    rule_kind: "word_pair", record_state: "pending", candidate_hits: 1,
    current_classification: { class: "term", reason: null }, current_promote_threshold: 3, classification_authority: "current_owner_classification",
    recorded_scope: { family: "document", mapping_id: "fixture.document", browser_host: null, native_bundle: "test.fixture.document" },
    origin: "unknown_not_persisted", tombstone_reason: null, latest_record_change_at: "2026-10-15 00:00:00",
    last_output_surface_match_at: null, current_dictionary_contains_after: false,
    replacement_policy: "context_scoped_promoted_rules", replacement_eligibility: "not_eligible_now", eligibility_authority: "current_lexicon_owner_filter",
    current_review: { confirm: true, ignore: true, undo: false }, ...overrides,
  };
}
export function auditObservation(overrides: Partial<LearningAuditActivity> = {}): LearningAuditActivity {
  return {
    activity_id: "33333333-3333-4333-8333-333333333333", sequence: "1", observer_epoch: "9007199254740993", created_at: "2026-10-15 00:00:00", pair_ref: auditPairRef, lifecycle: "closed",
    fact: { kind: "post_paste_attempt", registration: { status: "started", reason: "registered" }, verified_paste: true,
      outcome: "single_token", classification: { class: "term", reason: null }, candidate_decision: "pending",
      candidate_store: { status: "completed", reason: "candidate_db_returned" }, settings_store: { status: "not_attempted", reason: null }, attribution: "whole_field_difference_only" },
    ...overrides,
  };
}
export function auditSnapshot(overrides: Partial<DictionaryLearningAudit> = {}): DictionaryLearningAudit {
  const activity = overrides.activity ?? [auditObservation()];
  return {
    schema_version: 1, runtime_id: auditRuntime, audit_revision: "9007199254740994", clear_generation: "0", observer_epoch: "9007199254740993",
    coverage: { kind: "current_process", capacity: 128, dropped_entries: "0", dropped_updates: "0",
      oldest_sequence: activity[0]?.sequence ?? null, latest_sequence: activity[activity.length - 1]?.sequence ?? null,
      projection_consistency: "stable_revision", atomicity: "not_cross_store_transaction", consistency_scope: "audit_metadata_revision_only",
      projection_mutation_interval: "not_observed", projection_inflight_writers: null,
      pair_projection: { kind: "complete_within_budget", max_rows: 1024, max_utf8_bytes: 2097152, max_snapshot_encoded_bytes: 16777216 } },
    pairs: [auditPair()], activity, ...overrides,
  };
}
