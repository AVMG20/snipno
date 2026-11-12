//! `SNIPNO_SNAPSHOT=out.png snipno capture|settings` renders the window, saves what egui drew
//! to a PNG and quits. Lets you (or an agent) check the UI without Screen Recording permission.

use eframe::egui::{self, ViewportCommand};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use xcap::image::RgbaImage;

/// Frames to wait so fonts, textures and fade-ins have settled.
const SETTLE_FRAMES: u32 = 30;

pub struct Snapshot {
    path: PathBuf,
    frame: u32,
    done: Arc<AtomicBool>,
}

impl Snapshot {
    pub fn from_env() -> Option<Self> {
        std::env::var_os("SNIPNO_SNAPSHOT").map(|p| Self {
            path: p.into(),
            frame: 0,
            done: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        ctx.request_repaint();
        if self.frame == SETTLE_FRAMES {
            let (path, done, repaint) = (self.path.clone(), self.done.clone(), ctx.clone());
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let rgba = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
                let [w, h] = img.size;
                match RgbaImage::from_raw(w as u32, h as u32, rgba).map(|i| i.save(&path)) {
                    Some(Ok(())) => eprintln!("snipno: snapshot saved to {}", path.display()),
                    Some(Err(e)) => eprintln!("snipno: snapshot failed: {e}"),
                    None => eprintln!("snipno: snapshot buffer size mismatch"),
                }
                done.store(true, Ordering::SeqCst);
                repaint.request_repaint();
            })));
        }
        if self.done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}
