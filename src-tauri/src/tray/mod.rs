/// System tray management with a progress ring or fixed-width countdown digits.
/// Ring colors follow the app theme; digits follow the system tray appearance.
///
/// ## Linux / Wayland note
/// Tauri's tray support on Linux is backed by `libappindicator-sys`, which
/// `dlopen`s `libayatana-appindicator3` or `libappindicator3` at runtime and
/// panics (aborting the process) if neither is found.  KDE Plasma 6 on Wayland
/// typically does not ship these libraries.  `create_tray` probes for them
/// before calling `TrayIconBuilder::build()` and returns early with a warning
/// when they are absent, preventing the abort.
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::timer::TimerController;
use crate::timer::TimerSnapshot;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

mod display;
pub mod system_theme;

// ---------------------------------------------------------------------------
// Theme colors for tray rendering
// ---------------------------------------------------------------------------

/// Color tokens needed for tray icon rendering.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayColors {
    pub background: [u8; 4],
    pub focus_round: [u8; 4],
    pub short_round: [u8; 4],
    pub long_round: [u8; 4],
    pub foreground: [u8; 4],
}

impl Default for TrayColors {
    fn default() -> Self {
        // Pomotroid theme defaults (matches pomotroid.json bundled theme).
        Self {
            background: [47, 56, 75, 255],    // #2F384B
            focus_round: [226, 93, 96, 255],  // #E25D60
            short_round: [53, 188, 174, 255], // #35BCAE
            long_round: [89, 174, 209, 255],  // #59AED1
            foreground: [255, 255, 255, 255], // #FFFFFF
        }
    }
}

impl TrayColors {
    /// Build a `TrayColors` from a theme's CSS-variable color map.
    /// Falls back to `TrayColors::default()` values for any key that is
    /// missing or unparseable.
    pub fn from_colors_map(colors: &std::collections::HashMap<String, String>) -> Self {
        let d = Self::default();
        let get = |key: &str, fallback: [u8; 4]| {
            colors.get(key)
                .and_then(|hex| parse_hex_color(hex))
                .unwrap_or(fallback)
        };
        Self {
            background: get("--color-background", d.background),
            focus_round: get("--color-focus-round", d.focus_round),
            short_round: get("--color-short-round", d.short_round),
            long_round:  get("--color-long-round",  d.long_round),
            foreground:  get("--color-foreground",   d.foreground),
        }
    }
}

/// Parse a CSS hex color (#RRGGBB or #RRGGBBAA) into [r, g, b, a].
pub fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    let h = hex.strip_prefix('#')?;
    match h.len() {
        6 => {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            Some([r, g, b, 255])
        }
        8 => {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            let a = u8::from_str_radix(&h[6..8], 16).ok()?;
            Some([r, g, b, a])
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared tray state
// ---------------------------------------------------------------------------

/// Handles to the dynamic timer-control menu items.
/// Stored in `TrayState` so the timer event thread can update labels/enabled states.
#[derive(Clone)]
pub struct TrayMenuItems {
    pub toggle: MenuItem<tauri::Wry>,
    pub skip: MenuItem<tauri::Wry>,
    pub reset_round: MenuItem<tauri::Wry>,
}

/// Tauri-managed state for the tray icon (uses the default Wry runtime).
pub struct TrayState {
    pub icon: Mutex<Option<TrayIcon<tauri::Wry>>>,
    pub colors: Mutex<TrayColors>,
    pub countdown_mode: Mutex<bool>,
    pub menu_items: Mutex<Option<TrayMenuItems>>,
    available: AtomicBool,
    pub show_time: AtomicBool,
    system_dark: AtomicBool,
    timer: Mutex<Option<TimerSnapshot>>,
    rendered: Mutex<Option<display::IconKey>>,
    last_tooltip: Mutex<String>,
}

impl TrayState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            icon: Mutex::new(None),
            colors: Mutex::new(TrayColors::default()),
            countdown_mode: Mutex::new(false),
            menu_items: Mutex::new(None),
            available: AtomicBool::new(false),
            show_time: AtomicBool::new(false),
            system_dark: AtomicBool::new(true),
            timer: Mutex::new(None),
            rendered: Mutex::new(None),
            last_tooltip: Mutex::new(String::new()),
        })
    }

    pub fn initialize(app: &AppHandle, state: &Arc<TrayState>, snapshot: TimerSnapshot, show_time: bool) {
        *state.timer.lock().unwrap() = Some(snapshot);
        state.show_time.store(show_time, Ordering::Release);
        system_theme::initialize(app, state);
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayWindowAction {
    Hide,
    Restore,
}

fn tray_window_action(is_visible: bool, is_minimized: bool) -> TrayWindowAction {
    if is_visible && !is_minimized {
        TrayWindowAction::Hide
    } else {
        TrayWindowAction::Restore
    }
}

// ---------------------------------------------------------------------------
// Tray lifecycle
// ---------------------------------------------------------------------------

/// Check whether the appindicator shared library is loadable on this system.
///
/// `libappindicator-sys` panics with an unrecoverable abort when none of the
/// four candidate `.so` names can be opened.  We probe via `dlopen` first so
/// `create_tray` can return gracefully instead of crashing the process.
///
/// KDE Plasma 6 on Wayland typically does not ship these libraries.
#[cfg(target_os = "linux")]
pub(crate) fn appindicator_available() -> bool {
    use std::ffi::c_char;
    extern "C" {
        fn dlopen(filename: *const c_char, flags: i32) -> *mut std::ffi::c_void;
        fn dlclose(handle: *mut std::ffi::c_void) -> i32;
    }
    const RTLD_LAZY: i32 = 0x0001;
    // Mirror the four names tried by libappindicator-sys 0.9.
    let candidates: &[&[u8]] = &[
        b"libayatana-appindicator3.so.1\0",
        b"libappindicator3.so.1\0",
        b"libayatana-appindicator3.so\0",
        b"libappindicator3.so\0",
    ];
    log::debug!("[tray] probing for appindicator shared library");
    let result = candidates.iter().find_map(|name| unsafe {
        let name_str = std::str::from_utf8(name)
            .unwrap_or("?")
            .trim_end_matches('\0');
        log::debug!("[tray] trying {name_str}");
        let handle = dlopen(name.as_ptr() as *const c_char, RTLD_LAZY);
        if !handle.is_null() {
            dlclose(handle);
            Some(name_str)
        } else {
            None
        }
    });
    match result {
        Some(found) => {
            log::info!("[tray] appindicator found: {found}");
            true
        }
        None => {
            log::warn!(
                "[tray] appindicator not found — checked: {}",
                candidates
                    .iter()
                    .map(|n| std::str::from_utf8(n).unwrap_or("?").trim_end_matches('\0'))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            false
        }
    }
}

/// Show the system tray icon.
///
/// If the icon has already been created (e.g. it was previously hidden), it is
/// made visible again without allocating a second OS icon.  A new icon is only
/// built on the very first call.
pub fn create_tray(app: &AppHandle, state: &Arc<TrayState>) {
    // Re-show existing icon if present — avoids duplicate OS tray entries.
    {
        let existing = state.icon.lock().unwrap().clone();
        if let Some(existing) = existing {
            if let Err(e) = existing.set_visible(true) {
                log::warn!("[tray] failed to show tray icon: {e}");
                return;
            }
            state.available.store(true, Ordering::Release);
            if let Some(timer) = state.timer.lock().unwrap().clone() {
                update_menu_items(state, timer.is_running, timer.is_paused);
            }
            refresh(app, state);
            log::info!("[tray] shown (reused existing icon)");
            return;
        }
    }

    // On Linux, libappindicator-sys aborts the process if neither
    // libayatana-appindicator3 nor libappindicator3 is installed (common on
    // KDE Plasma 6 / Wayland).  Bail out gracefully if the probe fails.
    #[cfg(target_os = "linux")]
    if !appindicator_available() {
        log::warn!(
            "[tray] libayatana-appindicator3 / libappindicator3 not found — \
             system tray is unavailable on this system. \
             Install libayatana-appindicator3 to enable it."
        );
        return;
    }

    let toggle_item = match MenuItem::with_id(app, "toggle", "Start", true, None::<&str>) {
        Ok(i) => i,
        Err(e) => { log::warn!("[tray] menu item error: {e}"); return; }
    };
    let skip_item = match MenuItem::with_id(app, "skip", "Skip", false, None::<&str>) {
        Ok(i) => i,
        Err(e) => { log::warn!("[tray] menu item error: {e}"); return; }
    };
    let reset_item = match MenuItem::with_id(app, "reset-round", "Reset Round", false, None::<&str>) {
        Ok(i) => i,
        Err(e) => { log::warn!("[tray] menu item error: {e}"); return; }
    };
    let sep = match PredefinedMenuItem::separator(app) {
        Ok(i) => i,
        Err(e) => { log::warn!("[tray] menu item error: {e}"); return; }
    };
    let show_item = match MenuItem::with_id(app, "show", "Show", true, None::<&str>) {
        Ok(i) => i,
        Err(e) => { log::warn!("[tray] menu item error: {e}"); return; }
    };
    let exit_item = match MenuItem::with_id(app, "exit", "Exit", true, None::<&str>) {
        Ok(i) => i,
        Err(e) => { log::warn!("[tray] menu item error: {e}"); return; }
    };
    let menu = match Menu::with_items(app, &[&toggle_item, &skip_item, &reset_item, &sep, &show_item, &exit_item]) {
        Ok(m) => m,
        Err(e) => { log::warn!("[tray] menu error: {e}"); return; }
    };

    let Some(snapshot) = state.timer.lock().unwrap().clone() else {
        log::error!("[tray] cannot create icon before timer initialization");
        return;
    };
    let key = icon_key(state, &snapshot);
    let image = Image::new_owned(key.render(), SIZE, SIZE);

    let tray = TrayIconBuilder::new()
        .icon(image)
        .icon_as_template(matches!(key, display::IconKey::Time { .. }))
        .tooltip(display::tooltip(&snapshot))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray_icon, event| {
            // Left-click: toggle window visibility.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray_icon.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let visible = window.is_visible().unwrap_or(false);
                    let minimized = window.is_minimized().unwrap_or(false);
                    match tray_window_action(visible, minimized) {
                        TrayWindowAction::Hide => {
                            log::debug!("[tray] left-click → hide");
                            let _ = window.hide();
                        }
                        TrayWindowAction::Restore => {
                            log::debug!("[tray] left-click → show");
                            if let Err(e) = restore_main_window(app) {
                                log::error!("[tray] failed to restore window: {e}");
                            }
                        }
                    }
                }
            }
        })
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "toggle" => {
                    if let Some(timer) = app.try_state::<TimerController>() {
                        timer.toggle();
                    }
                }
                "skip" => {
                    if let Some(timer) = app.try_state::<TimerController>() {
                        timer.skip();
                    }
                }
                "reset-round" => {
                    if let Some(timer) = app.try_state::<TimerController>() {
                        timer.restart_round();
                    }
                }
                "show" => {
                    log::info!("[tray] show");
                    if let Err(e) = restore_main_window(app) {
                        log::error!("[tray] failed to restore window: {e}");
                    }
                }
                "exit" => {
                    log::info!("[tray] exit");
                    app.exit(0);
                }
                _ => {}
            }
        })
        .build(app);

    match tray {
        Ok(t) => {
            *state.icon.lock().unwrap() = Some(t);
            *state.menu_items.lock().unwrap() = Some(TrayMenuItems {
                toggle: toggle_item,
                skip: skip_item,
                reset_round: reset_item,
            });
            state.available.store(true, Ordering::Release);
            if let Some(timer) = state.timer.lock().unwrap().clone() {
                update_menu_items(state, timer.is_running, timer.is_paused);
            }
            // Creation may have waited for the OS while the timer advanced.
            refresh(app, state);
            log::info!("[tray] created");
        }
        Err(e) => log::warn!("[tray] failed to build tray icon: {e}"),
    }
}

/// Hide the system tray icon.
///
/// The underlying `TrayIcon` is kept alive so it can be shown again without
/// allocating a second OS icon.  Dropping the handle is not sufficient to
/// remove the icon on all platforms; `set_visible(false)` is the reliable path.
pub fn destroy_tray(state: &Arc<TrayState>) -> Result<(), String> {
    let existing = state.icon.lock().unwrap().clone();
    if let Some(existing) = existing {
        existing.set_visible(false).map_err(|e| e.to_string())?;
        log::info!("[tray] hidden");
    }
    state.available.store(false, Ordering::Release);
    Ok(())
}

pub fn restore_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.unminimize().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Icon update (called from the timer event listener)
// ---------------------------------------------------------------------------

pub fn update_timer(app: &AppHandle, state: &Arc<TrayState>, snapshot: TimerSnapshot) {
    *state.timer.lock().unwrap() = Some(snapshot);
    refresh(app, state);
}

fn icon_key(state: &TrayState, timer: &TimerSnapshot) -> display::IconKey {
    display::IconKey::new(
        timer,
        state.show_time.load(Ordering::Acquire),
        state.system_dark.load(Ordering::Acquire),
        state.colors.lock().unwrap().clone(),
        *state.countdown_mode.lock().unwrap(),
    )
}

/// Redraw only when the visible glyphs/ring change; keep the tooltip exact every second.
pub fn refresh(app: &AppHandle, state: &Arc<TrayState>) {
    let state = Arc::clone(state);
    if let Err(e) = app.run_on_main_thread(move || {
        let Some(tray) = state.icon.lock().unwrap().clone() else { return };
        let Some(timer) = state.timer.lock().unwrap().clone() else { return };
        let key = icon_key(&state, &timer);
        let mut rendered = state.rendered.lock().unwrap();
        if rendered.as_ref() != Some(&key) {
            #[cfg(target_os = "macos")]
            if let Err(e) = tray.set_icon_as_template(matches!(key, display::IconKey::Time { .. })) {
                log::error!("[tray] could not set template rendering: {e}");
                return;
            }
            if let Err(e) = tray.set_icon(Some(Image::new_owned(key.render(), SIZE, SIZE))) {
                log::error!("[tray] could not update icon: {e}");
                return;
            }
            *rendered = Some(key);
        }
        drop(rendered);
        let tooltip = display::tooltip(&timer);
        let mut previous = state.last_tooltip.lock().unwrap();
        if *previous != tooltip {
            match tray.set_tooltip(Some(&tooltip)) {
                Ok(()) => *previous = tooltip,
                Err(e) => log::error!("[tray] could not update countdown tooltip: {e}"),
            }
        }
    }) {
        log::error!("[tray] could not schedule icon refresh: {e}");
    }
}

// ---------------------------------------------------------------------------
// Menu item update (called from the timer event listener)
// ---------------------------------------------------------------------------

/// Update the tray menu items to reflect the current timer state.
///
/// - `is_running`: timer is actively counting down.
/// - `is_paused`: timer has been started and then paused (elapsed > 0, not running).
///
/// No-op when the tray menu has not been created yet.
pub fn update_menu_items(state: &Arc<TrayState>, is_running: bool, is_paused: bool) {
    let items = state.menu_items.lock().unwrap().clone();
    let Some(items) = items else { return };

    let toggle_label = if is_running { "Pause" } else if is_paused { "Resume" } else { "Start" };
    let controls_enabled = is_running || is_paused;

    let _ = items.toggle.set_text(toggle_label);
    let _ = items.skip.set_enabled(controls_enabled);
    let _ = items.reset_round.set_enabled(controls_enabled);
}

// ---------------------------------------------------------------------------
// Icon rendering (tiny-skia)
// ---------------------------------------------------------------------------

// Render at 64×64 so the icon looks sharp on HiDPI displays (Ubuntu often
// runs at 1.5× or 2× scale).  The tray host scales it down on standard
// density displays; the larger source means the circle stays clean either way.
const SIZE: u32 = 64;
const CENTER: f32 = SIZE as f32 / 2.0;
const RADIUS: f32 = CENTER - 5.0; // 5 px margin keeps the stroke inside the canvas
const STROKE_WIDTH: f32 = 6.0;
// Track opacity: the "empty" part of the ring at this brightness on a dark panel.
// 22% white on #1a1a1a ≈ #383838 — invisible. 65% ≈ #a6a6a6 — clearly visible.
const TRACK_ALPHA: u8 = 165; // ≈ 65 %

fn rgba_color(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

/// Render a 64×64 RGBA tray icon as a **ring** with a progress arc.
///
/// Using a ring (stroke-only circle) on a transparent background means the
/// icon reads as a clear circle regardless of panel colour or scale factor,
/// unlike a solid filled disc which looks like a dark blob at small sizes.
pub fn render_tray_icon_rgba(
    colors: &TrayColors,
    paused: bool,
    progress: f32,
    round_type: &str,
    countdown: bool,
) -> Vec<u8> {
    let mut pixmap = Pixmap::new(SIZE, SIZE).expect("pixmap alloc");

    let mut paint = Paint { anti_alias: true, ..Default::default() };

    let stroke = Stroke {
        width: STROKE_WIDTH,
        line_cap: tiny_skia::LineCap::Round,
        ..Default::default()
    };

    // Track ring: full circle at low opacity — defines the circular shape.
    {
        let [r, g, b, _] = colors.foreground;
        paint.set_color(Color::from_rgba8(r, g, b, TRACK_ALPHA));
    }
    let ring = {
        let mut pb = PathBuilder::new();
        pb.push_circle(CENTER, CENTER, RADIUS);
        pb.finish().expect("ring path")
    };
    pixmap.stroke_path(&ring, &paint, &stroke, Transform::identity(), None);

    // Round-type color: used for both the progress arc and the pause bars.
    let round_color = match round_type {
        "short-break" => rgba_color(colors.short_round),
        "long-break"  => rgba_color(colors.long_round),
        _             => rgba_color(colors.focus_round),
    };

    // Progress arc from 12 o'clock, clockwise, in the round-type colour.
    paint.set_color(round_color);

    // In elapsed mode the arc grows as time passes; in countdown mode it shrinks.
    let effective = if countdown { 1.0 - progress } else { progress };
    let sweep = effective.clamp(0.0, 1.0) * TAU;
    if sweep > 0.001 {
        let start = -FRAC_PI_2;
        let end   = start + sweep;
        let path  = build_arc_path(CENTER, CENTER, RADIUS, start, end);
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }

    if paused {
        // Two vertical bars centred in the ring, in the round-type colour so
        // they read clearly on any panel colour regardless of theme foreground.
        paint.set_color(round_color);
        let bar_h = RADIUS * 0.75;
        let bar_w = STROKE_WIDTH * 1.2;
        let bar_gap = STROKE_WIDTH * 1.1;
        let bar_y = CENTER - bar_h / 2.0;
        for x in [CENTER - bar_gap / 2.0 - bar_w, CENTER + bar_gap / 2.0] {
            if let Some(rect) = tiny_skia::Rect::from_xywh(x, bar_y, bar_w, bar_h) {
                let p = PathBuilder::from_rect(rect);
                pixmap.fill_path(
                    &p, &paint, tiny_skia::FillRule::Winding,
                    Transform::identity(), None,
                );
            }
        }
    }

    pixmap.take()
}

/// Approximate a circular arc with cubic Bézier segments (max π/2 per segment).
fn build_arc_path(cx: f32, cy: f32, r: f32, start: f32, end: f32) -> tiny_skia::Path {
    let total = end - start;
    let n = ((total / (PI / 2.0)).ceil() as usize).max(1);
    let step = total / n as f32;
    let mut pb = PathBuilder::new();

    for i in 0..n {
        let a0 = start + step * i as f32;
        let a1 = a0 + step;
        arc_segment(&mut pb, cx, cy, r, a0, a1, i == 0);
    }

    pb.finish().unwrap_or_else(|| {
        PathBuilder::from_rect(tiny_skia::Rect::from_xywh(cx, cy, 1.0, 1.0).unwrap())
    })
}

/// Append one arc segment (≤ π/2) as a cubic Bézier.
fn arc_segment(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, first: bool) {
    let alpha = ((a1 - a0) / 4.0).tan() * 4.0 / 3.0;
    let (s0, c0) = a0.sin_cos();
    let (s1, c1) = a1.sin_cos();
    let x0 = cx + r * c0; let y0 = cy + r * s0;
    let x3 = cx + r * c1; let y3 = cy + r * s1;
    let x1 = x0 - alpha * r * s0; let y1 = y0 + alpha * r * c0;
    let x2 = x3 + alpha * r * s1; let y2 = y3 - alpha * r * c1;
    if first { pb.move_to(x0, y0); }
    pb.cubic_to(x1, y1, x2, y2, x3, y3);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_returns_correct_byte_count() {
        let bytes = render_tray_icon_rgba(&TrayColors::default(), false, 0.5, "work", false);
        assert_eq!(bytes.len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn render_paused_returns_correct_byte_count() {
        let bytes = render_tray_icon_rgba(&TrayColors::default(), true, 0.0, "work", false);
        assert_eq!(bytes.len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn render_paused_preserves_progress_arc() {
        let empty = render_tray_icon_rgba(&TrayColors::default(), true, 0.0, "work", false);
        let half = render_tray_icon_rgba(&TrayColors::default(), true, 0.5, "work", false);
        assert!(empty != half);
    }

    #[test]
    fn render_paused_still_shows_pause_indicator() {
        let running = render_tray_icon_rgba(&TrayColors::default(), false, 0.5, "work", false);
        let paused = render_tray_icon_rgba(&TrayColors::default(), true, 0.5, "work", false);
        assert!(running != paused);
    }

    #[test]
    fn render_zero_progress_returns_correct_byte_count() {
        let bytes = render_tray_icon_rgba(&TrayColors::default(), false, 0.0, "work", false);
        assert_eq!(bytes.len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn parse_hex_6_digit() {
        assert_eq!(parse_hex_color("#FF8800"), Some([255, 136, 0, 255]));
    }

    #[test]
    fn parse_hex_8_digit() {
        assert_eq!(parse_hex_color("#FF880080"), Some([255, 136, 0, 128]));
    }

    #[test]
    fn parse_hex_invalid() {
        assert_eq!(parse_hex_color("not-a-color"), None);
        assert_eq!(parse_hex_color("#ZZZ"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn tray_click_hides_visible_unminimized_window() {
        assert_eq!(tray_window_action(true, false), TrayWindowAction::Hide);
    }

    #[test]
    fn tray_click_restores_minimized_window() {
        assert_eq!(tray_window_action(true, true), TrayWindowAction::Restore);
    }

    #[test]
    fn tray_click_restores_hidden_window() {
        assert_eq!(tray_window_action(false, false), TrayWindowAction::Restore);
    }
}
