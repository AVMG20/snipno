# snipno

Menu bar screenshot tool for macOS. Press the hotkey (default **⇧⌘2**). The screen freezes and a toolbar appears at the top. Draw on the screen if you want, then drag a region: the selected area, drawings included, goes straight to the clipboard.

Website: https://snipno.bitploy.nl

## Install

Requires macOS 12+ and a recent Rust toolchain (`rustup`, edition 2024).

```bash
git clone https://github.com/AVMG20/snipno && cd snipno
./make-signing-cert.sh      # once: local code-signing certificate so permissions survive rebuilds
./bundle.sh --install       # build target/Snipno.app, copy it to /Applications and start it
```

On the first capture, macOS asks for **Screen Recording** permission (System Settings → Privacy & Security). Grant it, then quit and reopen Snipno. Until you do, captures show only your wallpaper.

During development, `cargo run` starts the daemon from your terminal. In that case the permission is granted to the terminal app, not to Snipno.

| In the overlay | |
|---|---|
| Hover (Select tool) | highlights the window or UI panel under the cursor |
| Click | copy the highlighted area |
| Scroll while hovering | grow to the enclosing area (panel → window → screen) or shrink back |
| Drag (Select tool) | copy a custom region; a magnifier shows the exact pixels under the cursor |
| hold `⌘` while releasing the mouse (or clicking a highlight) | keep the selection open: drag inside to move it, drag an edge or handle to resize, let go of `⌘` to copy |
| `S` `P` `A` `R` `E` `N` `B` | Select, Pen, Arrow, Box, Circle, Numbered marker, Pixelate (change them in Settings → Tool shortcuts) |
| Numbered marker | click to drop ①, ②, ③…; drag to place one precisely. Undo removes the last one, so the next number follows on |
| Pixelate | drag over passwords, emails or names to hide them behind big blocks |
| hold `⇧` | square box / perfect circle |
| `⌘Z` | while dragging or holding a selection open: drop it and start over; otherwise undo the last drawing |
| `↩` / **Full screen** | copy the whole screen |
| `Esc` | drop a selection held open, otherwise cancel |

## Extract text

Press **⇧⌘1** (or **Extract text** in the menu bar menu) and select an area the same way: hover and click, drag, or press `↩` for the whole screen. The text in it is copied as plain text, nothing is saved. If there's no text there, the overlay says so and stays open.

It uses macOS's own text recognition (Vision, the engine behind Live Text), so it works offline and adds nothing to the app's size. English and Dutch are preferred; numbers, codes, email addresses and symbols come through as they are. Lines are kept, and a blank line marks a paragraph break. Large areas, up to the whole screen, are read in pieces cut between columns, lines and words. Vision shrinks big images before reading them, so small text in a wide area would otherwise come back as garbage. A screen full of dense text can take a few seconds. `snipno ocr <image.png>` prints the text in an image file, handy for testing.

## Settings

To change the capture or extract-text hotkey or the tool keys, click the menu bar icon → **Settings…**. A new hotkey works as soon as you've recorded it. Any key combination is allowed, including Insert and a Windows keyboard's PrtSc (which reaches a Mac as F13). Config is stored in `~/Library/Application Support/snipno/config.json`.

## Saved screenshots

Every capture is also saved as a PNG, by default in `~/Pictures/snipno`. The last ten are listed under **Recent screenshots** in the menu bar menu, where you can copy the image again or show it in Finder; **Open screenshots folder** opens the folder. Change it in **Settings → Saving**:

- **Save screenshots**: switch saving off to only copy to the clipboard.
- **Folder**: type a path (`~` is your home folder) or click **Choose…**.
- **File name**: `.png` is added. `{date}`, `{time}` and `{timestamp}` (Unix seconds) are filled in. The default, `Screenshot {date} at {time}`, gives `Screenshot 2026-11-12 at 14-05-09.png`. A name that's taken gets ` 2`, ` 3`… added; nothing is overwritten.
- **Keep**: how many saved screenshots to keep, 200 by default. On each new capture the oldest beyond that are deleted; 0 keeps them all. Only files snipno saved itself are deleted (they're listed in `history.json` next to the config), so pointing the folder at your Desktop is safe.

## How it's built

One binary, three modes:

- `snipno` (daemon, `src/daemon.rs`): runs a tao event loop with the menu bar icon (`tray-icon`) and the global hotkey (`global-hotkey`). It runs the other two modes as child processes.
- `snipno capture [--monitor <id>] [--text]` (`src/overlay.rs`): the daemon starts one per display (`snipno monitors` lists the ids). The display under the cursor starts last, so it gets keyboard focus. When one overlay copies or cancels, the daemon closes the rest. Each overlay captures its own display with `xcap` before its own window exists. It then shows that image in a borderless eframe window raised above the menu bar (`src/macos.rs`). With `--text` the drawing tools are hidden and the selection is read with Vision (`src/ocr.rs`) instead of copied as an image.
- `snipno settings` (`src/settings.rs`): records the capture hotkey and the tool keys, and sets up saving. It reports back to the daemon over stdout: while it records, the daemon pauses the global hotkey, and after a save the daemon reloads the config.

Smart selection (`src/snap.rs`) combines the window list with long straight edges found in the screenshot, so hovering highlights panels and cards as well as whole windows. It deliberately ignores small controls such as buttons and text fields, since you usually want the block around them.

Drawings are stored in window points. While you draw, egui paints them on screen. When you copy, `tiny-skia` draws them onto the captured pixels at the Retina scale (`src/annotate.rs`). The logo (`src/icon.rs`) is also drawn in code: `snipno icon out.png 1024` renders it.

## Debugging the UI

- `SNIPNO_SNAPSHOT=out.png snipno capture` (or `settings`) renders the window, saves what egui drew to `out.png` and quits. You can check the UI this way without Screen Recording permission.
- `SNIPNO_DEBUG=1 snipno capture` prints the frame rate and the number of pointer samples per second.

`cargo test` covers cropping and rendering, hotkey parsing and formatting, and the icon. None of it needs a display or screen permission.

## License

MIT — see [LICENSE](LICENSE).
