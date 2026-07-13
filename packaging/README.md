# Packaging

Where Tracium is (or could be) distributed beyond GitHub Releases.

## AUR (Arch Linux) — ready

`aur/PKGBUILD` builds the desktop app + `traciumd` from source and installs
both, plus the systemd user unit. To publish:

```bash
# One-time: create an AUR account + SSH key at https://aur.archlinux.org
git clone ssh://aur@aur.archlinux.org/tracium.git aur-tracium
cp packaging/aur/PKGBUILD aur-tracium/
cd aur-tracium
makepkg --printsrcinfo > .SRCINFO   # generates the required metadata file
makepkg -si                          # test-build locally first
git add PKGBUILD .SRCINFO
git commit -m "Initial release: 0.1.0"
git push
```

Bump `pkgver`/`pkgrel` and regenerate `.SRCINFO` on every release.

## Snap Store — ready to test

`snap/snapcraft.yaml` builds with `snapcraft` (core22, strict confinement,
GNOME extension for the webview). To publish:

```bash
sudo snap install snapcraft --classic
cd packaging/snap
snapcraft                             # produces tracium_0.1.0_amd64.snap
snapcraft login                       # one-time, needs an Ubuntu One account
snapcraft register tracium            # one-time, claims the name
snapcraft upload tracium_0.1.0_amd64.snap --release=stable
```

## Flathub — needs vendored deps first

See `flatpak/README.md`. Flathub builds offline, so the cargo/pnpm
dependency trees need to be pre-generated before the manifest will actually
build. Worth doing once there's demand beyond AUR/Snap.

## crates.io — `traciumd` only

Only the Rust side makes sense to publish; the desktop app is a Tauri bundle,
not a library. Path dependencies between the internal crates already carry
version numbers so `cargo publish` will resolve them from crates.io instead
of the local path once published. Publish in dependency order (one-time
`cargo login <token>` first, using a token from
https://crates.io/settings/tokens):

```bash
cargo publish -p tracium-store
cargo publish -p tracium-probe
cargo publish -p tracium-monitor
cargo publish -p tracium-cli
```

Then `cargo install tracium-cli` installs `traciumd` for anyone with a Rust
toolchain. Re-run in this order (leaf crates first) on every version bump.

## npm — not applicable

`package.json` is the frontend half of the Tauri app (`"private": true`),
not a standalone library — there's nothing here to `npm install`.
