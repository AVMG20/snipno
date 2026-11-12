//! Annotation shapes drawn on the frozen screenshot. Shapes live in window points; they are
//! painted live with egui and rasterised onto the captured pixels with tiny-skia on copy, so
//! both paths share the same geometry helpers.

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
}

#[derive(Clone, Debug)]
pub enum Kind {
    Pen(Vec<Pos2>),
    Arrow(Pos2, Pos2),
    Rect(Pos2, Pos2),
    Ellipse(Pos2, Pos2),
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub kind: Kind,
    pub color: Color32,
    pub width: f32,
}

impl Shape {
    pub fn start(tool: Tool, at: Pos2, color: Color32, width: f32) -> Option<Shape> {
        let kind = match tool {
            Tool::Select => return None,
            Tool::Pen => Kind::Pen(vec![at]),
            Tool::Arrow => Kind::Arrow(at, at),
            Tool::Rect => Kind::Rect(at, at),
            Tool::Ellipse => Kind::Ellipse(at, at),
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
            Kind::Rect(start, end) | Kind::Ellipse(start, end) => {
                *end = if square { square_end(*start, p) } else { p };
            }
        }
    }

    pub fn is_trivial(&self) -> bool {
        match &self.kind {
            Kind::Pen(points) => points.len() < 2,
            Kind::Arrow(a, b) | Kind::Rect(a, b) | Kind::Ellipse(a, b) => a.distance(*b) < 3.0,
        }
    }

    /// Live preview in the overlay.
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
/// `scale` is image pixels per window point (2.0 on Retina).
pub fn render(base: &RgbaImage, shapes: &[Shape], scale: f32, selection: Rect) -> RgbaImage {
    let (x, y, w, h) = pixel_rect(selection, scale, base.width(), base.height());
    let mut cropped = imageops::crop_imm(base, x, y, w, h).to_image();
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
        let out = render(&base, &[], 2.0, Rect::from_min_max(pos2(10.0, 10.0), pos2(40.0, 30.0)));
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
        let out = render(&base, &[shape], 2.0, Rect::from_min_max(pos2(10.0, 10.0), pos2(90.0, 90.0)));
        assert_eq!(out.dimensions(), (160, 160));
        assert_eq!(out.get_pixel(20, 80).0, [255, 0, 0, 255], "left edge is red");
        assert_eq!(out.get_pixel(80, 80).0, [255, 255, 255, 255], "inside stays white");
        assert_eq!(out.get_pixel(5, 5).0, [255, 255, 255, 255], "outside stays white");
    }

    #[test]
    fn shift_keeps_shapes_square() {
        let mut s = Shape::start(Tool::Ellipse, pos2(0.0, 0.0), Color32::RED, 2.0).unwrap();
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
    fn arrow_head_tip_is_the_end_point() {
        let (shaft_end, head) = arrow_geometry(pos2(0.0, 0.0), pos2(100.0, 0.0), 4.0);
        assert_eq!(head[0], pos2(100.0, 0.0));
        assert!(shaft_end.x < 100.0 && shaft_end.x > 80.0);
    }
}
