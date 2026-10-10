use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use crate::settings::Settings;
use crate::tray::{self, TrayState};

#[derive(Default)]
struct WindowState {
    auto_hidden: bool,
}

impl WindowState {
    fn update(
        &mut self,
        running_work: bool,
        enabled: bool,
        tray_available: bool,
        mut set_hidden: impl FnMut(bool) -> Result<(), String>,
    ) -> Result<(), String> {
        let hide = running_work && enabled && tray_available;
        if hide || self.auto_hidden {
            set_hidden(hide)?;
            self.auto_hidden = hide;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct WorkWindow {
    app: AppHandle,
    settings: Arc<Mutex<Settings>>,
    tray: Arc<TrayState>,
    state: Arc<Mutex<WindowState>>,
}

impl WorkWindow {
    pub fn new(app: AppHandle, settings: Arc<Mutex<Settings>>, tray: Arc<TrayState>) -> Self {
        Self {
            app,
            settings,
            tray,
            state: Arc::new(Mutex::new(WindowState::default())),
        }
    }

    pub fn set_running(&self, running_work: bool) {
        self.dispatch(running_work, false);
    }

    pub fn remove_tray(&self) {
        self.dispatch(false, true);
    }

    fn dispatch(&self, running_work: bool, remove_tray: bool) {
        let this = self.clone();
        // Serialize window operations on the UI thread, without holding a timer
        // lock while waiting for that thread to service a native window call.
        if let Err(e) = self.app.run_on_main_thread(move || {
            let enabled = {
                let settings = this.settings.lock().unwrap();
                if remove_tray && (settings.tray_icon_enabled || settings.min_to_tray) {
                    return;
                }
                settings.auto_hide_during_work && settings.tray_icon_enabled
            };
            let available = this.tray.is_available();
            if running_work && enabled && !available {
                log::warn!("[window] cannot auto-hide: system tray is unavailable");
            }
            let result = this.state.lock().unwrap().update(running_work, enabled, available, |hide| {
                if hide {
                    let window = this.app.get_webview_window("main")
                        .ok_or_else(|| "main window not found".to_string())?;
                    window.hide().map_err(|e| e.to_string())
                } else {
                    tray::restore_main_window(&this.app)
                }
            });
            if let Err(e) = result {
                log::error!("[window] failed to update work timer visibility: {e}");
                return;
            }
            if remove_tray {
                if let Err(e) = tray::destroy_tray(&this.tray) {
                    log::error!("[tray] failed to hide tray icon: {e}");
                }
            }
        }) {
            log::error!("[window] failed to schedule work timer visibility: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_transitions_hide_and_restore_without_touching_idle_windows() {
        let mut state = WindowState::default();
        let mut actions = Vec::new();
        let mut apply = |hide| {
            actions.push(hide);
            Ok(())
        };
        state.update(false, true, true, &mut apply).unwrap();
        state.update(true, true, true, &mut apply).unwrap();
        state.update(false, true, true, &mut apply).unwrap();
        state.update(true, true, true, &mut apply).unwrap();
        state.update(false, true, true, &mut apply).unwrap();
        state.update(false, true, true, &mut apply).unwrap();
        assert_eq!(actions, [true, false, true, false]);
        assert!(!state.auto_hidden);
    }

    #[test]
    fn resume_hides_again_after_manual_restore() {
        let mut state = WindowState::default();
        let mut actions = Vec::new();
        let mut apply = |hide| {
            actions.push(hide);
            Ok(())
        };
        state.update(true, true, true, &mut apply).unwrap();
        // Manual Show does not trigger an update; a subsequent work start does.
        state.update(true, true, true, &mut apply).unwrap();
        state.update(false, true, true, &mut apply).unwrap();
        assert_eq!(actions, [true, true, false]);
    }

    #[test]
    fn failed_actions_preserve_state_for_recovery() {
        let mut state = WindowState::default();
        assert!(state.update(true, true, true, |_| Err("hide failed".into())).is_err());
        assert!(!state.auto_hidden);
        state.update(true, true, true, |_| Ok(())).unwrap();
        assert!(state.update(false, true, true, |_| Err("restore failed".into())).is_err());
        assert!(state.auto_hidden);
        state.update(false, true, true, |_| Ok(())).unwrap();
        assert!(!state.auto_hidden);
    }

    #[test]
    fn disabled_setting_or_unavailable_tray_never_hides_and_restores_owned_window() {
        for (enabled, available) in [(false, true), (true, false), (false, false)] {
            let mut state = WindowState::default();
            state.update(true, enabled, available, |_| panic!("must not hide")).unwrap();
            state.update(true, true, true, |_| Ok(())).unwrap();
            let mut actions = Vec::new();
            state.update(true, enabled, available, |hide| {
                actions.push(hide);
                Ok(())
            }).unwrap();
            assert_eq!(actions, [false]);
            assert!(!state.auto_hidden);
        }
    }
}
