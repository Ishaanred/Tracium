#!/usr/bin/env bash
# Fetch bundled sidecar binaries into src-tauri/binaries/ named for the current
# Rust host target triple, as Tauri's `externalBin` expects. These are NOT
# committed to git — run this once before `pnpm tauri dev` / `pnpm tauri build`.
#
# Currently bundles: librespeed-cli (speed test engine, LGPL-3.0).
set -euo pipefail

LIBRESPEED_VERSION="${LIBRESPEED_VERSION:-1.0.13}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$here/src-tauri/binaries"
mkdir -p "$out"

triple="$(rustc -vV | awk '/^host:/{print $2}')"
echo "host triple: $triple"

# Map the Rust triple to librespeed-cli's release asset naming.
case "$triple" in
  x86_64-unknown-linux-gnu)   asset="linux_amd64";  ext="" ;;
  aarch64-unknown-linux-gnu)  asset="linux_arm64";  ext="" ;;
  x86_64-pc-windows-msvc)     asset="windows_amd64"; ext=".exe" ;;
  x86_64-apple-darwin)        asset="darwin_amd64"; ext="" ;;
  aarch64-apple-darwin)       asset="darwin_arm64"; ext="" ;;
  *) echo "unsupported triple: $triple" >&2; exit 1 ;;
esac

url="https://github.com/librespeed/speedtest-cli/releases/download/v${LIBRESPEED_VERSION}/librespeed-cli_${LIBRESPEED_VERSION}_${asset}.tar.gz"
dest="$out/librespeed-cli-$triple$ext"

echo "downloading $url"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/ls.tar.gz" "$url"
tar -xzf "$tmp/ls.tar.gz" -C "$tmp"
install -m 0755 "$tmp/librespeed-cli$ext" "$dest"
echo "installed sidecar: $dest"
