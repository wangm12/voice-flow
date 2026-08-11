//! Cross-build singleton guard for the desktop process.
//!
//! The Tauri single-instance plugin keys off the bundle identifier. That is
//! not enough while a dev binary and a packaged macOS app are both present,
//! so the desktop process also holds one OS-level lock shared by both builds.

#[cfg(unix)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::fd::AsRawFd;

    pub struct InstanceLock {
        _file: File,
    }

    pub fn acquire() -> io::Result<InstanceLock> {
        let path = std::env::temp_dir().join("voiceflow-single-instance.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;

        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(InstanceLock { _file: file })
    }
}

#[cfg(not(unix))]
mod platform {
    use std::io;

    pub struct InstanceLock;

    pub fn acquire() -> io::Result<InstanceLock> {
        Ok(InstanceLock)
    }
}

pub use platform::{acquire, InstanceLock};

pub fn acquire_or_exit() -> InstanceLock {
    match acquire() {
        Ok(lock) => lock,
        Err(error) => {
            log::info!("another VoiceFlow instance is already running: {error}");
            std::process::exit(0);
        }
    }
}
