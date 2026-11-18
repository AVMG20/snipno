//! Text recognition with macOS's Vision framework (the engine behind Live Text): nothing to
//! bundle, and it runs on the Neural Engine. English and Dutch are preferred; digits, codes and
//! symbols come through as they are.

use xcap::image::{Rgba, RgbaImage, imageops};

/// Read the text in `image` (screenshot pixels; `scale` is pixels per point, 2.0 on Retina).
/// Lines are top to bottom, segments on the same line joined by a space, and a blank line where
/// the gap between lines looks like a paragraph break. Empty when there's no text.
pub fn read(image: &RgbaImage, scale: f32) -> Result<String, String> {
    let limit = (TILE_PT * scale.max(1.0)) as u32;
    let pieces = tiles(image, limit, scale);
    let mut segments = Vec::new();
    // One after the other: Vision runs them one at a time anyway (measured no faster in parallel).
    for sheet in sheets(image, &pieces, limit, scale) {
        segments.extend(read_sheet(&sheet, scale)?);
    }
    Ok(layout(segments))
}

/// Read one sheet; segments come back in the screenshot's pixels.
fn read_sheet(sheet: &Sheet, scale: f32) -> Result<Vec<Segment>, String> {
    let (prepared, up, margin) = prepare(&sheet.image, scale);
    let mut segments = Vec::new();
    for mut seg in imp::recognize(&prepared)? {
        // Prepared pixels → sheet pixels → the screenshot's pixels.
        let to_sheet = |v: f64| (v - margin as f64) / up as f64;
        let (left, top, bottom) = (to_sheet(seg.left), to_sheet(seg.top), to_sheet(seg.bottom));
        let mid = (top + bottom) / 2.0;
        let Some(slot) = sheet.slots.iter().find(|s| mid >= s.at.1 as f64 && mid < (s.at.1 + s.tile.h) as f64) else {
            continue;
        };
        let (dx, dy) = (slot.tile.x as f64 - slot.at.0 as f64, slot.tile.y as f64 - slot.at.1 as f64);
        seg.left = left + dx;
        seg.top = top + dy;
        seg.bottom = bottom + dy;
        segments.push(seg);
    }
    Ok(segments)
}

/// Largest piece, in points, Vision gets to read at once. Its accurate mode scales big images
/// down before reading, so on a wide selection small text turns to mush (12pt text across
/// 2500pt comes back empty) — measured fine up to ~1600pt for 12pt text.
const TILE_PT: f32 = 900.0;

/// A rectangle of the screenshot, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Tile {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Cut the screenshot into pieces at most `limit` pixels a side, only through blank gaps
/// (between columns, lines or words) so no letter is split, and skip empty areas. A selection
/// that small is read whole, as you framed it.
fn tiles(image: &RgbaImage, limit: u32, scale: f32) -> Vec<Tile> {
    let (w, h) = image.dimensions();
    if w <= limit && h <= limit {
        return vec![Tile { x: 0, y: 0, w, h }];
    }
    let luma: Vec<u8> = image.pixels().map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8).collect();
    let ink = Ink { luma: &luma, width: w as usize };
    // A gap between columns of text, not just between words.
    let column_gap = (16.0 * scale) as usize;
    // Narrower than a word space, wider than the gap between letters.
    let word_gap = ((2.5 * scale).round() as usize).max(3);
    let mut out = Vec::new();
    let mut todo = vec![Tile { x: 0, y: 0, w, h }];
    while let Some(t) = todo.pop() {
        let Some((t, bg)) = ink.trim(t) else { continue };
        if t.w <= limit && t.h <= limit {
            out.push(t);
            continue;
        }
        let cols = ink.cols(t, bg);
        let rows = ink.rows(t, bg);
        // Between columns first, then between lines, and only a single line too wide is cut
        // between words: pieces of many lines cut between words read worse.
        let vertical = |x: usize| [Tile { w: x as u32, ..t }, Tile { x: t.x + x as u32, w: t.w - x as u32, ..t }];
        let split = if t.w > limit && let Some(x) = gap(&cols, column_gap) {
            vertical(x)
        } else if let Some(y) = gap(&rows, 1) {
            let y = y as u32;
            [Tile { h: y, ..t }, Tile { y: t.y + y, h: t.h - y, ..t }]
        } else if t.w > limit && let Some(x) = gap(&cols, word_gap) {
            vertical(x)
        } else if t.w > limit {
            // No gap anywhere (a picture?): cut it anyway.
            [Tile { w: t.w / 2, ..t }, Tile { x: t.x + t.w / 2, w: t.w - t.w / 2, ..t }]
        } else {
            [Tile { h: t.h / 2, ..t }, Tile { y: t.y + t.h / 2, h: t.h - t.h / 2, ..t }]
        };
        todo.extend(split);
    }
    out
}

/// Where the text is: a pixel is ink when its brightness is off the background's (the most
/// common brightness in the piece). A column or row with no ink is a safe place to cut.
/// Coloured panels count as ink too, which only means fewer places to cut.
struct Ink<'a> {
    luma: &'a [u8],
    width: usize,
}

impl Ink<'_> {
    /// Smallest difference from the background that counts, low enough for faint grey text.
    const THRESHOLD: u8 = 24;

    fn at(&self, x: u32, y: u32) -> u8 {
        self.luma[y as usize * self.width + x as usize]
    }

    fn background(&self, t: Tile) -> u8 {
        let mut counts = [0u32; 256];
        for y in t.y..t.y + t.h {
            for x in t.x..t.x + t.w {
                counts[self.at(x, y) as usize] += 1;
            }
        }
        (0..=255u8).max_by_key(|v| counts[*v as usize]).unwrap_or(255)
    }

    /// Per column of `t`: is anything drawn in it?
    fn cols(&self, t: Tile, bg: u8) -> Vec<bool> {
        (t.x..t.x + t.w).map(|x| (t.y..t.y + t.h).any(|y| self.at(x, y).abs_diff(bg) > Self::THRESHOLD)).collect()
    }

    /// Per row of `t`: is anything drawn in it?
    fn rows(&self, t: Tile, bg: u8) -> Vec<bool> {
        (t.y..t.y + t.h).map(|y| (t.x..t.x + t.w).any(|x| self.at(x, y).abs_diff(bg) > Self::THRESHOLD)).collect()
    }

    /// `t` without blank rows and columns around it, with its background; `None` if it's blank.
    fn trim(&self, t: Tile) -> Option<(Tile, u8)> {
        let bg = self.background(t);
        let cols = self.cols(t, bg);
        let (l, r) = (cols.iter().position(|c| *c)?, cols.iter().rposition(|c| *c)?);
        let t = Tile { x: t.x + l as u32, w: (r - l + 1) as u32, ..t };
        let rows = self.rows(t, bg);
        let (top, bottom) = (rows.iter().position(|c| *c)?, rows.iter().rposition(|c| *c)?);
        Some((Tile { y: t.y + top as u32, h: (bottom - top + 1) as u32, ..t }, bg))
    }
}

/// The middle of the blank run (at least `min` long, not at the ends) closest to the centre.
fn gap(ink: &[bool], min: usize) -> Option<usize> {
    let centre = ink.len() / 2;
    let mut best: Option<usize> = None;
    let mut i = 0;
    while i < ink.len() {
        if ink[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < ink.len() && !ink[i] {
            i += 1;
        }
        if start > 0 && i < ink.len() && i - start >= min {
            let mid = (start + i) / 2;
            if best.is_none_or(|b| mid.abs_diff(centre) < b.abs_diff(centre)) {
                best = Some(mid);
            }
        }
    }
    best
}

/// Pieces stacked into one image for a single Vision pass, each at its `at` position.
struct Sheet {
    image: RgbaImage,
    slots: Vec<Slot>,
}

struct Slot {
    tile: Tile,
    at: (u32, u32),
}

/// Stack the pieces top to bottom on sheets no taller than `limit`, with room around each in
/// its own background colour. A small selection is just one sheet with itself on it.
fn sheets(image: &RgbaImage, tiles: &[Tile], limit: u32, scale: f32) -> Vec<Sheet> {
    let pad = (12.0 * scale.max(1.0)) as u32;
    let mut groups: Vec<Vec<Tile>> = Vec::new();
    let mut height = 0;
    for t in tiles {
        let slot_h = t.h + 2 * pad;
        match groups.last_mut() {
            Some(group) if height + slot_h <= limit + 2 * pad => group.push(*t),
            _ => {
                groups.push(vec![*t]);
                height = 0;
            }
        }
        height += slot_h;
    }
    groups
        .into_iter()
        .map(|group| {
            let width = group.iter().map(|t| t.w).max().unwrap_or(0) + 2 * pad;
            let height = group.iter().map(|t| t.h + 2 * pad).sum();
            let crops: Vec<RgbaImage> = group.iter().map(|t| imageops::crop_imm(image, t.x, t.y, t.w, t.h).to_image()).collect();
            let mut sheet = RgbaImage::from_pixel(width, height, background(&crops[0]));
            let mut slots = Vec::new();
            let mut y = 0;
            for (t, crop) in group.iter().zip(&crops) {
                let bg = background(crop);
                for py in y..y + t.h + 2 * pad {
                    for px in 0..width {
                        sheet.put_pixel(px, py, bg);
                    }
                }
                imageops::replace(&mut sheet, crop, pad as i64, (y + pad) as i64);
                slots.push(Slot { tile: *t, at: (pad, y + pad) });
                y += t.h + 2 * pad;
            }
            Sheet { image: sheet, slots }
        })
        .collect()
}

/// Load Vision's models ahead of time: a cold read takes ~300 ms, one after this ~100 ms. Run it
/// on a background thread when a text capture opens.
pub fn warm_up() {
    let _ = imp::recognize(&warm_up_image());
}

/// "HELLO" in a blocky 5×7 font: recognising real letters loads the whole pipeline, which a
/// blank image doesn't.
fn warm_up_image() -> RgbaImage {
    const GLYPHS: [[u8; 7]; 5] = [
        [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11], // H
        [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f], // E
        [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f], // L
        [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f], // L
        [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e], // O
    ];
    const PX: u32 = 4;
    let mut img = RgbaImage::from_pixel(PX * (GLYPHS.len() as u32 * 6 + 8), PX * 15, Rgba([255, 255, 255, 255]));
    for (i, glyph) in GLYPHS.iter().enumerate() {
        for (row, bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) != 0 {
                    let (x0, y0) = (PX * (4 + i as u32 * 6 + col), PX * (4 + row as u32));
                    for y in y0..y0 + PX {
                        for x in x0..x0 + PX {
                            img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                        }
                    }
                }
            }
        }
    }
    img
}

/// `snipno ocr <image>`: print the text in an image file.
pub fn run_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.first().ok_or("usage: snipno ocr <image> [--scale <px per pt>]")?;
    let scale = args.iter().position(|a| a == "--scale").and_then(|i| args.get(i + 1)?.parse().ok()).unwrap_or(2.0);
    let image = xcap::image::open(path)?.to_rgba8();
    let start = std::time::Instant::now();
    let text = read(&image, scale)?;
    println!("{text}");
    eprintln!("snipno: read in {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

/// Most pixels a 1x screenshot is scaled up to: a few lines' worth gets 3×, a whole screen
/// stays near Retina size so it doesn't take seconds and a gigabyte.
const MAX_UPSCALED_PIXELS: u64 = 16_000_000;

/// Vision reads best with text well above screen size and some room around it: 1x screenshots
/// are scaled up, 3× where that stays under [`MAX_UPSCALED_PIXELS`] (measured best for small
/// text; Retina is read as is), and a tight crop gets a margin in its own background colour so
/// letters touching the edge aren't lost. Returns the image, the scale-up and the margin.
fn prepare(image: &RgbaImage, scale: f32) -> (RgbaImage, u32, u32) {
    let area = image.width() as u64 * image.height() as u64;
    let up = if scale >= 1.5 {
        1
    } else {
        (1..=3u32).rev().find(|f| area * (*f as u64).pow(2) <= MAX_UPSCALED_PIXELS).unwrap_or(1)
    };
    let image = if up > 1 {
        imageops::resize(image, image.width() * up, image.height() * up, imageops::FilterType::CatmullRom)
    } else {
        image.clone()
    };
    let margin = 16;
    let (w, h) = image.dimensions();
    let mut out = RgbaImage::from_pixel(w + 2 * margin, h + 2 * margin, edge_color(&image));
    imageops::replace(&mut out, &image, margin as i64, margin as i64);
    (out, up, margin)
}

/// The most common colour, near enough: the average of the pixels with the most common
/// brightness. A piece is trimmed tight to its text, so its border is no guide.
fn background(image: &RgbaImage) -> Rgba<u8> {
    let luma = |p: &Rgba<u8>| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as usize;
    let mut counts = [0u32; 256];
    for p in image.pixels() {
        counts[luma(p)] += 1;
    }
    let Some(mode) = (0..256).max_by_key(|v| counts[*v]).filter(|v| counts[*v] > 0) else {
        return Rgba([255, 255, 255, 255]);
    };
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for p in image.pixels().filter(|p| luma(p) == mode) {
        for c in 0..3 {
            sum[c] += p[c] as u64;
        }
        n += 1;
    }
    Rgba([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255])
}

/// Average colour along the border: the background, near enough, for padding.
fn edge_color(image: &RgbaImage) -> Rgba<u8> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Rgba([255, 255, 255, 255]);
    }
    let border = (0..w)
        .flat_map(|x| [(x, 0), (x, h - 1)])
        .chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]));
    let (mut sum, mut n) = ([0u64; 3], 0u64);
    for (x, y) in border {
        let p = image.get_pixel(x, y).0;
        for c in 0..3 {
            sum[c] += p[c] as u64;
        }
        n += 1;
    }
    Rgba([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255])
}

/// One recognised piece of text, with its box in image pixels (top-left origin).
#[derive(Clone, Debug)]
pub struct Segment {
    pub text: String,
    pub left: f64,
    pub top: f64,
    pub bottom: f64,
}

impl Segment {
    fn height(&self) -> f64 {
        self.bottom - self.top
    }
}

/// Put segments in reading order: group those that share a line, left to right within it.
fn layout(mut segments: Vec<Segment>) -> String {
    segments.retain(|s| !s.text.trim().is_empty());
    segments.sort_by(|a, b| a.top.total_cmp(&b.top));
    let mut lines: Vec<Vec<Segment>> = Vec::new();
    for seg in segments {
        // Recent lines too: pieces of one line can arrive after a piece of the next.
        match lines.iter_mut().rev().take(3).find(|line| same_line(line, &seg)) {
            Some(line) => line.push(seg),
            None => lines.push(vec![seg]),
        }
    }
    lines.sort_by(|a, b| centre(a).total_cmp(&centre(b)));
    let mut heights: Vec<f64> = lines.iter().flatten().map(Segment::height).collect();
    heights.sort_by(f64::total_cmp);
    let typical = heights.get(heights.len() / 2).copied().unwrap_or(0.0);

    let mut out = String::new();
    let mut prev_bottom: Option<f64> = None;
    for mut line in lines {
        line.sort_by(|a, b| a.left.total_cmp(&b.left));
        let top = line.iter().map(|s| s.top).fold(f64::INFINITY, f64::min);
        let bottom = line.iter().map(|s| s.bottom).fold(f64::NEG_INFINITY, f64::max);
        if let Some(prev) = prev_bottom {
            out.push('\n');
            if top - prev > typical * 0.9 {
                out.push('\n');
            }
        }
        let text: Vec<&str> = line.iter().map(|s| s.text.trim()).collect();
        out.push_str(&text.join(" "));
        prev_bottom = Some(bottom);
    }
    out
}

/// Average vertical centre of a line's segments.
fn centre(line: &[Segment]) -> f64 {
    line.iter().map(|s| (s.top + s.bottom) / 2.0).sum::<f64>() / line.len() as f64
}

/// Does `seg` sit on `line`: is its centre within half a text height of the line's? Measured
/// against the average, not the line's full extent, so one tall segment can't swallow the
/// lines around it.
fn same_line(line: &[Segment], seg: &Segment) -> bool {
    let height = line.iter().map(Segment::height).sum::<f64>() / line.len() as f64;
    ((seg.top + seg.bottom) / 2.0 - centre(line)).abs() < 0.5 * height.min(seg.height())
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Segment;
    use objc2::{
        Encoding, RefEncode,
        rc::{Allocated, Retained, autoreleasepool},
        runtime::{AnyClass, AnyObject},
        msg_send,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSError, NSIndexSet, NSRect, NSString};
    use std::ffi::{CStr, c_void};
    use xcap::image::RgbaImage;

    #[repr(C)]
    pub struct CGImage {
        _private: [u8; 0],
    }
    unsafe impl RefEncode for CGImage {
        const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGColorSpaceRelease(space: *mut c_void);
        fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
        fn CGDataProviderRelease(provider: *mut c_void);
        #[allow(clippy::too_many_arguments)]
        fn CGImageCreate(
            width: usize,
            height: usize,
            bits_per_component: usize,
            bits_per_pixel: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
            provider: *mut c_void,
            decode: *const f64,
            should_interpolate: bool,
            intent: i32,
        ) -> *mut CGImage;
        fn CGImageRelease(image: *mut CGImage);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "Vision", kind = "framework")]
    unsafe extern "C" {}

    const RECOGNITION_ACCURATE: isize = 0;
    const ALPHA_NONE_SKIP_LAST: u32 = 5;
    /// In order of preference; Vision drops any it doesn't support.
    const LANGUAGES: [&str; 2] = ["en-US", "nl-NL"];

    fn class(name: &CStr) -> Result<&'static AnyClass, String> {
        AnyClass::get(name).ok_or_else(|| format!("{} is not available", name.to_string_lossy()))
    }

    fn describe(e: Retained<NSError>) -> String {
        e.localizedDescription().to_string()
    }

    /// The pixels as a CGImage with its own copy of the buffer, so nothing Vision keeps can
    /// outlive `image`.
    unsafe fn cg_image(image: &RgbaImage) -> Result<*mut CGImage, String> {
        let (w, h) = (image.width() as usize, image.height() as usize);
        let bytes = image.as_raw();
        unsafe {
            let space = CGColorSpaceCreateDeviceRGB();
            let data = CFDataCreate(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize);
            if data.is_null() {
                return Err("out of memory".into());
            }
            let provider = CGDataProviderCreateWithCFData(data);
            CFRelease(data);
            let cg = CGImageCreate(w, h, 8, 32, w * 4, space, ALPHA_NONE_SKIP_LAST, provider, std::ptr::null(), false, 0);
            CGDataProviderRelease(provider);
            CGColorSpaceRelease(space);
            if cg.is_null() { Err("could not create an image for Vision".into()) } else { Ok(cg) }
        }
    }

    pub fn recognize(image: &RgbaImage) -> Result<Vec<Segment>, String> {
        if image.width() == 0 || image.height() == 0 {
            return Ok(Vec::new());
        }
        autoreleasepool(|_| unsafe {
            let cg = cg_image(image)?;
            let result = recognize_cg(cg, image.width() as f64, image.height() as f64);
            CGImageRelease(cg);
            result
        })
    }

    unsafe fn recognize_cg(cg: *mut CGImage, w: f64, h: f64) -> Result<Vec<Segment>, String> {
        unsafe {
            let request_class = class(c"VNRecognizeTextRequest")?;
            let request: Retained<AnyObject> = msg_send![request_class, new];
            let _: () = msg_send![&request, setRecognitionLevel: RECOGNITION_ACCURATE];
            // Newest engine this macOS has (the default follows the SDK the binary was built with).
            let revisions: Retained<NSIndexSet> = msg_send![request_class, supportedRevisions];
            if revisions.count() > 0 {
                let _: () = msg_send![&request, setRevision: revisions.lastIndex()];
            }
            let supported: Result<Retained<NSArray<NSString>>, Retained<NSError>> =
                msg_send![&request, supportedRecognitionLanguagesAndReturnError: _];
            let supported: Vec<String> = supported.map(|a| a.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
            let languages: Vec<Retained<NSString>> = LANGUAGES
                .iter()
                .filter(|l| supported.iter().any(|s| s == *l))
                .map(|l| NSString::from_str(l))
                .collect();
            if !languages.is_empty() {
                let languages = NSArray::from_retained_slice(&languages);
                let _: () = msg_send![&request, setRecognitionLanguages: &*languages];
            }
            let _: () = msg_send![&request, setUsesLanguageCorrection: true];

            let handler_class = class(c"VNImageRequestHandler")?;
            let handler: Allocated<AnyObject> = msg_send![handler_class, alloc];
            let options = NSDictionary::<NSString, AnyObject>::new();
            let handler: Option<Retained<AnyObject>> = msg_send![handler, initWithCGImage: cg, options: &*options];
            let handler = handler.ok_or("Vision could not open the image")?;
            let requests = NSArray::from_retained_slice(std::slice::from_ref(&request));
            let done: Result<(), Retained<NSError>> = msg_send![&handler, performRequests: &*requests, error: _];
            done.map_err(describe)?;

            let results: Option<Retained<NSArray<AnyObject>>> = msg_send![&request, results];
            let mut segments = Vec::new();
            for observation in results.iter().flat_map(|r| r.iter()) {
                let candidates: Retained<NSArray<AnyObject>> = msg_send![&observation, topCandidates: 1usize];
                let Some(best) = candidates.firstObject() else { continue };
                let text: Retained<NSString> = msg_send![&best, string];
                // Normalised, bottom-left origin.
                let b: NSRect = msg_send![&observation, boundingBox];
                segments.push(Segment {
                    text: text.to_string(),
                    left: b.origin.x * w,
                    top: (1.0 - b.origin.y - b.size.height) * h,
                    bottom: (1.0 - b.origin.y) * h,
                });
            }
            Ok(segments)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn recognize(_: &xcap::image::RgbaImage) -> Result<Vec<super::Segment>, String> {
        Err("text recognition needs macOS".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, left: f64, top: f64, _right: f64, bottom: f64) -> Segment {
        Segment { text: text.into(), left, top, bottom }
    }

    #[test]
    fn joins_segments_on_a_line_and_orders_lines() {
        let text = layout(vec![
            seg("world", 60.0, 0.0, 100.0, 20.0),
            seg("second", 0.0, 26.0, 50.0, 46.0),
            seg("hello", 0.0, 1.0, 50.0, 21.0),
        ]);
        assert_eq!(text, "hello world\nsecond");
    }

    #[test]
    fn blank_line_between_paragraphs() {
        let text = layout(vec![seg("one", 0.0, 0.0, 30.0, 20.0), seg("two", 0.0, 50.0, 30.0, 70.0)]);
        assert_eq!(text, "one\n\ntwo");
    }

    #[test]
    fn padding_keeps_size_and_background() {
        let img = RgbaImage::from_pixel(10, 4, Rgba([20, 30, 40, 255]));
        let (out, up, margin) = prepare(&img, 2.0);
        assert_eq!((out.dimensions(), up, margin), ((42, 36), 1, 16));
        assert_eq!(out.get_pixel(0, 0).0, [20, 30, 40, 255]);
        assert_eq!(prepare(&img, 1.0).0.dimensions(), (62, 44));
        // A big 1x image is scaled up less.
        let screen = RgbaImage::new(4000, 2000);
        assert_eq!(prepare(&screen, 1.0).0.dimensions(), (4000 + 32, 2000 + 32));
    }

    /// Black "text" blocks on white.
    fn page(w: u32, h: u32, blocks: &[(u32, u32, u32, u32)]) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        for &(x, y, bw, bh) in blocks {
            for yy in y..y + bh {
                for xx in x..x + bw {
                    img.put_pixel(xx, yy, Rgba([0, 0, 0, 255]));
                }
            }
        }
        img
    }

    #[test]
    fn small_area_is_one_tile() {
        let img = page(300, 200, &[(20, 30, 100, 12)]);
        assert_eq!(tiles(&img, 900, 1.0), vec![Tile { x: 0, y: 0, w: 300, h: 200 }]);
    }

    #[test]
    fn big_area_skips_blank_space() {
        let img = page(2000, 200, &[(20, 30, 100, 12)]);
        assert_eq!(tiles(&img, 900, 1.0), vec![Tile { x: 20, y: 30, w: 100, h: 12 }]);
        assert!(tiles(&page(2000, 200, &[]), 900, 1.0).is_empty());
    }

    #[test]
    fn wide_area_is_cut_in_gaps() {
        // Two lines of "words" 2000px wide, words 300px with 10px spaces.
        let words: Vec<_> = (0..6).flat_map(|i| [(i * 310 + 10, 10, 300, 12), (i * 310 + 10, 60, 300, 12)]).collect();
        let img = page(2000, 100, &words);
        let tiles = tiles(&img, 900, 1.0);
        assert!(tiles.len() > 1);
        for t in &tiles {
            assert!(t.w <= 900 && t.h <= 900);
            // Never through a word: every edge is in a space.
            assert!(words.iter().all(|&(x, _, w, _)| t.x <= x || t.x >= x + w), "{t:?}");
        }
    }

    #[test]
    fn sheets_stack_tiles_and_keep_their_place() {
        let img = page(2000, 80, &[(10, 10, 300, 12), (1500, 40, 300, 12)]);
        let tiles = tiles(&img, 900, 1.0);
        let sheets = sheets(&img, &tiles, 900, 1.0);
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].slots.len(), tiles.len());
        let total: u32 = tiles.iter().map(|t| t.h + 24).sum();
        assert_eq!(sheets[0].image.height(), total);
    }
}
