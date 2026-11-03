#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "This packaging command requires macOS and the Apple command line tools." >&2
  exit 2
fi

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
mode="${1:-native}"
bundle="$project_root/dist/WallpaperMac.app"
saver="$project_root/dist/WallpaperMac.saver"
identity="${WPE_SIGN_IDENTITY:--}"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources" "$saver/Contents/MacOS" "$saver/Contents/Resources"
export MACOSX_DEPLOYMENT_TARGET=14.2

case "$mode" in
  native)
    cargo build --locked --release --bin wallpaper-macos --bin wpe
    cargo build --locked --release --lib -p wpe-screensaver
    cp target/release/wallpaper-macos "$bundle/Contents/MacOS/wallpaper-macos"
    cp target/release/libwallpaper_screensaver.dylib "$saver/Contents/MacOS/wallpaper_screensaver"
    ;;
  universal)
    for architecture in aarch64-apple-darwin x86_64-apple-darwin; do
      cargo build --locked --release --bin wallpaper-macos --bin wpe --target "$architecture"
      cargo build --locked --release --lib -p wpe-screensaver --target "$architecture"
    done
    cargo build --locked --release --bin wpe
    lipo -create target/aarch64-apple-darwin/release/wallpaper-macos target/x86_64-apple-darwin/release/wallpaper-macos -output "$bundle/Contents/MacOS/wallpaper-macos"
    lipo -create target/aarch64-apple-darwin/release/libwallpaper_screensaver.dylib target/x86_64-apple-darwin/release/libwallpaper_screensaver.dylib -output "$saver/Contents/MacOS/wallpaper_screensaver"
    ;;
  *) echo "Usage: $0 [native|universal]" >&2; exit 2 ;;
esac
if [[ "$mode" == universal ]]; then
  lipo -create target/aarch64-apple-darwin/release/wpe target/x86_64-apple-darwin/release/wpe -output "$bundle/Contents/MacOS/wpe"
else
  cp target/release/wpe "$bundle/Contents/MacOS/wpe"
fi

cp packaging/Info.plist "$bundle/Contents/Info.plist"
cp packaging/ScreenSaver-Info.plist "$saver/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist" "$saver/Contents/Info.plist"
if [[ ! -e "$bundle/Contents/Resources/Demo/project.json" ]]; then
  target/release/wpe demo "$bundle/Contents/Resources/Demo"
fi
python3 scripts/collect-licenses.py --output "$bundle/Contents/Resources/ThirdPartyLicenses"
python3 scripts/collect-licenses.py --output "$saver/Contents/Resources/ThirdPartyLicenses"
codesign --force --sign "$identity" "$bundle/Contents/MacOS/wpe"
codesign --force --sign "$identity" --entitlements packaging/Entitlements.plist "$bundle"
codesign --force --sign "$identity" --entitlements packaging/Entitlements.plist "$saver"
codesign --verify "$bundle"
codesign --verify "$saver"
echo "$bundle"
echo "$saver"
