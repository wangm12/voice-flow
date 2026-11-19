#[allow(unused_imports)]
use super::*;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn export_history_json(dir: &Path) -> anyhow::Result<String> {
    let mut items = Vec::new();
    let mut before_id = None;
    loop {
        let page = get_history_page(dir, 100, before_id, None)?;
        if let Some(last) = page.items.last() {
            before_id = Some(last.id);
        }
        let done = !page.has_more;
        items.extend(page.items);
        if done {
            break;
        }
    }
    Ok(serde_json::to_string_pretty(&items)?)
}

pub fn write_export_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        anyhow::bail!("invalid export path");
    }
    write_atomic_bytes_with_parent_mode(path, contents.as_bytes(), false)
}
pub fn failed_spool(dir: &Path, id: i64) -> anyhow::Result<Option<String>> {
    let c = open_history(dir)?;
    let path: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=? AND status IN ('failed','degraded')",
            [id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let root = dir.join("spool");
    Ok(path.filter(|value| {
        let path = Path::new(value);
        is_safe_spool_path(&root, path) && path.is_file()
    }))
}
#[derive(Debug, Clone, Default)]
pub struct HistoryScene {
    pub policy: Option<crate::context::ContextPolicy>,
    pub profile_id: Option<String>,
    pub family: Option<String>,
    pub browser_host: Option<String>,
    pub native_bundle: Option<String>,
    pub mapping_identity: Option<String>,
}

pub struct ManualHistorySave {
    pub revision_id: Option<i64>,
    /// Stable original dictation transaction identity used for deduplication.
    pub origin_id: Option<String>,
    /// True only when no History revision has ever been written for this
    /// original delivery, so this edit compares against its original output.
    pub first_edit_of_original_delivery: bool,
    /// Original dictation timestamp and device-local calendar day, read from
    /// the History owner row in the same transaction as this save.
    pub source_created_at_ms: Option<u64>,
    pub natural_day: Option<String>,
    /// Short-lived source text exposed only to the trusted local History
    /// classifier; neither field is copied into preference evidence.
    pub raw_text: String,
    pub asr_text: Option<String>,
    pub status: String,
    pub delivery_method: Option<String>,
    pub fallback_reason: Option<String>,
    /// The adopted text immediately before this manual revision. For an
    /// unchanged retry of the latest manual revision, this is reconstructed
    /// from its predecessor so learning can reuse the same origin receipt.
    pub learning_before_text: String,
    pub scene: HistoryScene,
    pub changed: bool,
}

pub struct RecleanHistorySave<'a> {
    pub dictation_id: i64,
    pub final_text: &'a str,
    pub cleanup_status: &'a str,
    pub intent: &'a crate::llm::CleanupIntent,
    pub model: &'a str,
    pub context_policy: &'a crate::context::ContextPolicy,
    pub cleanup_rejected_candidate: Option<&'a str>,
    pub cleanup_guard_reason: Option<&'a str>,
    pub cleanup_attempt_kind: Option<&'a str>,
    pub cleanup_transformations: Option<&'a [String]>,
    pub cleanup_final_guard_reason: Option<&'a str>,
    pub cleanup_failure_reason: Option<&'a str>,
}

impl HistoryScene {
    pub fn learn_scope(&self) -> LearnPairScope {
        LearnPairScope {
            family: self.family.clone(),
            mapping_id: self
                .profile_id
                .as_deref()
                .and_then(|id| id.strip_prefix("user."))
                .map(str::to_owned),
            browser_host: self.browser_host.clone(),
            native_bundle: self.native_bundle.clone(),
        }
    }
}

#[cfg(test)]
pub fn history_context(
    dir: &Path,
    id: i64,
) -> anyhow::Result<Option<crate::context::ContextPolicy>> {
    Ok(history_scene(dir, id)?.policy)
}

pub fn history_scene(dir: &Path, id: i64) -> anyhow::Result<HistoryScene> {
    let c = open_history(dir)?;
    let scene = c
        .query_row(
            "SELECT context_policy_json, context_profile_id, context_family, context_browser_host, context_native_bundle, context_mapping_identity FROM dictations WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((policy_json, profile_id, family, browser_host, native_bundle, mapping_identity)) =
        scene
    else {
        return Ok(HistoryScene::default());
    };
    Ok(HistoryScene {
        policy: policy_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        profile_id,
        family,
        browser_host,
        native_bundle,
        mapping_identity,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn save_history_revision(
    dir: &Path,
    dictation_id: i64,
    final_text: &str,
    cleanup_status: Option<&str>,
    intent: Option<&crate::llm::CleanupIntent>,
    model: Option<&str>,
    context_policy: Option<&crate::context::ContextPolicy>,
    revision_reason: &str,
) -> anyhow::Result<i64> {
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    if revision_reason.trim().is_empty() || revision_reason.len() > 128 {
        anyhow::bail!("history revision reason is invalid");
    }
    let c = open_history(dir)?;
    let intent_json = intent.map(serde_json::to_string).transpose()?;
    let context_policy_json = context_policy
        .map(crate::context::ContextPolicy::history_metadata)
        .map(|policy| serde_json::to_string(&policy))
        .transpose()?;
    c.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) SELECT ?,datetime('now'),?,?,?,?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![
            dictation_id,
            final_text,
            cleanup_status,
            intent_json,
            model,
            context_policy_json,
            revision_reason,
            dictation_id
        ],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    c.execute(
        "UPDATE dictations SET cleanup_rejected_candidate=NULL, cleanup_guard_reason=NULL, cleanup_attempt_kind=NULL, cleanup_transformations_json=NULL, cleanup_final_guard_reason=NULL, cleanup_failure_reason=NULL WHERE id=?",
        [dictation_id],
    )?;
    Ok(c.last_insert_rowid())
}

/// Saves an explicit History edit while reading the prior adopted text and
/// captured scene under the same SQLite write transaction. A repeated save of
/// the current text is a no-op, so it cannot create a new learning event.
pub fn save_manual_history_revision(
    dir: &Path,
    dictation_id: i64,
    final_text: &str,
) -> anyhow::Result<ManualHistorySave> {
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let row = tx
        .query_row(
            "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=d.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(d.final_text,''),COALESCE(d.raw_text,'')), (SELECT revision_id FROM dictation_revisions WHERE dictation_id=d.id ORDER BY revision_id DESC LIMIT 1), (SELECT revision_reason FROM dictation_revisions WHERE dictation_id=d.id ORDER BY revision_id DESC LIMIT 1), COALESCE(NULLIF((SELECT final_text FROM dictation_revisions previous WHERE previous.dictation_id=d.id AND previous.revision_id < (SELECT revision_id FROM dictation_revisions latest WHERE latest.dictation_id=d.id ORDER BY latest.revision_id DESC LIMIT 1) ORDER BY previous.revision_id DESC LIMIT 1),''),NULLIF(d.final_text,''),COALESCE(d.raw_text,'')), d.context_policy_json, d.context_profile_id, d.context_family, d.context_browser_host, d.context_native_bundle, d.context_mapping_identity, d.transaction_id, CASE WHEN length(CAST(COALESCE(d.raw_text,'') AS BLOB)) <= 8192 THEN COALESCE(d.raw_text,'') ELSE '' END, CASE WHEN length(CAST(COALESCE(d.asr_text,'') AS BLOB)) <= 8192 THEN d.asr_text ELSE NULL END, d.status, d.delivery_method, d.fallback_reason FROM dictations d WHERE d.id=?1",
            [dictation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<String>>(15)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("history record was not found"))?;
    let (source_created_at_ms, natural_day, first_edit_of_original_delivery) = tx.query_row(
        "SELECT CAST(strftime('%s', d.created_at) AS INTEGER) * 1000,
                date(d.created_at, 'localtime'),
                NOT EXISTS (SELECT 1 FROM dictation_revisions prior
                            WHERE prior.dictation_id=d.id)
         FROM dictations d WHERE d.id=?1",
        [dictation_id],
        |row| {
            let created_at_ms = row
                .get::<_, Option<i64>>(0)?
                .and_then(|timestamp| u64::try_from(timestamp).ok());
            Ok((
                created_at_ms,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        },
    )?;
    let (
        previous_text,
        previous_revision_id,
        previous_reason,
        retry_before_text,
        policy_json,
        profile_id,
        family,
        browser_host,
        native_bundle,
        mapping_identity,
        origin_id,
        raw_text,
        asr_text,
        status,
        delivery_method,
        fallback_reason,
    ) = row;
    let scene = HistoryScene {
        policy: policy_json
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        profile_id,
        family,
        browser_host,
        native_bundle,
        mapping_identity,
    };
    if previous_text == final_text {
        tx.commit()?;
        let revision_id =
            previous_revision_id.filter(|_| previous_reason.as_deref() == Some("manual_edit"));
        return Ok(ManualHistorySave {
            revision_id,
            origin_id,
            first_edit_of_original_delivery: false,
            source_created_at_ms,
            natural_day,
            raw_text,
            asr_text,
            status,
            delivery_method,
            fallback_reason,
            learning_before_text: if revision_id.is_some() {
                retry_before_text
            } else {
                previous_text
            },
            scene,
            changed: false,
        });
    }
    tx.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) VALUES (?1,datetime('now'),?2,NULL,NULL,NULL,NULL,'manual_edit')",
        params![dictation_id, final_text],
    )?;
    let revision_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE dictations SET cleanup_rejected_candidate=NULL, cleanup_guard_reason=NULL, cleanup_attempt_kind=NULL, cleanup_transformations_json=NULL, cleanup_final_guard_reason=NULL, cleanup_failure_reason=NULL WHERE id=?1",
        [dictation_id],
    )?;
    tx.commit()?;
    Ok(ManualHistorySave {
        revision_id: Some(revision_id),
        origin_id,
        first_edit_of_original_delivery,
        source_created_at_ms,
        natural_day,
        raw_text,
        asr_text,
        status,
        delivery_method,
        fallback_reason,
        learning_before_text: previous_text,
        scene,
        changed: true,
    })
}

pub fn history_revision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryRevision> {
    let intent_json: Option<String> = row.get(5)?;
    let context_policy_json: Option<String> = row.get(7)?;
    Ok(HistoryRevision {
        revision_id: row.get(0)?,
        dictation_id: row.get(1)?,
        created_at: row.get(2)?,
        final_text: row.get(3)?,
        cleanup_status: row.get(4)?,
        intent: intent_json.and_then(|value| serde_json::from_str(&value).ok()),
        model: row.get(6)?,
        context_policy: context_policy_json.and_then(|value| serde_json::from_str(&value).ok()),
        revision_reason: row.get(8)?,
    })
}

pub const HISTORY_REVISION_COLUMNS: &str =
    "revision_id,dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason";

pub fn get_history_revisions(
    dir: &Path,
    dictation_id: i64,
) -> anyhow::Result<Vec<HistoryRevision>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {HISTORY_REVISION_COLUMNS} FROM dictation_revisions WHERE dictation_id=? ORDER BY revision_id ASC"
    ))?;
    let rows = statement.query_map([dictation_id], history_revision_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_history_revision(
    dir: &Path,
    dictation_id: i64,
    revision_id: i64,
) -> anyhow::Result<Option<HistoryRevision>> {
    let c = open_history(dir)?;
    let mut statement = c.prepare(&format!(
        "SELECT {HISTORY_REVISION_COLUMNS} FROM dictation_revisions WHERE dictation_id=? AND revision_id=?"
    ))?;
    statement
        .query_row(
            params![dictation_id, revision_id],
            history_revision_from_row,
        )
        .optional()
        .map_err(Into::into)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDelivery {
    Copied,
    ClipboardChanged,
    ClipboardOwnershipUnverified,
    ClipboardWriteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecleanDelivery {
    Copied,
    ClipboardChanged,
    ClipboardOwnershipUnverified,
    ClipboardWriteFailed,
}

impl RecleanDelivery {
    fn persistence(self) -> (&'static str, &'static str) {
        match self {
            Self::Copied => ("clipboard", "reclean_clipboard_only"),
            Self::ClipboardChanged => ("history", "reclean_clipboard_changed"),
            Self::ClipboardOwnershipUnverified => {
                ("history", "reclean_clipboard_ownership_unverified")
            }
            Self::ClipboardWriteFailed => ("history", "reclean_clipboard_write_failed"),
        }
    }
}

impl RetryDelivery {
    fn persistence(self) -> (&'static str, &'static str) {
        match self {
            Self::Copied => ("clipboard", "retry_clipboard_only"),
            Self::ClipboardChanged => ("history", "retry_clipboard_changed"),
            Self::ClipboardOwnershipUnverified => {
                ("history", "retry_clipboard_ownership_unverified")
            }
            Self::ClipboardWriteFailed => ("history", "retry_clipboard_write_failed"),
        }
    }
}

// Keep the persisted transcript, provider-original transcript, and delivery
// status explicit at this history boundary.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn mark_retried_with_texts(
    dir: &Path,
    id: i64,
    raw_text: Option<&str>,
    asr_text: Option<&str>,
    engine: Option<&str>,
    final_text: &str,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
    delivery: RetryDelivery,
) -> anyhow::Result<()> {
    mark_retried_with_texts_and_cleanup_rejection(
        dir,
        id,
        raw_text,
        asr_text,
        engine,
        final_text,
        degraded,
        degraded_reason,
        cleanup_status,
        delivery,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

pub fn save_reclean_revision_with_facts(
    dir: &Path,
    request: RecleanHistorySave<'_>,
) -> anyhow::Result<i64> {
    let RecleanHistorySave {
        dictation_id,
        final_text,
        cleanup_status,
        intent,
        model,
        context_policy,
        cleanup_rejected_candidate,
        cleanup_guard_reason,
        cleanup_attempt_kind,
        cleanup_transformations,
        cleanup_final_guard_reason,
        cleanup_failure_reason,
    } = request;
    if final_text.trim().is_empty() {
        anyhow::bail!("history revision cannot be empty");
    }
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let intent_json = serde_json::to_string(intent)?;
    let context_policy_json = serde_json::to_string(&context_policy.history_metadata())?;
    tx.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,intent_json,model,context_policy_json,revision_reason) SELECT ?,datetime('now'),?,?,?,?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![dictation_id, final_text, cleanup_status, intent_json, model, context_policy_json, "ai_reclean", dictation_id],
    )?;
    if tx.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    let revision_id = tx.last_insert_rowid();
    let transformations_json = cleanup_transformations
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "UPDATE dictations SET cleanup_rejected_candidate=?,cleanup_guard_reason=?,cleanup_attempt_kind=?,cleanup_transformations_json=?,cleanup_final_guard_reason=?,cleanup_failure_reason=? WHERE id=?",
        params![cleanup_rejected_candidate, cleanup_guard_reason, cleanup_attempt_kind, transformations_json, cleanup_final_guard_reason, cleanup_failure_reason, dictation_id],
    )?;
    tx.commit()?;
    Ok(revision_id)
}

#[allow(clippy::too_many_arguments)]
pub fn mark_retried_with_texts_and_cleanup_rejection(
    dir: &Path,
    id: i64,
    raw_text: Option<&str>,
    asr_text: Option<&str>,
    engine: Option<&str>,
    final_text: &str,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
    delivery: RetryDelivery,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
    cleanup_attempt_kind: Option<&str>,
    cleanup_transformations: Option<&[String]>,
    cleanup_final_guard_reason: Option<&str>,
    cleanup_failure_reason: Option<&str>,
) -> anyhow::Result<()> {
    let mut c = open_history(dir)?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (delivery_method, fallback_reason) = delivery.persistence();
    let status = if degraded {
        "degraded"
    } else if delivery == RetryDelivery::Copied {
        "copied"
    } else {
        "history"
    };
    tx.execute(
        "INSERT INTO dictation_revisions (dictation_id,created_at,final_text,cleanup_status,revision_reason) SELECT ?,datetime('now'),?,?,? WHERE EXISTS (SELECT 1 FROM dictations WHERE id=?)",
        params![id, final_text, cleanup_status, "retry", id],
    )?;
    if tx.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    let transformations_json = cleanup_transformations
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "UPDATE dictations SET status=?, degraded=?, degraded_reason=?, raw_text=COALESCE(?,raw_text), asr_text=COALESCE(?,asr_text), engine=COALESCE(?,engine), cleanup_status=COALESCE(?,cleanup_status), provider_cleaned_candidate=NULL, cleanup_rejected_candidate=?, cleanup_guard_reason=?, cleanup_attempt_kind=?, cleanup_transformations_json=?, cleanup_final_guard_reason=?, cleanup_failure_reason=?, raw_audio_path=NULL, delivery_method=?, fallback_reason=?, delivery_error_code=NULL, delivery_user_reason=NULL WHERE id=?",
        params![
            status,
            degraded as i32,
            degraded_reason,
            raw_text,
            asr_text,
            engine,
            cleanup_status,
            cleanup_rejected_candidate,
            cleanup_guard_reason,
            cleanup_attempt_kind,
            transformations_json,
            cleanup_final_guard_reason,
            cleanup_failure_reason,
            delivery_method,
            fallback_reason,
            id
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn record_recovered_history(dir: &Path, recovery: &RecoveredSpool) -> anyhow::Result<()> {
    let _mutation = begin_spool_root_mutation(dir)?;
    let spool_root = dir.join("spool");
    let gold_root = gold_root(dir);
    let (owner_path, expected_kind) = if is_safe_spool_path(&spool_root, &recovery.audio_path) {
        let owner_path = spool_session_key_for_artifact(&recovery.audio_path)
            .ok_or_else(|| anyhow::anyhow!("invalid recovered spool path"))?;
        let kind = spool_lease_kind_for_artifact(&recovery.audio_path, &owner_path);
        (owner_path, kind)
    } else if is_safe_spool_path(&gold_root, &recovery.audio_path) {
        let owner_path = spool_session_key_for_artifact(&recovery.audio_path)
            .ok_or_else(|| anyhow::anyhow!("invalid recovered artifact path"))?;
        (owner_path, SpoolLeaseKind::Artifact)
    } else {
        anyhow::bail!("recovered audio is outside the owned storage roots");
    };
    let lease = wait_for_history_delete_spool_lease(dir, &recovery.audio_path)?;
    if lease.inner.kind != expected_kind {
        anyhow::bail!("recovered audio owner kind changed during admission");
    }
    let _owner_commit = try_spool_io_commit_audio_session(&lease)?;
    match expected_kind {
        SpoolLeaseKind::Session => {
            let root_metadata = fs::symlink_metadata(&spool_root)?;
            if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
                anyhow::bail!("recovered spool root has an unsafe file type");
            }
            let metadata = fs::symlink_metadata(&owner_path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!("recovered session has an unsafe owner directory");
            }
            let session_id = owner_path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| anyhow::anyhow!("recovered session id is invalid"))?;
            if !valid_spool_session_id(session_id)
                || recovery.audio_path != owner_path.join("recovery.wav")
            {
                anyhow::bail!("recovered session path is invalid");
            }
            let manifest = load_manifest(&owner_path)?;
            if manifest.schema_version != SPOOL_MANIFEST_VERSION
                || manifest.session_id != session_id
                || !matches!(
                    manifest.status.as_str(),
                    "active" | "recoverable" | "degraded"
                )
            {
                anyhow::bail!("recovered session is no longer eligible for History");
            }
        }
        SpoolLeaseKind::Artifact => {
            let managed_root = if recovery.audio_path.starts_with(&spool_root) {
                &spool_root
            } else {
                &gold_root
            };
            let root_metadata = fs::symlink_metadata(managed_root)?;
            if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
                anyhow::bail!("recovered artifact root has an unsafe file type");
            }
            let metadata = fs::symlink_metadata(&recovery.audio_path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                anyhow::bail!("recovered artifact has an unsafe file type");
            }
        }
    }
    let artifact_metadata = fs::symlink_metadata(&recovery.audio_path)?;
    if !artifact_metadata.is_file() || artifact_metadata.file_type().is_symlink() {
        anyhow::bail!("recovered audio has an unsafe file type");
    }
    let _history_commit = try_history_database_commit_with_wait(dir)?;
    let mut c = open_history(dir)?;
    let transaction = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let already_recorded: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dictations WHERE raw_audio_path=?",
        [recovery.audio_path.to_string_lossy().as_ref()],
        |row| row.get(0),
    )?;
    if already_recorded > 0 {
        transaction.commit()?;
        return Ok(());
    }
    let context = crate::context::ContextSnapshot::general();
    insert_history_with_status_and_candidate_on_connection(
        &transaction,
        InsertHistory {
            dir,
            raw: "",
            asr_text: None,
            engine: None,
            final_text: "",
            cleanup_status: None,
            duration: recovery.duration_secs,
            degraded: true,
            degraded_reason: Some("interrupted_recording"),
            status: "failed".into(),
            delivery_method: Some("none"),
            fallback_reason: Some("crash_recovery"),
            spool: Some(&recovery.audio_path),
            count_asr: false,
            count_llm: false,
            context: Some(&context),
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

pub fn remove_spool_artifact(dir: &Path, path: &Path) {
    remove_spool_artifact_inner(dir, path, false);
}

pub fn remove_spool_artifact_inner(dir: &Path, path: &Path, root_commit_held: bool) {
    let spool_root = dir.join("spool");
    if is_safe_spool_path(&spool_root, path) {
        let Some(key) = spool_session_key_for_artifact(path) else {
            return;
        };
        let lease_kind = spool_lease_kind_for_artifact(path, &key);
        let lease = match reserve_spool_session_path(&key, false, lease_kind) {
            Ok(lease) => lease,
            Err(_) => {
                defer_spool_artifact_removal_until_release(path);
                return;
            }
        };
        let _commit = if root_commit_held {
            None
        } else {
            match try_spool_io_commit_audio_session(&lease) {
                Ok(commit) => Some(commit),
                Err(_) => return,
            }
        };
        if !spool_lease_can_write(&lease) {
            return;
        }
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
            return;
        }
        let _ = fs::remove_file(path);
        if let Some(parent) = path.parent() {
            if parent != spool_root && parent.join("manifest.json").is_file() {
                let _ = fs::remove_dir_all(parent);
            }
        }
        return;
    }
    if is_safe_spool_path(&gold_root(dir), path) && path.is_file() {
        let Some(key) = spool_session_key_for_artifact(path) else {
            return;
        };
        let lease = match reserve_spool_session_path(&key, false, SpoolLeaseKind::Artifact) {
            Ok(lease) => lease,
            Err(_) => return,
        };
        let _commit = if root_commit_held {
            None
        } else {
            match try_spool_io_commit_audio_session(&lease) {
                Ok(commit) => Some(commit),
                Err(_) => return,
            }
        };
        if spool_lease_can_write(&lease) {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
pub fn history_audio_bytes(dir: &Path, id: i64) -> anyhow::Result<Vec<u8>> {
    let c = open_history(dir)?;
    let path: Option<String> = c
        .query_row(
            "SELECT raw_audio_path FROM dictations WHERE id=?",
            [id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let path = path.ok_or_else(|| anyhow::anyhow!("history audio is no longer available"))?;
    let path = Path::new(&path);
    if !managed_audio_exists(dir, path) {
        anyhow::bail!("history audio is no longer available");
    }
    read_spool_file(path)
}

#[cfg(test)]
pub fn save_verbatim(dir: &Path, id: i64, text: &str, reviewed: bool) -> anyhow::Result<()> {
    let trimmed = text.trim();
    if reviewed && trimmed.is_empty() {
        anyhow::bail!("reviewed verbatim text cannot be empty");
    }
    let stored = if reviewed || !trimmed.is_empty() {
        Some(trimmed)
    } else {
        None
    };
    let c = open_history(dir)?;
    c.execute(
        "UPDATE dictations SET verbatim_text=?, verbatim_reviewed=? WHERE id=?",
        params![stored, reviewed as i32, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}

pub fn qwen_gold_language_tag(language: &str) -> &'static str {
    match language {
        "zh" => "Chinese",
        "en" => "English",
        _ => "None",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GoldExport {
    pub directory: String,
    pub jsonl_path: String,
    pub count: usize,
}

pub fn export_gold_corpus(
    dir: &Path,
    downloads: &Path,
    language: &str,
) -> anyhow::Result<GoldExport> {
    if downloads.as_os_str().is_empty() || !downloads.is_absolute() {
        anyhow::bail!("invalid export path");
    }
    let c = open_history(dir)?;
    let mut statement = c.prepare(
        "SELECT id, COALESCE(raw_text,''), raw_audio_path FROM dictations WHERE raw_audio_path IS NOT NULL ORDER BY id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    drop(c);

    let language_tag = qwen_gold_language_tag(language);
    let export_dir = downloads.join(format!("voiceflow-gold-{}", now_ms()));
    ensure_private_dir(&export_dir)?;
    let jsonl_path = export_dir.join("corpus.jsonl");
    let mut jsonl = String::new();
    let mut count = 0usize;
    let gold = gold_root(dir);
    for (id, raw_text, audio_path) in rows {
        let text = raw_text.trim();
        if text.is_empty() {
            continue;
        }
        let Some(audio_path) = audio_path else {
            continue;
        };
        let source = Path::new(&audio_path);
        if !is_safe_spool_path(&gold, source) || !source.is_file() {
            continue;
        }
        let wav = match read_spool_file(source) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("skipping gold export for history {id}: {error}");
                continue;
            }
        };
        count += 1;
        let file_name = format!("utt{count:04}.wav");
        let dest = export_dir.join(&file_name);
        write_atomic_bytes(&dest, &wav)?;
        let line = serde_json::json!({
            "audio": dest.to_string_lossy(),
            "text": format!("language {language_tag}<asr_text>{text}"),
        });
        jsonl.push_str(&line.to_string());
        jsonl.push('\n');
    }
    if count == 0 {
        let _ = fs::remove_dir_all(&export_dir);
        anyhow::bail!("没有可导出的训练音频。");
    }
    write_atomic_bytes(&jsonl_path, jsonl.as_bytes())?;
    Ok(GoldExport {
        directory: export_dir.to_string_lossy().into_owned(),
        jsonl_path: jsonl_path.to_string_lossy().into_owned(),
        count,
    })
}

pub fn purge_gold_audio(dir: &Path, keep_audio_days: u64) -> anyhow::Result<usize> {
    let _mutation = begin_spool_root_mutation(dir)?;
    // The operation reads the gold tree and updates History rows whose audio
    // references expired, so both typed regions are part of its ownership.
    let _history_commit = try_spool_io_commit_scope(dir, SpoolIoScope::HistoryDatabase)?;
    let _commit = try_spool_io_commit_scope(dir, SpoolIoScope::GoldAudio)?;
    let root = gold_root(dir);
    if !root.exists() {
        return Ok(0);
    }
    let max_age = keep_audio_days.saturating_mul(86_400);
    let mut deleted = 0usize;
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(max_age.saturating_add(1));
        if (keep_audio_days == 0 || age > max_age) && fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    let c = open_history(dir)?;
    let mut statement =
        c.prepare("SELECT id, raw_audio_path FROM dictations WHERE raw_audio_path IS NOT NULL")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (id, path) in rows.into_iter() {
        let Some(path) = path else {
            continue;
        };
        let path = Path::new(&path);
        if is_safe_spool_path(&root, path) && !path.is_file() {
            c.execute("UPDATE dictations SET raw_audio_path=NULL WHERE id=?", [id])?;
        }
    }
    Ok(deleted)
}
pub fn history_text(dir: &Path, id: i64) -> anyhow::Result<String> {
    let c = open_history(dir)?;
    Ok(c.query_row(
        "SELECT COALESCE(NULLIF((SELECT final_text FROM dictation_revisions WHERE dictation_id=dictations.id ORDER BY revision_id DESC LIMIT 1),''),NULLIF(final_text,''),raw_text,'') FROM dictations WHERE id=?",
        [id],
        |r| r.get(0),
    )?)
}

/// Return the provider's original ASR transcript for History re-cleanup.
/// Older rows intentionally remain unavailable when this nullable field is absent.
pub fn history_asr_text(dir: &Path, id: i64) -> anyhow::Result<Option<String>> {
    let c = open_history(dir)?;
    Ok(
        c.query_row("SELECT asr_text FROM dictations WHERE id=?", [id], |r| {
            r.get(0)
        })?,
    )
}

#[cfg(test)]
pub fn update_history_revision_state(
    dir: &Path,
    id: i64,
    degraded: bool,
    degraded_reason: Option<&str>,
    status: &str,
    cleanup_status: Option<&str>,
) -> anyhow::Result<()> {
    let c = open_history(dir)?;
    c.execute(
        "UPDATE dictations SET status=?,degraded=?,degraded_reason=?,cleanup_status=COALESCE(?,cleanup_status),cleanup_rejected_candidate=?,cleanup_guard_reason=?,delivery_method='clipboard',fallback_reason=NULL,delivery_error_code=NULL,delivery_user_reason=NULL WHERE id=?",
        params![status, degraded as i32, degraded_reason, cleanup_status, Option::<&str>::None, Option::<&str>::None, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn update_history_revision_state_with_reclean_delivery(
    dir: &Path,
    id: i64,
    degraded: bool,
    degraded_reason: Option<&str>,
    cleanup_status: Option<&str>,
    cleanup_rejected_candidate: Option<&str>,
    cleanup_guard_reason: Option<&str>,
    delivery: Option<RecleanDelivery>,
    cleanup_attempt_kind: Option<&str>,
    cleanup_transformations: Option<&[String]>,
    cleanup_final_guard_reason: Option<&str>,
    cleanup_failure_reason: Option<&str>,
) -> anyhow::Result<()> {
    let (status, delivery_method, fallback_reason) = if degraded {
        ("degraded", "history", None)
    } else if let Some(delivery) = delivery {
        let (method, reason) = delivery.persistence();
        (
            if delivery == RecleanDelivery::Copied {
                "copied"
            } else {
                "history"
            },
            method,
            Some(reason),
        )
    } else {
        ("history", "history", Some("reclean_copy_not_attempted"))
    };
    let c = open_history(dir)?;
    let transformations_json = cleanup_transformations
        .map(serde_json::to_string)
        .transpose()?;
    c.execute(
        "UPDATE dictations SET status=?,degraded=?,degraded_reason=?,cleanup_status=COALESCE(?,cleanup_status),cleanup_rejected_candidate=?,cleanup_guard_reason=?,cleanup_attempt_kind=?,cleanup_transformations_json=?,cleanup_final_guard_reason=?,cleanup_failure_reason=?,delivery_method=?,fallback_reason=?,delivery_error_code=NULL,delivery_user_reason=NULL WHERE id=?",
        params![status, degraded as i32, degraded_reason, cleanup_status, cleanup_rejected_candidate, cleanup_guard_reason, cleanup_attempt_kind, transformations_json, cleanup_final_guard_reason, cleanup_failure_reason, delivery_method, fallback_reason, id],
    )?;
    if c.changes() == 0 {
        anyhow::bail!("history record was not found");
    }
    Ok(())
}
pub fn delete_history(dir: &Path, id: i64) -> anyhow::Result<()> {
    // Keep the same admission order as recovery: claim the data-root mutation,
    // wait for the exact spool owner to release its path, then claim History.
    // Waiting for a spool lease while holding History could deadlock a worker
    // that still needs to finish its History commit before releasing the lease.
    let _mutation = begin_spool_root_mutation(dir)?;
    let c = open_history(dir)?;
    let (spool, transaction_id) = c
        .query_row(
            "SELECT raw_audio_path, transaction_id FROM dictations WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?
        .unwrap_or((None, None));
    let spool_path = spool.as_deref().map(Path::new);
    let _spool_lease = if let Some(path) = spool_path {
        Some(wait_for_history_delete_spool_lease(dir, path)?)
    } else {
        None
    };
    if let Some(transaction_id) = transaction_id.as_deref() {
        // A committed History row may still have its recovery intent if the
        // process stopped after the SQLite ACK. Remove that intent before
        // deleting the row so a later recovery pass cannot recreate the proof.
        remove_pending_history_intent(dir, transaction_id)?;
    }
    let _history_commit = try_history_database_commit_with_wait(dir)?;
    let mut connection = c;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(path) = spool_path {
        if is_safe_spool_path(&dir.join("spool"), path) {
            if let Some(lease) = _spool_lease.as_ref() {
                mark_spool_artifact_complete(dir, path, lease.inner.kind)?;
            }
        }
    }
    transaction.execute("DELETE FROM dictations WHERE id=?", [id])?;
    transaction.commit()?;
    if let Some(path) = spool_path {
        remove_spool_artifact_for_history_delete(dir, path)?;
    }
    Ok(())
}

pub fn wait_for_history_delete_spool_lease(
    dir: &Path,
    path: &Path,
) -> anyhow::Result<AudioSpoolSessionLease> {
    if is_safe_spool_path(&dir.join("spool"), path) {
        let key = spool_session_key_for_artifact(path)
            .ok_or_else(|| anyhow::anyhow!("invalid History spool artifact path"))?;
        let kind = spool_lease_kind_for_artifact(path, &key);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match reserve_spool_session_path(&key, false, kind) {
                Ok(lease) => return Ok(lease),
                Err(error) if Instant::now() < deadline => {
                    #[cfg(test)]
                    wait_for_history_delete_spool_test_gate(&key);
                    std::thread::sleep(Duration::from_millis(2));
                    drop(error);
                }
                Err(error) => return Err(error.context("History spool deletion timed out")),
            }
        }
    }
    if is_safe_spool_path(&gold_root(dir), path) {
        let key = spool_session_key_for_artifact(path)
            .ok_or_else(|| anyhow::anyhow!("invalid History gold artifact path"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match reserve_spool_session_path(&key, false, SpoolLeaseKind::Artifact) {
                Ok(lease) => return Ok(lease),
                Err(error) if Instant::now() < deadline => {
                    #[cfg(test)]
                    wait_for_history_delete_spool_test_gate(&key);
                    std::thread::sleep(Duration::from_millis(2));
                    drop(error);
                }
                Err(error) => return Err(error.context("History artifact deletion timed out")),
            }
        }
    }
    anyhow::bail!("History references a spool artifact outside the owned storage roots")
}

pub fn mark_spool_artifact_complete(
    dir: &Path,
    path: &Path,
    lease_kind: SpoolLeaseKind,
) -> anyhow::Result<()> {
    if is_safe_spool_path(&dir.join("spool"), path) {
        let Some(session_dir) = spool_session_key_for_artifact(path) else {
            anyhow::bail!("invalid History spool session path");
        };
        if lease_kind == SpoolLeaseKind::Session {
            match fs::symlink_metadata(&session_dir) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => anyhow::bail!("History spool session has an unsafe file type"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
        let artifact_metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => Some(metadata),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && lease_kind == SpoolLeaseKind::Artifact =>
            {
                return Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if artifact_metadata.as_ref().is_some_and(|metadata| {
            metadata.file_type().is_symlink()
                || (lease_kind == SpoolLeaseKind::Artifact && !metadata.is_file())
                || (lease_kind == SpoolLeaseKind::Session
                    && !metadata.is_file()
                    && !(path == session_dir && metadata.is_dir()))
        }) {
            anyhow::bail!("History spool artifact has an unsafe file type");
        }
        if lease_kind == SpoolLeaseKind::Artifact {
            return Ok(());
        }
        let manifest_metadata = fs::symlink_metadata(manifest_path(&session_dir))?;
        if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
            anyhow::bail!("History spool manifest has an unsafe file type");
        }
        let mut manifest = load_manifest(&session_dir)?;
        manifest.status = "complete".to_owned();
        save_manifest(&session_dir, &manifest)?;
    }
    Ok(())
}

pub fn remove_spool_artifact_for_history_delete(dir: &Path, path: &Path) -> anyhow::Result<()> {
    if is_safe_spool_path(&dir.join("spool"), path) || is_safe_spool_path(&gold_root(dir), path) {
        let owner_path = spool_session_key_for_artifact(path)
            .ok_or_else(|| anyhow::anyhow!("invalid History artifact owner path"))?;
        let manifest_path = owner_path.join("manifest.json");
        let remove_path = if fs::symlink_metadata(&manifest_path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            owner_path
        } else {
            path.to_path_buf()
        };
        match fs::symlink_metadata(&remove_path) {
            Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(&remove_path)?,
            Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(&remove_path)?,
            Ok(_) => anyhow::bail!("History spool artifact has an unsafe file type"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    } else {
        anyhow::bail!("History spool artifact path changed during deletion")
    }
    Ok(())
}
