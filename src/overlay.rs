//! `snipno capture`: freezes the screen under the cursor, shows it full-screen with a drawing
//! toolbar on top, and copies the selected region (annotations included) to the clipboard. A
//! copy also goes to the screenshots folder if saving is on.
//!
//! `snipno capture --text` is the same overlay without the drawing tools: the selected area's
//! text is read with Vision (`src/ocr.rs`) and copied as plain text.

use crate::{
    adjust::{self, Grip},
    annotate::{self, Kind, Mosaic, Shape, Tool},
    config::{self, Save},
    hotkey, macos, ocr,
    snap::{self, Snap},
    store, theme,
    toolbar::{self, Action},
};
use eframe::egui::{
    self, Color32, CursorIcon, Event, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, TextureFilter,
    TextureHandle, TextureOptions, ViewportCommand, pos2, vec2,
};
use std::{
    borrow::Cow,
    error::Error,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::Instant,
};
use xcap::{Monitor, image::RgbaImage};

/// How long the "drag to capture" hint stays up before fading.
const HINT_SECS: f32 = 3.5;

/// `monitor`: display id to cover (the daemon starts one overlay per display); `None` means the
/// display under the cursor. `text`: copy the text in the selection instead of the image.
pub fn run(monitor: Option<u32>, text: bool) -> Result<(), Box<dyn Error>> {
    let monitor = match monitor {
        Some(id) => Monitor::all()?
            .into_iter()
            .find(|m| m.id().ok() == Some(id))
            .ok_or_else(|| format!("display {id} not found"))?,
        None => monitor_under_cursor()?,
    };
    let (x, y) = (monitor.x()? as f32, monitor.y()? as f32);
    let (w, h) = (monitor.width()? as f32, monitor.height()? as f32);
    // Capture before our window exists so it isn't in the shot.
    let shot = Arc::new(monitor.capture_image()?);
    // Seed the hover position only on the display the cursor is actually on.
    let cursor = macos::cursor_position()
        .map(|(cx, cy)| pos2(cx as f32 - x, cy as f32 - y))
        .filter(|p| Rect::from_min_size(Pos2::ZERO, egui::vec2(w, h)).contains(*p));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("snipno")
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_position([x, y])
            .with_inner_size([w, h])
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_active(true),
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                builder
                    .with_activation_policy(ActivationPolicy::Accessory)
                    .with_activate_ignoring_other_apps(true);
            }
            let _ = builder;
        })),
        persist_window: false,
        // Don't wait for vsync. Above the menu bar (NSScreenSaverWindowLevel) a vsynced 5K
        // surface with two drawables runs at 30 fps and shows a mouse sample ~70 ms later:
        // the frame blocks a whole 33 ms in nextDrawable *after* egui read the pointer.
        // Without display sync it's ~30-40 ms. WindowServer still composites the window, so it
        // shouldn't tear. We only repaint on input, so it doesn't spin while idle.
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            surface: eframe::egui_wgpu::SurfaceConfig {
                present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: Some(1),
            },
            ..Default::default()
        },
        ..Default::default()
    };

    // Saving (and reading text) runs off the UI thread so the overlay closes at once; the
    // process waits for it before it exits.
    let jobs: Jobs = Arc::default();
    if text && cursor.is_some() {
        // Load Vision while you pick an area, so the read itself is quick. Only on the display
        // you're on; one load per display at once would just compete.
        if let Ok(mut jobs) = jobs.lock() {
            jobs.push(std::thread::spawn(ocr::warm_up));
        }
    }
    let overlay_jobs = jobs.clone();
    let result = eframe::run_native(
        "snipno-capture",
        options,
        Box::new(move |cc| {
            macos::float_above_everything(cc);
            theme::apply(&cc.egui_ctx);
            // Find snap targets in the background so the overlay appears instantly.
            let (tx, rx) = mpsc::channel();
            let (img, ctx) = (shot.clone(), cc.egui_ctx.clone());
            std::thread::spawn(move || {
                let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(w, h));
                let windows = snap::windows_on_monitor(pos2(x, y), screen);
                let _ = tx.send(Snap::new(&img, img.width() as f32 / w, screen, windows));
                ctx.request_repaint();
            });
            let overlay = Overlay::new(&cc.egui_ctx, shot, pos2(x, y), cursor, rx, config::load(), overlay_jobs, text);
            Ok(Box::new(overlay))
        }),
    );
    // Tell the daemon we're done, so it closes the other displays' overlays now rather than
    // after the save.
    if result.is_ok() {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{}", crate::daemon::MSG_CAPTURE_DONE).and_then(|()| out.flush());
    }
    let pending: Vec<_> = jobs.lock().map(|mut j| j.drain(..).collect()).unwrap_or_default();
    for job in pending {
        let _ = job.join();
    }
    result?;
    Ok(())
}

type Jobs = Arc<Mutex<Vec<JoinHandle<()>>>>;

fn monitor_under_cursor() -> Result<Monitor, Box<dyn Error>> {
    if let Some((cx, cy)) = macos::cursor_position()
        && let Ok(m) = Monitor::from_point(cx as i32, cy as i32)
    {
        return Ok(m);
    }
    let monitors = Monitor::all()?;
    let primary = monitors.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
    monitors.into_iter().nth(primary).ok_or_else(|| "no monitors found".into())
}

struct Overlay {
    shot: Arc<RgbaImage>,
    texture: TextureHandle,
    origin: Pos2,
    tool: Tool,
    color: Color32,
    width: f32,
    shapes: Vec<Shape>,
    drawing: Option<Shape>,
    /// `drawing` with its end pushed slightly ahead of the cursor, for display only.
    preview: Option<Shape>,
    selecting: Option<(Pos2, Pos2)>,
    /// A selection kept open because ⌘ was held when it was made; copied when ⌘ is let go.
    adjusting: Option<Adjusting>,
    opened: Instant,
    frame: u32,
    /// The cursor was on this display when the overlay opened.
    cursor_here: bool,
    /// A focus request is outstanding for the current pointer visit.
    asked_focus: bool,
    closing: bool,
    debug: Option<FrameStats>,
    tool_keys: config::ToolKeys,
    snapshot: Option<crate::snapshot::Snapshot>,
    hover: Hover,
    /// Saving to the screenshots folder, when it's on.
    save: Option<Save>,
    jobs: Jobs,
    /// Block-averaged screenshot for pixelate boxes, with its texture; made on first use.
    mosaic: Option<(Mosaic, TextureHandle)>,
    /// Text capture: copy the text in the selection, no drawing tools.
    text: bool,
    /// The area whose text is being read, and where the result arrives.
    reading: Option<(Rect, mpsc::Receiver<Result<String, String>>)>,
    /// A message shown under the toolbar since the given moment, e.g. that no text was found.
    notice: Option<(String, Instant)>,
}

struct Adjusting {
    rect: Rect,
    /// While dragging: what was grabbed, where the press was, and the selection at that moment.
    grab: Option<(Grip, Pos2, Rect)>,
}

/// Smart selection state: what's highlighted under the cursor in Select mode.
struct Hover {
    snap: Option<Snap>,
    pending: Option<mpsc::Receiver<Snap>>,
    /// Last known cursor position; seeded from the OS so it works before the mouse moves.
    pos: Option<Pos2>,
    /// Cursor position (rounded) the candidates were computed for.
    key: Option<(i32, i32)>,
    /// Snap targets under the cursor, smallest first.
    candidates: Vec<Rect>,
    /// Index into `candidates`; scrolling moves it.
    level: usize,
    scroll: f32,
}

impl Hover {
    fn update(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.pending
            && let Ok(snap) = rx.try_recv()
        {
            self.snap = Some(snap);
            self.pending = None;
            self.key = None;
        }
        let (hover, gone) =
            ctx.input(|i| (i.pointer.hover_pos(), i.events.iter().any(|e| matches!(e, Event::PointerGone))));
        if let Some(p) = hover {
            self.pos = Some(p);
        } else if gone {
            // The mouse moved to another display: drop this one's highlight.
            self.pos = None;
            self.key = None;
            self.candidates.clear();
        }
        let (Some(snap), Some(p)) = (&self.snap, self.pos) else { return };
        let key = (p.x.round() as i32, p.y.round() as i32);
        if self.key != Some(key) {
            let candidates = snap.candidates(p);
            if candidates.first() != self.candidates.first() {
                self.level = 0;
            }
            self.level = self.level.min(candidates.len().saturating_sub(1));
            self.candidates = candidates;
            self.key = Some(key);
        }
        // Scroll up to grow to the enclosing area, down to shrink back.
        self.scroll += ctx.input(|i| {
            i.events
                .iter()
                .map(|e| match e {
                    Event::MouseWheel { unit, delta, .. } => match unit {
                        egui::MouseWheelUnit::Point => delta.y,
                        egui::MouseWheelUnit::Line => delta.y * 24.0,
                        egui::MouseWheelUnit::Page => delta.y * 400.0,
                    },
                    _ => 0.0,
                })
                .sum::<f32>()
        });
        if self.scroll.abs() >= 24.0 {
            if self.scroll > 0.0 {
                self.level = (self.level + 1).min(self.candidates.len().saturating_sub(1));
            } else {
                self.level = self.level.saturating_sub(1);
            }
            self.scroll = 0.0;
        }
    }

    fn highlight(&self) -> Option<Rect> {
        self.candidates.get(self.level).copied()
    }
}

/// Darken the screen except for `sel`.
fn dim_outside(painter: &egui::Painter, screen: Rect, sel: Option<Rect>) {
    let dim = Color32::from_black_alpha(96);
    let Some(sel) = sel else {
        painter.rect_filled(screen, 0.0, dim);
        return;
    };
    for r in [
        Rect::from_min_max(screen.min, pos2(screen.max.x, sel.min.y)),
        Rect::from_min_max(pos2(screen.min.x, sel.max.y), screen.max),
        Rect::from_min_max(pos2(screen.min.x, sel.min.y), pos2(sel.min.x, sel.max.y)),
        Rect::from_min_max(pos2(sel.max.x, sel.min.y), pos2(screen.max.x, sel.max.y)),
    ] {
        painter.rect_filled(r, 0.0, dim);
    }
}

/// Slim dashed outline along the inside or outside edge of `r`, snapped to whole physical pixels
/// so the dashes stay crisp on both Retina and 1x displays.
fn dashed_outline(painter: &egui::Painter, r: Rect, kind: StrokeKind) {
    const WIDTH: f32 = 1.0;
    const DASH: f32 = 4.0;
    const GAP: f32 = 3.0;
    let ppp = painter.pixels_per_point();
    let r = match kind {
        StrokeKind::Inside => r.shrink(WIDTH / 2.0),
        StrokeKind::Outside => r.expand(WIDTH / 2.0),
        StrokeKind::Middle => r,
    };
    // A stroke an odd number of pixels wide is centred on a pixel centre, an even one on an edge.
    let odd = (WIDTH * ppp).round() as i32 % 2 == 1;
    let snap = |v: f32| {
        let px = v * ppp;
        let px = if odd { (px - 0.5).round() + 0.5 } else { px.round() };
        px / ppp
    };
    let (l, t, rr, b) = (snap(r.min.x), snap(r.min.y), snap(r.max.x), snap(r.max.y));
    let path = [pos2(l, t), pos2(rr, t), pos2(rr, b), pos2(l, b), pos2(l, t)];
    painter.extend(egui::Shape::dashed_line(&path, Stroke::new(WIDTH, theme::ACCENT), DASH, GAP));
}

/// Even with vsync off, the screen shows a pointer sample ~2 frames late while the hardware
/// cursor is instant, so straight shapes visibly trail it. Draw their live end where the cursor
/// will be ~one frame from now. Display only: the real shape keeps the real point, so the
/// stored result never overshoots.
fn predicted_preview(shape: &Shape, p: Pos2, velocity: egui::Vec2, square: bool) -> Option<Shape> {
    const LEAD_SECS: f32 = 0.016;
    const MAX_LEAD_PT: f32 = 30.0;
    const MIN_SPEED: f32 = 60.0; // pt/s; slower than this, prediction only adds jitter
    // A marker follows the cursor exactly: leading it would make it jump back on release.
    if matches!(shape.kind, Kind::Pen(_) | Kind::Number(..)) || velocity.length() < MIN_SPEED {
        return None;
    }
    let lead = velocity * LEAD_SECS;
    let lead = if lead.length() > MAX_LEAD_PT { lead.normalized() * MAX_LEAD_PT } else { lead };
    let mut preview = shape.clone();
    preview.drag_to(p + lead, square);
    Some(preview)
}

/// `SNIPNO_DEBUG=1`: log frame rate and pointer samples per second to stderr.
struct FrameStats {
    since: Instant,
    frames: u32,
    moves: u32,
}

impl Overlay {
    #[allow(clippy::too_many_arguments)]
    fn new(
        ctx: &egui::Context,
        shot: Arc<RgbaImage>,
        origin: Pos2,
        cursor: Option<Pos2>,
        snap: mpsc::Receiver<Snap>,
        cfg: config::Config,
        jobs: Jobs,
        text: bool,
    ) -> Self {
        let size = [shot.width() as usize, shot.height() as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, shot.as_raw());
        // Nearest when magnified, so the loupe shows crisp pixels; shown 1:1 otherwise.
        let options = TextureOptions { magnification: TextureFilter::Nearest, ..TextureOptions::LINEAR };
        let texture = ctx.load_texture("screenshot", image, options);
        Self {
            shot,
            texture,
            origin,
            tool: Tool::Select,
            color: toolbar::COLORS[0].0,
            width: toolbar::WIDTHS[1].0,
            shapes: Vec::new(),
            drawing: None,
            preview: None,
            selecting: None,
            adjusting: None,
            opened: Instant::now(),
            frame: 0,
            cursor_here: cursor.is_some(),
            asked_focus: false,
            closing: false,
            debug: std::env::var_os("SNIPNO_DEBUG")
                .map(|_| FrameStats { since: Instant::now(), frames: 0, moves: 0 }),
            snapshot: crate::snapshot::Snapshot::from_env(),
            tool_keys: cfg.tool_keys,
            hover: Hover {
                snap: None,
                pending: Some(snap),
                // SNIPNO_HOVER=x,y pins the cursor, for snapshot checks of the highlight.
                pos: std::env::var("SNIPNO_HOVER")
                    .ok()
                    .and_then(|v| {
                        let (x, y) = v.split_once(',')?;
                        Some(pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
                    })
                    .or(cursor),
                key: None,
                candidates: Vec::new(),
                level: 0,
                scroll: 0.0,
            },
            save: (cfg.save.enabled && !text).then_some(cfg.save),
            jobs,
            mosaic: None,
            text,
            reading: None,
            notice: None,
        }
    }

    fn ensure_mosaic(&mut self, ctx: &egui::Context, screen: Rect) {
        if self.mosaic.is_some() {
            return;
        }
        let mosaic = Mosaic::new(&self.shot, self.scale(screen));
        let size = [mosaic.small.width() as usize, mosaic.small.height() as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, mosaic.small.as_raw());
        let texture = ctx.load_texture("mosaic", image, TextureOptions::NEAREST);
        self.mosaic = Some((mosaic, texture));
    }

    /// What the next numbered marker shows: one more than the markers placed so far.
    fn next_number(&self) -> u32 {
        self.shapes.iter().filter(|s| matches!(s.kind, Kind::Number(..))).count() as u32 + 1
    }

    /// Image pixels per window point (2.0 on Retina).
    fn scale(&self, screen: Rect) -> f32 {
        self.shot.width() as f32 / screen.width().max(1.0)
    }

    fn copy_and_close(&mut self, ctx: &egui::Context, selection: Rect, screen: Rect) {
        if self.text {
            self.start_reading(ctx, selection, screen);
            return;
        }
        let mosaic = self.mosaic.as_ref().map(|(m, _)| m);
        let image = annotate::render(&self.shot, &self.shapes, self.scale(screen), selection, mosaic);
        let (w, h) = image.dimensions();
        let result = arboard::Clipboard::new().and_then(|mut cb| {
            cb.set_image(arboard::ImageData {
                width: w as usize,
                height: h as usize,
                bytes: Cow::Borrowed(image.as_raw()),
            })
        });
        if let Err(e) = result {
            eprintln!("snipno: copying to clipboard failed: {e}");
        }
        if let Some(save) = self.save.clone() {
            let job = std::thread::spawn(move || {
                if let Err(e) = store::save(&image, &save) {
                    eprintln!("snipno: could not save the screenshot: {e}");
                }
            });
            if let Ok(mut jobs) = self.jobs.lock() {
                jobs.push(job);
            }
        }
        self.close(ctx);
    }

    /// Read the text in `selection` on a background thread; [`Self::poll_reading`] copies it.
    fn start_reading(&mut self, ctx: &egui::Context, selection: Rect, screen: Rect) {
        let scale = self.scale(screen);
        let image = annotate::render(&self.shot, &[], scale, selection, None);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        // Joined before the process exits, like saves: never quit with Vision mid-run.
        let job = std::thread::spawn(move || {
            let _ = tx.send(ocr::read(&image, scale));
            ctx.request_repaint();
        });
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.push(job);
        }
        self.adjusting = None;
        self.notice = None;
        self.reading = Some((selection, rx));
    }

    /// Copy the text once it's read and close. With nothing found, say so and stay open so
    /// you can pick another area.
    fn poll_reading(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.reading else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("text recognition stopped".into()),
        };
        self.reading = None;
        let message = match result {
            Ok(text) if !text.trim().is_empty() => {
                match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
                    Ok(()) => return self.close(ctx),
                    Err(e) => format!("Copying failed: {e}  ·"),
                }
            }
            Ok(_) => "No text found there. Select another area  ·".to_string(),
            Err(e) => format!("Couldn't read the text: {e}  ·"),
        };
        eprintln!("snipno: {}", message.trim_end_matches(" ·").trim());
        self.notice = Some((message, Instant::now()));
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.closing = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn handle_keys(&mut self, ctx: &egui::Context, screen: Rect) {
        let keys = &self.tool_keys;
        let (esc, undo, enter, tool) = ctx.input(|i| {
            // Tool shortcuts from the config (S/P/A/R/E unless changed in Settings).
            let tool = i.events.iter().find_map(|e| match e {
                Event::Key { key, pressed: true, repeat: false, modifiers, .. } => toolbar::TOOL_ORDER
                    .into_iter()
                    .find(|t| hotkey::matches(keys.get(*t), *key, *modifiers)),
                _ => None,
            });
            (
                i.key_pressed(Key::Escape),
                i.modifiers.command && i.key_pressed(Key::Z),
                i.key_pressed(Key::Enter),
                tool,
            )
        });
        if esc {
            // First Esc drops a selection held open with ⌘, the next one closes the overlay.
            if self.adjusting.take().is_none() {
                self.close(ctx);
            }
        } else if enter {
            let area = self.adjusting.as_ref().map_or(screen, |a| a.rect);
            self.copy_and_close(ctx, area, screen);
        } else if undo {
            // Mid-drag or with a selection held open, ⌘Z drops that selection (or the shape being
            // dragged) so you can start over; otherwise it removes the last finished drawing.
            if self.selecting.take().is_none()
                && self.drawing.take().is_none()
                && self.adjusting.take().is_none()
            {
                self.shapes.pop();
            }
            self.preview = None;
        } else if let Some(t) = tool
            && !self.text
        {
            self.tool = t;
        }
    }

    /// The area a click would capture: only in Select mode, while not dragging.
    fn highlight(&self) -> Option<Rect> {
        (self.tool == Tool::Select
            && self.selecting.is_none()
            && self.drawing.is_none()
            && self.adjusting.is_none())
            .then(|| self.hover.highlight())
            .flatten()
    }

    fn handle_pointer(&mut self, ui: &egui::Ui, canvas: &egui::Response, screen: Rect) {
        // Every PointerMoved since the last frame, not just the latest position: macOS delivers
        // mouse samples faster than we repaint, and dropping them is what made strokes angular.
        let (press_origin, shift, command, moves) = ui.input(|i| {
            let moves: Vec<Pos2> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::PointerMoved(p) => Some(*p),
                    _ => None,
                })
                .collect();
            (i.pointer.press_origin(), i.modifiers.shift, i.modifiers.command, moves)
        });
        if let Some(stats) = &mut self.debug {
            stats.moves += moves.len() as u32;
        }
        let pos = canvas.interact_pointer_pos();

        // Click (no drag) in Select mode: take the highlighted area, or with ⌘ held keep it open.
        if canvas.clicked()
            && let Some(area) = self.highlight()
        {
            if command {
                self.adjusting = Some(Adjusting { rect: area, grab: None });
            } else {
                self.copy_and_close(ui.ctx(), area, screen);
            }
            return;
        }
        // Click with the marker tool: drop the next number there.
        if canvas.clicked()
            && self.tool == Tool::Number
            && let Some(p) = pos
            && let Some(shape) = Shape::start(Tool::Number, p, self.color, self.width, self.next_number())
        {
            self.shapes.push(shape);
            return;
        }

        if canvas.drag_started()
            && let Some(start) = press_origin.or(pos)
        {
            if self.tool == Tool::Pixelate {
                self.ensure_mosaic(ui.ctx(), screen);
            }
            // Pressing on a held-open selection moves or resizes it; anywhere else starts over.
            let grip = match &self.adjusting {
                Some(a) if self.tool == Tool::Select => Grip::at(a.rect, start),
                _ => None,
            };
            match (grip, self.tool) {
                (Some(grip), _) => {
                    if let Some(a) = &mut self.adjusting {
                        a.grab = Some((grip, start, a.rect));
                    }
                }
                (None, Tool::Select) => {
                    self.adjusting = None;
                    self.selecting = Some((start, start));
                }
                (None, tool) => {
                    self.drawing = Shape::start(tool, start, self.color, self.width, self.next_number())
                }
            }
        }
        if canvas.dragged()
            && let Some(p) = pos
        {
            if let Some(Adjusting { rect, grab: Some((grip, from, start)) }) = &mut self.adjusting {
                *rect = grip.apply(*start, p - *from, screen);
            }
            if let Some((_, end)) = &mut self.selecting {
                *end = p;
            }
            if let Some(shape) = &mut self.drawing {
                for m in moves.iter().chain(std::iter::once(&p)) {
                    shape.drag_to(*m, shift);
                }
                self.preview = predicted_preview(shape, p, ui.input(|i| i.pointer.velocity()), shift);
            }
        }
        if canvas.drag_stopped() {
            self.preview = None;
            if let Some(a) = &mut self.adjusting {
                a.grab = None;
            }
            if let Some((a, b)) = self.selecting.take() {
                let selection = Rect::from_two_pos(a, b).intersect(screen);
                if selection.width() >= 3.0 && selection.height() >= 3.0 {
                    if command {
                        self.adjusting = Some(Adjusting { rect: selection, grab: None });
                    } else {
                        self.copy_and_close(ui.ctx(), selection, screen);
                    }
                }
            }
            if let Some(shape) = self.drawing.take()
                && !shape.is_trivial()
            {
                self.shapes.push(shape);
            }
        }
    }

    fn paint(&self, painter: &egui::Painter, screen: Rect, pointer: Option<Pos2>) {
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(self.texture.id(), screen, uv, Color32::WHITE);

        // Dim everything except the area being selected (or the hover highlight).
        let hover = self.highlight();
        let held = self.adjusting.as_ref().map(|a| a.rect);
        dim_outside(painter, screen, self.selecting.map(|(a, b)| Rect::from_two_pos(a, b)).or(held).or(hover));

        // Pixelate boxes first so drawings on top stay sharp, the same order as the copy.
        let shapes = || self.shapes.iter().chain(self.preview.as_ref().or(self.drawing.as_ref()));
        if let Some((mosaic, texture)) = &self.mosaic {
            let scale = self.scale(screen);
            for shape in shapes() {
                if let Kind::Pixelate(a, b) = shape.kind {
                    let r = Rect::from_two_pos(a, b);
                    painter.image(texture.id(), r, mosaic.uv(r, scale), Color32::WHITE);
                }
            }
        }
        for shape in shapes().filter(|s| !s.is_pixelate()) {
            shape.paint(painter);
        }

        if let Some(area) = hover {
            // Inside the area, so it stays visible when the area is the whole screen.
            dashed_outline(painter, area, StrokeKind::Inside);
            let scale = self.scale(screen);
            let label = format!("{} × {}", (area.width() * scale).round(), (area.height() * scale).round());
            toolbar::size_label(painter, area, screen, label);
        }

        if let Some((a, b)) = self.selecting {
            let sel = Rect::from_two_pos(a, b);
            dashed_outline(painter, sel, StrokeKind::Outside);
            let scale = self.scale(screen);
            let label = format!("{} × {}", (sel.width() * scale).round(), (sel.height() * scale).round());
            toolbar::size_label(painter, sel, screen, label);
        }

        if let Some(sel) = held {
            dashed_outline(painter, sel, StrokeKind::Outside);
            for c in adjust::handles(sel) {
                let handle = Rect::from_center_size(c, egui::Vec2::splat(adjust::HANDLE_SIZE));
                painter.rect_filled(handle, 2.0, Color32::WHITE);
                painter.rect_stroke(handle, 2.0, Stroke::new(1.0, theme::ACCENT), StrokeKind::Inside);
            }
            let scale = self.scale(screen);
            let label = format!("{} × {}", (sel.width() * scale).round(), (sel.height() * scale).round());
            toolbar::size_label(painter, sel, screen, label);
        }

        // Magnifier for pixel-exact edges: while dragging out or resizing a selection, and in
        // Select mode before you start, so you can line up the first corner too.
        let grab = self.adjusting.as_ref().and_then(|a| a.grab.map(|(grip, ..)| grip));
        let loupe_at = if self.selecting.is_some() || grab.is_some_and(|g| g != Grip::MOVE) {
            pointer
        } else if self.tool == Tool::Select && self.drawing.is_none() && grab.is_none() {
            // `hover.pos` starts at the OS cursor and clears when the mouse leaves this display.
            self.hover.pos
        } else {
            None
        };
        // Not while the cursor is over the toolbar or another panel.
        let over_ui = |p: Pos2| painter.ctx().layer_id_at(p).is_some_and(|l| l != painter.layer_id());
        if let Some(p) = loupe_at
            && !over_ui(p)
        {
            self.paint_loupe(painter, screen, p);
        }
    }

    /// Zoomed view of the screenshot pixels around `p`, with the pixel under it outlined.
    fn paint_loupe(&self, painter: &egui::Painter, screen: Rect, p: Pos2) {
        const CELLS: i32 = 15; // odd, so one pixel sits in the middle
        const CELL: f32 = 8.0; // points per screenshot pixel
        const OFFSET: f32 = 24.0;
        let (iw, ih) = (self.shot.width() as i32, self.shot.height() as i32);
        if iw < CELLS || ih < CELLS {
            return;
        }
        let scale = self.scale(screen);
        let px = ((p.x * scale).floor() as i32).clamp(0, iw - 1);
        let py = ((p.y * scale).floor() as i32).clamp(0, ih - 1);
        // The window of pixels shown, kept inside the image at the edges.
        let x0 = (px - CELLS / 2).clamp(0, iw - CELLS);
        let y0 = (py - CELLS / 2).clamp(0, ih - CELLS);

        let size = CELLS as f32 * CELL;
        let label_h = 22.0;
        // Below-right of the cursor, flipped to the other side near the screen edges.
        let mut min = p + vec2(OFFSET, OFFSET);
        if min.x + size > screen.max.x {
            min.x = p.x - OFFSET - size;
        }
        if min.y + size + label_h > screen.max.y {
            min.y = p.y - OFFSET - size - label_h;
        }
        let rect = Rect::from_min_size(min, egui::Vec2::splat(size));
        let (fw, fh) = (iw as f32, ih as f32);
        let uv = Rect::from_min_max(
            pos2(x0 as f32 / fw, y0 as f32 / fh),
            pos2((x0 + CELLS) as f32 / fw, (y0 + CELLS) as f32 / fh),
        );
        painter.rect_filled(rect.expand(3.0), 6.0, theme::SURFACE);
        painter.image(self.texture.id(), rect, uv, Color32::WHITE);
        let cell = Rect::from_min_size(
            rect.min + vec2((px - x0) as f32 * CELL, (py - y0) as f32 * CELL),
            egui::Vec2::splat(CELL),
        );
        // Black and white together stay visible on any pixel colour.
        painter.rect_stroke(cell, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
        painter.rect_stroke(cell.expand(1.0), 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Outside);
        painter.rect_stroke(rect.expand(3.0), 6.0, Stroke::new(1.0, theme::BORDER_HI), StrokeKind::Inside);

        let text = format!("{px}, {py}");
        let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), theme::MUTED);
        let label = Rect::from_center_size(
            pos2(rect.center().x, rect.max.y + 3.0 + label_h / 2.0),
            galley.size() + vec2(12.0, 4.0),
        );
        painter.rect_filled(label, 5.0, theme::SURFACE);
        painter.galley(label.min + vec2(6.0, 2.0), galley, theme::MUTED);
    }

    /// Crosshair, or a move/resize cursor over a selection held open with ⌘.
    fn cursor(&self, ctx: &egui::Context) -> CursorIcon {
        let Some(a) = &self.adjusting else { return CursorIcon::Crosshair };
        if let Some((grip, ..)) = a.grab {
            return grip.cursor(true);
        }
        match ctx.input(|i| i.pointer.hover_pos()) {
            Some(p) if self.tool == Tool::Select => {
                Grip::at(a.rect, p).map_or(CursorIcon::Crosshair, |g| g.cursor(false))
            }
            _ => CursorIcon::Crosshair,
        }
    }

    fn log_stats(&mut self) {
        let Some(stats) = &mut self.debug else { return };
        stats.frames += 1;
        let secs = stats.since.elapsed().as_secs_f32();
        if secs >= 1.0 {
            eprintln!(
                "snipno: {:.0} fps, {:.0} pointer samples/s",
                stats.frames as f32 / secs,
                stats.moves as f32 / secs
            );
            *stats = FrameStats { since: Instant::now(), frames: 0, moves: 0 };
        }
    }
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screen = ctx.content_rect();

        // macOS may nudge a new window below the menu bar; put it back over the whole display.
        if self.frame < 3 {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(self.origin));
            // With one overlay per display, only the cursor's display grabs keyboard focus at
            // start; otherwise whichever process starts last wins and ⌃1–5/Esc go to a screen
            // you're not looking at.
            if self.cursor_here {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            self.frame += 1;
        }
        // Focus follows the mouse between displays, so shortcuts act on the screen you're on.
        let (pointer_here, focused) =
            ctx.input(|i| (i.pointer.hover_pos().is_some(), i.viewport().focused.unwrap_or(false)));
        if pointer_here && !focused {
            if !self.asked_focus {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
                self.asked_focus = true;
            }
        } else {
            self.asked_focus = false;
        }
        if self.closing {
            return;
        }
        self.log_stats();
        if let Some(snap) = &mut self.snapshot {
            snap.tick(&ctx);
        }
        if self.debug.is_some() {
            ctx.request_repaint();
        }

        self.poll_reading(&ctx);
        if self.closing {
            return;
        }
        if let Some((area, _)) = &self.reading {
            // Waiting for the text: show the area being read, only Esc does anything.
            let area = *area;
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                return self.close(&ctx);
            }
            let painter = ui.painter();
            painter.image(self.texture.id(), screen, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            dim_outside(painter, screen, Some(area));
            dashed_outline(painter, area, StrokeKind::Outside);
            toolbar::hint(&ctx, "Reading text…  ·", "Esc", 1.0);
            return;
        }

        self.handle_keys(&ctx, screen);
        if self.tool == Tool::Pixelate {
            // Build the mosaic as soon as the tool is picked, not on the first drag.
            self.ensure_mosaic(&ctx, screen);
        }
        self.hover.update(&ctx);
        let canvas = ui.interact(screen, ui.scope_id().with("canvas"), Sense::click_and_drag());
        self.handle_pointer(ui, &canvas, screen);
        if self.closing {
            return;
        }
        if canvas.hovered() || canvas.dragged() {
            ctx.set_cursor_icon(self.cursor(&ctx));
        }

        // A selection held open with ⌘ is copied as soon as ⌘ is let go (and the mouse is up).
        if let Some(Adjusting { rect, grab: None }) = self.adjusting
            && !ctx.input(|i| i.modifiers.command || i.pointer.any_down())
        {
            self.copy_and_close(&ctx, rect, screen);
            return;
        }

        let pointer = ctx.input(|i| i.pointer.latest_pos());
        self.paint(ui.painter(), screen, pointer);
        if self.adjusting.is_some() {
            let release = if self.text { "Release ⌘ to copy the text" } else { "Release ⌘ to copy" };
            toolbar::hint(&ctx, &format!("{release}  ·  drag to move or resize  ·"), "⌘Z", 1.0);
            return;
        }
        if self.selecting.is_some() || self.drawing.is_some() {
            return; // keep the screen clear while dragging
        }

        let action = toolbar::show(
            &ctx,
            toolbar::State {
                tool: &mut self.tool,
                color: &mut self.color,
                width: &mut self.width,
                can_undo: !self.shapes.is_empty(),
                keys: &self.tool_keys,
                text: self.text,
            },
        );
        match action {
            Some(Action::Undo) => {
                self.shapes.pop();
            }
            Some(Action::FullScreen) => self.copy_and_close(&ctx, screen, screen),
            Some(Action::Close) => self.close(&ctx),
            None => {}
        }

        // A notice (no text found) replaces the usual hint and fades the same way.
        let since = self.notice.as_ref().map_or(self.opened, |(_, at)| *at);
        let age = since.elapsed().as_secs_f32();
        let opacity = (1.0 - (age - HINT_SECS) / 0.4).clamp(0.0, 1.0);
        let text = match self.tool {
            _ if self.notice.is_some() => self.notice.as_ref().map(|(m, _)| m.clone()).unwrap_or_default(),
            _ if self.text => "Click to copy the text in the highlight, drag for your own, scroll to resize  ·".to_string(),
            Tool::Select => "Click to capture the highlight, drag for your own, scroll to resize  ·".to_string(),
            tool => {
                let what = match tool {
                    Tool::Number => "Click to place 1, 2, 3…",
                    Tool::Pixelate => "Drag over anything private",
                    _ => "Draw",
                };
                format!("{what}, then press {} and drag to capture  ·", hotkey::pretty(self.tool_keys.get(Tool::Select)))
            }
        };
        toolbar::hint(&ctx, &text, "Esc", opacity);
        if opacity > 0.0 {
            ctx.request_repaint();
        }
    }
}
