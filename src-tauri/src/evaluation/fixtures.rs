//! Test-only exclusive fixture namespaces, sharing production file-identity facts.
use super::artifacts::{id, FileId};
use chacha20poly1305::{
    aead::{AeadCore, OsRng},
    XChaCha20Poly1305,
};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(crate) struct FixtureDirectory(pub(crate) PathBuf, FileId, Option<File>);
fn retained() -> &'static Mutex<Vec<(PathBuf, File)>> {
    static RETAINED: OnceLock<Mutex<Vec<(PathBuf, File)>>> = OnceLock::new();
    RETAINED.get_or_init(Mutex::default)
}
impl FixtureDirectory {
    pub(crate) fn new() -> Self {
        Self::new_under(&std::env::temp_dir())
    }
    pub(crate) fn new_under(parent: &Path) -> Self {
        Self::allocate(|| {
            let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
            parent.join(format!(
                "voiceflow-fixture-{}-{}",
                std::process::id(),
                hex::encode(nonce)
            ))
        })
        .expect("allocate exclusive fixture namespace")
    }
    fn allocate(mut next: impl FnMut() -> PathBuf) -> std::io::Result<Self> {
        for _ in 0..8 {
            let path = next();
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {
                    let file = File::open(&path)?;
                    let identity = id(&file.metadata()?)?;
                    return Ok(Self(path, identity, Some(file)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "bounded fixture allocation exhausted; existing paths preserved",
        ))
    }
    pub(crate) fn write(&self, name: &str, value: &serde_json::Value) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }
    pub(crate) fn relocate(&mut self, path: &Path) {
        assert_eq!(id(&std::fs::metadata(path).unwrap()).unwrap(), self.1);
        self.0 = path.into();
    }
    fn cleanup(&mut self) -> std::io::Result<()> {
        if id(&std::fs::metadata(&self.0)?)? != self.1 {
            return Err(std::io::Error::other(
                "fixture namespace path identity changed",
            ));
        }
        std::fs::remove_dir_all(&self.0)?;
        self.2.take();
        Ok(())
    }
}
impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        if self.2.is_some() && self.cleanup().is_err() {
            if let Some(file) = self.2.take() {
                retained()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push((self.0.clone(), file));
            }
        }
    }
}
#[test]
fn exclusive_fixture_allocator_retries_collision_and_bounds_repeated_collisions() {
    let first = FixtureDirectory::new();
    let second_path = first.0.with_extension("second");
    let mut count = 0;
    let second = FixtureDirectory::allocate(|| {
        count += 1;
        if count == 1 {
            first.0.clone()
        } else {
            second_path.clone()
        }
    })
    .unwrap();
    assert_eq!(count, 2);
    assert_ne!(first.0, second.0);
    assert!(FixtureDirectory::allocate(|| first.0.clone()).is_err());
    assert!(first.0.is_dir());
}
#[test]
fn parallel_fixture_allocations_have_independent_exclusive_ownership() {
    let threads: Vec<_> = (0..32)
        .map(|_| std::thread::spawn(FixtureDirectory::new))
        .collect();
    let fixtures: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let names: std::collections::BTreeSet<_> = fixtures.iter().map(|f| &f.0).collect();
    assert_eq!(names.len(), fixtures.len());
}
#[test]
fn fixture_cleanup_requires_current_namespace_identity_after_move() {
    let mut original = FixtureDirectory::new();
    let selected = original.0.clone();
    let moved = selected.with_extension("moved");
    std::fs::rename(&selected, &moved).unwrap();
    let mut replacement = FixtureDirectory::new();
    std::fs::rename(&replacement.0, &selected).unwrap();
    replacement.relocate(&selected);
    std::fs::write(selected.join("sentinel"), b"replacement namespace").unwrap();
    assert!(original.cleanup().is_err());
    assert_eq!(
        std::fs::read(selected.join("sentinel")).unwrap(),
        b"replacement namespace"
    );
    original.relocate(&moved);
    original.cleanup().unwrap();
    assert!(!moved.exists());
    assert!(selected.exists());
}
