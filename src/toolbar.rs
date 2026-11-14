//! The floating toolbar at the top of the capture overlay: icon-only buttons with vector icons
//! (no emoji or font glyphs), colour swatches and stroke-width presets.

use crate::{annotate::Tool, theme};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Margin, Order, Painter, Pos2,
    Rect, Response, Sense, Shape, Stroke, Vec2, pos2, vec2,
};

pub const COLORS: [(Color32, &str); 6] = [
    (Color32::from_rgb(255, 92, 72), "Red"),
    (Color32::from_rgb(255, 196, 61), "Yellow"),
    (Color32::from_rgb(52, 199, 123), "Green"),
    (Color32::from_rgb(56, 152, 255), "Blue"),
    (Color32::from_rgb(245, 245, 247), "White"),
    (Color32::from_rgb(18, 18, 20), "Black"),
];

pub const WIDTHS: [(f32, &str); 3] = [(2.5, "Thin"), (5.0, "Medium"), (9.0, "Thick")];

/// Toolbar order. Their keyboard shortcuts come from the config (`config::ToolKeys`).
const TOOLS: [(Tool, Icon, &str); 7] = [
    (Tool::Select, Icon::Select, "Select area"),
    (Tool::Pen, Icon::Pen, "Pen"),
    (Tool::Arrow, Icon::Arrow, "Arrow"),
    (Tool::Rect, Icon::Rect, "Rectangle  ·  ⇧ for square"),
    (Tool::Ellipse, Icon::Ellipse, "Ellipse  ·  ⇧ for circle"),
    (Tool::Number, Icon::Number, "Numbered marker  ·  click to place 1, 2, 3…"),
    (Tool::Pixelate, Icon::Pixelate, "Pixelate  ·  hide passwords, emails, names"),
];

pub const TOOL_ORDER: [Tool; 7] =
    [TOOLS[0].0, TOOLS[1].0, TOOLS[2].0, TOOLS[3].0, TOOLS[4].0, TOOLS[5].0, TOOLS[6].0];

/// Display name per tool, for the settings window.
pub fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "Select area",
        Tool::Pen => "Pen",
        Tool::Arrow => "Arrow",
        Tool::Rect => "Rectangle",
        Tool::Ellipse => "Ellipse",
        Tool::Pixelate => "Pixelate",
        Tool::Number => "Numbered marker",
    }
}

const BTN: f32 = 32.0;
pub const TOP: f32 = 16.0;

pub enum Action {
    Undo,
    FullScreen,
    Close,
}

pub struct State<'a> {
    pub tool: &'a mut Tool,
    pub color: &'a mut Color32,
    pub width: &'a mut f32,
    pub can_undo: bool,
    pub keys: &'a crate::config::ToolKeys,
}

#[derive(Clone, Copy)]
enum Icon {
    Select,
    Pen,
    Arrow,
    Rect,
    Ellipse,
    Number,
    Pixelate,
    Undo,
    Screen,
    Close,
}

pub fn show(ctx: &egui::Context, st: State<'_>) -> Option<Action> {
    let mut action = None;
    egui::Area::new(Id::unique("snipno-toolbar"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_TOP, [0.0, TOP])
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(5))
                .shadow(theme::shadow())
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(2.0, 0.0);

                        for (tool, icon, name) in TOOLS {
                            let selected = *st.tool == tool;
                            let key = crate::hotkey::pretty(st.keys.get(tool));
                            if icon_button(ui, icon, selected, true).on_hover_text(tip(name, &key)).clicked() {
                                *st.tool = tool;
                            }
                        }
                        divider(ui);
                        for (color, name) in COLORS {
                            if swatch(ui, color, *st.color == color).on_hover_text(name).clicked() {
                                *st.color = color;
                                if matches!(*st.tool, Tool::Select | Tool::Pixelate) {
                                    *st.tool = Tool::Pen;
                                }
                            }
                        }
                        divider(ui);
                        for (w, name) in WIDTHS {
                            if width_button(ui, w, *st.width == w).on_hover_text(name).clicked() {
                                *st.width = w;
                            }
                        }
                        divider(ui);
                        if icon_button(ui, Icon::Undo, false, st.can_undo).on_hover_text(tip("Undo", "⌘Z")).clicked() {
                            action = Some(Action::Undo);
                        }
                        if icon_button(ui, Icon::Screen, false, true)
                            .on_hover_text(tip("Copy full screen", "↩"))
                            .clicked()
                        {
                            action = Some(Action::FullScreen);
                        }
                        if icon_button(ui, Icon::Close, false, true).on_hover_text(tip("Cancel", "Esc")).clicked() {
                            action = Some(Action::Close);
                        }
                    });
                });
        });
    action
}

/// Small pill under the toolbar that fades out after a few seconds.
pub fn hint(ctx: &egui::Context, text: &str, key: &str, opacity: f32) {
    if opacity <= 0.0 {
        return;
    }
    egui::Area::new(Id::unique("snipno-hint"))
        .order(Order::Foreground)
        .interactable(false)
        .anchor(Align2::CENTER_TOP, [0.0, TOP + BTN + 10.0 + 12.0])
        .show(ctx, |ui| {
            ui.set_opacity(opacity);
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(255))
                .inner_margin(Margin { left: 12, right: 15, top: 5, bottom: 5 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(egui::RichText::new(text).size(12.0).color(theme::MUTED));
                        theme::kbd(ui, key, 11.0);
                        ui.label(egui::RichText::new("to cancel").size(12.0).color(theme::MUTED));
                    });
                });
        });
}

fn tip(name: &str, key: &str) -> String {
    format!("{name}    {key}")
}

fn hover_t(ui: &egui::Ui, resp: &Response, on: bool) -> f32 {
    ui.ctx().animate_bool_with_time(resp.id.with("hover"), on, 0.08)
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, selected: bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(BTN), sense);
    let t = hover_t(ui, &resp, enabled && resp.hovered());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 8.0, theme::ACCENT_SOFT);
    } else if t > 0.0 {
        p.rect_filled(rect, 8.0, theme::HOVER.gamma_multiply(t));
    }
    let fg = if selected {
        theme::ACCENT
    } else if !enabled {
        theme::FAINT
    } else {
        lerp_color(Color32::from_rgb(200, 200, 206), theme::TEXT, t)
    };
    paint_icon(p, Rect::from_center_size(rect.center(), Vec2::splat(16.0)), icon, fg);
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

fn swatch(ui: &mut egui::Ui, color: Color32, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, BTN), Sense::click());
    let t = hover_t(ui, &resp, resp.hovered());
    let c = rect.center();
    let p = ui.painter();
    if selected {
        p.circle_stroke(c, 10.0, Stroke::new(1.5, theme::TEXT));
    }
    p.circle_filled(c, 7.0 + t, color);
    p.circle_stroke(c, 7.0 + t, Stroke::new(1.0, Color32::from_white_alpha(28)));
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn width_button(ui: &mut egui::Ui, width: f32, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, BTN), Sense::click());
    let t = hover_t(ui, &resp, resp.hovered());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 8.0, theme::SURFACE_HI);
        p.rect_stroke(rect, 8.0, Stroke::new(1.0, theme::BORDER_HI), egui::StrokeKind::Inside);
    } else if t > 0.0 {
        p.rect_filled(rect, 8.0, theme::HOVER.gamma_multiply(t));
    }
    let fg = if selected { theme::TEXT } else { theme::MUTED };
    p.circle_filled(rect.center(), width * 0.5 + 0.5, fg);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(11.0, BTN), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range().shrink(8.0), Stroke::new(1.0, theme::BORDER));
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Icons are drawn in a 16×16 box with 1.6pt round strokes.
fn paint_icon(p: &Painter, r: Rect, icon: Icon, c: Color32) {
    let s = Stroke::new(1.6, c);
    let at = |x: f32, y: f32| pos2(r.min.x + x, r.min.y + y);
    let line = |pts: &[Pos2]| {
        p.add(Shape::line(pts.to_vec(), s));
        for pt in pts {
            p.circle_filled(*pt, 0.8, c); // round joins/caps
        }
    };
    match icon {
        // Crop marquee — same glyph as the app logo.
        Icon::Select => {
            let (a, b, arm) = (1.5, 14.5, 4.5);
            line(&[at(a, a + arm), at(a, a), at(a + arm, a)]);
            line(&[at(b - arm, a), at(b, a), at(b, a + arm)]);
            line(&[at(b, b - arm), at(b, b), at(b - arm, b)]);
            line(&[at(a + arm, b), at(a, b), at(a, b - arm)]);
        }
        Icon::Pen => {
            // Pencil at 45°: body, tip and the ferrule line.
            let tip = at(1.5, 14.5);
            let dir = vec2(1.0, -1.0).normalized();
            let n = vec2(dir.y, -dir.x) * 2.4;
            let body_start = tip + dir * 4.0;
            let end = at(14.0, 2.0);
            line(&[body_start + n, end + n, end - n, body_start - n, body_start + n]);
            line(&[body_start + n, tip, body_start - n]);
            line(&[end - dir * 3.0 + n, end - dir * 3.0 - n]);
        }
        Icon::Arrow => {
            let (a, b) = (at(2.5, 13.5), at(13.5, 2.5));
            line(&[a, b]);
            line(&[at(6.5, 2.5), b, at(13.5, 9.5)]);
        }
        Icon::Rect => {
            p.rect_stroke(r.shrink(1.5), 3.0, s, egui::StrokeKind::Middle);
        }
        Icon::Ellipse => {
            p.circle_stroke(r.center(), 6.8, s);
        }
        Icon::Number => {
            p.circle_stroke(r.center(), 6.8, s);
            line(&[at(6.3, 6.0), at(8.6, 4.4), at(8.6, 11.6)]);
        }
        Icon::Pixelate => {
            // A 3×3 checkerboard of rounded squares.
            let cell = 14.0 / 3.0;
            for (i, j) in [(0, 0), (2, 0), (1, 1), (0, 2), (2, 2)] {
                let min = at(1.0 + i as f32 * cell, 1.0 + j as f32 * cell);
                p.rect_filled(Rect::from_min_size(min, Vec2::splat(cell)).shrink(0.4), 1.0, c);
            }
            p.rect_stroke(r.shrink(1.0), 2.0, Stroke::new(1.0, c.gamma_multiply(0.6)), egui::StrokeKind::Middle);
        }
        Icon::Undo => {
            // Arrow going left at the top, curving round the right and back along the bottom.
            let cy = 6.0;
            let mut pts = vec![at(2.0, cy), at(9.0, cy)];
            let (cx, rad) = (9.0, 4.0);
            for i in 1..=12 {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                pts.push(at(cx + rad * a.cos(), cy + rad + rad * a.sin()));
            }
            pts.push(at(5.0, cy + 2.0 * rad));
            line(&pts);
            line(&[at(5.0, cy - 3.0), at(2.0, cy), at(5.0, cy + 3.0)]);
        }
        Icon::Screen => {
            p.rect_stroke(
                Rect::from_min_max(at(1.0, 2.0), at(15.0, 11.5)),
                2.0,
                s,
                egui::StrokeKind::Middle,
            );
            line(&[at(8.0, 11.5), at(8.0, 14.0)]);
            line(&[at(5.0, 14.5), at(11.0, 14.5)]);
        }
        Icon::Close => {
            line(&[at(3.5, 3.5), at(12.5, 12.5)]);
            line(&[at(12.5, 3.5), at(3.5, 12.5)]);
        }
    }
}

/// Size pill shown above the selection.
pub fn size_label(p: &Painter, sel: Rect, screen: Rect, text: String) {
    let galley = p.layout_no_wrap(text, FontId::proportional(11.5), theme::TEXT);
    let size = galley.size() + vec2(14.0, 6.0);
    let mut min = pos2(sel.min.x, sel.min.y - size.y - 6.0);
    if min.y < screen.min.y + 4.0 {
        min.y = sel.min.y + 6.0; // no room above: put it inside the selection
        min.x += 6.0;
    }
    let bg = Rect::from_min_size(min, size);
    p.rect_filled(bg, 6.0, theme::SURFACE);
    p.rect_stroke(bg, 6.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    p.galley(bg.min + vec2(7.0, 3.0), galley, theme::TEXT);
}
