use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, Result};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn require_absolute(path: &Path, flag: &'static str) -> Result<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(Error::InvalidArgument(flag))
    }
}

pub fn reject_same_file(input: &Path, output: &Path) -> Result<()> {
    let input =
        std::fs::canonicalize(input).map_err(|_| Error::Input("input file is unavailable"))?;
    let output = absolute_destination(output)?;
    if input == output {
        return Err(Error::InvalidArgument(
            "input and output must be different files",
        ));
    }
    Ok(())
}

pub fn write_private_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    require_absolute(path, "output paths must be absolute")?;
    let (temp_path, mut file) = create_private_temp(path)?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| Error::Output("could not write private output file"))?;
        file.sync_all()
            .map_err(|_| Error::Output("could not sync private output file"))?;
        drop(file);
        install_temp(&temp_path, path, replace)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

pub fn create_private_temp(path: &Path) -> Result<(PathBuf, File)> {
    require_absolute(path, "output paths must be absolute")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(Error::Output("output parent directory is unavailable"))?;
    if !parent.is_dir() {
        return Err(Error::Output("output parent directory is unavailable"));
    }
    let file_name = path
        .file_name()
        .ok_or(Error::Output("output path is invalid"))?
        .to_string_lossy();
    for _ in 0..128 {
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{file_name}.voiceflow-{}-{count}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).read(true).create_new(true);
        set_private_mode(&mut options);
        match options.open(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(Error::Output("could not create private temporary output")),
        }
    }
    Err(Error::Output(
        "could not reserve a private temporary output",
    ))
}

pub fn install_temp(temp_path: &Path, path: &Path, replace: bool) -> Result<()> {
    if replace {
        std::fs::rename(temp_path, path)
            .map_err(|_| Error::Output("could not atomically install output file"))?;
    } else {
        std::fs::hard_link(temp_path, path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::Output("destination already exists; existing file was preserved")
            } else {
                Error::Output("could not atomically install output file")
            }
        })?;
        std::fs::remove_file(temp_path)
            .map_err(|_| Error::Output("could not finish installing output file"))?;
    }
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

pub fn absolute_destination(path: &Path) -> Result<PathBuf> {
    require_absolute(path, "output paths must be absolute")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(Error::Output("output parent directory is unavailable"))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|_| Error::Output("output parent directory is unavailable"))?;
    let name = path
        .file_name()
        .ok_or(Error::Output("output path is invalid"))?;
    Ok(parent.join(name))
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = File::open(path).map_err(|_| Error::Input("input file is unavailable"))?;
    let mut hash = Sha256::new();
    let mut buf = [0_u8; 32 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| Error::Input("input file could not be read"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(unix)]
fn set_private_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
pub fn enforce_private_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata =
        std::fs::metadata(path).map_err(|_| Error::Output("private output file is unavailable"))?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)
        .map_err(|_| Error::Output("could not protect private output file"))
}

#[cfg(not(unix))]
pub fn enforce_private_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "voiceflow-speaker-filter-fileutil-{name}-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn create_new_preserves_an_existing_profile_file() {
        let path = temp_path("preserve");
        let _ = std::fs::remove_file(&path);
        write_private_atomic(&path, b"original", false).unwrap();
        assert!(write_private_atomic(&path, b"replacement", false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        std::fs::remove_file(path).unwrap();
    }
}
