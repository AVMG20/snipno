//! Shared look for the overlay toolbar and the settings window: near-black surfaces, hairline
//! borders, one orange accent, the macOS system font.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText,
    Shadow, Stroke, TextStyle, vec2,
};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(13, 13, 15);
pub const SURFACE: Color32 = Color32::from_rgb(21, 21, 24);
pub const SURFACE_HI: Color32 = Color32::from_rgb(31, 31, 35);
pub const HOVER: Color32 = Color32::from_rgb(38, 38, 43);
pub const BORDER: Color32 = Color32::from_rgb(40, 40, 45);
pub const BORDER_HI: Color32 = Color32::from_rgb(58, 58, 64);
pub const TEXT: Color32 = Color32::from_rgb(236, 236, 239);
pub const MUTED: Color32 = Color32::from_rgb(140, 140, 148);
pub const FAINT: Color32 = Color32::from_rgb(88, 88, 95);
pub const ACCENT: Color32 = Color32::from_rgb(255, 106, 61);
/// ACCENT at ~16% opacity (premultiplied), for selected backgrounds.
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(41, 17, 10, 41);
pub const SUCCESS: Color32 = Color32::from_rgb(52, 199, 123);
pub const DANGER: Color32 = Color32::from_rgb(255, 92, 92);

pub fn shadow() -> Shadow {
    Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(140) }
}

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    install_fonts(ctx);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(17.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.item_spacing = vec2(8.0, 8.0);

        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.window_shadow = shadow();
        v.popup_shadow = shadow();
        v.extreme_bg_color = BG;
        v.selection.bg_fill = ACCENT_SOFT;
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.hyperlink_color = ACCENT;

        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (state, fill, border) in [
            (&mut w.inactive, SURFACE_HI, BORDER),
            (&mut w.hovered, HOVER, BORDER_HI),
            (&mut w.active, SURFACE_HI, BORDER_HI),
            (&mut w.open, SURFACE_HI, BORDER_HI),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, border);
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(8);
            state.expansion = 0.0;
        }
    });
}

/// Prefer San Francisco (proportional + mono); fall back to egui's bundled fonts.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, path, family) in [
        ("sf", "/System/Library/Fonts/SFNS.ttf", FontFamily::Proportional),
        ("sf-mono", "/System/Library/Fonts/SFNSMono.ttf", FontFamily::Monospace),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    ctx.set_fonts(fonts);
}

/// A keyboard key chip, e.g. `⌘` `⇧` `2`.
pub fn kbd(ui: &mut egui::Ui, key: &str, size: f32) {
    egui::Frame::new()
        .fill(SURFACE_HI)
        .stroke(Stroke::new(1.0, BORDER_HI))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(key).font(FontId::proportional(size)).color(TEXT));
        });
}

/// Split a pretty hotkey like `⇧⌘2` into chips: `⇧` `⌘` `2`.
pub fn hotkey_chips(pretty: &str) -> Vec<String> {
    let mut chips = Vec::new();
    let mut rest = pretty;
    while let Some(c) = rest.chars().next() {
        if "⌃⌥⇧⌘".contains(c) {
            chips.push(c.to_string());
            rest = &rest[c.len_utf8()..];
        } else {
            chips.push(rest.to_string());
            break;
        }
    }
    chips
}

#[cfg(test)]
mod tests {
    #[test]
    fn hotkey_chips_split_modifiers_from_key() {
        assert_eq!(super::hotkey_chips("⇧⌘2"), ["⇧", "⌘", "2"]);
        assert_eq!(super::hotkey_chips("⌘F13 (PrtSc)"), ["⌘", "F13 (PrtSc)"]);
        assert_eq!(super::hotkey_chips("F13"), ["F13"]);
    }
}
