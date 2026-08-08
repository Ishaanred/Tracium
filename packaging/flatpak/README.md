# Flathub packaging (not yet build-ready)

`com.tracium.app.yml` is a skeleton, not a working manifest. Flathub's build
servers have **no network access** during the build, so every Rust crate and
pnpm package has to be pre-fetched and listed as a local source. This is more
work than the AUR/Snap manifests and is worth doing once there's real user
demand — steps below for whenever that happens.

1. Install the generators:
   ```bash
   pip install --user aiohttp toml
   git clone https://github.com/flatpak/flatpak-builder-tools.git
   ```

2. Generate the cargo source list (run from the repo root, needs `Cargo.lock`):
   ```bash
   python3 flatpak-builder-tools/cargo/flatpak-cargo-generator.py \
     Cargo.lock -o packaging/flatpak/cargo-sources.json
   ```

3. Generate the pnpm/node source list (needs `pnpm-lock.yaml`):
   ```bash
   python3 flatpak-builder-tools/node/flatpak-node-generator.py \
     pnpm packaging/flatpak/../../pnpm-lock.yaml -o packaging/flatpak/node-sources.json
   ```

4. Test locally before ever opening a Flathub PR:
   ```bash
   flatpak-builder --user --install --force-clean build-dir \
     packaging/flatpak/com.tracium.app.yml
   ```

5. Once it builds clean, follow Flathub's submission process: fork
   `github.com/flathub/flathub`, open a PR adding this manifest (plus the two
   generated JSON files) under a new `com.tracium.app` repo, per
   <https://docs.flathub.org/docs/for-app-authors/submission>.

Re-run steps 2–4 on every dependency bump — the generated JSON pins exact
versions and hashes, so it goes stale whenever `Cargo.lock` or
`pnpm-lock.yaml` changes.
