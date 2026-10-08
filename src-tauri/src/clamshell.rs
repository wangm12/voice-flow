//! Bounded, whitespace-tolerant macOS lid-state detection.
pub fn resolve_input_device(selected: &str, clamshell: &str, closed: bool) -> String {
    if closed && !clamshell.trim().is_empty() {
        clamshell.trim().to_owned()
    } else {
        selected.trim().to_owned()
    }
}
fn parse_lid_state(output: &str) -> bool {
    output.lines().any(|line| {
        line.split_once('=').is_some_and(|(key, value)| {
            key.trim().ends_with("\"AppleClamshellState\"")
                && matches!(value.trim(), "Yes" | "true")
        })
    })
}
#[cfg(target_os = "macos")]
pub fn lid_closed() -> bool {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let Ok(mut child) = Command::new("ioreg")
        .args(["-r", "-k", "AppleClamshellState", "-d", "4"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return child
                    .wait_with_output()
                    .ok()
                    .is_some_and(|out| parse_lid_state(&String::from_utf8_lossy(&out.stdout)))
            }
            Ok(Some(_)) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub fn lid_closed() -> bool {
    false
}
#[cfg(test)]
mod tests {
    #[test]
    fn parses_real_ioreg_whitespace() {
        for text in [
            "| \"AppleClamshellState\" = Yes",
            "\"AppleClamshellState\"=Yes",
            "\"AppleClamshellState\" = true",
        ] {
            assert!(super::parse_lid_state(text));
        }
        for text in ["\"AppleClamshellState\" = No", "bad", "\"Other\" = Yes"] {
            assert!(!super::parse_lid_state(text));
        }
        assert_eq!(
            super::resolve_input_device(" Built-in ", " USB ", true),
            "USB"
        );
        assert_eq!(
            super::resolve_input_device(" Built-in ", "", true),
            "Built-in"
        );
    }
}
