use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn should_show_whats_new(last_seen_version: &str) -> bool {
    let last_seen = last_seen_version.trim();
    if last_seen.is_empty() {
        return false;
    }
    let current = current_version();
    if last_seen == current {
        return false;
    }
    version_tuple(last_seen) < version_tuple(current)
}

fn version_tuple(version: &str) -> semver::Version {
    semver::Version::parse(version).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

pub fn resolve_release_notes_path(resource_dir: Option<&Path>, version: &str) -> PathBuf {
    let filename = format!("{version}.md");
    if let Some(dir) = resource_dir {
        for bundled in [
            dir.join("resources").join("release-notes").join(&filename),
            dir.join("release-notes").join(&filename),
        ] {
            if bundled.exists() {
                return bundled;
            }
        }
    }
    #[cfg(debug_assertions)]
    {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/release-notes")
            .join(filename)
    }
    #[cfg(not(debug_assertions))]
    {
        PathBuf::new()
    }
}

pub fn load_release_notes(resource_dir: Option<&Path>, version: &str) -> Option<String> {
    let path = resolve_release_notes_path(resource_dir, version);
    std::fs::read_to_string(path).ok()
}

#[derive(Debug, Clone, Serialize)]
pub struct WhatsNewStatus {
    pub should_show: bool,
    pub version: String,
    pub notes: Option<String>,
}

pub fn whats_new_status(last_seen_version: &str, resource_dir: Option<&Path>) -> WhatsNewStatus {
    let version = current_version().to_owned();
    let should_show = should_show_whats_new(last_seen_version);
    let notes = if should_show {
        load_release_notes(resource_dir, &version)
    } else {
        None
    };
    WhatsNewStatus {
        should_show,
        version,
        notes,
    }
}

/// Always load release notes for the current package version, ignoring last-seen gating.
#[tauri::command]
pub fn preview_whats_new(app: AppHandle) -> WhatsNewStatus {
    let version = current_version().to_owned();
    let resource_dir = app.path().resource_dir().ok();
    WhatsNewStatus {
        should_show: true,
        version: version.clone(),
        notes: load_release_notes(resource_dir.as_deref(), &version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_show_whats_new_when_last_seen_is_older() {
        assert!(should_show_whats_new("0.0.9"));
    }

    #[test]
    fn should_not_show_whats_new_when_last_seen_matches_current() {
        assert!(!should_show_whats_new(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn should_not_show_whats_new_when_last_seen_is_empty() {
        assert!(!should_show_whats_new(""));
    }

    #[test]
    fn should_not_show_whats_new_when_last_seen_is_newer() {
        assert!(!should_show_whats_new("99.0.0"));
    }

    #[test]
    fn load_release_notes_reads_bundled_public_file() {
        let notes = load_release_notes(None, env!("CARGO_PKG_VERSION"));
        assert!(
            notes.is_some(),
            "expected release notes for current version"
        );
        assert!(notes.unwrap().contains("VoiceFlow"));
    }

    #[test]
    fn load_release_notes_reads_resource_dir_fixture() {
        let base =
            std::env::temp_dir().join(format!("voice-flow-whats-new-test-{}", std::process::id()));
        let notes_dir = base.join("resources").join("release-notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        let file = notes_dir.join("0.1.0.md");
        std::fs::write(&file, "# VoiceFlow 0.1.0\n\nBundled release notes.").unwrap();

        let notes = load_release_notes(Some(&base), "0.1.0");
        assert!(notes.is_some(), "expected notes from resource_dir fixture");
        assert!(notes.unwrap().contains("Bundled release notes"));

        let _ = std::fs::remove_dir_all(&base);
    }
}
