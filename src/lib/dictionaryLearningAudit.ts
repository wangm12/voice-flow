/** Exact EN10 v1 native wire. Audit facts are never action or focus authority. */
type Reader<T> = (value: unknown) => T;
type ReadValue<R> = R extends Reader<infer T> ? T : never;
const invalid = (): never => { throw new Error("invalid_dictionary_learning_audit"); };
const text: Reader<string> = (v) => typeof v === "string" ? v : invalid();
const boolean: Reader<boolean> = (v) => typeof v === "boolean" ? v : invalid();
const u32: Reader<number> = (v) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= 0xffffffff ? v : invalid();
const decimal: Reader<string> = (v) => typeof v === "string" && /^(0|[1-9][0-9]*)$/.test(v) && v.length <= 20 && BigInt(v) <= 18446744073709551615n ? v : invalid();
const uuid: Reader<string> = (v) => typeof v === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(v) && v !== "00000000-0000-0000-0000-000000000000" ? v : invalid();
const literal = <T extends string | number | null>(expected: T): Reader<T> => (v) => v === expected ? expected : invalid();
const choice = <T extends string>(...values: T[]): Reader<T> => (v) => typeof v === "string" && values.includes(v as T) ? v as T : invalid();
const nullable = <T>(read: Reader<T>): Reader<T | null> => (v) => v === null ? null : read(v);
const object = <S extends Record<string, Reader<unknown>>>(shape: S): Reader<{ [K in keyof S]: ReadValue<S[K]> }> => (v) => {
  if (!v || typeof v !== "object" || Array.isArray(v)) return invalid();
  const row = v as Record<string, unknown>;
  const keys = Object.keys(shape);
  if (Object.keys(row).length !== keys.length || keys.some((key) => !Object.prototype.hasOwnProperty.call(row, key))) return invalid();
  return Object.fromEntries(keys.map((key) => [key, shape[key](row[key])])) as { [K in keyof S]: ReadValue<S[K]> };
};
const array = <T>(read: Reader<T>, maximum: number): Reader<T[]> => (v) => Array.isArray(v) && v.length <= maximum ? v.map(read) : invalid();

const classification = object({ class: choice("name", "term", "rejected"), reason: nullable(choice("unchanged_or_empty", "case_only", "surface_rejected")) });
const persistence = object({
  status: choice("not_attempted", "completed", "failed", "unknown"),
  reason: nullable(choice("pending", "db_promotion_recorded", "tombstone_or_ignored", "cap", "settings_save_completed", "store_error", "stale_or_cancelled", "candidate_db_returned", "style_draft_returned", "no_settings_change", "manual_scoped_pair_saved")),
});
const observation = object({
  kind: literal("post_paste_attempt"),
  registration: object({ status: choice("checking", "started", "skipped"), reason: nullable(choice("registered", "unverified", "secure", "unreadable", "stale_or_cancelled", "no_grant", "protected_target", "no_mapping", "ax_or_learning_not_allowed", "scene_not_allowed", "target_changed", "ambiguous_inserted_range", "fixture_baseline_mismatch")) }),
  verified_paste: boolean,
  outcome: choice("not_observed", "observing", "nochange", "target_changed", "unreadable", "single_token", "short_style_rewrite", "style_signal", "ambiguous", "stale_or_cancelled"),
  classification: nullable(classification),
  candidate_decision: choice("not_attempted", "pending", "db_promoted", "already_db_promoted", "rejected", "tombstone_or_ignored", "cap", "unknown", "scope_conflict", "duplicate_observation", "db_promotion_failed"),
  candidate_store: persistence, settings_store: persistence,
  attribution: choice("not_observed", "whole_field_difference_only"),
});
const review = object({
  kind: literal("review_action"),
  action: choice("add_dictionary_entries", "confirm_pair", "ignore_pair", "undo_pair", "remove_dictionary_word", "confirm_style_draft", "dismiss_style_draft", "add_manual_scoped_pair"),
  source: choice("dictionary_command", "manual_confirmation_with_history_reference"),
  history_reference: choice("not_provided", "lookup_succeeded", "lookup_unknown"),
  outcome: choice("requested", "completed", "failed", "stale"),
  candidate_store: persistence, settings_store: persistence,
});
const fact: Reader<ReadValue<typeof observation> | ReadValue<typeof review>> = (v) => {
  if (!v || typeof v !== "object") return invalid();
  return (v as { kind?: unknown }).kind === "post_paste_attempt" ? observation(v) : review(v);
};
const activity = object({
  activity_id: uuid, sequence: decimal, observer_epoch: nullable(decimal), created_at: text, pair_ref: nullable(uuid),
  lifecycle: choice("in_progress", "closed", "superseded", "unknown"), fact,
});
const pair = object({
  pair_ref: uuid, pair_key: text, before_surface: text, after_surface: text,
  rule_kind: choice("word_pair", "style_preference", "cleanup_intensity"),
  record_state: choice("pending", "db_promoted", "tombstoned"), candidate_hits: u32,
  current_classification: nullable(classification), current_promote_threshold: nullable(u32),
  classification_authority: choice("current_owner_classification", "not_word_pair"),
  recorded_scope: object({ family: nullable(text), mapping_id: nullable(text), browser_host: nullable(text), native_bundle: nullable(text) }),
  origin: literal("unknown_not_persisted"), tombstone_reason: nullable(literal("unknown_not_persisted")),
  latest_record_change_at: text, last_output_surface_match_at: nullable(text), current_dictionary_contains_after: boolean,
  replacement_policy: literal("context_scoped_promoted_rules"), replacement_eligibility: choice("eligible_now", "not_eligible_now"), eligibility_authority: literal("current_lexicon_owner_filter"),
  current_review: object({ confirm: boolean, ignore: boolean, undo: boolean }),
});
const snapshot = object({
  schema_version: literal(1), runtime_id: uuid, audit_revision: decimal, clear_generation: decimal, observer_epoch: decimal,
  coverage: object({
    kind: literal("current_process"), capacity: literal(128), dropped_entries: decimal, dropped_updates: decimal,
    oldest_sequence: nullable(decimal), latest_sequence: nullable(decimal),
    projection_consistency: choice("stable_revision", "changed_during_read"),
    atomicity: literal("not_cross_store_transaction"), consistency_scope: literal("audit_metadata_revision_only"),
    projection_mutation_interval: literal("not_observed"), projection_inflight_writers: literal(null),
    pair_projection: object({ kind: literal("complete_within_budget"), max_rows: literal(1024), max_utf8_bytes: literal(2097152), max_snapshot_encoded_bytes: literal(16777216) }),
  }),
  pairs: array(pair, 1024), activity: array(activity, 128),
});
export type DictionaryLearningAudit = ReadValue<typeof snapshot>;
export type LearningAuditPair = ReadValue<typeof pair>;
export type LearningAuditActivity = ReadValue<typeof activity>;
export type LearningAuditPersistence = ReadValue<typeof persistence>;
export type LearningAuditClassification = ReadValue<typeof classification>;
export type LearningAuditObservation = ReadValue<typeof observation>;
export type LearningAuditReview = ReadValue<typeof review>;

export function parseDictionaryLearningAudit(value: unknown): DictionaryLearningAudit | null {
  try {
    const result = snapshot(value);
    if (new Set(result.pairs.map((row) => row.pair_ref)).size !== result.pairs.length
      || new Set(result.pairs.map((row) => row.pair_key)).size !== result.pairs.length
      || new Set(result.activity.map((row) => row.activity_id)).size !== result.activity.length) return null;
    for (let i = 1; i < result.activity.length; i++) if (BigInt(result.activity[i].sequence) <= BigInt(result.activity[i - 1].sequence)) return null;
    if ((result.activity[0]?.sequence ?? null) !== result.coverage.oldest_sequence || (result.activity[result.activity.length - 1]?.sequence ?? null) !== result.coverage.latest_sequence) return null;
    return result;
  } catch { return null; }
}

const changed = object({ runtime_id: uuid, audit_revision: decimal });
export type LearningAuditChanged = ReadValue<typeof changed>;
export function parseLearningAuditChanged(value: unknown): LearningAuditChanged | null {
  try { return changed(value); } catch { return null; }
}
const simpleError = object({ code: choice("app_data_unavailable", "pair_store_unavailable", "audit_identity_unavailable", "audit_not_available_in_research_mode", "audit_authority_unavailable") });
const capacityError = object({ code: literal("pair_projection_capacity"), resource: choice("rows", "utf8_bytes", "encoded_bytes"), limit: decimal });
export type LearningAuditReadError = ReadValue<typeof simpleError> | ReadValue<typeof capacityError>;
export function parseLearningAuditReadError(value: unknown): LearningAuditReadError | null {
  try {
    if (value && typeof value === "object" && (value as { code?: unknown }).code === "pair_projection_capacity") {
      const error = capacityError(value);
      const limits = { rows: "1024", utf8_bytes: "2097152", encoded_bytes: "16777216" };
      return error.limit === limits[error.resource] ? error : null;
    }
    return simpleError(value);
  } catch { return null; }
}
