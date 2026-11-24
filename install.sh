#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

command -v cargo >/dev/null 2>&1 || { echo 'Thiếu cargo (Rust toolchain). Hãy cài rustup trước.'; exit 1; }
pkg-config --exists gtk4 || { echo 'Thiếu libgtk-4-dev. Chạy: sudo apt install -y build-essential pkg-config libgtk-4-dev libcairo2-dev libpango1.0-dev'; exit 1; }

echo "Đang biên dịch Kai bản release..."
cargo build --release --manifest-path "$HERE/Cargo.toml"

BIN_DIR="$HOME/.local/bin"
DESKTOP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$BIN_DIR" "$DESKTOP_DIR" "$ICON_DIR"

install -m 755 "$HERE/target/release/kai" "$BIN_DIR/kai"
install -m 644 "$HERE/kai.svg" "$ICON_DIR/kai.svg"
sed "s|^Exec=.*|Exec=$BIN_DIR/kai|" "$HERE/kai.desktop" > "$DESKTOP_DIR/kai.desktop"
chmod 644 "$DESKTOP_DIR/kai.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DESKTOP_DIR" >/dev/null 2>&1 || true
fi

printf '\nĐã cài Kai (Rust) vào App Grid. Tìm "Kai" trong danh sách ứng dụng GNOME.\n'
