//! Adjusting a finished selection that was kept open by holding ⌘: drag inside it to move it,
//! or drag an edge or one of the eight handles to resize it. Pure geometry, no egui state.

use eframe::egui::{CursorIcon, Pos2, Rect, Vec2, pos2, vec2};

/// Side of the square drawn at each handle, in points.
pub const HANDLE_SIZE: f32 = 8.0;
/// How far from an edge (or handle) a press still grabs it, in points.
const REACH: f32 = 7.0;
/// A resize never makes the selection smaller than this.
const MIN_SIZE: f32 = 4.0;

/// Which edges of the selection follow the pointer. All four means a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grip {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Grip {
    pub const MOVE: Grip = Grip { left: true, right: true, top: true, bottom: true };

    /// What pressing at `p` grabs: an edge, a corner, the whole selection (inside), or nothing
    /// (outside, where a press starts a new selection instead).
    pub fn at(rect: Rect, p: Pos2) -> Option<Grip> {
        if !rect.expand(REACH).contains(p) {
            return None;
        }
        // On a small selection both edges can be in reach: take the nearer one.
        let (dl, dr) = ((p.x - rect.min.x).abs(), (p.x - rect.max.x).abs());
        let (dt, db) = ((p.y - rect.min.y).abs(), (p.y - rect.max.y).abs());
        let left = dl <= REACH && dl <= dr;
        let right = dr <= REACH && !left;
        let top = dt <= REACH && dt <= db;
        let bottom = db <= REACH && !top;
        let grip = Grip { left, right, top, bottom };
        if left || right || top || bottom {
            Some(grip)
        } else {
            rect.contains(p).then_some(Grip::MOVE)
        }
    }

    /// `start` with the grabbed edges moved by `delta`, kept inside `bounds`. A move keeps the
    /// size and stops at the screen edge; a resize can cross the opposite edge and flips over.
    pub fn apply(self, start: Rect, delta: Vec2, bounds: Rect) -> Rect {
        if self == Grip::MOVE {
            let dx = delta.x.max(bounds.min.x - start.min.x).min(bounds.max.x - start.max.x);
            let dy = delta.y.max(bounds.min.y - start.min.y).min(bounds.max.y - start.max.y);
            return start.translate(vec2(dx, dy));
        }
        let (mut a, mut b) = (start.min, start.max);
        if self.left {
            a.x += delta.x;
        }
        if self.right {
            b.x += delta.x;
        }
        if self.top {
            a.y += delta.y;
        }
        if self.bottom {
            b.y += delta.y;
        }
        let r = Rect::from_two_pos(a, b).intersect(bounds);
        Rect::from_min_size(r.min, r.size().max(Vec2::splat(MIN_SIZE)))
    }

    pub fn cursor(self, dragging: bool) -> CursorIcon {
        if self == Grip::MOVE {
            return if dragging { CursorIcon::Grabbing } else { CursorIcon::Grab };
        }
        let horizontal = self.left || self.right;
        let vertical = self.top || self.bottom;
        match (horizontal, vertical) {
            (true, false) => CursorIcon::ResizeHorizontal,
            (false, true) => CursorIcon::ResizeVertical,
            _ if (self.left && self.top) || (self.right && self.bottom) => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        }
    }
}

/// Centres of the eight handles: four corners, then the four edge midpoints.
pub fn handles(r: Rect) -> [Pos2; 8] {
    let c = r.center();
    [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        pos2(c.x, r.min.y),
        pos2(r.max.x, c.y),
        pos2(c.x, r.max.y),
        pos2(r.min.x, c.y),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_max(pos2(100.0, 100.0), pos2(300.0, 200.0))
    }
    fn screen() -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0))
    }

    #[test]
    fn grabs_corners_edges_inside_and_nothing_outside() {
        let r = rect();
        let corner = Grip::at(r, pos2(302.0, 198.0)).unwrap();
        assert_eq!(corner, Grip { left: false, right: true, top: false, bottom: true });
        let edge = Grip::at(r, pos2(200.0, 97.0)).unwrap();
        assert_eq!(edge, Grip { left: false, right: false, top: true, bottom: false });
        assert_eq!(Grip::at(r, pos2(200.0, 150.0)), Some(Grip::MOVE));
        assert_eq!(Grip::at(r, pos2(50.0, 50.0)), None);
        assert_eq!(Grip::at(r, pos2(200.0, 220.0)), None);
    }

    #[test]
    fn small_selection_grabs_the_nearer_edge() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(106.0, 106.0));
        let g = Grip::at(r, pos2(105.0, 101.0)).unwrap();
        assert!(g.right && !g.left && g.top && !g.bottom);
    }

    #[test]
    fn move_keeps_size_and_stays_on_screen() {
        let moved = Grip::MOVE.apply(rect(), vec2(50.0, 20.0), screen());
        assert_eq!(moved, Rect::from_min_max(pos2(150.0, 120.0), pos2(350.0, 220.0)));
        let clamped = Grip::MOVE.apply(rect(), vec2(-500.0, 900.0), screen());
        assert_eq!(clamped, Rect::from_min_max(pos2(0.0, 700.0), pos2(200.0, 800.0)));
    }

    #[test]
    fn resize_moves_only_grabbed_edges_and_flips_past_the_other_side() {
        let right = Grip { left: false, right: true, top: false, bottom: false };
        assert_eq!(
            right.apply(rect(), vec2(40.0, 30.0), screen()),
            Rect::from_min_max(pos2(100.0, 100.0), pos2(340.0, 200.0))
        );
        // Dragging the right edge 250pt left puts it 50pt left of the old left edge.
        assert_eq!(
            right.apply(rect(), vec2(-250.0, 0.0), screen()),
            Rect::from_min_max(pos2(50.0, 100.0), pos2(100.0, 200.0))
        );
        let top_left = Grip { left: true, right: false, top: true, bottom: false };
        assert_eq!(
            top_left.apply(rect(), vec2(-200.0, -200.0), screen()),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(300.0, 200.0))
        );
    }

    #[test]
    fn cursors_match_the_grip() {
        assert_eq!(Grip::MOVE.cursor(false), CursorIcon::Grab);
        let nw = Grip { left: true, right: false, top: true, bottom: false };
        assert_eq!(nw.cursor(false), CursorIcon::ResizeNwSe);
        let ne = Grip { left: false, right: true, top: true, bottom: false };
        assert_eq!(ne.cursor(false), CursorIcon::ResizeNeSw);
    }
}
