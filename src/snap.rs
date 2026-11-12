//! Smart selection: what to highlight when hovering in Select mode.
//!
//! Two sources of snap targets:
//! - **Windows** from the window server (front to back), so hovering an app highlights it.
//! - **UI regions** found in the screenshot itself: panels, sidebars, cards and toolbars are
//!   almost always bounded by long, straight colour edges. We extract horizontal and vertical
//!   edge segments once, then for a cursor position find the nearest lines on each side that
//!   actually span the box (so a stray text underline doesn't count).
//!
//! Candidates are ordered smallest first: fine region → coarse region → window → screen.
//! Everything here except [`windows_on_monitor`] is pure and unit tested.

use eframe::egui::{Pos2, Rect, pos2};
use xcap::image::RgbaImage;

/// Minimum luminance step (0–255) that counts as an edge. Low enough for subtle dark-mode
/// borders (#1e1e1e vs #2a2a2a), high enough to ignore gradients and compression noise.
const EDGE_THRESHOLD: u8 = 9;
/// Anti-aliasing can break a line for a pixel or two; bridge gaps up to this many pixels.
const GAP: u32 = 2;
/// Lines shorter than this (in points) are ignored. Long enough that words, icons, buttons and
/// input fields never bound a region, so the smallest target is a card-sized block.
const FINE_MIN_LEN_PT: f32 = 64.0;
/// Only lines at least this long (in points) bound "coarse" regions such as whole panels.
const COARSE_MIN_LEN_PT: f32 = 200.0;
/// A side only counts if its line covers this fraction of the box edge; real borders cover
/// nearly all of it, rows of text and partial separators don't.
const MIN_COVERAGE: f32 = 0.75;
/// Regions smaller than this (points) aren't worth snapping to: think cards, not buttons.
const MIN_REGION_PT: (f32, f32) = (80.0, 48.0);
/// Detected regions sit just inside their border; grow them by this much (points) so the
/// border itself and a hair of breathing room end up in the capture.
const REGION_PAD_PT: f32 = 2.0;
/// Candidates within this many points of each other on every side count as the same target,
/// so each scroll step visibly grows the selection.
const SAME_TOLERANCE_PT: f32 = 8.0;

/// Pixel-space box: [left, right) × [top, bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl PxRect {
    fn width(&self) -> u32 {
        self.right.saturating_sub(self.left)
    }
    fn height(&self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }
    fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// Edge segments: `rows[y]` holds horizontal runs `(x0, x1)` on the boundary between pixel
/// rows y-1 and y; `cols[x]` holds vertical runs `(y0, y1)` between columns x-1 and x.
pub struct Lines {
    rows: Vec<Vec<(u32, u32)>>,
    cols: Vec<Vec<(u32, u32)>>,
}

impl Lines {
    pub fn detect(img: &RgbaImage, min_len: u32) -> Lines {
        let (w, h) = img.dimensions();
        let (wu, hu) = (w as usize, h as usize);
        let luma: Vec<u8> = img
            .pixels()
            .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
            .collect();
        let edge = |a: u8, b: u8| a.abs_diff(b) >= EDGE_THRESHOLD;

        let mut rows = vec![Vec::new(); hu];
        for y in 1..hu {
            let (prev, cur) = (&luma[(y - 1) * wu..y * wu], &luma[y * wu..(y + 1) * wu]);
            let mut run: Option<(u32, u32)> = None;
            for x in 0..wu {
                if !edge(prev[x], cur[x]) {
                    continue;
                }
                let x = x as u32;
                run = match run {
                    Some((s, last)) if x - last <= GAP + 1 => Some((s, x)),
                    Some((s, last)) => {
                        push_run(&mut rows[y], s, last, min_len);
                        Some((x, x))
                    }
                    None => Some((x, x)),
                };
            }
            if let Some((s, last)) = run {
                push_run(&mut rows[y], s, last, min_len);
            }
        }

        // Vertical runs: walk rows (cache friendly) and keep one open run per column.
        let mut cols = vec![Vec::new(); wu];
        let mut open: Vec<Option<(u32, u32)>> = vec![None; wu];
        for y in 0..hu {
            let row = &luma[y * wu..(y + 1) * wu];
            for x in 1..wu {
                if !edge(row[x - 1], row[x]) {
                    continue;
                }
                let y = y as u32;
                open[x] = match open[x] {
                    Some((s, last)) if y - last <= GAP + 1 => Some((s, y)),
                    Some((s, last)) => {
                        push_run(&mut cols[x], s, last, min_len);
                        Some((y, y))
                    }
                    None => Some((y, y)),
                };
            }
        }
        for (x, run) in open.into_iter().enumerate() {
            if let Some((s, last)) = run {
                push_run(&mut cols[x], s, last, min_len);
            }
        }
        Lines { rows, cols }
    }

    /// Same lines, keeping only those at least `min_len` long.
    fn longer_than(&self, min_len: u32) -> Lines {
        let keep = |v: &Vec<Vec<(u32, u32)>>| {
            v.iter()
                .map(|segs| segs.iter().copied().filter(|(a, b)| b - a + 1 >= min_len).collect())
                .collect()
        };
        Lines { rows: keep(&self.rows), cols: keep(&self.cols) }
    }

    /// The box around (x, y) bounded by the nearest lines that span it, inside `bounds`.
    pub fn region_at(&self, x: u32, y: u32, bounds: PxRect) -> Option<PxRect> {
        if !bounds.contains(x, y) {
            return None;
        }
        // Nearest line on each side that crosses the cursor's row/column.
        let mut r = PxRect {
            top: self.row_up(y, x, bounds.top),
            bottom: self.row_down(y + 1, x, bounds.bottom),
            left: self.col_left(x, y, bounds.left),
            right: self.col_right(x + 1, y, bounds.right),
        };
        // Push out any side whose line doesn't really span the box (a short separator or a
        // text underline), until every side is a real edge or hits the bounds.
        for _ in 0..12 {
            let mut changed = false;
            if r.top > bounds.top && coverage(&self.rows[r.top as usize], r.left, r.right) < MIN_COVERAGE {
                r.top = self.row_up(r.top - 1, x, bounds.top);
                changed = true;
            }
            if r.bottom < bounds.bottom
                && coverage(&self.rows[r.bottom as usize], r.left, r.right) < MIN_COVERAGE
            {
                r.bottom = self.row_down(r.bottom + 1, x, bounds.bottom);
                changed = true;
            }
            if r.left > bounds.left && coverage(&self.cols[r.left as usize], r.top, r.bottom) < MIN_COVERAGE {
                r.left = self.col_left(r.left - 1, y, bounds.left);
                changed = true;
            }
            if r.right < bounds.right
                && coverage(&self.cols[r.right as usize], r.top, r.bottom) < MIN_COVERAGE
            {
                r.right = self.col_right(r.right + 1, y, bounds.right);
                changed = true;
            }
            if !changed {
                break;
            }
        }
        (r != bounds).then_some(r)
    }

    fn row_up(&self, from: u32, x: u32, limit: u32) -> u32 {
        (limit + 1..=from.min(self.rows.len() as u32 - 1))
            .rev()
            .find(|&y| covers(&self.rows[y as usize], x))
            .unwrap_or(limit)
    }
    fn row_down(&self, from: u32, x: u32, limit: u32) -> u32 {
        (from..limit.min(self.rows.len() as u32))
            .find(|&y| covers(&self.rows[y as usize], x))
            .unwrap_or(limit)
    }
    fn col_left(&self, from: u32, y: u32, limit: u32) -> u32 {
        (limit + 1..=from.min(self.cols.len() as u32 - 1))
            .rev()
            .find(|&x| covers(&self.cols[x as usize], y))
            .unwrap_or(limit)
    }
    fn col_right(&self, from: u32, y: u32, limit: u32) -> u32 {
        (from..limit.min(self.cols.len() as u32))
            .find(|&x| covers(&self.cols[x as usize], y))
            .unwrap_or(limit)
    }
}

fn push_run(segs: &mut Vec<(u32, u32)>, start: u32, last: u32, min_len: u32) {
    if last - start + 1 >= min_len {
        segs.push((start, last));
    }
}

fn covers(segs: &[(u32, u32)], p: u32) -> bool {
    segs.iter().any(|&(a, b)| a <= p && p <= b)
}

/// Fraction of [a, b) covered by the segments.
fn coverage(segs: &[(u32, u32)], a: u32, b: u32) -> f32 {
    if b <= a {
        return 0.0;
    }
    let covered: u32 = segs
        .iter()
        .map(|&(s, e)| (e + 1).min(b).saturating_sub(s.max(a)))
        .sum();
    covered as f32 / (b - a) as f32
}

/// Everything the overlay needs to answer "what's under the cursor?".
pub struct Snap {
    fine: Lines,
    coarse: Lines,
    /// Window rects in overlay points, front to back.
    windows: Vec<Rect>,
    /// Image pixels per point.
    scale: f32,
    screen: Rect,
}

impl Snap {
    pub fn new(img: &RgbaImage, scale: f32, screen: Rect, windows: Vec<Rect>) -> Snap {
        let fine = Lines::detect(img, (FINE_MIN_LEN_PT * scale) as u32);
        let coarse = fine.longer_than((COARSE_MIN_LEN_PT * scale) as u32);
        Snap { fine, coarse, windows, scale, screen }
    }

    /// Snap targets under `p` (points), smallest first, always ending with the whole screen.
    pub fn candidates(&self, p: Pos2) -> Vec<Rect> {
        let mut out: Vec<Rect> = Vec::new();
        let window = self.windows.iter().find(|w| w.contains(p)).map(|w| w.intersect(self.screen));
        let bounds_pt = window.unwrap_or(self.screen);
        let bounds = self.to_px(bounds_pt);
        let (x, y) = ((p.x * self.scale) as u32, (p.y * self.scale) as u32);

        for lines in [&self.fine, &self.coarse] {
            if let Some(r) = lines.region_at(x, y, bounds) {
                let min = (MIN_REGION_PT.0 * self.scale, MIN_REGION_PT.1 * self.scale);
                if r.width() as f32 >= min.0 && r.height() as f32 >= min.1 {
                    push_distinct(&mut out, self.to_pt(r).expand(REGION_PAD_PT).intersect(bounds_pt));
                }
            }
        }
        if let Some(w) = window {
            push_distinct(&mut out, w);
        }
        push_distinct(&mut out, self.screen);
        out
    }

    fn to_px(&self, r: Rect) -> PxRect {
        let s = self.scale;
        PxRect {
            left: (r.min.x * s).round().max(0.0) as u32,
            top: (r.min.y * s).round().max(0.0) as u32,
            right: (r.max.x * s).round().max(0.0) as u32,
            bottom: (r.max.y * s).round().max(0.0) as u32,
        }
    }

    fn to_pt(&self, r: PxRect) -> Rect {
        let s = self.scale;
        Rect::from_min_max(
            pos2(r.left as f32 / s, r.top as f32 / s),
            pos2(r.right as f32 / s, r.bottom as f32 / s),
        )
    }
}

/// Merge near-duplicates so scrolling always visibly changes size. A larger target replaces a
/// near-identical smaller one, so hovering a window whose content is inset by a border picks
/// the window itself.
fn push_distinct(out: &mut Vec<Rect>, r: Rect) {
    let t = SAME_TOLERANCE_PT;
    let same = |a: &Rect| {
        (a.min.x - r.min.x).abs() <= t
            && (a.min.y - r.min.y).abs() <= t
            && (a.max.x - r.max.x).abs() <= t
            && (a.max.y - r.max.y).abs() <= t
    };
    match out.iter_mut().find(|a| same(a)) {
        Some(a) => *a = a.union(r),
        None => out.push(r),
    }
}

/// Owners of system chrome that shouldn't be snap targets.
const SKIP_OWNERS: [&str; 9] = [
    "Window Server",
    "Dock",
    "Control Center",
    "SystemUIServer",
    "Notification Center",
    "NotificationCenter",
    "Spotlight",
    "WindowManager",
    "TextInputMenuAgent",
];

/// Visible windows on the monitor at `origin`/`screen` (global points), converted to overlay
/// points, front to back. Excludes our own windows and system chrome.
pub fn windows_on_monitor(origin: Pos2, screen: Rect) -> Vec<Rect> {
    let own_pid = std::process::id();
    let Ok(windows) = xcap::Window::all() else { return Vec::new() };
    windows
        .iter()
        .filter(|w| w.pid().is_ok_and(|pid| pid != own_pid))
        .filter(|w| !w.is_minimized().unwrap_or(false))
        .filter(|w| w.app_name().is_ok_and(|name| !SKIP_OWNERS.contains(&name.as_str())))
        .filter_map(|w| {
            let (x, y) = (w.x().ok()? as f32, w.y().ok()? as f32);
            let (width, height) = (w.width().ok()? as f32, w.height().ok()? as f32);
            (width >= 60.0 && height >= 40.0).then(|| {
                Rect::from_min_size(pos2(x - origin.x, y - origin.y), eframe::egui::vec2(width, height))
            })
        })
        .filter(|r| r.intersects(screen))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcap::image::Rgba;

    const BG: Rgba<u8> = Rgba([250, 250, 250, 255]);
    const BORDER: Rgba<u8> = Rgba([200, 200, 205, 255]);

    fn fill(img: &mut RgbaImage, l: u32, t: u32, r: u32, b: u32, c: Rgba<u8>) {
        for y in t..b {
            for x in l..r {
                img.put_pixel(x, y, c);
            }
        }
    }

    /// 1px bordered box with interior [l+1, r-1) × [t+1, b-1).
    fn bordered(img: &mut RgbaImage, l: u32, t: u32, r: u32, b: u32) {
        fill(img, l, t, r, b, BORDER);
        fill(img, l + 1, t + 1, r - 1, b - 1, BG);
    }

    fn screen(w: u32, h: u32) -> PxRect {
        PxRect { left: 0, top: 0, right: w, bottom: h }
    }

    #[test]
    fn finds_the_bordered_panel_around_the_cursor() {
        let mut img = RgbaImage::from_pixel(400, 300, BG);
        bordered(&mut img, 50, 40, 250, 200);
        let lines = Lines::detect(&img, 40);
        let r = lines.region_at(120, 100, screen(400, 300)).unwrap();
        assert_eq!(r, PxRect { left: 51, top: 41, right: 249, bottom: 199 });
    }

    #[test]
    fn short_separators_and_text_do_not_bound_the_region() {
        let mut img = RgbaImage::from_pixel(400, 300, BG);
        bordered(&mut img, 50, 40, 250, 200);
        // A short underline (30px) right above the cursor and "text" made of tiny blocks.
        fill(&mut img, 100, 90, 130, 91, BORDER);
        for i in 0..10 {
            fill(&mut img, 70 + i * 8, 120, 74 + i * 8, 128, Rgba([30, 30, 30, 255]));
        }
        let lines = Lines::detect(&img, 40);
        let r = lines.region_at(110, 110, screen(400, 300)).unwrap();
        assert_eq!((r.top, r.bottom), (41, 199));
    }

    #[test]
    fn a_line_that_only_clips_the_box_is_skipped() {
        let mut img = RgbaImage::from_pixel(400, 300, BG);
        bordered(&mut img, 50, 40, 350, 260);
        // 60px line crossing the cursor column but far narrower than the panel.
        fill(&mut img, 170, 120, 230, 121, BORDER);
        let lines = Lines::detect(&img, 40);
        let r = lines.region_at(200, 200, screen(400, 300)).unwrap();
        assert_eq!(r.top, 41, "the narrow line covers <75% of the panel width");
    }

    #[test]
    fn nested_panels_give_fine_then_coarse_then_window_then_screen() {
        // 2x "Retina" image of a 300x200pt screen with a sidebar split and a card inside.
        let (w, h) = (600, 400);
        let mut img = RgbaImage::from_pixel(w, h, BG);
        fill(&mut img, 160, 0, 162, h, BORDER); // sidebar divider, full height
        bordered(&mut img, 220, 60, 520, 260); // card: 150x100pt, lines 300px < coarse 400px
        let window = Rect::from_min_max(pos2(0.0, 0.0), pos2(300.0, 200.0));
        let snap = Snap::new(&img, 2.0, Rect::from_min_max(Pos2::ZERO, pos2(300.0, 200.0)), vec![window]);

        let c = snap.candidates(pos2(150.0, 50.0)); // inside the card
        assert_eq!(c.len(), 3, "{c:?}"); // card, right pane, screen (= window here)
        // Card interior is 110.5..259.5pt, padded by 2pt to take in its border.
        assert!((c[0].min.x - 108.5).abs() < 0.6 && (c[0].max.x - 261.5).abs() < 0.6, "{:?}", c[0]);
        assert!((c[1].min.x - 79.0).abs() < 0.6 && c[1].max.x == 300.0, "{:?}", c[1]);
        assert_eq!(c.last(), Some(&Rect::from_min_max(Pos2::ZERO, pos2(300.0, 200.0))));
    }

    #[test]
    fn hovering_a_window_without_structure_snaps_to_the_window() {
        let img = RgbaImage::from_pixel(400, 300, BG);
        let screen_pt = Rect::from_min_max(Pos2::ZERO, pos2(400.0, 300.0));
        let window = Rect::from_min_max(pos2(20.0, 20.0), pos2(200.0, 150.0));
        let snap = Snap::new(&img, 1.0, screen_pt, vec![window]);
        assert_eq!(snap.candidates(pos2(100.0, 100.0)), vec![window, screen_pt]);
        assert_eq!(snap.candidates(pos2(300.0, 250.0)), vec![screen_pt]);
    }

    #[test]
    fn small_controls_snap_to_the_card_around_them() {
        let mut img = RgbaImage::from_pixel(400, 300, BG);
        bordered(&mut img, 50, 40, 350, 260); // card
        bordered(&mut img, 100, 100, 160, 128); // 60x28 button inside it
        let screen_pt = Rect::from_min_max(Pos2::ZERO, pos2(400.0, 300.0));
        let snap = Snap::new(&img, 1.0, screen_pt, vec![]);
        let card = Rect::from_min_max(pos2(49.0, 39.0), pos2(351.0, 261.0));
        assert_eq!(snap.candidates(pos2(120.0, 110.0)), vec![card, screen_pt]);
    }

    #[test]
    fn near_identical_targets_merge_into_the_larger_one() {
        let mut out = Vec::new();
        let inner = Rect::from_min_max(pos2(14.0, 30.0), pos2(194.0, 146.0));
        let window = Rect::from_min_max(pos2(10.0, 24.0), pos2(200.0, 150.0));
        push_distinct(&mut out, inner);
        push_distinct(&mut out, window);
        assert_eq!(out, vec![window]);
    }

    #[test]
    fn coverage_counts_overlap_only() {
        assert_eq!(coverage(&[(0, 49)], 0, 100), 0.5);
        assert_eq!(coverage(&[(0, 9), (50, 99)], 0, 100), 0.6);
        assert_eq!(coverage(&[], 0, 100), 0.0);
    }
}
