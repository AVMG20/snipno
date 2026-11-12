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
| Drag (Select tool) | copy a custom region |
| hold `⌘` while releasing the mouse (or clicking a highlight) | keep the selection open: drag inside to move it, drag an edge or handle to resize, let go of `⌘` to copy |
| `S` `P` `A` `R` `E` | Select, Pen, Arrow, Box, Circle (change them in Settings → Tool shortcuts) |
| hold `⇧` | square box / perfect circle |
| `⌘Z` | while dragging or holding a selection open: drop it and start over; otherwise undo the last drawing |
| `↩` / **Full screen** | copy the whole screen |
| `Esc` | drop a selection held open, otherwise cancel |

To change the capture hotkey or the tool keys, click the menu bar icon → **Settings…**. A new capture hotkey works as soon as you've recorded it. Any key combination is allowed, including Insert and a Windows keyboard's PrtSc (which reaches a Mac as F13). Config is stored in `~/Library/Application Support/snipno/config.json`.

## Upload and copy the link

snipno can also upload a capture and put its link on the clipboard instead of the image. It works with most self-hosted image hosts (the ShareX "custom uploader" style). Set it up in **Settings → Upload**:

- **Upload URL**: the endpoint. The PNG is POSTed as multipart form data in the `file` field.
- **API token**: sent as `Authorization: Bearer <token>`.
- **Labels**: comma-separated, sent as the `tags` field. You can also edit them in the overlay before each upload.
- **Extra fields**: more form fields, `key=value&key=value`.
- **Link**: how to build the link from the JSON response, with `{json:path}` placeholders (`{response}` is the raw body). Leave it empty to use the response's `url` or `link` field.

Then record an **Upload shortcut**, or use **Screenshot & upload** in the menu bar menu. The overlay closes right away; a notification tells you when the link is copied or why the upload failed. The request is made by macOS's `curl`, with the token passed on stdin so it never appears in the process list. The token is stored in the config file in plain text.

For an imgstore instance:

| Field | Value |
|---|---|
| Upload URL | `https://img.example.com/api/images` |
| Extra fields | `isPublic=true` |
| Link | `https://img.example.com/i/{json:images[0].slug}` |

`snipno upload <file.png> [--labels a,b]` uploads a file from the terminal with the same settings.

## How it's built

One binary, three modes:

- `snipno` (daemon, `src/daemon.rs`): runs a tao event loop with the menu bar icon (`tray-icon`) and the global hotkey (`global-hotkey`). It runs the other two modes as child processes.
- `snipno capture [--monitor <id>]` (`src/overlay.rs`): the daemon starts one per display (`snipno monitors` lists the ids). The display under the cursor starts last, so it gets keyboard focus. When one overlay copies or cancels, the daemon closes the rest. Each overlay captures its own display with `xcap` before its own window exists. It then shows that image in a borderless eframe window raised above the menu bar (`src/macos.rs`).
- `snipno settings` (`src/settings.rs`): records the capture hotkey and the tool keys. It reports back to the daemon over stdout: while it records, the daemon pauses the global hotkey, and after a save the daemon reloads the config.

Smart selection (`src/snap.rs`) combines the window list with long straight edges found in the screenshot, so hovering highlights panels, cards and text boxes as well as whole windows.

Drawings are stored in window points. While you draw, egui paints them on screen. When you copy, `tiny-skia` draws them onto the captured pixels at the Retina scale (`src/annotate.rs`). The logo (`src/icon.rs`) is also drawn in code: `snipno icon out.png 1024` renders it.

## Debugging the UI

- `SNIPNO_SNAPSHOT=out.png snipno capture` (or `settings`) renders the window, saves what egui drew to `out.png` and quits. You can check the UI this way without Screen Recording permission.
- `SNIPNO_DEBUG=1 snipno capture` prints the frame rate and the number of pointer samples per second.

`cargo test` covers cropping and rendering, hotkey parsing and formatting, and the icon. None of it needs a display or screen permission.

## License

MIT — see [LICENSE](LICENSE).
