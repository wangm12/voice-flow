//! Opt-in immutable source admission. Uses the same pinned-file verifier as load.
use super::{checked_model_directory, file_identity, verify_pinned_model_files, FileIdentity};
use crate::ondevice_models::mlx_model;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) enum CopyFailure {
    Cancelled,
    Failed(String),
}
impl From<String> for CopyFailure {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}
impl From<&str> for CopyFailure {
    fn from(value: &str) -> Self {
        Self::Failed(value.into())
    }
}
#[derive(Debug)]
pub(crate) struct VerifiedModelSource {
    pub(crate) model_id: String,
    pub(crate) root: PathBuf,
    directory: PathBuf,
    expected: Vec<(String, u64, String)>,
    identities: Vec<FileIdentity>,
    files: Vec<File>,
    #[cfg(test)]
    verified_hash_bytes: u64,
}
impl VerifiedModelSource {
    pub(crate) fn open(root: &Path, model_id: &str) -> Result<Self, String> {
        let manifest = mlx_model(model_id).ok_or("unsupported local model")?;
        let expected = manifest
            .files
            .iter()
            .map(|f| (f.path.into(), f.bytes, f.sha256.into()))
            .collect();
        Self::open_expected(root, model_id, expected)
    }
    fn open_expected(
        root: &Path,
        model_id: &str,
        expected: Vec<(String, u64, String)>,
    ) -> Result<Self, String> {
        let directory = checked_model_directory(root, model_id)?;
        if std::fs::symlink_metadata(root.join(model_id))
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("local model source directory must not be a symlink".into());
        }
        // The source tree is bounded and contains only the declared files plus a
        // possible installed marker/derived tokenizer; no recursive discovery.
        if std::fs::read_dir(&directory)
            .map_err(|e| e.to_string())?
            .take(33)
            .count()
            > 32
        {
            return Err("local model source directory exceeds 32 entries".into());
        }
        let mut files = vec![];
        let hashed_bytes = std::sync::atomic::AtomicU64::new(0);
        let identities = verify_pinned_model_files(
            root,
            model_id,
            &expected,
            None,
            Some(&mut files),
            &hashed_bytes,
        )?;
        #[cfg(test)]
        let verified_hash_bytes = hashed_bytes.load(std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            model_id: model_id.into(),
            root: root.into(),
            directory,
            expected,
            identities,
            files,
            #[cfg(test)]
            verified_hash_bytes,
        })
    }
    pub(crate) fn files(&self) -> &[(String, u64, String)] {
        &self.expected
    }
    /// Copy from held descriptors, checking the same pinned byte identity while
    /// copying. Never hard-link or reopen a source under a previously held SHA.
    pub(crate) fn copy_file(
        &self,
        index: usize,
        output: &mut File,
        cancel: &tokio_util::sync::CancellationToken,
        mut progress: impl FnMut(u64),
    ) -> Result<(), CopyFailure> {
        use std::io::BufReader;
        let (name, bytes, sha) = &self.expected[index];
        let identity = &self.identities[index];
        let file = &self.files[index];
        if checked_model_directory(&self.root, &self.model_id)? != self.directory
            || file_identity(name, &file.metadata().map_err(|e| e.to_string())?) != *identity
        {
            return Err("local model source identity changed before copy".into());
        }
        let path = self.directory.join(name);
        let before = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if before.file_type().is_symlink() || file_identity(name, &before) != *identity {
            return Err("local model source path no longer names held input".into());
        }
        let mut input = file.try_clone().map_err(|e| e.to_string())?;
        input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(input.take(bytes.saturating_add(1)));
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut total = 0u64;
        let mut written = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(CopyFailure::Cancelled);
            }
            let n = reader.read(&mut buffer[..]).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > *bytes {
                return Err("local model source grew during copy".into());
            }
            hasher.update(&buffer[..n]);
            if cancel.is_cancelled() {
                return Err(CopyFailure::Cancelled);
            }
            let mut offset = 0;
            while offset < n {
                if cancel.is_cancelled() {
                    return Err(CopyFailure::Cancelled);
                }
                let count = output
                    .write(&buffer[offset..n])
                    .map_err(|e| e.to_string())?;
                if count == 0 {
                    return Err(CopyFailure::Failed("copy writer made no progress".into()));
                }
                offset += count;
                written += count as u64;
                progress(written);
            }
            if cancel.is_cancelled() {
                return Err(CopyFailure::Cancelled);
            }
        }
        let after = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if total != *bytes
            || hex::encode(hasher.finalize()) != *sha
            || after.file_type().is_symlink()
            || file_identity(name, &after) != *identity
            || file_identity(name, &file.metadata().map_err(|e| e.to_string())?) != *identity
        {
            return Err("local model source changed or failed pinned integrity during copy".into());
        }
        if cancel.is_cancelled() {
            return Err(CopyFailure::Cancelled);
        }
        output
            .sync_all()
            .map_err(|e| CopyFailure::Failed(e.to_string()))
    }
    #[cfg(test)]
    pub(crate) fn fixture(root: &Path, model_id: &str, bytes: &[u8]) -> Self {
        std::fs::create_dir(root.join(model_id)).unwrap();
        std::fs::write(root.join(model_id).join("model.safetensors"), bytes).unwrap();
        Self::open_expected(
            root,
            model_id,
            vec![(
                "model.safetensors".into(),
                bytes.len() as u64,
                hex::encode(Sha256::digest(bytes)),
            )],
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_writer_failure_and_pre_cancel_preserve_typed_reason_and_written_extent() {
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let source = VerifiedModelSource::fixture(
            &d.0.canonicalize().unwrap(),
            crate::ondevice_models::QWEN_ASR_06_ID,
            b"PINN",
        );
        let mut read_only =
            File::open(d.0.join(&source.model_id).join("model.safetensors")).unwrap();
        let mut observed = 0;
        let result = source.copy_file(
            0,
            &mut read_only,
            &tokio_util::sync::CancellationToken::new(),
            |n| observed = n,
        );
        assert!(matches!(result, Err(CopyFailure::Failed(_))));
        assert_eq!(observed, 0);
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let mut output = File::create(d.0.join("copy")).unwrap();
        let result = source.copy_file(0, &mut output, &cancel, |_| {
            panic!("cancelled copy cannot write")
        });
        assert!(matches!(result, Err(CopyFailure::Cancelled)));
        assert_eq!(output.metadata().unwrap().len(), 0);
    }
    #[test]
    fn pinned_source_rejects_changed_bytes_and_additional_weights() {
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let root = d.0.canonicalize().unwrap();
        let source =
            VerifiedModelSource::fixture(&root, crate::ondevice_models::QWEN_ASR_06_ID, b"PINN");
        let expected = source.files().to_vec();
        let directory = root.join(&source.model_id);
        std::fs::write(directory.join("extra.safetensors"), b"extra").unwrap();
        assert!(
            VerifiedModelSource::open_expected(&root, &source.model_id, expected.clone()).is_err()
        );
        std::fs::remove_file(directory.join("extra.safetensors")).unwrap();
        std::fs::write(directory.join("model.safetensors"), b"FAIL").unwrap();
        assert!(VerifiedModelSource::open_expected(&root, &source.model_id, expected).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn pinned_source_rejects_file_and_directory_symlinks() {
        use std::os::unix::fs::symlink;
        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let root = d.0.canonicalize().unwrap();
        let source =
            VerifiedModelSource::fixture(&root, crate::ondevice_models::QWEN_ASR_06_ID, b"PINN");
        let expected = source.files().to_vec();
        let directory = root.join(&source.model_id);
        let file = directory.join("model.safetensors");
        std::fs::rename(&file, root.join("saved-weight")).unwrap();
        symlink(root.join("saved-weight"), &file).unwrap();
        assert!(
            VerifiedModelSource::open_expected(&root, &source.model_id, expected.clone()).is_err()
        );
        std::fs::remove_file(&file).unwrap();
        std::fs::rename(root.join("saved-weight"), &file).unwrap();
        let moved = root.join("other-directory");
        std::fs::rename(&directory, &moved).unwrap();
        symlink(&moved, &directory).unwrap();
        assert!(VerifiedModelSource::open_expected(&root, &source.model_id, expected).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn existing_durable_proof_does_not_skip_research_descriptor_hash() {
        use std::sync::atomic::AtomicU64;

        let d = crate::evaluation::fixtures::FixtureDirectory::new();
        let root = d.0.join("models");
        let model_id = crate::ondevice_models::QWEN_ASR_06_ID;
        let directory = crate::ondevice_models::mlx_model(model_id).unwrap();
        let model_directory = crate::ondevice_runtime::model_dir(&root, model_id);
        std::fs::create_dir_all(&model_directory).unwrap();
        let payload = b"tiny research model payload";
        let expected = vec![(
            "model.safetensors".to_owned(),
            payload.len() as u64,
            hex::encode(Sha256::digest(payload)),
        )];
        std::fs::write(model_directory.join("model.safetensors"), payload).unwrap();
        let marker = serde_json::json!({
            "schema_version": 1,
            "id": model_id,
            "revision": directory.revision,
            "files": expected.iter().map(|(path, bytes, sha256)| serde_json::json!({
                "path": path,
                "bytes": bytes,
                "sha256": sha256,
            })).collect::<Vec<_>>(),
        });
        std::fs::write(
            model_directory.join(".voiceflow-model-manifest.json"),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        let root = root.canonicalize().unwrap();
        let cold_hash_bytes = AtomicU64::new(0);
        super::super::verify_pinned_model_files(
            &root,
            model_id,
            &expected,
            None,
            None,
            &cold_hash_bytes,
        )
        .unwrap();
        assert!(cold_hash_bytes.load(std::sync::atomic::Ordering::Relaxed) > 0);

        let source = VerifiedModelSource::open_expected(&root, model_id, expected).unwrap();
        assert_eq!(source.files.len(), 1);
        assert!(source.verified_hash_bytes > 0);
    }
}
