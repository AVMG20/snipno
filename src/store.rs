//! Saved screenshots and the history behind "Recent screenshots" in the menu bar menu.
//!
//! Every capture can be written to a folder (Settings → Saving). The history file next to the
//! config lists what snipno saved, newest first. Clean-up only ever deletes files
//! listed there, so pointing the folder at the Desktop or an existing folder never touches
//! anything else in it.

use crate::config::{self, Save};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};
use xcap::image::RgbaImage;

/// One saved capture. Entries whose file is gone have `file: None` until the next clean-up.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Entry {
    pub file: Option<PathBuf>,
    /// Size of `file` when it was saved. Clean-up only deletes a file that still has this size,
    /// so a file someone else put at the same path later is left alone.
    pub size: Option<u64>,
    /// Unix seconds.
    pub time: i64,
}

impl Entry {
    /// The saved file, if it's still there.
    pub fn existing_file(&self) -> Option<&Path> {
        self.file.as_deref().filter(|f| f.is_file())
    }
}

pub fn history_path() -> PathBuf {
    config::path().with_file_name("history.json")
}

/// The history, newest first; empty if it's missing or unreadable.
pub fn history() -> Vec<Entry> {
    read_history().ok().flatten().unwrap_or_default()
}

/// `Ok(None)`: the file is there but isn't a history.
fn read_history() -> io::Result<Option<Vec<Entry>>> {
    let text = match fs::read_to_string(history_path()) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Some(Vec::new())),
        Err(e) => return Err(e),
    };
    Ok(parse_history(&text))
}

/// The older format, a list of uploaded links, holds no files: it reads as empty.
pub fn parse_history(text: &str) -> Option<Vec<Entry>> {
    if let Ok(entries) = serde_json::from_str::<Vec<Entry>>(text) {
        return Some(entries);
    }
    serde_json::from_str::<Vec<String>>(text).map(|_| Vec::new()).ok()
}

/// Change the history under a lock, so overlays saving at the same moment don't undo each
/// other's changes. Files the change gave up on are deleted afterwards.
fn update(change: impl FnOnce(&mut Vec<Entry>) -> Vec<(PathBuf, Option<u64>)>) -> io::Result<()> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(path.with_file_name("history.lock"))?;
    lock.lock()?;
    let mut entries = match read_history()? {
        Some(entries) => entries,
        None => {
            // Don't overwrite a history we can't read: it's what keeps clean-up to our own files.
            let bad = path.with_file_name("history.bad.json");
            eprintln!("snipno: {} is unreadable, moved to {}", path.display(), bad.display());
            fs::rename(&path, bad)?;
            Vec::new()
        }
    };
    forget_missing(&mut entries);
    let doomed = change(&mut entries);
    write_history(&entries)?;
    drop(lock);
    delete(&doomed);
    Ok(())
}

/// Drop files that were moved or deleted outside snipno, so they don't count towards the limit
/// and a later file at the same path is never mistaken for ours. A file whose folder is gone too
/// (an unplugged drive) is kept: it may come back.
fn forget_missing(entries: &mut [Entry]) {
    for e in entries.iter_mut() {
        if e.file.as_deref().is_some_and(|f| !f.is_file() && f.parent().is_some_and(Path::is_dir)) {
            e.file = None;
            e.size = None;
        }
    }
}

fn write_history(entries: &[Entry]) -> io::Result<()> {
    let path = history_path();
    let json = serde_json::to_string_pretty(entries).map_err(io::Error::other)?;
    // Write then rename, so the daemon never reads a half-written file.
    let tmp = path.with_file_name(format!("history.{}.tmp", std::process::id()));
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)
}

/// Trim the history: saved files beyond the newest `keep` (0: no limit) are returned for
/// deletion with their recorded size, and dropped along with entries whose file is gone.
pub fn prune(entries: &mut Vec<Entry>, keep: u32) -> Vec<(PathBuf, Option<u64>)> {
    let mut doomed = Vec::new();
    let mut files = 0usize;
    entries.retain_mut(|e| {
        if e.file.is_none() {
            return false;
        }
        files += 1;
        if keep > 0 && files > keep as usize {
            doomed.extend(e.file.take().map(|f| (f, e.size.take())));
            return false;
        }
        true
    });
    doomed
}

/// Delete the files [`prune`] gave up on: only PNGs that still have the size they were saved
/// with. Missing ones are fine.
fn delete(files: &[(PathBuf, Option<u64>)]) {
    for (f, size) in files {
        let png = f.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let ours = fs::metadata(f).is_ok_and(|m| m.is_file() && Some(m.len()) == *size);
        if !png || !ours {
            continue;
        }
        if let Err(e) = fs::remove_file(f)
            && e.kind() != io::ErrorKind::NotFound
        {
            eprintln!("snipno: could not delete {}: {e}", f.display());
        }
    }
}

/// Save a capture in the configured folder, add it to the history and clean up the oldest.
pub fn save(image: &RgbaImage, cfg: &Save) -> Result<PathBuf, Box<dyn Error>> {
    let dir = cfg.folder_path();
    fs::create_dir_all(&dir)?;
    let now = chrono::Local::now();
    let (path, file) = create_unique(&dir, &file_name(&cfg.name, now))?;
    if let Err(e) = write_png(image, file) {
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    let size = fs::metadata(&path).map(|m| m.len()).ok();
    let entry = Entry { file: Some(path.clone()), size, time: now.timestamp() };
    let result = update(|entries| {
        // A path can come round again (a fixed name, after the old file was moved away).
        for e in entries.iter_mut().filter(|e| e.file.as_deref() == Some(&path)) {
            e.file = None;
            e.size = None;
        }
        entries.insert(0, entry);
        prune(entries, cfg.keep)
    });
    // The screenshot is saved either way; it just won't be in the recents.
    if let Err(e) = result {
        eprintln!("snipno: could not update the screenshot history: {e}");
    }
    Ok(path)
}

/// A new file `name` in `dir`, or `name 2.png`, `name 3.png`… if that's taken.
pub fn create_unique(dir: &Path, name: &str) -> io::Result<(PathBuf, fs::File)> {
    let stem = name.strip_suffix(".png").unwrap_or(name);
    for n in 1.. {
        let path = if n == 1 { dir.join(name) } else { dir.join(format!("{stem} {n}.png")) };
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 10_000 => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub fn write_png(image: &RgbaImage, file: fs::File) -> Result<(), Box<dyn Error>> {
    use xcap::image::{
        ExtendedColorType, ImageEncoder,
        codecs::png::{CompressionType, FilterType, PngEncoder},
    };
    let mut out = io::BufWriter::new(file);
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgba8,
    )?;
    io::Write::flush(&mut out)?;
    Ok(())
}

/// A file name from a template: `{date}` (2026-11-12), `{time}` (14-05-09) and `{timestamp}`
/// (Unix seconds) filled in, made safe for a file name, plus `.png`.
pub fn file_name(template: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let template = template.trim();
    let template = if template.is_empty() { "{date}-{time}" } else { template };
    let name = template
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H-%M-%S").to_string())
        .replace("{timestamp}", &now.timestamp().to_string());
    // `/` would mean a folder, `:` shows as `/` in Finder; keep names to one plain file.
    let name: String = name
        .chars()
        .map(|c| if c.is_control() || "/\\:\";".contains(c) { '-' } else { c })
        .collect();
    let name = name.trim().trim_end_matches(".png").trim();
    // A leading dot would hide the file in Finder.
    let name = name.trim_start_matches('.');
    format!("{}.png", if name.is_empty() { "screenshot" } else { name })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        chrono::Local.from_local_datetime(&naive).unwrap()
    }

    fn file(n: u32) -> Entry {
        Entry { file: Some(PathBuf::from(format!("/shots/{n}.png"))), size: Some(n as u64), time: n as i64 }
    }

    #[test]
    fn file_name_fills_in_the_date_and_time() {
        let now = at("2026-11-12 14:05:09");
        assert_eq!(file_name("screenshot-{date}-{time}", now), "screenshot-2026-11-12-14-05-09.png");
        assert_eq!(file_name("Screenshot {date} at {time}", now), "Screenshot 2026-11-12 at 14-05-09.png");
        assert_eq!(file_name("{timestamp}", now), format!("{}.png", now.timestamp()));
        assert_eq!(file_name("screenshot", now), "screenshot.png");
        assert_eq!(file_name("  ", now), "2026-11-12-14-05-09.png");
        assert_eq!(file_name("bug.png", now), "bug.png");
        assert_eq!(file_name("a/b: \"c\";d", now), "a-b- -c--d.png");
        assert_eq!(file_name("..hidden", now), "hidden.png");
    }

    #[test]
    fn prune_deletes_saved_files_beyond_the_limit() {
        let mut entries: Vec<Entry> = (1..=5).rev().map(file).collect();
        entries[1].file = None; // moved away in Finder
        let doomed = prune(&mut entries, 2);
        assert_eq!(doomed, [(PathBuf::from("/shots/2.png"), Some(2)), (PathBuf::from("/shots/1.png"), Some(1))]);
        assert_eq!(entries, [file(5), file(3)]);
    }

    #[test]
    fn delete_leaves_files_that_changed_size() {
        let dir = std::env::temp_dir().join(format!("snipno-del-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (ours, theirs, txt) = (dir.join("a.png"), dir.join("b.png"), dir.join("c.txt"));
        fs::write(&ours, "1234").unwrap();
        fs::write(&theirs, "someone else's").unwrap();
        fs::write(&txt, "1234").unwrap();
        delete(&[(ours.clone(), Some(4)), (theirs.clone(), Some(4)), (txt.clone(), Some(4)), (dir.join("gone.png"), Some(1))]);
        assert!(!ours.exists());
        assert!(theirs.exists() && txt.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prune_with_zero_keeps_every_file() {
        let mut entries: Vec<Entry> = (1..=300).rev().map(file).collect();
        assert!(prune(&mut entries, 0).is_empty());
        assert_eq!(entries.len(), 300);
    }

    #[test]
    fn create_unique_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("snipno-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, _) = create_unique(&dir, "shot.png").unwrap();
        let (b, _) = create_unique(&dir, "shot.png").unwrap();
        let (c, _) = create_unique(&dir, "shot.png").unwrap();
        assert_eq!(a.file_name().unwrap(), "shot.png");
        assert_eq!(b.file_name().unwrap(), "shot 2.png");
        assert_eq!(c.file_name().unwrap(), "shot 3.png");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_the_history_and_the_old_list_of_links() {
        assert_eq!(parse_history(r#"["https://x/1","https://x/2"]"#), Some(Vec::new()));
        let e = parse_history(r#"[{"file":"/a.png","link":"https://x/1","time":5}]"#).unwrap();
        assert_eq!(e[0].file.as_deref(), Some(Path::new("/a.png")));
        assert!(parse_history("{not json").is_none());
    }
}
