use crate::dictation;
use tauri::AppHandle;

#[cfg(feature = "research-harness")]
const TEST_GROQ_KEY_IMPORT_FLAG: &str = "--import-test-groq-key-stdin-v2";
#[cfg(feature = "research-harness")]
const TEST_GROQ_KEY_STATUS_FLAG: &str = "--test-groq-key-status-v1";
const OWNED_WAV_REPLAY_FLAG: &str = "--research-owned-wav";
const OWNED_WAV_TARGET_NONCE_FLAG: &str = "--target-nonce";
const OWNED_WAV_ROUTE_FLAG: &str = "--research-owned-route";

#[cfg(feature = "research-harness")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestGroqKeyImportInvocation {
    NotRequested,
    Import,
    InvalidInvocation,
}

#[cfg(feature = "research-harness")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestGroqKeyStatusInvocation {
    NotRequested,
    Status,
    InvalidInvocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnedWavReplayRoute {
    NativeNative,
    NativeGroq,
    GroqNative,
    GroqGroq,
}

impl OwnedWavReplayRoute {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "native-native" => Some(Self::NativeNative),
            "native-groq" => Some(Self::NativeGroq),
            "groq-native" => Some(Self::GroqNative),
            "groq-groq" => Some(Self::GroqGroq),
            _ => None,
        }
    }

    pub(crate) fn matches_settings(self, settings: &crate::store::Settings) -> bool {
        use crate::engine::EngineProvider::{Groq, OnDevice, OnDeviceMlx};

        let native_asr = settings.asr_provider == OnDevice
            && matches!(
                settings.asr_model.as_str(),
                crate::ondevice_models::QWEN_ASR_06_ID | crate::ondevice_models::QWEN_ASR_17_ID
            );
        let groq_asr = settings.asr_provider == Groq
            && settings.asr_model == crate::asr::MODEL
            && settings.asr_endpoint().as_deref()
                == Some("https://api.groq.com/openai/v1/audio/transcriptions")
            && !settings.asr_credential().is_empty();
        let native_cleanup = settings.cleanup_provider == OnDeviceMlx
            && settings.cleanup_model == crate::modelstore::MODEL_ID;
        let groq_cleanup = settings.cleanup_provider == Groq
            && settings.cleanup_model == crate::llm::MODEL
            && settings.cleanup_endpoint() == "https://api.groq.com/openai/v1/chat/completions"
            && !settings.cleanup_credential().is_empty();

        settings.cleanup_enabled
            && match self {
                Self::NativeNative => native_asr && native_cleanup,
                Self::NativeGroq => native_asr && groq_cleanup && !settings.strict_offline_enabled,
                Self::GroqNative => groq_asr && native_cleanup && !settings.strict_offline_enabled,
                Self::GroqGroq => groq_asr && groq_cleanup && !settings.strict_offline_enabled,
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnedWavReplayInvocation {
    NotRequested,
    Replay {
        path: std::path::PathBuf,
        target_nonce: String,
        route: OwnedWavReplayRoute,
    },
    InvalidInvocation,
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
        && matches!(value.as_bytes().get(14), Some(b'1'..=b'8'))
        && matches!(value.as_bytes().get(19), Some(b'8' | b'9' | b'a' | b'b'))
}

pub(crate) fn parse_owned_wav_replay(argv: &[impl AsRef<str>]) -> OwnedWavReplayInvocation {
    let flag_matches = argv
        .iter()
        .filter(|argument| argument.as_ref().starts_with(OWNED_WAV_REPLAY_FLAG))
        .count();
    let nonce_matches = argv
        .iter()
        .filter(|argument| argument.as_ref().starts_with(OWNED_WAV_TARGET_NONCE_FLAG))
        .count();
    let route_matches = argv
        .iter()
        .filter(|argument| argument.as_ref().starts_with(OWNED_WAV_ROUTE_FLAG))
        .count();
    if flag_matches == 0 && nonce_matches == 0 && route_matches == 0 {
        return OwnedWavReplayInvocation::NotRequested;
    }
    let flag_index = if argv
        .first()
        .is_some_and(|argument| argument.as_ref() == OWNED_WAV_REPLAY_FLAG)
    {
        0
    } else if argv
        .get(1)
        .is_some_and(|argument| argument.as_ref() == OWNED_WAV_REPLAY_FLAG)
    {
        1
    } else {
        return OwnedWavReplayInvocation::InvalidInvocation;
    };
    if flag_matches != 1 || nonce_matches != 1 || route_matches > 1 {
        return OwnedWavReplayInvocation::InvalidInvocation;
    }
    let expected_len = if route_matches == 0 {
        flag_index + 4
    } else {
        flag_index + 6
    };
    if argv.len() != expected_len {
        return OwnedWavReplayInvocation::InvalidInvocation;
    }
    let Some(path_argument) = argv.get(flag_index + 1) else {
        return OwnedWavReplayInvocation::InvalidInvocation;
    };
    let Some(nonce_flag) = argv.get(flag_index + 2) else {
        return OwnedWavReplayInvocation::InvalidInvocation;
    };
    let Some(target_nonce) = argv.get(flag_index + 3) else {
        return OwnedWavReplayInvocation::InvalidInvocation;
    };
    let path = std::path::PathBuf::from(path_argument.as_ref());
    let nonce_flag = nonce_flag.as_ref();
    let target_nonce = target_nonce.as_ref();
    let route = if route_matches == 0 {
        OwnedWavReplayRoute::NativeNative
    } else {
        let Some(route_flag) = argv.get(flag_index + 4) else {
            return OwnedWavReplayInvocation::InvalidInvocation;
        };
        let Some(route_value) = argv.get(flag_index + 5) else {
            return OwnedWavReplayInvocation::InvalidInvocation;
        };
        if route_flag.as_ref() != OWNED_WAV_ROUTE_FLAG {
            return OwnedWavReplayInvocation::InvalidInvocation;
        }
        let Some(route) = OwnedWavReplayRoute::parse(route_value.as_ref()) else {
            return OwnedWavReplayInvocation::InvalidInvocation;
        };
        route
    };
    if !path.is_absolute()
        || nonce_flag != OWNED_WAV_TARGET_NONCE_FLAG
        || !is_canonical_uuid(target_nonce)
    {
        return OwnedWavReplayInvocation::InvalidInvocation;
    }
    OwnedWavReplayInvocation::Replay {
        path,
        target_nonce: target_nonce.to_owned(),
        route,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SingleInstanceCallbackAdmission {
    Continue,
    RejectTestCredentialArguments,
}

fn contains_test_credential_reserved_argument(argv: &[impl AsRef<str>]) -> bool {
    argv.iter().any(|argument| {
        let argument = argument.as_ref();
        argument.starts_with("--import-test-groq-key-")
            || argument.starts_with("--test-groq-key-status")
    })
}

/// Reject credential import/status arguments before the single-instance
/// callback can parse ordinary actions, replay switches, or focus the window.
pub(crate) fn admit_single_instance_callback_args(
    argv: &[impl AsRef<str>],
) -> SingleInstanceCallbackAdmission {
    if contains_test_credential_reserved_argument(argv) {
        SingleInstanceCallbackAdmission::RejectTestCredentialArguments
    } else {
        SingleInstanceCallbackAdmission::Continue
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteCliAction {
    ToggleTranscription,
    ToggleVerbatim,
    Cancel,
}

/// Priority: `--cancel` > `--toggle-verbatim` > `--toggle-transcription`.
pub fn parse_cli(argv: &[impl AsRef<str>]) -> Option<RemoteCliAction> {
    if argv.iter().any(|arg| arg.as_ref() == "--cancel") {
        return Some(RemoteCliAction::Cancel);
    }
    if argv.iter().any(|arg| arg.as_ref() == "--toggle-verbatim") {
        return Some(RemoteCliAction::ToggleVerbatim);
    }
    if argv
        .iter()
        .any(|arg| arg.as_ref() == "--toggle-transcription")
    {
        return Some(RemoteCliAction::ToggleTranscription);
    }
    None
}

#[derive(Debug, Clone)]
pub struct CliArgs {
    argv: Vec<String>,
}

impl CliArgs {
    pub fn parse() -> Self {
        Self {
            argv: std::env::args().collect(),
        }
    }

    pub fn background(&self) -> bool {
        self.argv.iter().any(|arg| arg == "--background")
    }

    pub fn show_settings(&self) -> bool {
        !self.background()
            && !self.research_stdio()
            && self.remote_action().is_none()
            && !self.test_credential_cli_requested()
            && self.owned_wav_replay() == OwnedWavReplayInvocation::NotRequested
    }

    pub fn research_stdio(&self) -> bool {
        self.argv.iter().any(|arg| arg == "--research-stdio")
    }

    pub fn remote_action(&self) -> Option<RemoteCliAction> {
        parse_cli(&self.argv)
    }

    fn test_credential_cli_requested(&self) -> bool {
        contains_test_credential_reserved_argument(&self.argv)
    }

    /// Detect reserved Test credential switches so a default build can reject
    /// them before Tauri startup or single-instance forwarding.
    pub(crate) fn test_credential_reserved_argument_present(&self) -> bool {
        contains_test_credential_reserved_argument(&self.argv)
    }

    #[cfg(not(feature = "research-harness"))]
    pub(crate) fn unsupported_test_credential_cli_requested(&self) -> bool {
        self.test_credential_reserved_argument_present()
    }

    #[cfg(feature = "research-harness")]
    pub(crate) fn test_groq_key_import(&self) -> TestGroqKeyImportInvocation {
        let import_arguments = self
            .argv
            .iter()
            .skip(1)
            .filter(|argument| argument.starts_with(TEST_GROQ_KEY_IMPORT_FLAG))
            .count();
        if import_arguments == 0 {
            return TestGroqKeyImportInvocation::NotRequested;
        }
        if import_arguments == 1
            && self.argv.len() == 2
            && self.argv[1] == TEST_GROQ_KEY_IMPORT_FLAG
        {
            TestGroqKeyImportInvocation::Import
        } else {
            TestGroqKeyImportInvocation::InvalidInvocation
        }
    }

    #[cfg(feature = "research-harness")]
    pub(crate) fn test_groq_key_status(&self) -> TestGroqKeyStatusInvocation {
        let matches = self
            .argv
            .iter()
            .skip(1)
            .filter(|argument| argument.starts_with(TEST_GROQ_KEY_STATUS_FLAG))
            .count();
        if matches == 0 {
            return TestGroqKeyStatusInvocation::NotRequested;
        }
        if matches == 1 && self.argv.len() == 2 && self.argv[1] == TEST_GROQ_KEY_STATUS_FLAG {
            TestGroqKeyStatusInvocation::Status
        } else {
            TestGroqKeyStatusInvocation::InvalidInvocation
        }
    }

    pub(crate) fn owned_wav_replay(&self) -> OwnedWavReplayInvocation {
        parse_owned_wav_replay(&self.argv)
    }
}

/// Run the standalone, research-only learning stdio binary. This narrow bridge
/// intentionally does not initialize Tauri or reuse normal app settings.
#[cfg(feature = "research-harness")]
pub fn run_learning_harness() -> i32 {
    crate::learning_research::run_standalone_stdio(std::env::args_os().skip(1))
}

pub(crate) async fn dispatch_remote_cli_action(
    app: &AppHandle,
    state: &crate::AppState,
    action: RemoteCliAction,
) {
    match action {
        RemoteCliAction::Cancel => dictation::cancel_internal(app, state).await,
        RemoteCliAction::ToggleVerbatim => dictation::handle_cli_toggle_verbatim(app, state).await,
        RemoteCliAction::ToggleTranscription => dictation::handle_hotkey_toggle(app, state).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_empty_returns_none() {
        assert_eq!(parse_cli(&[] as &[&str]), None);
    }

    #[test]
    fn parse_cli_unknown_args_returns_none() {
        assert_eq!(parse_cli(&["voiceflow", "--help", "-v"]), None);
    }

    #[test]
    fn parse_cli_toggle_transcription() {
        assert_eq!(
            parse_cli(&["voiceflow", "--toggle-transcription"]),
            Some(RemoteCliAction::ToggleTranscription)
        );
    }

    #[test]
    fn parse_cli_toggle_verbatim() {
        assert_eq!(
            parse_cli(&["voiceflow", "--toggle-verbatim"]),
            Some(RemoteCliAction::ToggleVerbatim)
        );
    }

    #[test]
    fn parse_cli_cancel() {
        assert_eq!(
            parse_cli(&["voiceflow", "--cancel"]),
            Some(RemoteCliAction::Cancel)
        );
    }

    #[test]
    fn parse_cli_ignores_unknown_args_with_flags() {
        assert_eq!(
            parse_cli(&["voiceflow", "--foo", "--toggle-transcription", "--bar"]),
            Some(RemoteCliAction::ToggleTranscription)
        );
    }

    #[test]
    fn parse_cli_priority_cancel_over_verbatim_and_transcription() {
        assert_eq!(
            parse_cli(&[
                "voiceflow",
                "--toggle-transcription",
                "--toggle-verbatim",
                "--cancel",
            ]),
            Some(RemoteCliAction::Cancel)
        );
    }

    #[test]
    fn parse_cli_priority_verbatim_over_transcription() {
        assert_eq!(
            parse_cli(&["voiceflow", "--toggle-transcription", "--toggle-verbatim"]),
            Some(RemoteCliAction::ToggleVerbatim)
        );
    }

    #[test]
    fn flag_to_action_mapping_table() {
        let cases = [
            ("--cancel", RemoteCliAction::Cancel),
            ("--toggle-verbatim", RemoteCliAction::ToggleVerbatim),
            (
                "--toggle-transcription",
                RemoteCliAction::ToggleTranscription,
            ),
        ];
        for (flag, expected) in cases {
            assert_eq!(
                parse_cli(&["voiceflow", flag]),
                Some(expected),
                "flag {flag}"
            );
        }
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    #[test]
    fn cold_actions_and_login_are_hidden_manual_open_is_visible() {
        for flag in [
            "--cancel",
            "--toggle-transcription",
            "--toggle-verbatim",
            "--background",
        ] {
            assert!(!CliArgs {
                argv: vec!["voiceflow".into(), flag.into()]
            }
            .show_settings());
        }
        assert!(CliArgs {
            argv: vec!["voiceflow".into()]
        }
        .show_settings());
    }

    #[cfg(feature = "research-harness")]
    #[test]
    fn test_groq_key_import_accepts_only_the_single_fixed_flag() {
        let flag = TEST_GROQ_KEY_IMPORT_FLAG;
        assert_eq!(
            CliArgs {
                argv: vec!["voiceflow".into(), flag.into()]
            }
            .test_groq_key_import(),
            TestGroqKeyImportInvocation::Import
        );
        for argv in [
            vec!["voiceflow".into(), flag.into(), "--cancel".into()],
            vec!["voiceflow".into(), flag.into(), "secret-value".into()],
            vec!["voiceflow".into(), flag.into(), flag.into()],
            vec!["voiceflow".into(), format!("{flag}=secret-value")],
        ] {
            assert_eq!(
                CliArgs { argv }.test_groq_key_import(),
                TestGroqKeyImportInvocation::InvalidInvocation
            );
        }
        assert_eq!(
            CliArgs {
                argv: vec!["voiceflow".into(), "--background".into()]
            }
            .test_groq_key_import(),
            TestGroqKeyImportInvocation::NotRequested
        );
    }

    #[cfg(feature = "research-harness")]
    #[test]
    fn test_groq_key_status_is_a_single_fixed_read_only_invocation() {
        let flag = TEST_GROQ_KEY_STATUS_FLAG;
        assert_eq!(
            CliArgs {
                argv: vec!["voiceflow".into(), flag.into()]
            }
            .test_groq_key_status(),
            TestGroqKeyStatusInvocation::Status
        );
        for argv in [
            vec!["voiceflow".into(), flag.into(), "--cancel".into()],
            vec!["voiceflow".into(), flag.into(), flag.into()],
            vec!["voiceflow".into(), format!("{flag}=anything")],
            vec![
                "voiceflow".into(),
                flag.into(),
                TEST_GROQ_KEY_IMPORT_FLAG.into(),
            ],
        ] {
            assert_eq!(
                CliArgs { argv }.test_groq_key_status(),
                TestGroqKeyStatusInvocation::InvalidInvocation
            );
        }
        assert_eq!(
            CliArgs {
                argv: vec!["voiceflow".into(), "--background".into()]
            }
            .test_groq_key_status(),
            TestGroqKeyStatusInvocation::NotRequested
        );
    }

    #[test]
    fn single_instance_callback_admission_rejects_reserved_arguments_before_routes() {
        let cases = vec![
            vec!["voiceflow", "--test-groq-key-status-v1"],
            vec!["--test-groq-key-status-v1"],
            vec!["voiceflow", "--import-test-groq-key-stdin-v2"],
            vec!["--import-test-groq-key-stdin-v2", "--cancel"],
            vec!["voiceflow", "--test-groq-key-status-v1=unexpected"],
            vec!["voiceflow", "--test-groq-key-status"],
            vec!["voiceflow", "--import-test-groq-key-stdin-v1"],
            vec!["voiceflow", "--import-test-groq-key-anything"],
            vec![
                "voiceflow",
                "--test-groq-key-status-v1",
                "--test-groq-key-status-v1",
            ],
            vec!["voiceflow", "--cancel", "--test-groq-key-status-v1"],
            vec!["voiceflow", "--test-groq-key-status-v1", "--cancel"],
            vec![
                "voiceflow",
                "--background",
                "--import-test-groq-key-stdin-v2",
            ],
            vec![
                "voiceflow",
                "--import-test-groq-key-stdin-v2",
                "--background",
            ],
            vec![
                "voiceflow",
                "--research-owned-wav",
                "/tmp/owned.wav",
                "--target-nonce",
                "f45d5da3-fe13-445c-b105-acf6a1894c64",
                "--test-groq-key-status-v1",
            ],
            vec![
                "voiceflow",
                "--test-groq-key-status-v1",
                "--research-owned-wav",
                "/tmp/owned.wav",
                "--target-nonce",
                "f45d5da3-fe13-445c-b105-acf6a1894c64",
            ],
        ];

        for argv in cases {
            assert_eq!(
                admit_single_instance_callback_args(&argv),
                SingleInstanceCallbackAdmission::RejectTestCredentialArguments,
                "{argv:?}"
            );
        }
    }

    #[test]
    fn single_instance_callback_admission_preserves_ordinary_routes() {
        let cancel = ["voiceflow", "--cancel"];
        assert_eq!(
            admit_single_instance_callback_args(&cancel),
            SingleInstanceCallbackAdmission::Continue
        );
        assert_eq!(parse_cli(&cancel), Some(RemoteCliAction::Cancel));

        let background = ["voiceflow", "--background"];
        assert_eq!(
            admit_single_instance_callback_args(&background),
            SingleInstanceCallbackAdmission::Continue
        );
        assert!(!CliArgs {
            argv: background
                .iter()
                .map(|argument| (*argument).into())
                .collect(),
        }
        .show_settings());

        let replay = [
            "voiceflow",
            "--research-owned-wav",
            "/tmp/owned.wav",
            "--target-nonce",
            "f45d5da3-fe13-445c-b105-acf6a1894c64",
        ];
        assert_eq!(
            admit_single_instance_callback_args(&replay),
            SingleInstanceCallbackAdmission::Continue
        );
        assert!(matches!(
            parse_owned_wav_replay(&replay),
            OwnedWavReplayInvocation::Replay { .. }
        ));

        let ordinary = ["voiceflow"];
        assert_eq!(
            admit_single_instance_callback_args(&ordinary),
            SingleInstanceCallbackAdmission::Continue
        );
        assert!(CliArgs {
            argv: ordinary.iter().map(|argument| (*argument).into()).collect(),
        }
        .show_settings());
    }

    #[cfg(not(feature = "research-harness"))]
    #[test]
    fn default_build_rejects_test_credential_flags_without_showing_settings() {
        for flag in [
            "--import-test-groq-key-stdin-v2",
            "--import-test-groq-key-stdin-v2=secret-value",
            "--import-test-groq-key-stdin",
            "--test-groq-key-status-v1",
            "--test-groq-key-status",
            "--test-groq-key-status-v1=anything",
        ] {
            let args = CliArgs {
                argv: vec!["voiceflow".into(), flag.into()],
            };
            assert!(args.unsupported_test_credential_cli_requested(), "{flag}");
            assert!(args.test_credential_cli_requested(), "{flag}");
            assert!(!args.show_settings(), "{flag}");
        }
    }

    #[cfg(feature = "research-harness")]
    #[test]
    fn research_harness_test_credential_flags_are_owned_by_the_cli() {
        for flag in [TEST_GROQ_KEY_IMPORT_FLAG, TEST_GROQ_KEY_STATUS_FLAG] {
            let args = CliArgs {
                argv: vec!["voiceflow".into(), flag.into()],
            };
            assert!(args.test_credential_cli_requested(), "{flag}");
            assert!(!args.show_settings(), "{flag}");
        }
    }

    #[test]
    fn owned_wav_replay_requires_fixed_flags_absolute_path_and_uuid_nonce() {
        let nonce = "f45d5da3-fe13-445c-b105-acf6a1894c64";
        assert_eq!(
            parse_owned_wav_replay(&[
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
            ]),
            OwnedWavReplayInvocation::Replay {
                path: "/tmp/owned.wav".into(),
                target_nonce: nonce.into(),
                route: OwnedWavReplayRoute::NativeNative,
            }
        );
        assert_eq!(
            parse_owned_wav_replay(&[
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
            ]),
            OwnedWavReplayInvocation::Replay {
                path: "/tmp/owned.wav".into(),
                target_nonce: nonce.into(),
                route: OwnedWavReplayRoute::NativeNative,
            }
        );
        for (route, expected) in [
            ("native-native", OwnedWavReplayRoute::NativeNative),
            ("native-groq", OwnedWavReplayRoute::NativeGroq),
            ("groq-native", OwnedWavReplayRoute::GroqNative),
            ("groq-groq", OwnedWavReplayRoute::GroqGroq),
        ] {
            assert_eq!(
                parse_owned_wav_replay(&[
                    "voiceflow",
                    OWNED_WAV_REPLAY_FLAG,
                    "/tmp/owned.wav",
                    OWNED_WAV_TARGET_NONCE_FLAG,
                    nonce,
                    OWNED_WAV_ROUTE_FLAG,
                    route,
                ]),
                OwnedWavReplayInvocation::Replay {
                    path: "/tmp/owned.wav".into(),
                    target_nonce: nonce.into(),
                    route: expected,
                }
            );
        }
        for argv in [
            vec!["voiceflow", OWNED_WAV_REPLAY_FLAG],
            vec!["voiceflow", OWNED_WAV_REPLAY_FLAG, "/tmp/owned.wav"],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
            ],
            vec![OWNED_WAV_REPLAY_FLAG],
            vec![OWNED_WAV_REPLAY_FLAG, "/tmp/owned.wav"],
            vec![
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
            ],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "relative.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
            ],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                "not-a-uuid",
            ],
            vec!["voiceflow", "--research-owned-wav=/tmp/owned.wav"],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
                OWNED_WAV_ROUTE_FLAG,
            ],
            vec![
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
                OWNED_WAV_ROUTE_FLAG,
            ],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
                OWNED_WAV_ROUTE_FLAG,
                "lmstudio-groq",
            ],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
                OWNED_WAV_ROUTE_FLAG,
                "native-groq",
                OWNED_WAV_ROUTE_FLAG,
                "native-groq",
            ],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                OWNED_WAV_TARGET_NONCE_FLAG,
                nonce,
                "--toggle-transcription",
            ],
            vec![
                "voiceflow",
                OWNED_WAV_REPLAY_FLAG,
                "/tmp/owned.wav",
                "--target-nonce=secret",
                nonce,
            ],
        ] {
            assert_eq!(
                parse_owned_wav_replay(&argv),
                OwnedWavReplayInvocation::InvalidInvocation
            );
        }
        assert_eq!(
            parse_owned_wav_replay(&["voiceflow", "--toggle-transcription"]),
            OwnedWavReplayInvocation::NotRequested
        );
        assert!(!is_canonical_uuid("f45d5da3-fe13-445c-b105-acf6a1894c6Z"));
    }

    #[test]
    fn owned_wav_route_requires_the_exact_configured_route_and_groq_identity() {
        use crate::engine::EngineProvider::{Groq, OnDevice, OnDeviceMlx};

        fn settings() -> crate::store::Settings {
            crate::store::Settings {
                cleanup_enabled: true,
                strict_offline_enabled: false,
                asr_provider: OnDevice,
                asr_model: crate::ondevice_models::QWEN_ASR_06_ID.to_owned(),
                cleanup_provider: OnDeviceMlx,
                cleanup_model: crate::modelstore::MODEL_ID.to_owned(),
                ..crate::store::Settings::default()
            }
        }

        fn use_groq_asr(settings: &mut crate::store::Settings) {
            settings.asr_provider = Groq;
            settings.asr_model = crate::asr::MODEL.to_owned();
            settings.api_key = "test-only-key".into();
        }

        fn use_groq_cleanup(settings: &mut crate::store::Settings) {
            settings.cleanup_provider = Groq;
            settings.cleanup_model = crate::llm::MODEL.to_owned();
            settings.api_key = "test-only-key".into();
        }

        let native = settings();
        assert!(OwnedWavReplayRoute::NativeNative.matches_settings(&native));
        assert!(!OwnedWavReplayRoute::NativeGroq.matches_settings(&native));

        let mut native_groq = settings();
        use_groq_cleanup(&mut native_groq);
        assert!(OwnedWavReplayRoute::NativeGroq.matches_settings(&native_groq));
        native_groq.strict_offline_enabled = true;
        assert!(!OwnedWavReplayRoute::NativeGroq.matches_settings(&native_groq));

        let mut groq_native = settings();
        use_groq_asr(&mut groq_native);
        assert!(OwnedWavReplayRoute::GroqNative.matches_settings(&groq_native));
        groq_native.asr_model = "whisper-large-v3".into();
        assert!(!OwnedWavReplayRoute::GroqNative.matches_settings(&groq_native));

        let mut groq_groq = settings();
        use_groq_asr(&mut groq_groq);
        use_groq_cleanup(&mut groq_groq);
        assert!(OwnedWavReplayRoute::GroqGroq.matches_settings(&groq_groq));
        groq_groq.api_key.clear();
        assert!(!OwnedWavReplayRoute::GroqGroq.matches_settings(&groq_groq));
        groq_groq.api_key = "test-only-key".into();
        groq_groq.cleanup_model = "openai/gpt-oss-120b".into();
        assert!(!OwnedWavReplayRoute::GroqGroq.matches_settings(&groq_groq));
    }
}
