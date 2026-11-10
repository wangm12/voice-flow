//! Normal Settings audit: bounded metadata, never a second learning observer.
use crate::{dictionary_learn, lexicon, lock_recover, store, AppState};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};

const CAPACITY: usize = 128;
const MAX_ENCODED_BYTES: usize = 16 * 1024 * 1024;

/// One native byte representation supplies both random v4 and salted v8 UUIDs.
/// Input is exactly16 bytes; hex encoding is ASCII and lowercase by construction.
fn canonical_uuid(bytes: [u8; 16]) -> String {
    let encoded = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &encoded[..8],
        &encoded[8..12],
        &encoded[12..16],
        &encoded[16..20],
        &encoded[20..]
    )
}
fn random_uuid(random: &mut impl RngCore) -> Option<String> {
    let mut bytes = [0; 16];
    random.try_fill_bytes(&mut bytes).ok()?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Some(canonical_uuid(bytes))
}
struct NativeIdentity {
    runtime_id: String,
    salt: [u8; 16],
}
impl NativeIdentity {
    fn generate(random: &mut impl RngCore) -> Option<Self> {
        let runtime_id = random_uuid(random)?;
        let mut salt = [0; 16];
        // Separate OS entropy for the opaque pair map; no timestamp/path/default salt.
        random.try_fill_bytes(&mut salt).ok()?;
        Some(Self { runtime_id, salt })
    }
}
fn utc_display(now: SystemTime) -> Option<String> {
    // Pinned httpdate1.0.3 HttpDate::from accepts [1970-01-01,10000-01-01).
    // Display only, truthful UTC/GMT whole seconds; sequence owns ordering/identity.
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    (seconds < 253_402_300_800).then(|| httpdate::fmt_http_date(now))
}

#[derive(Clone, Serialize)]
pub(crate) struct ClassificationV1 {
    pub class: dictionary_learn::LearnClass,
    pub reason: Option<&'static str>,
}
#[derive(Clone, Serialize)]
pub(crate) struct PersistenceFactV1 {
    pub status: &'static str,
    pub reason: Option<&'static str>,
}
impl PersistenceFactV1 {
    pub fn untouched() -> Self {
        Self {
            status: "not_attempted",
            reason: None,
        }
    }
    pub fn completed(reason: &'static str) -> Self {
        Self {
            status: "completed",
            reason: Some(reason),
        }
    }
    pub fn failed() -> Self {
        Self {
            status: "failed",
            reason: Some("store_error"),
        }
    }
    fn of_result<T, E>(result: &Result<T, E>, reason: &'static str) -> Self {
        if result.is_ok() {
            Self::completed(reason)
        } else {
            Self::failed()
        }
    }
}
#[derive(Clone, Serialize)]
pub(crate) struct Registration {
    status: &'static str,
    reason: Option<&'static str>,
}
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Fact {
    PostPasteAttempt {
        registration: Registration,
        verified_paste: bool,
        outcome: &'static str,
        classification: Option<ClassificationV1>,
        candidate_decision: &'static str,
        candidate_store: PersistenceFactV1,
        settings_store: PersistenceFactV1,
        attribution: &'static str,
    },
    ReviewAction {
        action: &'static str,
        source: &'static str,
        history_reference: &'static str,
        outcome: &'static str,
        candidate_store: PersistenceFactV1,
        settings_store: PersistenceFactV1,
    },
}
#[derive(Clone, Serialize)]
pub(crate) struct LearningAuditActivityV1 {
    activity_id: String,
    sequence: String,
    observer_epoch: Option<String>,
    created_at: String,
    pair_ref: Option<String>,
    lifecycle: &'static str,
    fact: Fact,
}
#[derive(Clone)]
struct AuditTicket {
    runtime_id: String,
    generation: u64,
    activity_id: String,
}
/// Immutable launch admission comes from lib's native research_mode, not a task local.
pub(crate) struct Owner {
    enabled: bool,
    identity: Option<NativeIdentity>,
    revision: u64,
    generation: u64,
    sequence: u64,
    dropped_entries: u64,
    dropped_updates: u64,
    exhausted: bool,
    entries: VecDeque<LearningAuditActivityV1>,
}
impl Owner {
    pub fn new(research_mode: bool) -> Self {
        Self {
            enabled: !research_mode,
            identity: if research_mode {
                None
            } else {
                NativeIdentity::generate(&mut OsRng)
            },
            revision: 0,
            generation: 0,
            sequence: 0,
            dropped_entries: 0,
            dropped_updates: 0,
            exhausted: false,
            entries: VecDeque::new(),
        }
    }
    fn advance(&mut self) {
        match self.revision.checked_add(1) {
            Some(n) => self.revision = n,
            None => self.exhausted = true,
        }
    }
    fn available(&self) -> Result<(), LearningAuditReadErrorV1> {
        if !self.enabled {
            Err(LearningAuditReadErrorV1::Code {
                code: "audit_not_available_in_research_mode",
            })
        } else if self.exhausted || self.identity.is_none() {
            Err(LearningAuditReadErrorV1::Code {
                code: "audit_identity_unavailable",
            })
        } else {
            Ok(())
        }
    }
    fn insert(&mut self, fact: Fact, epoch: Option<u64>) -> Option<AuditTicket> {
        self.available().ok()?;
        let runtime_id = self.identity.as_ref()?.runtime_id.clone();
        // Required metadata that cannot be produced truthfully disables only this audit.
        let Some(id) = random_uuid(&mut OsRng) else {
            self.exhausted = true;
            self.advance();
            return None;
        };
        let Some(created_at) = utc_display(SystemTime::now()) else {
            self.exhausted = true;
            self.advance();
            return None;
        };
        self.sequence = match self.sequence.checked_add(1) {
            Some(n) => n,
            None => {
                self.exhausted = true;
                return None;
            }
        };
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
            self.dropped_entries = match self.dropped_entries.checked_add(1) {
                Some(n) => n,
                None => {
                    self.exhausted = true;
                    return None;
                }
            };
        }
        self.entries.push_back(LearningAuditActivityV1 {
            activity_id: id.clone(),
            sequence: self.sequence.to_string(),
            observer_epoch: epoch.map(|n| n.to_string()),
            created_at,
            pair_ref: None,
            lifecycle: "in_progress",
            fact,
        });
        self.advance();
        Some(AuditTicket {
            runtime_id,
            generation: self.generation,
            activity_id: id,
        })
    }
    fn update(
        &mut self,
        ticket: &AuditTicket,
        f: impl FnOnce(&mut LearningAuditActivityV1),
    ) -> bool {
        if !self.enabled
            || self.exhausted
            || self
                .identity
                .as_ref()
                .is_none_or(|identity| ticket.runtime_id != identity.runtime_id)
            || ticket.generation != self.generation
        {
            return false;
        }
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.activity_id == ticket.activity_id)
        {
            f(entry);
        } else {
            self.dropped_updates = match self.dropped_updates.checked_add(1) {
                Some(n) => n,
                None => {
                    self.exhausted = true;
                    return false;
                }
            };
        }
        self.advance();
        true
    }
    fn pair_ref(&self, key: &str) -> Option<String> {
        let identity = self.identity.as_ref()?;
        let mut hash = Sha256::new();
        hash.update(b"voiceflow-normal-learning-pair-v1\0");
        hash.update(identity.salt);
        hash.update(key.as_bytes());
        let hash = hash.finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&hash[..16]);
        bytes[6] = (bytes[6] & 15) | 0x80;
        bytes[8] = (bytes[8] & 63) | 0x80;
        Some(canonical_uuid(bytes))
    }
    fn clear(&mut self) {
        self.generation = match self.generation.checked_add(1) {
            Some(n) => n,
            None => {
                self.exhausted = true;
                return;
            }
        };
        self.entries.clear();
        self.dropped_entries = 0;
        self.dropped_updates = 0;
        self.advance();
    }
}
#[derive(Clone, Serialize)]
struct Changed {
    runtime_id: String,
    audit_revision: String,
}
fn notify(app: &tauri::AppHandle, owner: &Owner) {
    if !owner.enabled {
        return;
    }
    let Some(identity) = &owner.identity else {
        return;
    };
    let _ = app.emit(
        "dictionary_learning_audit://changed",
        Changed {
            runtime_id: identity.runtime_id.clone(),
            audit_revision: owner.revision.to_string(),
        },
    );
}

/// Clear the bounded in-memory audit ledger without emitting a UI event.
/// Retained store owners call this only after the data-root clear succeeds;
/// the command side emits the change notification after receiving its real
/// completion acknowledgement.
pub(crate) fn clear_state(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let mut owner = lock_recover(&state.dictionary_learning_audit);
    owner.clear();
}

pub(crate) fn notify_current(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let owner = lock_recover(&state.dictionary_learning_audit);
    notify(app, &owner);
}
pub(crate) fn invalidate(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let mut owner = lock_recover(&state.dictionary_learning_audit);
    if owner.enabled {
        owner.advance();
        notify(app, &owner);
    }
}
/// One move-only metadata token follows the existing observer's selected descendant.
/// A missing explicit completion is unknown, never inferred success.
pub(crate) struct AuditAttempt {
    app: tauri::AppHandle,
    ticket: Option<AuditTicket>,
    finished: bool,
}
impl AuditAttempt {
    fn new(app: &tauri::AppHandle, fact: Fact, epoch: Option<u64>) -> Self {
        // Task provenance may exclude an operation; it never establishes normal launch authority.
        if crate::research_trace::current().is_some() {
            return Self {
                app: app.clone(),
                ticket: None,
                finished: false,
            };
        }
        let ticket = {
            let state = app.state::<AppState>();
            let mut owner = lock_recover(&state.dictionary_learning_audit);
            let ticket = owner.insert(fact, epoch);
            if ticket.is_some() || owner.exhausted {
                notify(app, &owner);
            }
            ticket
        };
        Self {
            app: app.clone(),
            ticket,
            finished: false,
        }
    }
    pub fn observation(app: &tauri::AppHandle, verified: bool, epoch: u64) -> Self {
        Self::new(
            app,
            Fact::PostPasteAttempt {
                registration: Registration {
                    status: "checking",
                    reason: None,
                },
                verified_paste: verified,
                outcome: "not_observed",
                classification: None,
                candidate_decision: "not_attempted",
                candidate_store: PersistenceFactV1::untouched(),
                settings_store: PersistenceFactV1::untouched(),
                attribution: "not_observed",
            },
            Some(epoch),
        )
    }
    pub fn review(app: &tauri::AppHandle, action: &'static str) -> Self {
        Self::new(
            app,
            Fact::ReviewAction {
                action,
                source: "dictionary_command",
                history_reference: "not_provided",
                outcome: "requested",
                candidate_store: PersistenceFactV1::untouched(),
                settings_store: PersistenceFactV1::untouched(),
            },
            None,
        )
    }
    fn update(&mut self, f: impl FnOnce(&mut LearningAuditActivityV1)) {
        if let Some(ticket) = &self.ticket {
            let state = self.app.state::<AppState>();
            let mut owner = lock_recover(&state.dictionary_learning_audit);
            if owner.update(ticket, f) {
                notify(&self.app, &owner);
            }
        }
    }
    pub fn candidate_result<T, E>(
        &mut self,
        result: Result<T, E>,
        reason: &'static str,
    ) -> Result<T, E> {
        self.candidate_store(PersistenceFactV1::of_result(&result, reason));
        result
    }
    pub fn settings_result<T, E>(&mut self, result: Result<T, E>) -> Result<T, E> {
        self.settings_store(PersistenceFactV1::of_result(
            &result,
            "settings_save_completed",
        ));
        result
    }
    pub fn result<T, E>(&mut self, result: Result<T, E>) -> Result<T, E> {
        if result.is_ok() {
            self.finish();
        } else {
            self.fail();
        }
        result
    }
    pub fn registered(&mut self) {
        self.update(|a| {
            if let Fact::PostPasteAttempt {
                registration,
                outcome,
                ..
            } = &mut a.fact
            {
                *registration = Registration {
                    status: "started",
                    reason: Some("registered"),
                };
                *outcome = "observing";
            }
        });
    }
    /// Also emits the fixture-learning registration rejection for `reason`.
    pub fn skip(&mut self, reason: &'static str) {
        self.update(|a| {
            if let Fact::PostPasteAttempt {
                registration,
                outcome,
                ..
            } = &mut a.fact
            {
                *registration = Registration {
                    status: "skipped",
                    reason: Some(reason),
                };
                *outcome = if reason == "stale_or_cancelled" {
                    "stale_or_cancelled"
                } else {
                    "not_observed"
                };
            }
            a.lifecycle = if reason == "stale_or_cancelled" {
                "superseded"
            } else {
                "closed"
            };
        });
        self.finished = true;
        crate::learning_research::registration(false, reason);
    }
    pub fn observed(&mut self, result: &'static str) {
        self.update(|a| {
            if let Fact::PostPasteAttempt {
                outcome,
                attribution,
                ..
            } = &mut a.fact
            {
                *outcome = result;
                if matches!(
                    result,
                    "single_token" | "short_style_rewrite" | "style_signal" | "ambiguous"
                ) {
                    *attribution = "whole_field_difference_only";
                }
            }
        });
    }
    pub fn candidate(
        &mut self,
        decision: &'static str,
        class: Option<ClassificationV1>,
        persistence: PersistenceFactV1,
    ) {
        self.update(|a| {
            if let Fact::PostPasteAttempt {
                classification,
                candidate_decision,
                candidate_store,
                ..
            } = &mut a.fact
            {
                if class.is_some() {
                    *classification = class;
                }
                *candidate_decision = decision;
                if persistence.status != "not_attempted"
                    || candidate_store.status == "not_attempted"
                {
                    *candidate_store = persistence;
                }
            }
        });
    }
    pub fn candidate_store(&mut self, p: PersistenceFactV1) {
        self.update(|a| match &mut a.fact {
            Fact::PostPasteAttempt {
                candidate_store, ..
            }
            | Fact::ReviewAction {
                candidate_store, ..
            } => *candidate_store = p,
        });
    }
    pub fn settings_store(&mut self, p: PersistenceFactV1) {
        self.update(|a| match &mut a.fact {
            Fact::PostPasteAttempt { settings_store, .. }
            | Fact::ReviewAction { settings_store, .. } => *settings_store = p,
        });
    }
    pub fn history_reference(&mut self, looked_up: bool) {
        self.update(|a| {
            if let Fact::ReviewAction {
                source,
                history_reference,
                ..
            } = &mut a.fact
            {
                *source = "manual_confirmation_with_history_reference";
                *history_reference = if looked_up {
                    "lookup_succeeded"
                } else {
                    "lookup_unknown"
                };
            }
        });
    }
    /// Only an actual existing store row may supply this link. No rejected text is retained.
    pub fn pair(&mut self, row: &store::LearnPairRecord) {
        self.existing_pair_key(&row.pair_key);
    }
    /// Caller must hold a row read/returned by store, or an actual successful INSERT/matching UPDATE.
    /// Does not look up a pair or retain the key; a historic/UI key alone is insufficient.
    pub fn existing_pair_key(&mut self, key: &str) {
        let pair_ref = {
            let state = self.app.state::<AppState>();
            let value = lock_recover(&state.dictionary_learning_audit).pair_ref(key);
            value
        };
        if let Some(pair_ref) = pair_ref {
            self.update(|a| a.pair_ref = Some(pair_ref));
        }
    }
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.update(|a| {
            a.lifecycle = "closed";
            if let Fact::ReviewAction { outcome, .. } = &mut a.fact {
                *outcome = "completed";
            }
        });
        self.finished = true;
    }
    pub fn stale(&mut self) {
        if self.finished {
            return;
        }
        self.update(|a| {
            a.lifecycle = "superseded";
            match &mut a.fact {
                Fact::PostPasteAttempt { outcome, .. } => *outcome = "stale_or_cancelled",
                Fact::ReviewAction { outcome, .. } => *outcome = "stale",
            }
        });
        self.finished = true;
    }
    pub fn fail(&mut self) {
        self.update(|a| {
            a.lifecycle = "closed";
            if let Fact::ReviewAction { outcome, .. } = &mut a.fact {
                *outcome = "failed";
            }
        });
        self.finished = true;
    }
}
impl Drop for AuditAttempt {
    fn drop(&mut self) {
        if !self.finished {
            self.update(|a| a.lifecycle = "unknown");
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(untagged)]
pub(crate) enum LearningAuditReadErrorV1 {
    Code {
        code: &'static str,
    },
    Capacity {
        code: &'static str,
        resource: &'static str,
        limit: String,
    },
}
#[derive(Serialize)]
pub(crate) struct LearningAuditPairV1 {
    pair_ref: String,
    pair_key: String,
    before_surface: String,
    after_surface: String,
    rule_kind: &'static str,
    record_state: &'static str,
    candidate_hits: u32,
    current_classification: Option<ClassificationV1>,
    current_promote_threshold: Option<u32>,
    classification_authority: &'static str,
    recorded_scope: Scope,
    origin: &'static str,
    tombstone_reason: Option<&'static str>,
    latest_record_change_at: String,
    last_output_surface_match_at: Option<String>,
    current_dictionary_contains_after: bool,
    replacement_policy: &'static str,
    replacement_eligibility: &'static str,
    eligibility_authority: &'static str,
    current_review: Review,
}
#[derive(Serialize)]
struct Scope {
    family: Option<String>,
    mapping_id: Option<String>,
    browser_host: Option<String>,
    native_bundle: Option<String>,
}
#[derive(Serialize)]
struct Review {
    confirm: bool,
    ignore: bool,
    undo: bool,
}
#[derive(Serialize)]
struct PairCoverage {
    kind: &'static str,
    max_rows: usize,
    max_utf8_bytes: usize,
    max_snapshot_encoded_bytes: usize,
}
#[derive(Serialize)]
struct Coverage {
    kind: &'static str,
    capacity: usize,
    dropped_entries: String,
    dropped_updates: String,
    oldest_sequence: Option<String>,
    latest_sequence: Option<String>,
    projection_consistency: &'static str,
    atomicity: &'static str,
    consistency_scope: &'static str,
    projection_mutation_interval: &'static str,
    projection_inflight_writers: Option<u64>,
    pair_projection: PairCoverage,
}
#[derive(Serialize)]
pub(crate) struct LearningAuditSnapshotV1 {
    schema_version: u8,
    runtime_id: String,
    audit_revision: String,
    clear_generation: String,
    observer_epoch: String,
    coverage: Coverage,
    pairs: Vec<LearningAuditPairV1>,
    activity: Vec<LearningAuditActivityV1>,
}
fn project(
    row: store::LearnPairRecord,
    dictionary: &[String],
    owner: &Owner,
) -> Result<LearningAuditPairV1, LearningAuditReadErrorV1> {
    let rule_kind = dictionary_learn::learned_rule_kind(&row.pair_key);
    let word = rule_kind == dictionary_learn::LearnedRuleKind::Word;
    let kind = match rule_kind {
        dictionary_learn::LearnedRuleKind::Word => "word_pair",
        dictionary_learn::LearnedRuleKind::Style => "style_preference",
        dictionary_learn::LearnedRuleKind::Intensity => "cleanup_intensity",
    };
    let classification = word.then(|| {
        dictionary_learn::classify_learn_pair_decision(&row.before_surface, &row.after_surface)
    });
    let threshold = classification
        .as_ref()
        .filter(|c| c.class != dictionary_learn::LearnClass::Rejected)
        .map(|c| dictionary_learn::promote_hits_for(c.class));
    let contains = store::dictionary_contains_word(dictionary, &row.after_surface);
    let eligible = lexicon::pair_is_replaceable(&row, contains);
    let tombstone = row.ignored || row.tombstoned_at.is_some();
    Ok(LearningAuditPairV1 {
        pair_ref: owner
            .pair_ref(&row.pair_key)
            .ok_or(LearningAuditReadErrorV1::Code {
                code: "audit_identity_unavailable",
            })?,
        record_state: if tombstone {
            "tombstoned"
        } else if row.promoted {
            "db_promoted"
        } else {
            "pending"
        },
        rule_kind: kind,
        candidate_hits: row.hits,
        current_classification: classification,
        current_promote_threshold: threshold,
        classification_authority: if word {
            "current_owner_classification"
        } else {
            "not_word_pair"
        },
        origin: "unknown_not_persisted",
        tombstone_reason: tombstone.then_some("unknown_not_persisted"),
        current_review: Review {
            confirm: word && row.is_pending(),
            ignore: row.is_pending(),
            undo: word && row.is_live_promoted() && !row.before_surface.is_empty(),
        },
        current_dictionary_contains_after: contains,
        replacement_policy: "context_scoped_promoted_rules",
        replacement_eligibility: if eligible {
            "eligible_now"
        } else {
            "not_eligible_now"
        },
        eligibility_authority: "current_lexicon_owner_filter",
        recorded_scope: Scope {
            family: row.family,
            mapping_id: row.mapping_id,
            browser_host: row.browser_host,
            native_bundle: row.native_bundle,
        },
        pair_key: row.pair_key,
        before_surface: row.before_surface,
        after_surface: row.after_surface,
        latest_record_change_at: row.last_at,
        last_output_surface_match_at: row.last_used_at,
    })
}
struct CountWriter {
    bytes: usize,
    limit: usize,
}
impl Write for CountWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| io::Error::other("audit_encoding_capacity"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[tauri::command]
pub(crate) async fn get_dictionary_learning_audit(
    app: tauri::AppHandle,
) -> Result<LearningAuditSnapshotV1, LearningAuditReadErrorV1> {
    let state = app.state::<AppState>();
    let (revision, generation) = {
        let owner = lock_recover(&state.dictionary_learning_audit);
        owner.available()?;
        (owner.revision, owner.generation)
    };
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|_| LearningAuditReadErrorV1::Code {
            code: "app_data_unavailable",
        })?;
    let rows =
        tauri::async_runtime::spawn_blocking(move || store::list_learn_pairs_for_audit(&dir))
            .await
            .unwrap_or_else(|error| {
                log::warn!("learning audit pair read task failed: {error}");
                Err(store::AuditPairReadError::Store)
            })
            .map_err(|e| match e {
                store::AuditPairReadError::Capacity { resource, limit } => {
                    LearningAuditReadErrorV1::Capacity {
                        code: "pair_projection_capacity",
                        resource,
                        limit: limit.to_string(),
                    }
                }
                store::AuditPairReadError::Store => LearningAuditReadErrorV1::Code {
                    code: "pair_store_unavailable",
                },
            })?;
    let pairs = {
        let settings = lock_recover(&state.settings);
        let owner = lock_recover(&state.dictionary_learning_audit);
        owner.available()?;
        rows.into_iter()
            .map(|row| project(row, &settings.dictionary, &owner))
            .collect::<Result<Vec<_>, _>>()?
    };
    let owner = lock_recover(&state.dictionary_learning_audit);
    owner.available()?;
    let snapshot = LearningAuditSnapshotV1 {
        schema_version: 1,
        runtime_id: owner
            .identity
            .as_ref()
            .ok_or(LearningAuditReadErrorV1::Code {
                code: "audit_identity_unavailable",
            })?
            .runtime_id
            .clone(),
        audit_revision: owner.revision.to_string(),
        clear_generation: owner.generation.to_string(),
        observer_epoch: state
            .learning_observer_epoch
            .load(std::sync::atomic::Ordering::Acquire)
            .to_string(),
        coverage: Coverage {
            kind: "current_process",
            capacity: CAPACITY,
            dropped_entries: owner.dropped_entries.to_string(),
            dropped_updates: owner.dropped_updates.to_string(),
            oldest_sequence: owner.entries.front().map(|a| a.sequence.clone()),
            latest_sequence: owner.entries.back().map(|a| a.sequence.clone()),
            projection_consistency: if revision == owner.revision && generation == owner.generation
            {
                "stable_revision"
            } else {
                "changed_during_read"
            },
            atomicity: "not_cross_store_transaction",
            consistency_scope: "audit_metadata_revision_only",
            projection_mutation_interval: "not_observed",
            projection_inflight_writers: None,
            pair_projection: PairCoverage {
                kind: "complete_within_budget",
                max_rows: store::AUDIT_PAIR_MAX_ROWS,
                max_utf8_bytes: store::AUDIT_PAIR_MAX_UTF8_BYTES,
                max_snapshot_encoded_bytes: MAX_ENCODED_BYTES,
            },
        },
        pairs,
        activity: owner.entries.iter().cloned().collect(),
    };
    drop(owner);
    serde_json::to_writer(
        CountWriter {
            bytes: 0,
            limit: MAX_ENCODED_BYTES,
        },
        &snapshot,
    )
    .map_err(|_| LearningAuditReadErrorV1::Capacity {
        code: "pair_projection_capacity",
        resource: "encoded_bytes",
        limit: MAX_ENCODED_BYTES.to_string(),
    })?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn activity(
        sequence: u64,
        epoch: Option<u64>,
        lifecycle: &'static str,
        pair_ref: Option<&str>,
        fact: Fact,
    ) -> LearningAuditActivityV1 {
        LearningAuditActivityV1 {
            activity_id: format!("33333333-3333-4333-8333-{sequence:012}"),
            sequence: sequence.to_string(),
            observer_epoch: epoch.map(|value| value.to_string()),
            created_at: format!("Thu, 22 Oct 2026 00:00:{:02} GMT", sequence - 1),
            pair_ref: pair_ref.map(str::to_owned),
            lifecycle,
            fact,
        }
    }
    fn review(outcome: &'static str, candidate_store: PersistenceFactV1) -> Fact {
        Fact::ReviewAction {
            action: "add_manual_scoped_pair",
            source: "dictionary_command",
            history_reference: "not_provided",
            outcome,
            candidate_store,
            settings_store: PersistenceFactV1::untouched(),
        }
    }
    fn observation(
        registration: Registration,
        outcome: &'static str,
        decision: &'static str,
        store: PersistenceFactV1,
    ) -> Fact {
        Fact::PostPasteAttempt {
            registration,
            verified_paste: true,
            outcome,
            classification: (outcome == "single_token").then_some(ClassificationV1 {
                class: dictionary_learn::LearnClass::Term,
                reason: None,
            }),
            candidate_decision: decision,
            candidate_store: store,
            settings_store: PersistenceFactV1::untouched(),
            attribution: if outcome == "single_token" {
                "whole_field_difference_only"
            } else {
                "not_observed"
            },
        }
    }
    #[test]
    fn native_wire_fixture_uses_the_production_audit_serializer() {
        let saved: Result<(), ()> = Ok(());
        let failed: Result<(), ()> = Err(());
        let pair = LearningAuditPairV1 {
            pair_ref: "22222222-2222-4222-8222-222222222222".into(),
            pair_key: "fixture-manual-app-scoped-pair".into(),
            before_surface: "知呼".into(),
            after_surface: "知乎".into(),
            rule_kind: "word_pair",
            record_state: "db_promoted",
            candidate_hits: 3,
            current_classification: Some(ClassificationV1 {
                class: dictionary_learn::LearnClass::Term,
                reason: None,
            }),
            current_promote_threshold: Some(3),
            classification_authority: "current_owner_classification",
            recorded_scope: Scope {
                family: Some("document".into()),
                mapping_id: Some("fixture.document".into()),
                browser_host: None,
                native_bundle: Some("test.fixture.document".into()),
            },
            origin: "unknown_not_persisted",
            tombstone_reason: None,
            latest_record_change_at: "2026-10-22 00:00:00 GMT".into(),
            last_output_surface_match_at: None,
            current_dictionary_contains_after: false,
            replacement_policy: "context_scoped_promoted_rules",
            replacement_eligibility: "eligible_now",
            eligibility_authority: "current_lexicon_owner_filter",
            current_review: Review {
                confirm: false,
                ignore: false,
                undo: true,
            },
        };
        let no_registration = |reason| Registration {
            status: "skipped",
            reason: Some(reason),
        };
        let started = Registration {
            status: "started",
            reason: Some("registered"),
        };
        let snapshot = LearningAuditSnapshotV1 {
            schema_version: 1,
            runtime_id: "11111111-1111-4111-8111-111111111111".into(),
            audit_revision: "8".into(),
            clear_generation: "0".into(),
            observer_epoch: "7".into(),
            coverage: Coverage {
                kind: "current_process",
                capacity: CAPACITY,
                dropped_entries: "0".into(),
                dropped_updates: "0".into(),
                oldest_sequence: Some("1".into()),
                latest_sequence: Some("8".into()),
                projection_consistency: "stable_revision",
                atomicity: "not_cross_store_transaction",
                consistency_scope: "audit_metadata_revision_only",
                projection_mutation_interval: "not_observed",
                projection_inflight_writers: None,
                pair_projection: PairCoverage {
                    kind: "complete_within_budget",
                    max_rows: store::AUDIT_PAIR_MAX_ROWS,
                    max_utf8_bytes: store::AUDIT_PAIR_MAX_UTF8_BYTES,
                    max_snapshot_encoded_bytes: MAX_ENCODED_BYTES,
                },
            },
            pairs: vec![pair],
            activity: vec![
                activity(
                    1,
                    None,
                    "in_progress",
                    None,
                    review("requested", PersistenceFactV1::untouched()),
                ),
                activity(
                    2,
                    None,
                    "closed",
                    Some("22222222-2222-4222-8222-222222222222"),
                    review(
                        "completed",
                        PersistenceFactV1::of_result(&saved, "manual_scoped_pair_saved"),
                    ),
                ),
                activity(
                    3,
                    None,
                    "closed",
                    None,
                    review(
                        "failed",
                        PersistenceFactV1::of_result(&failed, "manual_scoped_pair_saved"),
                    ),
                ),
                activity(
                    4,
                    Some(7),
                    "closed",
                    None,
                    observation(
                        no_registration("fixture_baseline_mismatch"),
                        "not_observed",
                        "not_attempted",
                        PersistenceFactV1::untouched(),
                    ),
                ),
                activity(
                    5,
                    Some(7),
                    "closed",
                    None,
                    observation(
                        no_registration("ambiguous_inserted_range"),
                        "not_observed",
                        "not_attempted",
                        PersistenceFactV1::untouched(),
                    ),
                ),
                activity(
                    6,
                    Some(7),
                    "closed",
                    Some("22222222-2222-4222-8222-222222222222"),
                    observation(
                        started.clone(),
                        "single_token",
                        "scope_conflict",
                        PersistenceFactV1::untouched(),
                    ),
                ),
                activity(
                    7,
                    Some(7),
                    "closed",
                    Some("22222222-2222-4222-8222-222222222222"),
                    observation(
                        started.clone(),
                        "single_token",
                        "duplicate_observation",
                        PersistenceFactV1::untouched(),
                    ),
                ),
                activity(
                    8,
                    Some(7),
                    "closed",
                    Some("22222222-2222-4222-8222-222222222222"),
                    observation(
                        started,
                        "single_token",
                        "db_promotion_failed",
                        PersistenceFactV1::failed(),
                    ),
                ),
            ],
        };
        let serialized = serde_json::to_value(snapshot).unwrap();
        let shared: serde_json::Value = serde_json::from_str(include_str!(
            "../../src/test/fixtures/dictionary-learning-audit-native.json"
        ))
        .unwrap();
        assert_eq!(serialized, shared);
    }
    struct OwnedStore(std::path::PathBuf);
    impl OwnedStore {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "voiceflow-audit-{}",
                random_uuid(&mut OsRng).expect("owned fixture OS entropy")
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for OwnedStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fact() -> Fact {
        Fact::ReviewAction {
            action: "undo_pair",
            source: "dictionary_command",
            history_reference: "not_provided",
            outcome: "requested",
            candidate_store: PersistenceFactV1::untouched(),
            settings_store: PersistenceFactV1::untouched(),
        }
    }
    struct FailedEntropy;
    impl RngCore for FailedEntropy {
        fn next_u32(&mut self) -> u32 {
            panic!("audit must use fallible byte entropy")
        }
        fn next_u64(&mut self) -> u64 {
            panic!("audit must use fallible byte entropy")
        }
        fn fill_bytes(&mut self, _: &mut [u8]) {
            panic!("audit must use fallible byte entropy")
        }
        fn try_fill_bytes(
            &mut self,
            _: &mut [u8],
        ) -> Result<(), chacha20poly1305::aead::rand_core::Error> {
            let code =
                std::num::NonZeroU32::new(chacha20poly1305::aead::rand_core::Error::CUSTOM_START)
                    .unwrap();
            Err(code.into())
        }
    }
    #[test]
    fn actual_entropy_factory_and_unavailable_owner_never_mint_a_fallback() {
        let identity = NativeIdentity::generate(&mut FailedEntropy);
        assert!(identity.is_none());
        let mut owner = Owner {
            enabled: true,
            identity,
            ..Owner::new(true)
        };
        assert!(matches!(
            owner.available(),
            Err(LearningAuditReadErrorV1::Code {
                code: "audit_identity_unavailable"
            })
        ));
        assert!(owner.insert(fact(), None).is_none());
        assert!(owner.entries.is_empty());
        assert_eq!(owner.sequence, 0);
        assert!(owner.pair_ref("selected controlled row").is_none());
    }
    #[test]
    fn native_uuid_versions_and_display_clock_are_independent() {
        let identity = NativeIdentity::generate(&mut OsRng).expect("fixture entropy");
        crate::onboarding::validate_id(&identity.runtime_id).unwrap();
        assert_eq!(&identity.runtime_id[14..15], "4");
        let owner = Owner {
            identity: Some(identity),
            ..Owner::new(true)
        };
        let pair = owner.pair_ref("controlled fixture row").unwrap();
        crate::onboarding::validate_id(&pair).unwrap();
        assert_eq!(&pair[14..15], "8");
        assert_eq!(
            owner.pair_ref("controlled fixture row").as_deref(),
            Some(pair.as_str())
        );
        assert_eq!(
            utc_display(UNIX_EPOCH).as_deref(),
            Some("Thu, 01 Jan 1970 00:00:00 GMT")
        );
        assert!(utc_display(UNIX_EPOCH - std::time::Duration::from_secs(1)).is_none());
        let unsupported = UNIX_EPOCH + std::time::Duration::from_secs(253_402_300_800);
        assert!(utc_display(unsupported).is_none());
        assert!(utc_display(unsupported - std::time::Duration::from_secs(1))
            .unwrap()
            .contains("9999"));
    }
    #[test]
    fn captured_ticket_clear_eviction_runtime_and_research_admission() {
        let mut owner = Owner::new(false);
        let a = owner.insert(fact(), Some(4)).unwrap();
        owner.clear();
        let b = owner.insert(fact(), Some(5)).unwrap();
        let revision = owner.revision;
        assert!(!owner.update(&a, |_| panic!("old clear ticket reached new activity")));
        assert_eq!(owner.revision, revision);
        assert_eq!(owner.dropped_updates, 0);
        assert!(owner.update(&b, |entry| entry.lifecycle = "closed"));
        let c = owner.insert(fact(), None).unwrap();
        for _ in 0..CAPACITY {
            owner.insert(fact(), None).unwrap();
        }
        let count = owner.entries.len();
        let drops = owner.dropped_updates;
        owner.update(&c, |_| panic!("evicted attempt was reinserted"));
        assert_eq!(owner.entries.len(), count);
        assert_eq!(count, CAPACITY);
        assert_eq!(owner.dropped_updates, drops + 1);
        let mut other_runtime = Owner::new(false);
        assert!(!other_runtime.update(&b, |_| panic!("foreign runtime")));
        let mut research = Owner::new(true);
        assert!(research.insert(fact(), None).is_none());
        assert!(research.entries.is_empty());
        assert!(matches!(
            research.available(),
            Err(LearningAuditReadErrorV1::Code {
                code: "audit_not_available_in_research_mode"
            })
        ));
    }
    #[test]
    fn actual_pair_store_projection_is_not_durable_settings_or_legacy_origin() {
        let dir = OwnedStore::new();
        store::ensure_learn_pair_promoted(
            &dir.0,
            "controlled-pair",
            "知呼",
            "知乎",
            Some(&store::LearnPairScope::default()),
        )
        .unwrap();
        let rows = store::list_learn_pairs_for_audit(&dir.0).unwrap();
        let owner = Owner::new(false);
        let pair = project(rows[0].clone(), &[], &owner).unwrap();
        assert_eq!(pair.record_state, "db_promoted");
        assert_eq!(pair.replacement_policy, "context_scoped_promoted_rules");
        assert_eq!(pair.replacement_eligibility, "not_eligible_now");
        assert_eq!(pair.origin, "unknown_not_persisted");
        let eligible = project(rows[0].clone(), &["知乎".into()], &owner).unwrap();
        assert_eq!(eligible.replacement_eligibility, "eligible_now");
        assert_eq!(eligible.pair_ref, pair.pair_ref);
        store::tombstone_learn_pair(&dir.0, "controlled-pair").unwrap();
        let row = store::list_learn_pairs_for_audit(&dir.0).unwrap().remove(0);
        let tombstone = project(row, &["知乎".into()], &owner).unwrap();
        assert_eq!(tombstone.record_state, "tombstoned");
        assert_eq!(tombstone.tombstone_reason, Some("unknown_not_persisted"));
        assert!(!tombstone.current_review.undo);
        assert_eq!(tombstone.replacement_eligibility, "not_eligible_now");
        let db_leg = store::mark_learn_pair_promoted(&dir.0, "controlled-pair");
        // The actual settings-save leg fails; the earlier SQLite return is not undone.
        std::fs::create_dir(dir.0.join("settings.json")).unwrap();
        let file_leg = store::save_settings(&dir.0, &store::Settings::default());
        assert_eq!(
            PersistenceFactV1::of_result(&db_leg, "db_promotion_recorded").status,
            "completed"
        );
        assert_eq!(
            PersistenceFactV1::of_result(&file_leg, "settings_save_completed").status,
            "failed"
        );
        assert!(
            store::get_learn_pair(&dir.0, "controlled-pair")
                .unwrap()
                .unwrap()
                .promoted
        );
    }
    #[test]
    fn complete_store_budget_covers_promoted_rows_not_only_pending_cap() {
        let dir = OwnedStore::new();
        for index in 0..=store::AUDIT_PAIR_MAX_ROWS {
            store::ensure_learn_pair_promoted(
                &dir.0,
                &format!("fixture-{index}"),
                "before",
                "after",
                Some(&store::LearnPairScope::default()),
            )
            .unwrap();
        }
        assert!(matches!(
            store::list_learn_pairs_for_audit(&dir.0),
            Err(store::AuditPairReadError::Capacity {
                resource: "rows",
                ..
            })
        ));
        let oversized = OwnedStore::new();
        store::ensure_learn_pair_promoted(
            &oversized.0,
            "big",
            "before",
            &"字".repeat(store::AUDIT_PAIR_MAX_UTF8_BYTES / 3 + 1),
            Some(&store::LearnPairScope::default()),
        )
        .unwrap();
        assert!(matches!(
            store::list_learn_pairs_for_audit(&oversized.0),
            Err(store::AuditPairReadError::Capacity {
                resource: "utf8_bytes",
                ..
            })
        ));
    }
    #[test]
    fn exact_serde_count_includes_control_character_expansion() {
        let value = "\0\n\t字";
        let encoded = serde_json::to_vec(value).unwrap();
        assert!(encoded.len() > value.len());
        let mut at_limit = CountWriter {
            bytes: 0,
            limit: encoded.len(),
        };
        serde_json::to_writer(&mut at_limit, value).unwrap();
        assert_eq!(at_limit.bytes, encoded.len());
        let mut under_limit = CountWriter {
            bytes: 0,
            limit: encoded.len() - 1,
        };
        assert!(serde_json::to_writer(&mut under_limit, value).is_err());
        assert!(under_limit.bytes <= under_limit.limit);
    }
}
