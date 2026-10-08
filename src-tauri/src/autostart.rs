//! Checked OS registration with rollback until the serialized settings save succeeds.
use tauri_plugin_autostart::ManagerExt;

#[derive(serde::Serialize)]
pub struct Status {
    pub enabled: bool,
    pub error: Option<String>,
}

#[tauri::command]
pub fn get_autostart_status(app: tauri::AppHandle) -> Status {
    match app.autolaunch().is_enabled() {
        Ok(enabled) => Status {
            enabled,
            error: None,
        },
        Err(error) => Status {
            enabled: false,
            error: Some(error.to_string()),
        },
    }
}

pub(crate) trait Registration: Clone {
    fn is_enabled(&self) -> Result<bool, String>;
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
}

#[derive(Clone)]
pub(crate) struct AppRegistration(tauri::AppHandle);
impl Registration for AppRegistration {
    fn is_enabled(&self) -> Result<bool, String> {
        self.0.autolaunch().is_enabled().map_err(|e| e.to_string())
    }
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let manager = self.0.autolaunch();
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|e| e.to_string())
    }
}

fn apply(registration: &impl Registration, enabled: bool) -> Result<(), String> {
    if registration.is_enabled()? != enabled {
        registration.set_enabled(enabled)?;
    }
    if registration.is_enabled()? != enabled {
        return Err("macOS did not apply the login startup setting".into());
    }
    Ok(())
}

pub(crate) struct Change<S: Registration = AppRegistration> {
    registration: S,
    previous: bool,
    committed: bool,
}
impl Change<AppRegistration> {
    pub fn for_settings(
        app: &tauri::AppHandle,
        previous_saved: bool,
        target: bool,
        explicitly_requested: bool,
    ) -> Result<Option<Self>, String> {
        Self::for_settings_with(
            AppRegistration(app.clone()),
            previous_saved,
            target,
            explicitly_requested,
        )
    }
}
impl<S: Registration> Change<S> {
    fn for_settings_with(
        registration: S,
        previous_saved: bool,
        target: bool,
        explicitly_requested: bool,
    ) -> Result<Option<Self>, String> {
        if explicitly_requested || previous_saved != target {
            Self::begin_with(registration, target).map(Some)
        } else {
            Ok(None)
        }
    }
    fn begin_with(registration: S, enabled: bool) -> Result<Self, String> {
        let previous = registration.is_enabled()?;
        // Construct before applying, so partial OS failure is rolled back too.
        let change = Self {
            registration,
            previous,
            committed: false,
        };
        apply(&change.registration, enabled)?;
        Ok(change)
    }
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl<S: Registration> Drop for Change<S> {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = apply(&self.registration, self.previous) {
                log::warn!("login startup rollback failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};
    #[derive(Clone, Default)]
    struct Fake {
        enabled: Rc<Cell<bool>>,
        fail_enable: bool,
        ignore_change: bool,
    }
    impl Registration for Fake {
        fn is_enabled(&self) -> Result<bool, String> {
            Ok(self.enabled.get())
        }
        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            if !self.ignore_change {
                self.enabled.set(enabled);
            }
            if enabled && self.fail_enable {
                Err("system registration failed".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn failed_save_rolls_back_but_commit_retains_os_change() {
        let registration = Fake::default();
        {
            let _change = Change::begin_with(registration.clone(), true).unwrap();
            assert!(registration.enabled.get());
        }
        assert!(!registration.enabled.get());
        {
            let mut change = Change::begin_with(registration.clone(), true).unwrap();
            change.commit();
        }
        assert!(registration.enabled.get());
    }
    #[test]
    fn partially_applied_system_error_is_reported_and_rolled_back() {
        let registration = Fake {
            fail_enable: true,
            ..Default::default()
        };
        assert!(Change::begin_with(registration.clone(), true).is_err());
        assert!(!registration.enabled.get());
    }
    #[test]
    fn successful_call_with_wrong_system_state_is_not_success() {
        let registration = Fake {
            ignore_change: true,
            ..Default::default()
        };
        assert!(Change::begin_with(registration, true).is_err());
    }

    #[test]
    fn explicit_unchanged_setting_reconciles_both_os_mismatch_directions() {
        for saved in [false, true] {
            let registration = Fake::default();
            registration.enabled.set(!saved);
            {
                let _change = Change::for_settings_with(registration.clone(), saved, saved, true)
                    .unwrap()
                    .unwrap();
                assert_eq!(registration.enabled.get(), saved);
            }
            // A failed persistence still restores the original OS state.
            assert_eq!(registration.enabled.get(), !saved);
            let mut change = Change::for_settings_with(registration.clone(), saved, saved, true)
                .unwrap()
                .unwrap();
            change.commit();
            drop(change);
            assert_eq!(registration.enabled.get(), saved);
        }
    }

    #[test]
    fn unrelated_settings_patch_preserves_external_os_choice() {
        let registration = Fake::default();
        registration.enabled.set(true);
        let change = Change::for_settings_with(registration.clone(), false, false, false).unwrap();
        assert!(change.is_none());
        assert!(registration.enabled.get());
    }
}
