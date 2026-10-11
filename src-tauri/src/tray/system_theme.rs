use std::sync::{atomic::Ordering, Arc};

use tauri::AppHandle;
#[cfg(not(target_os = "windows"))]
use tauri::Manager;

use super::TrayState;

pub(super) fn initialize(app: &AppHandle, state: &Arc<TrayState>) {
    #[cfg(not(target_os = "windows"))]
    if let Some(window) = app.get_webview_window("main") {
        match window.theme() {
            Ok(theme) => state.system_dark.store(theme == tauri::Theme::Dark, Ordering::Release),
            Err(e) => log::warn!("[tray] could not read system theme: {e}"),
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows allows a light app theme with a dark taskbar (and vice versa).
        // Watch the shell preference rather than the webview's app preference.
        use windows_sys::Win32::System::Registry::{RegNotifyChangeKeyValue, REG_NOTIFY_CHANGE_LAST_SET};
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};

        let key = match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        {
            Ok(key) => key,
            Err(e) => {
                log::warn!("[tray] could not open taskbar theme preference: {e}");
                return;
            }
        };
        let read_theme = |key: &RegKey, state: &TrayState| {
            match key.get_value::<u32, _>("SystemUsesLightTheme") {
                Ok(light) => state.system_dark.store(light == 0, Ordering::Release),
                Err(e) => log::warn!("[tray] could not read taskbar theme; retaining previous color: {e}"),
            }
        };
        read_theme(&key, state);
        let app = app.clone();
        let state = Arc::clone(state);
        if let Err(e) = std::thread::Builder::new().name("tray-theme".into()).spawn(move || {
            loop {
                // SAFETY: key owns a valid registry handle for this entire thread.
                // Synchronous notification does not use an event handle.
                let result = unsafe {
                    RegNotifyChangeKeyValue(key.raw_handle(), 0, REG_NOTIFY_CHANGE_LAST_SET, std::ptr::null_mut(), 0)
                };
                if result != 0 {
                    log::warn!("[tray] taskbar theme watcher stopped: {result}");
                    break;
                }
                read_theme(&key, &state);
                super::refresh(&app, &state);
            }
        }) {
            log::error!("[tray] could not start taskbar theme watcher: {e}");
        }
    }
}

pub fn changed(app: &AppHandle, state: &Arc<TrayState>, theme: tauri::Theme) {
    #[cfg(not(target_os = "windows"))]
    state.system_dark.store(theme == tauri::Theme::Dark, Ordering::Release);
    #[cfg(target_os = "windows")]
    let _ = theme;
    super::refresh(app, state);
}
