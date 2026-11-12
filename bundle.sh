#!/usr/bin/env bash
# Build target/Snipno.app: a menu-bar-only app bundle with the generated icon.
# Running from a bundle (instead of `cargo run` in a terminal) means macOS asks for
# Screen Recording permission for Snipno itself rather than for your terminal.
set -euo pipefail
cd "$(dirname "$0")"

cargo build --release
BIN=target/release/snipno
APP=target/Snipno.app
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/snipno"

ICONSET="$(mktemp -d)/snipno.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  "$BIN" icon "$ICONSET/icon_${s}x${s}.png" "$s"
  "$BIN" icon "$ICONSET/icon_${s}x${s}@2x.png" "$((s * 2))"
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/snipno.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Snipno</string>
  <key>CFBundleDisplayName</key><string>Snipno</string>
  <key>CFBundleIdentifier</key><string>local.snipno</string>
  <key>CFBundleExecutable</key><string>snipno</string>
  <key>CFBundleIconFile</key><string>snipno</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Sign with the stable local certificate (see make-signing-cert.sh) so the Screen Recording
# grant survives rebuilds. Without it, fall back to ad-hoc: works, but macOS asks for the
# permission again after every rebuild.
IDENTITY="Snipno Local Signing"
if security find-certificate -c "$IDENTITY" >/dev/null 2>&1; then
  codesign --force --deep --sign "$IDENTITY" "$APP"
else
  echo "warning: no \"$IDENTITY\" certificate; signing ad-hoc. Run ./make-signing-cert.sh once." >&2
  codesign --force --deep --sign - "$APP"
fi

if [[ "${1:-}" == "--install" ]]; then
  pkill -x snipno || true
  sleep 0.5
  rm -rf /Applications/Snipno.app
  cp -R "$APP" /Applications/
  open /Applications/Snipno.app
  echo "Installed and started /Applications/Snipno.app"
else
  echo "Built $APP — run ./bundle.sh --install to copy it to /Applications and start it."
fi
