#!/usr/bin/env sh
# Build release dan pasang binary `easypanel`.
# Override lokasi dengan: PREFIX=~/.local/bin ./install.sh
set -e

cargo build --release

DEST="${PREFIX:-/usr/local/bin}"
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

echo "Pastikan '$DEST' ada di PATH, lalu jalankan: easypanel"
