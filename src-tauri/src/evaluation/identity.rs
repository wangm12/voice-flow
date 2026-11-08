use super::admission_progress::{Progress, Scope};
use super::contracts::{ArtifactIdentity, BaselineIdentity};
use super::source_identity::{self, CheckoutObservation, SourceFacts, SourceProvenance};
use super::validation::{error, read_bounded, repository, sha256, InputError};
use std::path::Path;
use std::process::Command;

fn platform_fact(program: &str, args: &[&str]) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() || out.stdout.len() > 4096 {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}
fn compiled_facts(raw: &str) -> (SourceFacts, SourceProvenance) {
    match serde_json::from_str::<SourceFacts>(raw) {
        Ok(facts) if facts.format == source_identity::FORMAT => {
            let provenance = if facts.complete() {
                SourceProvenance::BuildScriptSnapshotV3 {}
            } else {
                SourceProvenance::Unknown {
                    reason: facts
                        .unavailable_reason
                        .clone()
                        .unwrap_or_else(|| "compiled_source_snapshot_incomplete".into()),
                }
            };
            (facts, provenance)
        }
        _ => (
            SourceFacts::unknown("compiled_source_metadata_unavailable"),
            SourceProvenance::Unknown {
                reason: "compiled_source_metadata_unavailable".into(),
            },
        ),
    }
}
fn bind_source(
    facts: SourceFacts,
    provenance: SourceProvenance,
    root: &Path,
) -> (SourceFacts, SourceProvenance, CheckoutObservation) {
    let live = source_identity::capture(root);
    let observation = CheckoutObservation {
        path: root.to_owned(),
        canonical_path: root.canonicalize().ok(),
        content_comparison: facts.comparison(&live),
        facts: live,
    };
    (facts, provenance, observation)
}
pub fn executable_sha256() -> Result<String, InputError> {
    let executable = std::env::current_exe().map_err(|_| {
        error(
            Path::new("executable"),
            "identity_unavailable",
            "cannot locate evaluator executable",
        )
    })?;
    Ok(sha256(&read_bounded(&executable, 1024 * 1024 * 1024)?))
}
pub fn baseline(progress: Option<&mut Progress>) -> Result<BaselineIdentity, InputError> {
    let mut disabled = Progress::disabled();
    let progress = progress.unwrap_or(&mut disabled);
    progress.begin(Scope::RepositorySource);
    let (facts, provenance) = compiled_facts(include_str!(concat!(
        env!("OUT_DIR"),
        "/voiceflow-source-identity.json"
    )));
    // The compiled path is only the location of an optional runtime observation.
    // Neither cwd nor a live git/source result can supply a compiled-code fact.
    let (facts, provenance, live_checkout) = bind_source(facts, provenance, &repository());
    progress.end(
        Scope::RepositorySource,
        live_checkout.facts.source_sha256.is_some(),
    );
    progress.begin(Scope::Lockfiles);
    // These hashes were captured by the build producer, never reopened as code identity.
    progress.end(Scope::Lockfiles, facts.lockfile_sha256.len() == 2);
    progress.begin(Scope::Executable);
    progress.declare_files([None]);
    progress.next_entry();
    let executable_sha256 = executable_sha256()?;
    progress.entry_done();
    progress.end(Scope::Executable, true);
    progress.begin(Scope::Platform);
    let os_version = platform_fact("/usr/bin/sw_vers", &["-productVersion"]);
    let os_build = platform_fact("/usr/bin/sw_vers", &["-buildVersion"]);
    let cpu = platform_fact("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]);
    let ram_bytes =
        platform_fact("/usr/sbin/sysctl", &["-n", "hw.memsize"]).and_then(|v| v.parse().ok());
    progress.end(Scope::Platform, true);
    Ok(BaselineIdentity {
        schema_version: 2,
        source_commit: facts.source_commit,
        dirty: facts.dirty,
        source_sha256: facts.source_sha256,
        lockfile_sha256: facts.lockfile_sha256,
        source_unavailable_path: facts.unavailable_path,
        source_git_unavailable_reason: facts.git_unavailable_reason,
        source_provenance: provenance,
        live_checkout: Some(live_checkout),
        live_checkout_end: None,
        executable_sha256,
        version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        os_version,
        os_build,
        cpu,
        ram_bytes,
    })
}

/// The recipe expressly names a source commit; a runtime checkout cannot prove it.
pub(super) fn verify_artifact(
    current: &BaselineIdentity,
    expected: &ArtifactIdentity,
    path: &Path,
) -> Result<(), InputError> {
    if !current.has_build_source_snapshot() || current.source_commit.is_none() {
        let reason = match &current.source_provenance {
            SourceProvenance::Unknown { reason } => reason.as_str(),
            SourceProvenance::RuntimeCheckoutLegacy => "legacy_runtime_checkout_is_not_build_proof",
            SourceProvenance::BuildScriptSnapshotV3 {} => current
                .source_git_unavailable_reason
                .as_deref()
                .unwrap_or("compiled_source_commit_unavailable"),
        };
        return Err(error(path, "compiled_source_identity_unavailable",
            format!("recipe source_commit requires a build-time source snapshot and known commit: {reason}")));
    }
    if Some(expected.source_commit.as_str()) != current.source_commit.as_deref()
        || expected.version != current.version
        || expected
            .binary_sha256
            .as_ref()
            .is_some_and(|v| *v != current.executable_sha256)
    {
        return Err(error(
            path,
            "identity_mismatch",
            "job artifact must match this VoiceFlow compiled source/version/executable",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{fixtures::FixtureDirectory, source_identity::CheckoutComparison};
    use super::*;

    fn source(d: &FixtureDirectory) -> SourceFacts {
        std::fs::create_dir_all(d.0.join("src-tauri/src")).unwrap();
        std::fs::write(d.0.join("src-tauri/src/lib.rs"), b"pub const CODE: u8 = 1;").unwrap();
        std::fs::write(d.0.join("src-tauri/Cargo.lock"), b"cargo fixture").unwrap();
        std::fs::write(d.0.join("package-lock.json"), b"node fixture").unwrap();
        source_identity::capture(&d.0)
    }
    #[test]
    fn retained_build_facts_do_not_follow_a_later_live_source_or_missing_checkout() {
        let d = FixtureDirectory::new();
        let compiled = source(&d);
        assert!(compiled.complete());
        let embedded = serde_json::to_string(&compiled).unwrap();
        let (frozen, provenance) = compiled_facts(&embedded);
        let (_, _, unchanged) = bind_source(frozen.clone(), provenance.clone(), &d.0);
        assert!(matches!(
            unchanged.content_comparison,
            CheckoutComparison::Matches {}
        ));
        std::fs::write(d.0.join("src-tauri/src/lib.rs"), b"pub const CODE: u8 = 2;").unwrap();
        let (still_frozen, _, changed) = bind_source(frozen, provenance, &d.0);
        assert_eq!(still_frozen, compiled);
        assert!(matches!(
            changed.content_comparison,
            CheckoutComparison::Mismatch {}
        ));
        assert_ne!(changed.facts.source_sha256, compiled.source_sha256);
        let (new_build, _) = compiled_facts(&serde_json::to_string(&changed.facts).unwrap());
        assert_ne!(new_build.source_sha256, compiled.source_sha256);
        let (_, _, packaged) = bind_source(
            compiled.clone(),
            SourceProvenance::BuildScriptSnapshotV3 {},
            &d.0.join("absent"),
        );
        assert!(matches!(
            packaged.content_comparison,
            CheckoutComparison::Unknown { .. }
        ));
        assert_eq!(compiled.source_sha256, still_frozen.source_sha256);
        assert!(packaged.facts.source_sha256.is_none());
    }
    #[test]
    fn unknown_build_metadata_cannot_be_supplied_by_a_readable_checkout() {
        let d = FixtureDirectory::new();
        assert!(source(&d).complete());
        for raw in ["", "{}", "null"] {
            let (unknown, provenance) = compiled_facts(raw);
            let (bound, provenance, observed) = bind_source(unknown, provenance, &d.0);
            assert!(bound.source_sha256.is_none());
            assert!(matches!(provenance, SourceProvenance::Unknown { .. }));
            assert!(observed.facts.source_sha256.is_some());
            assert!(matches!(
                observed.content_comparison,
                CheckoutComparison::Unknown { .. }
            ));
        }
    }
    #[test]
    fn build_without_git_keeps_content_identity_but_cannot_verify_named_recipe_commit() {
        let d = FixtureDirectory::new();
        let built = source(&d);
        assert!(built.source_commit.is_none());
        assert!(built.dirty.is_none());
        assert!(built.git_unavailable_reason.is_some());
        let report_fixture = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&report_fixture);
        let mut baseline = journal.snapshot().identity.baseline().clone();
        baseline.source_sha256 = built.source_sha256;
        baseline.source_commit = None;
        let expected = ArtifactIdentity {
            version: baseline.version.clone(),
            source_commit: "claimed-head".into(),
            binary_sha256: Some(baseline.executable_sha256.clone()),
        };
        assert_eq!(
            verify_artifact(&baseline, &expected, Path::new("recipe"))
                .unwrap_err()
                .code,
            "compiled_source_identity_unavailable"
        );
        baseline.source_commit = Some("compiled-head".into());
        let expected = ArtifactIdentity {
            source_commit: "compiled-head".into(),
            ..expected
        };
        assert!(verify_artifact(&baseline, &expected, Path::new("recipe")).is_ok());
        baseline.source_provenance = SourceProvenance::RuntimeCheckoutLegacy;
        assert_eq!(
            verify_artifact(&baseline, &expected, Path::new("recipe"))
                .unwrap_err()
                .code,
            "compiled_source_identity_unavailable"
        );
    }
    #[test]
    fn preexisting_baseline_remains_readable_with_explicit_legacy_provenance() {
        let d = FixtureDirectory::new();
        let (_, journal) = super::super::tests::journal_fixture(&d);
        let current = journal.snapshot().identity.baseline().clone();
        let mut old = serde_json::to_value(&current).unwrap();
        let object = old.as_object_mut().unwrap();
        for name in [
            "schema_version",
            "source_provenance",
            "source_unavailable_path",
            "source_git_unavailable_reason",
            "live_checkout",
            "live_checkout_end",
        ] {
            object.remove(name);
        }
        let old: BaselineIdentity = serde_json::from_value(old).unwrap();
        assert_eq!(old.schema_version, 1);
        assert!(matches!(
            old.source_provenance,
            SourceProvenance::RuntimeCheckoutLegacy
        ));
        assert!(!old.has_build_source_snapshot());
        assert_eq!(old.executable_sha256, current.executable_sha256);
        assert_eq!(old.source_sha256, current.source_sha256);
    }
    #[test]
    fn embedded_build_snapshot_matches_runtime_manifest_tree() {
        let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .expect("Cargo test requires CARGO_MANIFEST_DIR");
        let workspace_root = manifest_dir
            .parent()
            .expect("CARGO_MANIFEST_DIR must have a parent directory");

        let raw = include_str!(concat!(env!("OUT_DIR"), "/voiceflow-source-identity.json"));
        let (embedded, provenance) = compiled_facts(raw);
        let runtime = source_identity::capture(workspace_root);

        assert!(
            embedded.complete(),
            "embedded build facts must be complete: {embedded:?}"
        );
        assert!(
            runtime.complete(),
            "runtime manifest workspace facts must be complete: {runtime:?}"
        );
        assert_eq!(
            provenance,
            SourceProvenance::BuildScriptSnapshotV3 {},
            "embedded facts must be identified as a build-script snapshot"
        );
        assert_eq!(
            embedded.source_sha256,
            runtime.source_sha256,
            "embedded source hash must match the parent of runtime CARGO_MANIFEST_DIR; a cached build script must not lend facts from another checkout"
        );
        assert_eq!(
            embedded.lockfile_sha256, runtime.lockfile_sha256,
            "embedded lockfile hashes must match the runtime manifest workspace"
        );
        assert_eq!(
            embedded.source_commit, runtime.source_commit,
            "embedded git commit must match the runtime manifest workspace"
        );
        assert_eq!(
            embedded.dirty, runtime.dirty,
            "embedded dirty-state provenance must match the runtime manifest workspace"
        );
        assert_eq!(
            embedded.git_unavailable_reason, runtime.git_unavailable_reason,
            "embedded git-availability provenance must match the runtime manifest workspace"
        );
    }
}
