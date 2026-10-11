use std::sync::OnceLock;

use ab_glyph::{point, Font, FontRef, PxScale};

use crate::timer::TimerSnapshot;

use super::{render_tray_icon_rgba, TrayColors, SIZE};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum IconKey {
    Time { text: String, paused: bool, dark: bool },
    Ring { colors: TrayColors, round: String, paused: bool, countdown: bool, percent: u8 },
}

impl IconKey {
    pub fn new(timer: &TimerSnapshot, show_time: bool, dark: bool, colors: TrayColors, countdown: bool) -> Self {
        if show_time {
            Self::Time {
                text: tray_text(timer.total_secs.saturating_sub(timer.elapsed_secs)),
                paused: timer.is_paused,
                dark,
            }
        } else {
            let percent = if timer.total_secs == 0 {
                0
            } else {
                (timer.elapsed_secs as f64 / timer.total_secs as f64 * 100.0).min(100.0) as u8
            };
            Self::Ring { colors, round: timer.round_type.clone(), paused: timer.is_paused, countdown, percent }
        }
    }

    pub fn render(&self) -> Vec<u8> {
        match self {
            Self::Time { text, paused, dark } => render_time(text, *paused, *dark, SIZE),
            Self::Ring { colors, round, paused, countdown, percent } => {
                render_tray_icon_rgba(colors, *paused, *percent as f32 / 100.0, round, *countdown)
            }
        }
    }
}

pub(super) fn tray_text(remaining_secs: u32) -> String {
    let value = if remaining_secs < 60 { remaining_secs } else { remaining_secs.div_ceil(60) };
    format!("{value:02}")
}

pub(super) fn tooltip(timer: &TimerSnapshot) -> String {
    let remaining = timer.total_secs.saturating_sub(timer.elapsed_secs);
    format!("Pomotroid - {:02}:{:02}", remaining / 60, remaining % 60)
}

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        FontRef::try_from_slice(include_bytes!("../../../src/lib/assets/fonts/Inter.ttf"))
            .expect("bundled Inter font must be valid")
    })
}

fn render_time(text: &str, paused: bool, dark: bool, size: u32) -> Vec<u8> {
    let font = font();
    let size_f = size as f32;
    let cells = text.len().max(2) as f32;
    let padding = (size_f * 0.06).max(1.0);
    let cell_width = (size_f - padding * 2.0) / cells;
    let mut max_width = 0.0_f32;
    let mut top = 0.0_f32;
    let mut bottom = 0.0_f32;
    // Measure all digits, not just this value, so size and baseline never change.
    for c in '0'..='9' {
        let outline = font.outline_glyph(font.glyph_id(c).with_scale(100.0))
            .expect("Inter must contain digits");
        let bounds = outline.px_bounds();
        max_width = max_width.max(bounds.width());
        top = top.min(bounds.min.y);
        bottom = bottom.max(bounds.max.y);
    }
    let factor = (size_f * 0.66 / (bottom - top)).min(cell_width * 0.9 / max_width);
    let scale = PxScale::from(100.0 * factor);
    let baseline = (size_f - (bottom - top) * factor) / 2.0 - top * factor;
    let foreground = if dark { 255 } else { 0 };
    let mut pixels = vec![0; (size * size * 4) as usize];

    for (index, c) in text.chars().enumerate() {
        let glyph = font.glyph_id(c).with_scale(scale);
        let outline = font.outline_glyph(glyph.clone()).expect("Inter must contain digits");
        let bounds = outline.px_bounds();
        let cell_center = padding + (index as f32 + 0.5) * cell_width;
        let positioned = ab_glyph::Glyph {
            position: point(cell_center - bounds.width() / 2.0 - bounds.min.x, baseline),
            ..glyph
        };
        let outline = font.outline_glyph(positioned).expect("Inter must contain digits");
        let bounds = outline.px_bounds();
        outline.draw(|x, y, coverage| {
            let x = x as i32 + bounds.min.x as i32;
            let y = y as i32 + bounds.min.y as i32;
            if x >= 0 && y >= 0 && x < size as i32 && y < size as i32 {
                let offset = ((y as u32 * size + x as u32) * 4) as usize;
                pixels[offset..offset + 4].copy_from_slice(&[
                    foreground, foreground, foreground, (coverage * 255.0).round() as u8,
                ]);
            }
        });
    }

    if paused {
        // A small mark below the digits, never over them or replacing them.
        let y = (size_f * 0.87) as u32;
        let width = (size / 32).max(1);
        for x in [size / 2 - width * 2, size / 2 + width] {
            for dx in 0..width {
                for dy in 0..width {
                    let offset = (((y + dy) * size + x + dx) * 4) as usize;
                    pixels[offset..offset + 4].copy_from_slice(&[foreground, foreground, foreground, 255]);
                }
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(total_secs: u32, elapsed_secs: u32) -> TimerSnapshot {
        TimerSnapshot {
            round_type: "work".into(),
            previous_round_type: String::new(),
            total_secs,
            elapsed_secs,
            is_running: true,
            is_paused: false,
            work_round_number: 1,
            work_rounds_total: 4,
            session_work_count: 1,
        }
    }

    #[test]
    fn countdown_boundaries_are_two_digits_without_units() {
        for (seconds, expected) in [
            (5400, "90"), (1500, "25"), (1499, "25"), (1440, "24"),
            (121, "03"), (120, "02"), (61, "02"), (60, "01"),
            (59, "59"), (10, "10"), (9, "09"), (1, "01"), (0, "00"),
        ] {
            assert_eq!(tray_text(seconds), expected);
        }
    }

    #[test]
    fn numeric_updates_do_not_wait_for_one_percent_progress() {
        let key = |timer: &TimerSnapshot| IconKey::new(timer, true, true, TrayColors::default(), true);
        assert_ne!(key(&snapshot(5400, 0)), key(&snapshot(5400, 60)));
        assert_ne!(key(&snapshot(5400, 5340)), key(&snapshot(5400, 5341)));
        assert_ne!(key(&snapshot(5400, 5341)), key(&snapshot(5400, 5342)));
        assert_eq!(key(&snapshot(1500, 1)), key(&snapshot(1500, 2)));
        assert_eq!(tooltip(&snapshot(1500, 1)), "Pomotroid - 24:59");
        assert_eq!(tooltip(&snapshot(60, 61)), "Pomotroid - 00:00");
    }

    #[test]
    fn glyphs_remain_inside_fixed_transparent_icon_at_tray_sizes() {
        for size in [16, 20, 24, 32, 64] {
            for text in ["00", "01", "11", "25", "59", "88", "90"] {
                for dark in [false, true] {
                    let pixels = render_time(text, false, dark, size);
                    assert_eq!(pixels.len(), (size * size * 4) as usize);
                    assert!(pixels.chunks_exact(4).any(|p| p[3] > 0));
                    for y in 0..size {
                        for x in 0..size {
                            let pixel = &pixels[((y * size + x) * 4) as usize..][..4];
                            if x == 0 || y == 0 || x == size - 1 || y == size - 1 {
                                assert_eq!(pixel[3], 0, "{text} clipped at size {size}");
                            }
                            if pixel[3] > 0 {
                                assert_eq!(&pixel[..3], &[if dark { 255 } else { 0 }; 3]);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pause_indicator_does_not_change_digit_pixels() {
        let running = render_time("25", false, true, 64);
        let paused = render_time("25", true, true, 64);
        assert_eq!(&running[..54 * 64 * 4], &paused[..54 * 64 * 4]);
        assert_ne!(running, paused);
    }
}
