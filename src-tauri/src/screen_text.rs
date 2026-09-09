//! Phase 1 screen-text context. Live Accessibility fills a fixture; titles,
//! PIDs, and raw URLs never enter the extract payload.
#![allow(dead_code)]

use crate::context::ContextFamily;
use std::time::Duration;

pub const MAX_TOKENS: usize = 40;
pub const MAX_CHARS: usize = 2000;
#[allow(dead_code)]
pub const EXTRACT_TIMEOUT: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenTextSource {
    #[default]
    Ax,
    #[allow(dead_code)]
    AxOcr,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenTextContext {
    pub tokens: Vec<String>,
    pub snippets: Vec<String>,
    pub family: ContextFamily,
    pub source: ScreenTextSource,
    pub truncated: bool,
}

impl ScreenTextContext {
    #[allow(dead_code)]
    pub fn proper_noun_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn usable_chars(&self) -> usize {
        self.tokens
            .iter()
            .map(|token| token.chars().count())
            .sum::<usize>()
            + self
                .snippets
                .iter()
                .map(|snippet| snippet.chars().count())
                .sum::<usize>()
    }

    pub fn visible_context_text(&self) -> String {
        let mut parts = Vec::with_capacity(self.tokens.len() + self.snippets.len());
        parts.extend(self.tokens.iter().cloned());
        parts.extend(self.snippets.iter().cloned());
        let mut text = parts.join(" ");
        if text.chars().count() > MAX_CHARS {
            text = text.chars().take(MAX_CHARS).collect();
        }
        text
    }

    #[allow(dead_code)]
    pub fn is_thin(&self) -> bool {
        self.usable_chars() < 20
    }
}

#[derive(Debug, Clone)]
pub struct AxWindowFixture {
    pub family: ContextFamily,
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
    if layer1_forbidden(fix) {
        return ScreenTextContext {
            family: fix.family,
            source: ScreenTextSource::Ax,
            ..ScreenTextContext::default()
        };
    }

    let mut tokens = Vec::new();
    let mut snippets = Vec::new();

    match fix.family {
        ContextFamily::PersonalChat | ContextFamily::WorkChat | ContextFamily::SocialMedia => {
            if let Some(name) = fix.counterpart.as_deref() {
                push_token(&mut tokens, name);
            }
            for bubble in last_visible_bubbles(&fix.bubbles) {
                push_snippet(&mut snippets, bubble);
            }
        }
        ContextFamily::Email => {
            for recipient in &fix.email_recipients {
                push_token(&mut tokens, recipient);
            }
            if let Some(subject) = &fix.email_subject {
                push_snippet(&mut snippets, subject);
            }
        }
        ContextFamily::PromptOrCode | ContextFamily::DeveloperCollaboration => {
            for name in &fix.ide_filenames {
                if is_allowed_filename(name) {
                    push_token(&mut tokens, name);
                }
            }
            for symbol in &fix.ide_symbols {
                push_token(&mut tokens, symbol);
            }
            if let Some(selected) = &fix.selected_text {
                push_snippet(&mut snippets, selected);
            }
        }
        ContextFamily::Document | ContextFamily::NotesJournaling => {
            if let Some(name) = &fix.document_name {
                push_token(&mut tokens, name);
            }
            if let Some(selected) = &fix.selected_text {
                push_snippet(&mut snippets, selected);
            }
            // Live extract fills document_name / selection. Tests may place
            // nearby AX tokens in ide_symbols; keep them as tokens only.
            for symbol in &fix.ide_symbols {
                push_token(&mut tokens, symbol);
            }
        }
        ContextFamily::BrowserSearch => {
            if let Some(selected) = &fix.selected_text {
                push_snippet(&mut snippets, selected);
            }
        }
        ContextFamily::Terminal | ContextFamily::FormFilling => {}
        ContextFamily::ProjectManagement
        | ContextFamily::CalendarTask
        | ContextFamily::CustomerSupport
        | ContextFamily::General => {
            if let Some(selected) = &fix.selected_text {
                push_snippet(&mut snippets, selected);
            }
        }
    }

    apply_caps(tokens, snippets, fix.family)
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
    guard: &crate::context::TargetAppGuard,
) -> ScreenTextContext {
    let guard = guard.clone();
    let mut ctx =
        extract_with_reader(move || extract_from_fixture(&live_fixture_from_guard(family, &guard)));
    ctx.family = family;
    ctx
}

pub fn resolve_screen_at_stop(
    expected: &crate::context::TargetAppGuard,
    live: &crate::context::TargetAppGuard,
    _family: ContextFamily,
    extract: impl FnOnce() -> ScreenTextContext,
) -> Option<ScreenTextContext> {
    if crate::context::focus_mismatch_reason(expected, live).is_some() {
        return None;
    }
    Some(extract())
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
    snap: LiveAxSnapshot,
) -> AxWindowFixture {
    let mut fix = empty_live_fixture(family, guard);
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
        return fixture_from_live_ax(family, guard, snap);
    }
    fix
}

fn layer1_forbidden(fix: &AxWindowFixture) -> bool {
    fix.secure
        || fix.banking_preset
        || fix.focused_role == "AXSecureTextField"
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

fn push_token(tokens: &mut Vec<String>, value: &str) {
    if let Some(token) = usable_text(value) {
        if !tokens.iter().any(|existing| existing == &token) {
            tokens.push(token);
        }
    }
}

fn push_snippet(snippets: &mut Vec<String>, value: &str) {
    if let Some(snippet) = usable_text(value) {
        snippets.push(snippet);
    }
}

fn apply_caps(
    tokens: Vec<String>,
    snippets: Vec<String>,
    family: ContextFamily,
) -> ScreenTextContext {
    let mut truncated = tokens.len() > MAX_TOKENS;
    let mut kept_tokens = Vec::new();
    let mut used = 0usize;
    for token in tokens.into_iter().take(MAX_TOKENS) {
        let n = token.chars().count();
        if used.saturating_add(n) > MAX_CHARS {
            truncated = true;
            break;
        }
        used = used.saturating_add(n);
        kept_tokens.push(token);
    }

    let mut kept_snippets = Vec::new();
    for snippet in snippets {
        let n = snippet.chars().count();
        if used.saturating_add(n) > MAX_CHARS {
            let remain = MAX_CHARS.saturating_sub(used);
            if remain > 0 {
                kept_snippets.push(snippet.chars().take(remain).collect());
            }
            truncated = true;
            break;
        }
        used = used.saturating_add(n);
        kept_snippets.push(snippet);
    }

    ScreenTextContext {
        tokens: kept_tokens,
        snippets: kept_snippets,
        family,
        source: ScreenTextSource::Ax,
        truncated,
    }
}

fn title_counterpart(title: &str) -> Option<String> {
    let first = title
        .split(['-', '—', '|', '·'])
        .next()
        .unwrap_or(title)
        .trim();
    usable_text(first)
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
        title_counterpart, usable_text, LiveAxSnapshot,
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
        if let Some(title) = copy_string_attr(&window, "AXTitle") {
            if let Some(name) = title_counterpart(&title) {
                snap.counterpart = Some(name);
            }
            if snap.document_name.is_none() {
                snap.document_name = document_basename(&title);
            }
            if let Some(subject) = usable_text(&title) {
                snap.email_subject = Some(subject);
            }
        }
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

    #[test]
    fn chat_reads_counterpart_and_two_bubbles() {
        let ctx = extract_from_fixture(&AxWindowFixture {
            family: ContextFamily::PersonalChat,
            counterpart: Some("晓雯".into()),
            bubbles: vec!["在吗".into(), "晚点回你".into()],
            window_title: "晓雯 - 微信".into(),
            raw_url: Some("https://wx.qq.com/chat/secret".into()),
            pid: 4242,
            ..empty_fix()
        });
        assert!(ctx.tokens.iter().any(|t| t == "晓雯"));
        assert_eq!(ctx.snippets.len(), 2);
        assert_eq!(ctx.proper_noun_count(), ctx.tokens.len());
        let text = ctx.visible_context_text();
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
        fix.email_recipients = vec!["alex@example.com".into()];
        fix.email_subject = Some("Q3 plan".into());
        fix.bubbles = vec!["THIS IS THE BODY AND MUST NOT APPEAR".into()];
        let ctx = extract_from_fixture(&fix);
        assert!(ctx.tokens.iter().any(|t| t.contains("alex@example.com")));
        assert!(ctx.snippets.iter().any(|s| s.contains("Q3 plan")));
        assert!(!ctx.visible_context_text().contains("THIS IS THE BODY"));
    }

    #[test]
    fn ide_filenames_need_extension_and_no_spaces() {
        let mut fix = empty_fix();
        fix.family = ContextFamily::PromptOrCode;
        fix.ide_filenames = vec!["foo.ts".into(), "bad name.rs".into(), ".eslintrc".into()];
        fix.ide_symbols = vec!["handleUserAuthCallback".into()];
        let ctx = extract_from_fixture(&fix);
        assert!(ctx.tokens.contains(&"foo.ts".into()));
        assert!(ctx.tokens.contains(&"handleUserAuthCallback".into()));
        assert!(!ctx.tokens.iter().any(|t| t.contains(' ')));
        assert!(!ctx.tokens.iter().any(|t| t == ".eslintrc"));
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
                ctx.tokens.is_empty() && ctx.snippets.is_empty(),
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
        fix.ide_symbols = (0..80).map(|i| format!("Token{i}")).collect();
        let ctx = extract_from_fixture(&fix);
        assert!(ctx.tokens.len() <= 40);
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
        let dropped = resolve_screen_at_stop(&expected, &live, ContextFamily::PersonalChat, || {
            ScreenTextContext {
                tokens: vec!["晓雯".into()],
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
        assert!(ctx.tokens.is_empty());
        assert!(ctx.snippets.is_empty());
    }

    #[test]
    fn extractor_error_does_not_fail_session() {
        let ctx = screen_context_for_session(ContextFamily::PersonalChat, Err("ax denied"));
        assert!(ctx.tokens.is_empty());
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
        assert!(ctx.tokens.is_empty() && ctx.snippets.is_empty());
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
        let fix = fixture_from_live_ax(ContextFamily::PersonalChat, &guard, snap);
        let ctx = extract_from_fixture(&AxWindowFixture {
            window_title: "晓雯 - 微信".into(),
            raw_url: Some("https://wx.qq.com/chat/secret".into()),
            pid: 4242,
            ..fix
        });
        assert!(ctx.tokens.iter().any(|token| token == "晓雯"));
        assert_eq!(ctx.snippets.len(), 2);
        let text = ctx.visible_context_text();
        assert!(!text.contains("微信"));
        assert!(!text.contains("4242"));
        assert!(!text.contains("https://"));
        assert!(!text.contains("window_title"));
    }
}
