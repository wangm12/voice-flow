//! Phase 1 screen-text context. Live Accessibility fills a fixture; titles,
//! PIDs, and raw URLs never enter the extract payload.
#![allow(dead_code)]

use crate::context::{ContextFamily, FocusKind, TargetAppGuard};
use std::time::Duration;

pub const MAX_TOKENS: usize = 40;
pub const MAX_CHARS: usize = 2000;
pub const MAX_SELECTED_CHARS: usize = 800;
pub const MAX_NEARBY_CHARS: usize = 400;
pub const MAX_NEARBY_ITEMS: usize = 2;
#[allow(dead_code)]
pub const EXTRACT_TIMEOUT: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenTextSource {
    #[default]
    Ax,
    AxOcr,
    CloudVision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextEvidenceSource {
    Ax,
    Ocr,
    CloudVision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextEvidenceKind {
    Scene,
    Term,
    SelectedText,
    NearbyText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextEvidenceItem {
    pub source: ContextEvidenceSource,
    pub kind: ContextEvidenceKind,
    pub value: String,
    pub confidence_milli: Option<u16>,
    pub truncated: bool,
}

/// Per-recording, source-tagged evidence. The target/session binding is local
/// memory only; callers must not serialize this value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextEvidence {
    pub items: Vec<ContextEvidenceItem>,
    pub target_guard: Option<TargetAppGuard>,
    pub session_generation: Option<u64>,
    /// Immutable content permissions captured when this recording began.
    /// Current settings are intersected with these before every request so a
    /// later settings change cannot broaden already captured evidence.
    pub capture_permissions: Option<crate::context::ContextSourcePermissions>,
    /// Monotonic in-memory source-policy generation. A revoke followed by a
    /// regrant cannot make evidence from the earlier policy usable again.
    pub policy_revision: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenTextContext {
    pub evidence: ContextEvidence,
    pub family: ContextFamily,
    pub source: ScreenTextSource,
    /// Set only after an allowed source was projected into an ASR request.
    pub provider_source: Option<ContextEvidenceSource>,
    pub truncated: bool,
}

impl ScreenTextContext {
    #[allow(dead_code)]
    pub fn proper_noun_count(&self) -> usize {
        self.evidence
            .items
            .iter()
            .filter(|item| item.kind == ContextEvidenceKind::Term)
            .count()
    }

    pub fn usable_chars(&self) -> usize {
        self.evidence
            .items
            .iter()
            .map(|item| item.value.chars().count())
            .sum::<usize>()
    }

    pub fn asr_terms(&self, permissions: crate::context::ContextSourcePermissions) -> Vec<String> {
        if !permissions.context_text_to_providers {
            return Vec::new();
        }
        self.granted_terms(permissions)
    }

    pub fn granted_terms(
        &self,
        permissions: crate::context::ContextSourcePermissions,
    ) -> Vec<String> {
        self.evidence
            .items
            .iter()
            .filter(|item| {
                item.kind == ContextEvidenceKind::Term
                    && source_is_granted(item.source, permissions)
            })
            .map(|item| item.value.clone())
            .take(MAX_TOKENS)
            .collect()
    }

    pub fn cleanup_projection(
        &self,
        permissions: crate::context::ContextSourcePermissions,
    ) -> Option<String> {
        if !permissions.context_text_to_providers {
            return None;
        }
        let mut parts = Vec::new();
        let mut used = 0usize;
        for item in &self.evidence.items {
            if !matches!(
                item.kind,
                ContextEvidenceKind::Term
                    | ContextEvidenceKind::SelectedText
                    | ContextEvidenceKind::NearbyText
            ) || !source_is_granted(item.source, permissions)
            {
                continue;
            }
            let remaining = MAX_CHARS.saturating_sub(used);
            if remaining == 0 {
                break;
            }
            let value: String = item.value.chars().take(remaining).collect();
            if value.trim().is_empty() {
                continue;
            }
            used = used.saturating_add(value.chars().count());
            parts.push(format!(
                "[source={}, kind={}] {}",
                evidence_source_label(item.source),
                evidence_kind_label(item.kind),
                value
            ));
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    pub fn projected_source(
        &self,
        permissions: crate::context::ContextSourcePermissions,
    ) -> Option<ContextEvidenceSource> {
        if !permissions.context_text_to_providers {
            return None;
        }
        [
            ContextEvidenceSource::CloudVision,
            ContextEvidenceSource::Ocr,
            ContextEvidenceSource::Ax,
        ]
        .into_iter()
        .find(|source| {
            self.evidence.items.iter().any(|item| {
                item.source == *source
                    && matches!(
                        item.kind,
                        ContextEvidenceKind::Term
                            | ContextEvidenceKind::SelectedText
                            | ContextEvidenceKind::NearbyText
                    )
                    && source_is_granted(*source, permissions)
            })
        })
    }

    pub fn projected_asr_source(
        &self,
        permissions: crate::context::ContextSourcePermissions,
        included_terms: &[String],
    ) -> Option<ContextEvidenceSource> {
        if !permissions.context_text_to_providers {
            return None;
        }
        [
            ContextEvidenceSource::CloudVision,
            ContextEvidenceSource::Ocr,
            ContextEvidenceSource::Ax,
        ]
        .into_iter()
        .find(|source| {
            self.evidence.items.iter().any(|item| {
                item.source == *source
                    && item.kind == ContextEvidenceKind::Term
                    && source_is_granted(*source, permissions)
                    && included_terms.contains(&item.value)
            })
        })
    }

    #[allow(dead_code)]
    pub fn is_thin(&self) -> bool {
        self.usable_chars() < 20
    }

    pub fn bind_to(&mut self, guard: &TargetAppGuard, session_generation: u64) {
        self.evidence.target_guard = Some(guard.clone());
        self.evidence.session_generation = Some(session_generation);
    }

    pub fn bind_to_with_permissions(
        &mut self,
        guard: &TargetAppGuard,
        session_generation: u64,
        permissions: crate::context::ContextSourcePermissions,
    ) {
        self.bind_to(guard, session_generation);
        self.evidence.capture_permissions = Some(permissions);
    }

    pub fn bind_to_with_policy_revision(
        &mut self,
        guard: &TargetAppGuard,
        session_generation: u64,
        permissions: crate::context::ContextSourcePermissions,
        policy_revision: u64,
    ) {
        self.bind_to_with_permissions(guard, session_generation, permissions);
        self.evidence.policy_revision = Some(policy_revision);
    }

    pub fn is_bound_to(&self, guard: &TargetAppGuard, session_generation: u64) -> bool {
        self.evidence.target_guard.as_ref() == Some(guard)
            && self.evidence.session_generation == Some(session_generation)
    }
}

/// Keep the AX call behind the same switch tested by production code, so the
/// disabled path can prove it never invokes the native reader.
pub fn capture_ax_if_allowed(
    context_enabled: bool,
    permissions: crate::context::ContextSourcePermissions,
    read: impl FnOnce() -> ScreenTextContext,
) -> ScreenTextContext {
    if context_enabled && permissions.ax_text {
        read()
    } else {
        ScreenTextContext::default()
    }
}

pub fn capture_ax_if_contextual(
    context_enabled: bool,
    permissions: crate::context::ContextSourcePermissions,
    family: ContextFamily,
    focus_kind: FocusKind,
    has_input_identity: bool,
    read: impl FnOnce() -> ScreenTextContext,
) -> ScreenTextContext {
    let protected_field = matches!(
        focus_kind,
        FocusKind::Unknown | FocusKind::Secure | FocusKind::Terminal | FocusKind::Form
    ) || (family == ContextFamily::PromptOrCode
        && focus_kind == FocusKind::Code);
    if !has_input_identity || protected_field {
        return ScreenTextContext {
            family,
            ..ScreenTextContext::default()
        };
    }
    let mut screen = capture_ax_if_allowed(context_enabled, permissions, read);
    screen.family = family;
    screen
}

fn source_is_granted(
    source: ContextEvidenceSource,
    permissions: crate::context::ContextSourcePermissions,
) -> bool {
    match source {
        ContextEvidenceSource::Ax => permissions.ax_text,
        ContextEvidenceSource::Ocr => permissions.local_ocr,
        ContextEvidenceSource::CloudVision => permissions.cloud_vision,
    }
}

pub fn evidence_source_label(source: ContextEvidenceSource) -> &'static str {
    match source {
        ContextEvidenceSource::Ax => "ax",
        ContextEvidenceSource::Ocr => "ocr",
        ContextEvidenceSource::CloudVision => "cloud_vision",
    }
}

pub fn terms_from_ocr_line(line: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let push_current = |terms: &mut Vec<String>, current: &mut String| {
        let value = current.trim_matches(['.', '-', '_']).trim();
        if value.chars().count() >= 2
            && !is_numeric_only(value)
            && !terms.iter().any(|term| term == value)
        {
            terms.push(value.chars().take(48).collect());
        }
        current.clear();
    };
    for character in line.chars() {
        if character.is_alphanumeric() || matches!(character, '.' | '-' | '_') {
            current.push(character);
        } else {
            push_current(&mut terms, &mut current);
            if terms.len() == 40 {
                break;
            }
        }
    }
    push_current(&mut terms, &mut current);
    terms.truncate(40);
    terms
}

fn evidence_kind_label(kind: ContextEvidenceKind) -> &'static str {
    match kind {
        ContextEvidenceKind::Scene => "scene",
        ContextEvidenceKind::Term => "term",
        ContextEvidenceKind::SelectedText => "selected_text",
        ContextEvidenceKind::NearbyText => "nearby_text",
    }
}

#[derive(Debug, Clone)]
pub struct AxWindowFixture {
    pub family: ContextFamily,
    pub focus_kind: FocusKind,
    pub known_ide: bool,
    pub counterpart: Option<String>,
    pub bubbles: Vec<String>,
    pub email_recipients: Vec<String>,
    pub email_subject: Option<String>,
    pub ide_filenames: Vec<String>,
    pub ide_symbols: Vec<String>,
    pub selected_text: Option<String>,
    pub document_name: Option<String>,
    pub focused_role: String,
    pub secure: bool,
    pub banking_preset: bool,
    /// Kept on the fixture so tests can prove these never enter the payload.
    #[allow(dead_code)]
    pub window_title: String,
    #[allow(dead_code)]
    pub raw_url: Option<String>,
    #[allow(dead_code)]
    pub pid: i32,
}

pub fn extract_from_fixture(fix: &AxWindowFixture) -> ScreenTextContext {
    let mut items = vec![ContextEvidenceItem {
        source: ContextEvidenceSource::Ax,
        kind: ContextEvidenceKind::Scene,
        value: crate::context::family_id(fix.family).to_owned(),
        confidence_milli: Some(950),
        truncated: false,
    }];
    if layer1_forbidden(fix)
        || matches!(fix.focus_kind, FocusKind::Unknown | FocusKind::Secure)
        || (fix.known_ide && matches!(fix.focus_kind, FocusKind::Code | FocusKind::Terminal))
    {
        return ScreenTextContext {
            evidence: ContextEvidence {
                items,
                ..ContextEvidence::default()
            },
            family: fix.family,
            source: ScreenTextSource::Ax,
            ..ScreenTextContext::default()
        };
    }

    if fix.focus_kind == FocusKind::Search {
        if let Some(selected) = &fix.selected_text {
            push_evidence(&mut items, ContextEvidenceKind::SelectedText, selected);
        }
        return apply_caps(items, fix.family);
    }

    match fix.family {
        ContextFamily::PersonalChat | ContextFamily::WorkChat | ContextFamily::SocialMedia => {
            if fix.focus_kind == FocusKind::Search {
                if let Some(selected) = &fix.selected_text {
                    push_evidence(&mut items, ContextEvidenceKind::SelectedText, selected);
                }
            } else if fix.focus_kind == FocusKind::Chat {
                if let Some(name) = fix.counterpart.as_deref() {
                    push_evidence(&mut items, ContextEvidenceKind::Term, name);
                }
                for bubble in last_visible_bubbles(&fix.bubbles) {
                    push_evidence(&mut items, ContextEvidenceKind::NearbyText, bubble);
                }
            }
        }
        ContextFamily::Email => {
            for recipient in &fix.email_recipients {
                push_evidence(&mut items, ContextEvidenceKind::Term, recipient);
            }
            if let Some(subject) = &fix.email_subject {
                push_evidence(&mut items, ContextEvidenceKind::NearbyText, subject);
            }
        }
        ContextFamily::PromptOrCode | ContextFamily::DeveloperCollaboration => {
            for name in &fix.ide_filenames {
                if is_allowed_filename(name) {
                    push_evidence(&mut items, ContextEvidenceKind::Term, name);
                }
            }
            for symbol in &fix.ide_symbols {
                push_evidence(&mut items, ContextEvidenceKind::Term, symbol);
            }
            if matches!(fix.focus_kind, FocusKind::Chat | FocusKind::CodingPrompt) {
                if let Some(selected) = &fix.selected_text {
                    push_evidence(&mut items, ContextEvidenceKind::SelectedText, selected);
                }
            }
        }
        ContextFamily::Document | ContextFamily::NotesJournaling => {
            if let Some(name) = &fix.document_name {
                push_evidence(&mut items, ContextEvidenceKind::Term, name);
            }
            if let Some(selected) = &fix.selected_text {
                push_evidence(&mut items, ContextEvidenceKind::SelectedText, selected);
            }
            for symbol in &fix.ide_symbols {
                push_evidence(&mut items, ContextEvidenceKind::Term, symbol);
            }
        }
        ContextFamily::BrowserSearch => {
            if let Some(selected) = &fix.selected_text {
                push_evidence(&mut items, ContextEvidenceKind::SelectedText, selected);
            }
        }
        ContextFamily::Terminal | ContextFamily::FormFilling => {}
        ContextFamily::ProjectManagement
        | ContextFamily::CalendarTask
        | ContextFamily::CustomerSupport
        | ContextFamily::General => {
            if let Some(selected) = &fix.selected_text {
                push_evidence(&mut items, ContextEvidenceKind::SelectedText, selected);
            }
        }
    }
    apply_caps(items, fix.family)
}

pub fn extract_with_reader<F>(reader: F) -> ScreenTextContext
where
    F: FnOnce() -> ScreenTextContext + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    let spawn = std::thread::Builder::new()
        .name("voiceflow-screen-text".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(reader));
            let _ = tx.send(result);
        });
    if spawn.is_err() {
        return ScreenTextContext::default();
    }
    match rx.recv_timeout(EXTRACT_TIMEOUT) {
        Ok(Ok(ctx)) => ctx,
        _ => ScreenTextContext::default(),
    }
}

pub fn screen_context_for_session(
    family: ContextFamily,
    result: Result<ScreenTextContext, impl std::fmt::Display>,
) -> ScreenTextContext {
    match result {
        Ok(ctx) => ctx,
        Err(_) => ScreenTextContext {
            family,
            source: ScreenTextSource::Ax,
            ..ScreenTextContext::default()
        },
    }
}

pub fn extract_live(
    family: ContextFamily,
    focus_kind: FocusKind,
    guard: &crate::context::TargetAppGuard,
) -> ScreenTextContext {
    let guard = guard.clone();
    let mut ctx = extract_with_reader(move || {
        let mut fixture = live_fixture_from_guard(family, &guard);
        fixture.focus_kind = focus_kind;
        extract_from_fixture(&fixture)
    });
    ctx.family = family;
    ctx
}

pub fn extract_live_for_session(
    family: ContextFamily,
    focus_kind: FocusKind,
    guard: &crate::context::TargetAppGuard,
    session_generation: u64,
) -> ScreenTextContext {
    let mut ctx = extract_live(family, focus_kind, guard);
    ctx.bind_to(guard, session_generation);
    ctx
}

pub fn resolve_screen_at_stop(
    expected: &crate::context::TargetAppGuard,
    live: &crate::context::TargetAppGuard,
    _family: ContextFamily,
    session_generation: u64,
    extract: impl FnOnce() -> ScreenTextContext,
) -> Option<ScreenTextContext> {
    if crate::context::focus_mismatch_reason(expected, live).is_some() {
        return None;
    }
    let mut ctx = extract();
    ctx.evidence.session_generation = Some(session_generation);
    ctx.bind_to(expected, session_generation);
    Some(ctx)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveAxSnapshot {
    pub focused_role: String,
    pub selected_text: Option<String>,
    pub counterpart: Option<String>,
    pub bubbles: Vec<String>,
    pub email_recipients: Vec<String>,
    pub email_subject: Option<String>,
    pub ide_filenames: Vec<String>,
    pub ide_symbols: Vec<String>,
    pub document_name: Option<String>,
}

pub fn fixture_from_live_ax(
    family: ContextFamily,
    guard: &crate::context::TargetAppGuard,
    focus_kind: FocusKind,
    snap: LiveAxSnapshot,
) -> AxWindowFixture {
    let mut fix = empty_live_fixture(family, guard);
    fix.focus_kind = focus_kind;
    if !snap.focused_role.is_empty() {
        fix.focused_role = snap.focused_role;
        if fix.focused_role == "AXSecureTextField" {
            fix.secure = true;
        }
    }
    fix.selected_text = snap.selected_text;
    fix.counterpart = snap.counterpart;
    fix.bubbles = snap.bubbles;
    fix.email_recipients = snap.email_recipients;
    fix.email_subject = snap.email_subject;
    fix.ide_filenames = snap.ide_filenames;
    fix.ide_symbols = snap.ide_symbols;
    fix.document_name = snap.document_name;
    fix
}

fn empty_live_fixture(
    family: ContextFamily,
    guard: &crate::context::TargetAppGuard,
) -> AxWindowFixture {
    AxWindowFixture {
        family,
        focus_kind: FocusKind::Unknown,
        known_ide: is_known_ide_bundle(guard.bundle_id.as_deref()),
        counterpart: None,
        bubbles: Vec::new(),
        email_recipients: Vec::new(),
        email_subject: None,
        ide_filenames: Vec::new(),
        ide_symbols: Vec::new(),
        selected_text: None,
        document_name: None,
        focused_role: if guard.secure_input {
            "AXSecureTextField".into()
        } else {
            "AXTextField".into()
        },
        secure: guard.secure_input,
        banking_preset: crate::lexicon::is_default_learn_off_target(
            guard.bundle_id.as_deref(),
            guard.browser_host.as_deref(),
        ),
        window_title: String::new(),
        raw_url: None,
        pid: guard.pid,
    }
}

fn is_known_ide_bundle(bundle_id: Option<&str>) -> bool {
    matches!(
        bundle_id,
        Some(
            "com.todesktop.230313mzl4w4u92"
                | "com.microsoft.VSCode"
                | "com.microsoft.VSCodeInsiders"
                | "com.apple.dt.Xcode"
                | "dev.zed.Zed"
        )
    )
}

fn live_fixture_from_guard(
    family: ContextFamily,
    guard: &crate::context::TargetAppGuard,
) -> AxWindowFixture {
    let fix = empty_live_fixture(family, guard);
    if layer1_forbidden(&fix) {
        return fix;
    }
    #[cfg(target_os = "macos")]
    if let Some(snap) = macos_live_ax::read_live_ax(guard) {
        return fixture_from_live_ax(family, guard, FocusKind::Unknown, snap);
    }
    fix
}

fn layer1_forbidden(fix: &AxWindowFixture) -> bool {
    fix.secure
        || fix.banking_preset
        || fix.focused_role == "AXSecureTextField"
        || fix.focus_kind == FocusKind::Form
        || matches!(
            fix.family,
            ContextFamily::Terminal | ContextFamily::FormFilling
        )
}

fn last_visible_bubbles(bubbles: &[String]) -> &[String] {
    let start = bubbles.len().saturating_sub(2);
    &bubbles[start..]
}

fn is_allowed_filename(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) || name.starts_with('.') {
        return false;
    }
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty() && !ext.is_empty()
}

fn is_placeholder_hint(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_lowercase();
    lower.starts_with("reply to claude")
}

fn looks_like_url(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.contains("://") || trimmed.starts_with("www.")
}

fn is_numeric_only(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch.is_ascii_digit())
}

fn usable_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || is_placeholder_hint(trimmed)
        || looks_like_url(trimmed)
        || is_numeric_only(trimmed)
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn push_evidence(items: &mut Vec<ContextEvidenceItem>, kind: ContextEvidenceKind, value: &str) {
    if let Some(snippet) = usable_text(value) {
        if kind == ContextEvidenceKind::Term
            && items
                .iter()
                .any(|item| item.kind == kind && item.value == snippet)
        {
            return;
        }
        items.push(ContextEvidenceItem {
            source: ContextEvidenceSource::Ax,
            kind,
            value: snippet,
            confidence_milli: None,
            truncated: false,
        });
    }
}

fn apply_caps(items: Vec<ContextEvidenceItem>, family: ContextFamily) -> ScreenTextContext {
    let mut truncated = false;
    let mut kept = Vec::new();
    let mut used = 0usize;
    let mut terms = 0usize;
    let mut selected = 0usize;
    let mut nearby = 0usize;
    let mut scene = 0usize;
    for mut item in items {
        let (item_limit, count, count_limit) = match item.kind {
            ContextEvidenceKind::Scene => (64, &mut scene, 1),
            ContextEvidenceKind::Term => (120, &mut terms, MAX_TOKENS),
            ContextEvidenceKind::SelectedText => (MAX_SELECTED_CHARS, &mut selected, 1),
            ContextEvidenceKind::NearbyText => (MAX_NEARBY_CHARS, &mut nearby, MAX_NEARBY_ITEMS),
        };
        if *count >= count_limit {
            truncated = true;
            continue;
        }
        let value_len = item.value.chars().count();
        if value_len > item_limit {
            item.value = item.value.chars().take(item_limit).collect();
            item.truncated = true;
            truncated = true;
        }
        let n = item.value.chars().count();
        if used.saturating_add(n) > MAX_CHARS {
            truncated = true;
            break;
        }
        used = used.saturating_add(n);
        *count += 1;
        kept.push(item);
    }

    ScreenTextContext {
        evidence: ContextEvidence {
            items: kept,
            truncated,
            ..ContextEvidence::default()
        },
        family,
        source: ScreenTextSource::Ax,
        provider_source: None,
        truncated,
    }
}

fn looks_like_email(value: &str) -> bool {
    let trimmed = value.trim();
    let Some((user, host)) = trimmed.split_once('@') else {
        return false;
    };
    !user.is_empty() && host.contains('.') && !host.contains(' ') && !trimmed.contains("://")
}

fn looks_like_identifier(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() < 3 || trimmed.contains(' ') || looks_like_url(trimmed) {
        return false;
    }
    let has_lower = trimmed.chars().any(|ch| ch.is_ascii_lowercase());
    let has_upper = trimmed.chars().any(|ch| ch.is_ascii_uppercase());
    (has_lower && has_upper) || trimmed.contains('_') || trimmed.contains('-')
}

fn document_basename(value: &str) -> Option<String> {
    let name = value.rsplit(['/', '\\']).next().unwrap_or(value).trim();
    if is_allowed_filename(name) {
        Some(name.to_owned())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod macos_live_ax {
    use super::{
        document_basename, is_allowed_filename, looks_like_email, looks_like_identifier,
        usable_text, LiveAxSnapshot,
    };
    use crate::context::{ContextFamily, TargetAppGuard};
    use core::ffi::c_void;
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFRelease, CFRetain, CFType, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};

    type AXUIElementRef = *const c_void;
    const AX_SUCCESS: i32 = 0;
    const MAX_NODES: usize = 80;
    const MAX_DEPTH: usize = 6;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_in_seconds: f32) -> i32;
    }

    struct AxElement(AXUIElementRef);

    impl Drop for AxElement {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0 as CFTypeRef) };
                self.0 = std::ptr::null();
            }
        }
    }

    pub(super) fn read_live_ax(guard: &TargetAppGuard) -> Option<LiveAxSnapshot> {
        if guard.pid <= 0 || !crate::permissions::accessibility_is_trusted() {
            return None;
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_live_ax_inner(guard)))
            .ok()
            .flatten()
    }

    fn read_live_ax_inner(guard: &TargetAppGuard) -> Option<LiveAxSnapshot> {
        let app = AxElement(unsafe { AXUIElementCreateApplication(guard.pid) });
        if app.0.is_null() {
            return None;
        }
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(app.0, 0.35);
        }
        let window = copy_element_attr(&app, "AXFocusedWindow")
            .or_else(|| copy_element_attr(&app, "AXMainWindow"))?;
        unsafe {
            let _ = AXUIElementSetMessagingTimeout(window.0, 0.35);
        }
        let focused = copy_element_attr(&app, "AXFocusedUIElement");
        if let Some(focused) = &focused {
            unsafe {
                let _ = AXUIElementSetMessagingTimeout(focused.0, 0.35);
            }
        }
        let mut snap = LiveAxSnapshot {
            focused_role: focused
                .as_ref()
                .and_then(|element| copy_string_attr(element, "AXRole"))
                .unwrap_or_default(),
            selected_text: focused
                .as_ref()
                .and_then(|element| copy_string_attr(element, "AXSelectedText"))
                .and_then(|value| usable_text(&value)),
            document_name: copy_string_attr(&window, "AXDocument")
                .as_deref()
                .and_then(document_basename),
            ..LiveAxSnapshot::default()
        };
        if let Some(focused) = &focused {
            if let Some(value) = copy_string_attr(focused, "AXValue") {
                collect_value_tokens(&mut snap, ContextFamily::General, &value);
            }
        }
        let mut budget = MAX_NODES;
        walk(&window, 0, &mut budget, &mut snap);
        Some(snap)
    }

    fn walk(element: &AxElement, depth: usize, budget: &mut usize, snap: &mut LiveAxSnapshot) {
        if depth > MAX_DEPTH || *budget == 0 {
            return;
        }
        *budget = budget.saturating_sub(1);
        let role = copy_string_attr(element, "AXRole").unwrap_or_default();
        let title = copy_string_attr(element, "AXTitle");
        let value = copy_string_attr(element, "AXValue");
        if let Some(title) = title.as_deref() {
            if is_allowed_filename(title) {
                push_unique(&mut snap.ide_filenames, title);
            }
            if looks_like_email(title) {
                push_unique(&mut snap.email_recipients, title);
            }
        }
        if let Some(value) = value.as_deref() {
            collect_value_tokens(snap, ContextFamily::General, value);
            if role == "AXStaticText" || role == "AXTextField" {
                if let Some(text) = usable_text(value) {
                    if text.chars().count() <= 80
                        && snap.counterpart.as_deref() != Some(text.as_str())
                    {
                        push_unique(&mut snap.bubbles, &text);
                    }
                }
            }
        }
        for child in copy_children(element) {
            walk(&child, depth + 1, budget, snap);
        }
    }

    fn collect_value_tokens(snap: &mut LiveAxSnapshot, _family: ContextFamily, value: &str) {
        if looks_like_email(value) {
            push_unique(&mut snap.email_recipients, value.trim());
        }
        if looks_like_identifier(value) {
            push_unique(&mut snap.ide_symbols, value.trim());
        }
        if let Some(name) = document_basename(value) {
            push_unique(&mut snap.ide_filenames, &name);
        }
    }

    fn push_unique(items: &mut Vec<String>, value: &str) {
        let trimmed = value.trim();
        if trimmed.is_empty() || items.iter().any(|existing| existing == trimmed) {
            return;
        }
        items.push(trimmed.to_owned());
    }

    fn copy_element_attr(element: &AxElement, name: &str) -> Option<AxElement> {
        let value = copy_raw_attr(element, name)?;
        Some(AxElement(value as AXUIElementRef))
    }

    fn copy_raw_attr(element: &AxElement, name: &str) -> Option<CFTypeRef> {
        let attr = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe {
            AXUIElementCopyAttributeValue(element.0, attr.as_concrete_TypeRef(), &mut value)
        };
        if err != AX_SUCCESS || value.is_null() {
            None
        } else {
            Some(value)
        }
    }

    fn copy_string_attr(element: &AxElement, name: &str) -> Option<String> {
        let value = copy_raw_attr(element, name)?;
        let cf_type = unsafe { CFType::wrap_under_create_rule(value) };
        if cf_type.type_of() != CFString::type_id() {
            return None;
        }
        Some(unsafe { CFString::wrap_under_get_rule(cf_type.as_CFTypeRef() as _) }.to_string())
    }

    fn copy_children(element: &AxElement) -> Vec<AxElement> {
        let Some(value) = copy_raw_attr(element, "AXChildren") else {
            return Vec::new();
        };
        let array =
            unsafe { CFArray::<*const c_void>::wrap_under_create_rule(value as CFArrayRef) };
        let mut children = Vec::new();
        for index in 0..array.len() {
            let Some(item) = array.get(index) else {
                continue;
            };
            if item.is_null() {
                continue;
            }
            unsafe { CFRetain(*item) };
            children.push(AxElement(*item));
        }
        children
    }
}

#[cfg(test)]
fn empty_fix() -> AxWindowFixture {
    AxWindowFixture {
        family: ContextFamily::General,
        focus_kind: FocusKind::Unknown,
        known_ide: false,
        counterpart: None,
        bubbles: Vec::new(),
        email_recipients: Vec::new(),
        email_subject: None,
        ide_filenames: Vec::new(),
        ide_symbols: Vec::new(),
        selected_text: None,
        document_name: None,
        focused_role: "AXTextField".into(),
        secure: false,
        banking_preset: false,
        window_title: String::new(),
        raw_url: None,
        pid: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_permissions() -> crate::context::ContextSourcePermissions {
        crate::context::ContextSourcePermissions {
            ax_text: true,
            local_ocr: true,
            cloud_vision: true,
            context_text_to_providers: true,
        }
    }

    fn item(
        source: ContextEvidenceSource,
        kind: ContextEvidenceKind,
        value: &str,
    ) -> ContextEvidenceItem {
        ContextEvidenceItem {
            source,
            kind,
            value: value.into(),
            confidence_milli: Some(900),
            truncated: false,
        }
    }

    #[test]
    fn disabled_context_and_text_grants_never_call_or_project_ax_reader_content() {
        use std::cell::Cell;

        let calls = Cell::new(0);
        let result = capture_ax_if_allowed(false, all_permissions(), || {
            calls.set(calls.get() + 1);
            ScreenTextContext::default()
        });
        assert_eq!(calls.get(), 0);
        assert!(content_is_empty(&result));

        let result = capture_ax_if_allowed(true, Default::default(), || {
            calls.set(calls.get() + 1);
            ScreenTextContext::default()
        });
        assert_eq!(calls.get(), 0);
        assert!(content_is_empty(&result));
    }

    #[test]
    fn missing_or_conservative_field_identity_never_calls_ax_reader() {
        use std::cell::Cell;

        let calls = Cell::new(0);
        for (family, focus, has_input) in [
            (ContextFamily::PersonalChat, FocusKind::Unknown, true),
            (ContextFamily::Terminal, FocusKind::Terminal, true),
            (ContextFamily::PromptOrCode, FocusKind::Code, true),
            (ContextFamily::Email, FocusKind::Email, false),
        ] {
            let result =
                capture_ax_if_contextual(true, all_permissions(), family, focus, has_input, || {
                    calls.set(calls.get() + 1);
                    ScreenTextContext::default()
                });
            assert!(content_is_empty(&result));
        }
        assert_eq!(calls.get(), 0);

        let result = capture_ax_if_contextual(
            true,
            all_permissions(),
            ContextFamily::PromptOrCode,
            FocusKind::CodingPrompt,
            true,
            || {
                calls.set(calls.get() + 1);
                ScreenTextContext::default()
            },
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(result.family, ContextFamily::PromptOrCode);
    }

    #[test]
    fn provider_text_permission_gates_ax_and_ocr_derived_context() {
        let ctx = ScreenTextContext {
            evidence: ContextEvidence {
                items: vec![
                    item(
                        ContextEvidenceSource::Ax,
                        ContextEvidenceKind::Term,
                        "AXName",
                    ),
                    item(
                        ContextEvidenceSource::Ocr,
                        ContextEvidenceKind::Term,
                        "OCRName",
                    ),
                    item(
                        ContextEvidenceSource::CloudVision,
                        ContextEvidenceKind::Term,
                        "VisionName",
                    ),
                    item(
                        ContextEvidenceSource::Ocr,
                        ContextEvidenceKind::SelectedText,
                        "private OCR excerpt",
                    ),
                ],
                ..ContextEvidence::default()
            },
            family: ContextFamily::Document,
            ..ScreenTextContext::default()
        };
        let mut permissions = all_permissions();
        permissions.context_text_to_providers = false;
        assert!(ctx.asr_terms(permissions).is_empty());
        assert!(ctx.cleanup_projection(permissions).is_none());
        assert!(ctx.granted_terms(permissions).contains(&"OCRName".into()));

        permissions.context_text_to_providers = true;
        permissions.cloud_vision = false;
        permissions.local_ocr = false;
        assert_eq!(ctx.asr_terms(permissions), vec!["AXName"]);
        assert!(!ctx
            .cleanup_projection(permissions)
            .unwrap_or_default()
            .contains("private OCR excerpt"));
        assert_eq!(
            ctx.projected_source(permissions),
            Some(ContextEvidenceSource::Ax)
        );
    }

    #[test]
    fn evidence_binding_and_snapshot_serialization_are_local_only() {
        let guard = crate::context::TargetAppGuard {
            pid: 42,
            bundle_id: Some("com.example.Editor".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(8),
            window_id: Some(8),
            input_token: Some(9),
            secure_input: false,
        };
        let mut screen = ScreenTextContext::default();
        screen.bind_to(&guard, 17);
        assert!(screen.is_bound_to(&guard, 17));
        assert!(!screen.is_bound_to(&guard, 18));

        let mut snapshot = crate::context::ContextSnapshot::general();
        snapshot.target_guard = guard;
        snapshot.evidence = ContextEvidence {
            items: vec![item(
                ContextEvidenceSource::Ax,
                ContextEvidenceKind::NearbyText,
                "private snippet https://user:pass@example.com/path?secret=yes",
            )],
            target_guard: screen.evidence.target_guard.clone(),
            session_generation: Some(17),
            truncated: false,
            capture_permissions: None,
            policy_revision: None,
        };
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("private snippet"));
        assert!(!serialized.contains("example.com"));
        assert!(!serialized.contains("user:pass"));
        assert!(!serialized.contains("session_generation"));
    }

    fn nearby_count(ctx: &ScreenTextContext) -> usize {
        ctx.evidence
            .items
            .iter()
            .filter(|item| item.kind == ContextEvidenceKind::NearbyText)
            .count()
    }

    fn content_is_empty(ctx: &ScreenTextContext) -> bool {
        ctx.evidence
            .items
            .iter()
            .all(|item| item.kind == ContextEvidenceKind::Scene || item.value.is_empty())
    }

    #[test]
    fn chat_reads_counterpart_and_two_bubbles() {
        let ctx = extract_from_fixture(&AxWindowFixture {
            family: ContextFamily::PersonalChat,
            focus_kind: FocusKind::Chat,
            counterpart: Some("晓雯".into()),
            bubbles: vec!["在吗".into(), "晚点回你".into()],
            window_title: "晓雯 - 微信".into(),
            raw_url: Some("https://wx.qq.com/chat/secret".into()),
            pid: 4242,
            ..empty_fix()
        });
        assert!(ctx.asr_terms(all_permissions()).iter().any(|t| t == "晓雯"));
        assert_eq!(nearby_count(&ctx), 2);
        assert_eq!(
            ctx.proper_noun_count(),
            ctx.asr_terms(all_permissions()).len()
        );
        let text = ctx
            .cleanup_projection(all_permissions())
            .unwrap_or_default();
        assert!(!text.contains("微信"));
        assert!(!text.contains("4242"));
        assert!(!text.contains("https://"));
        assert_eq!(EXTRACT_TIMEOUT, Duration::from_millis(350));
        let _ = ScreenTextSource::AxOcr;
    }

    #[test]
    fn email_excludes_body() {
        let mut fix = empty_fix();
        fix.family = ContextFamily::Email;
        fix.focus_kind = FocusKind::Email;
        fix.email_recipients = vec!["alex@example.com".into()];
        fix.email_subject = Some("Q3 plan".into());
        fix.bubbles = vec!["THIS IS THE BODY AND MUST NOT APPEAR".into()];
        let ctx = extract_from_fixture(&fix);
        assert!(ctx
            .asr_terms(all_permissions())
            .iter()
            .any(|t| t.contains("alex@example.com")));
        assert!(ctx
            .cleanup_projection(all_permissions())
            .is_some_and(|text| text.contains("Q3 plan")));
        assert!(!ctx
            .cleanup_projection(all_permissions())
            .unwrap_or_default()
            .contains("THIS IS THE BODY"));
    }

    #[test]
    fn window_title_is_never_projected_as_an_email_subject() {
        let mut fix = empty_fix();
        fix.family = ContextFamily::Email;
        fix.focus_kind = FocusKind::Email;
        fix.window_title = "Quarterly layoffs plan — private mailbox".into();
        let context = extract_from_fixture(&fix);
        assert!(!context
            .cleanup_projection(all_permissions())
            .unwrap_or_default()
            .contains("Quarterly layoffs"));
    }

    #[test]
    fn ide_filenames_need_extension_and_no_spaces() {
        let mut fix = empty_fix();
        fix.family = ContextFamily::PromptOrCode;
        fix.focus_kind = FocusKind::Code;
        fix.ide_filenames = vec!["foo.ts".into(), "bad name.rs".into(), ".eslintrc".into()];
        fix.ide_symbols = vec!["handleUserAuthCallback".into()];
        let ctx = extract_from_fixture(&fix);
        let terms = ctx.asr_terms(all_permissions());
        assert!(terms.contains(&"foo.ts".into()));
        assert!(terms.contains(&"handleUserAuthCallback".into()));
        assert!(!terms.iter().any(|t| t.contains(' ')));
        assert!(!terms.iter().any(|t| t == ".eslintrc"));
    }

    #[test]
    fn terminal_secure_banking_are_empty() {
        let cases = [
            {
                let mut fix = empty_fix();
                fix.family = ContextFamily::Terminal;
                fix.selected_text = Some("secret-buffer".into());
                fix.ide_symbols = vec!["ls".into()];
                fix
            },
            {
                let mut fix = empty_fix();
                fix.family = ContextFamily::PersonalChat;
                fix.secure = true;
                fix.counterpart = Some("晓雯".into());
                fix.bubbles = vec!["在吗".into()];
                fix
            },
            {
                let mut fix = empty_fix();
                fix.family = ContextFamily::FormFilling;
                fix.selected_text = Some("4111".into());
                fix
            },
            {
                let mut fix = empty_fix();
                fix.family = ContextFamily::Email;
                fix.banking_preset = true;
                fix.email_recipients = vec!["billing@bank.example".into()];
                fix.email_subject = Some("Statement".into());
                fix
            },
            {
                let mut fix = empty_fix();
                fix.family = ContextFamily::Document;
                fix.focused_role = "AXSecureTextField".into();
                fix.document_name = Some("passwords".into());
                fix.selected_text = Some("hunter2".into());
                fix
            },
        ];
        for fix in cases {
            let ctx = extract_from_fixture(&fix);
            assert!(
                content_is_empty(&ctx),
                "expected empty Layer 1 for {:?} secure={} banking={} role={}",
                fix.family,
                fix.secure,
                fix.banking_preset,
                fix.focused_role
            );
            assert!(ctx.is_thin());
        }
    }

    #[test]
    fn caps_forty_tokens_and_two_thousand_chars() {
        let mut fix = empty_fix();
        fix.family = ContextFamily::Document;
        fix.focus_kind = FocusKind::Document;
        fix.ide_symbols = (0..80).map(|i| format!("Token{i}")).collect();
        let ctx = extract_from_fixture(&fix);
        assert!(ctx.proper_noun_count() <= 40);
        assert!(ctx.usable_chars() <= 2000);
        assert!(ctx.truncated);
        assert!(!ctx.is_thin());
    }

    #[test]
    fn stale_lock_drops_screen_text() {
        let expected = crate::context::TargetAppGuard {
            pid: 1,
            bundle_id: Some("com.example.a".into()),
            browser_host: None,
            browser_target_token: None,
            window_token: Some(1),
            window_id: Some(1),
            input_token: Some(1),
            secure_input: false,
        };
        let mut live = expected.clone();
        live.pid = 2;
        live.bundle_id = Some("com.example.b".into());
        let dropped =
            resolve_screen_at_stop(&expected, &live, ContextFamily::PersonalChat, 1, || {
                ScreenTextContext {
                    evidence: ContextEvidence {
                        items: vec![ContextEvidenceItem {
                            source: ContextEvidenceSource::Ax,
                            kind: ContextEvidenceKind::Term,
                            value: "晓雯".into(),
                            confidence_milli: None,
                            truncated: false,
                        }],
                        ..ContextEvidence::default()
                    },
                    family: ContextFamily::PersonalChat,
                    ..ScreenTextContext::default()
                }
            });
        assert!(dropped.is_none());
    }

    #[test]
    fn extract_live_timeout_returns_empty_not_error() {
        let ctx = extract_with_reader(|| {
            std::thread::sleep(Duration::from_millis(400));
            panic!("should have been timed out");
        });
        assert!(content_is_empty(&ctx));
    }

    #[test]
    fn extractor_error_does_not_fail_session() {
        let ctx = screen_context_for_session(ContextFamily::PersonalChat, Err("ax denied"));
        assert!(content_is_empty(&ctx));
        assert_eq!(ctx.family, ContextFamily::PersonalChat);
    }

    fn test_guard(bundle: &str, host: Option<&str>) -> crate::context::TargetAppGuard {
        crate::context::TargetAppGuard {
            pid: 42,
            bundle_id: Some(bundle.into()),
            browser_host: host.map(str::to_owned),
            browser_target_token: None,
            window_token: Some(1),
            window_id: Some(1),
            input_token: Some(1),
            secure_input: false,
        }
    }

    #[test]
    fn live_fixture_marks_onepassword_as_banking() {
        let fix = live_fixture_from_guard(
            ContextFamily::FormFilling,
            &test_guard("com.agilebits.onepassword7", None),
        );
        assert!(fix.banking_preset);
        let ctx = extract_from_fixture(&fix);
        assert!(content_is_empty(&ctx));
    }

    #[test]
    fn live_fixture_marks_workday_host_as_banking() {
        let fix = live_fixture_from_guard(
            ContextFamily::BrowserSearch,
            &test_guard("com.google.Chrome", Some("company.myworkday.com")),
        );
        assert!(fix.banking_preset);
    }

    #[test]
    fn live_ax_snapshot_fills_chat_without_title_url_or_pid() {
        let snap = LiveAxSnapshot {
            counterpart: Some("晓雯".into()),
            bubbles: vec!["在吗".into(), "晚点回你".into()],
            ..LiveAxSnapshot::default()
        };
        let mut guard = test_guard("com.tencent.xinWeChat", None);
        guard.pid = 4242;
        let fix = fixture_from_live_ax(ContextFamily::PersonalChat, &guard, FocusKind::Chat, snap);
        let ctx = extract_from_fixture(&AxWindowFixture {
            window_title: "晓雯 - 微信".into(),
            raw_url: Some("https://wx.qq.com/chat/secret".into()),
            pid: 4242,
            ..fix
        });
        assert!(ctx
            .asr_terms(all_permissions())
            .iter()
            .any(|token| token == "晓雯"));
        assert_eq!(nearby_count(&ctx), 2);
        let text = ctx
            .cleanup_projection(all_permissions())
            .unwrap_or_default();
        assert!(!text.contains("微信"));
        assert!(!text.contains("4242"));
        assert!(!text.contains("https://"));
        assert!(!text.contains("window_title"));
    }

    #[test]
    fn chat_search_and_unknown_fields_do_not_project_conversation_bubbles() {
        let search = extract_from_fixture(&AxWindowFixture {
            family: ContextFamily::WorkChat,
            focus_kind: FocusKind::Search,
            selected_text: Some("release notes".into()),
            counterpart: Some("Private coworker".into()),
            bubbles: vec!["Private conversation text".into()],
            ..empty_fix()
        });
        assert_eq!(nearby_count(&search), 0);
        assert!(search
            .cleanup_projection(all_permissions())
            .is_some_and(|text| text.contains("release notes")));
        assert!(!search
            .cleanup_projection(all_permissions())
            .unwrap_or_default()
            .contains("Private"));

        let unknown = extract_from_fixture(&AxWindowFixture {
            family: ContextFamily::PersonalChat,
            focus_kind: FocusKind::Unknown,
            counterpart: Some("Private coworker".into()),
            bubbles: vec!["Private conversation text".into()],
            ..empty_fix()
        });
        assert!(content_is_empty(&unknown));
    }
}
