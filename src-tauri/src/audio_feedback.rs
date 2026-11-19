//! Start/stop earcons for dictation.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use crate::audio::CaptureCueHandle;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackSound {
    Start,
    Stop,
}

pub trait FeedbackPlayer: Send + Sync {
    fn play(&self, sound: FeedbackSound, volume: f32);
}

/// Map persisted user volume to `afplay -v` (0.0–1.0).
pub fn afplay_volume(user_volume: f32) -> f32 {
    if !user_volume.is_finite() {
        return 0.6;
    }
    user_volume.clamp(0.0, 1.0)
}

pub fn play_feedback(
    player: &dyn FeedbackPlayer,
    enabled: bool,
    sound: FeedbackSound,
    user_volume: f32,
) {
    if !enabled {
        return;
    }
    let volume = afplay_volume(user_volume);
    if volume <= 0.0 {
        return;
    }
    player.play(sound, volume);
}

/// Resolve bundled feedback WAV under `resource_dir/resources/{name}`, or manifest fallback.
pub fn resolve_feedback_wav(resource_dir: Option<&Path>, name: &str) -> PathBuf {
    if let Some(dir) = resource_dir {
        let bundled = dir.join("resources").join(name);
        if bundled.exists() {
            return bundled;
        }
    }
    #[cfg(debug_assertions)]
    {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(name)
    }
    #[cfg(not(debug_assertions))]
    {
        PathBuf::new()
    }
}

fn feedback_wav_path(app: Option<&AppHandle>, filename: &str) -> PathBuf {
    if let Some(app) = app {
        if let Ok(path) = app.path().resolve(
            format!("resources/{filename}"),
            tauri::path::BaseDirectory::Resource,
        ) {
            if path.exists() {
                return path;
            }
        }
        if let Ok(resource_dir) = app.path().resource_dir() {
            let path = resolve_feedback_wav(Some(&resource_dir), filename);
            if path.exists() {
                return path;
            }
        }
    }
    resolve_feedback_wav(None, filename)
}

struct AfplayPlayer {
    app: Option<AppHandle>,
}

impl AfplayPlayer {
    fn new(app: Option<AppHandle>) -> Self {
        Self { app }
    }
}

fn feedback_filename(sound: FeedbackSound) -> &'static str {
    match sound {
        FeedbackSound::Start => "feedback_start.wav",
        FeedbackSound::Stop => "feedback_stop.wav",
    }
}

/// Create the process inside its waiter so even failure to create the thread
/// cannot leave a spawned child without an owner. Playback stays off the caller.
fn spawn_feedback_process(
    command: Command,
) -> std::io::Result<std::thread::JoinHandle<std::io::Result<(u32, ExitStatus)>>> {
    spawn_feedback_process_with_wait(command, None, |mut child, _| {
        let pid = child.id();
        child.wait().map(|status| (pid, status))
    })
}

fn spawn_feedback_process_with_wait<W>(
    mut command: Command,
    cue: Option<CaptureCueHandle>,
    wait_child: W,
) -> std::io::Result<std::thread::JoinHandle<std::io::Result<(u32, ExitStatus)>>>
where
    W: FnOnce(std::process::Child, Option<CaptureCueHandle>) -> std::io::Result<(u32, ExitStatus)>
        + Send
        + 'static,
{
    std::thread::Builder::new()
        .name("voice-flow-feedback".into())
        .spawn(move || {
            let result = command.spawn().and_then(|child| wait_child(child, cue));
            match &result {
                Err(error) => log::warn!("feedback playback failed: {error}"),
                Ok((_, status)) if !status.success() => {
                    log::warn!("feedback playback exited unsuccessfully: {status}")
                }
                _ => {}
            }
            result
        })
}

impl FeedbackPlayer for AfplayPlayer {
    fn play(&self, sound: FeedbackSound, volume: f32) {
        let filename = feedback_filename(sound);
        let path = feedback_wav_path(self.app.as_ref(), filename);
        if !path.exists() {
            log::warn!("feedback wav missing: {}", path.display());
            return;
        }
        let mut command = Command::new("afplay");
        command
            .arg("-v")
            .arg(format!("{volume:.3}"))
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Err(error) = spawn_feedback_process(command) {
            log::warn!("feedback playback could not start: {error}");
        }
    }
}

#[cfg(test)]
pub struct MockFeedbackPlayer {
    calls: Mutex<Vec<(FeedbackSound, f32)>>,
}

#[cfg(test)]
impl MockFeedbackPlayer {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<(FeedbackSound, f32)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[cfg(test)]
impl FeedbackPlayer for MockFeedbackPlayer {
    fn play(&self, sound: FeedbackSound, volume: f32) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((sound, volume));
    }
}

#[cfg(test)]
static PLAYER_OVERRIDE: Mutex<Option<Arc<dyn FeedbackPlayer>>> = Mutex::new(None);

pub fn product_player(app: Option<&AppHandle>) -> Arc<dyn FeedbackPlayer> {
    #[cfg(test)]
    if let Some(player) = PLAYER_OVERRIDE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return player;
    }
    Arc::new(AfplayPlayer::new(app.cloned()))
}

#[cfg(test)]
pub fn set_test_player(player: Arc<dyn FeedbackPlayer>) {
    *PLAYER_OVERRIDE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(player);
}

#[cfg(test)]
pub fn clear_test_player() {
    *PLAYER_OVERRIDE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

fn cue_wav_duration(path: &Path) -> std::time::Duration {
    let Ok(reader) = hound::WavReader::open(path) else {
        return std::time::Duration::from_millis(120);
    };
    let spec = reader.spec();
    let sample_rate = u64::from(spec.sample_rate.max(1));
    let channels = u64::from(spec.channels.max(1));
    let frames = u64::from(reader.len()) / channels;
    let ms = frames
        .saturating_mul(1_000)
        .div_ceil(sample_rate)
        .clamp(20, 250);
    std::time::Duration::from_millis(ms)
}

fn spawn_start_feedback_process(
    command: Command,
    cue: Option<CaptureCueHandle>,
    cue_duration: std::time::Duration,
) -> std::io::Result<std::thread::JoinHandle<std::io::Result<(u32, ExitStatus)>>> {
    let max_cue_mask = (cue_duration + std::time::Duration::from_millis(240))
        .min(std::time::Duration::from_millis(300));
    spawn_feedback_process_with_wait(command, cue, move |mut child, cue| {
        let pid = child.id();
        if let Some(cue) = cue {
            cue.begin();
            let started = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        cue.finish(status.success());
                        return Ok((pid, status));
                    }
                    Ok(None) => {
                        let elapsed = started.elapsed();
                        if elapsed >= max_cue_mask {
                            // Unmask the mic once the cue window finishes even if
                            // `afplay` continues tearing down its audio graph.
                            cue.finish(true);
                            return child.wait().map(|status| (pid, status));
                        }
                        let step =
                            (max_cue_mask - elapsed).min(std::time::Duration::from_millis(10));
                        std::thread::sleep(step);
                    }
                    Err(error) => {
                        cue.finish(false);
                        return Err(error);
                    }
                }
            }
        }
        child.wait().map(|status| (pid, status))
    })
}

pub fn play_start(app: &AppHandle, enabled: bool, user_volume: f32, cue: Option<CaptureCueHandle>) {
    #[cfg(test)]
    if PLAYER_OVERRIDE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        let player = product_player(Some(app));
        play_feedback(player.as_ref(), enabled, FeedbackSound::Start, user_volume);
        return;
    }
    if !enabled {
        return;
    }
    let volume = afplay_volume(user_volume);
    if volume <= 0.0 {
        return;
    }
    let path = feedback_wav_path(Some(app), feedback_filename(FeedbackSound::Start));
    if !path.exists() {
        log::warn!("feedback wav missing: {}", path.display());
        return;
    }
    let cue_duration = cue_wav_duration(&path);
    let mut command = Command::new("afplay");
    command
        .arg("-v")
        .arg(format!("{volume:.3}"))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Err(error) = spawn_start_feedback_process(command, cue, cue_duration) {
        log::warn!("feedback playback could not start: {error}");
    }
}

pub fn play_stop(app: &AppHandle, enabled: bool, user_volume: f32) {
    let player = product_player(Some(app));
    play_feedback(player.as_ref(), enabled, FeedbackSound::Stop, user_volume);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[cfg(unix)]
    #[test]
    fn feedback_process_is_nonblocking_and_reaped_after_exit() {
        use std::os::{fd::OwnedFd, unix::net::UnixStream};

        // Keep cat waiting on input so returning cannot depend on child exit.
        // No speaker, microphone, audio process, or system setting is involved.
        let (input, hold_open) = UnixStream::pair().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .stdin(Stdio::from(OwnedFd::from(input)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let waiter = spawn_feedback_process(command).unwrap();
        assert!(!waiter.is_finished());
        drop(hold_open);
        let (pid, status) = waiter.join().unwrap().unwrap();
        assert!(status.success());

        // A completed process without wait() would still be available here.
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_helper_owns_cue_until_wait_and_marks_only_success() {
        use std::os::{fd::OwnedFd, unix::net::UnixStream};

        let cue = CaptureCueHandle::test_handle(16_000);
        let (input, hold_open) = UnixStream::pair().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .stdin(Stdio::from(OwnedFd::from(input)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (wait_started_tx, wait_started_rx) = std::sync::mpsc::channel();
        let waiter =
            spawn_feedback_process_with_wait(command, Some(cue.clone()), move |mut child, cue| {
                if let Some(cue) = &cue {
                    cue.begin();
                }
                wait_started_tx.send(()).unwrap();
                let pid = child.id();
                let result = child.wait().map(|status| (pid, status));
                if let Some(cue) = &cue {
                    cue.finish(matches!(&result, Ok((_, status)) if status.success()));
                }
                result
            })
            .unwrap();
        wait_started_rx.recv().unwrap();
        assert_eq!(cue.test_observation().0, Some(0));
        assert_eq!(cue.test_observation().1, None);
        drop(hold_open);
        let (_, status) = waiter.join().unwrap().unwrap();
        assert!(status.success());
        // No accepted packet covered the process envelope, so success stays pending
        // until a later packet can map it to source samples.
        assert_eq!(cue.test_observation().1, None);
        assert_eq!(cue.test_observation().0, Some(0));

        let failed = CaptureCueHandle::test_handle(16_000);
        let mut command = Command::new("/usr/bin/false");
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let (_, status) = spawn_start_feedback_process(
            command,
            Some(failed.clone()),
            std::time::Duration::from_millis(200),
        )
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
        assert!(!status.success());
        assert_eq!(failed.test_observation(), (None, None));

        let slow_teardown = CaptureCueHandle::test_handle(16_000);
        let (input, hold_open) = UnixStream::pair().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .stdin(Stdio::from(OwnedFd::from(input)))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let waiter = spawn_start_feedback_process(
            command,
            Some(slow_teardown.clone()),
            std::time::Duration::from_millis(30),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(320));
        // Cue mask window ended at the 300ms cap even while `cat` remained alive.
        assert_eq!(slow_teardown.test_observation().0, Some(0));
        assert!(slow_teardown.test_is_finished());
        drop(hold_open);
        let (_, status) = waiter.join().unwrap().unwrap();
        assert!(status.success());

        let wait_failed = CaptureCueHandle::test_handle(16_000);
        let mut command = Command::new("/bin/cat");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let result = spawn_feedback_process_with_wait(
            command,
            Some(wait_failed.clone()),
            |mut child, cue| {
                if let Some(cue) = &cue {
                    cue.begin();
                }
                let _pid = child.id();
                let _status = child.wait()?;
                if let Some(cue) = &cue {
                    cue.finish(false);
                }
                Err(std::io::Error::other(
                    "injected wait result failure after real child reap",
                ))
            },
        )
        .unwrap()
        .join()
        .unwrap();
        assert!(result.is_err());
        assert_eq!(wait_failed.test_observation(), (None, None));

        let missing = CaptureCueHandle::test_handle(16_000);
        let mut command = Command::new("/voice-flow-missing-feedback-fixture");
        command.stdout(Stdio::null()).stderr(Stdio::null());
        assert!(spawn_start_feedback_process(
            command,
            Some(missing.clone()),
            std::time::Duration::from_millis(50),
        )
        .unwrap()
        .join()
        .unwrap()
        .is_err());
        assert_eq!(missing.test_observation(), (None, None));
    }

    #[test]
    fn afplay_volume_clamps_and_recovers_non_finite() {
        assert_eq!(afplay_volume(1.5), 1.0);
        assert_eq!(afplay_volume(-0.2), 0.0);
        assert_eq!(afplay_volume(f32::NAN), 0.6);
        assert_eq!(afplay_volume(f32::INFINITY), 0.6);
        assert_eq!(afplay_volume(0.6), 0.6);
    }

    #[test]
    fn play_feedback_respects_enabled_and_zero_volume() {
        let player = MockFeedbackPlayer::new();
        play_feedback(&player, false, FeedbackSound::Start, 0.8);
        play_feedback(&player, true, FeedbackSound::Start, 0.0);
        assert!(player.calls().is_empty());

        play_feedback(&player, true, FeedbackSound::Stop, 0.5);
        assert_eq!(player.calls(), vec![(FeedbackSound::Stop, 0.5)]);
    }

    #[test]
    fn play_feedback_scales_volume_for_afplay() {
        let player = MockFeedbackPlayer::new();
        play_feedback(&player, true, FeedbackSound::Start, 1.8);
        assert_eq!(player.calls(), vec![(FeedbackSound::Start, 1.0)]);
    }

    #[test]
    fn resolve_feedback_wav_prefers_resource_dir() {
        let base =
            std::env::temp_dir().join(format!("voice-flow-feedback-test-{}", std::process::id()));
        let resources = base.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        let wav = resources.join("test_tone.wav");
        std::fs::write(&wav, b"RIFF").unwrap();

        let resolved = resolve_feedback_wav(Some(&base), "test_tone.wav");
        assert_eq!(resolved, wav);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_feedback_wav_uses_explicit_manifest_resource_dir() {
        let resource_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let expected = resource_dir.join("resources").join("feedback_start.wav");
        assert!(expected.is_file(), "bundled feedback asset must exist");

        let resolved = resolve_feedback_wav(Some(&resource_dir), "feedback_start.wav");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_player_override_is_used_and_cleared() {
        let mock = Arc::new(MockFeedbackPlayer::new());
        set_test_player(mock.clone());
        play_feedback(
            product_player(None).as_ref(),
            true,
            FeedbackSound::Stop,
            0.4,
        );
        clear_test_player();
        assert_eq!(mock.calls(), vec![(FeedbackSound::Stop, 0.4)]);
    }
}
