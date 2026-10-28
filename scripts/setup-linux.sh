#!/usr/bin/env bash
# Installs what a Linux build of Lethora needs (Debian/Ubuntu, apt): the
# system libraries for Tauri 2 (WebKitGTK 4.1, from Ubuntu 22.04 / Debian 12)
# and OpenSSL (server sync), Rust via rustup if missing, then the web
# dependencies. Checks for Node.js 24+ but does not install it.
#
#   scripts/setup-linux.sh
#
# Afterwards: npm --prefix app run tauri dev (or: tauri build). Other
# distributions: https://v2.tauri.app/start/prerequisites/#linux

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v apt-get >/dev/null; then
    echo "No apt-get: install the packages for your distribution from"
    echo "https://v2.tauri.app/start/prerequisites/#linux, then run npm --prefix app install."
    exit 1
fi

echo "== System packages"
sudo apt-get update
if ! apt-cache show libwebkit2gtk-4.1-dev >/dev/null 2>&1; then
    echo "libwebkit2gtk-4.1-dev is not available: Tauri 2 needs Ubuntu 22.04 / Debian 12 or newer."
    exit 1
fi
sudo apt-get install -y \
    build-essential pkg-config curl wget file \
    libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev

echo "== Rust"
if ! command -v cargo >/dev/null && [ -f "$HOME/.cargo/env" ]; then
    . "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    . "$HOME/.cargo/env"
elif command -v rustup >/dev/null; then
    rustup update stable
fi
cargo --version

echo "== Node.js"
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
if [ "$NODE_MAJOR" -lt 24 ]; then
    echo "Node.js 24+ is needed (found: $(node --version 2>/dev/null || echo none))."
    echo "E.g. with nvm (https://github.com/nvm-sh/nvm): nvm install 24, then run this script again."
    exit 1
fi
node --version

echo "== Web dependencies"
npm --prefix "$ROOT/app" install

echo
echo "Done. Open a new shell (or: . \"\$HOME/.cargo/env\"), then:"
echo "  npm --prefix app run tauri dev     # run the app"
echo "  npm --prefix app run tauri build   # .deb and .AppImage in target/release/bundle/"
