#!/usr/bin/env sh
# Build release dan pasang binary `easypanel`.
# Override lokasi dengan: PREFIX=~/.local/bin ./install.sh
set -e

cargo build --release

DEST="${PREFIX:-/usr/local/bin}"
mkdir -p "$DEST"
install -m 755 target/release/easypanel "$DEST/easypanel"

echo "Terpasang: $DEST/easypanel"
echo "Pastikan '$DEST' ada di PATH, lalu jalankan: easypanel"
