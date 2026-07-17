#!/usr/bin/env sh
# Build release dan pasang binary `easypanel`.
# Override lokasi dengan: PREFIX=~/.local/bin ./install.sh
set -e

cargo build --release

# Default: lokasi yang on-PATH TANPA sudo.
# - PREFIX override selalu menang.
# - /usr/local/bin ada di PATH bawaan macOS, tapi milik root di Apple Silicon
#   (butuh sudo) — pakai hanya kalau memang writable.
# - Kalau tidak, jatuh ke ~/.cargo/bin: script ini baru saja menjalankan cargo,
#   jadi toolchain Rust pasti ada, dan rustup sudah menaruh dir itu di PATH.
#   ~/.local/bin sengaja TIDAK dipakai: bukan bagian PATH default macOS.
if [ -n "$PREFIX" ]; then
    DEST="$PREFIX"
elif [ -w /usr/local/bin ]; then
    DEST="/usr/local/bin"
else
    DEST="${CARGO_HOME:-$HOME/.cargo}/bin"
fi
mkdir -p "$DEST"
install -m 755 target/release/easypanel "$DEST/easypanel"

echo "Terpasang: $DEST/easypanel"

# Completion dipasang hanya ke direktori yang memang sudah ada: membuat direktori
# completion sendiri di sistem orang lain bukan urusan installer ini.
BIN="$DEST/easypanel"
install_completion() {
    dir="$1"; file="$2"; shell="$3"
    [ -d "$dir" ] || return 0
    [ -w "$dir" ] || return 0
    "$BIN" completions "$shell" > "$dir/$file" 2>/dev/null &&
        echo "Completion $shell: $dir/$file"
}
install_completion "${ZDOTDIR:-$HOME}/.zfunc" "_easypanel" zsh
install_completion "$HOME/.config/fish/completions" "easypanel.fish" fish
for d in /etc/bash_completion.d /usr/local/etc/bash_completion.d "$HOME/.local/share/bash-completion/completions"; do
    install_completion "$d" "easypanel" bash && break
done

# Man page, sama seperti completion: hanya ke direktori yang sudah ada.
for m in "$(dirname "$DEST")/share/man/man1" /usr/local/share/man/man1 "$HOME/.local/share/man/man1"; do
    if [ -d "$m" ] && [ -w "$m" ]; then
        "$BIN" man > "$m/easypanel.1" 2>/dev/null && echo "Man page: $m/easypanel.1"
        break
    fi
done

echo "Pastikan '$DEST' ada di PATH, lalu jalankan: easypanel"
