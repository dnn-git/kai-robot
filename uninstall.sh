#!/usr/bin/env bash
set -euo pipefail
rm -f "$HOME/.local/bin/kai" "$HOME/.local/share/applications/kai.desktop" "$HOME/.local/share/icons/hicolor/scalable/apps/kai.svg"
rm -rf "$HOME/.local/share/kai"
echo 'Đã gỡ ứng dụng Kai. Cài đặt cá nhân tại ~/.config/kai vẫn được giữ lại.'
