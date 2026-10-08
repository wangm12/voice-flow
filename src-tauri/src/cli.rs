use crate::dictation;
use tauri::AppHandle;

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
        !self.background() && self.remote_action().is_none()
    }

    pub fn remote_action(&self) -> Option<RemoteCliAction> {
        parse_cli(&self.argv)
    }
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
}
