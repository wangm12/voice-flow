#[cfg(test)]
use crate::queue::QuotaView;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod history_export;
mod learn_pairs;
mod settings_model;
mod spool;

#[allow(unused_imports)]
pub(crate) use self::history_export::*;
#[allow(unused_imports)]
pub(crate) use self::learn_pairs::*;
#[allow(unused_imports)]
pub(crate) use self::settings_model::*;
#[allow(unused_imports)]
pub(crate) use self::spool::*;

pub const SETTINGS_SCHEMA_VERSION: u32 = 27;
pub(super) const HISTORY_SCHEMA_VERSION: i32 = 21;
// Migration rollback copies contain historical text, so they have a finite
// lifetime even when the user elects to keep the live history forever.
pub(super) const HISTORY_MIGRATION_BACKUP_MAX_DAYS: u64 = 7;
pub(super) const LEARN_PAIRS_PENDING_CAP: i64 = 256;
pub(super) const PENDING_HISTORY_JOB_CAP: usize = 16;
pub(super) const PENDING_HISTORY_BYTE_CAP: usize = 40 * 1024 * 1024;
pub(super) const PENDING_HISTORY_JOB_MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    pub id: i64,
    pub created_at: String,
    pub raw_text: String,
    /// Provider-returned transcript before local filtering, revisions, or
    /// dictionary replacement. Null on rows written before this field existed.
    pub asr_text: Option<String>,
    /// Provider-supplied cleanup candidate kept distinct from the ASR text and
    /// VoiceFlow's final guarded output. Null when the provider supplied none.
    pub provider_cleaned_candidate: Option<String>,
    /// Complete cleanup text rejected by a semantic preservation guard.
    /// Separate from provider_cleaned_candidate, which can belong to ASR.
    pub cleanup_rejected_candidate: Option<String>,
    /// Stage-qualified, non-sensitive preservation reason code.
    pub cleanup_guard_reason: Option<String>,
    /// Nullable provenance; None means this legacy/edited row has unknown provenance.
    #[serde(default)]
    pub cleanup_attempt_kind: Option<String>,
    /// None is unknown for legacy rows; Some([]) is an observed no-transform result.
    #[serde(default)]
    pub cleanup_transformations: Option<Vec<String>>,
    #[serde(default)]
    pub cleanup_final_guard_reason: Option<String>,
    #[serde(default)]
    pub cleanup_failure_reason: Option<String>,
    /// ASR provider/model that produced the selected transcript, when known.
    pub engine: Option<String>,
    /// Derived only from facts already retained by this History row. Missing
    /// context and screen fields remain unknown when the source did not store them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_receipt: Option<crate::preview_provenance::PreviewProvenanceReceipt>,
    pub final_text: String,
    pub cleanup_status: String,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub status: String,
    pub delivery_method: Option<String>,
    pub fallback_reason: Option<String>,
    /// Stable reason code for a paste delivery failure or unverified result.
    pub delivery_error_code: Option<String>,
    /// Safe, transcript-free next step shown in History.
    pub delivery_user_reason: Option<String>,
    pub context_profile_id: Option<String>,
    pub retryable: bool,
    pub revision_count: usize,
    pub has_audio: bool,
    pub verbatim_text: Option<String>,
    pub verbatim_reviewed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRevision {
    pub revision_id: i64,
    pub dictation_id: i64,
    pub created_at: String,
    pub final_text: String,
    pub cleanup_status: Option<String>,
    pub intent: Option<serde_json::Value>,
    pub model: Option<String>,
    pub context_policy: Option<serde_json::Value>,
    pub revision_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize)]
#[cfg(test)]
pub struct Usage {
    pub asr_requests: i64,
    pub llm_requests: i64,
    pub audio_seconds: f64,
    pub quota: QuotaView,
}

pub(super) fn schema(c: &Connection) -> anyhow::Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS dictations (
            id INTEGER PRIMARY KEY,
            created_at TEXT NOT NULL,
            duration_secs REAL,
            raw_text TEXT,
            final_text TEXT,
            cleanup_status TEXT,
            engine TEXT,
            degraded INTEGER,
            degraded_reason TEXT,
            status TEXT,
            raw_audio_path TEXT,
            delivery_method TEXT,
            fallback_reason TEXT,
            context_profile_id TEXT,
            context_policy_json TEXT
        );
        CREATE TABLE IF NOT EXISTS dictation_revisions (
            revision_id INTEGER PRIMARY KEY,
            dictation_id INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            final_text TEXT NOT NULL,
            cleanup_status TEXT,
            intent_json TEXT,
            model TEXT,
            context_policy_json TEXT,
            revision_reason TEXT NOT NULL,
            FOREIGN KEY(dictation_id) REFERENCES dictations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_dictation_revisions_dictation
            ON dictation_revisions(dictation_id, revision_id DESC);
        CREATE TABLE IF NOT EXISTS learn_observations (
            observation_key TEXT PRIMARY KEY,
            source_kind TEXT NOT NULL,
            source_history_id INTEGER,
            source_revision_id INTEGER,
            created_at TEXT NOT NULL,
            rule_key TEXT,
            dictation_origin TEXT,
            channel TEXT,
            proof_eligible INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY(source_history_id) REFERENCES dictations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_learn_observations_history
            ON learn_observations(source_history_id);
        CREATE TABLE IF NOT EXISTS usage (
            day TEXT PRIMARY KEY,
            asr_requests INTEGER NOT NULL DEFAULT 0,
            llm_requests INTEGER NOT NULL DEFAULT 0,
            audio_seconds REAL NOT NULL DEFAULT 0
        );",
    )?;
    ensure_column(c, "raw_audio_path", "TEXT")?;
    ensure_column(c, "degraded_reason", "TEXT")?;
    ensure_column(c, "delivery_method", "TEXT")?;
    ensure_column(c, "fallback_reason", "TEXT")?;
    ensure_column(c, "delivery_error_code", "TEXT")?;
    ensure_column(c, "delivery_user_reason", "TEXT")?;
    ensure_column(c, "context_profile_id", "TEXT")?;
    ensure_column(c, "context_policy_json", "TEXT")?;
    ensure_column(c, "context_family", "TEXT")?;
    ensure_column(c, "context_browser_host", "TEXT")?;
    ensure_column(c, "context_native_bundle", "TEXT")?;
    ensure_column(c, "context_mapping_identity", "TEXT")?;
    ensure_column(c, "cleanup_status", "TEXT")?;
    ensure_column(c, "verbatim_text", "TEXT")?;
    ensure_column(c, "verbatim_reviewed", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_column(c, "asr_text", "TEXT")?;
    ensure_column(c, "provider_cleaned_candidate", "TEXT")?;
    ensure_column(c, "cleanup_rejected_candidate", "TEXT")?;
    ensure_column(c, "cleanup_guard_reason", "TEXT")?;
    ensure_column(c, "cleanup_attempt_kind", "TEXT")?;
    ensure_column(c, "cleanup_transformations_json", "TEXT")?;
    ensure_column(c, "cleanup_final_guard_reason", "TEXT")?;
    ensure_column(c, "cleanup_failure_reason", "TEXT")?;
    ensure_column(c, "transaction_id", "TEXT")?;
    ensure_table_column(c, "learn_observations", "rule_key", "TEXT")?;
    ensure_table_column(c, "learn_observations", "dictation_origin", "TEXT")?;
    ensure_table_column(c, "learn_observations", "channel", "TEXT")?;
    ensure_table_column(
        c,
        "learn_observations",
        "proof_eligible",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_learn_observations_origin
         ON learn_observations(dictation_origin);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_learn_observations_independent_proof
         ON learn_observations(rule_key, dictation_origin)
         WHERE proof_eligible = 1 AND rule_key IS NOT NULL AND dictation_origin IS NOT NULL;",
    )?;
    c.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_dictations_transaction_id
         ON dictations(transaction_id) WHERE transaction_id IS NOT NULL;",
    )?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS learn_pairs (
            pair_key TEXT PRIMARY KEY,
            before_surface TEXT NOT NULL,
            after_surface TEXT NOT NULL,
            hits INTEGER NOT NULL,
            promoted INTEGER NOT NULL DEFAULT 0,
            last_at TEXT NOT NULL,
            dictionary_pending_action TEXT NOT NULL DEFAULT 'none',
            dictionary_effect_version INTEGER NOT NULL DEFAULT 0,
            match_policy TEXT NOT NULL DEFAULT 'ignore_case',
            legacy_pair_key TEXT
        );
        CREATE TABLE IF NOT EXISTS learned_term_usage (
            word TEXT PRIMARY KEY,
            replacement_runs INTEGER NOT NULL,
            last_replaced_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS style_drafts (
            draft_key TEXT PRIMARY KEY,
            mapping_id TEXT NOT NULL,
            style_key TEXT NOT NULL,
            excerpt TEXT NOT NULL,
            before_excerpt TEXT NOT NULL,
            after_excerpt TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS writing_preference_evidence (
            source_history_id INTEGER NOT NULL,
            source_revision_id INTEGER NOT NULL,
            mapping_id TEXT NOT NULL,
            mapping_identity_fingerprint TEXT NOT NULL DEFAULT '',
            mapping_revision INTEGER NOT NULL,
            task_id TEXT NOT NULL,
            dimension TEXT NOT NULL,
            value TEXT NOT NULL,
            origin_id TEXT NOT NULL,
            content_fingerprint TEXT NOT NULL,
            template_family_fingerprint TEXT,
            occurred_at_ms INTEGER NOT NULL,
            natural_day TEXT NOT NULL,
            editor_session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            eligible INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY(mapping_id, origin_id),
            FOREIGN KEY(source_history_id) REFERENCES dictations(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_preference_evidence_expiry
            ON writing_preference_evidence(occurred_at_ms);
        CREATE TABLE IF NOT EXISTS writing_preference_draft_decisions (
            candidate_id TEXT PRIMARY KEY,
            mapping_id TEXT NOT NULL,
            mapping_revision INTEGER NOT NULL,
            task_id TEXT NOT NULL,
            dimension TEXT NOT NULL,
            value TEXT NOT NULL,
            generation INTEGER NOT NULL,
            decision TEXT NOT NULL CHECK(decision IN ('dismissed', 'approved')),
            decided_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_preference_decisions_scope
            ON writing_preference_draft_decisions(mapping_id, mapping_revision, task_id, dimension, value, generation);
        CREATE INDEX IF NOT EXISTS idx_preference_decisions_expiry
            ON writing_preference_draft_decisions(decided_at_ms);",
    )?;
    ensure_table_column(
        c,
        "writing_preference_evidence",
        "mapping_identity_fingerprint",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    c.execute_batch(
        "DROP INDEX IF EXISTS idx_preference_evidence_scope;
         CREATE INDEX idx_preference_evidence_scope
            ON writing_preference_evidence(mapping_id, mapping_identity_fingerprint, task_id, dimension, value, generation, eligible);",
    )?;
    ensure_table_column(c, "learn_pairs", "family", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "mapping_id", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "browser_host", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "native_bundle", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "last_used_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "pinned", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_table_column(c, "learn_pairs", "tombstoned_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "ignored", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_table_column(
        c,
        "learn_pairs",
        "manual_scoped",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    // Automatically learned scene pairs are separately marked from pairs the
    // user confirmed manually. The default keeps all legacy rows inert unless
    // their target also remains in the global dictionary.
    ensure_table_column(
        c,
        "learn_pairs",
        "auto_scoped",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_table_column(
        c,
        "learn_pairs",
        "dictionary_pending_action",
        "TEXT NOT NULL DEFAULT 'none'",
    )?;
    ensure_table_column(
        c,
        "learn_pairs",
        "dictionary_effect_version",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_table_column(
        c,
        "learn_pairs",
        "match_policy",
        "TEXT NOT NULL DEFAULT 'ignore_case'",
    )?;
    ensure_table_column(c, "learn_pairs", "legacy_pair_key", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "binding_invalidated_at", "TEXT")?;
    ensure_table_column(c, "learn_pairs", "binding_invalidated_reason", "TEXT")?;
    ensure_table_column(
        c,
        "learn_pairs",
        "replacement_runs",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    c.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_learn_pairs_legacy_key
         ON learn_pairs(legacy_pair_key) WHERE legacy_pair_key IS NOT NULL;",
    )?;
    c.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)?;
    Ok(())
}

pub(super) fn open_history(dir: &Path) -> anyhow::Result<Connection> {
    ensure_private_dir(dir)?;
    let path = dir.join("history.sqlite");
    let existed = path.exists();
    let connection = Connection::open(&path)?;
    restrict_file_mode(&path)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
    // History writes can overlap with a completion, recovery scan, or the
    // settings window loading its list. Let SQLite briefly wait for the
    // active writer instead of turning a transient lock into lost history.
    connection.busy_timeout(Duration::from_secs(2))?;

    let schema_lock = HISTORY_SCHEMA_LOCK.get_or_init(|| Mutex::new(()));
    let _schema_guard = schema_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_version: i32 =
        connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if previous_version > HISTORY_SCHEMA_VERSION {
        anyhow::bail!("history schema {previous_version} is newer than supported schema {HISTORY_SCHEMA_VERSION}");
    }
    if previous_version != HISTORY_SCHEMA_VERSION {
        if existed && previous_version < HISTORY_SCHEMA_VERSION {
            let backup = dir.join(format!("history.sqlite.v{previous_version}.bak"));
            match fs::symlink_metadata(&backup) {
                Ok(metadata) if metadata.file_type().is_file() => {}
                Ok(_) => anyhow::bail!("unsafe history migration backup path"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    create_history_migration_backup(&connection, dir, &backup)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        // Opening the already-current schema is read-only. Reissuing the
        // CREATE/ensure statements on every History read takes a SQLite
        // schema writer lock and can block settings/preparation reads behind
        // an unrelated retained History transaction.
        connection.execute_batch("BEGIN IMMEDIATE")?;
        match schema(&connection) {
            Ok(()) => connection.execute_batch("COMMIT")?,
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
    }
    // Family labels identify broad app classes, not an individual target. Keep
    // legacy family-only promoted pairs inert; query first so normal History
    // opens stay read-only when no rows need the one-way policy migration.
    demote_family_only_promoted_learn_pairs(&connection)?;
    restrict_history_sidecars(dir)?;
    prune_history_migration_backups(
        dir,
        Some(HISTORY_MIGRATION_BACKUP_MAX_DAYS),
        SystemTime::now(),
    )?;
    Ok(connection)
}

pub(super) fn create_history_migration_backup(
    connection: &Connection,
    dir: &Path,
    backup: &Path,
) -> anyhow::Result<()> {
    use std::sync::atomic::AtomicU64;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    let temp_dir = dir.join(format!(
        ".history-backup-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    create_history_backup_temp_dir(&temp_dir)?;
    let result = (|| -> anyhow::Result<()> {
        let temporary_backup = temp_dir.join("history.sqlite");
        connection.execute(
            "VACUUM INTO ?1",
            [temporary_backup.to_string_lossy().as_ref()],
        )?;
        restrict_file_mode(&temporary_backup)?;
        let validation = Connection::open_with_flags(
            &temporary_backup,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let integrity: String =
            validation.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            anyhow::bail!("migration backup integrity check failed: {integrity}");
        }
        drop(validation);
        // A same-volume hard link atomically publishes the complete, checked
        // database and fails if any file or symlink already occupies the name.
        fs::hard_link(&temporary_backup, backup)?;
        sync_directory(dir)?;
        Ok(())
    })();
    let cleanup_result = fs::remove_dir_all(&temp_dir);
    result?;
    cleanup_result?;
    Ok(())
}

pub(super) fn create_history_backup_temp_dir(temp_dir: &Path) -> anyhow::Result<()> {
    let mut owns_temp_dir = false;
    let setup_result = (|| -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(temp_dir)?;
            owns_temp_dir = true;
            fs::set_permissions(temp_dir, fs::Permissions::from_mode(0o700))?;
            if fs::symlink_metadata(temp_dir)?.uid() != unsafe { libc::geteuid() } {
                anyhow::bail!("migration backup temporary directory is not owned by this user");
            }
        }
        #[cfg(not(unix))]
        {
            fs::create_dir(&temp_dir)?;
            owns_temp_dir = true;
        }
        Ok(())
    })();
    if let Err(error) = setup_result {
        if owns_temp_dir {
            let _ = fs::remove_dir_all(temp_dir);
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn demote_family_only_promoted_learn_pairs(
    connection: &Connection,
) -> anyhow::Result<()> {
    let has_legacy_rows: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM learn_pairs
            WHERE promoted = 1
              AND family IS NOT NULL
              AND family <> ''
              AND COALESCE(mapping_id, '') = ''
              AND COALESCE(browser_host, '') = ''
              AND COALESCE(native_bundle, '') = ''
        )",
        [],
        |row| row.get(0),
    )?;
    if has_legacy_rows {
        connection.execute(
            "UPDATE learn_pairs SET promoted = 0
             WHERE promoted = 1
               AND family IS NOT NULL
               AND family <> ''
               AND COALESCE(mapping_id, '') = ''
               AND COALESCE(browser_host, '') = ''
               AND COALESCE(native_bundle, '') = ''",
            [],
        )?;
    }
    Ok(())
}

/// Only names generated by open_history are owned rollback copies. Do not
/// recurse or follow links; user exports, settings and model files are outside
/// this cleanup boundary. None means the user explicitly cleared all data.
pub(super) fn prune_history_migration_backups(
    dir: &Path,
    keep_days: Option<u64>,
    now: SystemTime,
) -> anyhow::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(version) = name
            .to_str()
            .and_then(|name| name.strip_prefix("history.sqlite.v"))
            .and_then(|name| name.strip_suffix(".bak"))
        else {
            continue;
        };
        if !version
            .parse::<u32>()
            .is_ok_and(|value| value.to_string() == version)
        {
            continue;
        }
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let expired = match keep_days {
            None => true,
            Some(days) => now
                .duration_since(metadata.modified()?)
                .is_ok_and(|age| age >= Duration::from_secs(days.saturating_mul(86_400))),
        };
        if expired {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(removed)
}

pub(super) static HISTORY_SCHEMA_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) fn ensure_column(c: &Connection, name: &str, definition: &str) -> anyhow::Result<()> {
    ensure_table_column(c, "dictations", name, definition)
}

pub(super) fn ensure_table_column(
    c: &Connection,
    table: &str,
    name: &str,
    definition: &str,
) -> anyhow::Result<()> {
    let mut statement = c.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == name);
    if !exists {
        c.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}
pub struct InsertHistory<'a> {
    pub dir: &'a Path,
    pub raw: &'a str,
    pub asr_text: Option<&'a str>,
    pub engine: Option<&'a str>,
    pub final_text: &'a str,
    pub cleanup_status: Option<&'a str>,
    pub duration: f64,
    pub degraded: bool,
    pub degraded_reason: Option<&'a str>,
    pub status: String,
    pub delivery_method: Option<&'a str>,
    pub fallback_reason: Option<&'a str>,
    pub spool: Option<&'a Path>,
    pub count_asr: bool,
    pub count_llm: bool,
    pub context: Option<&'a crate::context::ContextSnapshot>,
    pub cleanup_attempt_kind: Option<&'a str>,
    pub cleanup_transformations_json: Option<&'a str>,
    pub cleanup_final_guard_reason: Option<&'a str>,
    pub cleanup_failure_reason: Option<&'a str>,
}

/// Durable, content-bearing History intent written before a paste/copy side
/// effect. It contains only fields that the normal History insert already
/// persists; ContextPolicy must be pre-filtered with `history_metadata`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingHistoryIntent {
    pub transaction_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_spool_session_id: Option<String>,
    pub created_at_unix_ms: i64,
    pub raw_text: String,
    pub asr_text: Option<String>,
    pub provider_cleaned_candidate: Option<String>,
    pub final_text: String,
    pub duration_secs: f64,
    pub degraded: bool,
    pub degraded_reason: Option<String>,
    pub status: String,
    pub delivery_method: String,
    pub fallback_reason: Option<String>,
    pub context_profile_id: Option<String>,
    pub context_policy_json: Option<String>,
    pub context_family: Option<String>,
    pub context_browser_host: Option<String>,
    pub context_native_bundle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mapping_identity: Option<String>,
    pub spool_path: Option<PathBuf>,
    pub engine: String,
    pub cleanup_status: String,
    pub diagnostic_code: Option<String>,
    pub diagnostic_user_reason: Option<String>,
    pub cleanup_rejected_candidate: Option<String>,
    pub cleanup_guard_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_attempt_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_transformations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_final_guard_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_failure_reason: Option<String>,
}

impl PendingHistoryIntent {
    fn validate(&self) -> anyhow::Result<()> {
        if self.transaction_id.is_empty()
            || self.transaction_id.len() > 96
            || !self
                .transaction_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            anyhow::bail!("invalid pending History transaction identity");
        }
        if self
            .capture_spool_session_id
            .as_deref()
            .is_some_and(|session_id| !valid_spool_session_id(session_id))
        {
            anyhow::bail!("invalid pending History capture spool identity");
        }
        if !matches!(
            self.status.as_str(),
            "delivery_pending"
                | "delivery_cancelled"
                | "ok"
                | "degraded"
                | "history"
                | "copied"
                | "unverified"
                | "failed"
        ) {
            anyhow::bail!("invalid pending History delivery status");
        }
        if self.final_text.trim().is_empty()
            || !self.duration_secs.is_finite()
            || self.duration_secs < 0.0
            || self.status.len() > 128
            || self.delivery_method.len() > 64
            || self
                .cleanup_failure_reason
                .as_deref()
                .is_some_and(|reason| {
                    reason.len() > 96
                        || reason.is_empty()
                        || !reason.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                        })
                })
        {
            anyhow::bail!("invalid pending History payload");
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn pending_history_intent_for_test(
    transaction_id: &str,
    status: &str,
    delivery_method: &str,
    text: &str,
) -> PendingHistoryIntent {
    PendingHistoryIntent {
        transaction_id: transaction_id.to_owned(),
        capture_spool_session_id: None,
        created_at_unix_ms: now_ms() as i64,
        raw_text: text.to_owned(),
        asr_text: Some(text.to_owned()),
        provider_cleaned_candidate: None,
        final_text: text.to_owned(),
        duration_secs: 1.0,
        degraded: false,
        degraded_reason: None,
        status: status.to_owned(),
        delivery_method: delivery_method.to_owned(),
        fallback_reason: None,
        context_profile_id: None,
        context_policy_json: None,
        context_family: None,
        context_browser_host: None,
        context_native_bundle: None,
        context_mapping_identity: None,
        spool_path: None,
        engine: "test-engine".into(),
        cleanup_status: "cleanup_not_requested".into(),
        diagnostic_code: None,
        diagnostic_user_reason: None,
        cleanup_rejected_candidate: None,
        cleanup_guard_reason: None,
        cleanup_attempt_kind: None,
        cleanup_transformations: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    }
}

pub(super) fn pending_history_root(data_root: &Path) -> PathBuf {
    normalized_data_root(data_root).join("history-pending")
}

pub(super) fn pending_history_path(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<PathBuf> {
    if transaction_id.is_empty()
        || transaction_id.len() > 96
        || !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid pending History transaction identity");
    }
    Ok(pending_history_root(data_root)
        .join(transaction_id)
        .join("intent.json"))
}

pub(super) fn pending_history_usage(root: &Path) -> anyhow::Result<(usize, u64)> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!("pending History directory is not a private directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error.into()),
    }
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() || file_type.is_symlink() {
            anyhow::bail!("pending History root contains an unowned entry");
        }
        let id = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .filter(|id| {
                !id.is_empty()
                    && id.len() <= 96
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            .ok_or_else(|| anyhow::anyhow!("pending History root contains an invalid identity"))?;
        let directory = entry.path();
        let mut files = 0usize;
        for child in fs::read_dir(&directory)? {
            let child = child?;
            let child_type = child.file_type()?;
            let name = child.file_name();
            if !child_type.is_file()
                || child_type.is_symlink()
                || !matches!(
                    name.to_str(),
                    Some("intent.json" | ".intent.pending" | ".delivery.cancelled")
                )
            {
                anyhow::bail!("pending History transaction contains an unowned entry");
            }
            if name == ".delivery.cancelled" && child.metadata()?.len() > 96 {
                anyhow::bail!("pending History cancellation marker is oversized");
            }
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(child.metadata()?.len());
        }
        if files > 0 {
            count = count.saturating_add(1);
        }
        if files > 3 {
            anyhow::bail!("pending History transaction exceeds its file-count bound");
        }
        let expected = root.join(id);
        if normalized_data_root(&directory) != normalized_data_root(&expected) {
            anyhow::bail!("pending History identity directory does not match its name");
        }
    }
    Ok((count, bytes))
}

pub(super) fn write_pending_history_atomically(
    data_root: &Path,
    path: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid pending History path"))?;
    let expected_root = normalized_data_root(&data_root.join("history-pending"));
    if normalized_data_root(parent).parent() != Some(expected_root.as_path())
        || path.file_name().is_none_or(|name| name != "intent.json")
    {
        anyhow::bail!("pending History writer does not own this intent path");
    }
    ensure_private_dir(parent)?;
    let staged = parent.join(".intent.pending");
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = File::create(&staged)?;
        restrict_file_mode(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(test)]
        wait_for_pending_history_write_test_gate(data_root);
        fs::rename(&staged, path)?;
        restrict_file_mode(path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        // A complete staged file is recovery-owned evidence. Leave it in
        // place for startup reconciliation instead of deleting possibly
        // unique transcript content after an ambiguous rename failure.
        if fs::metadata(&staged).is_ok_and(|metadata| metadata.len() == 0) {
            let _ = fs::remove_file(&staged);
        }
        if fs::read_dir(parent)
            .ok()
            .is_some_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(parent);
        }
    }
    write_result
}

pub(super) fn recover_pending_history_staging(root: &Path) -> anyhow::Result<usize> {
    let (count, _) = pending_history_usage(root)?;
    if count == 0 {
        return Ok(0);
    }
    let mut recovered = 0usize;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let id = entry.file_name().to_str().map(str::to_owned).filter(|id| {
            !id.is_empty()
                && id.len() <= 96
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
        let Some(id) = id else {
            continue;
        };
        if pending_history_cancelled(root, &id)? {
            let transaction_dir = entry.path();
            fs::remove_dir_all(&transaction_dir)?;
            if let Some(parent) = transaction_dir.parent() {
                File::open(parent)?.sync_all()?;
            }
            continue;
        }
        let staged = entry.path().join(".intent.pending");
        let metadata = match fs::symlink_metadata(&staged) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64
        {
            anyhow::bail!("pending History staging file is not a bounded regular file");
        }
        let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(&staged)?)?;
        intent.validate()?;
        if intent.transaction_id != id {
            anyhow::bail!("pending History staging identity does not match its directory");
        }
        // `root` is already the history-pending directory, whereas
        // `pending_history_path` accepts the application data root. Keep the
        // recovery destination under the enumerated transaction directory.
        let final_path = entry.path().join("intent.json");
        fs::rename(&staged, &final_path)?;
        restrict_file_mode(&final_path)?;
        File::open(entry.path())?.sync_all()?;
        recovered = recovered.saturating_add(1);
    }
    Ok(recovered)
}

pub(super) fn pending_history_cancelled(root: &Path, transaction_id: &str) -> anyhow::Result<bool> {
    let marker = root.join(transaction_id).join(".delivery.cancelled");
    match fs::symlink_metadata(&marker) {
        Ok(metadata)
            if metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= 96 =>
        {
            let recorded_id = fs::read_to_string(marker)?;
            if recorded_id != transaction_id {
                anyhow::bail!("pending History cancellation identity does not match its path");
            }
            Ok(true)
        }
        Ok(_) => anyhow::bail!("pending History cancellation marker is not a bounded file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn active_pending_history_origin(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<bool> {
    let key = (normalized_data_root(data_root), transaction_id.to_owned());
    let active = PENDING_HISTORY_ACTIVE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&key);
    if !active {
        return Ok(false);
    }
    let root = pending_history_root(data_root);
    if pending_history_cancelled(&root, transaction_id)? {
        return Ok(false);
    }
    let path = pending_history_path(data_root, transaction_id)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64
    {
        anyhow::bail!("pending History origin is not a bounded regular file");
    }
    let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(path)?)?;
    intent.validate()?;
    Ok(intent.transaction_id == transaction_id && intent.status != "delivery_cancelled")
}

pub(super) fn discard_pending_dictation_proofs(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<()> {
    if !data_root.join("history.sqlite").exists() {
        return Ok(());
    }
    let mut connection = open_history(data_root)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM learn_observations
         WHERE dictation_origin = ?1
           AND proof_eligible = 1
           AND source_history_id IS NULL",
        [transaction_id],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Persist one bounded recovery intent before any irreversible delivery
/// action. The shared pending-tree scope protects quota and enumeration while
/// remaining disjoint from History database commits and audio capture.
pub(crate) fn write_pending_history_intent(
    data_root: &Path,
    intent: &PendingHistoryIntent,
    owner: &PendingHistoryActiveLease,
) -> anyhow::Result<()> {
    owner.ensure_writable(data_root, &intent.transaction_id)?;
    intent.validate()?;
    let serialized = serde_json::to_vec(intent)?;
    if serialized.len() > PENDING_HISTORY_JOB_MAX_BYTES {
        anyhow::bail!("pending History payload exceeds the bounded size limit");
    }
    let _mutation = begin_spool_root_mutation(data_root)?;
    let _commit = try_spool_io_commit_pending_history(data_root, &intent.transaction_id)?;
    owner.ensure_writable(data_root, &intent.transaction_id)?;
    let root = pending_history_root(data_root);
    let path = pending_history_path(data_root, &intent.transaction_id)?;
    let budget_lock = PENDING_HISTORY_BUDGET_LOCK.get_or_init(|| Mutex::new(()));
    let _budget_guard = budget_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_private_dir(&root)?;
    let (count, bytes) = pending_history_usage(&root)?;
    if pending_history_cancelled(&root, &intent.transaction_id)? {
        anyhow::bail!("pending History identity was already cancelled");
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => anyhow::bail!("pending History intent path is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let transaction_dir_exists = path.parent().is_some_and(Path::exists);
    let projected_count = count.saturating_add(usize::from(!transaction_dir_exists));
    // The old final and new staging files coexist until atomic rename, so
    // charge the complete write while it is in flight.
    let projected_bytes = bytes.saturating_add(serialized.len() as u64);
    if projected_count > PENDING_HISTORY_JOB_CAP
        || projected_bytes > PENDING_HISTORY_BYTE_CAP as u64
    {
        anyhow::bail!("pending History recovery capacity is full");
    }
    write_pending_history_atomically(data_root, &path, &serialized)?;
    File::open(&root)?.sync_all()?;
    Ok(())
}

pub(crate) fn remove_pending_history_intent(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<()> {
    let (_mutation, _commit) =
        acquire_pending_history_mutation_and_commit(data_root, transaction_id)?;
    let budget_lock = PENDING_HISTORY_BUDGET_LOCK.get_or_init(|| Mutex::new(()));
    let _budget_guard = budget_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    remove_pending_history_intent_under_permit(data_root, transaction_id)
}

pub(super) fn remove_pending_history_intent_under_permit(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<()> {
    let path = pending_history_path(data_root, transaction_id)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(&path)?;
            if let Some(parent) = path.parent() {
                File::open(parent)?.sync_all()?;
                match fs::remove_dir(parent) {
                    Ok(()) => {
                        if let Some(root) = parent.parent() {
                            File::open(root)?.sync_all()?;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        }
        Ok(_) => anyhow::bail!("pending History intent path is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Mark a known no-side-effect cancellation before attempting cleanup. If
/// removing the file then fails, startup recognizes the tombstone and will
/// discard it without creating a misleading History row.
pub(crate) fn cancel_pending_history_intent(
    data_root: &Path,
    transaction_id: &str,
    owner: &PendingHistoryActiveLease,
) -> anyhow::Result<()> {
    owner.poison_for_known_no_effect_cancel(data_root, transaction_id)?;
    let (_mutation, _commit) =
        acquire_pending_history_mutation_and_commit(data_root, transaction_id)?;
    let budget_lock = PENDING_HISTORY_BUDGET_LOCK.get_or_init(|| Mutex::new(()));
    let _budget_guard = budget_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = pending_history_root(data_root);
    let transaction_dir = root.join(transaction_id);
    ensure_private_dir(&transaction_dir)?;
    let marker = transaction_dir.join(".delivery.cancelled");
    match fs::symlink_metadata(&marker) {
        Ok(_) => {
            if !pending_history_cancelled(&root, transaction_id)? {
                anyhow::bail!("pending History cancellation marker is invalid");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = File::create(&marker)?;
            restrict_file_mode(&marker)?;
            file.write_all(transaction_id.as_bytes())?;
            file.sync_all()?;
            File::open(&transaction_dir)?.sync_all()?;
        }
        Err(error) => return Err(error.into()),
    }
    discard_pending_dictation_proofs(data_root, transaction_id)?;
    let path = pending_history_path(data_root, transaction_id)?;
    let staged = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid pending History path"))?
        .join(".intent.pending");
    for candidate in [&path, &staged] {
        match fs::symlink_metadata(candidate) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= PENDING_HISTORY_JOB_MAX_BYTES as u64 =>
            {
                if candidate == &staged {
                    let staged_intent: PendingHistoryIntent =
                        serde_json::from_slice(&fs::read(candidate)?)?;
                    staged_intent.validate()?;
                    if staged_intent.transaction_id != transaction_id {
                        anyhow::bail!("pending History staging identity does not match its path");
                    }
                } else {
                    let intent: PendingHistoryIntent =
                        serde_json::from_slice(&fs::read(candidate)?)?;
                    intent.validate()?;
                    if intent.transaction_id != transaction_id {
                        anyhow::bail!("pending History intent identity does not match its path");
                    }
                }
                fs::remove_file(candidate)?;
            }
            Ok(_) => anyhow::bail!("pending History intent cannot be safely cancelled"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let marker = transaction_dir.join(".delivery.cancelled");
    match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(&marker)?;
        }
        Ok(_) => anyhow::bail!("pending History cancellation marker is invalid"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    File::open(&transaction_dir)?.sync_all()?;
    match fs::remove_dir(&transaction_dir) {
        Ok(()) => File::open(&root)?.sync_all()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Read the exact text owned by one live History recovery ticket. The ticket
/// caller supplies only the stable transaction identity; the text is loaded
/// from that transaction's validated pending intent or matching History row.
pub(crate) fn history_recovery_text_for_transaction(
    data_root: &Path,
    transaction_id: &str,
) -> anyhow::Result<String> {
    let _mutation = begin_spool_root_mutation(data_root)?;
    let pending_root = pending_history_root(data_root);
    {
        let _pending_commit = try_spool_io_commit_pending_history(data_root, transaction_id)?;
        if pending_history_cancelled(&pending_root, transaction_id)? {
            anyhow::bail!("History recovery was cancelled");
        }
        let path = pending_history_path(data_root, transaction_id)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= PENDING_HISTORY_JOB_MAX_BYTES as u64 =>
            {
                let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(path)?)?;
                intent.validate()?;
                if intent.transaction_id != transaction_id {
                    anyhow::bail!("pending History identity does not match its ticket");
                }
                if intent.final_text.trim().is_empty() {
                    anyhow::bail!("History recovery has no copyable text");
                }
                return Ok(intent.final_text);
            }
            Ok(_) => anyhow::bail!("pending History recovery is not a bounded regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let _history_commit = try_spool_io_commit_scope(data_root, SpoolIoScope::HistoryDatabase)?;
    let connection = open_history(data_root)?;
    let text = connection
        .query_row(
            "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(final_text,''),raw_text,'') FROM dictations WHERE transaction_id=?1",
            [transaction_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("History transaction has no copyable text"))?;
    Ok(text)
}

/// Promote a pending History intent exactly once. The transaction ID is a
/// unique database key, so a crash after SQLite commit but before deleting the
/// sidecar cannot create a duplicate on the next startup.
pub(crate) fn promote_pending_history_intent(
    data_root: &Path,
    transaction_id: &str,
    owner: &PendingHistoryActiveLease,
) -> anyhow::Result<()> {
    owner.ensure_writable(data_root, transaction_id)?;
    let _mutation = begin_spool_root_mutation(data_root)?;
    let _intent_commit = try_spool_io_commit_pending_history(data_root, transaction_id)?;
    owner.ensure_writable(data_root, transaction_id)?;
    let pending_root = pending_history_root(data_root);
    if pending_history_cancelled(&pending_root, transaction_id)? {
        discard_pending_dictation_proofs(data_root, transaction_id)?;
        let transaction_dir = pending_root.join(transaction_id);
        fs::remove_dir_all(&transaction_dir)?;
        File::open(&pending_root)?.sync_all()?;
        return Ok(());
    }
    let path = pending_history_path(data_root, transaction_id)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("pending History intent is not a regular file");
    }
    if metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64 {
        anyhow::bail!("pending History intent exceeds the bounded read size");
    }
    let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(&path)?)?;
    intent.validate()?;
    if intent.transaction_id != transaction_id {
        anyhow::bail!("pending History intent identity does not match its path");
    }
    if intent.status == "delivery_cancelled" {
        drop(_intent_commit);
        remove_pending_history_intent(data_root, transaction_id)?;
        return Ok(());
    }
    drop(_intent_commit);
    let _history_commit = try_spool_io_commit_scope(data_root, SpoolIoScope::HistoryDatabase)?;
    let mut connection = open_history(data_root)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing_id = transaction
        .query_row(
            "SELECT id FROM dictations WHERE transaction_id=?1",
            [&intent.transaction_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if existing_id.is_none() {
        let interrupted = intent.status == "delivery_pending";
        let status = if interrupted {
            "delivery_unconfirmed"
        } else {
            intent.status.as_str()
        };
        let delivery_method = if interrupted {
            "unknown"
        } else {
            intent.delivery_method.as_str()
        };
        let fallback_reason = if interrupted {
            Some("delivery_state_unknown_after_restart")
        } else {
            intent.fallback_reason.as_deref()
        };
        let cleanup_transformations_json = intent
            .cleanup_transformations
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        insert_history_with_status_and_candidate_on_connection(
            &transaction,
            InsertHistory {
                dir: data_root,
                raw: &intent.raw_text,
                asr_text: intent.asr_text.as_deref(),
                engine: Some(&intent.engine),
                final_text: &intent.final_text,
                cleanup_status: Some(&intent.cleanup_status),
                duration: intent.duration_secs,
                degraded: intent.degraded,
                degraded_reason: intent.degraded_reason.as_deref(),
                status: status.to_owned(),
                delivery_method: Some(delivery_method),
                fallback_reason,
                spool: intent.spool_path.as_deref(),
                count_asr: true,
                count_llm: true,
                context: None,
                cleanup_attempt_kind: intent.cleanup_attempt_kind.as_deref(),
                cleanup_transformations_json: cleanup_transformations_json.as_deref(),
                cleanup_final_guard_reason: intent.cleanup_final_guard_reason.as_deref(),
                cleanup_failure_reason: intent.cleanup_failure_reason.as_deref(),
            },
            intent.provider_cleaned_candidate.as_deref(),
            None,
            intent.cleanup_rejected_candidate.as_deref(),
            intent.cleanup_guard_reason.as_deref(),
        )?;
        let inserted_id = transaction.last_insert_rowid();
        transaction.execute(
            "UPDATE dictations SET transaction_id=?1, created_at=datetime(?2 / 1000, 'unixepoch'), context_profile_id=?3, context_policy_json=?4, context_family=?5, context_browser_host=?6, context_native_bundle=?7, context_mapping_identity=?8, delivery_error_code=?9, delivery_user_reason=?10 WHERE id=?11",
            params![
                intent.transaction_id,
                intent.created_at_unix_ms,
                intent.context_profile_id,
                intent.context_policy_json,
                intent.context_family,
                intent.context_browser_host,
                intent.context_native_bundle,
                intent.context_mapping_identity,
                if interrupted { Some("delivery_unconfirmed_after_restart") } else { intent.diagnostic_code.as_deref() },
                if interrupted { Some("Delivery was interrupted before its final result was recorded.") } else { intent.diagnostic_user_reason.as_deref() },
                inserted_id
            ],
        )?;
    }
    let history_id = transaction.query_row(
        "SELECT id FROM dictations WHERE transaction_id = ?1",
        [&intent.transaction_id],
        |row| row.get::<_, i64>(0),
    )?;
    transaction.execute(
        "UPDATE learn_observations
         SET source_history_id = ?1
         WHERE dictation_origin = ?2
           AND proof_eligible = 1
           AND source_history_id IS NULL",
        params![history_id, intent.transaction_id],
    )?;
    #[cfg(test)]
    wait_for_history_promotion_test_gate(data_root);
    transaction.commit()?;
    drop(_history_commit);
    // The row commit is the durable ACK. Keep the intent until its capture
    // spool has been retained or removed; startup repeats this idempotently if
    // the process exits between the database commit and spool cleanup.
    let capture_spool_settled = match intent.capture_spool_session_id.as_deref() {
        Some(session_id) => settle_capture_spool_after_history_ack(
            data_root,
            session_id,
            intent.spool_path.as_deref(),
        )?,
        None => true,
    };
    if capture_spool_settled {
        let _ = remove_pending_history_intent(data_root, transaction_id);
    }
    Ok(())
}

pub(crate) fn promote_all_pending_history_intents(data_root: &Path) -> anyhow::Result<usize> {
    let root = pending_history_root(data_root);
    let intents = {
        let _mutation = begin_spool_root_mutation(data_root)?;
        let _commit = try_spool_io_commit_scope(data_root, SpoolIoScope::PendingHistoryIntent)?;
        recover_pending_history_staging(&root)?;
        let (count, _) = pending_history_usage(&root)?;
        if count == 0 {
            return Ok(0);
        }
        let mut intents = fs::read_dir(&root)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        intents.sort();
        intents
    };
    let mut promoted = 0usize;
    let mut first_error: Option<anyhow::Error> = None;
    for transaction_id in intents {
        if !transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || transaction_id.is_empty()
            || transaction_id.len() > 96
        {
            first_error.get_or_insert_with(|| {
                anyhow::anyhow!("pending History root contains an invalid identity")
            });
            continue;
        }
        let Some(active_owner) = reserve_pending_history_startup_owner(data_root, &transaction_id)
        else {
            // A live dictation still owns this immutable transaction. Startup
            // maintenance must not promote its pre-delivery status while the
            // native delivery result is still being decided.
            continue;
        };
        let inspected = (|| -> anyhow::Result<bool> {
            let _mutation = begin_spool_root_mutation(data_root)?;
            let _commit = try_spool_io_commit_pending_history(data_root, &transaction_id)?;
            if pending_history_cancelled(&root, &transaction_id)? {
                discard_pending_dictation_proofs(data_root, &transaction_id)?;
                let transaction_dir = root.join(&transaction_id);
                fs::remove_dir_all(&transaction_dir)?;
                File::open(&root)?.sync_all()?;
                return Ok(true);
            }
            let intent_path = pending_history_path(data_root, &transaction_id)?;
            let metadata = fs::symlink_metadata(&intent_path)?;
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > PENDING_HISTORY_JOB_MAX_BYTES as u64
            {
                anyhow::bail!("pending History intent is not a bounded regular file");
            }
            let intent: PendingHistoryIntent = serde_json::from_slice(&fs::read(intent_path)?)?;
            intent.validate()?;
            if intent.transaction_id != transaction_id {
                anyhow::bail!("pending History identity does not match its directory");
            }
            Ok(false)
        })();
        match inspected {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        }
        match promote_pending_history_intent(data_root, &transaction_id, &active_owner) {
            Ok(()) => promoted = promoted.saturating_add(1),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(promoted),
    }
}

#[cfg(test)]
pub fn insert_history(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: None,
        final_text,
        cleanup_status: None,
        duration,
        degraded,
        degraded_reason: None,
        status: if degraded { "degraded" } else { "ok" }.to_owned(),
        delivery_method: Some("history"),
        fallback_reason: None,
        spool: None,
        count_asr: true,
        count_llm: true,
        context: None,
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

#[cfg(test)]
pub fn insert_history_with_context(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_delivery(
        dir,
        raw,
        final_text,
        duration,
        degraded,
        None,
        if degraded { "degraded" } else { "ok" },
        "history",
        None,
        context,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn insert_history_with_delivery(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
) -> anyhow::Result<()> {
    insert_history_with_delivery_and_spool(
        dir,
        raw,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn insert_history_with_delivery_and_spool(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: None,
        final_text,
        cleanup_status: None,
        duration,
        degraded,
        degraded_reason,
        status: status.to_owned(),
        delivery_method: Some(delivery_method),
        fallback_reason,
        spool,
        count_asr: true,
        count_llm: true,
        context: Some(context),
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_and_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
        dir,
        raw,
        asr_text,
        None,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_rejection(
        dir,
        raw,
        asr_text,
        provider_cleaned_candidate,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        engine,
        cleanup_status,
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_rejection(
    dir: &Path,
    raw: &str,
    asr_text: Option<&str>,
    provider_cleaned_candidate: Option<&str>,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    engine: &str,
    cleanup_status: &str,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
) -> anyhow::Result<()> {
    insert_history_with_status_and_candidate(
        InsertHistory {
            dir,
            raw,
            asr_text,
            engine: Some(engine),
            final_text,
            cleanup_status: Some(cleanup_status),
            duration,
            degraded,
            degraded_reason,
            status: status.to_owned(),
            delivery_method: Some(delivery_method),
            fallback_reason,
            spool,
            count_asr: true,
            count_llm: true,
            context: Some(context),
            cleanup_attempt_kind: None,
            cleanup_transformations_json: None,
            cleanup_final_guard_reason: None,
            cleanup_failure_reason: None,
        },
        provider_cleaned_candidate,
        diagnostic,
        cleanup_rejected_candidate,
        cleanup_guard_reason,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn insert_history_with_delivery_and_spool_and_cleanup(
    dir: &Path,
    raw: &str,
    final_text: &str,
    duration: f64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    delivery_method: &str,
    fallback_reason: Option<&str>,
    context: &crate::context::ContextSnapshot,
    spool: Option<&Path>,
    cleanup_status: &str,
) -> anyhow::Result<()> {
    insert_history_with_asr_and_delivery_and_spool_and_cleanup(
        dir,
        raw,
        None,
        final_text,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        context,
        spool,
        "unknown",
        cleanup_status,
    )
}
#[cfg(test)]
pub fn insert_failed_history(
    dir: &Path,
    raw: &str,
    duration: f64,
    spool: Option<&Path>,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: None,
        final_text: "",
        cleanup_status: None,
        duration,
        degraded: false,
        degraded_reason: Some("asr_failed"),
        status: "failed".to_owned(),
        delivery_method: Some("none"),
        fallback_reason: Some("asr_failed"),
        spool,
        count_asr: false,
        count_llm: false,
        context: None,
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

pub fn insert_failed_history_with_context(
    dir: &Path,
    raw: &str,
    duration: f64,
    spool: Option<&Path>,
    context: &crate::context::ContextSnapshot,
    engine: &str,
) -> anyhow::Result<()> {
    insert_history_with_status(InsertHistory {
        dir,
        raw,
        asr_text: None,
        engine: Some(engine),
        final_text: "",
        cleanup_status: None,
        duration,
        degraded: false,
        degraded_reason: Some("asr_failed"),
        status: "failed".to_owned(),
        delivery_method: Some("none"),
        fallback_reason: Some("asr_failed"),
        spool,
        count_asr: false,
        count_llm: false,
        context: Some(context),
        cleanup_attempt_kind: None,
        cleanup_transformations_json: None,
        cleanup_final_guard_reason: None,
        cleanup_failure_reason: None,
    })
}

/// Insert one failed-ASR History row for a recovery WAV at most once. The
/// acknowledged path is the stable identity retained across a lost caller
/// reply; a repeated owner attempt cannot create a duplicate row.
pub fn insert_failed_history_with_context_once(
    dir: &Path,
    duration: f64,
    spool: &Path,
    context: &crate::context::ContextSnapshot,
    engine: &str,
) -> anyhow::Result<()> {
    let _history_commit = try_spool_io_commit_scope(dir, SpoolIoScope::HistoryDatabase)?;
    let audio_path = spool.to_string_lossy().into_owned();
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let already_recorded: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dictations WHERE raw_audio_path=?",
        [&audio_path],
        |row| row.get(0),
    )?;
    if already_recorded > 0 {
        transaction.commit()?;
        return Ok(());
    }
    insert_history_with_status_and_candidate_on_connection(
        &transaction,
        InsertHistory {
            dir,
            raw: "",
            asr_text: None,
            engine: Some(engine),
            final_text: "",
            cleanup_status: None,
            duration,
            degraded: false,
            degraded_reason: Some("asr_failed"),
            status: "failed".to_owned(),
            delivery_method: Some("none"),
            fallback_reason: Some("asr_failed"),
            spool: Some(spool),
            count_asr: false,
            count_llm: false,
            context: Some(context),
            cleanup_attempt_kind: None,
            cleanup_transformations_json: None,
            cleanup_final_guard_reason: None,
            cleanup_failure_reason: None,
        },
        None,
        None,
        None,
        None,
    )?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn insert_history_with_status(input: InsertHistory) -> anyhow::Result<()> {
    insert_history_with_status_and_candidate(input, None, None, None, None)
}

pub(super) fn insert_history_with_status_and_candidate(
    input: InsertHistory,
    provider_cleaned_candidate: Option<&str>,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
) -> anyhow::Result<()> {
    let _history_commit = try_spool_io_commit_scope(input.dir, SpoolIoScope::HistoryDatabase)?;
    let c = open_history(input.dir)?;
    insert_history_with_status_and_candidate_on_connection(
        &c,
        input,
        provider_cleaned_candidate,
        diagnostic,
        cleanup_rejected_candidate,
        cleanup_guard_reason,
    )
}

pub(super) fn insert_history_with_status_and_candidate_on_connection(
    c: &Connection,
    input: InsertHistory<'_>,
    provider_cleaned_candidate: Option<&str>,
    diagnostic: Option<&crate::delivery_diagnostics::DeliveryDiagnostic>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
) -> anyhow::Result<()> {
    let InsertHistory {
        dir: _,
        raw,
        asr_text,
        engine,
        final_text,
        cleanup_status,
        duration,
        degraded,
        degraded_reason,
        status,
        delivery_method,
        fallback_reason,
        spool,
        count_asr,
        count_llm,
        context,
        cleanup_attempt_kind,
        cleanup_transformations_json,
        cleanup_final_guard_reason,
        cleanup_failure_reason,
    } = input;
    let context_profile_id = context.map(|snapshot| snapshot.profile.id.as_str());
    let context_policy_json = context
        .map(|snapshot| serde_json::to_string(&snapshot.policy.history_metadata()))
        .transpose()?;
    let context_family =
        context.map(|snapshot| crate::context::family_id(snapshot.profile.family).to_owned());
    let context_browser_host =
        context.and_then(|snapshot| snapshot.target_guard.browser_host.clone());
    let context_native_bundle = context.and_then(|snapshot| {
        if snapshot.target_guard.browser_host.is_some() {
            None
        } else {
            snapshot.target_guard.bundle_id.clone()
        }
    });
    let context_mapping_identity =
        context.and_then(|snapshot| snapshot.mapping_identity.as_deref());
    c.execute("INSERT INTO dictations (created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json,context_family,context_browser_host,context_native_bundle,context_mapping_identity,asr_text,provider_cleaned_candidate,cleanup_rejected_candidate,cleanup_guard_reason,delivery_error_code,delivery_user_reason,cleanup_attempt_kind,cleanup_transformations_json,cleanup_final_guard_reason,cleanup_failure_reason) VALUES (datetime('now'),?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", params![duration, raw, final_text, cleanup_status, engine.unwrap_or("unknown"), degraded as i32, degraded_reason, status, spool.map(|p| p.to_string_lossy().to_string()), delivery_method, fallback_reason, context_profile_id, context_policy_json, context_family, context_browser_host, context_native_bundle, context_mapping_identity, asr_text, provider_cleaned_candidate, cleanup_rejected_candidate, cleanup_guard_reason, diagnostic.map(|item| item.code), diagnostic.map(|item| item.user_reason.as_ref()), cleanup_attempt_kind, cleanup_transformations_json, cleanup_final_guard_reason, cleanup_failure_reason])?;
    if count_asr || count_llm {
        c.execute("INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES (date('now'),?,?,?) ON CONFLICT(day) DO UPDATE SET asr_requests=asr_requests+excluded.asr_requests,llm_requests=llm_requests+excluded.llm_requests,audio_seconds=audio_seconds+excluded.audio_seconds", params![count_asr as i64, count_llm as i64, duration])?;
    }
    Ok(())
}
#[cfg(test)]
pub fn get_usage(dir: &Path, quota: QuotaView) -> anyhow::Result<Usage> {
    let c = open_history(dir)?;
    let row = match c.query_row(
        "SELECT asr_requests,llm_requests,audio_seconds FROM usage WHERE day=date('now')",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ) {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => (0, 0, 0.0),
        Err(error) => return Err(error.into()),
    };
    Ok(Usage {
        asr_requests: row.0,
        llm_requests: row.1,
        audio_seconds: row.2,
        quota,
    })
}
pub fn get_history_page(
    dir: &Path,
    limit: i64,
    before_id: Option<i64>,
    query: Option<&str>,
) -> anyhow::Result<HistoryPage> {
    let c = open_history(dir)?;
    let limit = limit.clamp(1, 100);
    let fetch_limit = limit + 1;
    let search_pattern = query
        .filter(|value| !value.is_empty())
        .map(|value| format!("%{}%", escape_history_search(value)));
    let mut s = c.prepare("SELECT id,created_at,COALESCE(raw_text,''),COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,''),COALESCE(duration_secs,0),COALESCE(degraded,0),degraded_reason,COALESCE(status,'ok'),delivery_method,fallback_reason,context_profile_id,raw_audio_path,COALESCE(cleanup_status,'unknown'),(SELECT COUNT(*) FROM dictation_revisions WHERE dictation_id=dictations.id),verbatim_text,COALESCE(verbatim_reviewed,0),asr_text,engine,provider_cleaned_candidate,delivery_error_code,delivery_user_reason,cleanup_rejected_candidate,cleanup_guard_reason,cleanup_attempt_kind,cleanup_transformations_json,cleanup_final_guard_reason,cleanup_failure_reason,context_policy_json FROM dictations WHERE (?1 IS NULL OR id < ?1) AND (?2 IS NULL OR (COALESCE(raw_text,'') LIKE ?2 ESCAPE '\\' OR COALESCE(asr_text,'') LIKE ?2 ESCAPE '\\' OR COALESCE(provider_cleaned_candidate,'') LIKE ?2 ESCAPE '\\' OR COALESCE(cleanup_rejected_candidate,'') LIKE ?2 ESCAPE '\\' OR COALESCE((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),final_text,'') LIKE ?2 ESCAPE '\\')) ORDER BY id DESC LIMIT ?3")?;
    let mut items = s
        .query_map(
            params![before_id, search_pattern.as_deref(), fetch_limit],
            history_row(dir),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = items.len() > limit as usize;
    items.truncate(limit as usize);
    Ok(HistoryPage { items, has_more })
}

pub(super) fn escape_history_search(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(super) fn history_row<'a>(
    dir: &'a Path,
) -> impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<HistoryItem> + 'a {
    move |r| {
        let raw_audio_path: Option<String> = r.get(11)?;
        let status: String = r.get(7)?;
        let raw_text: String = r.get(2)?;
        let engine: Option<String> = r.get(17)?;
        let context_policy_json: Option<String> = r.get(27)?;
        let context_facts = context_policy_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .and_then(|policy| policy.get("preview_context_facts").cloned())
            .and_then(|value| {
                serde_json::from_value::<crate::preview_provenance::PreviewContextFacts>(value).ok()
            })
            .filter(|facts| {
                facts.context_source.len() <= 3
                    && facts
                        .context_source
                        .iter()
                        .enumerate()
                        .all(|(index, source)| !facts.context_source[..index].contains(source))
            });
        let mut preview_receipt =
            crate::preview_provenance::PreviewProvenanceReceipt::for_dictation_history(
                engine.as_deref(),
                &raw_text,
            );
        if let Some(facts) = context_facts {
            let receipt = preview_receipt.get_or_insert_with(|| {
                let (char_count, word_count) =
                    crate::preview_provenance::PreviewProvenanceReceipt::measured_text_counts(
                        &raw_text,
                    );
                crate::preview_provenance::PreviewProvenanceReceipt {
                    source_kind: crate::preview_provenance::PreviewSourceKind::Dictation,
                    char_count: Some(char_count),
                    word_count: Some(word_count),
                    screen_captured: None,
                    context_source: None,
                    provider_label: None,
                }
            });
            receipt.context_source = Some(facts.context_source);
            receipt.screen_captured = facts.screen_captured;
        }
        let has_audio = raw_audio_path
            .as_deref()
            .is_some_and(|path| managed_audio_exists(dir, Path::new(path)));
        let retryable = has_audio && matches!(status.as_str(), "failed" | "degraded");
        let verbatim_text: Option<String> = r.get::<_, Option<String>>(14)?.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(value)
            }
        });
        Ok(HistoryItem {
            id: r.get(0)?,
            created_at: r.get(1)?,
            raw_text,
            final_text: r.get(3)?,
            cleanup_status: r.get(12)?,
            duration: r.get(4)?,
            degraded: r.get::<_, i32>(5)? != 0,
            degraded_reason: r.get(6)?,
            status,
            delivery_method: r.get(8)?,
            fallback_reason: r.get(9)?,
            context_profile_id: r.get(10)?,
            retryable,
            revision_count: r.get::<_, i64>(13)? as usize,
            has_audio,
            verbatim_text,
            verbatim_reviewed: r.get::<_, i32>(15)? != 0,
            asr_text: r.get(16)?,
            engine,
            preview_receipt,
            provider_cleaned_candidate: r.get(18)?,
            delivery_error_code: r.get(19)?,
            delivery_user_reason: r.get(20)?,
            cleanup_rejected_candidate: r.get(21)?,
            cleanup_guard_reason: r.get(22)?,
            cleanup_attempt_kind: r.get(23)?,
            cleanup_transformations: r
                .get::<_, Option<String>>(24)?
                .map(|json| {
                    serde_json::from_str(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            24,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })
                .transpose()?,
            cleanup_final_guard_reason: r.get(25)?,
            cleanup_failure_reason: r.get(26)?,
        })
    }
}

#[cfg(test)]
pub fn get_history(dir: &Path, limit: i64) -> anyhow::Result<Vec<HistoryItem>> {
    Ok(get_history_page(dir, limit, None, None)?.items)
}

pub fn purge_history(dir: &Path, keep_history_days: u64) -> anyhow::Result<usize> {
    purge_history_with_details(dir, keep_history_days).map(|(deleted, _)| deleted)
}

pub(crate) fn purge_history_with_details(
    dir: &Path,
    keep_history_days: u64,
) -> anyhow::Result<(usize, bool)> {
    let _mutation = begin_spool_root_mutation(dir)?;
    // Retention may delete linked recovery or gold audio as well as History
    // rows. Keep the compatibility root-wide guard for the full transitive
    // operation instead of claiming a destination-only History scope.
    let _commit = try_spool_io_commit(dir)?;
    let backup_days = if keep_history_days == 0 {
        HISTORY_MIGRATION_BACKUP_MAX_DAYS
    } else {
        keep_history_days.min(HISTORY_MIGRATION_BACKUP_MAX_DAYS)
    };
    prune_history_migration_backups(dir, Some(backup_days), SystemTime::now())?;
    let now_ms = now_epoch_millis();
    let c = open_history(dir)?;
    let (expired_preference_evidence, expired_preference_decisions) =
        prune_writing_preference_metadata(&c, now_ms)?;
    // Zero is the explicit "keep forever" setting for live history text.
    if keep_history_days == 0 {
        return Ok((
            0,
            expired_preference_evidence + expired_preference_decisions > 0,
        ));
    }
    let cutoff = format!("-{keep_history_days} days");
    let mut statement =
        c.prepare("SELECT raw_audio_path FROM dictations WHERE created_at < datetime('now', ?)")?;
    let paths = statement
        .query_map([cutoff.as_str()], |row| row.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let deleted = c.execute(
        "DELETE FROM dictations WHERE created_at < datetime('now', ?)",
        [cutoff.as_str()],
    )?;
    let deleted_drafts = c.execute(
        "DELETE FROM style_drafts WHERE created_at < datetime('now', ?)",
        [cutoff.as_str()],
    )?;
    c.execute(
        "DELETE FROM usage WHERE day < date('now', ?)",
        [cutoff.as_str()],
    )?;
    drop(statement);
    drop(c);
    for path in paths.into_iter().flatten() {
        remove_spool_artifact_inner(dir, Path::new(&path), true);
    }
    Ok((
        deleted,
        deleted_drafts > 0 || expired_preference_evidence + expired_preference_decisions > 0,
    ))
}

pub(super) const WRITING_PREFERENCE_WINDOW_MS: u64 = 14 * 24 * 60 * 60 * 1_000;
pub(super) const MAX_WRITING_PREFERENCE_EVIDENCE_ROWS: i64 = 256;

pub(super) fn json_enum_string<T: Serialize>(value: &T) -> anyhow::Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("preference enum did not serialize as a string"))
}

pub(super) fn enum_from_database<T: serde::de::DeserializeOwned>(
    value: &str,
    column: usize,
) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

pub(super) fn is_sha256_fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn preference_evidence_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WritingPreferenceEvidenceRecord> {
    let task_id: String = row.get(5)?;
    let dimension: String = row.get(6)?;
    let value: String = row.get(7)?;
    Ok(WritingPreferenceEvidenceRecord {
        source_history_id: row.get(0)?,
        source_revision_id: row.get(1)?,
        mapping_id: row.get(2)?,
        mapping_identity_fingerprint: row.get(3)?,
        mapping_revision: row.get::<_, i64>(4)?.max(0) as u64,
        task_id: enum_from_database(&task_id, 5)?,
        dimension: enum_from_database(&dimension, 6)?,
        value: enum_from_database(&value, 7)?,
        origin_id: row.get(8)?,
        content_fingerprint: row.get(9)?,
        template_family_fingerprint: row.get(10)?,
        occurred_at_ms: row.get::<_, i64>(11)?.max(0) as u64,
        natural_day: row.get(12)?,
        editor_session_id: row.get(13)?,
        generation: row.get::<_, i64>(14)?.max(0) as u64,
        eligible: row.get::<_, i64>(15)? != 0,
    })
}

pub(super) fn preference_decision_from_database(
    value: &str,
) -> anyhow::Result<WritingPreferenceDraftDecision> {
    match value {
        "dismissed" => Ok(WritingPreferenceDraftDecision::Dismissed),
        "approved" => Ok(WritingPreferenceDraftDecision::Approved),
        other => anyhow::bail!("unknown writing preference decision: {other}"),
    }
}

pub(super) fn preference_decision_to_database(
    value: WritingPreferenceDraftDecision,
) -> &'static str {
    match value {
        WritingPreferenceDraftDecision::Dismissed => "dismissed",
        WritingPreferenceDraftDecision::Approved => "approved",
    }
}

pub(super) fn prune_writing_preference_metadata(
    connection: &Connection,
    now_ms: u64,
) -> anyhow::Result<(usize, usize)> {
    let cutoff = now_ms.saturating_sub(WRITING_PREFERENCE_WINDOW_MS) as i64;
    let current = now_ms.min(i64::MAX as u64) as i64;
    let evidence_deleted = connection.execute(
        "DELETE FROM writing_preference_evidence
         WHERE occurred_at_ms < ?1
            OR NOT EXISTS (
                SELECT 1 FROM dictations d
                WHERE d.id = writing_preference_evidence.source_history_id
                  AND CAST(strftime('%s', d.created_at) AS INTEGER) * 1000
                      BETWEEN ?1 AND ?2
            )",
        params![cutoff, current],
    )?;
    let decisions_deleted = connection.execute(
        "DELETE FROM writing_preference_draft_decisions WHERE decided_at_ms < ?1",
        [cutoff],
    )?;
    Ok((evidence_deleted, decisions_deleted))
}

pub(super) fn history_revision_is_latest_on_connection(
    connection: &Connection,
    source_history_id: i64,
    source_revision_id: i64,
    origin_id: &str,
    required_reason: Option<&str>,
) -> anyhow::Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM dictations d
             JOIN dictation_revisions r ON r.dictation_id = d.id
             WHERE d.id = ?1 AND d.transaction_id = ?2 AND r.revision_id = ?3
               AND r.revision_id = (SELECT MAX(latest.revision_id)
                                    FROM dictation_revisions latest
                                    WHERE latest.dictation_id = d.id)
               AND (?4 IS NULL OR r.revision_reason = ?4)
         )",
        params![
            source_history_id,
            origin_id,
            source_revision_id,
            required_reason
        ],
        |row| row.get::<_, bool>(0),
    )?)
}

pub(super) fn history_revision_is_first_edit_of_original_delivery_on_connection(
    connection: &Connection,
    source_history_id: i64,
    source_revision_id: i64,
) -> anyhow::Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM dictation_revisions current
             WHERE current.dictation_id = ?1
               AND current.revision_id = ?2
               AND current.revision_reason = 'manual_edit'
               AND NOT EXISTS (
                   SELECT 1 FROM dictation_revisions prior
                   WHERE prior.dictation_id = current.dictation_id
                     AND prior.revision_id < current.revision_id
               )
         )",
        params![source_history_id, source_revision_id],
        |row| row.get::<_, bool>(0),
    )?)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn history_preference_source_matches_on_connection(
    connection: &Connection,
    source_history_id: i64,
    source_revision_id: i64,
    origin_id: &str,
    mapping_id: &str,
    mapping_identity_fingerprint: &str,
    mapping_revision: u64,
    task_id: crate::context::WritingTaskId,
) -> anyhow::Result<bool> {
    let source = connection
        .query_row(
            "SELECT d.transaction_id, d.context_profile_id, d.context_mapping_identity,
                    d.context_policy_json,
                    r.revision_reason,
                    r.revision_id = (SELECT MAX(latest.revision_id)
                                    FROM dictation_revisions latest
                                    WHERE latest.dictation_id = d.id)
             FROM dictations d
             JOIN dictation_revisions r ON r.dictation_id = d.id
             WHERE d.id = ?1 AND r.revision_id = ?2",
            params![source_history_id, source_revision_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, bool>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((
        stored_origin,
        stored_profile,
        stored_mapping_identity,
        policy_json,
        reason,
        is_latest,
    )) = source
    else {
        return Ok(false);
    };
    if stored_origin.as_deref() != Some(origin_id)
        || stored_profile.as_deref() != Some(format!("user.{mapping_id}").as_str())
        || stored_mapping_identity.as_deref() != Some(mapping_identity_fingerprint)
        || reason != "manual_edit"
        || !is_latest
    {
        return Ok(false);
    }
    let Some(policy_json) = policy_json else {
        return Ok(false);
    };
    let policy: crate::context::ContextPolicy = serde_json::from_str(&policy_json)?;
    let Some(provenance) = policy.history_preference_provenance.as_ref() else {
        return Ok(false);
    };
    Ok(provenance.version == 1
        && provenance.task_id == Some(task_id)
        && provenance.mapping_preference_revision == Some(mapping_revision))
}

/// Compare edited text against the transaction's original material and prior
/// revisions without returning any of that text to the caller.
pub(crate) fn history_text_matches_existing_source(
    dir: &Path,
    source_history_id: i64,
    source_revision_id: i64,
    edited_text: &str,
    dimension: crate::context::WritingPreferenceDimension,
    value: crate::context::WritingPreferenceValue,
) -> anyhow::Result<bool> {
    let connection = open_history(dir)?;
    let normalized = edited_text.trim();
    let mut statement = connection.prepare(
        "SELECT source_text FROM (
             SELECT d.raw_text AS source_text FROM dictations d WHERE d.id = ?1
             UNION ALL SELECT d.asr_text FROM dictations d WHERE d.id = ?1
             UNION ALL SELECT d.provider_cleaned_candidate FROM dictations d WHERE d.id = ?1
             UNION ALL SELECT d.cleanup_rejected_candidate FROM dictations d WHERE d.id = ?1
             UNION ALL SELECT d.final_text FROM dictations d WHERE d.id = ?1
             UNION ALL SELECT r.final_text FROM dictation_revisions r
                 WHERE r.dictation_id = ?1 AND r.revision_id <> ?2
         ) WHERE source_text IS NOT NULL AND length(CAST(source_text AS BLOB)) <= ?3",
    )?;
    let mut rows = statement.query(params![
        source_history_id,
        source_revision_id,
        crate::preference_shadow::MAX_HISTORY_PREFERENCE_TEXT_BYTES as i64
    ])?;
    while let Some(row) = rows.next()? {
        let source = row.get::<_, String>(0)?;
        if source.trim() == normalized {
            return Ok(true);
        }
        if matches!(
            crate::preference_shadow::source_slot_match(&source, normalized, dimension, value,),
            crate::preference_shadow::SourceSlotAlignment::AlreadyPresent
                | crate::preference_shadow::SourceSlotAlignment::Ambiguous
        ) {
            // A prior version may have had this feature in the same slot even
            // if the rest of the text changed. Ambiguous alignment is also a
            // reason to abstain; this function returns only a compare result.
            return Ok(true);
        }
    }
    Ok(false)
}

/// Replace one transaction's current weak edit evidence. `None` turns any
/// previous eligible row into a hash-free origin watermark so revisions,
/// reversals, or retries cannot count the same dictation twice.
pub(crate) fn replace_writing_preference_evidence(
    dir: &Path,
    source_history_id: i64,
    source_revision_id: i64,
    origin_id: &str,
    input: Option<&WritingPreferenceEvidenceInput>,
) -> anyhow::Result<WritingPreferenceEvidenceWrite> {
    let now_ms = now_epoch_millis();
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    prune_writing_preference_metadata(&transaction, now_ms)?;
    if source_history_id <= 0
        || source_revision_id <= 0
        || origin_id.trim().is_empty()
        || origin_id.len() > 256
        || now_ms > i64::MAX as u64
    {
        transaction.commit()?;
        return Ok(WritingPreferenceEvidenceWrite::RejectedSource);
    }

    let Some(input) = input else {
        if !history_revision_is_latest_on_connection(
            &transaction,
            source_history_id,
            source_revision_id,
            origin_id,
            Some("manual_edit"),
        )? {
            transaction.commit()?;
            return Ok(WritingPreferenceEvidenceWrite::RejectedSource);
        }
        transaction.execute(
            "UPDATE writing_preference_evidence
             SET source_revision_id = ?1, content_fingerprint = '',
                 template_family_fingerprint = NULL, editor_session_id = '',
                 occurred_at_ms = ?2,
                 natural_day = '', eligible = 0
             WHERE source_history_id = ?3 AND origin_id = ?4
               AND source_revision_id <= ?1",
            params![
                source_revision_id,
                now_ms as i64,
                source_history_id,
                origin_id
            ],
        )?;
        let changed = transaction.changes() > 0;
        transaction.commit()?;
        return Ok(if changed {
            WritingPreferenceEvidenceWrite::Replaced
        } else {
            WritingPreferenceEvidenceWrite::Duplicate
        });
    };

    let source_day_is_current = transaction
        .query_row(
            "SELECT COALESCE(
             CAST(strftime('%s', d.created_at) AS INTEGER) * 1000
                 BETWEEN ?1 AND ?2
                 AND date(d.created_at, 'localtime') = ?3,
             0
         )
         FROM dictations d WHERE d.id = ?4",
            params![
                now_ms.saturating_sub(WRITING_PREFERENCE_WINDOW_MS) as i64,
                now_ms as i64,
                input.natural_day,
                source_history_id
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some_and(|matches| matches != 0);
    let valid_input = input.source_history_id == source_history_id
        && input.source_revision_id == source_revision_id
        && input.origin_id == origin_id
        && !input.mapping_id.trim().is_empty()
        && input.mapping_id.len() <= 256
        && !input.mapping_identity_fingerprint.trim().is_empty()
        && input.mapping_identity_fingerprint.len() <= 256
        && input
            .mapping_identity_fingerprint
            .starts_with("mapping:v1:")
        && input.mapping_revision <= i64::MAX as u64
        && input.occurred_at_ms <= i64::MAX as u64
        && input.source_history_id > 0
        && input.source_revision_id > 0
        && !input.origin_id.trim().is_empty()
        && input.origin_id.len() <= 256
        && is_sha256_fingerprint(&input.content_fingerprint)
        && input
            .template_family_fingerprint
            .as_deref()
            .is_none_or(is_sha256_fingerprint)
        && !input.editor_session_id.trim().is_empty()
        && input.editor_session_id.len() <= 128
        && input.natural_day.len() == 10
        && input.occurred_at_ms <= now_epoch_millis().saturating_add(60_000)
        && input.occurred_at_ms >= now_ms.saturating_sub(WRITING_PREFERENCE_WINDOW_MS)
        && source_day_is_current
        && history_revision_is_first_edit_of_original_delivery_on_connection(
            &transaction,
            source_history_id,
            source_revision_id,
        )?
        && history_preference_source_matches_on_connection(
            &transaction,
            source_history_id,
            source_revision_id,
            origin_id,
            &input.mapping_id,
            &input.mapping_identity_fingerprint,
            input.mapping_revision,
            input.task_id,
        )?;
    if !valid_input {
        // A late/non-latest callback cannot erase a newer origin row. If this
        // exact latest edit is current but its mapping proof is now invalid,
        // clear the old positive proof while retaining only its origin key.
        if history_revision_is_latest_on_connection(
            &transaction,
            source_history_id,
            source_revision_id,
            origin_id,
            Some("manual_edit"),
        )? {
            transaction.execute(
                "UPDATE writing_preference_evidence
                 SET source_revision_id = ?1, content_fingerprint = '',
                     template_family_fingerprint = NULL, editor_session_id = '',
                     occurred_at_ms = ?2,
                     natural_day = '', eligible = 0
                 WHERE source_history_id = ?3 AND origin_id = ?4
                   AND source_revision_id <= ?1",
                params![
                    source_revision_id,
                    now_ms as i64,
                    source_history_id,
                    origin_id
                ],
            )?;
        }
        transaction.commit()?;
        return Ok(WritingPreferenceEvidenceWrite::RejectedSource);
    }

    let task_id = json_enum_string(&input.task_id)?;
    let dimension = json_enum_string(&input.dimension)?;
    let value = json_enum_string(&input.value)?;
    let existing = transaction
        .query_row(
            "SELECT mapping_revision, task_id, dimension, value, generation,
                    source_revision_id, eligible
             FROM writing_preference_evidence
             WHERE mapping_id = ?1 AND origin_id = ?2",
            params![input.mapping_id, origin_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?.max(0) as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?.max(0) as u64,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)? != 0,
                ))
            },
        )
        .optional()?;
    if existing.as_ref().is_some_and(|row| {
        row.0 != input.mapping_revision
            || row.1 != task_id
            || row.2 != dimension
            || row.3 != value
            || !row.6
    }) {
        transaction.commit()?;
        return Ok(WritingPreferenceEvidenceWrite::StaleOrigin);
    }
    let generation = if let Some(row) = existing.as_ref() {
        row.4
    } else {
        let decision = transaction
            .query_row(
                "SELECT generation, decided_at_ms, decision
                 FROM writing_preference_draft_decisions
                 WHERE mapping_id = ?1 AND task_id = ?2
                   AND dimension = ?3 AND value = ?4
                 ORDER BY decided_at_ms DESC, generation DESC LIMIT 1",
                params![input.mapping_id, task_id, dimension, value],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?.max(0) as u64,
                        row.get::<_, i64>(1)?.max(0) as u64,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((generation, decided_at_ms, decision)) = decision {
            if input.occurred_at_ms <= decided_at_ms
                || preference_decision_from_database(&decision)?
                    == WritingPreferenceDraftDecision::Approved
            {
                transaction.commit()?;
                return Ok(WritingPreferenceEvidenceWrite::StaleOrigin);
            }
            generation.saturating_add(1).max(1)
        } else {
            transaction
                .query_row(
                    "SELECT COALESCE(MAX(generation), 0)
                     FROM writing_preference_evidence
                     WHERE mapping_id = ?1 AND mapping_identity_fingerprint = ?2
                       AND task_id = ?3 AND dimension = ?4 AND value = ?5",
                    params![
                        input.mapping_id,
                        input.mapping_identity_fingerprint,
                        task_id,
                        dimension,
                        value
                    ],
                    |row| row.get::<_, i64>(0),
                )?
                .max(1) as u64
        }
    };

    if existing.is_none() {
        let rows: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM writing_preference_evidence",
            [],
            |row| row.get(0),
        )?;
        if rows >= MAX_WRITING_PREFERENCE_EVIDENCE_ROWS {
            transaction.commit()?;
            return Ok(WritingPreferenceEvidenceWrite::AtCapacity);
        }
    }
    let natural_day = input.natural_day.clone();
    let content_fingerprint = input.content_fingerprint.to_ascii_lowercase();
    let template_family_fingerprint = input
        .template_family_fingerprint
        .as_deref()
        .map(str::to_ascii_lowercase);
    let write = if existing.is_some() {
        transaction.execute(
            "UPDATE writing_preference_evidence
             SET source_revision_id = ?1, content_fingerprint = ?2,
                 template_family_fingerprint = ?3, occurred_at_ms = ?4,
                 natural_day = ?5, editor_session_id = ?6, eligible = 1
             WHERE mapping_id = ?7 AND origin_id = ?8 AND source_history_id = ?9
               AND source_revision_id <= ?1 AND eligible = 1",
            params![
                source_revision_id,
                content_fingerprint,
                template_family_fingerprint,
                input.occurred_at_ms as i64,
                natural_day,
                input.editor_session_id,
                input.mapping_id,
                origin_id,
                source_history_id
            ],
        )?;
        WritingPreferenceEvidenceWrite::Replaced
    } else {
        transaction.execute(
            "INSERT INTO writing_preference_evidence (
                 source_history_id, source_revision_id, mapping_id,
                 mapping_identity_fingerprint, mapping_revision,
                 task_id, dimension, value, origin_id, content_fingerprint,
                 template_family_fingerprint, occurred_at_ms, natural_day,
                 editor_session_id, generation, eligible
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1)",
            params![
                source_history_id,
                source_revision_id,
                input.mapping_id,
                input.mapping_identity_fingerprint,
                input.mapping_revision as i64,
                task_id,
                dimension,
                value,
                origin_id,
                content_fingerprint,
                template_family_fingerprint,
                input.occurred_at_ms as i64,
                natural_day,
                input.editor_session_id,
                generation as i64
            ],
        )?;
        WritingPreferenceEvidenceWrite::Inserted
    };
    transaction.commit()?;
    Ok(write)
}

pub(super) fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub(crate) fn list_writing_preference_evidence(
    dir: &Path,
    now_ms: u64,
) -> anyhow::Result<Vec<WritingPreferenceEvidenceRecord>> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    prune_writing_preference_metadata(&transaction, now_ms)?;
    transaction.execute(
        "UPDATE writing_preference_evidence AS e
         SET eligible = 0, content_fingerprint = '',
             template_family_fingerprint = NULL, editor_session_id = ''
         WHERE e.eligible = 1 AND NOT EXISTS (
             SELECT 1 FROM dictations d
             JOIN dictation_revisions r ON r.dictation_id = d.id
             WHERE d.id = e.source_history_id
               AND d.transaction_id = e.origin_id
               AND r.revision_id = e.source_revision_id
               AND r.revision_reason = 'manual_edit'
               AND r.revision_id = (SELECT MAX(latest.revision_id)
                                    FROM dictation_revisions latest
                                    WHERE latest.dictation_id = d.id)
         )",
        [],
    )?;
    transaction.commit()?;
    let connection = open_history(dir)?;
    let cutoff = now_ms.saturating_sub(WRITING_PREFERENCE_WINDOW_MS) as i64;
    let current = now_ms.min(i64::MAX as u64) as i64;
    let mut statement = connection.prepare(
        "SELECT source_history_id, source_revision_id, mapping_id,
                mapping_identity_fingerprint, mapping_revision,
                task_id, dimension, value, origin_id, content_fingerprint,
                template_family_fingerprint, occurred_at_ms, natural_day,
                editor_session_id, generation, eligible
         FROM writing_preference_evidence
         WHERE occurred_at_ms >= ?1 AND eligible = 1
           AND EXISTS (
               SELECT 1 FROM dictations d
               WHERE d.id = writing_preference_evidence.source_history_id
                 AND CAST(strftime('%s', d.created_at) AS INTEGER) * 1000
                     BETWEEN ?1 AND ?2
           )
         ORDER BY mapping_id, mapping_revision, task_id, dimension, value, generation,
                  occurred_at_ms, origin_id",
    )?;
    let rows = statement.query_map(params![cutoff, current], preference_evidence_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub(crate) fn writing_preference_draft_decisions(
    dir: &Path,
    now_ms: u64,
) -> anyhow::Result<Vec<WritingPreferenceDraftDecisionRecord>> {
    let connection = open_history(dir)?;
    prune_writing_preference_metadata(&connection, now_ms)?;
    let mut statement = connection.prepare(
        "SELECT candidate_id, mapping_id, mapping_revision, task_id, dimension, value,
                generation, decision, decided_at_ms
         FROM writing_preference_draft_decisions
         ORDER BY mapping_id, mapping_revision, task_id, dimension, value, generation",
    )?;
    let rows = statement.query_map([], |row| {
        let task_id: String = row.get(3)?;
        let dimension: String = row.get(4)?;
        let value: String = row.get(5)?;
        let decision: String = row.get(7)?;
        Ok(WritingPreferenceDraftDecisionRecord {
            candidate_id: row.get(0)?,
            mapping_id: row.get(1)?,
            mapping_revision: row.get::<_, i64>(2)?.max(0) as u64,
            task_id: enum_from_database(&task_id, 3)?,
            dimension: enum_from_database(&dimension, 4)?,
            value: enum_from_database(&value, 5)?,
            generation: row.get::<_, i64>(6)?.max(0) as u64,
            decision: match decision.as_str() {
                "dismissed" => WritingPreferenceDraftDecision::Dismissed,
                "approved" => WritingPreferenceDraftDecision::Approved,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("unknown writing preference decision: {other}"),
                        )),
                    ));
                }
            },
            decided_at_ms: row.get::<_, i64>(8)?.max(0) as u64,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub(crate) fn record_writing_preference_draft_decision(
    dir: &Path,
    decision: &WritingPreferenceDraftDecisionRecord,
) -> anyhow::Result<bool> {
    if decision.candidate_id.trim().is_empty()
        || decision.candidate_id.len() > 128
        || decision.mapping_id.trim().is_empty()
        || decision.mapping_revision > i64::MAX as u64
        || decision.generation > i64::MAX as u64
        || decision.decided_at_ms > i64::MAX as u64
    {
        anyhow::bail!("writing preference draft decision is invalid");
    }
    let task_id = json_enum_string(&decision.task_id)?;
    let dimension = json_enum_string(&decision.dimension)?;
    let value = json_enum_string(&decision.value)?;
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    prune_writing_preference_metadata(&transaction, decision.decided_at_ms)?;
    transaction.execute(
        "INSERT OR IGNORE INTO writing_preference_draft_decisions (
             candidate_id, mapping_id, mapping_revision, task_id, dimension, value,
             generation, decision, decided_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            decision.candidate_id,
            decision.mapping_id,
            decision.mapping_revision as i64,
            task_id,
            dimension,
            value,
            decision.generation as i64,
            preference_decision_to_database(decision.decision),
            decision.decided_at_ms as i64
        ],
    )?;
    let inserted = transaction.changes() > 0;
    transaction.commit()?;
    Ok(inserted)
}

pub(crate) fn clear_writing_preference_metadata(dir: &Path) -> anyhow::Result<()> {
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "DELETE FROM writing_preference_evidence;
         DELETE FROM writing_preference_draft_decisions;",
    )?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn clear_writing_preference_metadata_for_mapping(
    dir: &Path,
    mapping_id: &str,
) -> anyhow::Result<()> {
    if mapping_id.trim().is_empty() || mapping_id.len() > 128 {
        anyhow::bail!("writing preference mapping ID is invalid");
    }
    let mut connection = open_history(dir)?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    transaction.execute(
        "DELETE FROM writing_preference_evidence WHERE mapping_id = ?1",
        [mapping_id],
    )?;
    transaction.execute(
        "DELETE FROM writing_preference_draft_decisions WHERE mapping_id = ?1",
        [mapping_id],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
pub fn clear_all_data(dir: &Path) -> anyhow::Result<()> {
    let guard = begin_clear_all_data(dir)?;
    clear_all_data_under_guard(dir, &guard)
}

/// Clear data after admission has already been acquired. This runs on the
/// retained spool I/O owner in production; no registry guard crosses the
/// filesystem or database work.
#[cfg(test)]
pub(crate) fn clear_all_data_under_guard(
    dir: &Path,
    guard: &SpoolClearGuard,
) -> anyhow::Result<()> {
    clear_all_data_under_guard_with_options(dir, guard, true)
}

pub(crate) fn clear_all_data_under_guard_with_options(
    dir: &Path,
    guard: &SpoolClearGuard,
    include_learned_rules: bool,
) -> anyhow::Result<()> {
    if guard.data_root != normalized_data_root(dir) {
        anyhow::bail!("data clear lease does not own this directory");
    }
    let _commit = try_spool_io_commit(dir)?;
    let c = open_history(dir)?;
    if include_learned_rules {
        c.execute_batch("DELETE FROM dictations; DELETE FROM learn_observations; DELETE FROM usage; DELETE FROM learn_pairs; DELETE FROM style_drafts; DELETE FROM learned_term_usage; DELETE FROM writing_preference_evidence; DELETE FROM writing_preference_draft_decisions;")?;
    } else {
        c.execute_batch("DELETE FROM dictations; DELETE FROM learn_observations; DELETE FROM usage; DELETE FROM style_drafts; DELETE FROM learned_term_usage; DELETE FROM writing_preference_evidence; DELETE FROM writing_preference_draft_decisions; DELETE FROM learn_pairs WHERE promoted = 0 AND COALESCE(manual_scoped, 0) = 0 AND COALESCE(dictionary_pending_action, 'none') = 'none' AND ignored = 0 AND tombstoned_at IS NULL;")?;
    }
    let vacuum_result = c.execute_batch("VACUUM");
    drop(c);
    // Run after open_history: clearing a legacy database can itself create a
    // rollback copy, which must not retain the very text the user cleared.
    prune_history_migration_backups(dir, None, SystemTime::now())?;
    let spool = dir.join("spool");
    if spool.exists() {
        fs::remove_dir_all(&spool)?;
    }
    ensure_private_dir(&spool)?;
    let gold = gold_root(dir);
    if gold.exists() {
        fs::remove_dir_all(&gold)?;
    }
    let pending = pending_history_root(dir);
    if pending.exists() {
        fs::remove_dir_all(&pending)?;
    }
    if let Err(error) = vacuum_result {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeCredentialBackend {
        values: RefCell<std::collections::BTreeMap<String, String>>,
        read_scripts: RefCell<
            std::collections::BTreeMap<
                String,
                std::collections::VecDeque<crate::keychain::ApiKeyState>,
            >,
        >,
        write_errors: RefCell<std::collections::BTreeMap<String, String>>,
        writes: RefCell<Vec<(String, String)>>,
    }

    impl FakeCredentialBackend {
        fn seed(&self, slot: &str, key: &str) {
            self.values
                .borrow_mut()
                .insert(slot.to_owned(), key.to_owned());
        }

        fn fail_write(&self, slot: &str, error: &str) {
            self.write_errors
                .borrow_mut()
                .insert(slot.to_owned(), error.to_owned());
        }

        fn script_reads(
            &self,
            slot: &str,
            states: impl IntoIterator<Item = crate::keychain::ApiKeyState>,
        ) {
            self.read_scripts
                .borrow_mut()
                .insert(slot.to_owned(), states.into_iter().collect());
        }
    }

    impl CredentialBackend for FakeCredentialBackend {
        fn read(&self, slot: &str) -> crate::keychain::ApiKeyState {
            if let Some(state) = self
                .read_scripts
                .borrow_mut()
                .get_mut(slot)
                .and_then(std::collections::VecDeque::pop_front)
            {
                return state;
            }
            let values = self.values.borrow();
            let configured = |slot: &str| {
                values
                    .get(slot)
                    .filter(|key| !key.trim().is_empty())
                    .cloned()
                    .map(crate::keychain::ApiKeyState::Configured)
            };
            if let Some(state) = configured(slot) {
                return state;
            }
            if slot == "provider_custom" {
                if let Some(state) =
                    configured("asr_api_key").or_else(|| configured("cleanup_api_key"))
                {
                    return state;
                }
            }
            crate::keychain::ApiKeyState::Missing
        }

        fn write(&self, slot: &str, key: &str) -> Result<(), String> {
            self.writes
                .borrow_mut()
                .push((slot.to_owned(), key.to_owned()));
            if let Some(error) = self.write_errors.borrow().get(slot) {
                return Err(error.clone());
            }
            if key.is_empty() {
                self.values.borrow_mut().remove(slot);
            } else {
                self.values
                    .borrow_mut()
                    .insert(slot.to_owned(), key.to_owned());
            }
            Ok(())
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-store-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn transfer_intent(
        dir: &Path,
        before_surface: &str,
        after_surface: &str,
        scope: LearnPairScope,
        match_policy: LearnPairMatchPolicy,
        manual_scoped: bool,
        replace_conflicts: bool,
    ) -> LearnPairTransferIntent {
        let pair_key =
            scoped_word_rule_key(before_surface, after_surface, &scope, match_policy).into_string();
        let rows = list_learn_pairs(dir).unwrap();
        let expected_rows = learn_pair_transfer_conflict_snapshot(
            &rows,
            &pair_key,
            before_surface,
            after_surface,
            &scope,
            match_policy,
        )
        .unwrap();
        LearnPairTransferIntent {
            pair_key,
            before_surface: before_surface.into(),
            after_surface: after_surface.into(),
            scope,
            match_policy,
            manual_scoped,
            expected_rows,
            replace_conflicts,
        }
    }

    #[cfg(unix)]
    #[test]
    fn export_preserves_selected_directory_permissions_and_restricts_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("export-mode");
        fs::create_dir_all(&root).unwrap();
        for mode in [0o755, 0o750] {
            fs::set_permissions(&root, fs::Permissions::from_mode(mode)).unwrap();
            let output = root.join(format!("export-{mode:o}.json"));
            write_export_file(&output, "private export").unwrap();
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read_to_string(&output).unwrap(), "private export");
            fs::set_permissions(&output, fs::Permissions::from_mode(0o644)).unwrap();
            write_export_file(&output, "replacement").unwrap();
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read_to_string(&output).unwrap(), "replacement");
            assert!(write_export_file(Path::new("relative-export.json"), "no").is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_settings_are_backed_up_before_first_persist_and_backup_is_stable() {
        let dir = temp_dir("malformed-settings");
        fs::create_dir_all(&dir).unwrap();
        let original = b"{ definitely not settings";
        fs::write(dir.join("settings.json"), original).unwrap();
        let (_loaded, needs_persist) =
            load_settings_with_backend(&dir, &FakeCredentialBackend::default());
        assert!(!needs_persist);
        let settings = Settings::default();
        persist_settings_without_credential_changes(&dir, &settings).unwrap();
        let backup = dir.join("settings.json.malformed.bak");
        assert_eq!(fs::read(&backup).unwrap(), original);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        persist_settings_without_credential_changes(&dir, &settings).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();

        let reuse_dir = temp_dir("malformed-settings-backup-reuse");
        fs::create_dir_all(&reuse_dir).unwrap();
        let original = b"{ still malformed settings";
        fs::write(reuse_dir.join("settings.json"), original).unwrap();
        let backup = reuse_dir.join("settings.json.malformed.bak");
        fs::write(&backup, original).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600)).unwrap();
        let backend = FakeCredentialBackend::default();
        let (_loaded, needs_persist) = load_settings_with_backend(&reuse_dir, &backend);
        assert!(!needs_persist);
        persist_settings_with_backend(&reuse_dir, &settings, &backend).unwrap();
        assert_eq!(fs::read(&backup).unwrap(), original);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(reuse_dir).unwrap();

        let conflict_dir = temp_dir("malformed-settings-conflict");
        fs::create_dir_all(&conflict_dir).unwrap();
        let original = b"{ bad settings }";
        fs::write(conflict_dir.join("settings.json"), original).unwrap();
        fs::write(
            conflict_dir.join("settings.json.malformed.bak"),
            b"different existing backup",
        )
        .unwrap();
        assert!(persist_settings_without_credential_changes(&conflict_dir, &settings).is_err());
        assert_eq!(
            fs::read(conflict_dir.join("settings.json")).unwrap(),
            original
        );
        fs::remove_dir_all(conflict_dir).unwrap();
    }

    #[test]
    fn pending_history_scope_covers_quota_tree_but_is_disjoint_from_capture_and_history() {
        let root = temp_dir("pending-scope");
        let pending = try_spool_io_commit_scope(&root, SpoolIoScope::PendingHistoryIntent)
            .expect("the pending directory tree is admitted");
        assert!(
            try_spool_io_commit_scope(&root, SpoolIoScope::PendingHistoryIntent).is_err(),
            "distinct transaction identities still share the quota/enumeration scope"
        );
        let history = try_spool_io_commit_scope(&root, SpoolIoScope::HistoryDatabase)
            .expect("pending metadata does not lock the History database");
        let capture = try_spool_io_commit_scope(&root, SpoolIoScope::RecoverySpool)
            .expect("pending metadata does not lock active audio capture");
        assert!(
            try_spool_io_commit(&root).is_err(),
            "the compatibility root-wide permit still conflicts with every typed scope"
        );
        drop(capture);
        drop(history);
        drop(pending);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn live_spool_session_scopes_are_disjoint_and_still_conflict_with_root_maintenance() {
        let root = temp_dir("live-session-scopes");
        let (first_dir, first_lease) = begin_audio_spool_session(&root, "scope-first")
            .expect("first live session reserves its derived path");
        let (second_dir, second_lease) = begin_audio_spool_session(&root, "scope-second")
            .expect("second live session reserves a disjoint derived path");
        initialize_audio_spool_session(&first_lease).unwrap();
        initialize_audio_spool_session(&second_lease).unwrap();

        let first = try_spool_io_commit_audio_session(&first_lease)
            .expect("first live session owns its own path scope");
        let second = try_spool_io_commit_audio_session(&second_lease)
            .expect("another live session may use a disjoint path scope");
        assert!(try_spool_io_commit_audio_session(&first_lease).is_err());
        assert!(try_spool_io_commit_scope(&root, SpoolIoScope::RecoverySpool).is_err());
        assert!(try_spool_io_commit(&root).is_err());

        drop(second);
        drop(first);
        drop(second_lease);
        drop(first_lease);
        let _ = fs::remove_dir_all(root);
        assert!(!first_dir.join("manifest.json").exists());
        assert!(!second_dir.join("manifest.json").exists());
    }

    #[test]
    fn deferred_session_cleanup_keeps_exact_owner_without_restoring_write_access() {
        let root = temp_dir("deferred-session-cleanup-scope");
        let (session_dir, lease) = begin_audio_spool_session(&root, "cleanup-owned-session")
            .expect("reserve the live session");
        initialize_audio_spool_session(&lease).unwrap();

        assert!(defer_spool_artifact_removal_until_release(&session_dir));
        assert!(lease.deletion_requested());
        assert!(try_spool_io_commit_audio_session(&lease).is_err());
        let cleanup = try_spool_io_commit_audio_session_cleanup(&lease)
            .expect("the exact current lease keeps cleanup permission");
        drop(cleanup);
        delete_audio_spool_session(&lease).expect("cleanup deletes its exact session tree");
        assert!(!session_dir.exists());
        assert!(try_spool_io_commit_audio_session(&lease).is_err());
        assert!(reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).is_err());

        drop(lease);
        let (reused_dir, reused_lease) = begin_audio_spool_session(&root, "cleanup-owned-session")
            .expect("reuse waits for the cleanup owner to release its lease");
        assert_eq!(reused_dir, session_dir);
        initialize_audio_spool_session(&reused_lease).unwrap();
        drop(reused_lease);
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn spool_quota_scan_tolerates_sibling_temp_rename_and_session_cleanup() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        let root = temp_dir("spool-quota-disappearing-entry");
        let (first_dir, first_lease) =
            begin_audio_spool_session(&root, "quota-first").expect("first session reserves a path");
        let (_second_dir, second_lease) = begin_audio_spool_session(&root, "quota-second")
            .expect("second session reserves a disjoint path");
        initialize_audio_spool_session(&first_lease).unwrap();
        initialize_audio_spool_session(&second_lease).unwrap();
        drop(first_lease);
        drop(second_lease);
        let quota_anchor = root.join("spool/quota-anchor.bin");
        File::create(&quota_anchor)
            .unwrap()
            .set_len(MAX_SPOOL_BYTES - 24 * 1024)
            .unwrap();

        let first_relative = Path::new("quota-first/rename-race.wav");
        let first_target = root.join("spool").join(first_relative);
        let first_temp = first_target.parent().unwrap().join(format!(
            ".{}.tmp-{}",
            first_target.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        let (rename_entered_tx, rename_entered_rx) = mpsc::channel();
        let (rename_release_tx, rename_release_rx) = mpsc::channel();
        pause_next_atomic_rename_for_test(&first_temp, rename_entered_tx, rename_release_rx);
        let first_root = root.clone();
        let first_writer = std::thread::spawn(move || {
            write_spool_file(&first_root, first_relative, &vec![0x25; 16 * 1024])
        });
        rename_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("A owns its session scope and has a synced temporary file");

        let (scan_entered_tx, scan_entered_rx) = mpsc::channel();
        let (scan_release_tx, scan_release_rx) = mpsc::channel();
        pause_next_spool_size_walk_for_test(&first_temp, scan_entered_tx, scan_release_rx);
        let second_root = root.clone();
        let second_writer = std::thread::spawn(move || {
            write_spool_file(
                &second_root,
                Path::new("quota-second/parallel.wav"),
                &vec![0x50; 16 * 1024],
            )
        });
        scan_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("B's real quota walk reaches A's temporary directory entry");

        rename_release_tx
            .send(())
            .expect("allow A's actual atomic rename");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !first_target.exists() {
            assert!(Instant::now() < deadline, "A's final file rename completes");
            std::thread::sleep(Duration::from_millis(1));
        }
        first_writer
            .join()
            .unwrap()
            .expect("A releases its reservation after the actual write returns");
        assert_eq!(
            recovery_spool_reserved_bytes_for_test(&root),
            0,
            "A's reservation can drop while B still owns the recursive scan"
        );
        scan_release_tx
            .send(())
            .expect("resume B's quota metadata lookup after A's temp entry vanished");
        let second_error = second_writer
            .join()
            .unwrap()
            .expect_err("B retains A's start-of-scan charge and does not over-admit quota");
        assert!(second_error
            .to_string()
            .contains("audio spool quota exceeded"));
        assert_eq!(fs::read(&first_target).unwrap().len(), 16 * 1024);
        let second_target = root.join("spool/quota-second/parallel.wav");
        assert!(!second_target.exists());
        fs::remove_file(&quota_anchor).unwrap();

        let (delete_scan_entered_tx, delete_scan_entered_rx) = mpsc::channel();
        let (delete_scan_release_tx, delete_scan_release_rx) = mpsc::channel();
        pause_next_spool_size_walk_for_test(
            &first_target,
            delete_scan_entered_tx,
            delete_scan_release_rx,
        );
        let second_root = root.clone();
        let second_writer = std::thread::spawn(move || {
            write_spool_file(
                &second_root,
                Path::new("quota-second/after-delete.wav"),
                &vec![0x75; 16 * 1024],
            )
        });
        delete_scan_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("B's next actual quota walk pauses on A's existing file");
        let first_root = root.clone();
        let first_dir_for_delete = first_dir.clone();
        let deleter = std::thread::spawn(move || {
            remove_spool_artifact(&first_root, &first_dir_for_delete);
        });
        deleter.join().unwrap();
        assert!(!first_dir.exists(), "A's sibling session cleanup completes");
        delete_scan_release_tx
            .send(())
            .expect("resume B's quota scan after A's session disappeared");
        second_writer
            .join()
            .unwrap()
            .expect("B's capture artifact is not lost to A's scoped cleanup");
        assert_eq!(
            fs::read(root.join("spool/quota-second/after-delete.wav"))
                .unwrap()
                .len(),
            16 * 1024
        );

        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn quota_ledger_fences_same_root_scans_and_clears_markers_after_faults() {
        use std::sync::mpsc;
        use std::time::Duration;

        let root = temp_dir("spool-quota-ledger-fence");
        let spool_root = root.join("spool");
        fs::create_dir_all(&spool_root).unwrap();
        let scan_target = spool_root.join("scan-target.bin");
        fs::write(&scan_target, b"counted before reservation").unwrap();

        let (scan_entered_tx, scan_entered_rx) = mpsc::channel();
        let (scan_release_tx, scan_release_rx) = mpsc::channel();
        pause_next_spool_size_walk_for_test(&scan_target, scan_entered_tx, scan_release_rx);
        let first_root = spool_root.clone();
        let (first_result_tx, first_result_rx) = mpsc::channel();
        let first = std::thread::spawn(move || {
            first_result_tx
                .send(reserve_recovery_spool_bytes(&first_root, 1).map_err(|e| e.to_string()))
                .unwrap();
        });
        scan_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the first reservation pauses in a real recursive scan");

        let (same_root_started_tx, same_root_started_rx) = mpsc::channel();
        let (same_root_result_tx, same_root_result_rx) = mpsc::channel();
        let same_root_spool = spool_root.clone();
        let same_root = std::thread::spawn(move || {
            same_root_started_tx.send(()).unwrap();
            same_root_result_tx
                .send(reserve_recovery_spool_bytes(&same_root_spool, 1).map_err(|e| e.to_string()))
                .unwrap();
        });
        same_root_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("a same-root waiter starts");
        let same_root_waited = matches!(
            same_root_result_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        let other_spool_root = temp_dir("spool-quota-other-root").join("spool");
        let (other_result_tx, other_result_rx) = mpsc::channel();
        let other_root = std::thread::spawn(move || {
            let result = reserve_recovery_spool_bytes(&other_spool_root, 1)
                .map(drop)
                .map_err(|error| error.to_string());
            other_result_tx.send(result).unwrap();
        });
        let other_progressed = other_result_rx.recv_timeout(Duration::from_secs(2));
        scan_release_tx.send(()).unwrap();
        let first_reservation = first_result_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .expect("first same-root reservation is admitted after its scan");
        let second_reservation = same_root_result_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .expect("the waiting same-root reservation follows the first admission");
        first.join().unwrap();
        same_root.join().unwrap();
        other_root.join().unwrap();
        other_progressed
            .expect("an unrelated root's quota scan is not serialized behind the paused scan")
            .expect("unrelated root reservation succeeds");
        assert!(
            same_root_waited,
            "no same-root reservation enters between scan start and admission"
        );
        assert_eq!(
            recovery_spool_reserved_bytes_for_test(&root),
            2,
            "the ledger includes both serialized same-root reservations"
        );
        drop(second_reservation);
        drop(first_reservation);

        inject_spool_quota_scan_fault_for_test(&spool_root, SpoolQuotaScanFault::Error);
        let error = reserve_recovery_spool_bytes(&spool_root, 1)
            .err()
            .expect("the injected scanner error is returned");
        assert!(error
            .to_string()
            .contains("injected spool quota scan failure"));
        drop(
            reserve_recovery_spool_bytes(&spool_root, 1)
                .expect("the scan marker clears after an I/O error"),
        );

        inject_spool_quota_scan_fault_for_test(&spool_root, SpoolQuotaScanFault::Panic);
        let panic_result = std::panic::catch_unwind(|| {
            let _ = reserve_recovery_spool_bytes(&spool_root, 1);
        });
        assert!(panic_result.is_err(), "the panic fault reaches the scanner");
        drop(
            reserve_recovery_spool_bytes(&spool_root, 1)
                .expect("the scan marker clears during unwinding"),
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_file_ack_requires_parent_directory_sync_after_rename() {
        let root = temp_dir("atomic-parent-sync");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("committed-but-unacked.bin");
        fail_next_atomic_parent_sync_for_test(&root);

        let error = write_atomic_bytes(&path, b"durable receipt requires directory sync")
            .expect_err("directory sync failure cannot produce a successful ACK");
        assert!(error
            .to_string()
            .contains("injected atomic parent directory sync failure"));
        assert_eq!(
            fs::read(&path).unwrap(),
            b"durable receipt requires directory sync",
            "rename may have happened, so the outcome is retained as unacknowledged"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_history_cancel_waits_for_paused_journal_and_blocks_late_promotion() {
        let root = temp_dir("pending-cancel-race");
        let intent = pending_history_intent_for_test(
            "dictation-test-cancel-race",
            "delivery_pending",
            "unknown",
            "cancelled before external delivery",
        );
        let transaction_id = intent.transaction_id.clone();
        let active_owner = register_pending_history_owner(&root, &transaction_id)
            .expect("reserve the live cancellation identity");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pause_next_pending_history_write_for_test(&root, entered_tx, release_rx);

        let writer_root = root.clone();
        let writer_owner = active_owner.clone();
        let writer = std::thread::spawn(move || {
            write_pending_history_intent(&writer_root, &intent, &writer_owner)
        });
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("the intent writer pauses after staging fsync while it owns quota admission");

        let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
        let cancel_root = root.clone();
        let cancel_id = transaction_id.clone();
        let cancel_owner = active_owner.clone();
        let cancel = std::thread::spawn(move || {
            let result = cancel_pending_history_intent(&cancel_root, &cancel_id, &cancel_owner)
                .map_err(|error| error.to_string());
            let _ = cancel_tx.send(result);
        });
        assert!(matches!(
            cancel_rx.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        release_tx
            .send(())
            .expect("release staged journal installation");
        writer
            .join()
            .expect("journal writer thread exits")
            .expect("the paused atomic journal write completes");
        cancel.join().expect("cancellation owner exits");
        cancel_rx
            .try_recv()
            .expect("cancellation reports its durable tombstone and cleanup")
            .expect("known no-side-effect cancellation is durably recorded");

        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 0);
        assert!(get_history(&root, 10).unwrap().is_empty());
        assert!(!pending_history_path(&root, &transaction_id)
            .unwrap()
            .exists());
        assert!(!pending_history_root(&root).join(&transaction_id).exists());
        assert!(
            write_pending_history_intent(
                &root,
                &pending_history_intent_for_test(
                    &transaction_id,
                    "delivery_pending",
                    "unknown",
                    "must not return after cancellation",
                ),
                &active_owner,
            )
            .is_err(),
            "a late writer carrying the cancelled owner token cannot resurrect the intent"
        );
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn history_recovery_text_uses_only_its_pending_intent_or_history_transaction() {
        let root = temp_dir("history-recovery-text");
        let transaction_id = "dictation-recovery-copy";
        let text = "Exact recovery text, with punctuation.";
        let intent =
            pending_history_intent_for_test(transaction_id, "delivery_pending", "unknown", text);
        let owner = register_pending_history_owner(&root, transaction_id)
            .expect("reserve the exact recovery transaction identity");
        write_pending_history_intent(&root, &intent, &owner)
            .expect("the transaction's original text is durably retained");

        assert_eq!(
            history_recovery_text_for_transaction(&root, transaction_id)
                .expect("read the exact retained pending text"),
            text
        );
        assert!(
            history_recovery_text_for_transaction(&root, "some-other-transaction").is_err(),
            "a recovery ticket cannot read another transaction's text"
        );

        promote_pending_history_intent(&root, transaction_id, &owner)
            .expect("promote the uncertain transaction exactly once");
        let rows = get_history(&root, 10).expect("the text has a durable History row");
        let row = rows.first().expect("the exact transaction promotes once");
        assert_eq!(row.status, "delivery_unconfirmed");
        assert_eq!(row.delivery_method.as_deref(), Some("unknown"));
        assert_eq!(row.final_text, text);
        assert_eq!(
            history_recovery_text_for_transaction(&root, transaction_id)
                .expect("read the exact acknowledged History text"),
            text
        );
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_history_startup_promotion_is_unknown_idempotent_and_cancel_safe() {
        let root = temp_dir("pending-promotion");
        let mut interrupted = pending_history_intent_for_test(
            "dictation-test-interrupted",
            "delivery_pending",
            "unknown",
            "the text from an interrupted delivery",
        );
        let interrupted_owner = register_pending_history_owner(&root, &interrupted.transaction_id)
            .expect("reserve the interrupted delivery identity");
        write_pending_history_intent(&root, &interrupted, &interrupted_owner)
            .expect("durable pre-delivery intent");
        drop(interrupted_owner);
        let path = pending_history_path(&root, &interrupted.transaction_id).unwrap();
        let stage = path.parent().unwrap().join(".intent.pending");
        fs::rename(&path, &stage).expect("simulate crash after stage fsync but before rename");
        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 1);
        let rows = get_history(&root, 10).expect("interrupted intent becomes visible in History");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_text, "the text from an interrupted delivery");
        assert_eq!(rows[0].status, "delivery_unconfirmed");
        assert_eq!(rows[0].delivery_method.as_deref(), Some("unknown"));
        assert_eq!(
            rows[0].fallback_reason.as_deref(),
            Some("delivery_state_unknown_after_restart")
        );

        // Replaying the stable transaction key cannot create a second row.
        interrupted.status = "ok".into();
        interrupted.delivery_method = "paste".into();
        let stale_owner = register_pending_history_owner(&root, &interrupted.transaction_id)
            .expect("reacquire the stable acknowledgement identity");
        write_pending_history_intent(&root, &interrupted, &stale_owner)
            .expect("stale acknowledgement sidecar");
        drop(stale_owner);
        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 1);
        assert_eq!(get_history(&root, 10).unwrap().len(), 1);

        let cancelled = pending_history_intent_for_test(
            "dictation-test-cancelled",
            "delivery_pending",
            "unknown",
            "do not resurrect this canceled text",
        );
        let cancelled_owner = register_pending_history_owner(&root, &cancelled.transaction_id)
            .expect("reserve the cancellation candidate identity");
        write_pending_history_intent(&root, &cancelled, &cancelled_owner)
            .expect("durable cancellation candidate");
        drop(cancelled_owner);
        let cancelled_path = pending_history_path(&root, &cancelled.transaction_id).unwrap();
        let cancelled_dir = cancelled_path.parent().unwrap();
        let mut marker = File::create(cancelled_dir.join(".delivery.cancelled")).unwrap();
        marker
            .write_all(cancelled.transaction_id.as_bytes())
            .unwrap();
        marker.sync_all().unwrap();
        File::open(cancelled_dir).unwrap().sync_all().unwrap();
        assert_eq!(promote_all_pending_history_intents(&root).unwrap(), 0);
        let rows = get_history(&root, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!cancelled_dir.exists());
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_history_promotion_persists_observed_cleanup_provenance() {
        let root = temp_dir("pending-cleanup-provenance");
        let mut intent = pending_history_intent_for_test(
            "dictation-cleanup-provenance",
            "ok",
            "paste",
            "accepted transformed text",
        );
        intent.cleanup_attempt_kind = Some("success".into());
        intent.cleanup_transformations = Some(vec![
            "strip_internal_cleanup_metadata".into(),
            "restore_spoken_layout".into(),
            "restore_prepared_preamble".into(),
        ]);
        intent.cleanup_final_guard_reason = Some("final_protected_token_changed".into());
        let owner = register_pending_history_owner(&root, &intent.transaction_id)
            .expect("reserve the actual transaction identity");
        write_pending_history_intent(&root, &intent, &owner)
            .expect("persist the serialized History intent");

        promote_pending_history_intent(&root, &intent.transaction_id, &owner)
            .expect("promote the owned intent through the public store entry point");
        let page = get_history_page(&root, 10, None, None).expect("read the promoted row");
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].cleanup_attempt_kind.as_deref(),
            Some("success")
        );
        assert_eq!(
            page.items[0].cleanup_transformations.as_deref(),
            Some(
                [
                    "strip_internal_cleanup_metadata".to_owned(),
                    "restore_spoken_layout".to_owned(),
                    "restore_prepared_preamble".to_owned(),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            page.items[0].cleanup_final_guard_reason.as_deref(),
            Some("final_protected_token_changed")
        );
        let exported: serde_json::Value =
            serde_json::from_str(&export_history_json(&root).unwrap()).unwrap();
        assert_eq!(exported[0]["cleanup_attempt_kind"], "success");
        assert_eq!(
            exported[0]["cleanup_transformations"],
            serde_json::json!([
                "strip_internal_cleanup_metadata",
                "restore_spoken_layout",
                "restore_prepared_preamble"
            ])
        );

        drop(owner);
        assert_eq!(
            promote_all_pending_history_intents(&root).unwrap(),
            0,
            "the promoted intent is removed and cannot create a duplicate after reopening"
        );
        assert_eq!(
            get_history_page(&root, 10, None, None).unwrap().items.len(),
            1
        );
        clear_all_data(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    fn write_spool_f32_chunk(root: &Path, session_id: &str, index: usize, samples: &[f32]) {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let relative = PathBuf::from(session_id)
            .join("chunks")
            .join(format!("{index:08}.f32"));
        write_spool_file(root, &relative, &bytes).unwrap();
    }

    #[test]
    fn failed_short_history_retry_insert_is_idempotent_for_acknowledged_audio() {
        let dir = temp_dir("failed-short-idempotent");
        let path = write_spool_file(&dir, Path::new("failed-job.wav"), b"complete wav")
            .expect("audio artifact is durably installed");
        let context = crate::context::ContextSnapshot::general();

        insert_failed_history_with_context_once(
            &dir,
            1.25,
            &path,
            &context,
            "on_device:qwen3-asr-0.6b",
        )
        .expect("History row is inserted and acknowledged");
        insert_failed_history_with_context_once(
            &dir,
            1.25,
            &path,
            &context,
            "on_device:qwen3-asr-0.6b",
        )
        .expect("a replay after a lost acknowledgement reuses the same row");

        let rows = get_history(&dir, 10).expect("History reads after insert");
        assert_eq!(rows.len(), 1);
        let connection = open_history(&dir).expect("History database is available");
        let stored_audio: String = connection
            .query_row(
                "SELECT raw_audio_path FROM dictations WHERE id=?",
                [rows[0].id],
                |row| row.get(0),
            )
            .expect("the History row references its acknowledged recovery WAV");
        assert_eq!(PathBuf::from(stored_audio), path);
        assert!(path.is_file());

        remove_spool_artifact(&dir, &path);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_failed_short_history_removes_flat_artifact_without_resurrection() {
        let dir = temp_dir("delete-failed-short-flat");
        let path = write_spool_file(&dir, Path::new("failed-job.wav"), b"short recording")
            .expect("persist the flat failed-short artifact");
        insert_failed_history_with_context_once(
            &dir,
            0.2,
            &path,
            &crate::context::ContextSnapshot::general(),
            "on_device:test",
        )
        .unwrap();

        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!path.exists());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_history_with_retention_removed_artifact_does_not_resurrect() {
        let dir = temp_dir("delete-expired-flat-artifact");
        let path = write_spool_file(&dir, Path::new("expired-job.wav"), b"retained audio")
            .expect("persist a flat managed artifact");
        insert_failed_history_with_context_once(
            &dir,
            0.3,
            &path,
            &crate::context::ContextSnapshot::general(),
            "on_device:test",
        )
        .unwrap();
        fs::remove_file(&path).unwrap();

        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!path.exists());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn spool_io_commit_is_root_scoped_and_rejects_alias_and_prefix_roots() {
        let root = temp_dir("root-scoped-commit");
        let alias = root.join("unused-child").join("..");
        let child = root.join("child-root");
        let sibling = root.with_file_name(format!(
            "{}-sibling",
            root.file_name().unwrap().to_string_lossy()
        ));

        let owner = try_spool_io_commit(&root).expect("first data root is admitted");
        assert!(
            try_spool_io_commit(&alias).is_err(),
            "lexical aliases cannot bypass the root-scoped quota/clear permit"
        );
        assert!(
            try_spool_io_commit(&child).is_err(),
            "a nested path cannot mutate while its parent data root is committing"
        );
        let sibling_owner = try_spool_io_commit(&sibling)
            .expect("independent sibling data roots can make progress concurrently");
        drop(sibling_owner);
        drop(owner);

        let child_owner = try_spool_io_commit(&child)
            .expect("the nested root is admitted after the parent commit closes");
        drop(child_owner);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(sibling);
    }

    fn decode_wav_samples(path: &Path) -> Vec<i16> {
        let wav = read_spool_file(path).unwrap();
        hound::WavReader::new(std::io::Cursor::new(wav))
            .unwrap()
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn usage_upsert_by_day() {
        let dir = temp_dir("usage");
        insert_history(&dir, "a", "A", 2.0, false).unwrap();
        insert_history(&dir, "b", "B", 3.0, true).unwrap();
        let q = crate::queue::RequestGate::new(None).snapshots();
        let u = get_usage(&dir, q).unwrap();
        assert_eq!(u.asr_requests, 2);
        assert_eq!(u.llm_requests, 2);
        assert_eq!(u.audio_seconds, 5.0);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn schema_20_defaults_cleanup_intensity_auto() {
        let settings = Settings::default();
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert!(settings.accurate_asr_model.is_empty());
        assert_eq!(
            settings.accurate_asr_provider,
            crate::engine::EngineProvider::Groq
        );
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        let view = SettingsView::from(&settings);
        assert_eq!(view.cleanup_intensity, "auto");
        assert_eq!(view.cascade_timeout_ms, 5000);
        assert_eq!(view.cascade_proper_noun_threshold, 3);
        assert!(view.accurate_asr_model.is_empty());
        assert!(!settings.accurate_asr_configured());
        assert!(!settings.window_ocr_enabled);
        assert!(!view.window_ocr_enabled);
        assert!(settings.screen_action_hotkey.is_empty());
        assert!(settings.vision_provider.is_empty());
        assert!(settings.vision_model.is_empty());
        assert!(!settings.vision_configured());
        assert!(view.screen_action_hotkey.is_empty());
        assert!(view.vision_model.is_empty());
    }
    #[test]
    fn migration_keeps_an_explicit_saved_cleanup_intensity() {
        let mut settings = Settings {
            schema_version: 19,
            cleanup_intensity: "heavy".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.cleanup_intensity, "heavy");
    }

    #[test]
    fn accurate_asr_helpers_use_model_and_provider_keychain() {
        let mut settings = Settings::default();
        assert!(!settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "");
        settings.accurate_asr_model = "whisper-large-v3".into();
        settings.accurate_asr_provider = crate::engine::EngineProvider::OpenAi;
        settings
            .provider_api_keys
            .insert("openai".into(), "sk-accurate".into());
        assert!(settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "sk-accurate");
        assert!(settings
            .accurate_asr_endpoint()
            .expect("HTTP accurate endpoint")
            .contains("transcriptions"));
    }

    #[test]
    fn accurate_asr_configured_requires_nonempty_credential() {
        let mut settings = Settings {
            api_key: "gsk-groq".into(),
            accurate_asr_provider: crate::engine::EngineProvider::Custom,
            accurate_asr_model: "qwen3-asr-flash".into(),
            ..Settings::default()
        };
        assert_eq!(settings.accurate_asr_credential().trim(), "");
        assert!(!settings.accurate_asr_configured());

        settings
            .provider_api_keys
            .insert("custom".into(), "   ".into());
        assert_eq!(settings.accurate_asr_credential().trim(), "");
        assert!(!settings.accurate_asr_configured());

        settings
            .provider_api_keys
            .insert("custom".into(), "sk-bailian".into());
        assert_eq!(settings.accurate_asr_credential(), "sk-bailian");
        assert!(settings.accurate_asr_configured());

        settings.accurate_asr_provider = crate::engine::EngineProvider::Groq;
        settings.accurate_asr_model = "whisper-large-v3".into();
        settings.provider_api_keys.remove("custom");
        assert!(settings.accurate_asr_configured());
        assert_eq!(settings.accurate_asr_credential(), "gsk-groq");
    }

    #[test]
    fn vision_configured_needs_provider_model_and_key() {
        let mut settings = Settings {
            vision_model: "gpt-4o".into(),
            ..Settings::default()
        };
        assert!(!settings.vision_configured());
        settings.vision_provider = "openai".into();
        assert!(!settings.vision_configured());
        settings
            .provider_api_keys
            .insert("openai".into(), "sk-vision".into());
        assert!(settings.vision_configured());
        settings.vision_provider = "ollama".into();
        settings.vision_model = "llava".into();
        assert!(settings.vision_configured());
    }

    fn write_sensevoice_fixture(models_root: &Path) {
        let dir = models_root.join("sensevoice-small");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.int8.onnx"), b"fixture-onnx").unwrap();
        std::fs::write(dir.join("tokens.txt"), b"fixture-tokens").unwrap();
        std::fs::write(
            crate::ondevice_asr::archive_sha_path(models_root, "sensevoice-small"),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256,
        )
        .unwrap();
    }

    fn on_device_settings(onboarded: bool) -> Settings {
        Settings {
            asr_provider: crate::engine::EngineProvider::OnDevice,
            asr_model: "sensevoice-small".into(),
            api_key: String::new(),
            onboarded,
            cleanup_enabled: false,
            ..Settings::default()
        }
    }

    #[test]
    fn on_device_files_ready_do_not_allow_onboarding_without_inference() {
        let dir = temp_dir("on-device-ready-validate");
        let models_root = dir.join("models");
        write_sensevoice_fixture(&models_root);
        let settings = on_device_settings(true);
        assert!(settings
            .validate_with_models_root(Some(&models_root))
            .is_err());
        assert!(!settings.on_device_asr_ready(Some(&models_root)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn onboarded_on_device_empty_groq_key_rejected_when_model_missing() {
        let dir = temp_dir("on-device-missing-validate");
        let models_root = dir.join("models");
        std::fs::create_dir_all(&models_root).unwrap();
        let settings = on_device_settings(true);
        assert!(settings
            .validate_with_models_root(Some(&models_root))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_without_models_root_rejects_on_device_empty_key() {
        let dir = temp_dir("on-device-validate-no-root");
        write_sensevoice_fixture(&dir.join("models"));
        let settings = on_device_settings(true);
        assert!(
            settings.validate().is_err(),
            "validate() must not grant the empty-key exception without a Ready check"
        );
        assert!(settings.validate_with_models_root(None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_settings_does_not_keep_onboarding_for_files_only_on_device() {
        let dir = temp_dir("on-device-load-ready");
        std::fs::create_dir_all(&dir).unwrap();
        write_sensevoice_fixture(&dir.join("models"));
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":19,"onboarded":true,"asr_provider":"on_device","asr_model":"sensevoice-small","cleanup_enabled":false}"#,
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(
            settings.asr_provider,
            crate::engine::EngineProvider::OnDevice
        );
        assert!(!settings.onboarded);
        assert!(settings.api_key.trim().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn vision_and_cleanup_reject_on_device() {
        let mut settings = on_device_settings(false);
        settings.cleanup_enabled = true;
        settings.cleanup_provider = crate::engine::EngineProvider::OnDevice;
        settings.cleanup_model = "llama3.2".into();
        assert!(settings.validate().is_err());

        settings.cleanup_enabled = false;
        settings.vision_provider = "on_device".into();
        settings.vision_model = "sensevoice-small".into();
        assert!(!settings.vision_configured());
        assert!(settings.vision_endpoint().is_none());
    }

    #[test]
    fn accurate_asr_rejects_on_device_and_empty_key() {
        let settings = Settings {
            accurate_asr_provider: crate::engine::EngineProvider::OnDevice,
            accurate_asr_model: "sensevoice-small".into(),
            ..Settings::default()
        };
        assert!(!settings.accurate_asr_configured());
        assert!(settings.accurate_asr_endpoint().is_none());
    }

    #[test]
    fn schema_20_copies_legacy_style_example_into_pairs() {
        let mut settings = Settings {
            cleanup_intensity: String::new(),
            cascade_timeout_ms: 0,
            cascade_proper_noun_threshold: 0,
            context_mappings: vec![crate::context::AppMapping {
                id: "wechat".into(),
                label: "微信".into(),
                family: crate::context::ContextFamily::PersonalChat,
                writing_task_id: None,
                writing_preferences: Vec::new(),
                preference_guidance_provider_approved: false,
                preference_revision: 0,
                deleted: false,
                mode_id: None,
                bundle_id: Some("com.tencent.xinWeChat".into()),
                executable: None,
                browser_host: None,
                browser_path_prefix: None,
                focused_field: None,
                source_permissions: Default::default(),
                style_example_input: Some("好的哈哈".into()),
                style_example_output: Some("好的哈哈。".into()),
                style_example_pairs: Vec::new(),
                style_examples_approved: false,
                enabled: true,
                cleanup_effort: None,
                cleanup_intensity: None,
                cleanup_enabled: true,
                dictionary_learn_enabled: true,
            }],
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert_eq!(settings.context_mappings[0].style_example_pairs.len(), 1);
        assert_eq!(
            settings.context_mappings[0].style_example_pairs[0].input,
            "好的哈哈"
        );
        assert_eq!(
            settings.context_mappings[0].style_example_pairs[0].output,
            "好的哈哈。"
        );
    }

    #[test]
    fn schema_20_preserves_only_explicit_legacy_local_ocr_choice() {
        let mapping = crate::context::AppMapping {
            id: "notes".into(),
            label: "Notes".into(),
            family: crate::context::ContextFamily::Document,
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 0,
            deleted: false,
            mode_id: None,
            bundle_id: Some("com.example.Notes".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let mut settings = Settings {
            schema_version: 20,
            window_ocr_enabled: true,
            context_mappings: vec![mapping],
            ..Settings::default()
        };
        settings.normalize();
        let grants = settings.context_mappings[0].source_permissions;
        assert!(grants.local_ocr);
        assert!(!grants.ax_text);
        assert!(!grants.cloud_vision);
        assert!(!grants.context_text_to_providers);

        let legacy: crate::context::AppMapping = serde_json::from_str(
            r#"{"id":"legacy","label":"Legacy","family":"document","bundle_id":"com.example.Editor","style_example_input":"private","style_example_output":"private."}"#,
        )
        .unwrap();
        assert_eq!(legacy.style_example_input.as_deref(), Some("private"));
        assert!(!legacy.style_examples_approved);
        assert_eq!(legacy.source_permissions, Default::default());
    }

    #[test]
    fn deleted_mapping_tombstone_persists_identity_but_is_hidden_from_settings_view() {
        let tombstone = crate::context::AppMapping {
            id: "rule:deleted-identity".into(),
            label: "Deleted mapping".into(),
            family: crate::context::ContextFamily::General,
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 6,
            deleted: true,
            mode_id: None,
            bundle_id: None,
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: false,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: false,
            dictionary_learn_enabled: false,
        };
        let mut settings = Settings {
            context_mappings: vec![tombstone],
            ..Settings::default()
        };
        settings.normalize();
        let encoded = serde_json::to_string(&settings).expect("settings serialize");
        let mut loaded: Settings = serde_json::from_str(&encoded).expect("settings deserialize");
        loaded.normalize();

        assert_eq!(loaded.context_mappings.len(), 1);
        assert_eq!(loaded.context_mappings[0].id, "rule:deleted-identity");
        assert_eq!(loaded.context_mappings[0].preference_revision, 6);
        assert!(loaded.context_mappings[0].deleted);
        assert!(SettingsView::from(&loaded).context_mappings.is_empty());
    }

    #[test]
    fn old_settings_get_new_defaults() {
        let dir = temp_dir("settings");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":8,"api_key":"test","selected_action_hotkey":"","selected_actions_enabled":false,"output_mode":"plain"}"#,
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert!(!settings.onboarded);
        assert!(settings.cleanup_enabled);
        assert_eq!(settings.cleanup_intensity, "auto");
        assert_eq!(settings.cascade_timeout_ms, 5000);
        assert_eq!(settings.cascade_proper_noun_threshold, 3);
        assert!(settings.accurate_asr_model.is_empty());
        assert!(settings.show_tray_icon);
        assert_eq!(settings.cleanup_model, crate::llm::MODEL);
        assert_eq!(settings.activation_mode, "tap");
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.ui_language, "system");
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.output_mode, "auto");
        assert_eq!(settings.translation_target_language, "en");
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.hotkey, "CmdOrControl+Alt+Space");
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+Slash");
        assert!(settings.selected_actions_enabled);
        assert!(settings.dictionary_learn_enabled);
        assert!(settings.input_device.is_empty());
        assert_eq!(settings.input_gain, 1.0);
        assert!(!settings.keep_success_audio);
        assert!(settings.asr_base_url.is_empty());
        assert!(settings.asr_api_key.is_empty());
        assert!(settings
            .writing_modes
            .iter()
            .any(|mode| mode.id == "general" && mode.builtin));
        let backup: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("settings.json.pre-migration.bak")).unwrap(),
        )
        .unwrap();
        assert_eq!(backup["api_key"], "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_14_replaces_legacy_shift_slash_defaults_and_option_divide() {
        let mut settings = Settings {
            schema_version: 13,
            hotkey: "CmdOrControl+Shift+Space".into(),
            selected_action_hotkey: "CmdOrControl+Alt+÷".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.hotkey, "CmdOrControl+Alt+Space");
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+Slash");

        let mut legacy_slash = Settings {
            schema_version: 13,
            selected_action_hotkey: "CmdOrControl+Shift+Slash".into(),
            ..Settings::default()
        };
        legacy_slash.normalize();
        assert_eq!(
            legacy_slash.selected_action_hotkey,
            "CmdOrControl+Alt+Slash"
        );

        let mut previous_default = Settings {
            schema_version: 14,
            selected_action_hotkey: "CmdOrControl+Alt+Backslash".into(),
            ..Settings::default()
        };
        previous_default.normalize();
        assert_eq!(
            previous_default.selected_action_hotkey,
            "CmdOrControl+Alt+Slash"
        );

        let mut custom = Settings {
            schema_version: 13,
            hotkey: "CmdOrControl+Shift+K".into(),
            selected_action_hotkey: "CmdOrControl+Alt+K".into(),
            ..Settings::default()
        };
        custom.normalize();
        assert_eq!(custom.hotkey, "CmdOrControl+Shift+K");
        assert_eq!(custom.selected_action_hotkey, "CmdOrControl+Alt+K");
    }

    #[test]
    fn settings_schema_migrates_retired_gestures_to_tap_idempotently_without_rebinding() {
        for mode in ["double_tap", "hybrid", "hold", "tap"] {
            for hotkey in ["Fn", "CmdOrControl+Alt+K", "Shift"] {
                let mut settings = Settings {
                    schema_version: 24,
                    activation_mode: mode.into(),
                    hotkey: hotkey.into(),
                    ..Settings::default()
                };
                settings.normalize();
                assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
                assert_eq!(settings.activation_mode, "tap");
                assert_eq!(settings.hotkey, hotkey);
                settings.normalize();
                assert_eq!(settings.activation_mode, "tap");
                settings.validate().unwrap();
            }
        }
    }
    #[test]
    fn all_current_modes_support_fn_and_combinations() {
        assert_eq!(Settings::default().activation_mode, "tap");
        for mode in ["tap", "hold_to_talk", "hybrid_hold_lock"] {
            for hotkey in ["Fn", "CmdOrControl+Alt+Space"] {
                let mut settings = Settings {
                    activation_mode: mode.into(),
                    hotkey: hotkey.into(),
                    ..Settings::default()
                };
                settings.normalize();
                assert_eq!(settings.activation_mode, mode);
                settings.validate().unwrap();
            }
        }
    }
    #[test]
    fn changing_binding_preserves_mode_and_rejects_new_standalone_modifiers() {
        let previous = Settings {
            activation_mode: "hold_to_talk".into(),
            ..Settings::default()
        };
        let mut next = previous.clone();
        next.hotkey = "Fn".into();
        next.validate_binding_changes(&previous).unwrap();
        assert_eq!(next.activation_mode, "hold_to_talk");
        next.hotkey = "Shift".into();
        assert!(next.validate_binding_changes(&previous).is_err());
        let legacy = next.clone();
        next.validate_binding_changes(&legacy).unwrap();
    }
    #[test]
    fn new_global_bindings_do_not_take_over_typing_or_navigation() {
        let previous = Settings::default();
        for binding in ["A", "Enter", "Tab", "Space", "Shift+A", "Shift+Space"] {
            let mut next = previous.clone();
            next.hotkey = binding.into();
            next.validate_bindings().unwrap();
            assert!(
                next.validate_binding_changes(&previous).is_err(),
                "{binding}"
            );
            next.hotkey = previous.hotkey.clone();
            next.selected_action_hotkey = binding.into();
            assert!(
                next.validate_binding_changes(&previous).is_err(),
                "{binding}"
            );
        }
    }
    #[test]
    fn new_combinations_and_function_keys_remain_available() {
        let previous = Settings::default();
        for binding in [
            "Fn",
            "CmdOrControl+Shift+1",
            "Alt+A",
            "Control+K",
            "F13",
            "Shift+F13",
        ] {
            let mut next = previous.clone();
            next.hotkey = binding.into();
            next.validate_binding_changes(&previous).unwrap();
        }
    }
    #[test]
    fn mode_changes_preserve_existing_plain_key_bindings() {
        let previous = Settings {
            hotkey: "Shift+A".into(),
            ..Settings::default()
        };
        let mut next = previous.clone();
        next.activation_mode = "hold_to_talk".into();
        next.validate_binding_changes(&previous).unwrap();
        assert_eq!(next.hotkey, "Shift+A");
    }
    #[test]
    fn configuration_validation_allows_missing_unchanged_keys_but_readiness_does_not() {
        let settings = Settings {
            onboarded: true,
            activation_mode: "hold_to_talk".into(),
            ..Settings::default()
        };
        assert!(settings.validate_configuration().is_ok());
        assert!(settings.validate().is_err());
        let dir = temp_dir("binding-without-key");
        let backend = FakeCredentialBackend::default();
        save_settings_with_validation(&dir, &settings, &backend, false).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["activation_mode"], "hold_to_talk");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn non_service_configuration_save_preserves_deferred_credentials_without_store_access() {
        let dir = temp_dir("non-service-save-deferred-credentials");
        let sidecar_value = "deferred-sidecar-fixture";
        write_secret_sidecar(&dir, "asr_api_key", sidecar_value).unwrap();
        let sidecar_path = secret_sidecar_path(&dir, "asr_api_key");
        let sidecar_before = std::fs::read(&sidecar_path).unwrap();
        let settings_credential = "deferred-settings-fixture";
        let settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_model: "whisper-1".into(),
            asr_api_key: sidecar_value.into(),
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_model: "router-cleanup-model".into(),
            cleanup_api_key: settings_credential.into(),
            custom_base_url: "https://router.example.test/v1".into(),
            input_device: "MacBook Pro Microphone".into(),
            ..Settings::default()
        };
        let mut settings = settings;
        settings
            .unverified_credential_sources
            .insert("sidecar:asr_api_key".into(), sidecar_value.into());
        settings.unverified_credential_sources.insert(
            "settings:cleanup_api_key".into(),
            settings_credential.into(),
        );
        settings
            .credential_baselines
            .insert("provider_custom".into(), "existing-baseline-fixture".into());

        save_configuration(&dir, &settings)
            .expect("a structurally valid device preference does not require provider readiness");

        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(on_disk["input_device"], "MacBook Pro Microphone");
        assert_eq!(on_disk["custom_base_url"], "https://router.example.test/v1");
        assert_eq!(on_disk["asr_api_key"], "");
        assert_eq!(on_disk["cleanup_api_key"], settings_credential);
        assert_eq!(on_disk["provider_api_keys"], serde_json::json!({}));
        assert_eq!(std::fs::read(&sidecar_path).unwrap(), sidecar_before);
        assert_eq!(
            settings.credential_baselines.get("provider_custom"),
            Some(&"existing-baseline-fixture".to_owned())
        );
        assert!(settings.validate_configuration().is_ok());
        let missing_credentials = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_model: "whisper-1".into(),
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_model: "router-cleanup-model".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        assert!(missing_credentials.validate_configuration().is_ok());
        assert!(missing_credentials.validate_with_models_root(None).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_local_model_does_not_block_structural_preferences_but_readiness_still_fails() {
        let dir = temp_dir("non-service-save-missing-local-model");
        let settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::OnDevice,
            cleanup_enabled: false,
            input_device: "MacBook Pro Microphone".into(),
            ..Settings::default()
        };

        save_configuration(&dir, &settings)
            .expect("local model availability is not a prerequisite for saving preferences");
        assert!(settings
            .validate_with_models_root(Some(&dir.join("models")))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remote_keyless_local_route_stays_readiness_gated_during_preference_saves() {
        let dir = temp_dir("non-service-save-remote-keyless-local-route");
        let settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::LocalWhisper,
            local_whisper_base_url: "https://remote.example.test/v1".into(),
            cleanup_enabled: false,
            input_device: "MacBook Pro Microphone".into(),
            ..Settings::default()
        };

        save_configuration(&dir, &settings)
            .expect("structurally valid preferences may be saved without route readiness");
        assert!(settings
            .validate_with_models_root(Some(&dir.join("models")))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn structurally_invalid_audio_preferences_are_still_rejected() {
        let mut settings = Settings {
            input_device: "x".repeat(513),
            ..Settings::default()
        };
        assert!(settings.validate_configuration().is_err());

        settings.input_device = "MacBook Pro Microphone".into();
        settings.input_gain = 4.1;
        assert!(settings.validate_configuration().is_err());
    }

    #[test]
    fn selected_action_migration_preserves_custom_shortcuts() {
        let mut settings = Settings {
            schema_version: 8,
            selected_action_hotkey: "CmdOrControl+Alt+K".into(),
            selected_actions_enabled: false,
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.selected_action_hotkey, "CmdOrControl+Alt+K");
        assert!(!settings.selected_actions_enabled);
    }

    #[test]
    fn input_device_preference_is_trimmed_and_keeps_default_empty() {
        let mut settings = Settings {
            input_device: "  EarPods Microphone  ".into(),
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.input_device, "EarPods Microphone");
        assert!(Settings::default().input_device.is_empty());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn long_recording_defaults_to_auto_paste() {
        assert_eq!(Settings::default().long_output_mode, "paste");
        assert_eq!(Settings::default().delivery_policy, "auto");

        let mut settings = Settings {
            long_output_mode: "unsupported".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.delivery_policy, "auto");
    }

    #[test]
    fn legacy_long_recording_default_migrates_to_auto_paste() {
        let mut settings = Settings {
            schema_version: 11,
            long_output_mode: "clipboard".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.long_output_mode, "paste");
        assert_eq!(settings.delivery_policy, "auto");

        let mut current_choice = Settings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            long_output_mode: "clipboard".into(),
            ..Settings::default()
        };
        current_choice.normalize();
        assert_eq!(current_choice.long_output_mode, "clipboard");

        let mut explicit_delivery = Settings {
            delivery_policy: "clipboard_only".into(),
            ..Settings::default()
        };
        explicit_delivery.normalize();
        assert_eq!(explicit_delivery.delivery_policy, "clipboard_only");
    }

    #[test]
    fn invalid_ui_language_and_theme_fall_back_to_safe_defaults() {
        let mut settings = Settings {
            ui_language: "fr".into(),
            theme: "sepia".into(),
            ..Settings::default()
        };

        settings.normalize();

        assert_eq!(settings.ui_language, "system");
        assert_eq!(settings.theme, "system");
    }

    #[test]
    fn unfinished_setup_keeps_explicit_chinese_after_later_schema_bumps() {
        let mut current = Settings {
            schema_version: 25,
            onboarded: false,
            ui_language: "zh".into(),
            ..Settings::default()
        };
        current.normalize();
        assert_eq!(current.ui_language, "zh");

        let mut legacy = Settings {
            schema_version: 6,
            onboarded: false,
            ui_language: "zh".into(),
            ..Settings::default()
        };
        legacy.normalize();
        assert_eq!(legacy.ui_language, "system");
    }

    #[test]
    fn deprecated_single_modifier_bindings_remain_visible_after_migration() {
        let mut settings = Settings {
            selected_action_hotkey: "Shift".into(),
            verbatim_hotkey: "Alt".into(),
            translation_hotkey: "Control".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.selected_action_hotkey, "Shift");
        assert_eq!(settings.verbatim_hotkey, "Alt");
        assert_eq!(settings.translation_hotkey, "Control");
        settings.validate().unwrap();
    }

    #[test]
    fn selected_actions_can_be_enabled_before_a_hotkey_is_assigned() {
        let settings = Settings {
            selected_actions_enabled: true,
            ..Settings::default()
        };

        assert!(settings.validate().is_ok());
    }

    #[test]
    fn unsupported_cleanup_model_falls_back_to_default() {
        let mut settings = Settings {
            cleanup_model: "retired/model".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.cleanup_model, crate::llm::MODEL);
        assert!(Settings {
            cleanup_model: "retired/model".into(),
            ..Settings::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn cleanup_default_moves_off_retired_llama_without_rewriting_saved_choices() {
        assert_eq!(Settings::default().cleanup_model, "openai/gpt-oss-20b");
        let dir = temp_dir("cleanup-model-retirement");
        std::fs::create_dir_all(&dir).unwrap();
        for model in [
            "llama-3.1-8b-instant",
            "llama-3.3-70b-versatile",
            "openai/gpt-oss-120b",
        ] {
            std::fs::write(
                dir.join("settings.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema_version": SETTINGS_SCHEMA_VERSION,
                    "onboarded": false,
                    "cleanup_enabled": false,
                    "cleanup_provider": "groq",
                    "cleanup_model": model,
                }))
                .unwrap(),
            )
            .unwrap();
            let (settings, _) = load_settings(&dir);
            assert_eq!(settings.cleanup_model, model);
        }

        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "onboarded": false,
                "cleanup_enabled": false,
                "cleanup_provider": "groq",
            }))
            .unwrap(),
        )
        .unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(settings.cleanup_model, "openai/gpt-oss-20b");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn normalize_keeps_custom_asr_url_when_the_key_is_empty() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_api_key: String::new(),
            asr_model: "my-local-whisper".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert_eq!(settings.asr_base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(settings.asr_model, "my-local-whisper");
    }

    #[test]
    fn load_settings_repairs_incomplete_custom_asr_left_on_disk() {
        let dir = temp_dir("incomplete-custom-asr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"schema_version":16,"asr_provider":"custom","asr_base_url":"https://relay.example.com/v1","asr_model":"whisper-1"}"#,
        )
        .unwrap();
        let (settings, needs_persist) = load_settings(&dir);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
        assert!(settings.validate().is_ok());
        assert!(needs_persist);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_16_keeps_groq_urls_on_the_groq_provider() {
        let mut settings = Settings {
            schema_version: 15,
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Groq);
        assert!(settings.asr_base_url.is_empty());
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn schema_16_infers_custom_asr_from_a_saved_url() {
        let mut settings = Settings {
            schema_version: 15,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_api_key: "local-key".into(),
            asr_model: "my-local-whisper".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::Custom);
        assert_eq!(settings.asr_model, "my-local-whisper");
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::Groq
        );
        assert!(settings.cleanup_base_url.is_empty());
    }

    #[test]
    fn groq_provider_clears_custom_url_and_resets_unknown_asr_model() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Groq,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            asr_model: "whisper-1".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert!(settings.asr_base_url.is_empty());
        assert_eq!(settings.asr_model, crate::asr::MODEL);
    }

    #[test]
    fn custom_cleanup_keeps_unknown_models_and_does_not_reuse_the_groq_key() {
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            api_key: "gsk_fallback".into(),
            cleanup_api_key: String::new(),
            onboarded: true,
            ..Settings::default()
        };
        assert_eq!(settings.cleanup_credential(), "");
        assert_eq!(
            settings.validate().unwrap_err().to_string(),
            "自定义整理地址需要填写整理密钥。"
        );

        let mut kept = settings.clone();
        kept.cleanup_api_key = "sk-openai".into();
        kept.normalize();
        assert_eq!(kept.cleanup_model, "gpt-4o-mini");
        assert_eq!(kept.cleanup_request_model(), "gpt-4o-mini");
        assert_eq!(
            kept.cleanup_endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(kept.cleanup_credential(), "sk-openai");
        assert!(kept.validate().is_ok());
    }

    #[test]
    fn load_and_public_view_keep_keychain_unavailable_distinct_from_missing() {
        let dir = temp_dir("credential-read-state");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "asr_provider": "custom",
                "custom_base_url": "https://relay.example.test/v1",
                "cleanup_enabled": true,
                "cleanup_provider": "custom",
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "provider_custom",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic keychain failure".into(),
            )],
        );
        backend.script_reads("asr_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("cleanup_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("groq_api_key", [crate::keychain::ApiKeyState::Missing]);

        let (settings, _) = load_settings_with_backend(&dir, &backend);
        let view = SettingsView::from(&settings);

        assert_eq!(
            settings.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            settings.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Missing)
        );
        assert_eq!(
            view.provider_keys["custom"].keychain_state,
            CredentialReadState::Unavailable
        );
        assert_eq!(
            view.asr_api_key_keychain_state,
            CredentialReadState::Missing
        );
        assert_eq!(
            view.cleanup_api_key_keychain_state,
            CredentialReadState::Missing
        );
        assert_eq!(view.custom_base_url, "https://relay.example.test/v1");
        assert!(settings.asr_api_key.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_keychain_keeps_an_unverified_session_value_without_claiming_it_is_durable() {
        let dir = temp_dir("unavailable-custom-session-value");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "asr_provider": "custom",
                "asr_base_url": "https://relay.example.test/v1",
                "custom_base_url": "https://relay.example.test/v1",
                "asr_api_key": "synthetic-deferred-key",
                "cleanup_enabled": false,
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads("provider_custom", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads(
            "asr_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic keychain failure".into(),
            )],
        );
        backend.script_reads("cleanup_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("groq_api_key", [crate::keychain::ApiKeyState::Missing]);

        let (settings, _) = load_settings_with_backend(&dir, &backend);
        let view = SettingsView::from(&settings);

        assert_eq!(settings.asr_api_key, "synthetic-deferred-key");
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:asr_api_key"));
        assert!(view.provider_keys["custom"].configured);
        assert_eq!(
            view.provider_keys["custom"].keychain_state,
            CredentialReadState::Unavailable
        );
        assert_eq!(
            view.asr_api_key_keychain_state,
            CredentialReadState::Unavailable
        );
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unknown_asr_origin_keeps_legacy_slot_unavailable_outside_custom_recovery_scope() {
        let dir = temp_dir("unknown-origin-custom-session-value");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "asr_provider": "custom",
                "custom_base_url": "https://relay.example.test/v1",
                "asr_api_key": "synthetic-deferred-key",
                "cleanup_enabled": false,
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads("provider_custom", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads(
            "asr_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic keychain failure".into(),
            )],
        );
        backend.script_reads("cleanup_api_key", [crate::keychain::ApiKeyState::Missing]);
        backend.script_reads("groq_api_key", [crate::keychain::ApiKeyState::Missing]);

        let (settings, _) = load_settings_with_backend(&dir, &backend);
        let view = SettingsView::from(&settings);

        let asr_scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert!(!asr_scope.include_asr_legacy);
        assert_eq!(settings.asr_api_key, "synthetic-deferred-key");
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:asr_api_key"));
        assert!(view.provider_keys["custom"].configured);
        assert_eq!(
            view.provider_keys["custom"].keychain_state,
            CredentialReadState::Missing
        );
        assert_eq!(
            view.asr_api_key_keychain_state,
            CredentialReadState::Unavailable
        );
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn custom_recovery_scope_excludes_non_custom_purpose_aliases() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Groq,
            cleanup_enabled: true,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            custom_base_url: "https://router.example.test/v1/chat/completions".into(),
            cleanup_base_url: "https://router.example.test/v1/cleanup".into(),
            ..Settings::default()
        };
        settings.asr_base_url = "https://router.example.test/v1/asr".into();
        let cleanup_scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Cleanup).unwrap();
        assert_eq!(cleanup_scope.origin, "https://router.example.test");
        assert_eq!(cleanup_scope.purpose, CustomCredentialPurpose::Cleanup);
        assert!(!cleanup_scope.include_asr_legacy);
        assert!(cleanup_scope.include_cleanup_legacy);
        assert!(
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).is_none()
        );

        settings.asr_provider = crate::engine::EngineProvider::Custom;
        let both_custom =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert!(both_custom.include_asr_legacy);
        assert!(both_custom.include_cleanup_legacy);

        settings.cleanup_enabled = false;
        let asr_only =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert!(asr_only.include_asr_legacy);
        assert!(!asr_only.include_cleanup_legacy);

        settings.custom_base_url = "file:///tmp/invalid".into();
        assert!(
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).is_none()
        );
    }

    #[test]
    fn foreground_recovery_omits_a_legacy_alias_from_a_different_origin() {
        let settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            cleanup_enabled: true,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://router-b.example.test/v1".into(),
            custom_base_url: "https://router-a.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        assert_eq!(scope.origin, "https://router-a.example.test");
        assert!(scope.include_asr_legacy);
        assert!(!scope.include_cleanup_legacy);
        let cleanup_scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Cleanup).unwrap();
        assert!(!cleanup_scope.include_cleanup_legacy);
    }

    #[test]
    fn unavailable_custom_readiness_does_not_claim_the_key_is_missing() {
        let mut settings = Settings {
            onboarded: true,
            asr_provider: crate::engine::EngineProvider::Custom,
            custom_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        settings
            .credential_read_states
            .insert("provider_custom".into(), CredentialReadState::Unavailable);
        assert_eq!(
            settings.validate().unwrap_err().to_string(),
            "钥匙串暂时不可读取；当前设置未更改，请在前台设置中重试读取。"
        );
    }

    #[test]
    fn custom_recovery_adoption_is_session_only_and_does_not_fan_out_on_save() {
        let dir = temp_dir("session-only-custom-recovery");
        let backend = FakeCredentialBackend::default();
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut settings,
            "synthetic-session-only-key".into(),
            scope,
        );

        assert!(settings.session_only_custom_credential);
        assert_eq!(
            settings.provider_api_keys.get("custom").map(String::as_str),
            Some("synthetic-session-only-key")
        );
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Custom),
            ""
        );
        settings.vision_provider = "custom".into();
        assert_eq!(settings.vision_credential(), "");
        let public = SettingsView::from(&settings);
        assert!(public.provider_keys["custom"].configured);
        assert_eq!(
            public.provider_keys["custom"].hint.as_deref(),
            Some("••••y-key")
        );
        assert!(backend.writes.borrow().is_empty());
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        assert!(backend.writes.borrow().is_empty());
        let on_disk = fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!on_disk.contains("synthetic-session-only-key"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parsed_preference_patch_can_preserve_nonsecret_credential_state() {
        let mut previous = Settings::default();
        previous
            .credential_read_states
            .insert("provider_custom".into(), CredentialReadState::Unavailable);
        previous.session_only_custom_credential = true;
        previous.session_custom_credential_scope = Some(CustomCredentialRecoveryScope {
            origin: "https://router.example.test".into(),
            purpose: CustomCredentialPurpose::Asr,
            asr_active: true,
            cleanup_active: false,
            include_asr_legacy: true,
            include_cleanup_legacy: false,
        });
        previous.unverified_credential_sources.insert(
            "settings:cleanup_api_key".into(),
            "synthetic-deferred".into(),
        );
        let parsed: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        let mut next = parsed;

        next.preserve_credential_runtime_metadata_from(&previous);

        assert_eq!(
            next.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Unavailable)
        );
        assert!(next.session_only_custom_credential);
        assert_eq!(
            next.session_custom_credential_scope,
            previous.session_custom_credential_scope
        );
        assert_eq!(
            next.unverified_credential_sources
                .get("settings:cleanup_api_key"),
            Some(&"synthetic-deferred".to_owned())
        );
    }

    #[test]
    fn public_settings_view_never_contains_the_full_runtime_credential() {
        let secret = "synthetic-full-secret-12345";
        let settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            custom_base_url: "https://router.example.test/v1".into(),
            provider_api_keys: [("custom".to_owned(), secret.to_owned())]
                .into_iter()
                .collect(),
            ..Settings::default()
        };
        let serialized = serde_json::to_string(&SettingsView::from(&settings)).unwrap();
        assert!(!serialized.contains(secret));
        assert!(serialized.contains("configured"));
        assert!(serialized.contains("keychain_state"));
    }

    #[test]
    fn settings_view_masks_every_short_ascii_and_unicode_credential() {
        for secret in ["x", "ab", "abc", "abcd", "abcde", "🔑秘密"] {
            let settings = Settings {
                api_key: secret.into(),
                provider_api_keys: [("custom".to_owned(), secret.into())].into_iter().collect(),
                ..Settings::default()
            };
            let view = SettingsView::from(&settings);
            let serialized = serde_json::to_string(&view).unwrap();
            assert_eq!(view.api_key_hint.as_deref(), Some("••••"));
            assert_eq!(view.provider_keys["custom"].hint.as_deref(), Some("••••"));
            let public: serde_json::Value = serde_json::from_str(&serialized).unwrap();
            assert_eq!(public["api_key_hint"], "••••");
            assert_eq!(public["provider_keys"]["custom"]["hint"], "••••");
        }
    }

    #[test]
    fn session_custom_credential_is_bound_to_origin_across_a_settings_patch_round_trip() {
        let dir = temp_dir("session-custom-origin-patch");
        let backend = FakeCredentialBackend::default();
        let mut current = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&current, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut current,
            "synthetic-session-origin-key".into(),
            scope,
        );
        assert_eq!(current.asr_credential(), "synthetic-session-origin-key");

        // Match update_settings_patch's serde merge plus runtime-metadata
        // restoration; the recovered value is not sent by the frontend.
        let mut merged = serde_json::to_value(&current).unwrap();
        merged.as_object_mut().unwrap().insert(
            "asr_base_url".into(),
            serde_json::Value::String("https://router-b.example.test/v1".into()),
        );
        let mut next: Settings = serde_json::from_value(merged).unwrap();
        next.preserve_credential_runtime_metadata_from(&current);
        next.session_only_custom_credential = current.session_only_custom_credential;
        assert!(next.clear_stale_session_custom_credential());
        assert!(!next.session_only_custom_credential);
        assert!(next.session_custom_credential_scope.is_none());
        assert!(next.session_custom_credential_rejected);
        assert_eq!(
            next.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Configured),
            "revoking a process-only route lease must not rewrite the last Keychain read result"
        );
        assert!(!next.provider_api_keys.contains_key("custom"));
        assert_eq!(next.asr_credential(), "");

        save_settings_with_backend(&dir, &next, &backend).unwrap();
        assert!(backend.writes.borrow().is_empty());
        let on_disk = fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!on_disk.contains("synthetic-session-origin-key"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn revoked_custom_value_stays_rejected_through_full_and_patch_round_trips() {
        let dir = temp_dir("revoked-custom-full-patch-round-trip");
        let backend = FakeCredentialBackend::default();
        let mut previous = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            custom_base_url: "https://router-a.example.test/v1".into(),
            asr_api_key: "synthetic-deferred-asr-alias".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        previous.unverified_credential_sources.insert(
            "settings:asr_api_key".into(),
            "synthetic-deferred-asr-alias".into(),
        );
        previous
            .credential_baselines
            .insert("asr_api_key".into(), "synthetic-older-asr-value".into());
        previous
            .credential_read_states
            .insert("asr_api_key".into(), CredentialReadState::Unavailable);

        let scope =
            custom_credential_recovery_scope(&previous, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut previous,
            "synthetic-session-custom-value".into(),
            scope,
        );
        previous.cleanup_enabled = true;
        previous.cleanup_provider = crate::engine::EngineProvider::Custom;
        previous.cleanup_base_url = "https://router-b.example.test/v1".into();
        assert!(previous.clear_stale_session_custom_credential());
        assert!(previous.session_custom_credential_rejected);
        assert!(!previous.provider_api_keys.contains_key("custom"));

        // Full settings payloads do not serialize the rejection/read-state
        // metadata and may omit aliases. The production compatibility helper
        // must restore provenance before bind_* restores unchanged-route values.
        let mut full: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        full.asr_api_key.clear();
        crate::preserve_session_custom_credential_for_full_settings_roundtrip(&previous, &mut full);
        full.asr_api_key = bind_asr_key_to_host(
            full.asr_provider,
            &previous.asr_base_url,
            &full.asr_base_url,
            &full.asr_api_key,
            &previous.asr_api_key,
        );
        assert_eq!(full.asr_api_key, "synthetic-deferred-asr-alias");
        assert!(full.session_custom_credential_rejected);
        assert_eq!(full.asr_credential(), "");
        assert_eq!(
            full.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.unverified_credential_sources
                .get("settings:asr_api_key")
                .map(String::as_str),
            Some("synthetic-deferred-asr-alias")
        );
        assert_eq!(
            full.credential_baselines
                .get("asr_api_key")
                .map(String::as_str),
            Some("synthetic-older-asr-value")
        );

        let mut patch = serde_json::to_value(&full).unwrap();
        patch
            .as_object_mut()
            .unwrap()
            .insert("ui_language".into(), serde_json::Value::String("en".into()));
        let mut after_patch: Settings = serde_json::from_value(patch).unwrap();
        crate::restore_session_custom_credential_after_patch(&full, &mut after_patch, false);
        assert_eq!(after_patch.ui_language, "en");
        assert!(after_patch.session_custom_credential_rejected);
        assert_eq!(after_patch.asr_credential(), "");
        assert_eq!(
            after_patch.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        save_configuration(&dir, &after_patch).unwrap();
        assert!(backend.writes.borrow().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn omitted_full_settings_secret_preserves_unavailable_deferred_source_without_keychain_write() {
        let dir = temp_dir("full-settings-deferred-unavailable");
        let backend = FakeCredentialBackend::default();
        let mut previous = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            asr_api_key: "synthetic-deferred-key".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        previous.unverified_credential_sources.insert(
            "settings:asr_api_key".into(),
            "synthetic-deferred-key".into(),
        );
        previous
            .credential_baselines
            .insert("asr_api_key".into(), "synthetic-prior-baseline".into());
        previous
            .credential_read_states
            .insert("provider_custom".into(), CredentialReadState::Unavailable);
        previous
            .credential_read_states
            .insert("asr_api_key".into(), CredentialReadState::Unavailable);

        let mut full: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        full.asr_api_key.clear();
        crate::preserve_session_custom_credential_for_full_settings_roundtrip(&previous, &mut full);
        full.asr_api_key = bind_asr_key_to_host(
            full.asr_provider,
            &previous.asr_base_url,
            &full.asr_base_url,
            &full.asr_api_key,
            &previous.asr_api_key,
        );
        assert_eq!(full.asr_api_key, "synthetic-deferred-key");
        assert_eq!(
            full.credential_read_states.get("provider_custom"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.unverified_credential_sources
                .get("settings:asr_api_key")
                .map(String::as_str),
            Some("synthetic-deferred-key")
        );

        // Ordinary preference persistence must not touch secure storage.
        save_configuration(&dir, &full).unwrap();
        let persisted_before = fs::read(dir.join("settings.json")).unwrap();
        assert!(backend.writes.borrow().is_empty());

        // The compatibility service-save path sees the original unavailable
        // read state and refuses migration without writing or relabeling it.
        backend.script_reads(
            "asr_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic Keychain unavailable".into(),
            )],
        );
        assert!(save_settings_with_backend(&dir, &full, &backend).is_err());
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(
            fs::read(dir.join("settings.json")).unwrap(),
            persisted_before
        );
        assert_eq!(
            full.credential_read_states.get("asr_api_key"),
            Some(&CredentialReadState::Unavailable)
        );
        assert_eq!(
            full.unverified_credential_sources
                .get("settings:asr_api_key")
                .map(String::as_str),
            Some("synthetic-deferred-key")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn explicit_full_settings_custom_key_clears_revocation_and_persists() {
        let dir = temp_dir("explicit-custom-after-revocation");
        let backend = FakeCredentialBackend::default();
        let previous = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            session_custom_credential_rejected: true,
            ..Settings::default()
        };
        let mut incoming: Settings =
            serde_json::from_value(serde_json::to_value(&previous).unwrap()).unwrap();
        incoming
            .provider_api_keys
            .insert("custom".into(), "synthetic-explicit-new-key".into());
        crate::preserve_session_custom_credential_for_full_settings_roundtrip(
            &previous,
            &mut incoming,
        );
        assert!(!incoming.session_custom_credential_rejected);
        assert!(!incoming.session_only_custom_credential);
        assert_eq!(
            incoming.provider_secret(crate::engine::EngineProvider::Custom),
            "synthetic-explicit-new-key"
        );

        save_settings_with_backend(&dir, &incoming, &backend).unwrap();
        assert_eq!(
            backend.writes.borrow().as_slice(),
            &[(
                "provider_custom".into(),
                "synthetic-explicit-new-key".into()
            )]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn revoked_session_custom_credential_cannot_fall_back_to_a_legacy_alias() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            asr_api_key: "synthetic-legacy-a-key".into(),
            cleanup_api_key: "synthetic-legacy-cleanup-key".into(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut settings,
            "synthetic-session-origin-key".into(),
            scope,
        );
        assert_eq!(settings.asr_credential(), "synthetic-session-origin-key");

        settings.asr_base_url = "https://router-b.example.test/v1".into();
        assert!(settings.clear_stale_session_custom_credential());
        assert!(settings.session_custom_credential_rejected);
        assert_eq!(settings.asr_credential(), "");
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Custom),
            ""
        );
    }

    #[test]
    fn session_custom_value_is_limited_to_the_captured_active_purpose_set() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router.example.test/v1/asr".into(),
            cleanup_enabled: true,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://router.example.test/v1/cleanup".into(),
            custom_base_url: "https://router.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(
            &mut settings,
            "synthetic-shared-custom-key".into(),
            scope,
        );
        assert_eq!(settings.asr_credential(), "synthetic-shared-custom-key");
        assert_eq!(settings.cleanup_credential(), "synthetic-shared-custom-key");

        settings.cleanup_provider = crate::engine::EngineProvider::Groq;
        assert!(settings.clear_stale_session_custom_credential());
        assert_eq!(settings.asr_credential(), "");
        assert_eq!(settings.cleanup_credential(), "");
    }

    #[test]
    fn recovered_custom_credential_does_not_cross_to_a_different_accurate_asr_origin() {
        let mut settings = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://router-a.example.test/v1".into(),
            accurate_asr_provider: crate::engine::EngineProvider::Custom,
            accurate_asr_base_url: "https://router-b.example.test/v1".into(),
            ..Settings::default()
        };
        let scope =
            custom_credential_recovery_scope(&settings, CustomCredentialPurpose::Asr).unwrap();
        adopt_interactive_custom_credential(&mut settings, "synthetic-router-a-key".into(), scope);
        assert_eq!(settings.asr_credential(), "synthetic-router-a-key");
        assert_eq!(settings.accurate_asr_credential(), "");

        settings.accurate_asr_base_url = "https://router-a.example.test/alternate".into();
        assert_eq!(settings.accurate_asr_credential(), "synthetic-router-a-key");
    }

    #[test]
    fn on_device_mlx_cleanup_is_ready_without_url_or_api_key() {
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::OnDeviceMlx,
            cleanup_model: crate::engine::EngineProvider::OnDeviceMlx
                .default_llm_model()
                .into(),
            cleanup_base_url: String::new(),
            cleanup_api_key: String::new(),
            ..Settings::default()
        };
        settings
            .validate_provider_side(
                crate::engine::EngineProvider::OnDeviceMlx,
                false,
                None,
                true,
            )
            .expect("the bundled native provider has no URL or API key");
        assert_eq!(settings.cleanup_endpoint(), "");
        assert_eq!(settings.cleanup_credential(), "");
    }

    #[test]
    fn schema_27_moves_retired_lmstudio_cleanup_to_the_bundled_mlx_route() {
        let dir = temp_dir("retired-lmstudio-cleanup");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 26,
                "cleanup_enabled": true,
                "cleanup_provider": "lmstudio",
                "cleanup_model": "qwen/qwen3.8-27b@6bit",
                "strict_offline_enabled": true,
            }))
            .unwrap(),
        )
        .unwrap();
        let backend = FakeCredentialBackend::default();

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert!(needs_persist);
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::OnDeviceMlx
        );
        assert_eq!(settings.cleanup_model, crate::modelstore::MODEL_ID);
        assert!(settings.cleanup_enabled);
        assert!(settings.strict_offline_enabled);
        assert!(dir.join("settings.json.pre-migration.bak").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_27_repairs_only_pre_migration_mlx_selections_of_the_lmstudio_model() {
        let mut legacy = Settings {
            schema_version: 26,
            cleanup_provider: crate::engine::EngineProvider::OnDeviceMlx,
            cleanup_model: RETIRED_LMSTUDIO_CLEANUP_MODEL.into(),
            ..Settings::default()
        };
        legacy.normalize();
        assert_eq!(legacy.cleanup_model, crate::modelstore::MODEL_ID);

        let mut ollama = Settings {
            schema_version: 26,
            cleanup_provider: crate::engine::EngineProvider::Ollama,
            cleanup_model: RETIRED_LMSTUDIO_CLEANUP_MODEL.into(),
            ..Settings::default()
        };
        ollama.normalize();
        assert_eq!(ollama.cleanup_model, RETIRED_LMSTUDIO_CLEANUP_MODEL);

        assert!(
            serde_json::from_value::<crate::engine::EngineProvider>(serde_json::json!("lmstudio"))
                .is_err()
        );
        assert!(serde_json::from_value::<Settings>(serde_json::json!({
            "cleanup_provider": "not_a_provider"
        }))
        .is_err());
    }

    #[test]
    fn history_schema_migration_keeps_a_versioned_backup() {
        let dir = temp_dir("history-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
        .execute_batch(
                "CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); INSERT INTO dictations (created_at,raw_text,final_text) VALUES (datetime('now'),'legacy raw','legacy final'); PRAGMA user_version=2;",
            )
            .unwrap();
        drop(connection);

        let history = get_history(&dir, 10).unwrap();

        let backup = dir.join("history.sqlite.v2.bak");
        assert!(backup.is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backup_db =
            Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let integrity: String = backup_db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        assert_eq!(
            backup_db
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            2
        );
        assert_eq!(history[0].asr_text, None);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn clear_all_data_removes_backups_created_while_migrating_legacy_history() {
        let dir = temp_dir("clear-legacy-migration-backups");
        fs::create_dir_all(&dir).unwrap();
        let connection = Connection::open(dir.join("history.sqlite")).unwrap();
        connection.execute_batch("CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, engine TEXT, degraded INTEGER, status TEXT, raw_audio_path TEXT); INSERT INTO dictations (created_at,raw_text,final_text) VALUES (datetime('now'),'private legacy','private legacy'); PRAGMA user_version=2;").unwrap();
        drop(connection);
        fs::write(dir.join("history.sqlite.v1.bak"), "private previous").unwrap();
        fs::write(dir.join("history.sqlite.v2.bak.user"), "user export").unwrap();
        fs::write(dir.join("settings.json"), "user settings").unwrap();
        clear_all_data(&dir).unwrap();
        assert!(!dir.join("history.sqlite.v1.bak").exists());
        assert!(!dir.join("history.sqlite.v2.bak").exists());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(dir.join("history.sqlite.v2.bak.user")).unwrap(),
            "user export"
        );
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            "user settings"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn schema_17_adds_nullable_cleanup_attribution_and_keeps_existing_rows() {
        let dir = temp_dir("schema-17-cleanup-attribution");
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("history.sqlite");
        let connection = Connection::open(&db_path).unwrap();
        connection.execute_batch("CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, duration_secs REAL, raw_text TEXT, final_text TEXT, cleanup_status TEXT, engine TEXT, degraded INTEGER, degraded_reason TEXT, status TEXT, raw_audio_path TEXT, delivery_method TEXT, fallback_reason TEXT, context_profile_id TEXT, context_policy_json TEXT, asr_text TEXT, provider_cleaned_candidate TEXT, cleanup_rejected_candidate TEXT, cleanup_guard_reason TEXT); CREATE TABLE dictation_revisions (revision_id INTEGER PRIMARY KEY, dictation_id INTEGER NOT NULL, created_at TEXT NOT NULL, final_text TEXT NOT NULL, cleanup_status TEXT, intent_json TEXT, model TEXT, context_policy_json TEXT, revision_reason TEXT NOT NULL); INSERT INTO dictations(id,created_at,duration_secs,raw_text,final_text,cleanup_status,engine,degraded,degraded_reason,status,raw_audio_path,delivery_method,fallback_reason,context_profile_id,context_policy_json,asr_text,provider_cleaned_candidate,cleanup_rejected_candidate,cleanup_guard_reason) VALUES(41,'2026-11-01',3.5,'old raw','old final','local_only','whisper',1,'provider_failure','degraded','spool/session.wav','clipboard','delivery_fallback','profile-7','{\"family\":\"document\"}','old asr','provider candidate','rejected candidate','protected_token_changed'); INSERT INTO dictation_revisions(revision_id,dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) VALUES(9,41,'2026-11-02','old revision','local_only','{}','model-7','{\"family\":\"document\"}','manual'); PRAGMA user_version=17;").unwrap();
        let old_columns: Vec<String> = {
            let mut columns_statement =
                connection.prepare("PRAGMA table_info(dictations)").unwrap();
            columns_statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_attempt_kind"));
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_transformations_json"));
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_final_guard_reason"));
        assert!(!old_columns
            .iter()
            .any(|column| column == "cleanup_failure_reason"));
        drop(connection);
        let item = get_history_page(&dir, 10, None, None)
            .unwrap()
            .items
            .remove(0);
        assert_eq!(item.raw_text, "old raw");
        assert_eq!(item.asr_text.as_deref(), Some("old asr"));
        assert_eq!(
            item.provider_cleaned_candidate.as_deref(),
            Some("provider candidate")
        );
        assert_eq!(
            item.cleanup_rejected_candidate.as_deref(),
            Some("rejected candidate")
        );
        assert_eq!(
            item.cleanup_guard_reason.as_deref(),
            Some("protected_token_changed")
        );
        assert_eq!(item.context_profile_id.as_deref(), Some("profile-7"));
        let persisted_audio_path: Option<String> = Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT raw_audio_path FROM dictations WHERE id=41",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(persisted_audio_path.as_deref(), Some("spool/session.wav"));
        assert_eq!(item.delivery_method.as_deref(), Some("clipboard"));
        assert_eq!(item.cleanup_attempt_kind, None);
        assert_eq!(item.cleanup_transformations, None);
        assert_eq!(item.revision_count, 1);
        let migrated: Connection = open_history(&dir).unwrap();
        assert_eq!(
            migrated
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
                .unwrap(),
            HISTORY_SCHEMA_VERSION
        );
        migrated.execute("INSERT INTO dictations(id,created_at,raw_text,final_text,cleanup_status,status,cleanup_attempt_kind,cleanup_transformations_json,cleanup_final_guard_reason,cleanup_failure_reason) VALUES(42,'2026-11-03','new raw','new final','cleanup_not_requested','ok','not_called','[]',NULL,NULL)", []).unwrap();
        drop(migrated);
        let reopened = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(
            reopened.items[0].cleanup_attempt_kind.as_deref(),
            Some("not_called")
        );
        assert_eq!(
            reopened.items[0].cleanup_transformations.as_deref(),
            Some([].as_slice())
        );
        assert_eq!(reopened.items[0].cleanup_final_guard_reason, None);
        assert_eq!(reopened.items[0].cleanup_failure_reason, None);
        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(export[0]["cleanup_attempt_kind"], "not_called");
        assert_eq!(export[0]["cleanup_transformations"], serde_json::json!([]));
        assert!(export[0]["cleanup_final_guard_reason"].is_null());
        assert!(export[0]["cleanup_failure_reason"].is_null());
        assert!(export[1]["cleanup_attempt_kind"].is_null());
        let backup = dir.join("history.sqlite.v17.bak");
        assert!(backup.is_file());
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let backup_db =
            Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            backup_db
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            backup_db
                .query_row(
                    "SELECT final_text FROM dictation_revisions WHERE revision_id=9",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "old revision"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn newer_history_schema_is_rejected_without_changing_database_bytes() {
        let dir = temp_dir("future-history-schema");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE future_only(id INTEGER PRIMARY KEY, sentinel TEXT); INSERT INTO future_only VALUES(1,'keep'); CREATE TABLE dictations(id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, raw_text TEXT, final_text TEXT); INSERT INTO dictations VALUES(2,'2026-11-01','keep raw','keep final');").unwrap();
        connection
            .pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION + 1)
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(open_history(&dir).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!dir
            .join(format!("history.sqlite.v{}.bak", HISTORY_SCHEMA_VERSION))
            .exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backups_have_finite_retention_even_with_forever_live_history() {
        let dir = temp_dir("migration-backup-retention");
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for (version, days) in [(1, 8), (2, 3), (3, 0)] {
            let file = File::create(dir.join(format!("history.sqlite.v{version}.bak"))).unwrap();
            file.set_times(
                fs::FileTimes::new().set_modified(now - Duration::from_secs(days * 86_400)),
            )
            .unwrap();
        }
        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        assert!(!dir.join("history.sqlite.v1.bak").exists());
        assert!(dir.join("history.sqlite.v2.bak").exists());
        assert!(dir.join("history.sqlite.v3.bak").exists());
        purge_history(&dir, 1).unwrap();
        assert!(!dir.join("history.sqlite.v2.bak").exists());
        assert!(dir.join("history.sqlite.v3.bak").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_cleanup_ignores_future_dates_and_nonowned_names() {
        let dir = temp_dir("migration-backup-boundary");
        fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for name in [
            "history.sqlite.v1.bak",
            "history.sqlite.v02.bak",
            "history.sqlite.v-1.bak",
            "settings.json.pre-migration.bak",
            "history.sqlite.v2.bak.user",
        ] {
            fs::write(dir.join(name), "private").unwrap();
        }
        File::open(dir.join("history.sqlite.v1.bak"))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(now + Duration::from_secs(86_400)))
            .unwrap();
        fs::create_dir(dir.join("history.sqlite.v3.bak")).unwrap();
        assert_eq!(
            prune_history_migration_backups(&dir, Some(7), now).unwrap(),
            0
        );
        assert_eq!(prune_history_migration_backups(&dir, None, now).unwrap(), 1);
        assert!(dir.join("history.sqlite.v02.bak").exists());
        assert!(dir.join("history.sqlite.v-1.bak").exists());
        assert!(dir.join("settings.json.pre-migration.bak").exists());
        assert!(dir.join("history.sqlite.v2.bak.user").exists());
        assert!(dir.join("history.sqlite.v3.bak").is_dir());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn migration_backup_links_are_neither_followed_nor_overwritten() {
        use std::os::unix::fs::symlink;
        let dir = temp_dir("migration-backup-links");
        fs::create_dir_all(&dir).unwrap();
        let external = dir.join("user-backup.txt");
        fs::write(&external, "user-owned private backup").unwrap();
        symlink(&external, dir.join("history.sqlite.v1.bak")).unwrap();
        let dangling_target = dir.join("must-not-create.txt");
        symlink(&dangling_target, dir.join("history.sqlite.v2.bak")).unwrap();
        assert_eq!(
            prune_history_migration_backups(&dir, None, SystemTime::now()).unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(&external).unwrap(),
            "user-owned private backup"
        );
        let connection = Connection::open(dir.join("history.sqlite")).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        drop(connection);
        assert!(open_history(&dir).is_err());
        assert!(!dangling_target.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_does_not_overwrite_existing_regular_file() {
        let dir = temp_dir("migration-backup-existing-file");
        fs::create_dir_all(&dir).unwrap();
        let database = Connection::open(dir.join("history.sqlite")).unwrap();
        database.execute_batch("PRAGMA user_version=2;").unwrap();
        drop(database);
        let backup = dir.join("history.sqlite.v2.bak");
        fs::write(&backup, b"preexisting owner data").unwrap();

        open_history(&dir).unwrap();

        assert_eq!(fs::read(&backup).unwrap(), b"preexisting owner data");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_temp_collision_preserves_existing_directory_contents() {
        let dir = temp_dir("migration-temp-collision");
        fs::create_dir_all(&dir).unwrap();
        let occupied = dir.join("occupied-stage");
        fs::create_dir(&occupied).unwrap();
        let sentinel = occupied.join("owner-data.txt");
        fs::write(&sentinel, b"preexisting owner data").unwrap();

        assert!(create_history_backup_temp_dir(&occupied).is_err());

        assert_eq!(fs::read(&sentinel).unwrap(), b"preexisting owner data");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_backup_is_integral_during_concurrent_wal_writes() {
        let dir = temp_dir("migration-backup-wal-writer");
        fs::create_dir_all(&dir).unwrap();
        let database_path = dir.join("history.sqlite");
        let database = Connection::open(&database_path).unwrap();
        database
            .execute_batch(
                "PRAGMA journal_mode=WAL; CREATE TABLE dictations (id INTEGER PRIMARY KEY, value TEXT NOT NULL); INSERT INTO dictations VALUES (1, 'initial'); PRAGMA user_version=2;",
            )
            .unwrap();
        let writer_path = database_path.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let writer_db = Connection::open(writer_path).unwrap();
            writer_db.busy_timeout(Duration::from_secs(2)).unwrap();
            started_tx.send(()).unwrap();
            let mut revision = 0_u64;
            while stop_rx.try_recv().is_err() {
                revision += 1;
                let _ = writer_db.execute(
                    "UPDATE dictations SET value=?1 WHERE id=1",
                    [revision.to_string()],
                );
                std::thread::yield_now();
            }
        });
        started_rx.recv().unwrap();

        let backup = dir.join("history.sqlite.v2.bak");
        create_history_migration_backup(&database, &dir, &backup).unwrap();

        stop_tx.send(()).unwrap();
        writer.join().unwrap();
        let backup_db =
            Connection::open_with_flags(&backup, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let integrity: String = backup_db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        let value: String = backup_db
            .query_row("SELECT value FROM dictations WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!value.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_settings_never_writes_plaintext_key() {
        let dir = temp_dir("keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            api_key: "test_key_should_not_be_on_disk".into(),
            provider_api_keys: [("openai".to_owned(), "synthetic-openai-key".to_owned())]
                .into_iter()
                .collect(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("test_key_should_not_be_on_disk"),
            "plaintext API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["api_key"].as_str().unwrap_or(""), "");
        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("test_key_should_not_be_on_disk")
        );
        assert_eq!(
            backend
                .values
                .borrow()
                .get("provider_openai")
                .map(String::as_str),
            Some("synthetic-openai-key")
        );
        assert!(!on_disk.contains("synthetic-openai-key"));
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        assert!(!secret_sidecar_path(&dir, "provider_openai").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secure_store_failure_preserves_sources_and_creates_no_plaintext() {
        let dir = temp_dir("credential-write-failure");
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        let previous_settings = br#"{"api_key":"legacy-original","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), previous_settings).unwrap();
        write_secret_sidecar(&dir, "api_key", "legacy-sidecar").unwrap();
        let previous_sidecar = std::fs::read(secret_sidecar_path(&dir, "api_key")).unwrap();
        let backend = FakeCredentialBackend::default();
        backend.fail_write("groq_api_key", "synthetic secure-store failure");
        let settings = Settings {
            api_key: "new-synthetic-key".into(),
            ..Settings::default()
        };

        let error = save_settings_with_backend(&dir, &settings, &backend).unwrap_err();

        assert!(error.to_string().contains("credential_storage"));
        assert_eq!(
            std::fs::read(dir.join("settings.json")).unwrap(),
            previous_settings
        );
        assert_eq!(
            std::fs::read(secret_sidecar_path(&dir, "api_key")).unwrap(),
            previous_sidecar
        );
        assert!(
            !std::fs::read_to_string(secret_sidecar_path(&dir, "api_key"))
                .unwrap()
                .contains("new-synthetic-key")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_baseline_key_is_not_rewritten_when_keychain_is_unavailable() {
        let dir = temp_dir("same-baseline-unavailable");
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "groq_api_key",
            [crate::keychain::ApiKeyState::Unavailable(
                "synthetic unavailable".into(),
            )],
        );
        let mut settings = Settings::default();
        adopt_interactive_groq_credential(&mut settings, "same-baseline-key".into());

        let result = save_settings_with_backend(&dir, &settings, &backend);

        assert!(result.is_err());
        assert!(backend.writes.borrow().is_empty());
        assert!(!dir.join("settings.json").exists());
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn interactive_groq_read_adoption_updates_only_ram_baseline() {
        let mut settings = Settings::default();
        settings
            .unverified_credential_sources
            .insert("settings:api_key".into(), "legacy-key".into());

        adopt_interactive_groq_credential(&mut settings, "session-key".into());

        assert_eq!(settings.api_key, "session-key");
        assert_eq!(
            settings.provider_api_keys.get("groq").unwrap(),
            "session-key"
        );
        assert_eq!(
            settings.credential_baselines.get("groq_api_key").unwrap(),
            "session-key"
        );
        assert!(!settings
            .unverified_credential_sources
            .contains_key("settings:api_key"));
        let serialized = serde_json::to_string(&SettingsView::from(&settings)).unwrap();
        assert!(!serialized.contains("session-key"));
    }

    #[test]
    fn interactive_required_provider_read_adoption_never_exposes_key_in_settings_view() {
        let mut settings = Settings::default();
        settings
            .unverified_credential_sources
            .insert("settings:provider:openai".into(), "legacy-key".into());
        settings
            .unverified_credential_sources
            .insert("sidecar:provider_openai".into(), "stale-sidecar-key".into());

        adopt_interactive_provider_credential(
            &mut settings,
            crate::providers::EngineProvider::OpenAi,
            "fixture-session-key".into(),
        );

        assert_eq!(
            settings.provider_api_keys.get("openai").map(String::as_str),
            Some("fixture-session-key")
        );
        assert_eq!(
            settings
                .credential_baselines
                .get("provider_openai")
                .map(String::as_str),
            Some("fixture-session-key")
        );
        assert_eq!(
            settings.credential_read_states.get("provider_openai"),
            Some(&CredentialReadState::Configured)
        );
        assert!(!settings
            .unverified_credential_sources
            .contains_key("settings:provider:openai"));
        assert!(!settings
            .unverified_credential_sources
            .contains_key("sidecar:provider_openai"));
        let serialized = serde_json::to_string(&SettingsView::from(&settings)).unwrap();
        assert!(!serialized.contains("fixture-session-key"));
    }

    #[test]
    fn verified_settings_migration_scrubs_settings_only_after_secure_readback() {
        let dir = temp_dir("settings-credential-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let legacy_settings =
            br#"{"schema_version":20,"api_key":"synthetic-legacy-key","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), legacy_settings).unwrap();
        let backend = FakeCredentialBackend::default();

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert_eq!(settings.api_key, "synthetic-legacy-key");
        assert!(needs_persist);
        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("synthetic-legacy-key")
        );
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["api_key"].as_str(), Some(""));
        assert!(!std::fs::read_to_string(dir.join("settings.json"))
            .unwrap()
            .contains("synthetic-legacy-key"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verified_sidecar_migration_removes_only_matching_legacy_file() {
        let dir = temp_dir("sidecar-migration");
        write_secret_sidecar(&dir, "api_key", "synthetic-groq-legacy").unwrap();
        write_secret_sidecar(&dir, "provider_openai", "unrelated-openai-sidecar").unwrap();
        let backend = FakeCredentialBackend::default();
        let mut settings = Settings::default();

        let resolution =
            resolve_legacy_credential(&dir, "groq_api_key", "", None, &mut settings, &backend);

        assert_eq!(resolution.value, "synthetic-groq-legacy");
        assert!(matches!(
            resolution.state,
            crate::keychain::ApiKeyState::Configured(ref key) if key == "synthetic-groq-legacy"
        ));
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        assert_eq!(
            read_secret_sidecar(&dir, "provider_openai").as_deref(),
            Some("unrelated-openai-sidecar")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn provider_and_custom_alias_sidecars_migrate_to_their_secure_accounts() {
        let provider_dir = temp_dir("provider-sidecar-migration");
        write_secret_sidecar(&provider_dir, "provider_openai", "synthetic-provider-key").unwrap();
        let provider_backend = FakeCredentialBackend::default();
        let mut provider_settings = Settings::default();
        let provider_resolution = resolve_legacy_credential(
            &provider_dir,
            "provider_openai",
            "",
            None,
            &mut provider_settings,
            &provider_backend,
        );
        assert_eq!(provider_resolution.value, "synthetic-provider-key");
        assert_eq!(
            provider_backend
                .values
                .borrow()
                .get("provider_openai")
                .map(String::as_str),
            Some("synthetic-provider-key")
        );
        assert!(!secret_sidecar_path(&provider_dir, "provider_openai").exists());

        let custom_dir = temp_dir("custom-alias-sidecar-migration");
        write_secret_sidecar(&custom_dir, "asr_api_key", "synthetic-custom-asr").unwrap();
        write_secret_sidecar(&custom_dir, "cleanup_api_key", "synthetic-custom-cleanup").unwrap();
        let custom_backend = FakeCredentialBackend::default();
        let (custom_settings, _) = load_settings_with_backend(&custom_dir, &custom_backend);
        assert_eq!(custom_settings.asr_api_key, "synthetic-custom-asr");
        assert_eq!(custom_settings.cleanup_api_key, "synthetic-custom-cleanup");
        assert_eq!(
            custom_backend
                .values
                .borrow()
                .get("asr_api_key")
                .map(String::as_str),
            Some("synthetic-custom-asr")
        );
        assert_eq!(
            custom_backend
                .values
                .borrow()
                .get("cleanup_api_key")
                .map(String::as_str),
            Some("synthetic-custom-cleanup")
        );
        assert!(!secret_sidecar_path(&custom_dir, "asr_api_key").exists());
        assert!(!secret_sidecar_path(&custom_dir, "cleanup_api_key").exists());
        let _ = std::fs::remove_dir_all(provider_dir);
        let _ = std::fs::remove_dir_all(custom_dir);
    }

    #[test]
    fn failed_or_unverified_legacy_migration_retains_settings_and_sidecar_sources() {
        let dir = temp_dir("failed-credential-migration");
        std::fs::create_dir_all(&dir).unwrap();
        let raw =
            br#"{"schema_version":22,"api_key":"synthetic-settings-source","onboarded":false}"#;
        std::fs::write(dir.join("settings.json"), raw).unwrap();
        write_secret_sidecar(&dir, "api_key", "synthetic-sidecar-source").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.script_reads(
            "groq_api_key",
            [
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
            ],
        );
        backend.fail_write("groq_api_key", "synthetic write failure");

        let (settings, needs_persist) = load_settings_with_backend(&dir, &backend);

        assert_eq!(settings.api_key, "synthetic-settings-source");
        // Schema 23 requests a save, but a failed credential migration must
        // still prevent overwriting the original source configuration.
        assert!(needs_persist);
        assert!(save_settings_with_backend(&dir, &settings, &backend).is_err());
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), raw);
        assert_eq!(
            read_secret_sidecar(&dir, "api_key").as_deref(),
            Some("synthetic-sidecar-source")
        );
        assert!(settings
            .unverified_credential_sources
            .contains_key("settings:api_key"));

        let unverifiable_dir = temp_dir("unverified-credential-migration");
        write_secret_sidecar(&unverifiable_dir, "api_key", "synthetic-unverified-sidecar").unwrap();
        let unverifiable_backend = FakeCredentialBackend::default();
        unverifiable_backend.script_reads(
            "groq_api_key",
            [
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
                crate::keychain::ApiKeyState::Missing,
            ],
        );
        let mut unresolved = Settings::default();
        let result = resolve_legacy_credential(
            &unverifiable_dir,
            "groq_api_key",
            "",
            None,
            &mut unresolved,
            &unverifiable_backend,
        );
        assert_eq!(result.value, "synthetic-unverified-sidecar");
        assert!(matches!(
            result.state,
            crate::keychain::ApiKeyState::Unavailable(_)
        ));
        assert_eq!(
            read_secret_sidecar(&unverifiable_dir, "api_key").as_deref(),
            Some("synthetic-unverified-sidecar")
        );
        assert!(unresolved
            .unverified_credential_sources
            .contains_key("sidecar:groq_api_key"));
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(unverifiable_dir);
    }

    #[test]
    fn existing_secure_credential_wins_over_stale_sidecar_without_removing_it() {
        let dir = temp_dir("secure-precedence");
        write_secret_sidecar(&dir, "api_key", "stale-sidecar-value").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.seed("groq_api_key", "newer-secure-value");
        let mut settings = Settings::default();

        let resolution =
            resolve_legacy_credential(&dir, "groq_api_key", "", None, &mut settings, &backend);

        assert_eq!(resolution.value, "newer-secure-value");
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(
            read_secret_sidecar(&dir, "api_key").as_deref(),
            Some("stale-sidecar-value")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_key_replacement_updates_secure_value_and_clears_legacy_alias() {
        let dir = temp_dir("explicit-credential-replacement");
        write_secret_sidecar(&dir, "groq_api_key", "synthetic-old-sidecar").unwrap();
        let backend = FakeCredentialBackend::default();
        backend.seed("groq_api_key", "synthetic-old-secure");
        let mut settings = Settings {
            api_key: "synthetic-new-key".into(),
            ..Settings::default()
        };
        settings
            .credential_baselines
            .insert("groq_api_key".into(), "synthetic-old-secure".into());

        save_settings_with_backend(&dir, &settings, &backend).unwrap();

        assert_eq!(
            backend
                .values
                .borrow()
                .get("groq_api_key")
                .map(String::as_str),
            Some("synthetic-new-key")
        );
        assert!(!secret_sidecar_path(&dir, "api_key").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explicit_provider_deletion_clears_groq_and_custom_legacy_aliases() {
        let dir = temp_dir("credential-alias-delete");
        for slot in [
            "api_key",
            "provider_custom",
            "asr_api_key",
            "cleanup_api_key",
        ] {
            write_secret_sidecar(&dir, slot, &format!("synthetic-{slot}")).unwrap();
        }

        clear_provider_key_sidecars(&dir, crate::engine::EngineProvider::Groq);
        assert!(!secret_sidecar_path(&dir, "groq_api_key").exists());
        clear_provider_key_sidecars(&dir, crate::engine::EngineProvider::Custom);
        for slot in ["provider_custom", "asr_api_key", "cleanup_api_key"] {
            assert!(!secret_sidecar_path(&dir, slot).exists());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    fn write_custom_host_settings_fixture(dir: &Path) -> Vec<u8> {
        let original = br#"{"custom_base_url":"https://old.example/v1"}"#.to_vec();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("settings.json"), &original).unwrap();
        original
    }

    fn seed_custom_host_credentials(backend: &FakeCredentialBackend) {
        backend.seed("provider_custom", "old-custom-key");
        backend.seed("asr_api_key", "old-asr-alias");
        backend.seed("cleanup_api_key", "old-cleanup-alias");
        backend.seed("api_key", "keep-groq-key");
        backend.seed("provider_openai", "keep-openai-key");
    }

    fn assert_custom_host_settings_unchanged(dir: &Path, original: &[u8]) {
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), original);
    }

    #[test]
    fn custom_host_change_clear_failure_keeps_settings_and_unrelated_keys() {
        for failing_slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
            let dir = temp_dir(&format!("custom-host-clear-{failing_slot}"));
            let original = write_custom_host_settings_fixture(&dir);
            let backend = FakeCredentialBackend::default();
            seed_custom_host_credentials(&backend);
            backend.fail_write(failing_slot, "injected delete failure");

            assert!(
                clear_custom_credentials_before_host_change_with_backend(&dir, &backend).is_err()
            );
            assert_custom_host_settings_unchanged(&dir, &original);
            assert_eq!(
                backend.read("api_key"),
                crate::keychain::ApiKeyState::Configured("keep-groq-key".into())
            );
            assert_eq!(
                backend.read("provider_openai"),
                crate::keychain::ApiKeyState::Configured("keep-openai-key".into())
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn custom_host_change_clear_unavailable_readback_keeps_settings() {
        let dir = temp_dir("custom-host-clear-unavailable-readback");
        let original = write_custom_host_settings_fixture(&dir);
        let backend = FakeCredentialBackend::default();
        seed_custom_host_credentials(&backend);
        backend.script_reads(
            "provider_custom",
            [crate::keychain::ApiKeyState::Unavailable(
                "injected readback failure".into(),
            )],
        );

        assert!(clear_custom_credentials_before_host_change_with_backend(&dir, &backend).is_err());
        assert_custom_host_settings_unchanged(&dir, &original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn custom_host_change_clear_sidecar_failure_keeps_settings() {
        let dir = temp_dir("custom-host-clear-sidecar-failure");
        let original = write_custom_host_settings_fixture(&dir);
        let backend = FakeCredentialBackend::default();
        seed_custom_host_credentials(&backend);
        write_secret_sidecar(&dir, "provider_custom", "old-custom-sidecar").unwrap();
        write_secret_sidecar(&dir, "cleanup_api_key", "old-cleanup-sidecar").unwrap();
        let blocked_sidecar = secret_sidecar_path(&dir, "asr_api_key");
        std::fs::create_dir(&blocked_sidecar).unwrap();

        assert!(clear_custom_credentials_before_host_change_with_backend(&dir, &backend).is_err());
        assert_custom_host_settings_unchanged(&dir, &original);
        assert!(blocked_sidecar.is_dir());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn custom_host_change_clear_removes_all_custom_slots_before_verifying_fallback() {
        let dir = temp_dir("custom-host-clear-success");
        let original = write_custom_host_settings_fixture(&dir);
        let backend = FakeCredentialBackend::default();
        seed_custom_host_credentials(&backend);
        for slot in ["provider_custom", "asr_api_key", "cleanup_api_key"] {
            write_secret_sidecar(&dir, slot, &format!("old-{slot}-sidecar")).unwrap();
        }

        clear_custom_credentials_before_host_change_with_backend(&dir, &backend).unwrap();

        for slot in CUSTOM_HOST_CHANGE_CREDENTIAL_SLOTS {
            assert_eq!(backend.read(slot), crate::keychain::ApiKeyState::Missing);
            assert!(!secret_sidecar_path(&dir, slot).exists());
        }
        assert_eq!(
            backend.read("api_key"),
            crate::keychain::ApiKeyState::Configured("keep-groq-key".into())
        );
        assert_eq!(
            backend.read("provider_openai"),
            crate::keychain::ApiKeyState::Configured("keep-openai-key".into())
        );
        assert_custom_host_settings_unchanged(&dir, &original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_settings_reads_api_key_from_sidecar_when_keychain_is_empty() {
        let dir = temp_dir("sidecar-load");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"onboarded":true,"api_key":""}"#,
        )
        .unwrap();
        write_secret_sidecar(&dir, "api_key", "gsk_from_sidecar").unwrap();
        let (settings, _) = load_settings(&dir);
        assert_eq!(settings.api_key, "gsk_from_sidecar");
        assert!(settings.onboarded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn credential_hint_shows_last_five_characters() {
        assert_eq!(
            credential_hint("gsk_abcdefghij").as_deref(),
            Some("••••fghij")
        );
        assert_eq!(credential_hint("only").as_deref(), Some("••••"));
        assert_eq!(credential_hint("🔑秘密").as_deref(), Some("••••"));
        assert_eq!(credential_hint("asr_only").as_deref(), Some("••••_only"));
    }

    #[test]
    fn settings_view_redacts_api_key() {
        let settings = Settings {
            api_key: "test_key_1234".into(),
            ..Settings::default()
        };
        let view = SettingsView::from(&settings);
        let json = serde_json::to_string(&view).unwrap();
        assert!(view.api_key_configured);
        assert_eq!(view.api_key_hint.as_deref(), Some("••••_1234"));
        assert_eq!(view.asr_model, crate::asr::MODEL);
        assert_eq!(view.cleanup_model, crate::llm::MODEL);
        assert!(!json.contains("test_key_1234"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn custom_cleanup_parameters_round_trip_through_settings_view_and_validate() {
        let mut settings = Settings {
            custom_cleanup_parameters: serde_json::from_value(serde_json::json!({
                "temperature": 0,
                "vendor_extension": {"enabled": false, "bias": 0},
            }))
            .expect("parameter object"),
            ..Settings::default()
        };
        settings
            .validate_configuration()
            .expect("bounded parameters validate");
        let view = SettingsView::from(&settings);
        assert_eq!(
            view.custom_cleanup_parameters,
            settings.custom_cleanup_parameters
        );

        settings
            .custom_cleanup_parameters
            .insert("messages".into(), serde_json::Value::Array(Vec::new()));
        assert!(settings.validate_configuration().is_err());
    }

    #[test]
    fn settings_view_exposes_configured_asr_model() {
        let settings = Settings {
            asr_model: "whisper-1".into(),
            ..Settings::default()
        };
        assert_eq!(SettingsView::from(&settings).asr_model, "whisper-1");
    }

    #[test]
    fn onboarding_requires_the_selected_provider_credential() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            cleanup_enabled: false,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn onboarding_accepts_selected_provider_key_without_a_groq_key() {
        let mut settings = Settings {
            onboarded: true,
            api_key: String::new(),
            asr_provider: crate::engine::EngineProvider::SiliconFlow,
            asr_model: "FunAudioLLM/SenseVoiceSmall".into(),
            cleanup_provider: crate::engine::EngineProvider::SiliconFlow,
            cleanup_model: "Qwen/Qwen2.5-7B-Instruct".into(),
            ..Settings::default()
        };
        settings
            .provider_api_keys
            .insert("siliconflow".into(), "configured-in-memory".into());
        settings
            .validate()
            .expect("selected ASR/cleanup provider keys validate");
    }

    #[test]
    fn onboarding_accepts_keyless_loopback_asr_without_a_groq_key() {
        let settings = Settings {
            onboarded: true,
            api_key: String::new(),
            asr_provider: crate::engine::EngineProvider::LocalWhisper,
            cleanup_enabled: false,
            ..Settings::default()
        };
        settings
            .validate()
            .expect("loopback ASR has a valid keyless route");
    }

    #[test]
    fn onboarding_rejects_remote_keyless_cleanup_provider() {
        let mut settings = Settings {
            onboarded: true,
            cleanup_provider: crate::engine::EngineProvider::Ollama,
            ollama_base_url: "https://ollama.example/v1".into(),
            cleanup_model: "llama3.2".into(),
            ..Settings::default()
        };
        settings.api_key = "configured-groq-key".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn load_settings_keeps_onboarding_for_selected_provider_sidecar() {
        let dir = temp_dir("selected-provider-onboarding");
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SETTINGS_SCHEMA_VERSION,
                "onboarded": true,
                "asr_provider": "openai",
                "asr_model": "whisper-1",
                "cleanup_enabled": false,
                "api_key": ""
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            secret_sidecar_path(&dir, "provider_openai"),
            "selected-provider-secret",
        )
        .unwrap();

        let (settings, _) = load_settings(&dir);
        assert!(settings.onboarded);
        assert_eq!(settings.asr_provider, crate::engine::EngineProvider::OpenAi);
        assert_eq!(settings.asr_credential(), "selected-provider-secret");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_key_clears_stale_onboarded_state_for_persistence() {
        let dir = temp_dir("stale-onboarding");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"onboarded":true,"api_key":""}"#,
        )
        .unwrap();
        let (settings, needs_persist) = load_settings(&dir);
        assert!(!settings.onboarded);
        assert!(needs_persist);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_writes_are_atomic_and_path_bounded() {
        let dir = temp_dir("spool");
        let path = write_spool_file(&dir, Path::new("session/chunk.wav"), b"audio").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"audio");
        assert!(write_spool_file(&dir, Path::new("../escape.wav"), b"nope").is_err());
        assert!(!dir.join("escape.wav").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spool/session/chunk.wav"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
            let dir_mode = std::fs::metadata(dir.join("spool/session"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_audio_spool_lease_blocks_reuse_reads_and_defers_all_data_deletion() {
        let dir = temp_dir("audio-spool-lease");
        let (session, lease) = begin_audio_spool_session(&dir, "live-session").unwrap();
        assert!(
            !dir.join("spool").exists(),
            "audio reservation performs no filesystem initialization"
        );
        assert!(begin_audio_spool_session(&dir, "nested/session").is_err());
        let alias_root = dir.join("unused-child").join("..");
        assert!(begin_audio_spool_session(&alias_root, "live-session").is_err());
        let chunk = session.join("chunks/00000000.f32");

        assert!(begin_audio_spool_session(&dir, "live-session").is_err());
        assert!(read_spool_file(&chunk)
            .unwrap_err()
            .to_string()
            .contains("already owned by an audio worker"));
        assert!(recover_spool_session(&dir, "live-session", 7)
            .unwrap_err()
            .to_string()
            .contains("still owned"));
        assert!(
            write_spool_file(&dir, Path::new("live-session/chunks/late.f32"), b"late")
                .unwrap_err()
                .to_string()
                .contains("already owned by an audio worker")
        );

        assert!(clear_all_data(&dir)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        assert!(
            !lease.deletion_requested(),
            "a rejected data clear must not schedule the live recording for deletion"
        );
        assert!(
            !dir.join("spool").exists(),
            "clear does not create or mutate a path while initialization is reserved"
        );
        assert!(begin_audio_spool_session(&dir, "live-session").is_err());

        let release_started = std::time::Instant::now();
        drop(lease);
        assert!(release_started.elapsed() < Duration::from_millis(100));
        assert!(
            !session.exists(),
            "a memory-only lease drop does not create or delete files"
        );
        clear_all_data(&dir).unwrap();
        let (reused, lease) = begin_audio_spool_session(&dir, "live-session").unwrap();
        assert!(!reused.exists());
        initialize_audio_spool_session(&lease).unwrap();
        drop(lease);
        assert!(reused.join("manifest.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_spool_session_drop_cannot_release_a_successor_generation() {
        let dir = temp_dir("audio-spool-lease-aba");
        let session_dir = dir.join("spool").join("aba-session");
        let stale =
            reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).unwrap();
        let stale_generation = stale.generation();

        // Model a predecessor whose owner slot was revoked while an old clone
        // is still retained. A new generation is allowed to claim the exact
        // same logical session path.
        {
            let mut active = ACTIVE_AUDIO_SPOOL_SESSIONS
                .get_or_init(Default::default)
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert_eq!(
                active
                    .sessions
                    .get(&session_dir)
                    .map(|value| value.generation),
                Some(stale_generation)
            );
            active.sessions.remove(&session_dir);
        }
        let successor =
            reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).unwrap();
        assert_ne!(stale.generation(), successor.generation());
        drop(stale);

        assert!(spool_lease_is_current(&successor));
        assert!(reserve_spool_session_path(&session_dir, true, SpoolLeaseKind::Session).is_err());
        drop(successor);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn root_mutation_and_clear_admissions_are_prefix_scoped_and_memory_only() {
        let dir = temp_dir("audio-spool-root-admission");
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("preserve.bin");
        std::fs::write(&marker, b"clear busy must preserve this file").unwrap();
        let before = std::fs::read(&marker).unwrap();
        let nested_root = dir.join("nested").join("profile");

        let mutation = begin_spool_root_mutation(&nested_root).unwrap();
        let alias_root = nested_root.join("unused-child").join("..");
        assert!(begin_clear_all_data(&alias_root).is_err());
        assert!(clear_all_data(&dir)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        assert_eq!(std::fs::read(&marker).unwrap(), before);
        assert!(
            !nested_root.exists(),
            "admission must not create directories"
        );
        drop(mutation);

        let clear = begin_clear_all_data(&dir).unwrap();
        assert!(begin_spool_root_mutation(&nested_root).is_err());
        assert!(begin_audio_spool_session(&nested_root, "nested-session").is_err());
        assert_eq!(std::fs::read(&marker).unwrap(), before);
        drop(clear);

        let mutation_after_release = begin_spool_root_mutation(&nested_root).unwrap();
        drop(mutation_after_release);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_readers_and_retention_reject_an_active_clear_before_side_effects() {
        let dir = temp_dir("audio-spool-clear-fences-recovery");
        std::fs::create_dir_all(dir.join("spool").join("legacy")).unwrap();
        let audio = dir.join("spool").join("legacy").join("recovery.wav");
        std::fs::write(&audio, b"preserve until clear completes").unwrap();
        let before = std::fs::read(&audio).unwrap();
        let recovery = RecoveredSpool {
            audio_path: audio.clone(),
            duration_secs: 1.0,
        };

        let clear = begin_clear_all_data(&dir).unwrap();
        assert!(read_spool_file(&audio).is_err());
        assert!(recover_spool(&dir, 7).is_err());
        assert!(recover_spool_session(&dir, "legacy", 7).is_err());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(purge_history(&dir, 0).is_err());
        assert!(purge_gold_audio(&dir, 0).is_err());
        assert_eq!(std::fs::read(&audio).unwrap(), before);
        drop(clear);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_audio_spool_initialization_and_cancel_preserve_preexisting_artifacts() {
        use sha2::Digest;

        let dir = temp_dir("audio-spool-init-existing");
        let session = begin_spool_session(&dir, "preexisting-session").unwrap();
        let marker = session.join("marker.bin");
        std::fs::write(&marker, b"recovery artifact must remain untouched").unwrap();
        let before = sha2::Sha256::digest(std::fs::read(&marker).unwrap());

        let (reserved, lease) = begin_audio_spool_session(&dir, "preexisting-session").unwrap();
        assert_eq!(reserved, session);
        assert!(initialize_audio_spool_session(&lease).is_err());
        assert!(clear_all_data(&dir)
            .unwrap_err()
            .to_string()
            .contains("cleanup is still pending"));
        let release_started = std::time::Instant::now();
        drop(lease);
        assert!(release_started.elapsed() < Duration::from_millis(100));

        let after = sha2::Sha256::digest(std::fs::read(&marker).unwrap());
        assert_eq!(
            before, after,
            "failed initialization cannot erase recovery data"
        );
        assert!(session.join("manifest.json").exists());
        clear_all_data(&dir).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_writes_plaintext_and_rejects_retired_encrypted_envelopes() {
        let dir = temp_dir("plaintext-spool");
        let wav = b"RIFF recovery";
        write_spool_file_internal(&dir, Path::new("session/recovery.wav"), wav, None).unwrap();
        let on_disk = dir.join("spool/session/recovery.wav");
        assert_eq!(std::fs::read(&on_disk).unwrap(), wav);
        assert_eq!(read_spool_file(&on_disk).unwrap(), wav);

        let legacy = dir.join("legacy.wav");
        std::fs::write(&legacy, wav).unwrap();
        assert_eq!(read_spool_file(&legacy).unwrap(), wav);

        let mut envelope = SPOOL_ENVELOPE_MAGIC.to_vec();
        envelope.extend_from_slice(b"ciphertext");
        let managed = dir.join("spool/session/encrypted.wav");
        std::fs::write(&managed, &envelope).unwrap();
        assert!(read_spool_file(&managed).is_err());
        assert!(read_spool_file_limited(&managed, 1024).is_err());
        std::fs::write(&legacy, &envelope).unwrap();
        assert!(read_spool_file(&legacy).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spool_manifest_tracks_chunks_and_recovers_active_sessions() {
        let dir = temp_dir("manifest");
        let session = begin_spool_session(&dir, "session-test").unwrap();
        record_spool_chunk(&session, 0, 0.0, 15.0, "written").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "active");
        assert_eq!(manifest.chunks[0].status, "written");
        let recovered = recover_spool(&dir, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].audio_path.is_file());
        assert!(!read_spool_file(&recovered[0].audio_path)
            .unwrap()
            .is_empty());
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.status, "recoverable");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_error_recovery_touches_only_the_matching_spool_session() {
        let dir = temp_dir("scoped-live-recovery");
        let matching = begin_spool_session(&dir, "current-session").unwrap();
        write_spool_f32_chunk(&dir, "current-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&matching, 0, 0.0, 0.0001875, "written").unwrap();
        let unrelated = begin_spool_session(&dir, "other-session").unwrap();
        write_spool_f32_chunk(&dir, "other-session", 0, &[0.4, 0.5]);
        record_spool_chunk(&unrelated, 0, 0.0, 0.000125, "written").unwrap();

        let recovered = recover_spool_session(&dir, "current-session", 7)
            .unwrap()
            .expect("matching live audio should remain recoverable");

        assert!(recovered.audio_path.is_file());
        assert_eq!(load_manifest(&matching).unwrap().status, "recoverable");
        assert_eq!(load_manifest(&unrelated).unwrap().status, "active");
        assert!(!unrelated.join("recovery.wav").exists());
        assert!(recover_spool_session(&dir, "../other-session", 7).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_flat_symlink_does_not_interrupt_startup_spool_recovery() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("dangling-flat-link-recovery");
        let session = begin_spool_session(&dir, "valid-session-after-link").unwrap();
        write_spool_f32_chunk(&dir, "valid-session-after-link", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 0.0001875, "written").unwrap();
        symlink(
            dir.join("missing-target.wav"),
            dir.join("spool/dangling.wav"),
        )
        .unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].audio_path, session.join("recovery.wav"));
        assert!(recovered[0].audio_path.is_file());
        assert!(dir.join("spool/dangling.wav").symlink_metadata().is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_day_retention_never_recovers_a_same_second_live_spool() {
        let dir = temp_dir("zero-day-live-recovery");
        let session = begin_spool_session(&dir, "zero-day-session").unwrap();
        write_spool_f32_chunk(&dir, "zero-day-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 0.0001875, "written").unwrap();

        assert!(recover_spool_session(&dir, "zero-day-session", 0)
            .unwrap()
            .is_none());
        assert!(!session.join("recovery.wav").exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    fn set_spool_created_at_for_test(session: &Path, created_at_ms: u64) {
        let mut manifest = load_manifest(session).unwrap();
        manifest.created_at_ms = created_at_ms;
        save_manifest(session, &manifest).unwrap();
    }

    fn assert_spool_mtime_is_recent(session: &Path) {
        let elapsed = std::fs::metadata(session)
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap();
        assert!(
            elapsed.as_secs() < 60,
            "test must model a recent directory mtime"
        );
    }

    #[test]
    fn retention_tick_removes_expired_spool() {
        let dir = temp_dir("spool-daily-retention");
        let old = begin_spool_session(&dir, "expired-session").unwrap();
        let fresh = begin_spool_session(&dir, "fresh-session").unwrap();
        let completed = begin_spool_session(&dir, "leased-complete-session").unwrap();
        set_spool_created_at_for_test(&old, now_ms().saturating_sub(8 * 86_400_000));
        let mut completed_manifest = load_manifest(&completed).unwrap();
        completed_manifest.status = "complete".into();
        completed_manifest.created_at_ms = now_ms().saturating_sub(8 * 86_400_000);
        save_manifest(&completed, &completed_manifest).unwrap();
        let _completed_lease = lease_spool_session(&completed).unwrap();
        let flat_old = dir.join("spool/expired.wav");
        let flat_fresh = dir.join("spool/fresh.wav");
        std::fs::write(&flat_old, b"old").unwrap();
        std::fs::write(&flat_fresh, b"fresh").unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(8 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&flat_old)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        let injected_now = now_ms();
        set_spool_created_at_for_test(&fresh, injected_now.saturating_sub(3_000));
        let fresh_created_at = load_manifest(&fresh).unwrap().created_at_ms;

        expire_spool(&dir, 7, || injected_now).unwrap();

        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(completed.exists());
        assert!(!flat_old.exists());
        assert!(flat_fresh.exists());
        assert_eq!(
            load_manifest(&fresh).unwrap().created_at_ms,
            fresh_created_at
        );
        drop(_completed_lease);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovery_expiry_uses_session_creation_time_after_success_or_failure() {
        const KEEP_DAYS: u64 = 7;
        let dir = temp_dir("spool-origin-retention");

        let successful = begin_spool_session(&dir, "successful-old-session").unwrap();
        write_spool_f32_chunk(&dir, "successful-old-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&successful, 0, 0.0, 0.0001875, "written").unwrap();
        assert_eq!(recover_spool(&dir, KEEP_DAYS).unwrap().len(), 1);
        set_spool_created_at_for_test(
            &successful,
            now_ms().saturating_sub((KEEP_DAYS + 1) * 86_400_000),
        );
        assert_spool_mtime_is_recent(&successful);

        // A successful startup rewrites recovery.wav and manifest.json. Those
        // filesystem mtimes must not renew the immutable session retention age.
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert!(
            !successful.exists(),
            "expired source and recovery files are purged"
        );

        let failed = begin_spool_session(&dir, "failed-old-session").unwrap();
        write_spool_f32_chunk(&dir, "failed-old-session", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "failed-old-session", 1, &[0.9, 0.3]);
        record_spool_chunk(&failed, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&failed, 1, 0.5, 1.5, "written").unwrap();
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert_eq!(load_manifest(&failed).unwrap().status, "abandoned");
        set_spool_created_at_for_test(
            &failed,
            now_ms().saturating_sub((KEEP_DAYS + 1) * 86_400_000),
        );
        assert_spool_mtime_is_recent(&failed);

        // Marking a failed rebuild abandoned changes the directory mtime too,
        // but it does not start a new retention period.
        assert!(recover_spool(&dir, KEEP_DAYS).unwrap().is_empty());
        assert!(
            !failed.exists(),
            "expired unreconstructable chunks are purged"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_day_startup_retention_removes_fresh_sessions_and_legacy_files() {
        let dir = temp_dir("zero-day-spool-retention");
        let session = begin_spool_session(&dir, "fresh-zero-day-session").unwrap();
        write_spool_f32_chunk(&dir, "fresh-zero-day-session", 0, &[0.1, 0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 0.0001875, "written").unwrap();
        let flat = write_spool_file(&dir, Path::new("fresh-failed-job.wav"), b"legacy audio")
            .expect("flat legacy recovery artifact uses its exact file scope");

        assert!(recover_spool(&dir, 0).unwrap().is_empty());
        assert!(
            !session.exists(),
            "zero days removes even a same-second session"
        );
        assert!(
            !flat.exists(),
            "zero days removes fresh flat legacy artifacts too"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_recovery_keeps_immutable_creation_time_and_invalid_dates_expire() {
        const KEEP_DAYS: u64 = 7;
        let dir = temp_dir("spool-valid-retention-dates");

        let in_window = begin_spool_session(&dir, "in-window").unwrap();
        let created_at_ms = load_manifest(&in_window).unwrap().created_at_ms;
        write_spool_f32_chunk(&dir, "in-window", 0, &[0.1, 0.2]);
        record_spool_chunk(&in_window, 0, 0.0, 0.000125, "written").unwrap();
        set_spool_created_at_for_test(
            &in_window,
            now_ms().saturating_sub((KEEP_DAYS - 1) * 86_400_000),
        );
        let retained_created_at_ms = load_manifest(&in_window).unwrap().created_at_ms;
        assert_ne!(retained_created_at_ms, created_at_ms);
        assert_eq!(recover_spool(&dir, KEEP_DAYS).unwrap().len(), 1);
        assert_eq!(
            load_manifest(&in_window).unwrap().created_at_ms,
            retained_created_at_ms,
            "successful recovery must preserve its original creation timestamp"
        );
        assert!(in_window.exists());

        for (session_id, created_at_ms) in [
            ("zero-timestamp", 0),
            ("future-timestamp", now_ms().saturating_add(86_400_000)),
        ] {
            let invalid_dir = temp_dir(session_id);
            let invalid = begin_spool_session(&invalid_dir, session_id).unwrap();
            write_spool_f32_chunk(&invalid_dir, session_id, 0, &[0.1, 0.2]);
            record_spool_chunk(&invalid, 0, 0.0, 0.000125, "written").unwrap();
            set_spool_created_at_for_test(&invalid, created_at_ms);
            assert_spool_mtime_is_recent(&invalid);
            assert!(recover_spool(&invalid_dir, u64::MAX).unwrap().is_empty());
            assert!(
                !invalid.exists(),
                "untrustworthy creation timestamps expire fail-closed: {session_id}"
            );
            let _ = std::fs::remove_dir_all(invalid_dir);
        }

        // A schema-one manifest from a build predating the required immutable
        // creation timestamp cannot establish an age, so preserve the existing
        // cleanup behavior for timestamp-less legacy manifests.
        let legacy_dir = temp_dir("legacy-without-creation-time");
        let legacy = legacy_dir.join("spool/legacy-without-creation-time");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("manifest.json"),
            r#"{"schema_version":1,"session_id":"legacy-without-creation-time","status":"active","chunks":[]}"#,
        )
        .unwrap();
        assert!(recover_spool(&legacy_dir, KEEP_DAYS).unwrap().is_empty());
        assert!(!legacy.exists());

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(legacy_dir);
    }

    #[test]
    fn chunker_spool_recovery_reconstructs_exact_overlapping_sample_coverage() {
        const SAMPLE_RATE: usize = 16_000;
        const SOURCE_LEN: usize = 50 * SAMPLE_RATE;
        let dir = temp_dir("exact-chunk-recovery");
        let session_id = "exact-chunks";
        let session = begin_spool_session(&dir, session_id).unwrap();

        let mut source = (0..SOURCE_LEN)
            .map(|index| ((index % 997) as f32 / 997.0 - 0.5) * 0.008)
            .collect::<Vec<_>>();
        let config = crate::chunker::ChunkerConfig {
            chunk_length_secs: 15,
        };
        let mut probe = crate::chunker::Chunker::new(config);
        let mut planned = probe.push(&source);
        planned.extend(probe.finish());
        assert!(planned.len() >= 2);
        let boundary_sample = planned[1].source_start_sample + 100;
        source[boundary_sample] = 0.008;

        let mut chunker = crate::chunker::Chunker::new(config);
        let mut chunks = chunker.push(&source);
        chunks.extend(chunker.finish());
        assert!(chunks.len() >= 2);
        assert_eq!(
            chunks[1].source_start_sample,
            planned[1].source_start_sample
        );
        assert!(chunks[1].source_start_sample <= boundary_sample);
        assert!(boundary_sample < chunks[0].source_start_sample + chunks[0].samples.len());

        for chunk in &chunks {
            write_spool_f32_chunk(&dir, session_id, chunk.index, &chunk.samples);
            record_spool_chunk_with_samples(
                &session,
                chunk.index,
                chunk.start_secs,
                chunk.end_secs,
                "written",
                chunk.source_start_sample as u64,
                chunk.samples.len() as u64,
            )
            .unwrap();
        }
        // The older status API is also used as chunks progress; it must not
        // erase the exact interval required for recovery.
        record_spool_chunk(
            &session,
            chunks[0].index,
            chunks[0].start_secs,
            chunks[0].end_secs,
            "transcribed",
        )
        .unwrap();
        let manifest: SpoolManifest =
            serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap()).unwrap();
        let first = manifest
            .chunks
            .iter()
            .find(|chunk| chunk.index == chunks[0].index)
            .unwrap();
        assert_eq!(first.source_start_sample, Some(0));
        assert_eq!(first.sample_count, Some(chunks[0].samples.len() as u64));

        let recovered = rebuild_spool_recovery(&session).unwrap();
        assert_eq!(
            recovered.duration_secs,
            SOURCE_LEN as f64 / SAMPLE_RATE as f64
        );
        let expected = source
            .iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect::<Vec<_>>();
        let recovered_samples = decode_wav_samples(&recovered.audio_path);
        assert_eq!(recovered_samples.len(), SOURCE_LEN);
        assert_eq!(recovered_samples, expected);
        let marker = expected[boundary_sample];
        assert_eq!(
            recovered_samples
                .iter()
                .filter(|sample| **sample == marker)
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_nonoverlapping_spool_chunks_still_concatenate_in_index_order() {
        let dir = temp_dir("legacy-nonoverlap-recovery");
        let session = begin_spool_session(&dir, "legacy-chunks").unwrap();
        let first = [0.1_f32, 0.2];
        let second = [-0.3_f32, 0.4];
        write_spool_f32_chunk(&dir, "legacy-chunks", 0, &first);
        write_spool_f32_chunk(&dir, "legacy-chunks", 1, &second);
        record_spool_chunk(&session, 0, 0.0, 0.000125, "written").unwrap();
        record_spool_chunk(&session, 1, 0.000125, 0.00025, "written").unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();
        assert_eq!(recovered.len(), 1);
        let expected = first
            .iter()
            .chain(&second)
            .map(|sample| (*sample * i16::MAX as f32) as i16)
            .collect::<Vec<_>>();
        assert_eq!(decode_wav_samples(&recovered[0].audio_path), expected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn exact_spool_intervals_reject_missing_gaps_and_conflicting_samples() {
        let dir = temp_dir("invalid-exact-recovery");

        let missing = begin_spool_session(&dir, "missing").unwrap();
        write_spool_f32_chunk(&dir, "missing", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "missing", 1, &[0.3, 0.4]);
        record_spool_chunk_with_samples(&missing, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        record_spool_chunk(&missing, 1, 0.000125, 0.00025, "written").unwrap();
        let error = rebuild_spool_recovery(&missing).unwrap_err().to_string();
        assert!(error.contains("intervals are missing"), "{error}");

        let unmatched = begin_spool_session(&dir, "unmatched-exact").unwrap();
        record_spool_chunk_with_samples(&unmatched, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        write_spool_f32_chunk(&dir, "unmatched-exact", 1, &[0.3, 0.4]);
        let error = rebuild_spool_recovery(&unmatched).unwrap_err().to_string();
        assert!(error.contains("intervals are missing"), "{error}");

        let gap = begin_spool_session(&dir, "gap").unwrap();
        write_spool_f32_chunk(&dir, "gap", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "gap", 1, &[0.3, 0.4]);
        record_spool_chunk_with_samples(&gap, 0, 0.0, 0.000125, "written", 0, 2).unwrap();
        record_spool_chunk_with_samples(&gap, 1, 0.0001875, 0.0003125, "written", 3, 2).unwrap();
        let error = rebuild_spool_recovery(&gap).unwrap_err().to_string();
        assert!(error.contains("gap in source samples"), "{error}");

        let conflict = begin_spool_session(&dir, "conflict").unwrap();
        write_spool_f32_chunk(&dir, "conflict", 0, &[0.1, 0.2, 0.3]);
        write_spool_f32_chunk(&dir, "conflict", 1, &[0.9, 0.4]);
        record_spool_chunk_with_samples(&conflict, 0, 0.0, 0.0001875, "written", 0, 3).unwrap();
        record_spool_chunk_with_samples(&conflict, 1, 0.000125, 0.00025, "written", 2, 2).unwrap();
        let error = rebuild_spool_recovery(&conflict).unwrap_err().to_string();
        assert!(error.contains("conflicting samples"), "{error}");

        assert!(record_spool_chunk_with_samples(&conflict, 2, 0.0, 0.0, "written", 0, 0).is_err());
        assert!(
            record_spool_chunk_with_samples(&conflict, 2, 0.0, 0.0, "written", u64::MAX, 1,)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_overlapping_chunks_without_sample_intervals_fail_truthfully() {
        let dir = temp_dir("legacy-overlap-recovery");
        let session = begin_spool_session(&dir, "legacy-overlap").unwrap();
        write_spool_f32_chunk(&dir, "legacy-overlap", 0, &[0.1, 0.2]);
        write_spool_f32_chunk(&dir, "legacy-overlap", 1, &[0.2, 0.3]);
        record_spool_chunk(&session, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&session, 1, 0.5, 1.5, "written").unwrap();

        let error = rebuild_spool_recovery(&session).unwrap_err().to_string();
        assert!(error.contains("overlap but lack exact source sample intervals"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unreconstructable_recovery_keeps_original_chunks_until_retention() {
        let dir = temp_dir("unreconstructable-recovery-retained");

        let legacy = begin_spool_session(&dir, "legacy-overlap-retained").unwrap();
        let legacy_first = [0.1_f32, 0.2];
        let legacy_second = [0.2_f32, 0.3];
        write_spool_f32_chunk(&dir, "legacy-overlap-retained", 0, &legacy_first);
        write_spool_f32_chunk(&dir, "legacy-overlap-retained", 1, &legacy_second);
        record_spool_chunk(&legacy, 0, 0.0, 1.0, "written").unwrap();
        record_spool_chunk(&legacy, 1, 0.5, 1.5, "written").unwrap();
        let legacy_first_bytes = read_spool_file(&legacy.join("chunks/00000000.f32")).unwrap();
        let legacy_second_bytes = read_spool_file(&legacy.join("chunks/00000001.f32")).unwrap();
        let legacy_manifest_before: SpoolManifest =
            serde_json::from_slice(&std::fs::read(legacy.join("manifest.json")).unwrap()).unwrap();

        let conflict = begin_spool_session(&dir, "conflict-retained").unwrap();
        let conflict_first = [0.1_f32, 0.2, 0.3];
        let conflict_second = [0.9_f32, 0.4];
        write_spool_f32_chunk(&dir, "conflict-retained", 0, &conflict_first);
        write_spool_f32_chunk(&dir, "conflict-retained", 1, &conflict_second);
        record_spool_chunk_with_samples(&conflict, 0, 0.0, 0.0001875, "written", 0, 3).unwrap();
        record_spool_chunk_with_samples(&conflict, 1, 0.000125, 0.00025, "written", 2, 2).unwrap();
        let conflict_first_bytes = read_spool_file(&conflict.join("chunks/00000000.f32")).unwrap();
        let conflict_second_bytes = read_spool_file(&conflict.join("chunks/00000001.f32")).unwrap();
        let conflict_manifest_before: SpoolManifest =
            serde_json::from_slice(&std::fs::read(conflict.join("manifest.json")).unwrap())
                .unwrap();

        let recovered = recover_spool(&dir, 7).unwrap();
        assert!(
            recovered.is_empty(),
            "unreconstructable audio must not be reported recovered"
        );

        for (session, expected, second_expected, manifest_before) in [
            (
                legacy,
                legacy_first_bytes,
                legacy_second_bytes,
                legacy_manifest_before,
            ),
            (
                conflict,
                conflict_first_bytes,
                conflict_second_bytes,
                conflict_manifest_before,
            ),
        ] {
            assert!(session.is_dir());
            assert!(!session.join("recovery.wav").exists());
            assert_eq!(
                read_spool_file(&session.join("chunks/00000000.f32")).unwrap(),
                expected
            );
            assert_eq!(
                read_spool_file(&session.join("chunks/00000001.f32")).unwrap(),
                second_expected
            );
            let manifest_after: SpoolManifest =
                serde_json::from_slice(&std::fs::read(session.join("manifest.json")).unwrap())
                    .unwrap();
            assert_eq!(manifest_after.status, "abandoned");
            assert_eq!(manifest_after.chunks.len(), manifest_before.chunks.len());
            for (before, after) in manifest_before.chunks.iter().zip(&manifest_after.chunks) {
                assert_eq!(before.index, after.index);
                assert_eq!(before.start_secs, after.start_secs);
                assert_eq!(before.end_secs, after.end_secs);
                assert_eq!(before.source_start_sample, after.source_start_sample);
                assert_eq!(before.sample_count, after.sample_count);
                assert_eq!(before.status, after.status);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_with_missing_recovery_wav_completes_manifest_before_commit() {
        let dir = temp_dir("delete-history-missing-session-wav");
        let session = begin_spool_session(&dir, "session-missing-wav").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        assert_eq!(load_manifest(&session).unwrap().status, "recoverable");
        record_recovered_history(&dir, &recovery).unwrap();
        std::fs::remove_file(&recovery.audio_path).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;

        let lease = wait_for_history_delete_spool_lease(&dir, &recovery.audio_path).unwrap();
        let connection = open_history(&dir).unwrap();
        let transaction = connection
            .unchecked_transaction()
            .expect("start History delete transaction");
        mark_spool_artifact_complete(&dir, &recovery.audio_path, lease.inner.kind).unwrap();
        assert_eq!(load_manifest(&session).unwrap().status, "complete");
        transaction
            .execute("DELETE FROM dictations WHERE id=?", [id])
            .unwrap();
        transaction.commit().unwrap();
        drop(lease);

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(session.exists());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(session.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cached_recovery_cannot_resurrect_history_after_delete() {
        let dir = temp_dir("cached-recovery-after-delete");
        let session = begin_spool_session(&dir, "cached-delete-session").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        record_recovered_history(&dir, &recovery).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;

        delete_history(&dir, id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!session.exists());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!session.exists());
        assert!(!recovery.audio_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn completed_session_with_retained_recovery_wav_cannot_be_promoted() {
        let dir = temp_dir("completed-recovery-not-promoted");
        let session = begin_spool_session(&dir, "completed-recovery-session").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        let mut manifest = load_manifest(&session).unwrap();
        manifest.status = "complete".into();
        save_manifest(&session, &manifest).unwrap();

        assert!(recovery.audio_path.exists());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flat_managed_recovery_promotes_idempotently_and_cannot_resurrect_after_delete() {
        let dir = temp_dir("flat-recovery-promotion");
        let audio_path =
            write_spool_file(&dir, Path::new("legacy-recovery.wav"), b"legacy audio").unwrap();
        let recovery = RecoveredSpool {
            audio_path: audio_path.clone(),
            duration_secs: 1.0,
        };
        record_recovered_history(&dir, &recovery).unwrap();
        record_recovered_history(&dir, &recovery).unwrap();
        let rows = get_history(&dir, 10).unwrap();
        assert_eq!(rows.len(), 1);

        delete_history(&dir, rows[0].id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(!audio_path.exists());
        assert!(record_recovered_history(&dir, &recovery).is_err());
        assert!(get_history(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn recovered_history_is_idempotent_and_deletes_session_artifacts() {
        let dir = temp_dir("recovered-history");
        let session = begin_spool_session(&dir, "session-recovered").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        record_recovered_history(&dir, &recovery).unwrap();
        record_recovered_history(&dir, &recovery).unwrap();
        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_recovered_history_insert_is_idempotent_for_one_audio_path() {
        let dir = temp_dir("concurrent-recovered-history");
        let session = begin_spool_session(&dir, "session-concurrent-recovered").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        let recovery = recover_spool(&dir, 7).unwrap().remove(0);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let dir = dir.clone();
                let recovery = recovery.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    record_recovered_history(&dir, &recovery)
                })
            })
            .collect();
        barrier.wait();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn degraded_history_exposes_recovery_audio_for_retry() {
        let dir = temp_dir("degraded-retry");
        let session = begin_spool_session(&dir, "session-degraded-retry").unwrap();
        std::fs::create_dir_all(session.join("chunks")).unwrap();
        std::fs::write(session.join("chunks/00000000.f32"), 0.25_f32.to_le_bytes()).unwrap();
        mark_spool_status(&session, "degraded").unwrap();
        let recovery = rebuild_spool_recovery(&session).unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            recovery.duration_secs,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&recovery.audio_path),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        assert_eq!(
            failed_spool(&dir, id).unwrap(),
            Some(recovery.audio_path.to_string_lossy().into())
        );
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_removes_a_degraded_spool_directory() {
        let dir = temp_dir("delete-spool-dir");
        let session = begin_spool_session(&dir, "session-degraded").unwrap();
        std::fs::write(session.join("chunk_0.wav"), b"audio").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_can_be_deleted() {
        let dir = temp_dir("delete");
        insert_history(&dir, "raw", "final", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        delete_history(&dir, id).unwrap();
        assert!(get_history(&dir, 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_history_waits_for_spool_owner_and_leaves_record_on_timeout() {
        let dir = temp_dir("delete-history-busy-spool");
        let session = begin_spool_session(&dir, "delete-busy").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let owner = lease_spool_session(&session).unwrap();

        let error = delete_history(&dir, id).unwrap_err();

        assert!(error.to_string().contains("timed out"));
        assert_eq!(get_history(&dir, 10).unwrap().len(), 1);
        assert_eq!(load_manifest(&session).unwrap().status, "active");
        drop(owner);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_history_after_spool_release_does_not_resurrect_on_recovery() {
        let dir = temp_dir("delete-history-spool-release");
        let session = begin_spool_session(&dir, "delete-release").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            true,
            Some("partial_asr_failure"),
            "degraded",
            "history",
            None,
            &context,
            Some(&session),
        )
        .unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let owner = lease_spool_session(&session).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        pause_history_delete_spool_wait_for_test(&session, entered_tx, release_rx);

        let delete_root = dir.clone();
        let deleting = std::thread::spawn(move || delete_history(&delete_root, id));
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(owner);
        release_tx.send(()).unwrap();
        deleting.join().unwrap().unwrap();

        assert!(get_history(&dir, 10).unwrap().is_empty());
        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(!session.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clear_all_data_scrubs_text_and_keeps_database_usable() {
        let dir = temp_dir("clear-history-scrub-text");
        let secret = "unique_clear_history_secret_7f0d8a2c";
        insert_history(&dir, secret, secret, 1.0, false).unwrap();
        let database = dir.join("history.sqlite");

        clear_all_data(&dir).unwrap();

        assert!(!fs::read(database)
            .unwrap()
            .windows(secret.len())
            .any(|w| w == secret.as_bytes()));
        assert!(get_history(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn history_copy_falls_back_to_raw_text_when_final_text_is_empty() {
        let dir = temp_dir("history-copy-fallback");
        insert_history(&dir, "raw transcript", "", 1.0, true).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        assert_eq!(history_text(&dir, id).unwrap(), "raw transcript");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_keeps_raw_final_and_cleanup_status_separately() {
        let dir = temp_dir("history-cleanup-texts");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool_and_cleanup(
            &dir,
            "spoken before cleanup",
            "cleaned after cleanup",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            None,
            "ai_success",
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.raw_text, "spoken before cleanup");
        assert_eq!(item.final_text, "cleaned after cleanup");
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_exports_original_asr_separately_and_preserves_null_for_legacy_rows() {
        let dir = temp_dir("history-asr-text");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_asr_and_delivery_and_spool_and_cleanup(
            &dir,
            "prepared and dictionary-replaced",
            Some("uh, raw provider output"),
            "cleaned final",
            1.0,
            false,
            None,
            "ok",
            "history",
            None,
            &context,
            None,
            "openai:whisper-1",
            "ai_success",
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.raw_text, "prepared and dictionary-replaced");
        assert_eq!(item.asr_text.as_deref(), Some("uh, raw provider output"));
        assert_eq!(item.engine.as_deref(), Some("openai:whisper-1"));
        assert_eq!(
            get_history_page(&dir, 10, None, Some("raw provider"))
                .unwrap()
                .items
                .len(),
            1
        );
        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(export[0]["asr_text"], "uh, raw provider output");
        assert_eq!(export[0]["engine"], "openai:whisper-1");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retryable_only_when_recovery_audio_exists() {
        let dir = temp_dir("history-retryable");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            1.0,
            true,
            Some("llm_cleanup_failed"),
            "degraded",
            "paste",
            None,
            &context,
            None,
        )
        .unwrap();
        assert!(!get_history(&dir, 10).unwrap()[0].retryable);

        let path = write_spool_file(&dir, Path::new("failed-short.wav"), b"audio").unwrap();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "partial",
            1.0,
            true,
            Some("llm_cleanup_failed"),
            "degraded",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        assert!(get_history(&dir, 1).unwrap()[0].retryable);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cleanup_rejection_candidate_is_separate_exportable_and_generation_scoped() {
        let dir = temp_dir("cleanup-rejection-history");
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_asr_candidate_and_delivery_and_spool_and_cleanup_rejection(
            &dir,
            "prepared transcript",
            Some("provider ASR text"),
            Some("ASR-side cleanup candidate"),
            "safe final text",
            1.0,
            true,
            Some("preservation_guard"),
            "degraded",
            "history",
            Some("paste_unverified"),
            &context,
            None,
            "test-asr",
            "preservation_guard",
            None,
            Some("complete cleanup candidate"),
            Some("provider_protected_token_changed"),
        )
        .unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        let old_transforms = [
            "strip_internal_cleanup_metadata".to_owned(),
            "restore_spoken_layout".to_owned(),
        ];
        update_history_revision_state_with_reclean_delivery(
            &dir,
            id,
            true,
            Some("preservation_guard"),
            Some("preservation_guard"),
            Some("prior rejected candidate"),
            Some("provider_protected_token_changed"),
            None,
            Some("network"),
            Some(&old_transforms),
            Some("prior_final_reason"),
            Some("network_failure"),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(
            item.provider_cleaned_candidate.as_deref(),
            Some("ASR-side cleanup candidate")
        );
        assert_eq!(
            item.cleanup_rejected_candidate.as_deref(),
            Some("prior rejected candidate")
        );
        assert_eq!(
            item.cleanup_guard_reason.as_deref(),
            Some("provider_protected_token_changed")
        );
        assert_eq!(item.cleanup_attempt_kind.as_deref(), Some("network"));
        assert_eq!(
            item.cleanup_transformations.as_deref(),
            Some(old_transforms.as_slice())
        );
        assert_eq!(
            item.cleanup_final_guard_reason.as_deref(),
            Some("prior_final_reason")
        );
        assert_eq!(
            item.cleanup_failure_reason.as_deref(),
            Some("network_failure")
        );
        assert_eq!(
            get_history_page(&dir, 10, None, Some("prior rejected candidate"))
                .unwrap()
                .items[0]
                .id,
            id
        );

        let export: serde_json::Value =
            serde_json::from_str(&export_history_json(&dir).unwrap()).unwrap();
        assert_eq!(
            export[0]["provider_cleaned_candidate"],
            "ASR-side cleanup candidate"
        );
        assert_eq!(
            export[0]["cleanup_rejected_candidate"],
            "prior rejected candidate"
        );
        assert_eq!(
            export[0]["cleanup_guard_reason"],
            "provider_protected_token_changed"
        );

        mark_retried_with_texts_and_cleanup_rejection(
            &dir,
            id,
            Some("new prepared transcript"),
            Some("new ASR text"),
            Some("on_device:qwen3-asr-0.6b"),
            "new final text",
            true,
            Some("preservation_guard"),
            Some("preservation_guard"),
            RetryDelivery::ClipboardChanged,
            Some("new cleanup candidate"),
            Some("final_source_script_changed"),
            Some("final_guard"),
            Some(&[]),
            Some("final_explicit_layout_changed"),
            Some("final_guard_explicit_layout_changed"),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.provider_cleaned_candidate, None);
        assert_eq!(item.asr_text.as_deref(), Some("new ASR text"));
        assert_eq!(
            item.cleanup_rejected_candidate.as_deref(),
            Some("new cleanup candidate")
        );
        assert_eq!(
            item.cleanup_guard_reason.as_deref(),
            Some("final_source_script_changed")
        );
        assert_eq!(item.cleanup_attempt_kind.as_deref(), Some("final_guard"));
        assert_eq!(item.cleanup_transformations.as_deref(), Some([].as_slice()));
        assert_eq!(
            item.cleanup_final_guard_reason.as_deref(),
            Some("final_explicit_layout_changed")
        );
        assert_eq!(
            item.cleanup_failure_reason.as_deref(),
            Some("final_guard_explicit_layout_changed")
        );
        assert_eq!(item.raw_text, "new prepared transcript");
        assert_eq!(item.final_text, "new final text");

        let manual = save_manual_history_revision(&dir, id, "manually edited final").unwrap();
        assert!(manual.changed);
        assert_eq!(manual.learning_before_text, "new final text");
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.cleanup_rejected_candidate, None);
        assert_eq!(item.cleanup_guard_reason, None);
        assert_eq!(item.cleanup_attempt_kind, None);
        assert_eq!(item.cleanup_transformations, None);
        assert_eq!(item.cleanup_final_guard_reason, None);
        assert_eq!(item.cleanup_failure_reason, None);
        assert_eq!(item.raw_text, "new prepared transcript");
        assert_eq!(item.asr_text.as_deref(), Some("new ASR text"));
        assert_eq!(item.final_text, "manually edited final");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_scope_keeps_unknown_distinct_from_a_foreign_scope() {
        let row = LearnPairRecord {
            pair_key: "pair".into(),
            before_surface: "LOM".into(),
            after_surface: "LLM".into(),
            hits: 3,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            browser_host: None,
            native_bundle: Some("com.example.codex".into()),
            last_used_at: None,
            pinned: false,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
            match_policy: LearnPairMatchPolicy::default(),
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };
        let matching = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            browser_host: None,
            native_bundle: Some("com.example.codex".into()),
        };
        assert_eq!(matching.assess_record(&row), LearnPairScopeMatch::Match);
        assert_eq!(
            LearnPairScope {
                native_bundle: Some("com.example.other".into()),
                ..matching.clone()
            }
            .assess_record(&row),
            LearnPairScopeMatch::Mismatch
        );
        assert_eq!(
            LearnPairScope {
                mapping_id: None,
                ..matching.clone()
            }
            .assess_record(&row),
            LearnPairScopeMatch::Unknown
        );
        let selected = LearnPairScope {
            mapping_id: None,
            native_bundle: None,
            ..matching
        }
        .applicable_promoted_records(std::slice::from_ref(&row));
        assert!(selected.is_empty());
    }

    #[test]
    fn c1_scoped_word_rules_apply_track_usage_and_forget_per_mapping() {
        let dir = temp_dir("c1-scoped-word-rule-a-b");
        let scope_a = LearnPairScope {
            family: Some("developer_collaboration".into()),
            mapping_id: Some("app-a".into()),
            native_bundle: Some("com.example.a".into()),
            ..LearnPairScope::default()
        };
        let scope_b = LearnPairScope {
            mapping_id: Some("app-b".into()),
            native_bundle: Some("com.example.b".into()),
            ..scope_a.clone()
        };
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let policy = word_rule_match_policy(before, after);
        let key_a = scoped_word_rule_key(before, after, &scope_a, policy).into_string();
        let key_b = scoped_word_rule_key(before, after, &scope_b, policy).into_string();
        assert!(key_a.starts_with("word:v2:"));
        assert!(key_b.starts_with("word:v2:"));
        assert_ne!(key_a, key_b);
        ensure_manual_scoped_learn_pair(&dir, &key_a, before, after, &scope_a).unwrap();
        ensure_manual_scoped_learn_pair(&dir, &key_b, before, after, &scope_b).unwrap();

        let rows = list_learn_pairs(&dir).unwrap();
        assert_eq!(rows.len(), 2);
        let selected_a = scope_a.applicable_promoted_records(&rows);
        let selected_b = scope_b.applicable_promoted_records(&rows);
        assert_eq!(selected_a.len(), 1);
        assert_eq!(selected_a[0].pair_key, key_a);
        assert_eq!(selected_b.len(), 1);
        assert_eq!(selected_b[0].pair_key, key_b);

        let rules_a = crate::lexicon::replaceable_pairs(&selected_a, &[]);
        let (text_a, _, applied_a) = crate::lexicon::apply_explicit_replacements_with_facts(
            "Use VOICEFLO today",
            &rules_a,
            &[],
        );
        assert_eq!(text_a, "Use VoiceFlow today");
        let used_a = crate::lexicon::used_pair_keys(&applied_a, &selected_a);
        assert_eq!(used_a.as_slice(), std::slice::from_ref(&key_a));
        bump_learn_pairs_used(&dir, &used_a).unwrap();
        let after_a_usage = list_learn_pairs(&dir).unwrap();
        assert_eq!(
            after_a_usage
                .iter()
                .find(|row| row.pair_key == key_a)
                .unwrap()
                .replacement_runs,
            1
        );
        assert_eq!(
            after_a_usage
                .iter()
                .find(|row| row.pair_key == key_b)
                .unwrap()
                .replacement_runs,
            0
        );

        assert!(tombstone_learn_pair(&dir, &key_a).unwrap());
        let after_forget = list_learn_pairs(&dir).unwrap();
        assert!(scope_a
            .applicable_promoted_records(&after_forget)
            .is_empty());
        let selected_b_after_forget = scope_b.applicable_promoted_records(&after_forget);
        let rules_b = crate::lexicon::replaceable_pairs(&selected_b_after_forget, &[]);
        let (text_b, _, applied_b) = crate::lexicon::apply_explicit_replacements_with_facts(
            "Use VOICEFLO today",
            &rules_b,
            &[],
        );
        assert_eq!(text_b, "Use VoiceFlow today");
        let used_b = crate::lexicon::used_pair_keys(&applied_b, &selected_b_after_forget);
        assert_eq!(used_b.as_slice(), std::slice::from_ref(&key_b));
        bump_learn_pairs_used(&dir, &used_b).unwrap();
        let final_rows = list_learn_pairs(&dir).unwrap();
        assert!(
            final_rows
                .iter()
                .find(|row| row.pair_key == key_a)
                .unwrap()
                .ignored
        );
        assert_eq!(
            final_rows
                .iter()
                .find(|row| row.pair_key == key_b)
                .unwrap()
                .replacement_runs,
            1
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn c1_automatic_pair_does_not_expand_across_scopes_but_manual_confirmation_does() {
        let dir = temp_dir("c1-cross-scope-auto-owner");
        let scope_a = LearnPairScope {
            family: Some("developer_collaboration".into()),
            mapping_id: Some("app-a".into()),
            native_bundle: Some("com.example.a".into()),
            ..LearnPairScope::default()
        };
        let scope_b = LearnPairScope {
            mapping_id: Some("app-b".into()),
            native_bundle: Some("com.example.b".into()),
            ..scope_a.clone()
        };
        let before = "VoiceFlo";
        let after = "VoiceFlow";
        let policy = word_rule_match_policy(before, after);
        let key_a = scoped_word_rule_key(before, after, &scope_a, policy).into_string();
        let key_b = scoped_word_rule_key(before, after, &scope_b, policy).into_string();

        let origin_a = "c1-auto-origin-a";
        let observation_a = LearnPairObservation::research_fixture_origin(origin_a, &key_a);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key_a,
                before,
                after,
                Some(&scope_a),
                Some(&observation_a),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            promote_learn_pair_if_eligible_with_status(
                &dir,
                &key_a,
                before,
                after,
                Some(&scope_a),
                1,
                true,
            )
            .unwrap(),
            LearnPairPromotionStatus::Promoted
        );
        assert_eq!(
            count_research_fixture_learn_pair_hits(&dir, &key_a).unwrap(),
            1
        );

        let origin_b = "c1-auto-origin-b";
        let observation_b = LearnPairObservation::research_fixture_origin(origin_b, &key_b);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key_b,
                before,
                after,
                Some(&scope_b),
                Some(&observation_b),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            promote_learn_pair_if_eligible_with_status(
                &dir,
                &key_b,
                before,
                after,
                Some(&scope_b),
                1,
                true,
            )
            .unwrap(),
            LearnPairPromotionStatus::CrossScopeAutoBlocked
        );
        assert_eq!(
            count_research_fixture_learn_pair_hits(&dir, &key_b).unwrap(),
            1
        );
        let pending_b = get_learn_pair(&dir, &key_b).unwrap().unwrap();
        assert!(pending_b.is_pending());
        assert!(!pending_b.auto_scoped);

        ensure_manual_scoped_learn_pair(&dir, &key_b, before, after, &scope_b).unwrap();
        let rows = list_learn_pairs(&dir).unwrap();
        assert_eq!(scope_a.applicable_promoted_records(&rows).len(), 1);
        assert_eq!(scope_b.applicable_promoted_records(&rows).len(), 1);
        assert!(
            rows.iter()
                .find(|row| row.pair_key == key_b)
                .unwrap()
                .manual_scoped
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn c1_conflicts_abstain_for_word_targets_without_style_or_pin_suppression() {
        let dir = temp_dir("c1-word-conflict-scope");
        let scope = LearnPairScope {
            family: Some("developer_collaboration".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        let first_key = scoped_word_rule_key(
            "VoiceFlo",
            "VoiceFlow",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let second_key = scoped_word_rule_key(
            "VOICEFLO",
            "VoiceFlow Inc.",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        ensure_manual_scoped_learn_pair(&dir, &first_key, "VoiceFlo", "VoiceFlow", &scope).unwrap();
        ensure_manual_scoped_learn_pair(&dir, &second_key, "VOICEFLO", "VoiceFlow Inc.", &scope)
            .unwrap();
        let style_key = "learn:style:codex";
        ensure_learn_pair_promoted(&dir, style_key, "VoiceFlo", "Stylistic", Some(&scope)).unwrap();
        ensure_pinned_dictionary_term(&dir, "PinnedWord", true).unwrap();

        let rows = list_learn_pairs(&dir).unwrap();
        let first = rows.iter().find(|row| row.pair_key == first_key).unwrap();
        let without_competitor = scope.applicable_promoted_records(&[
            first.clone(),
            rows.iter()
                .find(|row| row.pair_key == style_key)
                .unwrap()
                .clone(),
            rows.iter()
                .find(|row| row.before_surface.is_empty())
                .unwrap()
                .clone(),
        ]);
        assert!(without_competitor
            .iter()
            .any(|row| row.pair_key == first_key));

        let applicable = scope.applicable_promoted_records(&rows);
        assert!(!applicable
            .iter()
            .any(|row| row.pair_key == first_key || row.pair_key == second_key));
        assert!(applicable.iter().any(|row| row.pair_key == style_key));
        let pin = applicable
            .iter()
            .find(|row| row.before_surface.is_empty())
            .unwrap();
        assert!(pin.pinned);
        assert!(pin.binding_invalidated_at.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v19_and_v20_conflicting_word_rules_abstain_without_rekeying_legacy_rows() {
        let dir = temp_dir("c1-v19-v20-conflict");
        let scope = LearnPairScope {
            family: Some("developer_collaboration".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        let legacy_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        let scoped_key = scoped_word_rule_key(
            "voiceflo",
            "VoiceFlow Next",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        ensure_learn_pair_promoted(&dir, &legacy_key, "VoiceFlo", "VoiceFlow", Some(&scope))
            .unwrap();
        ensure_manual_scoped_learn_pair(&dir, &scoped_key, "voiceflo", "VoiceFlow Next", &scope)
            .unwrap();
        let legacy_before = get_learn_pair(&dir, &legacy_key).unwrap().unwrap();
        let scoped_before = get_learn_pair(&dir, &scoped_key).unwrap().unwrap();
        let rows = list_learn_pairs(&dir).unwrap();
        assert!(!scope
            .applicable_promoted_records(&rows)
            .iter()
            .any(|row| row.pair_key == legacy_key || row.pair_key == scoped_key));
        let legacy_after = get_learn_pair(&dir, &legacy_key).unwrap().unwrap();
        let scoped_after = get_learn_pair(&dir, &scoped_key).unwrap().unwrap();
        assert_eq!(
            legacy_after, legacy_before,
            "v19 key and counters remain untouched"
        );
        assert_eq!(scoped_after, scoped_before);
        assert_eq!(legacy_after.pair_key, legacy_key);

        assert!(tombstone_learn_pair(&dir, &legacy_key).unwrap());
        let new_key = scoped_word_rule_key(
            "VoiceFlo",
            "VoiceFlow",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let observation = LearnPairObservation::paste_session("v19-tombstone-passive", &new_key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &new_key,
                "VoiceFlo",
                "VoiceFlow",
                Some(&scope),
                Some(&observation),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::TombstoneOrIgnored)
        ));
        assert!(get_learn_pair(&dir, &new_key).unwrap().is_none());
        assert!(
            ensure_manual_scoped_learn_pair(&dir, &new_key, "VoiceFlo", "VoiceFlow", &scope,)
                .is_ok(),
            "explicit current-scope action may proceed while preserving the old tombstone"
        );
        assert!(get_learn_pair(&dir, &legacy_key).unwrap().unwrap().ignored);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn c1_mapping_invalidation_disarms_only_bound_words_and_rebind_adds_no_hits() {
        let dir = temp_dir("c1-mapping-invalidation");
        let scope = LearnPairScope {
            family: Some("developer_collaboration".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        let key_same = scoped_word_rule_key(
            "VoiceFlo",
            "VoiceFlow",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let key_changed =
            scoped_word_rule_key("Reakt", "React", &scope, LearnPairMatchPolicy::IgnoreCase)
                .into_string();
        let key_auto = scoped_word_rule_key(
            "TauriApp",
            "VoiceFlowApp",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let key_global = scoped_word_rule_key(
            "GloblTypo",
            "GlobalTerm",
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        ensure_manual_scoped_learn_pair(&dir, &key_same, "VoiceFlo", "VoiceFlow", &scope).unwrap();
        ensure_manual_scoped_learn_pair(&dir, &key_changed, "Reakt", "React", &scope).unwrap();
        assert!(matches!(
            upsert_auto_scoped_learn_pair_with_observation(
                &dir,
                &key_auto,
                "TauriApp",
                "VoiceFlowApp",
                Some(&scope),
                None,
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        ensure_local_only_learn_pair_promoted(
            &dir,
            &key_auto,
            "TauriApp",
            "VoiceFlowApp",
            Some(&scope),
        )
        .unwrap();
        ensure_learn_pair_promoted(&dir, &key_global, "GloblTypo", "GlobalTerm", Some(&scope))
            .unwrap();
        assert_eq!(
            list_pending_dictionary_effects(&dir).unwrap()[0].action,
            DictionaryEffectAction::Add
        );
        let style_key = "learn:style:codex";
        ensure_learn_pair_promoted(&dir, style_key, "x", "style", Some(&scope)).unwrap();
        ensure_pinned_dictionary_term(&dir, "PinnedWord", true).unwrap();

        assert_eq!(
            invalidate_word_rule_mapping_bindings(
                &dir,
                &[(
                    "codex".into(),
                    LearnPairBindingInvalidationReason::DeclaredIdentityChanged
                )]
            )
            .unwrap(),
            4
        );
        let pending = list_pending_dictionary_effects(&dir).unwrap();
        assert!(pending.iter().any(|effect| {
            effect.pair_key == key_global && effect.action == DictionaryEffectAction::Remove
        }));
        let invalidated = list_learn_pairs(&dir).unwrap();
        for key in [&key_same, &key_changed, &key_auto] {
            let row = invalidated.iter().find(|row| &row.pair_key == key).unwrap();
            assert!(row.promoted);
            assert!(row.is_binding_invalidated());
            assert_eq!(
                row.binding_invalidated_reason.as_deref(),
                Some("declared_identity_changed")
            );
            assert!(scope
                .applicable_promoted_records(std::slice::from_ref(row))
                .is_empty());
            assert!(ensure_learn_pair_promoted(
                &dir,
                key,
                &row.before_surface,
                &row.after_surface,
                Some(&scope),
            )
            .is_err());
        }
        let style = invalidated
            .iter()
            .find(|row| row.pair_key == style_key)
            .unwrap();
        assert!(!style.is_binding_invalidated());
        let pin = invalidated
            .iter()
            .find(|row| row.before_surface.is_empty())
            .unwrap();
        assert!(pin.pinned);
        assert!(!pin.is_binding_invalidated());

        let same_scope_rebound = rebind_invalidated_word_rule(&dir, &key_same, &scope).unwrap();
        assert_eq!(same_scope_rebound.pair_key, key_same);
        assert!(!same_scope_rebound.is_binding_invalidated());
        assert_eq!(same_scope_rebound.hits, 0);
        let changed_scope = LearnPairScope {
            mapping_id: Some("codex-v2".into()),
            ..scope.clone()
        };
        let changed_scope_rebound =
            rebind_invalidated_word_rule(&dir, &key_changed, &changed_scope).unwrap();
        assert_ne!(changed_scope_rebound.pair_key, key_changed);
        assert!(changed_scope_rebound.is_live_promoted());
        assert!(changed_scope_rebound.manual_scoped);
        assert_eq!(changed_scope_rebound.hits, 0);
        let auto_scope_rebound =
            rebind_invalidated_word_rule(&dir, &key_auto, &changed_scope).unwrap();
        assert_ne!(auto_scope_rebound.pair_key, key_auto);
        assert!(auto_scope_rebound.is_live_promoted());
        assert!(!auto_scope_rebound.manual_scoped);
        assert!(auto_scope_rebound.auto_scoped);
        assert_eq!(auto_scope_rebound.hits, 0);
        let final_rows = list_learn_pairs(&dir).unwrap();
        assert!(final_rows
            .iter()
            .find(|row| row.pair_key == key_changed)
            .unwrap()
            .is_binding_invalidated());
        assert_eq!(
            final_rows
                .iter()
                .find(|row| row.pair_key == changed_scope_rebound.pair_key)
                .unwrap()
                .replacement_runs,
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn c1_missing_mapping_invalidation_targets_only_v2_word_rules_with_mapping_ids() {
        let dir = temp_dir("c1-missing-mapping-bindings");
        let missing_scope = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("deleted-app".into()),
            native_bundle: Some("com.example.deleted".into()),
            ..LearnPairScope::default()
        };
        let missing_key = scoped_word_rule_key(
            "Teh",
            "The",
            &missing_scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let unbound_scope = LearnPairScope {
            mapping_id: None,
            native_bundle: Some("com.example.native".into()),
            ..missing_scope.clone()
        };
        let unbound_key = scoped_word_rule_key(
            "Voys",
            "Voice",
            &unbound_scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        ensure_manual_scoped_learn_pair(&dir, &missing_key, "Teh", "The", &missing_scope).unwrap();
        ensure_manual_scoped_learn_pair(&dir, &unbound_key, "Voys", "Voice", &unbound_scope)
            .unwrap();
        assert_eq!(
            invalidate_missing_word_rule_mapping_bindings(&dir, &[]).unwrap(),
            1
        );
        let rows = list_learn_pairs(&dir).unwrap();
        let missing = rows.iter().find(|row| row.pair_key == missing_key).unwrap();
        assert_eq!(
            missing.binding_invalidated_reason.as_deref(),
            Some("missing_binding_at_migration")
        );
        assert!(!rows
            .iter()
            .find(|row| row.pair_key == unbound_key)
            .unwrap()
            .is_binding_invalidated());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn existing_family_only_promoted_rows_no_longer_apply_globally() {
        let dir = temp_dir("family-only-learn-pair-migration");
        let connection = open_history(&dir).unwrap();
        let family_key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let mapping_key = crate::dictionary_learn::pair_key("派森", "Python");
        let global_key = crate::dictionary_learn::pair_key("大模型", "LLM");
        let empty_family_key = crate::dictionary_learn::pair_key("空族", "EmptyFamily");
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family)
                 VALUES (?1, '知呼', '知乎', 3, 1, datetime('now'), 'work_chat')",
                [&family_key],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at)
                 VALUES (?1, '大模型', 'LLM', 3, 1, datetime('now'))",
                [&global_key],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family)
                 VALUES (?1, '空族', 'EmptyFamily', 4, 1, datetime('now'), '')",
                [&empty_family_key],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO learn_pairs (pair_key, before_surface, after_surface, hits, promoted, last_at, family, mapping_id, last_used_at, pinned)
                 VALUES (?1, '派森', 'Python', 3, 1, datetime('now'), 'work_chat', 'codex', '2026-11-03', 1)",
                [&mapping_key],
            )
            .unwrap();
        drop(connection);

        let reopened = open_history(&dir).unwrap();
        let version: i32 = reopened
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, HISTORY_SCHEMA_VERSION);
        let family_only = get_learn_pair(&dir, &family_key).unwrap().unwrap();
        let mapping_scoped = get_learn_pair(&dir, &mapping_key).unwrap().unwrap();
        let global = get_learn_pair(&dir, &global_key).unwrap().unwrap();
        let empty_family = get_learn_pair(&dir, &empty_family_key).unwrap().unwrap();
        assert!(!family_only.is_live_promoted());
        assert_eq!(family_only.before_surface, "知呼");
        assert_eq!(family_only.after_surface, "知乎");
        assert_eq!(family_only.hits, 3);
        assert!(mapping_scoped.is_live_promoted());
        assert_eq!(mapping_scoped.before_surface, "派森");
        assert_eq!(mapping_scoped.after_surface, "Python");
        assert_eq!(mapping_scoped.hits, 3);
        assert_eq!(mapping_scoped.mapping_id.as_deref(), Some("codex"));
        assert_eq!(mapping_scoped.last_used_at.as_deref(), Some("2026-11-03"));
        assert!(mapping_scoped.pinned);
        assert!(global.is_live_promoted());
        assert!(empty_family.is_live_promoted());
        assert_eq!(empty_family.family.as_deref(), Some(""));
        assert_eq!(empty_family.before_surface, "空族");
        assert_eq!(empty_family.after_surface, "EmptyFamily");
        assert_eq!(empty_family.hits, 4);

        let same_family_other_app = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("slack".into()),
            ..LearnPairScope::default()
        };
        assert!(same_family_other_app
            .applicable_promoted_records(&[family_only.clone(), mapping_scoped.clone()])
            .is_empty());
        assert_eq!(
            LearnPairScope {
                family: Some("work_chat".into()),
                mapping_id: Some("codex".into()),
                ..LearnPairScope::default()
            }
            .applicable_promoted_records(&[mapping_scoped])
            .len(),
            1
        );
        drop(reopened);

        let reopened_again = open_history(&dir).unwrap();
        assert!(!get_learn_pair(&dir, &family_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        assert!(get_learn_pair(&dir, &empty_family_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        drop(reopened_again);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reclean_revision_and_attribution_roll_back_together_on_sql_failure() {
        let dir = temp_dir("reclean-atomic-adoption");
        insert_history(&dir, "raw source", "old adopted text", 1.0, false).unwrap();
        let c = open_history(&dir).unwrap();
        c.execute("UPDATE dictations SET cleanup_attempt_kind='network',cleanup_failure_reason='network_failure' WHERE id=1", []).unwrap();
        c.execute_batch("CREATE TRIGGER reject_reclean_facts BEFORE UPDATE OF cleanup_attempt_kind ON dictations BEGIN SELECT RAISE(ABORT, 'injected attribution failure'); END;").unwrap();
        drop(c);
        let intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Cleanup,
            "raw source",
        );
        let result = save_reclean_revision_with_facts(
            &dir,
            RecleanHistorySave {
                dictation_id: 1,
                final_text: "new adopted text",
                cleanup_status: "ai_success",
                intent: &intent,
                model: "model",
                context_policy: &crate::context::ContextPolicy::default(),
                cleanup_rejected_candidate: None,
                cleanup_guard_reason: None,
                cleanup_attempt_kind: Some("success"),
                cleanup_transformations: Some(&[]),
                cleanup_final_guard_reason: None,
                cleanup_failure_reason: None,
            },
        );
        assert!(result.is_err());
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.final_text, "old adopted text");
        assert_eq!(item.cleanup_attempt_kind.as_deref(), Some("network"));
        assert_eq!(
            item.cleanup_failure_reason.as_deref(),
            Some("network_failure")
        );
        assert_eq!(item.revision_count, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_updates_history_text() {
        let dir = temp_dir("retry");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("retried raw"),
            Some("retry ASR before local filters"),
            Some("deepgram:nova-3"),
            "final",
            false,
            None,
            Some("ai_success"),
            RetryDelivery::Copied,
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.raw_text, "retried raw");
        assert_eq!(
            item.asr_text.as_deref(),
            Some("retry ASR before local filters")
        );
        assert_eq!(item.final_text, "final");
        assert_eq!(item.cleanup_status, "ai_success");
        assert_eq!(item.engine.as_deref(), Some("deepgram:nova-3"));
        assert_eq!(item.delivery_method.as_deref(), Some("clipboard"));
        assert_eq!(
            item.fallback_reason.as_deref(),
            Some("retry_clipboard_only")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_clipboard_abstention_saves_transcript_as_history() {
        let dir = temp_dir("retry-clipboard-changed");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("retried raw"),
            Some("retry ASR"),
            Some("on_device:qwen3-asr-0.6b"),
            "final",
            false,
            None,
            Some("local_only"),
            RetryDelivery::ClipboardChanged,
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "history");
        assert_eq!(item.final_text, "final");
        assert_eq!(item.delivery_method.as_deref(), Some("history"));
        assert_eq!(
            item.fallback_reason.as_deref(),
            Some("retry_clipboard_changed")
        );
        assert!(failed_spool(&dir, id).unwrap().is_none());
        assert_eq!(history_text(&dir, id).unwrap(), "final");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_clipboard_ownership_unknown_and_write_failure_keep_history_status() {
        for delivery in [
            RetryDelivery::ClipboardOwnershipUnverified,
            RetryDelivery::ClipboardWriteFailed,
        ] {
            let dir = temp_dir("retry-clipboard-unverified");
            insert_failed_history(&dir, "raw", 1.0, None).unwrap();
            let id = get_history(&dir, 10).unwrap()[0].id;
            mark_retried_with_texts(
                &dir,
                id,
                Some("retried raw"),
                Some("retry ASR"),
                Some("on_device:qwen3-asr-0.6b"),
                "final",
                false,
                None,
                Some("local_only"),
                delivery,
            )
            .unwrap();

            let item = get_history(&dir, 10).unwrap().remove(0);
            assert_eq!(item.status, "history");
            assert_eq!(item.delivery_method.as_deref(), Some("history"));
            assert_eq!(
                item.fallback_reason.as_deref(),
                Some(match delivery {
                    RetryDelivery::ClipboardOwnershipUnverified => {
                        "retry_clipboard_ownership_unverified"
                    }
                    RetryDelivery::ClipboardWriteFailed => "retry_clipboard_write_failed",
                    _ => unreachable!(),
                })
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn reclean_delivery_records_only_a_current_owned_copy_as_copied() {
        let cases = [
            (
                Some(RecleanDelivery::Copied),
                "copied",
                "clipboard",
                Some("reclean_clipboard_only"),
            ),
            (
                Some(RecleanDelivery::ClipboardChanged),
                "history",
                "history",
                Some("reclean_clipboard_changed"),
            ),
            (
                Some(RecleanDelivery::ClipboardOwnershipUnverified),
                "history",
                "history",
                Some("reclean_clipboard_ownership_unverified"),
            ),
            (
                Some(RecleanDelivery::ClipboardWriteFailed),
                "history",
                "history",
                Some("reclean_clipboard_write_failed"),
            ),
        ];
        for (delivery, status, delivery_method, fallback_reason) in cases {
            let dir = temp_dir("reclean-delivery");
            insert_failed_history(&dir, "raw source", 1.0, None).unwrap();
            let id = get_history(&dir, 10).unwrap()[0].id;
            save_history_revision(
                &dir,
                id,
                "prepared result",
                Some("ai_success"),
                None,
                None,
                None,
                "ai_reclean",
            )
            .unwrap();
            update_history_revision_state_with_reclean_delivery(
                &dir,
                id,
                false,
                None,
                Some("ai_success"),
                None,
                None,
                delivery,
                Some("success"),
                Some(&[]),
                None,
                None,
            )
            .unwrap();

            let item = get_history(&dir, 10).unwrap().remove(0);
            assert_eq!(item.status, status);
            assert_eq!(item.final_text, "prepared result");
            assert_eq!(item.delivery_method.as_deref(), Some(delivery_method));
            assert_eq!(item.fallback_reason.as_deref(), fallback_reason);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn reclean_degraded_result_stays_in_history_without_a_clipboard_claim() {
        let dir = temp_dir("reclean-degraded-no-copy");
        insert_failed_history(&dir, "raw source", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        save_history_revision(
            &dir,
            id,
            "raw source",
            Some("ai_failed_raw"),
            None,
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        update_history_revision_state_with_reclean_delivery(
            &dir,
            id,
            true,
            Some("native_input_too_large"),
            Some("ai_failed_raw"),
            None,
            None,
            None,
            Some("success"),
            Some(&[]),
            None,
            None,
        )
        .unwrap();

        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "degraded");
        assert_eq!(
            item.degraded_reason.as_deref(),
            Some("native_input_too_large")
        );
        assert_eq!(item.delivery_method.as_deref(), Some("history"));
        assert_eq!(item.fallback_reason, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retry_cleanup_failure_stays_degraded() {
        let dir = temp_dir("retry-degraded");
        insert_failed_history(&dir, "raw", 1.0, None).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        mark_retried_with_texts(
            &dir,
            id,
            Some("raw"),
            Some("raw"),
            None,
            "raw",
            true,
            Some("llm_cleanup_failed"),
            None,
            RetryDelivery::ClipboardChanged,
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "degraded");
        assert!(item.degraded);
        assert_eq!(item.degraded_reason.as_deref(), Some("llm_cleanup_failed"));
        assert_eq!(item.delivery_method.as_deref(), Some("history"));
        assert_eq!(
            item.fallback_reason.as_deref(),
            Some("retry_clipboard_changed")
        );
        assert!(failed_spool(&dir, id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_revisions_keep_the_parent_and_latest_text() {
        let dir = temp_dir("revisions");
        insert_history(&dir, "raw", "first", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let intent =
            crate::llm::CleanupIntent::selected_text(crate::llm::CleanupOperation::Shorten, "raw");
        save_history_revision(
            &dir,
            id,
            "second",
            Some("ai_success"),
            Some(&intent),
            Some(crate::llm::MODEL),
            Some(&crate::context::ContextPolicy::default()),
            "ai_reclean",
        )
        .unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.final_text, "second");
        assert_eq!(item.revision_count, 1);
        assert_eq!(history_text(&dir, id).unwrap(), "second");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 1);
        delete_history(&dir, id).unwrap();
        assert!(get_history_revisions(&dir, id).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retrying_manual_history_save_reuses_original_revision_evidence() {
        let dir = temp_dir("manual-history-learning-retry");
        insert_history(&dir, "raw", "adopted before", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        let recorded_at_ms = now_epoch_millis().saturating_sub(3 * 24 * 60 * 60 * 1_000);
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime(?1 / 1000, 'unixepoch') WHERE id=?2",
                params![recorded_at_ms as i64, id],
            )
            .unwrap();
        let expected_natural_day: String = connection
            .query_row(
                "SELECT date(created_at, 'localtime') FROM dictations WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        let first = save_manual_history_revision(&dir, id, "adopted after").unwrap();
        assert!(first.changed);
        assert!(first.first_edit_of_original_delivery);
        assert_eq!(
            first.source_created_at_ms,
            Some(recorded_at_ms / 1_000 * 1_000)
        );
        assert_eq!(
            first.natural_day.as_deref(),
            Some(expected_natural_day.as_str())
        );
        assert_eq!(first.learning_before_text, "adopted before");
        let first_revision_id = first.revision_id.unwrap();

        let retry = save_manual_history_revision(&dir, id, "adopted after").unwrap();
        assert!(!retry.changed);
        assert!(!retry.first_edit_of_original_delivery);
        assert_eq!(retry.revision_id, Some(first_revision_id));
        assert_eq!(retry.learning_before_text, "adopted before");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 1);

        let next = save_manual_history_revision(&dir, id, "third adopted text").unwrap();
        assert!(next.changed);
        assert!(!next.first_edit_of_original_delivery);
        assert_eq!(next.learning_before_text, "adopted after");
        assert_eq!(get_history_revisions(&dir, id).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn external_revision_lookup_requires_the_exact_dictation_and_revision_pair() {
        let dir = temp_dir("external-revision-pair");
        insert_history(&dir, "raw one", "parent final one", 1.0, false).unwrap();
        insert_history(&dir, "raw two", "parent final two", 1.0, false).unwrap();
        let rows = get_history(&dir, 10).unwrap();
        let first_parent = rows
            .iter()
            .find(|row| row.raw_text == "raw one")
            .unwrap()
            .id;
        let second_parent = rows
            .iter()
            .find(|row| row.raw_text == "raw two")
            .unwrap()
            .id;
        let intent = crate::llm::CleanupIntent::selected_text(
            crate::llm::CleanupOperation::Cleanup,
            "cleanup",
        );
        let first_revision = save_history_revision(
            &dir,
            first_parent,
            "first successful cleanup",
            Some("ai_success"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        let second_revision = save_history_revision(
            &dir,
            first_parent,
            "second successful cleanup",
            Some("ai_success"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        let degraded_revision = save_history_revision(
            &dir,
            first_parent,
            "degraded raw result",
            Some("ai_failed_raw"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();
        let other_parent_revision = save_history_revision(
            &dir,
            second_parent,
            "another history result",
            Some("ai_success"),
            Some(&intent),
            None,
            None,
            "ai_reclean",
        )
        .unwrap();

        assert_eq!(
            get_history_revision(&dir, first_parent, first_revision)
                .unwrap()
                .unwrap()
                .final_text,
            "first successful cleanup"
        );
        assert_eq!(
            get_history_revision(&dir, first_parent, second_revision)
                .unwrap()
                .unwrap()
                .final_text,
            "second successful cleanup"
        );
        assert_eq!(
            get_history_revision(&dir, first_parent, degraded_revision)
                .unwrap()
                .unwrap()
                .cleanup_status
                .as_deref(),
            Some("ai_failed_raw")
        );
        assert!(
            get_history_revision(&dir, first_parent, other_parent_revision)
                .unwrap()
                .is_none()
        );
        assert!(get_history_revision(&dir, second_parent, first_revision)
            .unwrap()
            .is_none());
        assert!(get_history_revision(&dir, first_parent, i64::MAX)
            .unwrap()
            .is_none());
        delete_history(&dir, first_parent).unwrap();
        assert!(get_history_revision(&dir, first_parent, first_revision)
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_reclean_state_does_not_create_a_fake_fallback_error() {
        let dir = temp_dir("revision-state");
        insert_history(&dir, "raw", "first", 1.0, false).unwrap();
        let id = get_history(&dir, 10).unwrap()[0].id;
        update_history_revision_state(&dir, id, false, None, "copied", Some("ai_success")).unwrap();
        let item = get_history(&dir, 10).unwrap().remove(0);
        assert_eq!(item.status, "copied");
        assert_eq!(item.fallback_reason, None);
        assert_eq!(item.cleanup_status, "ai_success");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dictionary_round_trips() {
        let mut settings = Settings::default();
        settings.dictionary.push("VoiceFlow".into());
        settings.dictionary.retain(|word| word != "missing");
        settings.dictionary.retain(|word| word != "VoiceFlow");
        assert!(settings.dictionary.is_empty());
    }

    #[test]
    fn default_chunk_settings_match_recommended_values() {
        let settings = Settings::default();
        assert_eq!(settings.chunk_threshold_secs, 25);
        assert_eq!(settings.chunk_length_secs, 35);
    }

    #[test]
    fn input_gain_defaults_to_one_and_clamps() {
        assert_eq!(Settings::default().input_gain, 1.0);
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        assert_eq!(parsed.input_gain, 1.0);
        assert_eq!(SettingsView::from(&parsed).input_gain, 1.0);
        let mut quiet = Settings {
            input_gain: 0.1,
            ..Settings::default()
        };
        quiet.normalize();
        assert_eq!(quiet.input_gain, 0.5);
        let mut loud = Settings {
            input_gain: 9.0,
            ..Settings::default()
        };
        loud.normalize();
        assert_eq!(loud.input_gain, 4.0);
        let mut invalid = Settings {
            input_gain: f32::NAN,
            ..Settings::default()
        };
        invalid.normalize();
        assert_eq!(invalid.input_gain, 1.0);
    }

    #[test]
    fn hud_visuals_default_and_normalize() {
        let defaults = Settings::default();
        assert_eq!(defaults.hud_color_variant, "colorful");
        assert_eq!(defaults.hud_processing_color_variant, "colorful");
        assert_eq!(defaults.hud_voice_style, "ripple");
        assert_eq!(defaults.hud_processing_style, "flow_line");
        assert_eq!(defaults.hud_voice_intensity, 1.0);

        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        let view = SettingsView::from(&parsed);
        assert_eq!(view.hud_color_variant, "colorful");
        assert_eq!(view.hud_processing_color_variant, "colorful");
        assert_eq!(view.hud_voice_style, "ripple");
        assert_eq!(view.hud_processing_style, "flow_line");
        assert_eq!(view.hud_voice_intensity, 1.0);

        let mut custom = Settings {
            hud_color_variant: "ocean".into(),
            hud_processing_color_variant: "forest".into(),
            hud_voice_style: "prism".into(),
            hud_processing_style: "brilliant_sweep".into(),
            hud_voice_intensity: 1.35,
            ..Settings::default()
        };
        custom.normalize();
        assert_eq!(custom.hud_color_variant, "ocean");
        assert_eq!(custom.hud_processing_color_variant, "forest");
        assert_eq!(custom.hud_voice_style, "prism");
        assert_eq!(custom.hud_processing_style, "brilliant_sweep");
        assert!((custom.hud_voice_intensity - 1.35).abs() < 1e-6);

        let mut invalid = Settings {
            hud_color_variant: "neon".into(),
            hud_processing_color_variant: "rainbow".into(),
            hud_voice_style: "unknown".into(),
            hud_processing_style: "laser".into(),
            hud_voice_intensity: 4.0,
            ..Settings::default()
        };
        invalid.normalize();
        assert_eq!(invalid.hud_color_variant, "colorful");
        assert_eq!(invalid.hud_processing_color_variant, "colorful");
        assert_eq!(invalid.hud_voice_style, "ripple");
        assert_eq!(invalid.hud_processing_style, "flow_line");
        assert_eq!(invalid.hud_voice_intensity, 1.5);

        let mut nan_intensity = Settings {
            hud_voice_intensity: f32::NAN,
            ..Settings::default()
        };
        nan_intensity.normalize();
        assert_eq!(nan_intensity.hud_voice_intensity, 1.0);

        let mut low_intensity = Settings {
            hud_voice_intensity: 0.1,
            ..Settings::default()
        };
        low_intensity.normalize();
        assert_eq!(low_intensity.hud_voice_intensity, 0.5);
    }

    #[test]
    fn dictionary_learn_enabled_defaults_true() {
        assert!(Settings::default().dictionary_learn_enabled);
        assert!(!Settings::default().local_edit_learning_enabled);
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"x"}"#).unwrap();
        assert!(parsed.dictionary_learn_enabled);
        assert!(!parsed.local_edit_learning_enabled);
        let view = SettingsView::from(&parsed);
        assert!(view.dictionary_learn_enabled);
        assert!(!view.local_edit_learning_enabled);
        let opted_in: Settings =
            serde_json::from_str(r#"{"api_key":"x","local_edit_learning_enabled":true}"#).unwrap();
        assert!(opted_in.local_edit_learning_enabled);
    }

    #[test]
    fn asr_base_url_defaults_empty_and_falls_back_to_groq_key() {
        assert!(Settings::default().asr_base_url.is_empty());
        let parsed: Settings = serde_json::from_str(r#"{"api_key":"gsk_fallback"}"#).unwrap();
        assert!(parsed.asr_base_url.is_empty());
        assert!(parsed.asr_api_key.is_empty());
        assert_eq!(parsed.asr_credential(), "gsk_fallback");
        let groq_url = Settings {
            api_key: "gsk_fallback".into(),
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        assert_eq!(groq_url.asr_credential(), "gsk_fallback");
        let custom_without_asr_key = Settings {
            api_key: "gsk_fallback".into(),
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert_eq!(custom_without_asr_key.asr_credential(), "");
        let with_asr_key = Settings {
            api_key: "gsk_fallback".into(),
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_api_key: "asr_only".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert_eq!(with_asr_key.asr_credential(), "asr_only");
        let view = SettingsView::from(&with_asr_key);
        assert_eq!(view.asr_base_url, "http://127.0.0.1:8000/v1");
        assert!(view.asr_api_key_configured);
        assert_eq!(view.asr_api_key_hint.as_deref(), Some("••••_only"));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("asr_only"));
        assert!(!json.contains("gsk_fallback"));
    }

    #[test]
    fn asr_base_url_validate_rejects_bad_scheme_plaintext_and_keyless_custom() {
        assert!(Settings::default().validate().is_ok());
        let groq = Settings {
            asr_base_url: "https://api.groq.com/openai/v1".into(),
            ..Settings::default()
        };
        assert!(groq.validate().is_ok());
        let loopback_with_key = Settings {
            asr_api_key: "local-key".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert!(loopback_with_key.validate().is_ok());
        let relative = Settings {
            asr_base_url: "asr.example.com/v1".into(),
            asr_api_key: "k".into(),
            ..Settings::default()
        };
        assert!(relative
            .validate()
            .unwrap_err()
            .to_string()
            .contains("http:// 或 https://"));
        let plaintext = Settings {
            asr_base_url: "http://asr.example.com/v1".into(),
            asr_api_key: "k".into(),
            ..Settings::default()
        };
        assert!(plaintext
            .validate()
            .unwrap_err()
            .to_string()
            .contains("https://"));
        let custom_without_key = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "https://asr.example.com/v1".into(),
            custom_base_url: "https://asr.example.com/v1".into(),
            onboarded: true,
            ..Settings::default()
        };
        assert_eq!(
            custom_without_key.validate().unwrap_err().to_string(),
            "自定义 ASR 地址需要填写 ASR 密钥。"
        );
        let loopback_without_key = Settings {
            asr_provider: crate::engine::EngineProvider::Custom,
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            custom_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        assert!(loopback_without_key.validate().is_ok());
    }

    #[test]
    fn schema_17_promotes_openai_cleanup_host_into_the_provider_pool() {
        let mut settings = Settings {
            schema_version: 16,
            api_key: "gsk_keep".into(),
            asr_provider: crate::engine::EngineProvider::Groq,
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_api_key: "sk-openai".into(),
            cleanup_model: "gpt-4o-mini".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            settings.cleanup_provider,
            crate::engine::EngineProvider::OpenAi
        );
        assert_eq!(
            settings.provider_api_keys.get("openai").map(String::as_str),
            Some("sk-openai")
        );
        assert_eq!(
            settings.provider_api_keys.get("groq").map(String::as_str),
            Some("gsk_keep")
        );
        assert!(settings.cleanup_base_url.is_empty());
        assert_eq!(settings.cleanup_model, "gpt-4o-mini");
        assert_eq!(settings.cleanup_credential(), "sk-openai");
        assert_eq!(settings.asr_credential(), "gsk_keep");
    }

    #[test]
    fn named_asr_does_not_reuse_the_groq_key() {
        let settings = Settings {
            api_key: "gsk_keep".into(),
            asr_provider: crate::engine::EngineProvider::OpenAi,
            ..Settings::default()
        };
        assert!(settings.asr_credential().is_empty());
        assert_eq!(
            settings.provider_secret(crate::engine::EngineProvider::Groq),
            "gsk_keep"
        );
    }

    #[test]
    fn bind_cleanup_key_clears_on_host_change_unless_a_new_key_is_supplied() {
        assert_eq!(
            bind_cleanup_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.groq.com/openai/v1",
                "https://api.openai.com/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.openai.com/v1",
                "https://api.openai.com/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn custom_cleanup_key_binding_detects_scheme_and_port_changes() {
        use crate::engine::EngineProvider;

        assert_eq!(
            bind_cleanup_key_to_host(EngineProvider::Custom, "", "", "", "old-key"),
            "old-key",
            "an untouched empty Custom endpoint must not clear credentials"
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:8443/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                EngineProvider::Custom,
                "http://router.example/v1",
                "https://router.example/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_cleanup_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:443/chat/completions",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn bind_asr_key_clears_on_host_change_unless_a_new_key_is_supplied() {
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.groq.com/openai/v1",
                "https://asr.example.com/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://api.groq.com/openai/v1",
                "https://asr.example.com/v1",
                "new-key",
                "old-key"
            ),
            "new-key"
        );
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::Groq,
                "",
                "https://api.groq.com/openai/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
        assert_eq!(
            bind_asr_key_to_host(
                crate::engine::EngineProvider::OpenAi,
                "https://asr.example.com/v1",
                "https://asr.example.com/v1",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn custom_asr_key_binding_detects_scheme_and_port_changes() {
        use crate::engine::EngineProvider;

        assert_eq!(
            bind_asr_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:8443/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                EngineProvider::Custom,
                "http://router.example/v1",
                "https://router.example/v1",
                "",
                "old-key"
            ),
            ""
        );
        assert_eq!(
            bind_asr_key_to_host(
                EngineProvider::Custom,
                "https://router.example/v1",
                "https://router.example:443/audio/transcriptions",
                "",
                "old-key"
            ),
            "old-key"
        );
    }

    #[test]
    fn save_settings_never_writes_plaintext_asr_key() {
        let dir = temp_dir("asr-keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            asr_api_key: "asr_secret_should_not_be_on_disk".into(),
            asr_base_url: "http://127.0.0.1:8000/v1".into(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("asr_secret_should_not_be_on_disk"),
            "plaintext ASR API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["asr_api_key"].as_str().unwrap_or(""), "");
        assert_eq!(
            parsed["asr_base_url"].as_str().unwrap_or(""),
            "http://127.0.0.1:8000/v1"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_settings_never_writes_plaintext_cleanup_key() {
        let dir = temp_dir("cleanup-keyblank");
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            cleanup_provider: crate::engine::EngineProvider::Custom,
            cleanup_base_url: "https://api.openai.com/v1".into(),
            cleanup_model: "gpt-4o-mini".into(),
            cleanup_api_key: "cleanup_secret_should_not_be_on_disk".into(),
            ..Settings::default()
        };
        save_settings_with_backend(&dir, &settings, &backend).unwrap();
        let on_disk = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            !on_disk.contains("cleanup_secret_should_not_be_on_disk"),
            "plaintext cleanup API key must never be written to settings.json"
        );
        let parsed: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed["cleanup_api_key"].as_str().unwrap_or(""), "");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_page_uses_a_cursor_and_reports_more_rows() {
        let dir = temp_dir("history-page");
        insert_history(&dir, "one", "one", 1.0, false).unwrap();
        insert_history(&dir, "two", "two", 1.0, false).unwrap();
        insert_history(&dir, "three", "three", 1.0, false).unwrap();

        let first = get_history_page(&dir, 2, None, None).unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.has_more);
        let before_id = first.items.last().unwrap().id;

        let second = get_history_page(&dir, 2, Some(before_id), None).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);
        assert!(second.items[0].id < before_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_receipt_reads_only_bounded_source_facts_and_keeps_legacy_state_unknown() {
        let dir = temp_dir("history-preview-context-facts");
        insert_history(&dir, "hello 世界", "hello 世界", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET engine=?1, context_policy_json=?2 WHERE id=(SELECT MAX(id) FROM dictations)",
                params![
                    "groq:whisper-large-v3-turbo",
                    r#"{"family":"document","preview_context_facts":{"context_source":["ocr"],"screen_captured":true}}"#
                ],
            )
            .unwrap();
        drop(connection);

        insert_history(&dir, "legacy row", "legacy row", 1.0, false).unwrap();
        let page = get_history_page(&dir, 10, None, None).unwrap();
        let receipt = page
            .items
            .iter()
            .find(|item| item.raw_text == "hello 世界")
            .unwrap()
            .preview_receipt
            .clone()
            .unwrap();
        assert_eq!(
            receipt.source_kind,
            crate::preview_provenance::PreviewSourceKind::Dictation
        );
        assert_eq!(receipt.char_count, Some(8));
        assert_eq!(receipt.word_count, Some(2));
        assert_eq!(receipt.provider_label.as_deref(), Some("Groq"));
        assert_eq!(
            receipt.context_source,
            Some(vec![crate::preview_provenance::PreviewContextSource::Ocr])
        );
        assert_eq!(receipt.screen_captured, Some(true));

        let legacy = page
            .items
            .iter()
            .find(|item| item.raw_text == "legacy row")
            .unwrap()
            .preview_receipt
            .as_ref()
            .unwrap();
        assert_eq!(legacy.context_source, None);
        assert_eq!(legacy.screen_captured, None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_page_filters_both_text_columns_before_cursor_pagination() {
        let dir = temp_dir("history-search");
        insert_history(&dir, "first raw", "VoiceFlow first", 1.0, false).unwrap();
        insert_history(&dir, "unrelated", "other", 1.0, false).unwrap();
        insert_history(&dir, "VoiceFlow second", "cleaned", 1.0, false).unwrap();

        let first = get_history_page(&dir, 1, None, Some("VoiceFlow")).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].raw_text, "VoiceFlow second");
        assert!(first.has_more);

        let second = get_history_page(&dir, 1, Some(first.items[0].id), Some("VoiceFlow")).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].final_text, "VoiceFlow first");
        assert!(!second.has_more);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_search_escapes_like_wildcards() {
        let dir = temp_dir("history-search-escape");
        insert_history(&dir, "plain", "plain", 1.0, false).unwrap();
        insert_history(&dir, "100% complete", "done", 1.0, false).unwrap();
        insert_history(&dir, "under_score", "other", 1.0, false).unwrap();
        insert_history(&dir, r"path\to\file", "slash", 1.0, false).unwrap();

        let percent = get_history_page(&dir, 10, None, Some("%")).unwrap();
        assert_eq!(percent.items.len(), 1);
        assert_eq!(percent.items[0].raw_text, "100% complete");

        let underscore = get_history_page(&dir, 10, None, Some("_")).unwrap();
        assert_eq!(underscore.items.len(), 1);
        assert_eq!(underscore.items[0].raw_text, "under_score");

        let slash = get_history_page(&dir, 10, None, Some("\\")).unwrap();
        assert_eq!(slash.items.len(), 1);
        assert_eq!(slash.items[0].raw_text, r"path\to\file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retention_removes_old_text_and_audio_reference() {
        let dir = temp_dir("history-retention");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        insert_history(&dir, "new", "new", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime('now','-400 days') WHERE raw_text='old'",
                [],
            )
            .unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 365).unwrap(), 1);
        let page = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].raw_text, "new");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn purge_history_prunes_style_drafts_and_usage() {
        let dir = temp_dir("history-retention-related-tables");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        insert_history(&dir, "new", "new", 1.0, false).unwrap();
        upsert_style_draft(&dir, "mapping", "style", "excerpt", "before", "after").unwrap();
        upsert_style_draft(&dir, "mapping-new", "style", "excerpt", "before", "after").unwrap();
        upsert_style_draft(
            &dir,
            "mapping-boundary",
            "style",
            "excerpt",
            "before",
            "after",
        )
        .unwrap();
        let connection = open_history(&dir).unwrap();
        connection.execute_batch(
            "UPDATE dictations SET created_at = CASE raw_text WHEN 'old' THEN datetime('now','-400 days') ELSE datetime('now') END;
             UPDATE style_drafts SET created_at = CASE mapping_id WHEN 'mapping' THEN datetime('now','-400 days') WHEN 'mapping-boundary' THEN datetime('now','-365 days','+1 minute') ELSE datetime('now') END;
             INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES(date('now','-400 days'),4,2,8),(date('now','-365 days'),2,1,4);",
        ).unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 365).unwrap(), 1);
        let connection = open_history(&dir).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM dictations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM style_drafts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage WHERE day=date('now','-400 days')",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage WHERE day IN (date('now','-365 days'), date('now'))", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(connection);

        insert_history(&dir, "forever", "forever", 1.0, false).unwrap();
        upsert_style_draft(&dir, "mapping-old", "style", "excerpt", "before", "after").unwrap();
        let connection = open_history(&dir).unwrap();
        connection.execute_batch(
            "UPDATE dictations SET created_at = datetime('now','-4000 days');
             UPDATE style_drafts SET created_at = datetime('now','-4000 days');
             INSERT INTO usage(day,asr_requests,llm_requests,audio_seconds) VALUES(date('now','-4000 days'),4,2,8);",
        ).unwrap();
        drop(connection);
        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        let connection = open_history(&dir).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM dictations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM style_drafts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM usage", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        drop(connection);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_retention_zero_keeps_history_forever() {
        let dir = temp_dir("history-retention-forever");
        insert_history(&dir, "old", "old", 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "UPDATE dictations SET created_at=datetime('now','-4000 days')",
                [],
            )
            .unwrap();
        drop(connection);

        assert_eq!(purge_history(&dir, 0).unwrap(), 0);
        let page = get_history_page(&dir, 10, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].raw_text, "old");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_removes_history_usage_and_spool() {
        let dir = temp_dir("clear-all-data");
        insert_history(&dir, "raw", "final", 2.0, false).unwrap();
        write_spool_file(&dir, Path::new("recovery.wav"), b"audio").unwrap();
        write_gold_file(&dir, "kept.wav", b"gold").unwrap();

        clear_all_data(&dir).unwrap();

        assert!(get_history_page(&dir, 10, None, None)
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            get_usage(&dir, crate::queue::RequestGate::new(None).snapshots())
                .unwrap()
                .asr_requests,
            0
        );
        assert!(dir.join("spool").is_dir());
        assert!(std::fs::read_dir(dir.join("spool"))
            .unwrap()
            .next()
            .is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("spool"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
        assert!(!dir.join("gold").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_does_not_delete_on_device_models() {
        let dir = temp_dir("clear-keeps-models");
        insert_history(&dir, "raw", "final", 1.0, false).unwrap();
        let models = dir.join("models").join("sensevoice-small");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("model.int8.onnx"), b"onnx").unwrap();
        std::fs::write(models.join("tokens.txt"), b"tokens").unwrap();
        let sidecar = dir.join("models").join("sensevoice-small.archive.sha256");
        std::fs::write(&sidecar, crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256).unwrap();

        clear_all_data(&dir).unwrap();

        assert_eq!(
            std::fs::read(models.join("model.int8.onnx")).unwrap(),
            b"onnx"
        );
        assert_eq!(std::fs::read(models.join("tokens.txt")).unwrap(), b"tokens");
        assert_eq!(
            std::fs::read_to_string(&sidecar).unwrap(),
            crate::ondevice_models::SENSEVOICE_ARCHIVE_SHA256
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_utf8_backup_precedes_credential_writes_and_backup_failures() {
        let dir = temp_dir("malformed-utf8-credential");
        std::fs::create_dir_all(&dir).unwrap();
        let settings_path = dir.join("settings.json");
        let raw = [0xff, 0xfe, b'{'];
        std::fs::write(&settings_path, raw).unwrap();
        let backend = FakeCredentialBackend::default();
        let (loaded, needs_persist) = load_settings_with_backend(&dir, &backend);
        assert!(!needs_persist);
        assert!(!loaded.onboarded);
        assert_eq!(std::fs::read(&settings_path).unwrap(), raw);
        let settings = Settings {
            api_key: "synthetic-key".into(),
            ..Settings::default()
        };
        persist_settings_with_backend(&dir, &settings, &backend).unwrap();
        assert_eq!(
            std::fs::read(dir.join("settings.json.malformed.bak")).unwrap(),
            raw
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(dir.join("settings.json.malformed.bak"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            serde_json::from_slice::<Settings>(&std::fs::read(&settings_path).unwrap()).is_ok()
        );
        assert!(!backend.writes.borrow().is_empty());
        let _ = std::fs::remove_dir_all(&dir);

        let dir = temp_dir("malformed-backup-directory");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = b"{ malformed";
        std::fs::write(dir.join("settings.json"), raw).unwrap();
        std::fs::create_dir(dir.join("settings.json.malformed.bak")).unwrap();
        let backend = FakeCredentialBackend::default();
        let settings = Settings {
            api_key: "must-not-write".into(),
            ..Settings::default()
        };
        assert!(persist_settings_with_backend(&dir, &settings, &backend).is_err());
        assert!(backend.writes.borrow().is_empty());
        assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), raw);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_settings_are_kept_untouched() {
        let dir = temp_dir("malformed-settings");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let raw = b"{not valid json";
        std::fs::write(&path, raw).unwrap();

        let (settings, needs_persist) =
            load_settings_with_backend(&dir, &FakeCredentialBackend::default());

        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert!(!needs_persist);
        assert!(!settings.onboarded);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_upsert_counts_only_and_leaves_dictionary_empty() {
        let dir = temp_dir("learn-pairs-count");
        std::fs::create_dir_all(&dir).unwrap();
        save_settings(
            &dir,
            &Settings {
                dictionary: vec![],
                ..Settings::default()
            },
        )
        .unwrap();
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        assert_eq!(
            upsert_learn_pair(&dir, &key, "知呼", "知乎")
                .unwrap()
                .unwrap()
                .hits,
            1
        );
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        let third = upsert_learn_pair(&dir, &key, "知呼", "知乎")
            .unwrap()
            .unwrap();
        assert_eq!(third.hits, 3);
        assert!(!third.promoted);
        assert!(load_settings(&dir).0.dictionary.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn transferred_pairs_preserve_match_policy_and_stay_local_only() {
        let dir = temp_dir("learn-pair-transfer-local-only");
        let intent = transfer_intent(
            &dir,
            "Futu",
            "FUTU",
            LearnPairScope::default(),
            LearnPairMatchPolicy::Exact,
            false,
            false,
        );
        let pair_key = intent.pair_key.clone();
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[intent]).unwrap(),
            [LearnPairTransferOutcome::Inserted]
        );

        let row = get_learn_pair(&dir, &pair_key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert_eq!(row.match_policy, LearnPairMatchPolicy::Exact);
        assert!(row.auto_scoped);
        assert!(!row.is_provider_egress_eligible_word_pair());
        assert!(
            LearnPairScope::default()
                .applicable_promoted_records(std::slice::from_ref(&row))
                .contains(&row),
            "local-only global imports still apply without a provider grant"
        );
        assert!(list_pending_dictionary_effects(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn transfer_preflight_detects_changed_conflicts_without_writing_the_import() {
        let dir = temp_dir("learn-pair-transfer-preflight");
        let scope = LearnPairScope::default();
        let intent = transfer_intent(
            &dir,
            "teh",
            "the",
            scope.clone(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            true,
        );
        let imported_key = intent.pair_key.clone();
        let initial_rows = list_learn_pairs(&dir).unwrap();
        assert!(learn_pair_transfer_intents_match_snapshot(
            &initial_rows,
            std::slice::from_ref(&intent)
        )
        .unwrap());

        let conflict_key =
            scoped_word_rule_key("teh", "they", &scope, LearnPairMatchPolicy::IgnoreCase)
                .into_string();
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &conflict_key, "teh", "they", None).unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert!(mark_learn_pair_promoted(&dir, &conflict_key).unwrap());

        let changed_rows = list_learn_pairs(&dir).unwrap();
        assert!(!learn_pair_transfer_intents_match_snapshot(
            &changed_rows,
            std::slice::from_ref(&intent)
        )
        .unwrap());
        assert!(get_learn_pair(&dir, &imported_key).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn imported_direct_scope_pair_is_not_deduplicated_by_an_equivalent_global_pair() {
        let dir = temp_dir("learn-pair-transfer-exact-scope");
        let global_key = scoped_word_rule_key(
            "teh",
            "the",
            &LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &global_key, "teh", "the", None).unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert!(mark_learn_pair_promoted(&dir, &global_key).unwrap());

        let app_scope = LearnPairScope {
            native_bundle: Some("com.example.notes".into()),
            ..LearnPairScope::default()
        };
        let app_intent = transfer_intent(
            &dir,
            "teh",
            "the",
            app_scope.clone(),
            LearnPairMatchPolicy::IgnoreCase,
            true,
            false,
        );
        let app_key = app_intent.pair_key.clone();
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[app_intent]).unwrap(),
            [LearnPairTransferOutcome::Inserted],
            "same output at another exact scope must keep its own local rule"
        );
        let rows = list_learn_pairs(&dir).unwrap();
        assert_eq!(rows.len(), 2);
        let app_row = rows.iter().find(|row| row.pair_key == app_key).unwrap();
        assert!(app_row.is_live_promoted());
        assert!(app_row.auto_scoped);
        assert!(app_row.manual_scoped);
        assert_eq!(app_row.native_bundle.as_deref(), Some("com.example.notes"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn transfer_replacement_stages_old_global_owner_and_keeps_forget_markers() {
        let dir = temp_dir("learn-pair-transfer-replacement");
        let old_key = scoped_word_rule_key(
            "teh",
            "the",
            &LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &old_key, "teh", "the", None).unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert!(mark_learn_pair_promoted(&dir, &old_key).unwrap());

        let replacement = transfer_intent(
            &dir,
            "teh",
            "thee",
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            true,
        );
        let new_key = replacement.pair_key.clone();
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[replacement]).unwrap(),
            [LearnPairTransferOutcome::Replaced {
                pair_keys: vec![old_key.clone()]
            }]
        );
        let old = get_learn_pair(&dir, &old_key).unwrap().unwrap();
        assert!(old.ignored && old.tombstoned_at.is_some());
        let new = get_learn_pair(&dir, &new_key).unwrap().unwrap();
        assert!(new.is_live_promoted());
        assert!(new.auto_scoped);
        assert_eq!(new.match_policy, LearnPairMatchPolicy::IgnoreCase);
        let effects = list_pending_dictionary_effects(&dir).unwrap();
        assert!(effects.iter().any(|effect| {
            effect.pair_key == old_key
                && effect.word == "the"
                && effect.action == DictionaryEffectAction::Remove
        }));

        let repeat = transfer_intent(
            &dir,
            "teh",
            "thee",
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            false,
        );
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[repeat]).unwrap(),
            [LearnPairTransferOutcome::AlreadyPresent]
        );

        let forgotten_reimport = transfer_intent(
            &dir,
            "teh",
            "the",
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            true,
        );
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[forgotten_reimport]).unwrap(),
            [LearnPairTransferOutcome::Blocked]
        );

        let second_replacement = transfer_intent(
            &dir,
            "teh",
            "they",
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            true,
        );
        let latest_key = second_replacement.pair_key.clone();
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[second_replacement]).unwrap(),
            [LearnPairTransferOutcome::Replaced {
                pair_keys: vec![new_key]
            }]
        );
        assert!(get_learn_pair(&dir, &latest_key)
            .unwrap()
            .unwrap()
            .is_live_promoted());
        assert!(get_learn_pair(&dir, &old_key)
            .unwrap()
            .unwrap()
            .tombstoned_at
            .is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn transfer_snapshot_ignores_usage_counters_and_same_output_legacy_aliases() {
        let dir = temp_dir("learn-pair-transfer-snapshot");
        let before = "teh";
        let after = "the";
        let legacy_key = format!("{}\u{1e}{}", before.to_lowercase(), after.to_lowercase());
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &legacy_key, before, after, None).unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert!(mark_learn_pair_promoted(&dir, &legacy_key).unwrap());
        let competing_key = scoped_word_rule_key(
            before,
            "they",
            &LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &competing_key, before, "they", None).unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert!(mark_learn_pair_promoted(&dir, &competing_key).unwrap());
        let v2_key = scoped_word_rule_key(
            before,
            after,
            &LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        let same_output = transfer_intent(
            &dir,
            before,
            after,
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            false,
        );
        assert_eq!(same_output.pair_key, v2_key);
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[same_output]).unwrap(),
            [LearnPairTransferOutcome::Conflict],
            "an egress-enabled matching row requires explicit local-only replacement"
        );
        assert_eq!(list_learn_pairs(&dir).unwrap().len(), 2);

        let localize = transfer_intent(
            &dir,
            before,
            after,
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            true,
        );
        let localized_outcome = apply_learn_pair_transfers(&dir, &[localize]).unwrap();
        let LearnPairTransferOutcome::Replaced { pair_keys } = &localized_outcome[0] else {
            panic!("expected explicit replacement and local-only update");
        };
        assert!(pair_keys.contains(&legacy_key));
        assert!(pair_keys.contains(&competing_key));
        let localized = get_learn_pair(&dir, &legacy_key).unwrap().unwrap();
        assert!(localized.auto_scoped);
        assert!(!localized.is_provider_egress_eligible_word_pair());
        assert!(get_learn_pair(&dir, &competing_key)
            .unwrap()
            .unwrap()
            .tombstoned_at
            .is_some());

        let same_local = transfer_intent(
            &dir,
            before,
            after,
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            false,
        );
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[same_local]).unwrap(),
            [LearnPairTransferOutcome::AlreadyPresent]
        );

        let candidate = transfer_intent(
            &dir,
            before,
            "their",
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            false,
        );
        bump_learn_pairs_used(&dir, std::slice::from_ref(&legacy_key)).unwrap();
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[candidate]).unwrap(),
            [LearnPairTransferOutcome::Conflict],
            "usage changes must not invalidate the preview"
        );

        let stale = transfer_intent(
            &dir,
            before,
            "their",
            LearnPairScope::default(),
            LearnPairMatchPolicy::IgnoreCase,
            false,
            true,
        );
        let stale_key = stale.pair_key.clone();
        assert!(tombstone_learn_pair(&dir, &legacy_key).unwrap());
        assert_eq!(
            apply_learn_pair_transfers(&dir, &[stale]).unwrap(),
            [LearnPairTransferOutcome::StalePreview]
        );
        assert!(get_learn_pair(&dir, &stale_key).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dictionary_sources_preserve_owners_and_reconcile_case_insensitively() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert(
            "dictionary".into(),
            serde_json::json!(["VoiceFlow", "OldTerm"]),
        );
        object.remove("dictionary_sources");
        let mut settings: Settings = serde_json::from_value(value).unwrap();
        settings.normalize_dictionary_sources();

        assert!(settings.has_dictionary_source("voiceflow", &DictionarySource::LegacyUnknown));
        assert!(settings.has_dictionary_source("oldterm", &DictionarySource::LegacyUnknown));
        assert!(settings
            .add_dictionary_source("voiceflow", DictionarySource::Rule("rule:voiceflo".into())));
        assert!(settings.add_dictionary_source(
            "ScreenTerm",
            DictionarySource::Screen("screen-origin".into())
        ));
        assert!(!settings.has_dictionary_source("screenterm", &DictionarySource::LegacyUnknown));
        assert_eq!(
            add_manual_dictionary_words(
                &mut settings,
                &["voiceflow".into(), "NewTerm".into(), "NEWTERM".into()]
            ),
            2
        );
        assert!(settings.has_dictionary_source("VOICEFLOW", &DictionarySource::Manual));
        assert!(settings
            .has_dictionary_source("VoiceFlow", &DictionarySource::Rule("rule:voiceflo".into())));
        assert_eq!(
            settings
                .dictionary
                .iter()
                .filter(|word| word.eq_ignore_ascii_case("voiceflow"))
                .count(),
            1
        );

        let requested = vec!["voiceflow".into(), "FreshTerm".into(), "FRESHTERM".into()];
        settings.dictionary = requested.clone();
        let removed = reconcile_dictionary_patch(&mut settings, requested);
        assert!(removed.iter().any(|word| word == "OldTerm"));
        assert!(removed.iter().any(|word| word == "ScreenTerm"));
        assert!(removed.iter().any(|word| word == "NewTerm"));
        assert!(settings.has_dictionary_source("VoiceFlow", &DictionarySource::Manual));
        assert!(settings
            .has_dictionary_source("voiceflow", &DictionarySource::Rule("rule:voiceflo".into())));
        assert!(settings.has_dictionary_source("freshterm", &DictionarySource::Manual));
        assert!(!settings.has_dictionary_source("freshterm", &DictionarySource::LegacyUnknown));
        assert!(!settings.has_dictionary_source(
            "screenterm",
            &DictionarySource::Screen("screen-origin".into())
        ));
    }

    #[test]
    fn asr_dictionary_egress_requires_an_independent_global_owner() {
        let scoped = |pair_key: &str, before: &str, after: &str| LearnPairRecord {
            pair_key: pair_key.into(),
            before_surface: before.into(),
            after_surface: after.into(),
            hits: 100,
            promoted: true,
            last_at: "2026-01-01".into(),
            family: Some("general".into()),
            mapping_id: Some("app-a".into()),
            browser_host: None,
            native_bundle: Some("com.example.app-a".into()),
            last_used_at: Some("2026-01-02".into()),
            pinned: true,
            tombstoned_at: None,
            ignored: false,
            promote_hits: 0,
            manual_scoped: false,
            auto_scoped: false,
            match_policy: LearnPairMatchPolicy::IgnoreCase,
            binding_invalidated_at: None,
            binding_invalidated_reason: None,
            replacement_runs: 0,
        };
        let scoped_rule = scoped("word:v2:app-a", "acme alias", "AcMe");
        let mut settings = Settings {
            dictionary: vec!["AcMe".into()],
            ..Settings::default()
        };
        settings.dictionary_sources.owners.insert(
            "AcMe".into(),
            [DictionarySource::Rule(scoped_rule.pair_key.clone())]
                .into_iter()
                .collect(),
        );
        assert!(settings
            .asr_dictionary_words(std::slice::from_ref(&scoped_rule))
            .is_empty());

        assert!(settings.add_dictionary_source("acme", DictionarySource::Manual));
        assert_eq!(
            settings.asr_dictionary_words(std::slice::from_ref(&scoped_rule)),
            vec!["AcMe"]
        );

        let mut legacy_unknown = Settings {
            dictionary: vec!["AcMe".into()],
            ..Settings::default()
        };
        legacy_unknown.dictionary_sources.owners.insert(
            "AcMe".into(),
            [DictionarySource::LegacyUnknown].into_iter().collect(),
        );
        assert!(legacy_unknown
            .asr_dictionary_words(std::slice::from_ref(&scoped_rule))
            .is_empty());

        let unowned = Settings {
            dictionary: vec!["AcMe".into()],
            ..Settings::default()
        };
        assert!(unowned.asr_dictionary_words(&[]).is_empty());

        let mut orphaned_rule = Settings {
            dictionary: vec!["AcMe".into()],
            ..Settings::default()
        };
        orphaned_rule.dictionary_sources.owners.insert(
            "AcMe".into(),
            [DictionarySource::Rule("word:v2:cleared-rule".into())]
                .into_iter()
                .collect(),
        );
        assert!(orphaned_rule.asr_dictionary_words(&[]).is_empty());

        let scoped_screen_seed = scoped("\u{1e}acme", "", "AcMe");
        assert!(legacy_unknown
            .asr_dictionary_words(std::slice::from_ref(&scoped_screen_seed))
            .is_empty());
        let scoped_style = scoped("learn:style:app-a", "style cue", "AcMe");
        assert!(legacy_unknown
            .asr_dictionary_words(std::slice::from_ref(&scoped_style))
            .is_empty());

        for source in [
            DictionarySource::Screen("current-screen-context".into()),
            DictionarySource::Other("explicit-global-import".into()),
        ] {
            let mut independent = Settings {
                dictionary: vec!["AcMe".into()],
                ..Settings::default()
            };
            independent
                .dictionary_sources
                .owners
                .insert("AcMe".into(), [source].into_iter().collect());
            assert_eq!(
                independent.asr_dictionary_words(std::slice::from_ref(&scoped_rule)),
                vec!["AcMe"]
            );
        }

        let mut global_rule = scoped_rule.clone();
        global_rule.pair_key = format!("{}\u{1e}{}", "acme alias", "acme");
        global_rule.family = None;
        global_rule.mapping_id = None;
        global_rule.native_bundle = None;
        let mut verified_rule_owner = Settings {
            dictionary: vec!["AcMe".into()],
            ..Settings::default()
        };
        verified_rule_owner.dictionary_sources.owners.insert(
            "AcMe".into(),
            [DictionarySource::Rule(global_rule.pair_key.clone())]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            verified_rule_owner.asr_dictionary_words(std::slice::from_ref(&global_rule)),
            vec!["AcMe"]
        );
    }

    #[test]
    fn dictionary_source_migration_merges_ascii_case_variants_and_owners() {
        let mut settings = Settings {
            dictionary: vec!["VoiceFlow".into(), "voiceflow".into(), "OtherTerm".into()],
            dictionary_sources: legacy_dictionary_sources(),
            ..Settings::default()
        };
        settings.dictionary_sources.owners.insert(
            "VOICEFLOW".into(),
            [DictionarySource::Rule("rule:voiceflo".into())]
                .into_iter()
                .collect(),
        );

        settings.normalize_dictionary_sources();

        assert_eq!(settings.dictionary, vec!["VoiceFlow", "OtherTerm"]);
        assert!(settings.has_dictionary_source("voiceflow", &DictionarySource::LegacyUnknown));
        assert!(settings
            .has_dictionary_source("VOICEFLOW", &DictionarySource::Rule("rule:voiceflo".into())));
        assert_eq!(settings.dictionary_sources.owners.len(), 2);
    }

    #[test]
    fn dictionary_projection_and_persistence_are_capped_without_resurrecting_dropped_words() {
        let dir = temp_dir("dictionary-owner-cap");
        let mut settings = Settings {
            dictionary: (0..257).map(|index| format!("term-{index:03}")).collect(),
            ..Settings::default()
        };
        settings.dictionary_sources.owners.insert(
            "term-256".into(),
            [DictionarySource::Screen("late-screen-origin".into())]
                .into_iter()
                .collect(),
        );

        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
        let (mut loaded, needs_persist) =
            load_settings_with_backend(&dir, &FakeCredentialBackend::default());
        assert!(needs_persist);
        assert_eq!(loaded.dictionary.len(), 256);
        assert_eq!(loaded.dictionary_sources.owners.len(), 256);
        assert!(!loaded.has_dictionary_source(
            "term-256",
            &DictionarySource::Screen("late-screen-origin".into())
        ));
        assert!(loaded.can_add_dictionary_source("TERM-000"));
        assert!(!loaded.can_add_dictionary_source("term-256"));

        let requested = (0..257).map(|index| format!("term-{index:03}")).collect();
        reconcile_dictionary_patch(&mut loaded, requested);
        assert_eq!(loaded.dictionary.len(), 256);
        assert_eq!(loaded.dictionary_sources.owners.len(), 256);
        assert!(!dictionary_contains_word(&loaded.dictionary, "term-256"));
        assert!(!loaded.dictionary_sources.owners.contains_key("term-256"));

        persist_settings_without_credential_changes(&dir, &loaded).unwrap();
        let (reloaded, _) = load_settings_with_backend(&dir, &FakeCredentialBackend::default());
        assert_eq!(reloaded.dictionary.len(), 256);
        assert_eq!(reloaded.dictionary_sources.owners.len(), 256);
        assert!(!dictionary_contains_word(&reloaded.dictionary, "term-256"));
        assert!(!reloaded.dictionary_sources.owners.contains_key("term-256"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn history_v18_migration_preserves_legacy_receipts_without_counting_them_as_proof() {
        let dir = temp_dir("history-v18-learning-proof-migration");
        fs::create_dir_all(&dir).unwrap();
        let database = dir.join("history.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE dictations (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL);
                 CREATE TABLE learn_observations (
                     observation_key TEXT PRIMARY KEY,
                     source_kind TEXT NOT NULL,
                     source_history_id INTEGER,
                     source_revision_id INTEGER,
                     created_at TEXT NOT NULL
                 );
                 INSERT INTO learn_observations VALUES ('legacy-key', 'paste_session', NULL, NULL, '2026-11-06');
                 PRAGMA user_version = 18;",
            )
            .unwrap();
        drop(connection);

        let migrated = open_history(&dir).unwrap();
        let version: i32 = migrated
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, HISTORY_SCHEMA_VERSION);
        let legacy = migrated
            .query_row(
                "SELECT source_kind, rule_key, dictation_origin, channel, proof_eligible
                 FROM learn_observations WHERE observation_key = 'legacy-key'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(legacy, ("paste_session".into(), None, None, None, 0));
        let proof_index: bool = migrated
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_learn_observations_independent_proof')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(proof_index);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, "legacy-key").unwrap(),
            0
        );
        drop(migrated);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_research_fixture_receipt_replay_keeps_key_without_adding_hit_or_proof() {
        let dir = temp_dir("legacy-research-receipt-replay");
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        let source_origin = "voiceflow-origin-legacy";
        let observation_id = format!("stable_source:{source_origin}");
        let legacy_receipt =
            LearnPairObservation::research_fixture_edit(&observation_id, &pair_key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&legacy_receipt),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits, 1);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );

        let replay = LearnPairObservation::research_fixture_origin_with_observation_id(
            &observation_id,
            source_origin,
            &pair_key,
        );
        assert!(has_learn_pair_observation(&dir, &replay).unwrap());
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&replay),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits, 1);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let connection = open_history(&dir).unwrap();
        let receipt: (String, Option<String>, Option<String>, i64) = connection
            .query_row(
                "SELECT source_kind, rule_key, dictation_origin, proof_eligible
                 FROM learn_observations WHERE observation_key = ?1",
                [&replay.observation_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(receipt, ("research_fixture_edit".into(), None, None, 0));
        drop(connection);
        fs::remove_dir_all(dir).unwrap();
    }

    fn insert_test_history_origin(dir: &Path, text: &str, transaction_id: &str) -> i64 {
        insert_history(dir, text, text, 1.0, false).unwrap();
        let connection = open_history(dir).unwrap();
        let id = connection
            .query_row(
                "SELECT id FROM dictations WHERE raw_text = ?1 ORDER BY id DESC LIMIT 1",
                [text],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE dictations SET transaction_id = ?1 WHERE id = ?2",
                params![transaction_id, id],
            )
            .unwrap();
        id
    }

    #[test]
    fn paste_proof_links_to_pending_history_and_deletes_with_its_origin() {
        let dir = temp_dir("learning-proof-pending-history-link");
        let transaction_id = "fixture-transaction-001";
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        let owner = register_pending_history_owner(&dir, transaction_id).unwrap();
        let intent =
            pending_history_intent_for_test(transaction_id, "ok", "history", "I prefer VoiceFlow");
        write_pending_history_intent(&dir, &intent, &owner).unwrap();

        let observation = LearnPairObservation::paste_session_with_dictation_origin(
            "session-001",
            &pair_key,
            transaction_id,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&observation),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        let connection = open_history(&dir).unwrap();
        let pending_link: Option<i64> = connection
            .query_row(
                "SELECT source_history_id FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_link, None);
        drop(connection);

        promote_pending_history_intent(&dir, transaction_id, &owner).unwrap();
        drop(owner);
        let connection = open_history(&dir).unwrap();
        let history_id: i64 = connection
            .query_row(
                "SELECT id FROM dictations WHERE transaction_id = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        let linked_id: Option<i64> = connection
            .query_row(
                "SELECT source_history_id FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked_id, Some(history_id));
        connection
            .execute(
                "INSERT INTO dictation_revisions (dictation_id, created_at, final_text, revision_reason)
                 VALUES (?1, datetime('now'), 'I prefer VoiceFlo', 'manual')",
                [history_id],
            )
            .unwrap();
        let revision_id = connection.last_insert_rowid();
        drop(connection);

        let history_edit =
            LearnPairObservation::history_edit_from_store(&dir, history_id, revision_id, &pair_key)
                .unwrap()
                .unwrap();
        assert!(history_edit.proof_eligible());
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&history_edit),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        assert_eq!(get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits, 1);
        let connection = open_history(&dir).unwrap();
        let enrichment: (Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT source_revision_id, channel FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(enrichment, (Some(revision_id), Some("history_edit".into())));
        drop(connection);

        delete_history(&dir, history_id).unwrap();
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let connection = open_history(&dir).unwrap();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM learn_observations WHERE dictation_origin = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
        drop(connection);
        let late_paste = LearnPairObservation::paste_session_with_dictation_origin(
            "late-session",
            &pair_key,
            transaction_id,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&late_paste),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::UnknownDictationOrigin)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );

        let history_first_origin = "history-first-origin-002";
        let history_first_id =
            insert_test_history_origin(&dir, "history-first proof origin", history_first_origin);
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "INSERT INTO dictation_revisions (dictation_id, created_at, final_text, revision_reason)
                 VALUES (?1, datetime('now'), 'I prefer VoiceFlo', 'manual')",
                [history_first_id],
            )
            .unwrap();
        let history_first_revision = connection.last_insert_rowid();
        drop(connection);

        let history_first = LearnPairObservation::history_edit_from_store(
            &dir,
            history_first_id,
            history_first_revision,
            &pair_key,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&history_first),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        let paste_replay = LearnPairObservation::paste_session_with_dictation_origin(
            "history-first-session",
            &pair_key,
            history_first_origin,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&paste_replay),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        let connection = open_history(&dir).unwrap();
        let history_first_readback: (Option<i64>, Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT source_history_id, source_revision_id, channel
                 FROM learn_observations WHERE dictation_origin = ?1",
                [history_first_origin],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            history_first_readback,
            (
                Some(history_first_id),
                Some(history_first_revision),
                Some("history_edit".into())
            )
        );
        drop(connection);

        delete_history(&dir, history_first_id).unwrap();
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let connection = open_history(&dir).unwrap();
        let deleted_origin_receipts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM learn_observations WHERE dictation_origin = ?1",
                [history_first_origin],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(deleted_origin_receipts, 0);
        drop(connection);
        let late_paste = LearnPairObservation::paste_session_with_dictation_origin(
            "history-first-late-session",
            &pair_key,
            history_first_origin,
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&late_paste),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::UnknownDictationOrigin)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn independent_proof_threshold_ignores_receipt_hits_and_replays() {
        let dir = temp_dir("learn-pair-independent-proof-threshold");
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        for _ in 0..3 {
            upsert_learn_pair(&dir, &pair_key, "VoiceFlo", "VoiceFlow").unwrap();
        }
        assert!(!promote_learn_pair_if_eligible(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            None,
            2,
            false,
        )
        .unwrap());

        let first_id = insert_test_history_origin(&dir, "first proof origin", "origin-first");
        let second_id = insert_test_history_origin(&dir, "second proof origin", "origin-second");
        let weak = LearnPairObservation::paste_session_with_dictation_origin(
            "session-weak-first",
            &pair_key,
            "origin-first",
        );
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&weak),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            1
        );
        assert_eq!(
            count_research_fixture_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        assert_eq!(
            promote_learn_pair_if_eligible_with_status(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                2,
                false,
            )
            .unwrap(),
            LearnPairPromotionStatus::Ineligible,
            "a production paste proof cannot satisfy the research-only threshold"
        );

        let first_fixture =
            LearnPairObservation::research_fixture_origin("research-origin-first", &pair_key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&first_fixture),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            promote_learn_pair_if_eligible_with_status(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                2,
                false,
            )
            .unwrap(),
            LearnPairPromotionStatus::Ineligible,
            "one distinct research fixture origin remains below threshold"
        );
        let second_fixture =
            LearnPairObservation::research_fixture_origin("research-origin-second", &pair_key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&second_fixture),
            )
            .unwrap(),
            LearnPairUpsert::Stored(_)
        ));
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            3,
            "shared observation storage retains both weak and research sources"
        );
        assert_eq!(
            count_research_fixture_learn_pair_hits(&dir, &pair_key).unwrap(),
            2
        );
        assert_eq!(
            promote_learn_pair_if_eligible_with_status(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                2,
                false,
            )
            .unwrap(),
            LearnPairPromotionStatus::Promoted
        );
        let hits_before_replay = get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits;
        let replay =
            LearnPairObservation::research_fixture_origin("research-origin-first", &pair_key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &pair_key,
                "VoiceFlo",
                "VoiceFlow",
                None,
                Some(&replay),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(
            get_learn_pair(&dir, &pair_key).unwrap().unwrap().hits,
            hits_before_replay
        );
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            3
        );
        assert_eq!(
            count_research_fixture_learn_pair_hits(&dir, &pair_key).unwrap(),
            2
        );
        let connection = open_history(&dir).unwrap();
        connection
            .execute(
                "DELETE FROM dictations WHERE id IN (?1, ?2)",
                params![first_id, second_id],
            )
            .unwrap();
        drop(connection);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            2,
            "fixture receipts remain while the History-linked weak receipt is deleted"
        );
        assert_eq!(
            count_research_fixture_learn_pair_hits(&dir, &pair_key).unwrap(),
            2
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dictionary_effect_versions_reject_stale_ack_and_cancel_pending_work() {
        let dir = temp_dir("dictionary-effect-versions");
        let pair_key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        ensure_learn_pair_promoted(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        let add = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert_eq!(add.action, DictionaryEffectAction::Add);
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, add.version - 1).unwrap());
        assert!(acknowledge_dictionary_effect(&dir, &pair_key, add.version).unwrap());

        ensure_learn_pair_promoted(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        let reconfirmed_add = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert_eq!(reconfirmed_add.action, DictionaryEffectAction::Add);
        assert!(reconfirmed_add.version > add.version);
        assert!(cancel_pending_dictionary_effects_for_word(&dir, "voiceflow").unwrap());
        assert!(list_pending_dictionary_effects(&dir).unwrap().is_empty());
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, reconfirmed_add.version).unwrap());

        ensure_learn_pair_promoted(
            &dir,
            &pair_key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        let next_add = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert!(acknowledge_dictionary_effect(&dir, &pair_key, next_add.version).unwrap());
        assert!(tombstone_learn_pair(&dir, &pair_key).unwrap());
        let remove = list_pending_dictionary_effects(&dir).unwrap().remove(0);
        assert_eq!(remove.action, DictionaryEffectAction::Remove);
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, next_add.version).unwrap());
        assert!(!cancel_pending_dictionary_effects_for_word(&dir, "voiceflow").unwrap());
        assert_eq!(
            list_pending_dictionary_effects(&dir).unwrap(),
            vec![remove.clone()]
        );
        assert!(!acknowledge_dictionary_effect(&dir, &pair_key, remove.version - 1).unwrap());
        assert!(acknowledge_dictionary_effect(&dir, &pair_key, remove.version).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clear_all_data_deletes_learn_pairs() {
        let dir = temp_dir("learn-pairs-clear");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        clear_all_data(&dir).unwrap();
        assert!(list_learn_pairs(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learned_usage_deduplicates_processing_passes_hides_forgotten_terms_and_clears() {
        let dir = temp_dir("learned-usage");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&LearnPairScope::default()))
            .unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        record_learned_term_usage(&dir, &["知乎".into(), "知乎".into()]).unwrap();
        record_learned_term_usage(&dir, &["知乎".into()]).unwrap();
        let usage = list_learned_term_usage(&dir).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].word, "知乎");
        assert_eq!(usage[0].replacement_runs, 2);
        assert!(!usage[0].last_replaced_at.is_empty());
        tombstone_learn_pair(&dir, &key).unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        clear_all_data(&dir).unwrap();
        let count: u64 = open_history(&dir)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM learned_term_usage", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learned_usage_membership_matches_ascii_case_of_promoted_rule_surface() {
        let dir = temp_dir("learned-usage-ascii-case");
        let key = crate::dictionary_learn::pair_key("VoiceFlo", "VoiceFlow");
        ensure_learn_pair_promoted(
            &dir,
            &key,
            "VoiceFlo",
            "VoiceFlow",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        record_learned_term_usage(&dir, &["voiceflow".into()]).unwrap();

        let usage = list_learned_term_usage(&dir).unwrap();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].word, "voiceflow");
        assert_eq!(usage[0].replacement_runs, 1);

        tombstone_learn_pair(&dir, &key).unwrap();
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn automatic_scene_pair_marker_is_opt_in_and_tombstones_still_disarm_it() {
        let dir = temp_dir("auto-scoped-pair");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let scope = LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&scope)).unwrap();
        let legacy = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(!legacy.auto_scoped);
        assert!(!legacy.manual_scoped);

        mark_learn_pair_promoted_with_scope_kind(&dir, &key, true).unwrap();
        let learned = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(learned.auto_scoped);
        assert!(!learned.manual_scoped);
        tombstone_learn_pair(&dir, &key).unwrap();
        let forgotten = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(forgotten.auto_scoped);
        assert!(!forgotten.is_live_promoted());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn guarded_automatic_pair_promotion_requires_exact_live_candidate() {
        let dir = temp_dir("guarded-auto-promotion");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let scope = LearnPairScope {
            family: Some("prompt_or_code".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        upsert_learn_pair_with_observation(&dir, &key, "知呼", "知乎", Some(&scope), None).unwrap();

        assert!(
            !promote_learn_pair_if_eligible(&dir, &key, "知呼", "知乎", Some(&scope), 2, true,)
                .unwrap()
        );
        assert!(!promote_learn_pair_if_eligible(
            &dir,
            &key,
            "wrong",
            "知乎",
            Some(&scope),
            1,
            true,
        )
        .unwrap());
        assert!(!promote_learn_pair_if_eligible(
            &dir,
            &key,
            "知呼",
            "wrong",
            Some(&scope),
            1,
            true,
        )
        .unwrap());
        assert!(
            !promote_learn_pair_if_eligible(&dir, &key, "知呼", "知乎", None, 1, true,).unwrap()
        );

        upsert_learn_pair_with_observation(&dir, &key, "知呼", "知乎", Some(&scope), None).unwrap();
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        assert!(
            !promote_learn_pair_if_eligible(&dir, &key, "知呼", "知乎", Some(&scope), 2, true,)
                .unwrap()
        );
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(row.hits, 2);
        assert!(!row.promoted);
        assert!(!row.auto_scoped);
        assert!(row.ignored);
        assert!(row.tombstoned_at.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_v12_migration_preserves_learning_without_inventing_usage() {
        let dir = temp_dir("learned-usage-migration");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        ensure_learn_pair_promoted(&dir, &key, "知呼", "知乎", Some(&LearnPairScope::default()))
            .unwrap();
        let connection = open_history(&dir).unwrap();
        connection
            .execute_batch("DROP TABLE learned_term_usage; PRAGMA user_version = 12;")
            .unwrap();
        drop(connection);
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());
        assert!(list_learn_pairs(&dir).unwrap()[0].is_live_promoted());
        assert!(dir.join("history.sqlite.v12.bak").is_file());
        clear_all_data(&dir).unwrap();
        assert!(!dir.join("history.sqlite.v12.bak").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_concurrent_increments_are_not_lost() {
        let dir = temp_dir("learn-pairs-race");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let dir = &dir;
                let key = &key;
                scope.spawn(move || {
                    upsert_learn_pair(dir, key, "知呼", "知乎").unwrap();
                });
            }
        });
        assert_eq!(get_learn_pair(&dir, &key).unwrap().unwrap().hits, 8);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_pending_cap_rejects_new_pairs() {
        let dir = temp_dir("learn-pairs-cap");
        for index in 0..256 {
            let before = format!("b{index}");
            let after = format!("a{index}");
            let key = crate::dictionary_learn::pair_key(&before, &after);
            assert!(upsert_learn_pair(&dir, &key, &before, &after)
                .unwrap()
                .is_some());
        }
        let extra = crate::dictionary_learn::pair_key("知呼", "知乎");
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &extra, "知呼", "知乎", None).unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::PendingCapacity)
        ));
        let first = crate::dictionary_learn::pair_key("b0", "a0");
        assert_eq!(
            upsert_learn_pair(&dir, &first, "b0", "a0")
                .unwrap()
                .unwrap()
                .hits,
            2
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ignore_writes_tombstone_and_blocks_upsert() {
        let dir = temp_dir("learn-pairs-tombstone");
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        upsert_learn_pair(&dir, &key, "知呼", "知乎").unwrap();
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.ignored);
        assert!(row.tombstoned_at.is_some());
        assert!(!row.is_pending());
        assert!(matches!(
            upsert_learn_pair_with_scope(&dir, &key, "知呼", "知乎", None).unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::TombstoneOrIgnored)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learning_receipts_count_distinct_history_rows_once_and_follow_tombstones() {
        let dir = temp_dir("learning-observation-receipts");
        for index in 0..4 {
            insert_history(
                &dir,
                &format!("raw {index}"),
                &format!("final {index}"),
                1.0,
                false,
            )
            .unwrap();
        }
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let scope = LearnPairScope {
            family: Some("work_chat".into()),
            mapping_id: Some("codex".into()),
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        let first_revision = LearnPairObservation::history_edit(1, 11, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&first_revision),
            )
            .unwrap(),
            LearnPairUpsert::Stored(row) if row.hits == 1
        ));

        // A later revision of the same History row is still only one edit
        // source for this exact pair.
        let same_history = LearnPairObservation::history_edit(1, 12, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&same_history),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        assert_eq!(get_learn_pair(&dir, &key).unwrap().unwrap().hits, 1);

        let second_history = LearnPairObservation::history_edit(2, 21, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&second_history),
            )
            .unwrap(),
            LearnPairUpsert::Stored(row) if row.hits == 2
        ));
        let conflict = LearnPairScope {
            native_bundle: Some("com.apple.TextEdit".into()),
            ..scope.clone()
        };
        let conflicting_history = LearnPairObservation::history_edit(3, 31, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&conflict),
                Some(&conflicting_history),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::ScopeConflict)
        ));
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&conflict),
                Some(&conflicting_history),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::ScopeConflict)
        ));
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let after_tombstone = LearnPairObservation::history_edit(4, 41, &key);
        assert!(matches!(
            upsert_learn_pair_with_observation(
                &dir,
                &key,
                "知呼",
                "知乎",
                Some(&scope),
                Some(&after_tombstone),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::TombstoneOrIgnored)
        ));
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert_eq!(row.hits, 2);
        assert!(row.ignored && row.tombstoned_at.is_some());

        delete_history(&dir, 1).unwrap();
        let connection = open_history(&dir).unwrap();
        let remaining_receipts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM learn_observations WHERE source_history_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_receipts, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn originless_history_edit_keeps_scoped_identity_and_dedupes_by_history_row() {
        let dir = temp_dir("originless-history-scoped-observation");
        let before = "黎明";
        let after = "李明";
        let scope = LearnPairScope {
            family: Some("personal_chat".into()),
            mapping_id: Some("messages".into()),
            native_bundle: Some("com.apple.MobileSMS".into()),
            ..LearnPairScope::default()
        };
        let pair_key = crate::store::scoped_word_rule_key(
            before,
            after,
            &scope,
            LearnPairMatchPolicy::IgnoreCase,
        )
        .into_string();
        insert_history(&dir, before, before, 1.0, false).unwrap();
        let connection = open_history(&dir).unwrap();
        let history_id = connection
            .query_row(
                "SELECT id FROM dictations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE dictations SET transaction_id = NULL WHERE id = ?1",
                [history_id],
            )
            .unwrap();
        drop(connection);

        let first_revision = save_manual_history_revision(&dir, history_id, after)
            .unwrap()
            .revision_id
            .unwrap();
        let first_observation = LearnPairObservation::history_edit_from_store(
            &dir,
            history_id,
            first_revision,
            &pair_key,
        )
        .unwrap()
        .unwrap();
        assert!(first_observation.uses_scoped_word_identity());
        assert_eq!(
            first_observation.rule_key.as_deref(),
            Some(pair_key.as_str())
        );
        assert_eq!(first_observation.channel, Some("history_edit"));
        assert!(!first_observation.proof_eligible());
        assert_eq!(first_observation.source_history_id, Some(history_id));
        assert_eq!(first_observation.source_revision_id, Some(first_revision));

        assert!(matches!(
            upsert_auto_scoped_learn_pair_with_observation(
                &dir,
                &pair_key,
                before,
                after,
                Some(&scope),
                Some(&first_observation),
            )
            .unwrap(),
            LearnPairUpsert::Stored(row) if row.hits == 1 && row.auto_scoped
        ));

        // Revisions that reverse and repeat the same pair on one originless
        // History row must not make the candidate look like a second source.
        save_manual_history_revision(&dir, history_id, before).unwrap();
        let repeated_revision = save_manual_history_revision(&dir, history_id, after)
            .unwrap()
            .revision_id
            .unwrap();
        let repeated_observation = LearnPairObservation::history_edit_from_store(
            &dir,
            history_id,
            repeated_revision,
            &pair_key,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            repeated_observation.observation_key,
            first_observation.observation_key
        );
        assert!(matches!(
            upsert_auto_scoped_learn_pair_with_observation(
                &dir,
                &pair_key,
                before,
                after,
                Some(&scope),
                Some(&repeated_observation),
            )
            .unwrap(),
            LearnPairUpsert::Skipped(LearnPairSkipReason::DuplicateObservation)
        ));
        let row = get_learn_pair(&dir, &pair_key).unwrap().unwrap();
        assert_eq!(row.hits, 1);
        assert!(!row.promoted);
        assert!(row.auto_scoped);
        assert_eq!(
            count_independent_learn_pair_hits(&dir, &pair_key).unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_all_data_deletes_style_drafts() {
        let dir = temp_dir("style-drafts-clear");
        upsert_style_draft(&dir, "wechat", "fewer_periods", "你好。", "你好。", "你好").unwrap();
        assert!(!list_style_drafts(&dir).unwrap().is_empty());
        clear_all_data(&dir).unwrap();
        assert!(list_style_drafts(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn learn_pair_schema_adds_scope_columns_on_existing_db() {
        let dir = temp_dir("learn-pairs-alter");
        std::fs::create_dir_all(&dir).unwrap();
        let connection = rusqlite::Connection::open(dir.join("history.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE learn_pairs (
                    pair_key TEXT PRIMARY KEY,
                    before_surface TEXT NOT NULL,
                    after_surface TEXT NOT NULL,
                    hits INTEGER NOT NULL,
                    promoted INTEGER NOT NULL DEFAULT 0,
                    last_at TEXT NOT NULL
                );
                PRAGMA user_version = 6;",
            )
            .unwrap();
        drop(connection);
        let key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let row = upsert_learn_pair_with_scope(
            &dir,
            &key,
            "知呼",
            "知乎",
            Some(&LearnPairScope {
                family: Some("personal_chat".into()),
                mapping_id: Some("wechat".into()),
                browser_host: None,
                native_bundle: Some("com.tencent.xinWeChat".into()),
            }),
        )
        .unwrap();
        let LearnPairUpsert::Stored(row) = row else {
            panic!("fresh migration fixture was unexpectedly suppressed")
        };
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.mapping_id.as_deref(), Some("wechat"));
        assert!(!row.pinned);
        assert!(!row.ignored);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_scene_and_confirm_keep_wechat_snapshot_scope() {
        let dir = temp_dir("history-wechat-scope");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.profile.id = "chat.personal".into();
        snapshot.profile.family = crate::context::ContextFamily::PersonalChat;
        snapshot.target_guard.bundle_id = Some("com.tencent.xinWeChat".into());
        snapshot.target_guard.browser_host = None;
        insert_history_with_context(&dir, "知呼", "知乎", 1.0, false, &snapshot).unwrap();
        let id = get_history_page(&dir, 1, None, None).unwrap().items[0].id;
        let scene = history_scene(&dir, id).unwrap();
        assert_eq!(scene.profile_id.as_deref(), Some("chat.personal"));
        assert_eq!(scene.family.as_deref(), Some("personal_chat"));
        assert_eq!(
            scene.native_bundle.as_deref(),
            Some("com.tencent.xinWeChat")
        );
        assert!(history_context(&dir, id).unwrap().is_some());
        let key = crate::dictionary_learn::pair_key("派森", "Python");
        ensure_learn_pair_promoted(&dir, &key, "派森", "Python", Some(&scene.learn_scope()))
            .unwrap();
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert_eq!(row.family.as_deref(), Some("personal_chat"));
        assert_eq!(row.native_bundle.as_deref(), Some("com.tencent.xinWeChat"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_persists_only_the_selected_mapping_identity_fingerprint() {
        let dir = temp_dir("history-mapping-identity");
        let mapping = crate::context::AppMapping {
            id: "codex".into(),
            label: "Codex".into(),
            family: crate::context::ContextFamily::DeveloperCollaboration,
            writing_task_id: None,
            writing_preferences: Vec::new(),
            preference_guidance_provider_approved: false,
            preference_revision: 0,
            deleted: false,
            mode_id: None,
            bundle_id: Some("com.openai.codex".into()),
            executable: None,
            browser_host: None,
            browser_path_prefix: None,
            focused_field: None,
            source_permissions: Default::default(),
            style_example_input: None,
            style_example_output: None,
            style_example_pairs: Vec::new(),
            style_examples_approved: false,
            enabled: true,
            cleanup_effort: None,
            cleanup_intensity: None,
            cleanup_enabled: true,
            dictionary_learn_enabled: true,
        };
        let fingerprint = mapping.declared_identity_fingerprint();
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.profile.id = "user.codex".into();
        snapshot.profile.family = mapping.family;
        snapshot.target_guard.bundle_id = mapping.bundle_id.clone();
        snapshot.mapping_identity = Some(fingerprint.clone());
        insert_history_with_context(&dir, "raw", "final", 1.0, false, &snapshot).unwrap();
        let id = get_history_page(&dir, 1, None, None).unwrap().items[0].id;
        assert_eq!(
            history_scene(&dir, id).unwrap().mapping_identity.as_deref(),
            Some(fingerprint.as_str())
        );
        assert_eq!(
            save_manual_history_revision(&dir, id, "edited final")
                .unwrap()
                .scene
                .mapping_identity
                .as_deref(),
            Some(fingerprint.as_str())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn manual_scoped_pair_is_app_bound_and_refuses_scope_collisions() {
        let dir = temp_dir("manual-scoped-pair");
        let key = crate::dictionary_learn::pair_key("L O M", "LLM");
        let scope = LearnPairScope {
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        ensure_manual_scoped_learn_pair(&dir, &key, "L O M", "LLM", &scope).unwrap();
        let row = get_learn_pair(&dir, &key).unwrap().unwrap();
        assert!(row.is_live_promoted());
        assert!(row.manual_scoped);
        assert_eq!(row.native_bundle.as_deref(), Some("com.openai.codex"));
        assert_eq!(row.family, None);
        assert_eq!(row.mapping_id, None);
        assert_eq!(row.browser_host, None);

        let observed = LearnPairScope {
            native_bundle: Some("com.openai.codex".into()),
            ..LearnPairScope::default()
        };
        assert_eq!(observed.assess_record(&row), LearnPairScopeMatch::Match);
        let foreign = LearnPairScope {
            native_bundle: Some("com.apple.TextEdit".into()),
            ..LearnPairScope::default()
        };
        assert_eq!(foreign.assess_record(&row), LearnPairScopeMatch::Mismatch);
        let unknown = LearnPairScope::default();
        assert_eq!(unknown.assess_record(&row), LearnPairScopeMatch::Unknown);

        let conflicting_scope = LearnPairScope {
            native_bundle: Some("com.apple.TextEdit".into()),
            ..LearnPairScope::default()
        };
        assert!(
            ensure_manual_scoped_learn_pair(&dir, &key, "L O M", "LLM", &conflicting_scope)
                .is_err()
        );
        assert!(ensure_manual_scoped_learn_pair(
            &dir,
            &key,
            "L O M",
            "LLM",
            &LearnPairScope::default()
        )
        .is_err());
        assert_eq!(
            get_learn_pair(&dir, &key).unwrap().unwrap().native_bundle,
            scope.native_bundle
        );
        assert!(tombstone_learn_pair(&dir, &key).unwrap());
        let rebound =
            ensure_manual_scoped_learn_pair(&dir, &key, "L O M", "LLM", &conflicting_scope)
                .unwrap();
        assert!(rebound.is_live_promoted());
        assert_eq!(rebound.native_bundle.as_deref(), Some("com.apple.TextEdit"));
        assert_eq!(rebound.hits, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_and_revision_policies_keep_only_safe_metadata() {
        let dir = temp_dir("history-context-source-safety");
        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.policy.style_examples_approved = true;
        snapshot.policy.style_example_input = Some("private style input".into());
        snapshot.policy.style_example_output = Some("private style output".into());
        snapshot.policy.style_example_pairs = vec![crate::context::StyleExamplePair {
            input: "private pair input".into(),
            output: "private pair output".into(),
        }];
        snapshot
            .evidence
            .items
            .push(crate::screen_text::ContextEvidenceItem {
                source: crate::screen_text::ContextEvidenceSource::Ax,
                kind: crate::screen_text::ContextEvidenceKind::NearbyText,
                value: "private live window excerpt".into(),
                confidence_milli: Some(900),
                truncated: false,
            });

        insert_history_with_context(&dir, "spoken", "final", 1.0, false, &snapshot).unwrap();
        let stored = history_scene(&dir, 1).unwrap().policy.unwrap();
        assert!(!stored.style_examples_approved);
        assert!(stored.style_example_input.is_none());
        assert!(stored.style_example_output.is_none());
        assert!(stored.style_example_pairs.is_empty());

        save_history_revision(
            &dir,
            1,
            "revised",
            None,
            None,
            None,
            Some(&snapshot.policy),
            "manual_edit",
        )
        .unwrap();
        let revision = get_history_revisions(&dir, 1).unwrap().remove(0);
        let serialized = revision.context_policy.unwrap().to_string();
        assert!(!serialized.contains("private style"));
        assert!(!serialized.contains("private pair"));
        assert!(!serialized.contains("private live window"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unsupported_spool_manifest_is_removed_before_it_consumes_quota() {
        let dir = temp_dir("unsupported-spool");
        let session = dir.join("spool").join("unsupported");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("manifest.json"),
            r#"{"schema_version":999,"session_id":"unsupported","created_at_ms":0,"status":"active","chunks":[]}"#,
        )
        .unwrap();

        assert!(recover_spool(&dir, 7).unwrap().is_empty());
        assert!(!session.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn success_gold_audio_is_playable_but_not_retryable() {
        let dir = temp_dir("gold-success");
        let path = write_gold_file(&dir, "12.wav", b"RIFF").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw spoken",
            "Cleaned text.",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert!(item.has_audio);
        assert!(!item.retryable);
        assert!(!item.verbatim_reviewed);
        assert_eq!(item.verbatim_text, None);
        assert!(failed_spool(&dir, item.id).unwrap().is_none());
        assert_eq!(history_audio_bytes(&dir, item.id).unwrap(), b"RIFF");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_gold_file_rejects_parent_dir_escape() {
        let dir = temp_dir("gold-escape");
        assert!(write_gold_file(&dir, "../escape.wav", b"nope").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_verbatim_marks_reviewed_and_can_unreview() {
        let dir = temp_dir("verbatim-review");
        insert_history(&dir, "draft words", "Cleaned.", 1.0, false).unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        save_verbatim(&dir, id, "  mouth words  ", true).unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.verbatim_text.as_deref(), Some("mouth words"));
        assert!(item.verbatim_reviewed);
        save_verbatim(&dir, id, "", false).unwrap();
        let item = get_history(&dir, 1).unwrap().remove(0);
        assert_eq!(item.verbatim_text, None);
        assert!(!item.verbatim_reviewed);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn purge_and_delete_remove_gold_files() {
        let dir = temp_dir("gold-purge");
        let path = write_gold_file(&dir, "kept.wav", b"gold").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "raw",
            "final",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&path),
        )
        .unwrap();
        let id = get_history(&dir, 1).unwrap()[0].id;
        assert_eq!(purge_gold_audio(&dir, 0).unwrap(), 1);
        assert!(!path.exists());
        assert!(!get_history(&dir, 1).unwrap()[0].has_audio);

        let path = write_gold_file(&dir, "again.wav", b"gold").unwrap();
        let c = open_history(&dir).unwrap();
        c.execute(
            "UPDATE dictations SET raw_audio_path=? WHERE id=?",
            params![path.to_string_lossy(), id],
        )
        .unwrap();
        drop(c);
        delete_history(&dir, id).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn export_gold_corpus_exports_gold_audio_with_raw_text() {
        let dir = temp_dir("gold-export");
        let downloads = dir.join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let gold = write_gold_file(&dir, "kept.wav", b"RIFFGOLD").unwrap();
        let spool = write_spool_file(&dir, Path::new("failed-short.wav"), b"SPOOL").unwrap();
        let context = crate::context::ContextSnapshot::general();
        insert_history_with_delivery_and_spool(
            &dir,
            "spoken words",
            "Cleaned words.",
            1.0,
            false,
            None,
            "ok",
            "paste",
            None,
            &context,
            Some(&gold),
        )
        .unwrap();
        insert_history_with_delivery_and_spool(
            &dir,
            "failed raw",
            "",
            1.0,
            false,
            Some("asr_failed"),
            "failed",
            "none",
            None,
            &context,
            Some(&spool),
        )
        .unwrap();

        #[cfg(unix)]
        let downloads_mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&downloads).unwrap().permissions().mode() & 0o777
        };
        let exported = export_gold_corpus(&dir, &downloads, "zh").unwrap();
        assert_eq!(exported.count, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&downloads).unwrap().permissions().mode() & 0o777,
                downloads_mode
            );
            assert_eq!(
                std::fs::metadata(&exported.directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let wav_path = Path::new(&exported.directory).join("utt0001.wav");
        let wav = std::fs::read(&wav_path).unwrap();
        assert_eq!(wav, b"RIFFGOLD");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&wav_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&exported.jsonl_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let jsonl = std::fs::read_to_string(&exported.jsonl_path).unwrap();
        assert!(jsonl.contains("language Chinese<asr_text>spoken words"));
        assert!(!jsonl.contains("Cleaned words"));
        assert!(!jsonl.contains("failed raw"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod handy_settings_tests {
    use super::*;
    #[test]
    fn translation_shortcut_defaults_off_and_round_trips_in_public_view() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.translation_hotkey.is_empty());
        settings.translation_hotkey = "Command+Shift+T".into();
        settings.translation_target_language = "ja".into();
        settings.normalize();
        let restored: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(restored.translation_hotkey, "Command+Shift+T");
        let view = serde_json::to_value(SettingsView::from(&restored)).unwrap();
        assert_eq!(view["translation_hotkey"], "Command+Shift+T");
        assert_eq!(view["translation_target_language"], "ja");
    }

    #[test]
    fn translation_shortcut_rejects_alias_conflicts_and_accepts_fn() {
        for field in ["hotkey", "selected", "screen", "verbatim"] {
            let mut settings = Settings {
                translation_hotkey: "Cmd+Shift+T".into(),
                ..Settings::default()
            };
            match field {
                "hotkey" => settings.hotkey = "Command+Shift+T".into(),
                "selected" => settings.selected_action_hotkey = "Command+Shift+T".into(),
                "screen" => settings.screen_action_hotkey = "Command+Shift+T".into(),
                _ => settings.verbatim_hotkey = "Command+Shift+T".into(),
            }
            assert!(settings
                .validate()
                .unwrap_err()
                .to_string()
                .contains("hotkey conflicts"));
        }
        let settings = Settings {
            translation_hotkey: "Fn".into(),
            ..Settings::default()
        };
        settings.validate().unwrap();
        let mut fn_conflict = settings.clone();
        fn_conflict.hotkey = "Globe".into();
        assert!(fn_conflict.validate().is_err());
    }
    #[test]
    fn missing_legacy_mode_deserializes_as_tap() {
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.activation_mode, "tap");
    }
    #[test]
    fn conservative_settings_round_trip_and_clamp() {
        let mut settings = Settings {
            extra_recording_buffer_ms: 9000,
            audio_feedback_volume: f32::NAN,
            verbatim_hotkey: "Cmd+Shift+V".into(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.extra_recording_buffer_ms, 2000);
        assert_eq!(settings.audio_feedback_volume, 0.6);
        assert!(
            !settings.vad_enabled
                && !settings.audio_feedback_enabled
                && !settings.fuzzy_dictionary_enabled
                && !settings.always_on_microphone
                && !settings.autostart_enabled
                && !settings.debug_mode
        );
        let encoded = serde_json::to_value(&settings).unwrap();
        let restored: Settings = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.verbatim_hotkey, settings.verbatim_hotkey);
        let view = serde_json::to_value(SettingsView::from(&restored)).unwrap();
        for key in [
            "verbatim_hotkey",
            "extra_recording_buffer_ms",
            "audio_feedback_enabled",
            "audio_feedback_volume",
            "vad_enabled",
            "fuzzy_dictionary_enabled",
            "always_on_microphone",
            "clamshell_microphone",
            "autostart_enabled",
            "whats_new_last_seen_version",
            "debug_mode",
        ] {
            assert!(view.get(key).is_some(), "{key}");
        }
    }
}

#[cfg(test)]
mod guide_acknowledgement_fixtures {
    use super::*;
    #[test]
    fn old_settings_and_independent_explicit_ack_round_trip() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            settings.guide_acknowledgements,
            GuideAcknowledgements::default()
        );
        assert!(settings
            .guide_acknowledgements
            .acknowledge(OnDemandGuideId::ContextScreen, 1)
            .unwrap());
        assert!(!settings
            .guide_acknowledgements
            .acknowledge(OnDemandGuideId::ContextScreen, 1)
            .unwrap());
        assert_eq!(settings.guide_acknowledgements.dictionary_learning, None);
        assert_eq!(settings.guide_acknowledgements.context_screen, Some(1));
        let saved: Settings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(
            saved.guide_acknowledgements,
            settings.guide_acknowledgements
        );
        assert!(saved
            .guide_acknowledgements
            .clone()
            .acknowledge(OnDemandGuideId::ContextScreen, 2)
            .is_err());
        assert!(serde_json::from_str::<OnDemandGuideId>(r#""unknown_guide""#).is_err());
    }
    #[test]
    fn teaching_ack_does_not_become_recording_configuration_or_feature_success() {
        let before = Settings::default();
        let mut after = before.clone();
        after
            .guide_acknowledgements
            .acknowledge(OnDemandGuideId::SelectedTranslation, 1)
            .unwrap();
        assert!(!crate::dictation_configuration_changed(&before, &after));
        assert_eq!(before.onboarded, after.onboarded);
        assert_eq!(
            before.dictionary_learn_enabled,
            after.dictionary_learn_enabled
        );
        after.asr_model.push_str("-changed");
        assert!(crate::dictation_configuration_changed(&before, &after));
    }

    #[test]
    fn clear_all_data_with_options_preserves_confirmed_rules_by_default_and_reconciles_sources() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-clear-options-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let confirmed_key = crate::dictionary_learn::pair_key("知呼", "知乎");
        let pending_key = crate::dictionary_learn::pair_key("微兴", "微信");
        ensure_learn_pair_promoted(
            &dir,
            &confirmed_key,
            "知呼",
            "知乎",
            Some(&LearnPairScope::default()),
        )
        .unwrap();
        upsert_learn_pair(&dir, &pending_key, "微兴", "微信").unwrap();
        upsert_style_draft(&dir, "wechat", "fewer_periods", "你好。", "你好。", "你好").unwrap();
        record_learned_term_usage(&dir, &["知乎".into()]).unwrap();
        insert_failed_history(&dir, "raw source", 1.0, None).unwrap();

        let guard = begin_clear_all_data(&dir).unwrap();
        clear_all_data_under_guard_with_options(&dir, &guard, false).unwrap();
        drop(guard);

        assert!(get_history_page(&dir, 10, None, None)
            .unwrap()
            .items
            .is_empty());
        assert!(get_learn_pair(&dir, &confirmed_key).unwrap().is_some());
        assert!(get_learn_pair(&dir, &pending_key).unwrap().is_none());
        assert!(list_style_drafts(&dir).unwrap().is_empty());
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());

        let guard2 = begin_clear_all_data(&dir).unwrap();
        clear_all_data_under_guard_with_options(&dir, &guard2, true).unwrap();
        drop(guard2);
        assert!(list_learn_pairs(&dir).unwrap().is_empty());
        assert!(list_style_drafts(&dir).unwrap().is_empty());
        assert!(list_learned_term_usage(&dir).unwrap().is_empty());

        let mut settings = Settings::default();
        settings.add_dictionary_source("VoiceFlow", DictionarySource::Manual);
        settings.add_dictionary_source("知乎", DictionarySource::Rule(confirmed_key.clone()));
        settings.add_dictionary_source("Shared", DictionarySource::Manual);
        settings.add_dictionary_source("Shared", DictionarySource::Rule("word:v2:shared".into()));
        assert!(settings.remove_all_rule_dictionary_sources());
        assert!(settings.dictionary.contains(&"VoiceFlow".to_owned()));
        assert!(settings.dictionary.contains(&"Shared".to_owned()));
        assert!(!settings.dictionary.contains(&"知乎".to_owned()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scoped_pair_never_stages_global_dictionary_effect_and_global_promotion_preserves_hits() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "voiceflow-scoped-effect-hits-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let scoped_key = "word:v2:host-only";
        let scope = LearnPairScope {
            family: None,
            mapping_id: None,
            browser_host: Some("github.com".into()),
            native_bundle: None,
        };
        upsert_learn_pair_with_scope(&dir, scoped_key, "githab", "GitHub", Some(&scope)).unwrap();
        ensure_learn_pair_promoted(&dir, scoped_key, "githab", "GitHub", Some(&scope)).unwrap();
        let scoped_effects = list_pending_dictionary_effects(&dir).unwrap();
        assert_eq!(scoped_effects.len(), 1);
        assert!(!scoped_effects[0].is_global_rule);
        assert!(!get_learn_pair(&dir, scoped_key)
            .unwrap()
            .unwrap()
            .is_provider_egress_eligible_word_pair());

        let local_key = "word:v2:host-local-only";
        upsert_learn_pair_with_scope(&dir, local_key, "githbu", "GitHub", Some(&scope)).unwrap();
        ensure_local_only_learn_pair_promoted(&dir, local_key, "githbu", "GitHub", Some(&scope))
            .unwrap();
        assert_eq!(list_pending_dictionary_effects(&dir).unwrap().len(), 1);

        let candidate_key = "word:v2:single-hit-candidate";
        upsert_learn_pair_with_scope(&dir, candidate_key, "zhihu", "知乎", Some(&scope)).unwrap();
        let global_key = "word:v2:global-zhihu";
        ensure_learn_pair_promoted_as_global(&dir, candidate_key, global_key, "zhihu", "知乎")
            .unwrap();
        let promoted = get_learn_pair(&dir, global_key).unwrap().unwrap();
        assert!(promoted.promoted);
        assert_eq!(promoted.hits, 1);
        assert!(promoted.is_provider_egress_eligible_word_pair());
        assert!(get_learn_pair(&dir, candidate_key).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
