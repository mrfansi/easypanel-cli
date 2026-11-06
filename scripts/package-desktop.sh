#!/usr/bin/env bash
# Run after building the desktop binary with an explicit --target.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
target="${1:?Usage: bash scripts/package-desktop.sh TARGET}"
case "$target" in
  x86_64-unknown-linux-gnu) [[ "$(uname -s)" == Linux ]] ;;
  aarch64-apple-darwin|x86_64-apple-darwin) [[ "$(uname -s)" == Darwin ]] ;;
  *) echo "Unsupported desktop target: $target" >&2; exit 1 ;;
esac
binary="target/$target/release/easypanel-desktop"
[[ -s "$binary" && -x "$binary" ]] || { echo "Build $binary first" >&2; exit 1; }
mkdir -p dist

if [[ "$target" == x86_64-unknown-linux-gnu ]]; then
  archive="$root/dist/easypanel-desktop-$target.tar.gz"
  tar -czf "$archive" -C "target/$target/release" easypanel-desktop
  [[ "$(tar -tzf "$archive")" == easypanel-desktop ]]
else
  package_id="$(cargo pkgid --frozen -p easypanel)"
  version="${package_id##*[#@]}"
  # Apple's bundle versions are numeric, unlike Cargo prerelease suffixes.
  version="${version%%[-+]*}"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
  stage="$(mktemp -d "${TMPDIR:-/tmp}/easypanel-desktop.XXXXXX")"
  trap 'rm -rf "$stage"' EXIT
  app="$stage/EasyPanel.app"
  mkdir -p "$app/Contents/MacOS"
  install -m 755 "$binary" "$app/Contents/MacOS/easypanel-desktop"
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.github.mrfansi.easypanel</string>
  <key>CFBundleName</key><string>EasyPanel</string>
  <key>CFBundleDisplayName</key><string>EasyPanel</string>
  <key>CFBundleExecutable</key><string>easypanel-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
  plutil -lint "$app/Contents/Info.plist"
  # Local ad-hoc signing only: no Developer ID certificate or notarization.
  codesign --force --sign - "$app"
  codesign --verify --deep --strict "$app"
  archive="$root/dist/easypanel-desktop-$target.zip"
  ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
  unzip -tq "$archive"
fi
[[ -s "$archive" ]]
printf 'Packaged %s\n' "$archive"
