import { describe, expect, it } from "vitest";
import nativeLearningAuditWire from "../test/fixtures/dictionary-learning-audit-native.json";
import { auditObservation, auditPair, auditSnapshot } from "../test/dictionaryLearningAuditFixtures";
import { parseDictionaryLearningAudit, parseLearningAuditChanged, parseLearningAuditReadError } from "./dictionaryLearningAudit";

function nativeObservationFact(patch: Record<string, unknown>): unknown {
  const value = JSON.parse(JSON.stringify(auditSnapshot())) as {
    activity: Array<{ fact: Record<string, unknown> }>;
  };
  Object.assign(value.activity[0].fact, patch);
  return value;
}

describe("normal learning audit native boundary", () => {
  it("parses the shared content-controlled wire serialized by the native audit DTOs", () => {
    const result = parseDictionaryLearningAudit(nativeLearningAuditWire);
    expect(result?.activity[0].fact.kind === "review_action" && result.activity[0].fact.action).toBe("add_manual_scoped_pair");
    expect(result?.activity[1].fact.kind === "review_action" && result.activity[1].fact.candidate_store).toEqual({ status: "completed", reason: "manual_scoped_pair_saved" });
    expect(result?.activity[2].fact.kind === "review_action" && result.activity[2].fact.candidate_store).toEqual({ status: "failed", reason: "store_error" });
    const decisions = result?.activity.flatMap((entry) => entry.fact.kind === "post_paste_attempt" ? [entry.fact.candidate_decision] : []) ?? [];
    expect(decisions).toEqual(["not_attempted", "not_attempted", "scope_conflict", "duplicate_observation", "db_promotion_failed"]);
    const registrationReasons = result?.activity.flatMap((entry) => entry.fact.kind === "post_paste_attempt" ? [entry.fact.registration.reason] : []) ?? [];
    expect(registrationReasons).toContain("fixture_baseline_mismatch");
    expect(registrationReasons).toContain("ambiguous_inserted_range");
    expect(result?.pairs[0].replacement_policy).toBe("context_scoped_promoted_rules");
  });
  it("still rejects unknown native contract values in the shared producer fixture", () => {
    const withActivityFact = (index: number, patch: Record<string, unknown>): unknown => {
      const value = JSON.parse(JSON.stringify(nativeLearningAuditWire)) as Record<string, unknown>;
      const activities = value.activity as Array<Record<string, unknown>>;
      const current = activities[index].fact as Record<string, unknown>;
      activities[index] = { ...activities[index], fact: { ...current, ...patch } };
      return value;
    };
    expect(parseDictionaryLearningAudit(withActivityFact(1, { action: "future_action" }))).toBeNull();
    expect(parseDictionaryLearningAudit(withActivityFact(1, { candidate_store: { status: "completed", reason: "future_reason" } }))).toBeNull();
    expect(parseDictionaryLearningAudit(withActivityFact(4, { registration: { status: "skipped", reason: "future_skip" } }))).toBeNull();
    expect(parseDictionaryLearningAudit(withActivityFact(5, { candidate_decision: "future_decision" }))).toBeNull();
    expect(parseDictionaryLearningAudit(withActivityFact(5, { outcome: "future_outcome" }))).toBeNull();
    expect(parseDictionaryLearningAudit({ ...nativeLearningAuditWire, schema_version: 2 })).toBeNull();
    expect(parseDictionaryLearningAudit({ ...nativeLearningAuditWire, unexpected: true })).toBeNull();
  });
  it("accepts every current Rust registration, outcome, and candidate decision code", () => {
    // Keep these values aligned with dictionary_learn.rs audit call sites and
    // LearnPairSkipReason::code; dictation_origin_unknown is a candidate decision.
    const registrationReasons = [
      "unverified", "read_owner_unavailable", "delivery_owner_mismatch", "read_lease_expired",
      "secure", "no_grant", "unreadable", "delivery_fact_mismatch", "fixture_baseline_mismatch",
      "ambiguous_inserted_range", "stale_or_cancelled", "protected_target", "no_mapping",
      "ax_or_learning_not_allowed", "scene_not_allowed", "target_changed",
    ];
    for (const reason of registrationReasons) {
      const value = nativeObservationFact({
        registration: { status: "skipped", reason },
        outcome: reason === "stale_or_cancelled" ? "stale_or_cancelled" : "not_observed",
      });
      expect(parseDictionaryLearningAudit(value), reason).not.toBeNull();
    }

    const outcomes = [
      "not_observed", "observing", "nochange", "target_changed", "unreadable", "single_token",
      "multiple_tokens", "short_style_rewrite", "style_signal", "ambiguous", "stale_or_cancelled",
    ];
    for (const outcome of outcomes) {
      expect(parseDictionaryLearningAudit(nativeObservationFact({ outcome })), outcome).not.toBeNull();
    }

    const candidateDecisions = [
      "not_attempted", "pending", "db_promoted", "already_db_promoted", "rejected",
      "tombstone_or_ignored", "cap", "unknown", "unknown_scope", "scope_conflict",
      "duplicate_observation", "dictation_origin_unknown", "db_promotion_failed",
    ];
    for (const candidate_decision of candidateDecisions) {
      expect(parseDictionaryLearningAudit(nativeObservationFact({ candidate_decision })), candidate_decision).not.toBeNull();
    }
  });
  it("retains lossless identity, explicit nulls and independent persistence legs", () => {
    const activity = auditObservation();
    if (activity.fact.kind !== "post_paste_attempt") throw new Error("fixture kind");
    activity.fact.candidate_decision = "db_promoted";
    activity.fact.settings_store = { status: "failed", reason: "store_error" };
    const result = parseDictionaryLearningAudit(auditSnapshot({ activity: [activity] }));
    expect(result?.observer_epoch).toBe("9007199254740993");
    expect(result?.coverage.projection_inflight_writers).toBeNull();
    expect(result?.activity[0].fact.candidate_store.status).toBe("completed");
    expect(result?.activity[0].fact.settings_store.status).toBe("failed");
  });
  it.each([
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, schema_version: 2 }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, observer_epoch: 3 }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, audit_revision: "01" }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, audit_revision: "18446744073709551616" }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, runtime_id: "00000000-0000-0000-0000-000000000000" }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, coverage: { ...value.coverage, projection_inflight_writers: 0 } }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, coverage: { ...value.coverage, projection_mutation_interval: undefined } }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, pairs: [{ ...value.pairs[0], rule_kind: "future_profile" }] }),
    (value: ReturnType<typeof auditSnapshot>) => ({ ...value, activity: [{ ...value.activity[0], fact: { ...value.activity[0].fact, unexpected_term: "must not enter metadata" } }] }),
  ])("rejects unsupported facts instead of coercing unavailable into empty (%#)", (mutate) => {
    expect(parseDictionaryLearningAudit(mutate(auditSnapshot()))).toBeNull();
  });
  it("accepts tombstoned preference rows without inventing word classification or thresholds", () => {
    const result = parseDictionaryLearningAudit(auditSnapshot({ pairs: [auditPair({ rule_kind: "style_preference", record_state: "tombstoned", current_classification: null, current_promote_threshold: null, classification_authority: "not_word_pair", tombstone_reason: "unknown_not_persisted" })] }));
    expect(result?.pairs[0].current_classification).toBeNull();
    expect(result?.pairs[0].tombstone_reason).toBe("unknown_not_persisted");
  });
  it("preserves the real queued interval's null reason and unknown lifecycle", () => {
    const activity = auditObservation({ lifecycle: "unknown", pair_ref: null });
    if (activity.fact.kind !== "post_paste_attempt") throw new Error("fixture kind");
    activity.fact.registration = { status: "checking", reason: null };
    activity.fact.classification = null;
    activity.fact.outcome = "not_observed";
    activity.fact.attribution = "not_observed";
    const result = parseDictionaryLearningAudit(auditSnapshot({ activity: [activity] }));
    expect(result?.activity[0].lifecycle).toBe("unknown");
    const fact = result?.activity[0].fact;
    expect(fact?.kind === "post_paste_attempt" ? fact.registration.reason : "wrong kind").toBeNull();
  });
  it("requires complete bounded rows and truthful retained sequence extrema", () => {
    expect(parseDictionaryLearningAudit(auditSnapshot({ pairs: Array.from({ length: 1025 }, () => auditPair()) }))).toBeNull();
    const value = auditSnapshot();
    expect(parseDictionaryLearningAudit({ ...value, activity: [...value.activity, ...value.activity] })).toBeNull();
    expect(parseDictionaryLearningAudit({ ...value, coverage: { ...value.coverage, oldest_sequence: null } })).toBeNull();
    expect(parseDictionaryLearningAudit(auditSnapshot({ activity: [] }))?.coverage.latest_sequence).toBeNull();
  });
  it("accepts only content-free changed events and the exact typed capacity limits", () => {
    const { runtime_id, audit_revision } = auditSnapshot();
    expect(parseLearningAuditChanged({ runtime_id, audit_revision })).toEqual({ runtime_id, audit_revision });
    expect(parseLearningAuditChanged({ runtime_id, audit_revision, source_text: "unexpected" })).toBeNull();
    expect(parseLearningAuditReadError({ code: "pair_projection_capacity", resource: "rows", limit: "1024" })).toEqual({ code: "pair_projection_capacity", resource: "rows", limit: "1024" });
    expect(parseLearningAuditReadError({ code: "pair_projection_capacity", resource: "rows", limit: 1024 })).toBeNull();
    expect(parseLearningAuditReadError({ code: "pair_projection_capacity", resource: "rows", limit: "100" })).toBeNull();
    expect(parseLearningAuditReadError("database path: private")).toBeNull();
  });
});
