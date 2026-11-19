#[allow(unused_imports)]
use super::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const LEARN_PAIR_COLUMNS: &str = "pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, last_used_at, pinned, tombstoned_at, ignored, manual_scoped, auto_scoped, match_policy, binding_invalidated_at, binding_invalidated_reason, replacement_runs";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearnPairMatchPolicy {
    #[default]
    IgnoreCase,
    Exact,
}

impl LearnPairMatchPolicy {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::IgnoreCase => "ignore_case",
            Self::Exact => "exact",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnPairBindingInvalidationReason {
    MappingDeleted,
    DeclaredIdentityChanged,
    MissingBindingAtMigration,
}

impl LearnPairBindingInvalidationReason {
    fn as_db_str(self) -> &'static str {
        match self {
            Self::MappingDeleted => "mapping_deleted",
            Self::DeclaredIdentityChanged => "declared_identity_changed",
            Self::MissingBindingAtMigration => "missing_binding_at_migration",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LearnPairKey(String);

impl LearnPairKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Serialize)]
pub struct WordRuleIdentityV2<'a> {
    pub(super) version: u8,
    pub(super) kind: &'static str,
    pub(super) match_policy: LearnPairMatchPolicy,
    pub(super) alias_identity: String,
    pub(super) canonical_after: &'a str,
    pub(super) family: Option<&'a str>,
    pub(super) mapping_id: Option<&'a str>,
    pub(super) browser_host: Option<&'a str>,
    pub(super) native_bundle: Option<&'a str>,
}

pub fn word_rule_match_policy(before: &str, after: &str) -> LearnPairMatchPolicy {
    if before != after && before.eq_ignore_ascii_case(after) {
        LearnPairMatchPolicy::Exact
    } else {
        LearnPairMatchPolicy::IgnoreCase
    }
}

pub fn learn_pair_alias_matches(row: &LearnPairRecord, alias: &str) -> bool {
    match row.match_policy {
        LearnPairMatchPolicy::IgnoreCase => row.before_surface.eq_ignore_ascii_case(alias),
        LearnPairMatchPolicy::Exact => row.before_surface == alias,
    }
}

pub fn scoped_word_rule_key(
    before: &str,
    after: &str,
    scope: &LearnPairScope,
    match_policy: LearnPairMatchPolicy,
) -> LearnPairKey {
    let alias_identity = match match_policy {
        LearnPairMatchPolicy::Exact => before.to_owned(),
        LearnPairMatchPolicy::IgnoreCase => before.to_ascii_lowercase(),
    };
    let identity = WordRuleIdentityV2 {
        version: 2,
        kind: "word",
        match_policy,
        alias_identity,
        canonical_after: after,
        family: scope.family.as_deref(),
        mapping_id: scope.mapping_id.as_deref(),
        browser_host: scope.browser_host.as_deref(),
        native_bundle: scope.native_bundle.as_deref(),
    };
    let encoded = serde_json::to_vec(&identity).expect("word rule identity is serializable");
    let digest = hex::encode(Sha256::digest(encoded));
    LearnPairKey(format!("word:v2:{digest}"))
}

pub fn legacy_pair_key_for_surfaces(pair_key: &str, before: &str, after: &str) -> Option<String> {
    (pair_key.starts_with("word:v2:") && !before.is_empty())
        .then(|| format!("{}\u{1e}{}", before.to_lowercase(), after.to_lowercase()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnPairScope {
    pub family: Option<String>,
    pub mapping_id: Option<String>,
    pub browser_host: Option<String>,
    pub native_bundle: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnPairScopeMatch {
    Match,
    Mismatch,
    Unknown,
}

impl LearnPairScope {
    pub fn from_snapshot(snapshot: &crate::context::ContextSnapshot) -> Self {
        Self {
            family: Some(crate::context::family_id(snapshot.profile.family).to_owned()),
            mapping_id: snapshot.profile.id.strip_prefix("user.").map(str::to_owned),
            browser_host: snapshot.target_guard.browser_host.clone(),
            native_bundle: if snapshot.target_guard.browser_host.is_some() {
                None
            } else {
                snapshot.target_guard.bundle_id.clone()
            },
        }
    }

    /// A scoped pair applies only when every declared dimension is known and
    /// equal. An absent observed value is kept distinct from a mismatch.
    pub fn assess_record(&self, row: &LearnPairRecord) -> LearnPairScopeMatch {
        let dimensions = [
            (row.family.as_deref(), self.family.as_deref()),
            (row.mapping_id.as_deref(), self.mapping_id.as_deref()),
            (row.browser_host.as_deref(), self.browser_host.as_deref()),
            (row.native_bundle.as_deref(), self.native_bundle.as_deref()),
        ];
        let mut unknown = false;
        for (required, observed) in dimensions {
            match (required, observed) {
                (None, _) => {}
                (Some(_), None) => unknown = true,
                (Some(required), Some(observed)) if required != observed => {
                    return LearnPairScopeMatch::Mismatch;
                }
                (Some(_), Some(_)) => {}
            }
        }
        if unknown {
            LearnPairScopeMatch::Unknown
        } else {
            LearnPairScopeMatch::Match
        }
    }

    /// Select the one set of promoted pairs authorized for this observed
    /// context. Downstream replacement, hinting, usage, and final protection
    /// must all consume this set rather than reinterpreting scope fields.
    pub fn applicable_promoted_records(&self, rows: &[LearnPairRecord]) -> Vec<LearnPairRecord> {
        let applicable = rows
            .iter()
            .filter(|row| {
                row.is_live_promoted() && self.assess_record(row) == LearnPairScopeMatch::Match
            })
            .collect::<Vec<_>>();
        applicable
            .iter()
            .filter(|row| {
                !is_word_rule_candidate(row)
                    || !applicable.iter().any(|other| {
                        other.pair_key != row.pair_key
                            && is_word_rule_candidate(other)
                            && other.after_surface != row.after_surface
                            && learn_pair_alias_domains_intersect(row, other)
                    })
            })
            .map(|row| (*row).clone())
            .collect()
    }
}

pub fn is_scoped_word_rule(row: &LearnPairRecord) -> bool {
    row.pair_key.starts_with("word:v2:") && !row.before_surface.is_empty()
}

pub fn is_word_rule_candidate(row: &LearnPairRecord) -> bool {
    !row.before_surface.is_empty() && (is_scoped_word_rule(row) || row.pair_key.contains('\u{1e}'))
}

pub fn learn_pair_alias_domains_intersect(left: &LearnPairRecord, right: &LearnPairRecord) -> bool {
    match (left.match_policy, right.match_policy) {
        (LearnPairMatchPolicy::Exact, LearnPairMatchPolicy::Exact) => {
            left.before_surface == right.before_surface
        }
        (LearnPairMatchPolicy::IgnoreCase, LearnPairMatchPolicy::IgnoreCase) => left
            .before_surface
            .eq_ignore_ascii_case(&right.before_surface),
        (LearnPairMatchPolicy::Exact, LearnPairMatchPolicy::IgnoreCase) => left
            .before_surface
            .eq_ignore_ascii_case(&right.before_surface),
        (LearnPairMatchPolicy::IgnoreCase, LearnPairMatchPolicy::Exact) => left
            .before_surface
            .eq_ignore_ascii_case(&right.before_surface),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnPairRecord {
    pub pair_key: String,
    pub before_surface: String,
    pub after_surface: String,
    pub hits: u32,
    pub promoted: bool,
    pub last_at: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub mapping_id: Option<String>,
    #[serde(default)]
    pub browser_host: Option<String>,
    #[serde(default)]
    pub native_bundle: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub tombstoned_at: Option<String>,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub promote_hits: u32,
    #[serde(default)]
    pub manual_scoped: bool,
    #[serde(default)]
    pub auto_scoped: bool,
    #[serde(default)]
    pub match_policy: LearnPairMatchPolicy,
    #[serde(default)]
    pub binding_invalidated_at: Option<String>,
    #[serde(default)]
    pub binding_invalidated_reason: Option<String>,
    #[serde(default)]
    pub replacement_runs: u64,
}

impl LearnPairRecord {
    /// Whether this row declares any context scope. A family-only legacy row
    /// is still scoped for provider egress; missing newer flags do not erase
    /// that declaration.
    pub fn has_declared_scope(&self) -> bool {
        [
            self.family.as_deref(),
            self.mapping_id.as_deref(),
            self.browser_host.as_deref(),
            self.native_bundle.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty())
    }

    /// A pair can be sent as a provider hint only when it is a known global
    /// word rule. Manual/local-only rows, any declared scope, and unrecognized
    /// legacy identities fail closed. `after_surface` still needs independent
    /// global dictionary ownership at the call site.
    pub fn is_provider_egress_eligible_word_pair(&self) -> bool {
        if self.manual_scoped
            || self.auto_scoped
            || self.has_declared_scope()
            || self.before_surface.trim().is_empty()
            || self.after_surface.trim().is_empty()
            || self.pair_key.starts_with("learn:style:")
            || self.pair_key.starts_with("learn:intensity:")
        {
            return false;
        }
        self.pair_key.starts_with("word:v2:")
            || self.pair_key
                == format!(
                    "{}\u{1e}{}",
                    self.before_surface.to_lowercase(),
                    self.after_surface.to_lowercase()
                )
    }

    /// A dictionary owner is safe for global ASR term ranking when it is a
    /// known global word rule or an unscoped pinned term. Scoped screen seeds
    /// and local-only rules stay out of the global ranking path.
    pub fn is_provider_egress_eligible_dictionary_owner(&self) -> bool {
        if !self.before_surface.trim().is_empty() {
            return self.is_provider_egress_eligible_word_pair();
        }
        !self.manual_scoped
            && !self.auto_scoped
            && !self.has_declared_scope()
            && !self.after_surface.trim().is_empty()
            && !self.pair_key.starts_with("learn:style:")
            && !self.pair_key.starts_with("learn:intensity:")
            && self.pair_key == format!("\u{1e}{}", self.after_surface.to_lowercase())
    }

    pub fn is_live_promoted(&self) -> bool {
        self.promoted
            && !self.ignored
            && self.tombstoned_at.is_none()
            && self.binding_invalidated_at.is_none()
    }

    pub fn is_pending(&self) -> bool {
        !self.promoted
            && !self.ignored
            && self.tombstoned_at.is_none()
            && self.binding_invalidated_at.is_none()
    }

    pub fn is_binding_invalidated(&self) -> bool {
        self.binding_invalidated_at.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleDraftRecord {
    pub draft_key: String,
    pub mapping_id: String,
    pub style_key: String,
    pub excerpt: String,
    pub before_excerpt: String,
    pub after_excerpt: String,
    pub created_at: String,
}

/// A content-free evidence row owned by one committed History edit. The
/// source revision is a binding, while `origin_id` identifies the original
/// dictation transaction and therefore contributes at most one origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritingPreferenceEvidenceRecord {
    pub(crate) source_history_id: i64,
    pub(crate) source_revision_id: i64,
    pub(crate) mapping_id: String,
    pub(crate) mapping_identity_fingerprint: String,
    pub(crate) mapping_revision: u64,
    pub(crate) task_id: crate::context::WritingTaskId,
    pub(crate) dimension: crate::context::WritingPreferenceDimension,
    pub(crate) value: crate::context::WritingPreferenceValue,
    pub(crate) origin_id: String,
    pub(crate) content_fingerprint: String,
    pub(crate) template_family_fingerprint: Option<String>,
    pub(crate) occurred_at_ms: u64,
    pub(crate) editor_session_id: String,
    pub(crate) generation: u64,
    pub(crate) eligible: bool,
    pub(crate) natural_day: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritingPreferenceEvidenceInput {
    pub(crate) source_history_id: i64,
    pub(crate) source_revision_id: i64,
    pub(crate) mapping_id: String,
    pub(crate) mapping_identity_fingerprint: String,
    pub(crate) mapping_revision: u64,
    pub(crate) task_id: crate::context::WritingTaskId,
    pub(crate) dimension: crate::context::WritingPreferenceDimension,
    pub(crate) value: crate::context::WritingPreferenceValue,
    pub(crate) origin_id: String,
    pub(crate) content_fingerprint: String,
    pub(crate) template_family_fingerprint: Option<String>,
    pub(crate) occurred_at_ms: u64,
    /// Original dictation's device-local calendar day, not its later edit day.
    pub(crate) natural_day: String,
    pub(crate) editor_session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritingPreferenceDraftDecisionRecord {
    pub(crate) candidate_id: String,
    pub(crate) mapping_id: String,
    pub(crate) mapping_revision: u64,
    pub(crate) task_id: crate::context::WritingTaskId,
    pub(crate) dimension: crate::context::WritingPreferenceDimension,
    pub(crate) value: crate::context::WritingPreferenceValue,
    pub(crate) generation: u64,
    pub(crate) decision: WritingPreferenceDraftDecision,
    pub(crate) decided_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritingPreferenceDraftDecision {
    Dismissed,
    Approved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritingPreferenceEvidenceWrite {
    Inserted,
    Replaced,
    Duplicate,
    StaleOrigin,
    RejectedSource,
    AtCapacity,
}

pub fn learn_pair_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LearnPairRecord> {
    Ok(LearnPairRecord {
        pair_key: row.get(0)?,
        before_surface: row.get(1)?,
        after_surface: row.get(2)?,
        hits: row.get::<_, i64>(3)? as u32,
        promoted: row.get::<_, i64>(4)? != 0,
        last_at: row.get(5)?,
        family: row.get(6)?,
        mapping_id: row.get(7)?,
        browser_host: row.get(8)?,
        native_bundle: row.get(9)?,
        last_used_at: row.get(10)?,
        pinned: row.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0,
        tombstoned_at: row.get(12)?,
        ignored: row.get::<_, Option<i64>>(13)?.unwrap_or(0) != 0,
        promote_hits: 0,
        manual_scoped: row.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0,
        auto_scoped: row.get::<_, Option<i64>>(15)?.unwrap_or(0) != 0,
        match_policy: match row.get::<_, String>(16)?.as_str() {
            "ignore_case" => LearnPairMatchPolicy::IgnoreCase,
            "exact" => LearnPairMatchPolicy::Exact,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    16,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown learn pair match policy: {other}"),
                    )),
                ));
            }
        },
        binding_invalidated_at: row.get(17)?,
        binding_invalidated_reason: row.get(18)?,
        replacement_runs: row.get::<_, i64>(19)?.max(0) as u64,
    })
}

#[cfg(test)]
pub fn upsert_learn_pair(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
) -> anyhow::Result<Option<LearnPairRecord>> {
    Ok(
        match upsert_learn_pair_with_scope(dir, pair_key, before_surface, after_surface, None)? {
            LearnPairUpsert::Stored(row) => Some(*row),
            LearnPairUpsert::Skipped(_) => None,
        },
    )
}

/// The DB owner distinguishes intentional suppression from missing/failed data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnPairSkipReason {
    TombstoneOrIgnored,
    BindingInvalidated,
    PendingCapacity,
    ScopeConflict,
    DuplicateObservation,
    UnknownDictationOrigin,
}
impl LearnPairSkipReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::TombstoneOrIgnored => "tombstone_or_ignored",
            Self::BindingInvalidated => "mapping_binding_invalidated",
            Self::PendingCapacity => "cap",
            Self::ScopeConflict => "scope_conflict",
            Self::DuplicateObservation => "duplicate_observation",
            Self::UnknownDictationOrigin => "dictation_origin_unknown",
        }
    }
}
pub enum LearnPairUpsert {
    Stored(Box<LearnPairRecord>),
    Skipped(LearnPairSkipReason),
}

/// Opaque identity for one user-originated learning event. The key hashes the
/// source and pair identity; only History row/revision IDs are retained as
/// metadata so deleting the row also deletes its receipts.
#[derive(Clone, Debug)]
pub struct LearnPairObservation {
    pub(super) observation_key: String,
    pub(super) source_kind: &'static str,
    pub(super) source_history_id: Option<i64>,
    pub(super) source_revision_id: Option<i64>,
    pub(super) rule_key: Option<String>,
    pub(super) dictation_origin: Option<String>,
    pub(super) channel: Option<&'static str>,
    pub(super) proof_eligible: bool,
    pub(super) scoped_word_identity: bool,
}

impl LearnPairObservation {
    pub(crate) fn uses_scoped_word_identity(&self) -> bool {
        self.scoped_word_identity || matches!(self.channel, Some("paste" | "history_edit"))
    }

    pub(crate) fn history_edit(history_id: i64, revision_id: i64, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key(
                "history_edit",
                &history_id.to_string(),
                pair_key,
            ),
            source_kind: "history_edit",
            source_history_id: Some(history_id),
            source_revision_id: Some(revision_id),
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: None,
            channel: Some("history_edit"),
            proof_eligible: false,
            scoped_word_identity: true,
        }
    }

    pub(crate) fn history_edit_from_store(
        dir: &Path,
        history_id: i64,
        revision_id: i64,
        pair_key: &str,
    ) -> anyhow::Result<Option<Self>> {
        let connection = open_history(dir)?;
        let origin = connection
            .query_row(
                "SELECT d.transaction_id
                 FROM dictations d
                 JOIN dictation_revisions r ON r.dictation_id = d.id
                 WHERE d.id = ?1 AND r.revision_id = ?2",
                params![history_id, revision_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        let Some(origin) = origin else {
            return Ok(None);
        };
        let Some(origin) = origin.filter(|value| !value.is_empty()) else {
            return Ok(Some(Self::history_edit(history_id, revision_id, pair_key)));
        };
        Ok(Some(Self {
            observation_key: learn_observation_key(
                "history_edit",
                &format!("{origin}:{revision_id}"),
                pair_key,
            ),
            source_kind: "history_edit",
            source_history_id: Some(history_id),
            source_revision_id: Some(revision_id),
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: Some(origin),
            channel: Some("history_edit"),
            proof_eligible: true,
            scoped_word_identity: true,
        }))
    }

    pub(crate) fn paste_session(session_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("paste_session", session_id, pair_key),
            source_kind: "paste_session",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
            scoped_word_identity: false,
        }
    }

    pub(crate) fn paste_session_with_dictation_origin(
        session_id: &str,
        pair_key: &str,
        dictation_origin: &str,
    ) -> Self {
        Self {
            observation_key: learn_observation_key("paste_session", session_id, pair_key),
            source_kind: "paste_session",
            source_history_id: None,
            source_revision_id: None,
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: Some(dictation_origin.to_owned()),
            channel: Some("paste"),
            proof_eligible: true,
            scoped_word_identity: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn research_fixture_origin(source_origin_id: &str, pair_key: &str) -> Self {
        Self::research_fixture_origin_with_observation_id(
            source_origin_id,
            source_origin_id,
            pair_key,
        )
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_origin_with_observation_id(
        observation_id: &str,
        source_origin_id: &str,
        pair_key: &str,
    ) -> Self {
        let dictation_origin = format!("research_fixture:{source_origin_id}");
        Self {
            observation_key: learn_observation_key(
                "research_fixture_edit",
                observation_id,
                pair_key,
            ),
            source_kind: "research_fixture_edit",
            source_history_id: None,
            source_revision_id: None,
            rule_key: Some(pair_key.to_owned()),
            dictation_origin: Some(dictation_origin),
            channel: Some("research_fixture"),
            proof_eligible: true,
            scoped_word_identity: false,
        }
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_c1_origin_with_observation_id(
        observation_id: &str,
        source_origin_id: &str,
        pair_key: &str,
    ) -> Self {
        let mut obs = Self::research_fixture_origin_with_observation_id(
            observation_id,
            source_origin_id,
            pair_key,
        );
        obs.scoped_word_identity = true;
        obs
    }

    #[cfg(test)]
    pub(crate) fn proof_eligible(&self) -> bool {
        self.proof_eligible
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_edit(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("research_fixture_edit", operation_id, pair_key),
            source_kind: "research_fixture_edit",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
            scoped_word_identity: false,
        }
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_c1_edit(operation_id: &str, pair_key: &str) -> Self {
        let mut obs = Self::research_fixture_edit(operation_id, pair_key);
        obs.scoped_word_identity = true;
        obs
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_apply(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key(
                "research_fixture_apply",
                operation_id,
                pair_key,
            ),
            source_kind: "research_fixture_apply",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
            scoped_word_identity: false,
        }
    }

    #[cfg(feature = "research-harness")]
    pub(crate) fn research_fixture_noop(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("research_fixture_noop", operation_id, pair_key),
            source_kind: "research_fixture_noop",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
            scoped_word_identity: false,
        }
    }

    #[cfg(any(test, feature = "research-harness"))]
    pub(crate) fn research_fixture_undo(operation_id: &str, pair_key: &str) -> Self {
        Self {
            observation_key: learn_observation_key("research_fixture_undo", operation_id, pair_key),
            source_kind: "research_fixture_undo",
            source_history_id: None,
            source_revision_id: None,
            rule_key: None,
            dictation_origin: None,
            channel: None,
            proof_eligible: false,
            scoped_word_identity: false,
        }
    }
}

pub fn learn_observation_key(source_kind: &str, origin: &str, pair_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for value in [
        source_kind.as_bytes(),
        origin.as_bytes(),
        pair_key.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    hex::encode(digest.finalize())
}

pub enum VerifiedDictationOrigin {
    Committed(i64),
    Pending(SpoolIoCommitPermit),
    Fixture,
}

pub fn verify_dictation_origin(
    dir: &Path,
    connection: &Connection,
    observation: &LearnPairObservation,
) -> anyhow::Result<Option<VerifiedDictationOrigin>> {
    let (Some(rule_key), Some(dictation_origin), Some(channel)) = (
        observation.rule_key.as_deref(),
        observation.dictation_origin.as_deref(),
        observation.channel,
    ) else {
        return Ok(None);
    };
    let expected_source_kind = match channel {
        "paste" => "paste_session",
        "history_edit" => "history_edit",
        "research_fixture" => "research_fixture_edit",
        _ => return Ok(None),
    };
    if rule_key.is_empty()
        || rule_key.len() > 256
        || dictation_origin.is_empty()
        || dictation_origin.len() > 256
        || observation.source_kind != expected_source_kind
    {
        return Ok(None);
    }
    if channel == "research_fixture" {
        return Ok(dictation_origin
            .strip_prefix("research_fixture:")
            .filter(|source_id| !source_id.is_empty() && source_id.len() <= 240)
            .map(|_| VerifiedDictationOrigin::Fixture));
    }
    if let Some(history_id) = connection
        .query_row(
            "SELECT id FROM dictations WHERE transaction_id = ?1",
            [dictation_origin],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(Some(VerifiedDictationOrigin::Committed(history_id)));
    }
    if channel == "history_edit" {
        return Ok(None);
    }
    let pending_commit = try_spool_io_commit_pending_history(dir, dictation_origin)?;
    if !active_pending_history_origin(dir, dictation_origin)? {
        return Ok(None);
    }
    Ok(Some(VerifiedDictationOrigin::Pending(pending_commit)))
}

#[cfg(test)]
pub fn count_independent_learn_pair_hits(dir: &Path, pair_key: &str) -> anyhow::Result<u32> {
    let connection = open_history(dir)?;
    let count = connection.query_row(
        "SELECT COUNT(DISTINCT dictation_origin)
         FROM learn_observations
         WHERE rule_key = ?1
           AND proof_eligible = 1
           AND dictation_origin IS NOT NULL",
        [pair_key],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count as u32)
}

pub fn count_research_fixture_learn_pair_hits(dir: &Path, pair_key: &str) -> anyhow::Result<u32> {
    let connection = open_history(dir)?;
    let count = connection.query_row(
        "SELECT COUNT(DISTINCT dictation_origin)
         FROM learn_observations
         WHERE rule_key = ?1
           AND proof_eligible = 1
           AND dictation_origin IS NOT NULL
           AND source_kind = 'research_fixture_edit'
           AND channel = 'research_fixture'",
        [pair_key],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count as u32)
}

#[cfg(test)]
pub fn upsert_learn_pair_with_scope(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<LearnPairUpsert> {
    upsert_learn_pair_with_observation(dir, pair_key, before_surface, after_surface, scope, None)
}

pub fn upsert_learn_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<LearnPairUpsert> {
    upsert_learn_pair_with_observation_policy(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        observation,
        false,
        false,
    )
}

pub fn upsert_auto_scoped_learn_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<LearnPairUpsert> {
    upsert_learn_pair_with_observation_policy(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        observation,
        false,
        true,
    )
}

pub fn upsert_style_learning_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<LearnPairUpsert> {
    if !(pair_key.starts_with("learn:style:") || pair_key.starts_with("learn:intensity:")) {
        anyhow::bail!("text-refresh upsert is limited to internal style learning keys");
    }
    upsert_learn_pair_with_observation_policy(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        observation,
        true,
        false,
    )
}

#[cfg(any(test, feature = "research-harness"))]
pub fn has_learn_pair_observation(
    dir: &Path,
    observation: &LearnPairObservation,
) -> anyhow::Result<bool> {
    let connection = open_history(dir)?;
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM learn_observations WHERE observation_key = ?1)",
        [&observation.observation_key],
        |row| row.get(0),
    )?)
}

#[cfg(any(test, feature = "research-harness"))]
pub fn record_research_fixture_application(
    dir: &Path,
    pair_key: &str,
    after_surface: &str,
    observation: &LearnPairObservation,
    applied: bool,
) -> anyhow::Result<bool> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR IGNORE INTO learn_observations (observation_key, source_kind, source_history_id, source_revision_id, created_at) VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        params![
            observation.observation_key,
            observation.source_kind,
            observation.source_history_id,
            observation.source_revision_id
        ],
    )?;
    if transaction.changes() == 0 {
        return Ok(false);
    }
    if applied {
        transaction.execute(
            "UPDATE learn_pairs SET last_used_at = datetime('now'), replacement_runs = replacement_runs + 1 WHERE pair_key = ?1 AND after_surface = ?2 AND promoted = 1 AND ignored = 0 AND tombstoned_at IS NULL AND binding_invalidated_at IS NULL",
            params![pair_key, after_surface],
        )?;
        if transaction.changes() != 1 {
            anyhow::bail!("research application pair is no longer live");
        }
        transaction.execute(
            "INSERT INTO learned_term_usage (word, replacement_runs, last_replaced_at) VALUES (?1, 1, datetime('now')) ON CONFLICT(word) DO UPDATE SET replacement_runs = replacement_runs + 1, last_replaced_at = datetime('now')",
            [after_surface],
        )?;
    }
    transaction.execute(
        "DELETE FROM learn_observations WHERE source_history_id IS NULL AND observation_key IN (SELECT observation_key FROM learn_observations WHERE source_history_id IS NULL ORDER BY rowid DESC LIMIT -1 OFFSET 4096)",
        [],
    )?;
    transaction.commit()?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub fn upsert_learn_pair_with_observation_policy(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    observation: Option<&LearnPairObservation>,
    refresh_conflicting_text: bool,
    auto_scoped: bool,
) -> anyhow::Result<LearnPairUpsert> {
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(legacy_key) = legacy_pair_key_for_surfaces(pair_key, before_surface, after_surface)
    {
        let legacy_tombstoned: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM learn_pairs
                WHERE pair_key = ?1
                  AND before_surface != ''
                  AND (ignored = 1 OR tombstoned_at IS NOT NULL)
            )",
            [&legacy_key],
            |row| row.get(0),
        )?;
        if legacy_tombstoned {
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::TombstoneOrIgnored,
            ));
        }
    }
    if let Some(existing) = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()?
    {
        if existing.ignored || existing.tombstoned_at.is_some() {
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::TombstoneOrIgnored,
            ));
        }
        if existing.is_binding_invalidated() {
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::BindingInvalidated,
            ));
        }
        let existing_scope = LearnPairScope {
            family: existing.family.clone(),
            mapping_id: existing.mapping_id.clone(),
            browser_host: existing.browser_host.clone(),
            native_bundle: existing.native_bundle.clone(),
        };
        if existing_scope != scope.cloned().unwrap_or_default()
            || existing.match_policy != word_rule_match_policy(before_surface, after_surface)
            || (!refresh_conflicting_text
                && (!learn_pair_alias_matches(&existing, before_surface)
                    || existing.after_surface != after_surface))
        {
            return Ok(LearnPairUpsert::Skipped(LearnPairSkipReason::ScopeConflict));
        }
    } else {
        let pending: i64 = tx.query_row(
            "SELECT COUNT(*) FROM learn_pairs WHERE promoted = 0 AND ignored = 0 AND tombstoned_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        if pending >= LEARN_PAIRS_PENDING_CAP {
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::PendingCapacity,
            ));
        }
    }
    let mut pending_origin_commit = None;
    if let Some(observation) = observation {
        let source_history_id = if observation.proof_eligible {
            if observation.rule_key.as_deref() != Some(pair_key) {
                return Ok(LearnPairUpsert::Skipped(
                    LearnPairSkipReason::UnknownDictationOrigin,
                ));
            }
            match verify_dictation_origin(dir, &tx, observation)? {
                Some(VerifiedDictationOrigin::Committed(history_id)) => Some(history_id),
                Some(VerifiedDictationOrigin::Pending(commit)) => {
                    pending_origin_commit = Some(commit);
                    None
                }
                Some(VerifiedDictationOrigin::Fixture) => None,
                None => {
                    return Ok(LearnPairUpsert::Skipped(
                        LearnPairSkipReason::UnknownDictationOrigin,
                    ));
                }
            }
        } else {
            observation.source_history_id
        };
        tx.execute(
            "INSERT OR IGNORE INTO learn_observations (
                observation_key, source_kind, source_history_id, source_revision_id,
                created_at, rule_key, dictation_origin, channel, proof_eligible
             ) VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5, ?6, ?7, ?8)",
            params![
                observation.observation_key,
                observation.source_kind,
                source_history_id,
                observation.source_revision_id,
                observation.rule_key,
                observation.dictation_origin,
                observation.channel,
                observation.proof_eligible
            ],
        )?;
        if tx.changes() == 0 {
            if observation.proof_eligible {
                tx.execute(
                    "UPDATE learn_observations
                     SET source_history_id = COALESCE(source_history_id, ?3),
                         source_revision_id = COALESCE(source_revision_id, ?4),
                         channel = CASE WHEN ?5 = 'history_edit' THEN ?5 ELSE channel END
                     WHERE rule_key = ?1 AND dictation_origin = ?2 AND proof_eligible = 1",
                    params![
                        observation.rule_key,
                        observation.dictation_origin,
                        source_history_id,
                        observation.source_revision_id,
                        observation.channel
                    ],
                )?;
            }
            if auto_scoped {
                tx.execute(
                    "UPDATE learn_pairs
                     SET auto_scoped = 1
                     WHERE pair_key = ?1
                       AND promoted = 0
                       AND COALESCE(manual_scoped, 0) = 0
                       AND ignored = 0
                       AND tombstoned_at IS NULL
                       AND binding_invalidated_at IS NULL",
                    [pair_key],
                )?;
            }
            tx.commit()?;
            return Ok(LearnPairUpsert::Skipped(
                LearnPairSkipReason::DuplicateObservation,
            ));
        }
        // Paste receipts only need to cover recently completed process/session
        // retries. History receipts live with their parent History row.
        tx.execute(
            "DELETE FROM learn_observations WHERE source_history_id IS NULL AND observation_key IN (SELECT observation_key FROM learn_observations WHERE source_history_id IS NULL ORDER BY rowid DESC LIMIT -1 OFFSET 4096)",
            [],
        )?;
    }
    let scope = scope.cloned().unwrap_or_default();
    let match_policy = word_rule_match_policy(before_surface, after_surface);
    let before_update = if refresh_conflicting_text {
        "excluded.before_surface"
    } else {
        "learn_pairs.before_surface"
    };
    let after_update = if refresh_conflicting_text {
        "excluded.after_surface"
    } else {
        "learn_pairs.after_surface"
    };
    let legacy_pair_key = legacy_pair_key_for_surfaces(pair_key, before_surface, after_surface);
    let record = tx.query_row(
        &format!(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, last_used_at, pinned, tombstoned_at, ignored, match_policy, legacy_pair_key, auto_scoped)
             VALUES (?1, ?2, ?3, 1, 0, datetime('now'), ?4, ?5, ?6, ?7, NULL, 0, NULL, 0, ?8, ?9, ?10)
             ON CONFLICT(pair_key) DO UPDATE SET
               before_surface = {before_update},
               after_surface = {after_update},
               hits = hits + 1,
               last_at = datetime('now'),
               family = excluded.family,
               mapping_id = excluded.mapping_id,
               browser_host = excluded.browser_host,
               native_bundle = excluded.native_bundle,
               match_policy = excluded.match_policy,
               legacy_pair_key = COALESCE(learn_pairs.legacy_pair_key, excluded.legacy_pair_key),
               auto_scoped = CASE
                 WHEN ?10 AND learn_pairs.promoted = 0 THEN 1
                 ELSE learn_pairs.auto_scoped
               END
             RETURNING {LEARN_PAIR_COLUMNS}"
        ),
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle,
            match_policy.as_db_str(),
            legacy_pair_key,
            auto_scoped
        ],
        learn_pair_from_row,
    )?;
    tx.commit()?;
    drop(pending_origin_commit);
    Ok(LearnPairUpsert::Stored(Box::new(record)))
}

pub fn get_learn_pair(dir: &Path, pair_key: &str) -> anyhow::Result<Option<LearnPairRecord>> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
        [pair_key],
        learn_pair_from_row,
    )
    .optional()?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictionaryEffectAction {
    Add,
    Remove,
}

impl DictionaryEffectAction {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }

    pub(super) fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "add" => Ok(Self::Add),
            "remove" => Ok(Self::Remove),
            _ => anyhow::bail!("invalid pending dictionary effect"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingDictionaryEffect {
    pub pair_key: String,
    pub word: String,
    pub action: DictionaryEffectAction,
    pub version: i64,
    pub is_global_rule: bool,
}

pub fn has_unscoped_columns(
    family: Option<&str>,
    mapping_id: Option<&str>,
    browser_host: Option<&str>,
    native_bundle: Option<&str>,
) -> bool {
    [family, mapping_id, browser_host, native_bundle]
        .into_iter()
        .flatten()
        .all(|val| val.trim().is_empty())
}

/// Whether a confirmed word rule syncs into `Settings.dictionary` (as `GlobalRule` when
/// unscoped, or mapping-bound `Rule` when a scope is recorded). Manual/auto local-only rows stay
/// out of `Settings.dictionary`, while `list_pending_dictionary_effects` gates `is_global_rule`
/// (and `GlobalRule` / provider egress) on empty `[family, mapping_id, browser_host, native_bundle]`.
fn is_settings_dictionary_word_rule(
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    manual_scoped: bool,
    auto_scoped: bool,
) -> bool {
    !before_surface.trim().is_empty()
        && !after_surface.trim().is_empty()
        && !manual_scoped
        && !auto_scoped
        && !pair_key.starts_with("learn:style:")
        && !pair_key.starts_with("learn:intensity:")
}

fn stage_dictionary_effect(
    transaction: &rusqlite::Transaction<'_>,
    pair_key: &str,
    action: DictionaryEffectAction,
) -> anyhow::Result<()> {
    transaction.execute(
        "UPDATE learn_pairs
         SET dictionary_pending_action = ?2,
             dictionary_effect_version = dictionary_effect_version + 1
         WHERE pair_key = ?1",
        params![pair_key, action.as_str()],
    )?;
    Ok(())
}

pub fn list_pending_dictionary_effects(dir: &Path) -> anyhow::Result<Vec<PendingDictionaryEffect>> {
    let connection = open_history(dir)?;
    let mut statement = connection.prepare(
        "SELECT pair_key, after_surface, dictionary_pending_action, dictionary_effect_version,
                family, mapping_id, browser_host, native_bundle, manual_scoped, auto_scoped
         FROM learn_pairs
         WHERE dictionary_pending_action != 'none'
         ORDER BY dictionary_effect_version, pair_key",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<i64>>(8)?.unwrap_or(0) != 0,
            row.get::<_, Option<i64>>(9)?.unwrap_or(0) != 0,
        ))
    })?;
    rows.map(|row| {
        let (
            pair_key,
            word,
            action,
            version,
            family,
            mapping_id,
            browser_host,
            native_bundle,
            manual_scoped,
            auto_scoped,
        ) = row?;
        let is_unscoped = has_unscoped_columns(
            family.as_deref(),
            mapping_id.as_deref(),
            browser_host.as_deref(),
            native_bundle.as_deref(),
        );
        let is_global_rule = is_unscoped
            && !manual_scoped
            && !auto_scoped
            && !pair_key.starts_with("learn:style:")
            && !pair_key.starts_with("learn:intensity:");
        Ok(PendingDictionaryEffect {
            pair_key,
            word,
            action: DictionaryEffectAction::from_str(&action)?,
            version,
            is_global_rule,
        })
    })
    .collect()
}

pub fn acknowledge_dictionary_effect(
    dir: &Path,
    pair_key: &str,
    version: i64,
) -> anyhow::Result<bool> {
    let connection = open_history(dir)?;
    connection.execute(
        "UPDATE learn_pairs
         SET dictionary_pending_action = 'none'
         WHERE pair_key = ?1
           AND dictionary_effect_version = ?2
           AND dictionary_pending_action != 'none'",
        params![pair_key, version],
    )?;
    Ok(connection.changes() == 1)
}

pub fn cancel_pending_dictionary_effects_for_word(dir: &Path, word: &str) -> anyhow::Result<bool> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "UPDATE learn_pairs
         SET dictionary_pending_action = 'none',
             dictionary_effect_version = dictionary_effect_version + 1
         WHERE after_surface = ?1 COLLATE NOCASE
           AND dictionary_pending_action = 'add'",
        [word],
    )?;
    let cancelled = transaction.changes() > 0;
    transaction.commit()?;
    Ok(cancelled)
}

#[cfg(test)]
pub fn mark_learn_pair_promoted(dir: &Path, pair_key: &str) -> anyhow::Result<bool> {
    mark_learn_pair_promoted_with_scope_kind(dir, pair_key, false)
}

#[cfg(test)]
pub fn mark_learn_pair_promoted_with_scope_kind(
    dir: &Path,
    pair_key: &str,
    auto_scoped: bool,
) -> anyhow::Result<bool> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let prior = transaction
        .query_row(
            "SELECT before_surface, after_surface, promoted, ignored, tombstoned_at,
                    manual_scoped, auto_scoped, dictionary_pending_action
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let prior_global = prior.as_ref().is_some_and(
        |(before, after, promoted, ignored, tombstoned_at, manual_scoped, prior_auto, _)| {
            *promoted
                && !*ignored
                && tombstoned_at.is_none()
                && is_settings_dictionary_word_rule(
                    pair_key,
                    before,
                    after,
                    *manual_scoped,
                    *prior_auto,
                )
        },
    );
    let had_pending_global_add = prior.as_ref().is_some_and(
        |(before, after, _, _, _, manual_scoped, prior_auto, action)| {
            action == "add"
                && is_settings_dictionary_word_rule(
                    pair_key,
                    before,
                    after,
                    *manual_scoped,
                    *prior_auto,
                )
        },
    );
    transaction.execute(
        "UPDATE learn_pairs SET promoted = 1, ignored = 0, tombstoned_at = NULL, auto_scoped = CASE WHEN ?2 THEN 1 ELSE auto_scoped END, last_at = datetime('now') WHERE pair_key = ?1",
        params![pair_key, auto_scoped],
    )?;
    let changed = transaction.changes() > 0;
    if changed {
        let (before, after, manual_scoped, stored_auto_scoped) = transaction.query_row(
            "SELECT before_surface, after_surface, manual_scoped, auto_scoped
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                ))
            },
        )?;
        let is_global = is_settings_dictionary_word_rule(
            pair_key,
            &before,
            &after,
            manual_scoped,
            stored_auto_scoped,
        );
        if is_global {
            stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Add)?;
        } else if prior_global || had_pending_global_add {
            stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Remove)?;
        }
    }
    transaction.commit()?;
    Ok(changed)
}

/// Promote an automatically learned pair only while its exact candidate is
/// still live, in the same scope, and at its required hit threshold. This
/// conditional update never clears a tombstone or ignored marker.
#[cfg(test)]
pub fn promote_learn_pair_if_eligible(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    threshold: u32,
    auto_scoped: bool,
) -> anyhow::Result<bool> {
    Ok(matches!(
        promote_learn_pair_if_eligible_with_status(
            dir,
            pair_key,
            before_surface,
            after_surface,
            scope,
            threshold,
            auto_scoped,
        )?,
        LearnPairPromotionStatus::Promoted
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearnPairPromotionStatus {
    Promoted,
    Ineligible,
    CrossScopeAutoBlocked,
}

pub fn promote_learn_pair_if_eligible_with_status(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    threshold: u32,
    auto_scoped: bool,
) -> anyhow::Result<LearnPairPromotionStatus> {
    let scope = scope.cloned().unwrap_or_default();
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(legacy_pair_key) =
        legacy_pair_key_for_surfaces(pair_key, before_surface, after_surface)
    {
        let competing_live_rule: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM learn_pairs
                WHERE pair_key != ?1
                  AND before_surface != ''
                  AND (legacy_pair_key = ?2 OR pair_key = ?2)
                  AND promoted = 1
                  AND ignored = 0
                  AND tombstoned_at IS NULL
                  AND binding_invalidated_at IS NULL
            )",
            params![pair_key, legacy_pair_key],
            |row| row.get(0),
        )?;
        if competing_live_rule {
            return Ok(LearnPairPromotionStatus::CrossScopeAutoBlocked);
        }
    }
    let changed = transaction.execute(
        "UPDATE learn_pairs
         SET promoted = 1,
             auto_scoped = CASE WHEN ?9 THEN 1 ELSE auto_scoped END,
             last_at = datetime('now')
         WHERE pair_key = ?1
           AND before_surface = ?2
           AND after_surface = ?3
           AND family IS ?4
           AND mapping_id IS ?5
           AND browser_host IS ?6
           AND native_bundle IS ?7
           AND (?2 = '' AND hits >= ?8 OR ?2 != '' AND (
               SELECT COUNT(DISTINCT dictation_origin)
               FROM learn_observations
               WHERE rule_key = ?1
                 AND proof_eligible = 1
                 AND dictation_origin IS NOT NULL
                 AND source_kind = 'research_fixture_edit'
                 AND channel = 'research_fixture'
           ) >= ?8)
           AND promoted = 0
           AND ignored = 0
           AND tombstoned_at IS NULL
           AND binding_invalidated_at IS NULL",
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle,
            threshold as i64,
            auto_scoped
        ],
    )?;
    if changed > 0 {
        let (before, after, manual_scoped, stored_auto_scoped) = transaction.query_row(
            "SELECT before_surface, after_surface, manual_scoped, auto_scoped
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                ))
            },
        )?;
        if is_settings_dictionary_word_rule(
            pair_key,
            &before,
            &after,
            manual_scoped,
            stored_auto_scoped,
        ) {
            stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Add)?;
        }
    }
    transaction.commit()?;
    Ok(if changed > 0 {
        LearnPairPromotionStatus::Promoted
    } else {
        LearnPairPromotionStatus::Ineligible
    })
}

pub fn ensure_learn_pair_promoted(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<()> {
    ensure_learn_pair_promoted_with_local_only(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        false,
    )
}

pub fn ensure_local_only_learn_pair_promoted(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
) -> anyhow::Result<()> {
    ensure_learn_pair_promoted_with_local_only(
        dir,
        pair_key,
        before_surface,
        after_surface,
        scope,
        true,
    )
}

pub fn ensure_learn_pair_promoted_with_local_only(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: Option<&LearnPairScope>,
    local_only: bool,
) -> anyhow::Result<()> {
    let scope = scope.ok_or_else(|| anyhow::anyhow!("learn pair scope is required"))?;
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing.is_binding_invalidated() {
            anyhow::bail!("learn pair mapping binding is invalidated");
        }
        let existing_scope = LearnPairScope {
            family: existing.family.clone(),
            mapping_id: existing.mapping_id.clone(),
            browser_host: existing.browser_host.clone(),
            native_bundle: existing.native_bundle.clone(),
        };
        if existing_scope != *scope {
            anyhow::bail!("learn pair scope conflicts with the recorded scope");
        }
        let match_policy = word_rule_match_policy(before_surface, after_surface);
        if existing.match_policy != match_policy
            || !learn_pair_alias_matches(&existing, before_surface)
            || existing.after_surface != after_surface
        {
            anyhow::bail!("learn pair surface conflicts with the recorded pair key");
        }
    }
    tx.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored, manual_scoped, auto_scoped, match_policy, legacy_pair_key)
         VALUES (?1, ?2, ?3, CASE WHEN ?10 THEN 1 ELSE 3 END, 1, datetime('now'), ?4, ?5, ?6, ?7, 0, 0, 0, ?10, ?8, ?9)
         ON CONFLICT(pair_key) DO UPDATE SET
           promoted = 1,
           ignored = 0,
           tombstoned_at = NULL,
           last_at = datetime('now'),
           family = excluded.family,
           mapping_id = excluded.mapping_id,
           browser_host = excluded.browser_host,
           native_bundle = excluded.native_bundle,
           auto_scoped = CASE
             WHEN ?10 AND learn_pairs.promoted = 0 THEN 1
             ELSE learn_pairs.auto_scoped
           END,
           legacy_pair_key = COALESCE(learn_pairs.legacy_pair_key, excluded.legacy_pair_key)",
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle,
            word_rule_match_policy(before_surface, after_surface).as_db_str(),
            legacy_pair_key_for_surfaces(pair_key, before_surface, after_surface),
            local_only
        ],
    )?;
    let (before, after, manual_scoped, auto_scoped) = tx.query_row(
        "SELECT before_surface, after_surface, manual_scoped, auto_scoped
         FROM learn_pairs WHERE pair_key = ?1",
        [pair_key],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
            ))
        },
    )?;
    if !local_only
        && is_settings_dictionary_word_rule(pair_key, &before, &after, manual_scoped, auto_scoped)
    {
        // Explicit confirmation also repairs a missing Settings projection for
        // an already-promoted global rule. The Settings owner applies this
        // source idempotently before conditionally acknowledging this version.
        stage_dictionary_effect(&tx, pair_key, DictionaryEffectAction::Add)?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug)]
pub enum ManualScopedPairStoreError {
    InvalidScope,
    ScopeConflict,
    BindingInvalidated,
    #[expect(
        dead_code,
        reason = "retain the private storage cause for internal diagnostics while code() exposes only a stable category"
    )]
    Storage(anyhow::Error),
}

impl ManualScopedPairStoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidScope => "manual_pair_scope_unknown",
            Self::ScopeConflict => "manual_pair_scope_conflict",
            Self::BindingInvalidated => "manual_pair_binding_invalidated",
            Self::Storage(_) => "manual_pair_save_failed",
        }
    }
}

pub fn ensure_learn_pair_promoted_as_global(
    dir: &Path,
    original_pair_key: &str,
    global_pair_key: &str,
    before_surface: &str,
    after_surface: &str,
) -> anyhow::Result<()> {
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let match_policy = word_rule_match_policy(before_surface, after_surface);
    if let Some(existing) = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [global_pair_key],
            learn_pair_from_row,
        )
        .optional()?
    {
        if existing.match_policy != match_policy
            || !learn_pair_alias_matches(&existing, before_surface)
            || existing.after_surface != after_surface
        {
            anyhow::bail!("learn pair surface conflicts with the recorded global rule");
        }
    }
    let original_hits: u32 = tx
        .query_row(
            "SELECT hits FROM learn_pairs WHERE pair_key = ?1",
            [original_pair_key],
            |row| row.get::<_, u32>(0),
        )
        .optional()?
        .unwrap_or(1)
        .max(1);
    tx.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored, manual_scoped, auto_scoped, match_policy, legacy_pair_key, binding_invalidated_at, binding_invalidated_reason)
         VALUES (?1, ?2, ?3, ?6, 1, datetime('now'), NULL, NULL, NULL, NULL, 0, 0, 0, 0, ?4, ?5, NULL, NULL)
         ON CONFLICT(pair_key) DO UPDATE SET
           hits = MAX(learn_pairs.hits, excluded.hits),
           promoted = 1,
           ignored = 0,
           tombstoned_at = NULL,
           last_at = datetime('now'),
           family = NULL,
           mapping_id = NULL,
           browser_host = NULL,
           native_bundle = NULL,
           manual_scoped = 0,
           auto_scoped = 0,
           binding_invalidated_at = NULL,
           binding_invalidated_reason = NULL,
           legacy_pair_key = COALESCE(learn_pairs.legacy_pair_key, excluded.legacy_pair_key)",
        params![
            global_pair_key,
            before_surface,
            after_surface,
            match_policy.as_db_str(),
            legacy_pair_key_for_surfaces(global_pair_key, before_surface, after_surface),
            original_hits as i64,
        ],
    )?;
    stage_dictionary_effect(&tx, global_pair_key, DictionaryEffectAction::Add)?;
    if original_pair_key != global_pair_key {
        tx.execute(
            "DELETE FROM learn_pairs WHERE pair_key = ?1 AND promoted = 0",
            [original_pair_key],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn ensure_manual_global_learn_pair(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
) -> Result<LearnPairRecord, ManualScopedPairStoreError> {
    let mut c = open_history(dir).map_err(ManualScopedPairStoreError::Storage)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let existing = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let match_policy = word_rule_match_policy(before_surface, after_surface);
    if let Some(existing) = existing {
        if existing.match_policy != match_policy
            || !learn_pair_alias_matches(&existing, before_surface)
            || existing.after_surface != after_surface
        {
            return Err(ManualScopedPairStoreError::ScopeConflict);
        }
    }
    let row = tx
        .query_row(
            &format!(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored, manual_scoped, auto_scoped, match_policy, legacy_pair_key, binding_invalidated_at, binding_invalidated_reason)
                 VALUES (?1, ?2, ?3, 0, 1, datetime('now'), NULL, NULL, NULL, NULL, 0, 0, 0, 0, ?4, ?5, NULL, NULL)
                 ON CONFLICT(pair_key) DO UPDATE SET
                   promoted = 1,
                   ignored = 0,
                   tombstoned_at = NULL,
                   last_at = datetime('now'),
                   family = NULL,
                   mapping_id = NULL,
                   browser_host = NULL,
                   native_bundle = NULL,
                   manual_scoped = 0,
                   auto_scoped = 0,
                   binding_invalidated_at = NULL,
                   binding_invalidated_reason = NULL,
                   match_policy = excluded.match_policy,
                   legacy_pair_key = COALESCE(learn_pairs.legacy_pair_key, excluded.legacy_pair_key)
                 RETURNING {LEARN_PAIR_COLUMNS}"
            ),
            params![
                pair_key,
                before_surface,
                after_surface,
                match_policy.as_db_str(),
                legacy_pair_key_for_surfaces(pair_key, before_surface, after_surface)
            ],
            learn_pair_from_row,
        )
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    stage_dictionary_effect(&tx, pair_key, DictionaryEffectAction::Add)
        .map_err(ManualScopedPairStoreError::Storage)?;
    tx.commit()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    Ok(row)
}

pub fn ensure_manual_scoped_learn_pair(
    dir: &Path,
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: &LearnPairScope,
) -> Result<LearnPairRecord, ManualScopedPairStoreError> {
    let has_bundle = scope
        .native_bundle
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let has_host = scope.browser_host.as_deref().is_some_and(|s| !s.is_empty());
    if has_bundle == has_host {
        return Err(ManualScopedPairStoreError::InvalidScope);
    }
    let mut c = open_history(dir).map_err(ManualScopedPairStoreError::Storage)?;
    let tx = c
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let existing = tx
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let was_global_promoted = existing.as_ref().is_some_and(|existing| {
        existing.is_live_promoted()
            && is_settings_dictionary_word_rule(
                pair_key,
                &existing.before_surface,
                &existing.after_surface,
                existing.manual_scoped,
                existing.auto_scoped,
            )
    });
    let had_pending_global_add = tx
        .query_row(
            "SELECT dictionary_pending_action = 'add' FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?
        .unwrap_or(false);
    if let Some(existing) = existing {
        if existing.is_binding_invalidated() {
            return Err(ManualScopedPairStoreError::BindingInvalidated);
        }
        let existing_scope = LearnPairScope {
            family: existing.family.clone(),
            mapping_id: existing.mapping_id.clone(),
            browser_host: existing.browser_host.clone(),
            native_bundle: existing.native_bundle.clone(),
        };
        let match_policy = word_rule_match_policy(before_surface, after_surface);
        if existing.match_policy != match_policy
            || !learn_pair_alias_matches(&existing, before_surface)
            || existing.after_surface != after_surface
        {
            return Err(ManualScopedPairStoreError::ScopeConflict);
        }
        // A user may explicitly rebind an existing exact pair only after its
        // previous rule has been disarmed. Active rules remain scope-bound.
        if existing_scope != *scope && !existing.ignored && existing.tombstoned_at.is_none() {
            return Err(ManualScopedPairStoreError::ScopeConflict);
        }
    }
    let row = tx.query_row(
        &format!(
            "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, browser_host, native_bundle, pinned, ignored, manual_scoped, match_policy, legacy_pair_key)
         VALUES (?1, ?2, ?3, 0, 1, datetime('now'), ?4, ?5, ?6, ?7, 0, 0, 1, ?8, ?9)
         ON CONFLICT(pair_key) DO UPDATE SET
           promoted = 1,
           ignored = 0,
           tombstoned_at = NULL,
           last_at = datetime('now'),
           family = excluded.family,
           mapping_id = excluded.mapping_id,
           browser_host = excluded.browser_host,
           native_bundle = excluded.native_bundle,
           manual_scoped = 1,
           auto_scoped = 0,
           match_policy = excluded.match_policy,
           legacy_pair_key = COALESCE(learn_pairs.legacy_pair_key, excluded.legacy_pair_key)
         RETURNING {LEARN_PAIR_COLUMNS}"
        ),
        params![
            pair_key,
            before_surface,
            after_surface,
            scope.family,
            scope.mapping_id,
            scope.browser_host,
            scope.native_bundle,
            word_rule_match_policy(before_surface, after_surface).as_db_str(),
            legacy_pair_key_for_surfaces(pair_key, before_surface, after_surface)
        ],
        learn_pair_from_row,
    ).map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    if was_global_promoted || had_pending_global_add {
        stage_dictionary_effect(&tx, pair_key, DictionaryEffectAction::Remove)
            .map_err(ManualScopedPairStoreError::Storage)?;
    }
    tx.commit()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    Ok(row)
}

pub fn set_learn_pair_hits(dir: &Path, pair_key: &str, hits: u32) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE learn_pairs SET hits = ?2, promoted = 0, last_at = datetime('now') WHERE pair_key = ?1 AND ignored = 0 AND tombstoned_at IS NULL",
        params![pair_key, hits as i64],
    )?;
    Ok(c.changes() > 0)
}

pub fn tombstone_learn_pair_with_observation(
    dir: &Path,
    pair_key: &str,
    observation: Option<&LearnPairObservation>,
) -> anyhow::Result<(bool, bool)> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let prior = transaction
        .query_row(
            "SELECT before_surface, after_surface, promoted, ignored, tombstoned_at,
                    manual_scoped, auto_scoped, dictionary_pending_action
             FROM learn_pairs WHERE pair_key = ?1",
            [pair_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
                    row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let should_remove_dictionary_effect = prior.as_ref().is_some_and(
        |(before, after, promoted, ignored, tombstoned_at, manual_scoped, auto_scoped, action)| {
            is_settings_dictionary_word_rule(pair_key, before, after, *manual_scoped, *auto_scoped)
                && ((*promoted && !*ignored && tombstoned_at.is_none()) || action == "add")
        },
    );
    if let Some(observation) = observation {
        transaction.execute(
            "INSERT OR IGNORE INTO learn_observations (observation_key, source_kind, source_history_id, source_revision_id, created_at) VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![
                observation.observation_key,
                observation.source_kind,
                observation.source_history_id,
                observation.source_revision_id
            ],
        )?;
        if transaction.changes() == 0 {
            return Ok((false, true));
        }
        transaction.execute(
            "DELETE FROM learn_observations WHERE source_history_id IS NULL AND observation_key IN (SELECT observation_key FROM learn_observations WHERE source_history_id IS NULL ORDER BY rowid DESC LIMIT -1 OFFSET 4096)",
            [],
        )?;
    }
    if observation.is_some() {
        transaction.execute(
            "UPDATE learn_pairs SET ignored = 1, tombstoned_at = datetime('now'), last_at = datetime('now') WHERE pair_key = ?1 AND tombstoned_at IS NULL",
            [pair_key],
        )?;
    } else {
        transaction.execute(
            "UPDATE learn_pairs SET ignored = 1, tombstoned_at = datetime('now'), last_at = datetime('now') WHERE pair_key = ?1",
            [pair_key],
        )?;
    }
    let changed = transaction.changes() > 0;
    if observation.is_some() && !changed {
        anyhow::bail!("research undo pair is no longer live");
    }
    if changed && should_remove_dictionary_effect {
        stage_dictionary_effect(&transaction, pair_key, DictionaryEffectAction::Remove)?;
    }
    transaction.commit()?;
    Ok((changed, false))
}

pub fn tombstone_learn_pair(dir: &Path, pair_key: &str) -> anyhow::Result<bool> {
    Ok(tombstone_learn_pair_with_observation(dir, pair_key, None)?.0)
}

pub fn invalidate_word_rule_mapping_bindings(
    dir: &Path,
    changes: &[(String, LearnPairBindingInvalidationReason)],
) -> anyhow::Result<usize> {
    if changes.is_empty() {
        return Ok(0);
    }
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut changed = 0usize;
    for (mapping_id, reason) in changes {
        changed += invalidate_mapping_bound_word_rules(&transaction, mapping_id, *reason)?;
    }
    transaction.commit()?;
    Ok(changed)
}

pub fn invalidate_missing_word_rule_mapping_bindings(
    dir: &Path,
    current_mapping_ids: &[String],
) -> anyhow::Result<usize> {
    let current_mapping_ids = current_mapping_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let missing = {
        let mut statement = transaction.prepare(
            "SELECT DISTINCT mapping_id FROM learn_pairs
             WHERE pair_key LIKE 'word:v2:%'
               AND mapping_id IS NOT NULL
               AND binding_invalidated_at IS NULL",
        )?;
        let referenced = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        referenced
            .into_iter()
            .filter(|mapping_id| !current_mapping_ids.contains(mapping_id.as_str()))
            .collect::<Vec<_>>()
    };
    let mut changed = 0usize;
    for mapping_id in missing {
        changed += invalidate_mapping_bound_word_rules(
            &transaction,
            &mapping_id,
            LearnPairBindingInvalidationReason::MissingBindingAtMigration,
        )?;
    }
    transaction.commit()?;
    Ok(changed)
}

pub fn invalidate_mapping_bound_word_rules(
    transaction: &rusqlite::Transaction<'_>,
    mapping_id: &str,
    reason: LearnPairBindingInvalidationReason,
) -> anyhow::Result<usize> {
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT pair_key, before_surface, promoted, manual_scoped, auto_scoped,
                    dictionary_pending_action
             FROM learn_pairs
             WHERE pair_key LIKE 'word:v2:%'
               AND mapping_id = ?1
               AND binding_invalidated_at IS NULL",
        )?;
        let rows = statement
            .query_map([mapping_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<bool>>(3)?.unwrap_or(false),
                    row.get::<_, Option<bool>>(4)?.unwrap_or(false),
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (pair_key, before, promoted, manual_scoped, auto_scoped, pending_action) in &rows {
        transaction.execute(
            "UPDATE learn_pairs
             SET binding_invalidated_at = datetime('now'), binding_invalidated_reason = ?2
             WHERE pair_key = ?1 AND binding_invalidated_at IS NULL",
            params![pair_key, reason.as_db_str()],
        )?;
        if !before.is_empty()
            && (pending_action == "add" || (!manual_scoped && !auto_scoped && *promoted))
        {
            // Replacing a pending Add with Remove reconciles both cases where
            // Settings never received the source and where its previous save
            // succeeded but acknowledgement did not. Only this Rule owner is
            // removed; Manual and Screen owners remain separate.
            stage_dictionary_effect(transaction, pair_key, DictionaryEffectAction::Remove)?;
        }
    }
    Ok(rows.len())
}

pub fn rebind_invalidated_word_rule(
    dir: &Path,
    pair_key: &str,
    scope: &LearnPairScope,
) -> Result<LearnPairRecord, ManualScopedPairStoreError> {
    if !pair_key.starts_with("word:v2:") || scope.mapping_id.as_deref().is_none_or(str::is_empty) {
        return Err(ManualScopedPairStoreError::InvalidScope);
    }
    let mut connection = open_history(dir).map_err(ManualScopedPairStoreError::Storage)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    let existing = transaction
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [pair_key],
            learn_pair_from_row,
        )
        .optional()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?
        .ok_or_else(|| ManualScopedPairStoreError::ScopeConflict)?;
    if !existing.is_binding_invalidated()
        || !existing.promoted
        || existing.ignored
        || existing.tombstoned_at.is_some()
        || existing.before_surface.is_empty()
    {
        return Err(ManualScopedPairStoreError::ScopeConflict);
    }
    let match_policy = word_rule_match_policy(&existing.before_surface, &existing.after_surface);
    if existing.match_policy != match_policy {
        return Err(ManualScopedPairStoreError::ScopeConflict);
    }
    let rebound_key = scoped_word_rule_key(
        &existing.before_surface,
        &existing.after_surface,
        scope,
        match_policy,
    );
    if rebound_key.as_str() == pair_key {
        transaction
            .execute(
                "UPDATE learn_pairs
                 SET binding_invalidated_at = NULL,
                     binding_invalidated_reason = NULL,
                     legacy_pair_key = COALESCE(legacy_pair_key, ?2),
                     last_at = datetime('now')
                 WHERE pair_key = ?1
                   AND binding_invalidated_at IS NOT NULL
                   AND ignored = 0
                   AND tombstoned_at IS NULL",
                params![
                    pair_key,
                    legacy_pair_key_for_surfaces(
                        pair_key,
                        &existing.before_surface,
                        &existing.after_surface,
                    )
                ],
            )
            .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    } else {
        let new_key = rebound_key.as_str();
        let collision = transaction
            .query_row(
                "SELECT 1 FROM learn_pairs WHERE pair_key = ?1",
                [new_key],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?
            .is_some();
        if collision {
            return Err(ManualScopedPairStoreError::ScopeConflict);
        }
        transaction
            .execute(
                "INSERT INTO learn_pairs (
                    pair_key, before_surface, after_surface, hits, promoted, last_at,
                    family, mapping_id, browser_host, native_bundle, pinned, ignored,
                    manual_scoped, auto_scoped, match_policy, legacy_pair_key
                 ) VALUES (?1, ?2, ?3, 0, 1, datetime('now'), ?4, ?5, ?6, ?7, 0, 0, ?8, ?9, ?10, ?11)",
                params![
                    new_key,
                    existing.before_surface,
                    existing.after_surface,
                    scope.family,
                    scope.mapping_id,
                    scope.browser_host,
                    scope.native_bundle,
                    existing.manual_scoped,
                    existing.auto_scoped,
                    match_policy.as_db_str(),
                    legacy_pair_key_for_surfaces(
                        new_key,
                        &existing.before_surface,
                        &existing.after_surface,
                    ),
                ],
            )
            .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    }
    let rebound = transaction
        .query_row(
            &format!("SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs WHERE pair_key = ?1"),
            [rebound_key.as_str()],
            learn_pair_from_row,
        )
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    transaction
        .commit()
        .map_err(|error| ManualScopedPairStoreError::Storage(error.into()))?;
    Ok(rebound)
}

#[cfg(any(test, feature = "research-harness"))]
pub fn tombstone_research_learn_pair(
    dir: &Path,
    pair_key: &str,
    observation: &LearnPairObservation,
) -> anyhow::Result<(bool, bool)> {
    tombstone_learn_pair_with_observation(dir, pair_key, Some(observation))
}

pub fn ensure_pinned_dictionary_term(
    dir: &Path,
    after_surface: &str,
    pinned: bool,
) -> anyhow::Result<()> {
    let pair_key = crate::dictionary_learn::pair_key("", after_surface);
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, pinned, ignored)
         VALUES (?1, '', ?2, 0, 1, datetime('now'), ?3, 0)
         ON CONFLICT(pair_key) DO UPDATE SET
           pinned = excluded.pinned,
           last_at = datetime('now')",
        params![pair_key, after_surface, i64::from(pinned)],
    )?;
    Ok(())
}

pub fn bump_learn_pairs_used(dir: &Path, pair_keys: &[String]) -> anyhow::Result<()> {
    if pair_keys.is_empty() {
        return Ok(());
    }
    let c = open_history(dir)?;
    let mut statement =
        c.prepare("UPDATE learn_pairs SET last_used_at = datetime('now'), replacement_runs = replacement_runs + 1 WHERE pair_key = ?1 AND promoted = 1 AND ignored = 0 AND tombstoned_at IS NULL AND binding_invalidated_at IS NULL")?;
    for key in pair_keys {
        statement.execute([key])?;
    }
    Ok(())
}

pub fn list_learn_pairs(dir: &Path) -> anyhow::Result<Vec<LearnPairRecord>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs ORDER BY last_at DESC, pair_key ASC"
    ))?;
    let rows = statement.query_map([], learn_pair_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnPairTransferExpectedRow {
    pub(crate) pair_key: String,
    pub(crate) before_surface: String,
    pub(crate) after_surface: String,
    pub(crate) scope: LearnPairScope,
    pub(crate) match_policy: LearnPairMatchPolicy,
    pub(crate) promoted: bool,
    pub(crate) ignored: bool,
    pub(crate) pinned: bool,
    pub(crate) tombstoned: bool,
    pub(crate) binding_invalidated: bool,
    pub(crate) manual_scoped: bool,
    pub(crate) auto_scoped: bool,
}

impl From<&LearnPairRecord> for LearnPairTransferExpectedRow {
    fn from(row: &LearnPairRecord) -> Self {
        Self {
            pair_key: row.pair_key.clone(),
            before_surface: row.before_surface.clone(),
            after_surface: row.after_surface.clone(),
            scope: LearnPairScope {
                family: row.family.clone(),
                mapping_id: row.mapping_id.clone(),
                browser_host: row.browser_host.clone(),
                native_bundle: row.native_bundle.clone(),
            },
            match_policy: row.match_policy,
            promoted: row.promoted,
            ignored: row.ignored,
            pinned: row.pinned,
            tombstoned: row.tombstoned_at.is_some(),
            binding_invalidated: row.binding_invalidated_at.is_some(),
            manual_scoped: row.manual_scoped,
            auto_scoped: row.auto_scoped,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LearnPairTransferIntent {
    /// Destination key recomputed with the destination mapping identity.
    pub(crate) pair_key: String,
    pub(crate) before_surface: String,
    pub(crate) after_surface: String,
    pub(crate) scope: LearnPairScope,
    pub(crate) match_policy: LearnPairMatchPolicy,
    pub(crate) manual_scoped: bool,
    /// Full relevant conflict snapshot collected during preview. Usage
    /// counters and timestamps are intentionally absent.
    pub(crate) expected_rows: Vec<LearnPairTransferExpectedRow>,
    pub(crate) replace_conflicts: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LearnPairTransferOutcome {
    Inserted,
    Replaced { pair_keys: Vec<String> },
    AlreadyPresent,
    Conflict,
    Blocked,
    StalePreview,
}

pub fn transfer_scope_overlaps(left: &LearnPairScope, right: &LearnPairScope) -> bool {
    if (left.browser_host.is_some() && right.native_bundle.is_some())
        || (right.browser_host.is_some() && left.native_bundle.is_some())
    {
        return false;
    }
    [
        (left.family.as_deref(), right.family.as_deref()),
        (left.mapping_id.as_deref(), right.mapping_id.as_deref()),
        (left.browser_host.as_deref(), right.browser_host.as_deref()),
        (
            left.native_bundle.as_deref(),
            right.native_bundle.as_deref(),
        ),
    ]
    .into_iter()
    .all(|(left, right)| match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    })
}

pub fn transfer_alias_domains_intersect(
    before_surface: &str,
    match_policy: LearnPairMatchPolicy,
    row: &LearnPairRecord,
) -> bool {
    match (match_policy, row.match_policy) {
        (LearnPairMatchPolicy::Exact, LearnPairMatchPolicy::Exact) => {
            before_surface == row.before_surface
        }
        _ => before_surface.eq_ignore_ascii_case(&row.before_surface),
    }
}

pub fn transfer_alias_matches(
    before_surface: &str,
    match_policy: LearnPairMatchPolicy,
    row: &LearnPairRecord,
) -> bool {
    match match_policy {
        LearnPairMatchPolicy::Exact => before_surface == row.before_surface,
        LearnPairMatchPolicy::IgnoreCase => {
            before_surface.eq_ignore_ascii_case(&row.before_surface)
        }
    }
}

pub fn transfer_pair_key_is_valid(
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: &LearnPairScope,
    match_policy: LearnPairMatchPolicy,
) -> bool {
    !before_surface.trim().is_empty()
        && !after_surface.trim().is_empty()
        && scoped_word_rule_key(before_surface, after_surface, scope, match_policy).as_str()
            == pair_key
}

pub fn transfer_conflict_snapshot(
    rows: &[LearnPairRecord],
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: &LearnPairScope,
    match_policy: LearnPairMatchPolicy,
) -> Vec<LearnPairTransferExpectedRow> {
    let mut expected = rows
        .iter()
        .filter(|row| {
            let row_scope = LearnPairScope {
                family: row.family.clone(),
                mapping_id: row.mapping_id.clone(),
                browser_host: row.browser_host.clone(),
                native_bundle: row.native_bundle.clone(),
            };
            let exact_marker = transfer_alias_matches(before_surface, match_policy, row)
                && row.after_surface == after_surface
                && row_scope == *scope
                && row.match_policy == match_policy
                && (row.ignored || row.tombstoned_at.is_some() || row.is_binding_invalidated());
            row.pair_key == pair_key
                || exact_marker
                || (is_word_rule_candidate(row)
                    && transfer_alias_domains_intersect(before_surface, match_policy, row)
                    && transfer_scope_overlaps(scope, &row_scope)
                    && (row.is_live_promoted()
                        || (row.after_surface != after_surface
                            && !row.ignored
                            && row.tombstoned_at.is_none()
                            && !row.is_binding_invalidated()))
                    && !row.ignored
                    && row.tombstoned_at.is_none()
                    && !row.is_binding_invalidated())
        })
        .map(LearnPairTransferExpectedRow::from)
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| left.pair_key.cmp(&right.pair_key));
    expected
}

/// Capture only rows that can collide with this imported alias at its target
/// scope. The resulting snapshot is suitable for preview/apply revalidation;
/// unrelated learning counters and use timestamps are never part of it.
pub fn learn_pair_transfer_conflict_snapshot(
    rows: &[LearnPairRecord],
    pair_key: &str,
    before_surface: &str,
    after_surface: &str,
    scope: &LearnPairScope,
    match_policy: LearnPairMatchPolicy,
) -> anyhow::Result<Vec<LearnPairTransferExpectedRow>> {
    if !transfer_pair_key_is_valid(pair_key, before_surface, after_surface, scope, match_policy) {
        anyhow::bail!("invalid transferred word rule identity");
    }
    Ok(transfer_conflict_snapshot(
        rows,
        pair_key,
        before_surface,
        after_surface,
        scope,
        match_policy,
    ))
}

/// Check previewed pair intents against a previously read pair snapshot.
/// This is a pure preflight used before the separate Settings store is written;
/// `apply_learn_pair_transfers` still repeats the comparison in its IMMEDIATE
/// transaction to protect the actual SQLite mutation.
pub fn learn_pair_transfer_intents_match_snapshot(
    rows: &[LearnPairRecord],
    intents: &[LearnPairTransferIntent],
) -> anyhow::Result<bool> {
    let mut seen_keys = std::collections::HashSet::new();
    for intent in intents {
        if !seen_keys.insert(intent.pair_key.as_str()) {
            anyhow::bail!("duplicate transferred word rule identity");
        }
        if !transfer_pair_key_is_valid(
            &intent.pair_key,
            &intent.before_surface,
            &intent.after_surface,
            &intent.scope,
            intent.match_policy,
        ) {
            anyhow::bail!("invalid transferred word rule identity");
        }
        let current = transfer_conflict_snapshot(
            rows,
            &intent.pair_key,
            &intent.before_surface,
            &intent.after_surface,
            &intent.scope,
            intent.match_policy,
        );
        let mut expected = intent.expected_rows.clone();
        expected.sort_by(|left, right| left.pair_key.cmp(&right.pair_key));
        if current != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Apply a set of already previewed word-pair imports in one SQLite
/// `IMMEDIATE` transaction. Each row rechecks its complete conflict set so a
/// concurrent add/forget/confirm cannot be overwritten by a stale preview.
pub fn apply_learn_pair_transfers(
    dir: &Path,
    intents: &[LearnPairTransferIntent],
) -> anyhow::Result<Vec<LearnPairTransferOutcome>> {
    if intents.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen_keys = std::collections::HashSet::new();
    if intents
        .iter()
        .any(|intent| !seen_keys.insert(intent.pair_key.as_str()))
    {
        anyhow::bail!("duplicate transferred word rule identity");
    }
    for intent in intents {
        if !transfer_pair_key_is_valid(
            &intent.pair_key,
            &intent.before_surface,
            &intent.after_surface,
            &intent.scope,
            intent.match_policy,
        ) {
            anyhow::bail!("invalid transferred word rule identity");
        }
    }

    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut statement = transaction.prepare(&format!(
        "SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs ORDER BY pair_key ASC"
    ))?;
    let initial_rows = statement
        .query_map([], learn_pair_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut rows = initial_rows.clone();
    let mut outcomes = Vec::with_capacity(intents.len());
    let mut batch_imported_keys = std::collections::HashSet::<String>::new();

    for intent in intents {
        let initial = transfer_conflict_snapshot(
            &initial_rows,
            &intent.pair_key,
            &intent.before_surface,
            &intent.after_surface,
            &intent.scope,
            intent.match_policy,
        );
        let mut expected = intent.expected_rows.clone();
        expected.sort_by(|left, right| left.pair_key.cmp(&right.pair_key));
        if initial != expected {
            outcomes.push(LearnPairTransferOutcome::StalePreview);
            continue;
        }

        let current = transfer_conflict_snapshot(
            &rows,
            &intent.pair_key,
            &intent.before_surface,
            &intent.after_surface,
            &intent.scope,
            intent.match_policy,
        );
        let matching_rows = current
            .iter()
            .filter_map(|snapshot| {
                rows.iter()
                    .find(|row| row.pair_key == snapshot.pair_key)
                    .cloned()
            })
            .collect::<Vec<_>>();
        if matching_rows
            .iter()
            .any(|row| row.ignored || row.tombstoned_at.is_some() || row.is_binding_invalidated())
        {
            outcomes.push(LearnPairTransferOutcome::Blocked);
            continue;
        }

        let row_scope = |row: &LearnPairRecord| LearnPairScope {
            family: row.family.clone(),
            mapping_id: row.mapping_id.clone(),
            browser_host: row.browser_host.clone(),
            native_bundle: row.native_bundle.clone(),
        };
        let key_collision_mismatch = matching_rows.iter().any(|row| {
            row.pair_key == intent.pair_key
                && (!transfer_alias_matches(&intent.before_surface, intent.match_policy, row)
                    || row.after_surface != intent.after_surface
                    || row.match_policy != intent.match_policy
                    || row_scope(row) != intent.scope)
        });
        if key_collision_mismatch
            || matching_rows
                .iter()
                .any(|row| row.pair_key == intent.pair_key && !row.is_live_promoted())
        {
            outcomes.push(LearnPairTransferOutcome::Blocked);
            continue;
        }

        let candidate_scope = &intent.scope;
        let exact_present = matching_rows.iter().any(|row| {
            row.pair_key == intent.pair_key
                && transfer_alias_matches(&intent.before_surface, intent.match_policy, row)
                && row.after_surface == intent.after_surface
                && row.match_policy == intent.match_policy
                && row_scope(row) == *candidate_scope
                && row.is_live_promoted()
        });
        let equivalent_present = matching_rows.iter().any(|row| {
            row.is_live_promoted()
                && row.after_surface == intent.after_surface
                && row.match_policy == intent.match_policy
                && transfer_alias_domains_intersect(
                    &intent.before_surface,
                    intent.match_policy,
                    row,
                )
                && row_scope(row) == *candidate_scope
        });
        let conflicts = matching_rows
            .iter()
            .filter(|row| {
                let compatible_batch_policy_sibling = batch_imported_keys.contains(&row.pair_key)
                    && row.is_live_promoted()
                    && row.after_surface == intent.after_surface
                    && row.match_policy != intent.match_policy
                    && transfer_alias_domains_intersect(
                        &intent.before_surface,
                        intent.match_policy,
                        row,
                    )
                    && row_scope(row) == intent.scope;
                if compatible_batch_policy_sibling {
                    return false;
                }
                row.after_surface != intent.after_surface
                    || row.match_policy != intent.match_policy
                    || row.pair_key == intent.pair_key
                        && (!transfer_alias_matches(
                            &intent.before_surface,
                            intent.match_policy,
                            row,
                        ) || row.match_policy != intent.match_policy
                            || row_scope(row) != intent.scope)
            })
            .cloned()
            .collect::<Vec<_>>();
        let equivalent_scope_rows = matching_rows
            .iter()
            .filter(|row| {
                row.is_live_promoted()
                    && row.after_surface == intent.after_surface
                    && row.match_policy == intent.match_policy
                    && transfer_alias_domains_intersect(
                        &intent.before_surface,
                        intent.match_policy,
                        row,
                    )
                    && row_scope(row) == *candidate_scope
            })
            .cloned()
            .collect::<Vec<_>>();
        let needs_localization = equivalent_scope_rows
            .iter()
            .any(|row| row.is_provider_egress_eligible_word_pair());

        if (exact_present || equivalent_present) && conflicts.is_empty() && !needs_localization {
            outcomes.push(LearnPairTransferOutcome::AlreadyPresent);
            continue;
        }
        if conflicts
            .iter()
            .any(|row| !row.is_live_promoted() || row_scope(row) != *candidate_scope)
        {
            outcomes.push(LearnPairTransferOutcome::Blocked);
            continue;
        }
        if conflicts.iter().any(|row| row.pair_key == intent.pair_key) {
            // A key/surface mismatch cannot be safely replaced in place:
            // pending dictionary effects store only the key and read the word
            // from the current row, so rewriting it would leave an old owner.
            outcomes.push(LearnPairTransferOutcome::Blocked);
            continue;
        }
        if !conflicts.is_empty() && !intent.replace_conflicts {
            outcomes.push(LearnPairTransferOutcome::Conflict);
            continue;
        }

        let replaceable = conflicts
            .iter()
            .all(|row| row.is_live_promoted() && row_scope(row) == *candidate_scope);
        if !conflicts.is_empty() && !replaceable {
            outcomes.push(LearnPairTransferOutcome::Blocked);
            continue;
        }

        let mut replaced_keys = Vec::new();
        let mut inherited_pin = false;
        for row in conflicts {
            inherited_pin |= row.pinned;
            let changed = transaction.execute(
                "UPDATE learn_pairs
                 SET ignored = 1,
                     tombstoned_at = COALESCE(tombstoned_at, datetime('now')),
                     last_at = datetime('now')
                 WHERE pair_key = ?1
                   AND promoted = 1
                   AND ignored = 0
                   AND tombstoned_at IS NULL
                   AND binding_invalidated_at IS NULL",
                [&row.pair_key],
            )?;
            if changed != 1 {
                anyhow::bail!("learn pair changed during transfer replacement");
            }
            if is_settings_dictionary_word_rule(
                &row.pair_key,
                &row.before_surface,
                &row.after_surface,
                row.manual_scoped,
                row.auto_scoped,
            ) {
                stage_dictionary_effect(
                    &transaction,
                    &row.pair_key,
                    DictionaryEffectAction::Remove,
                )?;
            }
            if let Some(stored) = rows
                .iter_mut()
                .find(|stored| stored.pair_key == row.pair_key)
            {
                stored.ignored = true;
                stored.tombstoned_at = Some("transfer_replaced".into());
            }
            replaced_keys.push(row.pair_key.clone());
        }

        if exact_present || equivalent_present {
            if needs_localization {
                if !intent.replace_conflicts {
                    outcomes.push(LearnPairTransferOutcome::Conflict);
                    continue;
                }
                for row in equivalent_scope_rows
                    .iter()
                    .filter(|row| row.is_provider_egress_eligible_word_pair())
                {
                    let changed = transaction.execute(
                        "UPDATE learn_pairs
                         SET auto_scoped = 1, last_at = datetime('now')
                         WHERE pair_key = ?1
                           AND promoted = 1
                           AND ignored = 0
                           AND tombstoned_at IS NULL
                           AND binding_invalidated_at IS NULL
                           AND COALESCE(manual_scoped, 0) = 0
                           AND COALESCE(auto_scoped, 0) = 0",
                        [&row.pair_key],
                    )?;
                    if changed != 1 {
                        anyhow::bail!("learn pair changed during transfer localization");
                    }
                    stage_dictionary_effect(
                        &transaction,
                        &row.pair_key,
                        DictionaryEffectAction::Remove,
                    )?;
                    if let Some(stored) = rows
                        .iter_mut()
                        .find(|stored| stored.pair_key == row.pair_key)
                    {
                        stored.auto_scoped = true;
                    }
                    replaced_keys.push(row.pair_key.clone());
                }
            }
            if replaced_keys.is_empty() {
                outcomes.push(LearnPairTransferOutcome::AlreadyPresent);
            } else {
                outcomes.push(LearnPairTransferOutcome::Replaced {
                    pair_keys: replaced_keys,
                });
            }
            continue;
        }

        let legacy_pair_key = legacy_pair_key_for_surfaces(
            &intent.pair_key,
            &intent.before_surface,
            &intent.after_surface,
        );
        transaction.execute(
            "INSERT INTO learn_pairs (
                pair_key, before_surface, after_surface, hits, promoted, last_at,
                family, mapping_id, browser_host, native_bundle, pinned, tombstoned_at,
                ignored, manual_scoped, auto_scoped, match_policy, legacy_pair_key
             ) VALUES (
                ?1, ?2, ?3, 0, 1, datetime('now'), ?4, ?5, ?6, ?7, ?8, NULL,
                0, ?9, 1, ?10, ?11
             )",
            params![
                intent.pair_key,
                intent.before_surface,
                intent.after_surface,
                intent.scope.family,
                intent.scope.mapping_id,
                intent.scope.browser_host,
                intent.scope.native_bundle,
                inherited_pin,
                intent.manual_scoped,
                intent.match_policy.as_db_str(),
                legacy_pair_key,
            ],
        )?;
        rows.push(LearnPairRecord {
            pair_key: intent.pair_key.clone(),
            before_surface: intent.before_surface.clone(),
            after_surface: intent.after_surface.clone(),
            hits: 0,
            promoted: true,
            last_at: String::new(),
            family: intent.scope.family.clone(),
            mapping_id: intent.scope.mapping_id.clone(),
            browser_host: intent.scope.browser_host.clone(),
            native_bundle: intent.scope.native_bundle.clone(),
            last_used_at: None,
            pinned: inherited_pin,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: intent.manual_scoped,
            auto_scoped: true,
            match_policy: intent.match_policy,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        });
        batch_imported_keys.insert(intent.pair_key.clone());
        if replaced_keys.is_empty() {
            outcomes.push(LearnPairTransferOutcome::Inserted);
        } else {
            outcomes.push(LearnPairTransferOutcome::Replaced {
                pair_keys: replaced_keys,
            });
        }
    }

    transaction.commit()?;
    Ok(outcomes)
}

/// Application allocation limits for the complete, non-paged Settings audit.
pub const AUDIT_PAIR_MAX_ROWS: usize = 1024;
pub const AUDIT_PAIR_MAX_UTF8_BYTES: usize = 2 * 1024 * 1024;
#[derive(Debug)]
pub enum AuditPairReadError {
    Capacity {
        resource: &'static str,
        limit: usize,
    },
    Store,
}
pub fn list_learn_pairs_for_audit(dir: &Path) -> Result<Vec<LearnPairRecord>, AuditPairReadError> {
    use rusqlite::types::ValueRef;
    let c = open_history(dir).map_err(|_| AuditPairReadError::Store)?;
    let mut query = c
        .prepare(&format!(
            "SELECT {LEARN_PAIR_COLUMNS} FROM learn_pairs ORDER BY last_at DESC, pair_key ASC"
        ))
        .map_err(|_| AuditPairReadError::Store)?;
    let mut cursor = query.query([]).map_err(|_| AuditPairReadError::Store)?;
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = cursor.next().map_err(|_| AuditPairReadError::Store)? {
        if rows.len() == AUDIT_PAIR_MAX_ROWS {
            return Err(AuditPairReadError::Capacity {
                resource: "rows",
                limit: AUDIT_PAIR_MAX_ROWS,
            });
        }
        // SQLite may materialize its current row. No owned String/row copy is made before admission.
        for index in [0_usize, 1, 2, 5, 6, 7, 8, 9, 10, 12, 16, 17, 18] {
            let size = match row.get_ref(index).map_err(|_| AuditPairReadError::Store)? {
                ValueRef::Text(text) => text.len(),
                ValueRef::Null => 0,
                _ => return Err(AuditPairReadError::Store),
            };
            bytes = bytes
                .checked_add(size)
                .filter(|n| *n <= AUDIT_PAIR_MAX_UTF8_BYTES)
                .ok_or(AuditPairReadError::Capacity {
                    resource: "utf8_bytes",
                    limit: AUDIT_PAIR_MAX_UTF8_BYTES,
                })?;
        }
        rows.push(learn_pair_from_row(row).map_err(|_| AuditPairReadError::Store)?);
    }
    Ok(rows)
}

#[derive(Clone, Serialize)]
pub struct LearnedTermUsage {
    pub word: String,
    pub replacement_runs: u64,
    pub last_replaced_at: String,
}

/// One observation per distinct corrected term in one local processing pass.
pub fn record_learned_term_usage(dir: &Path, words: &[String]) -> anyhow::Result<()> {
    if words.is_empty() {
        return Ok(());
    }
    let mut c = open_history(dir)?;
    let transaction = c.transaction()?;
    for word in words.iter().collect::<std::collections::BTreeSet<_>>() {
        transaction.execute("INSERT INTO learned_term_usage (word, replacement_runs, last_replaced_at) VALUES (?1, 1, datetime('now')) ON CONFLICT(word) DO UPDATE SET replacement_runs = replacement_runs + 1, last_replaced_at = datetime('now')", [word])?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn list_learned_term_usage(dir: &Path) -> anyhow::Result<Vec<LearnedTermUsage>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare("SELECT word, replacement_runs, last_replaced_at FROM learned_term_usage WHERE EXISTS (SELECT 1 FROM learn_pairs WHERE after_surface COLLATE NOCASE = word COLLATE NOCASE AND promoted = 1 AND ignored = 0 AND tombstoned_at IS NULL) ORDER BY last_replaced_at DESC, word ASC")?;
    let rows = statement.query_map([], |row| {
        Ok(LearnedTermUsage {
            word: row.get(0)?,
            replacement_runs: row.get(1)?,
            last_replaced_at: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn upsert_style_draft(
    dir: &Path,
    mapping_id: &str,
    style_key: &str,
    excerpt: &str,
    before_excerpt: &str,
    after_excerpt: &str,
) -> anyhow::Result<StyleDraftRecord> {
    let draft_key = format!("{mapping_id}\u{1e}{style_key}");
    let c = open_history(dir)?;
    c.execute(
        "INSERT INTO style_drafts (draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
         ON CONFLICT(draft_key) DO UPDATE SET
           excerpt = excluded.excerpt,
           before_excerpt = excluded.before_excerpt,
           after_excerpt = excluded.after_excerpt,
           created_at = datetime('now')",
        params![draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt],
    )?;
    Ok(StyleDraftRecord {
        draft_key,
        mapping_id: mapping_id.to_owned(),
        style_key: style_key.to_owned(),
        excerpt: excerpt.to_owned(),
        before_excerpt: before_excerpt.to_owned(),
        after_excerpt: after_excerpt.to_owned(),
        created_at: String::new(),
    })
}

pub fn list_style_drafts(dir: &Path) -> anyhow::Result<Vec<StyleDraftRecord>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "SELECT draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at
         FROM style_drafts ORDER BY created_at DESC, draft_key ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(StyleDraftRecord {
            draft_key: row.get(0)?,
            mapping_id: row.get(1)?,
            style_key: row.get(2)?,
            excerpt: row.get(3)?,
            before_excerpt: row.get(4)?,
            after_excerpt: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_style_draft(dir: &Path, draft_key: &str) -> anyhow::Result<Option<StyleDraftRecord>> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT draft_key, mapping_id, style_key, excerpt, before_excerpt, after_excerpt, created_at
         FROM style_drafts WHERE draft_key = ?1",
        [draft_key],
        |row| {
            Ok(StyleDraftRecord {
                draft_key: row.get(0)?,
                mapping_id: row.get(1)?,
                style_key: row.get(2)?,
                excerpt: row.get(3)?,
                before_excerpt: row.get(4)?,
                after_excerpt: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()?)
}

pub fn delete_style_draft(dir: &Path, draft_key: &str) -> anyhow::Result<bool> {
    let c = open_history(dir)?;
    c.execute("DELETE FROM style_drafts WHERE draft_key = ?1", [draft_key])?;
    Ok(c.changes() > 0)
}
