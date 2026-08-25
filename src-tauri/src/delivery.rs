//! Delivery policy and result vocabulary shared by recording workflows.
//!
//! The keyboard shortcut is an attempt to deliver text, not proof that the
//! focused application accepted it. Keeping that distinction in one module
//! prevents the UI, History, and terminal state from drifting apart.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPolicy {
    /// Try the focused input first, then preserve the result on the clipboard.
    Auto,
    /// Prefer the focused input, but still fall back to the clipboard if the
    /// target cannot be safely reached.
    PasteShortcut,
    /// Do not synthesize a keyboard shortcut.
    ClipboardOnly,
    /// Persist to History without changing the active application.
    HistoryOnly,
}

impl DeliveryPolicy {
    pub const fn default_value() -> Self {
        Self::Auto
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "paste" | "paste_shortcut" => Self::PasteShortcut,
            "clipboard" | "clipboard_only" => Self::ClipboardOnly,
            "history" | "history_only" => Self::HistoryOnly,
            _ => Self::Auto,
        }
    }

    pub fn is_valid(value: &str) -> bool {
        matches!(
            value,
            "auto" | "paste_shortcut" | "clipboard_only" | "history_only"
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::PasteShortcut => "paste_shortcut",
            Self::ClipboardOnly => "clipboard_only",
            Self::HistoryOnly => "history_only",
        }
    }

    pub const fn attempts_paste(self) -> bool {
        matches!(self, Self::Auto | Self::PasteShortcut)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMethod {
    None,
    Paste,
    PasteUnverified,
    Clipboard,
    History,
}

impl DeliveryMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Paste => "paste",
            Self::PasteUnverified => "paste_unverified",
            Self::Clipboard => "clipboard",
            Self::History => "history",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryResult {
    pub method: DeliveryMethod,
    pub verified: bool,
    pub fallback_reason: Option<&'static str>,
}

impl DeliveryResult {
    /// Verified insert is a paste. Anything else keeps the clipboard and asks
    /// the user to press ⌘V — including when VoiceFlow stole frontmost.
    pub fn from_insert_verified(verified: bool) -> Self {
        if verified {
            Self {
                method: DeliveryMethod::Paste,
                verified: true,
                fallback_reason: None,
            }
        } else {
            Self {
                method: DeliveryMethod::Clipboard,
                verified: true,
                fallback_reason: Some("paste_unverified"),
            }
        }
    }

    pub fn for_method(method: &str, fallback_reason: Option<&'static str>) -> Self {
        let method = match method {
            "paste" => DeliveryMethod::Paste,
            "paste_unverified" => DeliveryMethod::PasteUnverified,
            "clipboard" => DeliveryMethod::Clipboard,
            "history" => DeliveryMethod::History,
            _ => DeliveryMethod::None,
        };
        Self {
            verified: matches!(
                method,
                DeliveryMethod::Paste | DeliveryMethod::Clipboard | DeliveryMethod::History
            ),
            method,
            fallback_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_and_new_policy_values() {
        assert_eq!(
            DeliveryPolicy::parse("paste"),
            DeliveryPolicy::PasteShortcut
        );
        assert_eq!(
            DeliveryPolicy::parse("paste_shortcut"),
            DeliveryPolicy::PasteShortcut
        );
        assert_eq!(
            DeliveryPolicy::parse("clipboard"),
            DeliveryPolicy::ClipboardOnly
        );
        assert_eq!(
            DeliveryPolicy::parse("history_only"),
            DeliveryPolicy::HistoryOnly
        );
        assert_eq!(DeliveryPolicy::parse("unknown"), DeliveryPolicy::Auto);
    }

    #[test]
    fn insert_without_frontmost_target_is_a_copy_fallback() {
        let verified = DeliveryResult::from_insert_verified(true);
        assert_eq!(verified.method, DeliveryMethod::Paste);
        assert!(verified.fallback_reason.is_none());

        let copied = DeliveryResult::from_insert_verified(false);
        assert_eq!(copied.method, DeliveryMethod::Clipboard);
        assert_eq!(copied.fallback_reason, Some("paste_unverified"));
    }

    #[test]
    fn auto_and_paste_preserve_a_clipboard_fallback() {
        assert!(DeliveryPolicy::Auto.attempts_paste());
        assert!(!DeliveryPolicy::ClipboardOnly.attempts_paste());
        assert!(!DeliveryPolicy::HistoryOnly.attempts_paste());
    }
}
