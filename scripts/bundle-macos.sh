#!/usr/bin/env bash
# Builds a release binary and produces dist/Mori.app, dist/Mori-<version>.dmg and .zip.
set -euo pipefail

cd "$(dirname "$0")/.."
# Keep the build machine's home directory out of the source paths embedded in the binary.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=."
cargo build --release

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d '"' -f2)
ARCH=$(uname -m)
APP=dist/Mori.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/mori "$APP/Contents/MacOS/mori"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Mori</string>
    <key>CFBundleDisplayName</key><string>Mori</string>
    <key>CFBundleIdentifier</key><string>app.mori.desktop</string>
    <key>CFBundleExecutable</key><string>mori</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${VERSION}</string>
    <key>CFBundleVersion</key><string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"

DMG="dist/Mori-${VERSION}-${ARCH}.dmg"
ZIP="dist/Mori-${VERSION}-${ARCH}.zip"
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG" "$ZIP"
hdiutil create -volname "Mori" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
ditto -c -k --keepParent "$APP" "$ZIP"

echo "Built $APP"
echo "Built $DMG"
echo "Built $ZIP"
