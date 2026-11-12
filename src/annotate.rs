//! Annotation shapes drawn on the frozen screenshot. Shapes live in window points; they are
//! painted live with egui and rasterised onto the captured pixels with tiny-skia on copy, so
//! both paths share the same geometry helpers.
//!
//! Two kinds are special: **pixelate** boxes replace the screenshot underneath with big blocks
//! (see [`Mosaic`]) and are always applied before any other shape, and **numbered markers** are
//! badges whose digits are drawn as strokes (see [`digit_strokes`]), since tiny-skia has no text.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind, Vec2};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Transform};
use xcap::image::{RgbaImage, imageops};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Pen,
    Arrow,
    Rect,
    Ellipse,
    Pixelate,
    Number,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Pen(Vec<Pos2>),
    Arrow(Pos2, Pos2),
    Rect(Pos2, Pos2),
    Ellipse(Pos2, Pos2),
    Pixelate(Pos2, Pos2),
    /// Badge centre and the number on it.
    Number(Pos2, u32),
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub kind: Kind,
    pub color: Color32,
    pub width: f32,
}

impl Shape {
    /// `number` is what a numbered marker shows; other tools ignore it.
    pub fn start(tool: Tool, at: Pos2, color: Color32, width: f32, number: u32) -> Option<Shape> {
        let kind = match tool {
            Tool::Select => return None,
            Tool::Pen => Kind::Pen(vec![at]),
            Tool::Arrow => Kind::Arrow(at, at),
            Tool::Rect => Kind::Rect(at, at),
            Tool::Ellipse => Kind::Ellipse(at, at),
            Tool::Pixelate => Kind::Pixelate(at, at),
            Tool::Number => Kind::Number(at, number),
        };
        Some(Shape { kind, color, width })
    }

    /// `square` (Shift held) keeps rectangles square and ellipses circular.
    pub fn drag_to(&mut self, p: Pos2, square: bool) {
        match &mut self.kind {
            Kind::Pen(points) => {
                if points.last().is_none_or(|last| last.distance(p) >= 0.75) {
                    points.push(p);
                }
            }
            Kind::Arrow(_, end) => *end = p,
            Kind::Rect(start, end) | Kind::Ellipse(start, end) | Kind::Pixelate(start, end) => {
                *end = if square { square_end(*start, p) } else { p };
            }
            // Dragging a marker moves it, so you can place it precisely.
            Kind::Number(at, _) => *at = p,
        }
    }

    pub fn is_trivial(&self) -> bool {
        match &self.kind {
            Kind::Pen(points) => points.len() < 2,
            Kind::Arrow(a, b) | Kind::Rect(a, b) | Kind::Ellipse(a, b) => a.distance(*b) < 3.0,
            Kind::Pixelate(a, b) => (a.x - b.x).abs() < 3.0 || (a.y - b.y).abs() < 3.0,
            Kind::Number(..) => false,
        }
    }

    pub fn is_pixelate(&self) -> bool {
        matches!(self.kind, Kind::Pixelate(..))
    }

    /// Live preview in the overlay. Pixelate boxes need the mosaic texture, so the overlay paints
    /// those itself (see [`Mosaic::uv`]).
    pub fn paint(&self, painter: &egui::Painter) {
        let stroke = Stroke::new(self.width, self.color);
        match &self.kind {
            Kind::Pen(points) => {
                painter.add(egui::Shape::line(smooth(points), stroke));
                // Round ends so short strokes and dots don't look clipped.
                for p in [points.first(), points.last()].into_iter().flatten() {
                    painter.circle_filled(*p, self.width / 2.0, self.color);
                }
            }
            Kind::Arrow(a, b) => {
                let (shaft_end, head) = arrow_geometry(*a, *b, self.width);
                painter.line_segment([*a, shaft_end], stroke);
                painter.add(egui::Shape::convex_polygon(head.to_vec(), self.color, Stroke::NONE));
            }
            Kind::Rect(a, b) => {
                painter.rect_stroke(Rect::from_two_pos(*a, *b), 0.0, stroke, StrokeKind::Middle);
            }
            Kind::Ellipse(a, b) => {
                let r = Rect::from_two_pos(*a, *b);
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            Kind::Pixelate(..) => {}
            Kind::Number(at, n) => {
                let badge = Badge::new(*at, *n, self.width, self.color);
                painter.circle_filled(*at, badge.radius, self.color);
                let ink = Stroke::new(badge.ink_width, badge.ink);
                for line in &badge.strokes {
                    painter.add(egui::Shape::line(line.clone(), ink));
                    // Round caps, like tiny-skia's.
                    for p in [line.first(), line.last()].into_iter().flatten() {
                        painter.circle_filled(*p, badge.ink_width / 2.0, badge.ink);
                    }
                }
            }
        }
    }

    fn rasterise(&self, pixmap: &mut Pixmap, transform: Transform) {
        let [r, g, b, a] = self.color.to_srgba_unmultiplied();
        let mut paint = Paint::default();
        paint.set_color_rgba8(r, g, b, a);
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: self.width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };

        let mut stroke_path = |path: Option<Path>| {
            if let Some(path) = path {
                pixmap.stroke_path(&path, &paint, &stroke, transform, None);
            }
        };
        match &self.kind {
            Kind::Pen(points) => stroke_path(polyline(&smooth(points))),
            Kind::Arrow(a, b) => {
                let (shaft_end, head) = arrow_geometry(*a, *b, self.width);
                stroke_path(polyline(&[*a, shaft_end]));
                if let Some(head) = polygon(&head) {
                    pixmap.fill_path(&head, &paint, FillRule::Winding, transform, None);
                }
            }
            Kind::Rect(a, b) => stroke_path(sk_rect(*a, *b).map(PathBuilder::from_rect)),
            Kind::Ellipse(a, b) => stroke_path(sk_rect(*a, *b).and_then(PathBuilder::from_oval)),
            Kind::Pixelate(..) => {} // applied to the pixels beforehand, see `render`
            Kind::Number(at, n) => {
                let badge = Badge::new(*at, *n, self.width, self.color);
                if let Some(circle) = PathBuilder::from_circle(at.x, at.y, badge.radius) {
                    pixmap.fill_path(&circle, &paint, FillRule::Winding, transform, None);
                }
                let [r, g, b, a] = badge.ink.to_srgba_unmultiplied();
                let mut ink = Paint::default();
                ink.set_color_rgba8(r, g, b, a);
                ink.anti_alias = true;
                let ink_stroke = tiny_skia::Stroke { width: badge.ink_width, ..stroke.clone() };
                for line in &badge.strokes {
                    if let Some(path) = polyline(line) {
                        pixmap.stroke_path(&path, &ink, &ink_stroke, transform, None);
                    }
                }
            }
        }
    }
}

/// A numbered marker's geometry, shared by the live preview and the copied image.
struct Badge {
    radius: f32,
    ink: Color32,
    ink_width: f32,
    strokes: Vec<Vec<Pos2>>,
}

impl Badge {
    fn new(at: Pos2, n: u32, width: f32, color: Color32) -> Badge {
        // The stroke-width presets (2.5 / 5 / 9) give 12 / 15 / 19pt badges.
        let radius = 9.5 + width;
        // Dark digits on light badges (yellow, white), white on the rest.
        let [r, g, b, _] = color.to_srgba_unmultiplied();
        let light = r as u32 * 299 + g as u32 * 587 + b as u32 * 114 > 160_000;
        let ink = if light { Color32::from_rgb(18, 18, 20) } else { Color32::WHITE };

        let digits: Vec<u32> = n.to_string().chars().filter_map(|c| c.to_digit(10)).collect();
        // Glyphs are 1 wide and 1.6 tall; digits sit 0.45 apart and shrink to stay on the badge.
        let height = radius
            * match digits.len() {
                1 => 0.95,
                2 => 0.8,
                _ => 0.55,
            };
        let unit = height / 1.6;
        let total = digits.len() as f32 + (digits.len() as f32 - 1.0) * 0.45;
        let origin = at - Vec2::new(total * unit / 2.0, height / 2.0);
        let strokes = digits
            .iter()
            .enumerate()
            .flat_map(|(i, d)| {
                let x0 = i as f32 * 1.45;
                digit_strokes(*d)
                    .into_iter()
                    .map(move |line| line.iter().map(|p| origin + Vec2::new((p.x + x0) * unit, p.y * unit)).collect::<Vec<Pos2>>())
            })
            .collect();
        Badge { radius, ink, ink_width: (unit * 0.24).max(1.2), strokes }
    }
}

/// Strokes for one digit in a 1×1.6 box (y down), drawn with round caps.
pub fn digit_strokes(d: u32) -> Vec<Vec<Pos2>> {
    use std::f32::consts::PI;
    let p = Pos2::new;
    // Elliptical arc from angle `a0` to `a1` (radians, y down: PI/2 is the bottom).
    let arc = |cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, a1: f32| -> Vec<Pos2> {
        let steps = ((a1 - a0).abs() / 0.2).ceil().max(2.0) as usize;
        (0..=steps)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / steps as f32;
                p(cx + rx * a.cos(), cy + ry * a.sin())
            })
            .collect()
    };
    match d {
        0 => vec![arc(0.5, 0.8, 0.5, 0.8, 0.0, 2.0 * PI)],
        1 => vec![vec![p(0.2, 0.35), p(0.62, 0.0), p(0.62, 1.6)]],
        2 => {
            let mut top = arc(0.5, 0.5, 0.5, 0.5, PI, 2.0 * PI + 0.6);
            top.extend([p(0.0, 1.6), p(1.0, 1.6)]);
            vec![top]
        }
        3 => vec![
            arc(0.5, 0.42, 0.45, 0.42, PI + 0.5, 2.5 * PI),
            arc(0.5, 1.22, 0.5, 0.38, -0.5 * PI, PI - 0.5),
        ],
        4 => vec![vec![p(0.75, 1.6), p(0.75, 0.0), p(0.0, 1.1), p(1.0, 1.1)]],
        5 => {
            let mut s = vec![p(0.9, 0.0), p(0.15, 0.0), p(0.12, 0.75)];
            s.extend(arc(0.5, 1.1, 0.5, 0.5, -0.75 * PI, 0.8 * PI));
            vec![s]
        }
        6 => vec![vec![p(0.75, 0.0), p(0.07, 0.84)], arc(0.5, 1.1, 0.5, 0.5, 0.0, 2.0 * PI)],
        7 => vec![vec![p(0.0, 0.0), p(1.0, 0.0), p(0.35, 1.6)]],
        8 => vec![arc(0.5, 0.38, 0.4, 0.38, 0.0, 2.0 * PI), arc(0.5, 1.18, 0.5, 0.42, 0.0, 2.0 * PI)],
        _ => vec![arc(0.5, 0.5, 0.5, 0.5, 0.0, 2.0 * PI), vec![p(0.93, 0.76), p(0.25, 1.6)]],
    }
}

/// The screenshot shrunk to one pixel per block (each the average of the block), for
/// pixelate boxes. Drawn back up with nearest-neighbour sampling in the overlay and copied
/// block by block into the result, so both line up on the same grid.
pub struct Mosaic {
    /// One pixel per `block`×`block` block of the screenshot.
    pub small: RgbaImage,
    pub block: u32,
    /// Size of the screenshot it was made from.
    full: (u32, u32),
}

impl Mosaic {
    /// Block size in points; big enough that text underneath can't be read back.
    pub const BLOCK_PT: f32 = 10.0;

    pub fn new(base: &RgbaImage, scale: f32) -> Mosaic {
        let block = (Self::BLOCK_PT * scale).round().max(1.0) as u32;
        let (w, h) = base.dimensions();
        let (bw, bh) = (w.div_ceil(block), h.div_ceil(block));
        let mut sums = vec![[0u64; 4]; (bw * bh) as usize];
        for (x, y, px) in base.enumerate_pixels() {
            let sum = &mut sums[((y / block) * bw + x / block) as usize];
            for (total, c) in sum.iter_mut().zip(px.0) {
                *total += c as u64;
            }
        }
        let small = RgbaImage::from_fn(bw, bh, |bx, by| {
            let n = (block.min(w - bx * block) * block.min(h - by * block)) as u64;
            let s = sums[(by * bw + bx) as usize];
            xcap::image::Rgba([(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8, 255])
        });
        Mosaic { small, block, full: (w, h) }
    }

    /// Texture coordinates of `r` (window points) in the `small` image, for the live preview.
    pub fn uv(&self, r: Rect, scale: f32) -> Rect {
        let (sw, sh) = ((self.small.width() * self.block) as f32, (self.small.height() * self.block) as f32);
        Rect::from_min_max(
            Pos2::new(r.min.x * scale / sw, r.min.y * scale / sh),
            Pos2::new(r.max.x * scale / sw, r.max.y * scale / sh),
        )
    }

    /// Pixelate `r` (window points) in `img`, a crop of the screenshot whose top-left pixel is
    /// at `(ox, oy)` in the full screenshot.
    fn apply(&self, img: &mut RgbaImage, r: Rect, scale: f32, (ox, oy): (u32, u32)) {
        let (x0, y0, w, h) = pixel_rect(r, scale, self.full.0, self.full.1);
        for y in y0.max(oy)..(y0 + h).min(oy + img.height()) {
            for x in x0.max(ox)..(x0 + w).min(ox + img.width()) {
                img.put_pixel(x - ox, y - oy, *self.small.get_pixel(x / self.block, y / self.block));
            }
        }
    }
}

/// Midpoint-quadratic smoothing: every raw mouse sample becomes a control point and the curve
/// runs through the midpoints between samples. Turns jagged mouse input into a fluid stroke
/// without lagging behind the cursor (the end point is kept exactly).
pub fn smooth(points: &[Pos2]) -> Vec<Pos2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len() * 3);
    out.push(points[0]);
    let mut from = points[0];
    for i in 1..points.len() - 1 {
        let ctrl = points[i];
        let to = ctrl.lerp(points[i + 1], 0.5);
        // About one sample per 1.5pt of curve, capped so long jumps stay cheap.
        let steps = ((from.distance(ctrl) + ctrl.distance(to)) / 1.5).ceil().clamp(1.0, 24.0) as usize;
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            out.push(from.lerp(ctrl, t).lerp(ctrl.lerp(to, t), t));
        }
        from = to;
    }
    out.push(*points.last().expect("len >= 3"));
    out
}

fn square_end(start: Pos2, p: Pos2) -> Pos2 {
    let d = p - start;
    let side = d.x.abs().max(d.y.abs());
    start + Vec2::new(side.copysign(d.x), side.copysign(d.y))
}

/// Returns where the shaft stops and the filled triangle head `[tip, left, right]`.
pub fn arrow_geometry(a: Pos2, b: Pos2, width: f32) -> (Pos2, [Pos2; 3]) {
    let len = a.distance(b);
    let dir = if len > 0.0 { (b - a) / len } else { Vec2::X };
    let head_len = (width * 3.5).max(12.0).min(len.max(1.0));
    let half = head_len * 0.55;
    let base = b - dir * head_len;
    let normal = Vec2::new(-dir.y, dir.x);
    (b - dir * head_len * 0.8, [b, base + normal * half, base - normal * half])
}

fn polyline(points: &[Pos2]) -> Option<Path> {
    let (first, rest) = points.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    if rest.is_empty() {
        pb.line_to(first.x, first.y);
    }
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.finish()
}

fn polygon(points: &[Pos2]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.move_to(first.x, first.y);
    for p in rest {
        pb.line_to(p.x, p.y);
    }
    pb.close();
    pb.finish()
}

fn sk_rect(a: Pos2, b: Pos2) -> Option<tiny_skia::Rect> {
    let r = Rect::from_two_pos(a, b);
    tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y)
}

/// Map a selection in window points to a pixel rect inside an image of `img_w`×`img_h`.
pub fn pixel_rect(selection: Rect, scale: f32, img_w: u32, img_h: u32) -> (u32, u32, u32, u32) {
    let clamp = |v: f32, max: u32| (v * scale).round().clamp(0.0, max as f32) as u32;
    let x0 = clamp(selection.min.x, img_w.saturating_sub(1));
    let y0 = clamp(selection.min.y, img_h.saturating_sub(1));
    let x1 = clamp(selection.max.x, img_w).max(x0 + 1);
    let y1 = clamp(selection.max.y, img_h).max(y0 + 1);
    (x0, y0, x1 - x0, y1 - y0)
}

/// Crop `base` to `selection` (window points) and burn the annotations into it.
/// `scale` is image pixels per window point (2.0 on Retina). `mosaic` must be set when any
/// shape is a pixelate box (made from `base` at the same `scale`).
pub fn render(base: &RgbaImage, shapes: &[Shape], scale: f32, selection: Rect, mosaic: Option<&Mosaic>) -> RgbaImage {
    let (x, y, w, h) = pixel_rect(selection, scale, base.width(), base.height());
    let mut cropped = imageops::crop_imm(base, x, y, w, h).to_image();
    // Pixelate first, so drawings on top stay sharp (the overlay paints them in this order too).
    if let Some(mosaic) = mosaic {
        for shape in shapes {
            if let Kind::Pixelate(a, b) = shape.kind {
                mosaic.apply(&mut cropped, Rect::from_two_pos(a, b), scale, (x, y));
            }
        }
    }
    // tiny-skia wants premultiplied pixels; screen captures are opaque, so force alpha to 255.
    for px in cropped.pixels_mut() {
        px.0[3] = 255;
    }
    let (w, h) = cropped.dimensions();
    let Some(mut pixmap) = tiny_skia::IntSize::from_wh(w, h)
        .and_then(|size| Pixmap::from_vec(cropped.into_raw(), size))
    else {
        return imageops::crop_imm(base, x, y, w, h).to_image();
    };

    let transform = Transform::from_row(scale, 0.0, 0.0, scale, -(x as f32), -(y as f32));
    for shape in shapes {
        shape.rasterise(&mut pixmap, transform);
    }
    // Opaque background + source-over keeps every pixel opaque, so premultiplied == straight.
    RgbaImage::from_raw(w, h, pixmap.take()).expect("pixmap size matches image")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;
    use xcap::image::Rgba;

    const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

    #[test]
    fn crops_at_retina_scale() {
        let base = RgbaImage::from_pixel(200, 100, WHITE);
        let out = render(&base, &[], 2.0, Rect::from_min_max(pos2(10.0, 10.0), pos2(40.0, 30.0)), None);
        assert_eq!(out.dimensions(), (60, 40));
    }

    #[test]
    fn selection_is_clamped_to_the_image() {
        assert_eq!(
            pixel_rect(Rect::from_min_max(pos2(-5.0, -5.0), pos2(500.0, 500.0)), 2.0, 200, 100),
            (0, 0, 200, 100)
        );
    }

    #[test]
    fn burns_rectangle_into_cropped_pixels() {
        let base = RgbaImage::from_pixel(200, 200, WHITE);
        let shape = Shape {
            kind: Kind::Rect(pos2(20.0, 20.0), pos2(80.0, 80.0)),
            color: Color32::RED,
            width: 4.0,
        };
        // Select 10..90 pt at 2× → 160×160 px, rect edge at (20-10)*2 = 20 px.
        let out = render(&base, &[shape], 2.0, Rect::from_min_max(pos2(10.0, 10.0), pos2(90.0, 90.0)), None);
        assert_eq!(out.dimensions(), (160, 160));
        assert_eq!(out.get_pixel(20, 80).0, [255, 0, 0, 255], "left edge is red");
        assert_eq!(out.get_pixel(80, 80).0, [255, 255, 255, 255], "inside stays white");
        assert_eq!(out.get_pixel(5, 5).0, [255, 255, 255, 255], "outside stays white");
    }

    #[test]
    fn shift_keeps_shapes_square() {
        let mut s = Shape::start(Tool::Ellipse, pos2(0.0, 0.0), Color32::RED, 2.0, 0).unwrap();
        s.drag_to(pos2(30.0, -10.0), true);
        let Kind::Ellipse(_, end) = s.kind else { unreachable!() };
        assert_eq!(end, pos2(30.0, -30.0));
    }

    #[test]
    fn smoothing_keeps_endpoints_and_rounds_corners() {
        let raw = [pos2(0.0, 0.0), pos2(20.0, 0.0), pos2(20.0, 20.0)];
        let out = smooth(&raw);
        assert_eq!(out.first(), Some(&raw[0]));
        assert_eq!(out.last(), Some(&raw[2]));
        // The sharp corner at (20,0) is cut: no output point reaches it.
        assert!(out.iter().all(|p| p.distance(raw[1]) > 1.0));
        assert!(out.len() > raw.len());
    }

    #[test]
    fn pixelate_averages_blocks_on_a_fixed_grid() {
        // 40×20 px at 1×: left half black, right half white; blocks are 10 px.
        let mut base = RgbaImage::from_pixel(40, 20, WHITE);
        for y in 0..20 {
            for x in 0..15 {
                base.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let mosaic = Mosaic::new(&base, 1.0);
        assert_eq!(mosaic.small.dimensions(), (4, 2));
        assert_eq!(mosaic.small.get_pixel(1, 0).0, [127, 127, 127, 255], "half black, half white");

        let shape = Shape { kind: Kind::Pixelate(pos2(5.0, 0.0), pos2(25.0, 20.0)), color: Color32::RED, width: 2.0 };
        // Select from x=5 so the crop is offset from the block grid.
        let sel = Rect::from_min_max(pos2(5.0, 0.0), pos2(40.0, 20.0));
        let out = render(&base, &[shape], 1.0, sel, Some(&mosaic));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 255], "x=5 lies in the all-black block");
        assert_eq!(out.get_pixel(6, 0).0, [127, 127, 127, 255], "x=11 lies in the mixed block");
        assert_eq!(out.get_pixel(25, 0).0, [255, 255, 255, 255], "x=30 is outside the box");
    }

    #[test]
    fn numbered_marker_is_a_filled_badge_with_contrasting_digits() {
        let base = RgbaImage::from_pixel(100, 100, WHITE);
        let shape = Shape { kind: Kind::Number(pos2(50.0, 50.0), 7), color: Color32::from_rgb(56, 152, 255), width: 5.0 };
        let out = render(&base, &[shape], 1.0, Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 100.0)), None);
        assert_eq!(out.get_pixel(50, 37).0, [56, 152, 255, 255], "badge fill (radius 14.5)");
        assert_eq!(out.get_pixel(50, 70).0, [255, 255, 255, 255], "outside the badge");
        // The 7's top bar (y ≈ 43.1, 2pt thick) is white ink on the blue badge.
        assert_eq!(out.get_pixel(50, 43).0, [255, 255, 255, 255]);
        assert!(digit_strokes(8).len() == 2 && (0..10).all(|d| !digit_strokes(d).is_empty()));
    }

    #[test]
    fn arrow_head_tip_is_the_end_point() {
        let (shaft_end, head) = arrow_geometry(pos2(0.0, 0.0), pos2(100.0, 0.0), 4.0);
        assert_eq!(head[0], pos2(100.0, 0.0));
        assert!(shaft_end.x < 100.0 && shaft_end.x > 80.0);
    }
}
