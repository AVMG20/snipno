//! The snipno logo, drawn in code so the app icon and the menu bar icon share one source:
//! a white pointy-top hexagon holding a crop marquee (four corner brackets).
//! - `app_icon`: on a black rounded tile, for the .app bundle.
//! - `tray_icon`: just the glyph in black, used as a macOS template image (tinted by the system).

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path, PathBuilder,
    Pixmap, Point, SpreadMode, Stroke, Transform,
};

pub fn app_icon(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("non-zero icon size");
    let s = size as f32;

    // macOS icon grid: the tile is 824/1024 of the canvas with a ~185/1024 corner radius.
    let inset = s * 100.0 / 1024.0;
    let tile = rounded_rect(inset, inset, s - 2.0 * inset, s - 2.0 * inset, s * 185.0 / 1024.0);
    let paint = Paint {
        anti_alias: true,
        shader: LinearGradient::new(
            Point::from_xy(0.0, inset),
            Point::from_xy(0.0, s - inset),
            vec![
                GradientStop::new(0.0, Color::from_rgba8(44, 44, 46, 255)),
                GradientStop::new(1.0, Color::from_rgba8(8, 8, 9, 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .expect("valid gradient"),
        ..Default::default()
    };
    if let Some(tile) = tile {
        pm.fill_path(&tile, &paint, FillRule::Winding, Transform::identity(), None);
    }

    draw_glyph(&mut pm, s / 2.0, s / 2.0, s * 0.30, s * 0.042, Color::WHITE);
    pm
}

pub fn tray_icon(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("non-zero icon size");
    let s = size as f32;
    draw_glyph(&mut pm, s / 2.0, s / 2.0, s * 0.46, s * 0.085, Color::BLACK);
    pm
}

/// Hexagon of circumradius `r` around (cx, cy), with the crop marquee inside.
fn draw_glyph(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, line: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: line,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };

    let mut hex = PathBuilder::new();
    for i in 0..6 {
        let a = (-90.0 + 60.0 * i as f32).to_radians();
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 { hex.move_to(x, y) } else { hex.line_to(x, y) }
    }
    hex.close();

    // Marquee: a square of half-size `h` whose corners are drawn as L-shaped brackets.
    let h = r * 0.40;
    let arm = h * 0.62;
    let mut marquee = PathBuilder::new();
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        marquee.move_to(cx + sx * h, cy + sy * (h - arm));
        marquee.line_to(cx + sx * h, cy + sy * h);
        marquee.line_to(cx + sx * (h - arm), cy + sy * h);
    }

    for path in [hex.finish(), marquee.finish()].into_iter().flatten() {
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    // Cubic approximation of a quarter circle.
    let k = r * 0.552_284_8;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Straight (non-premultiplied) RGBA for APIs like `tray_icon::Icon::from_rgba`.
pub fn to_rgba(pm: &Pixmap) -> Vec<u8> {
    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

/// `snipno icon <out.png> [size] [--tray]` — used by bundle.sh to build the .icns.
pub fn write_png(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let out = args.first().ok_or("usage: snipno icon <out.png> [size] [--tray]")?;
    let size = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let pm = if args.iter().any(|a| a == "--tray") { tray_icon(size) } else { app_icon(size) };
    xcap::image::RgbaImage::from_raw(size, size, to_rgba(&pm))
        .ok_or("icon buffer size mismatch")?
        .save(out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_has_black_tile_white_glyph_and_transparent_margin() {
        let pm = app_icon(256);
        let px = |x, y| pm.pixel(x, y).unwrap().demultiply();
        assert_eq!(px(2, 2).alpha(), 0, "margin outside the tile is transparent");
        let tile = px(128, 40);
        assert!(tile.alpha() == 255 && tile.red() < 60, "tile is near black");
        // Top vertex of the hexagon: centre - r = 128 - 0.30*256 ≈ 51.
        let vertex = px(128, 52);
        assert!(vertex.red() > 200, "hexagon stroke is white");
    }

    #[test]
    fn tray_icon_is_monochrome_template() {
        let pm = tray_icon(44);
        assert!(pm.pixels().iter().all(|p| {
            let c = p.demultiply();
            c.red() == 0 && c.green() == 0 && c.blue() == 0
        }));
        assert!(pm.pixels().iter().any(|p| p.alpha() > 0));
    }
}
