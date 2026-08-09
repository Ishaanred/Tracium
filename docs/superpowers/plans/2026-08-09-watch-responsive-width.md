# Responsive `traciumd watch` Column Widths Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `traciumd watch`'s fixed-width columns (target label/host, devices IP, DNS resolver name) scale with the terminal's actual width instead of being hardcoded for ~80 columns, so wide terminals stop rendering with wasted space on the right.

**Architecture:** Add the `terminal_size` crate to read the terminal's column count once per tick. A pure function `column_widths(cols: usize) -> (usize, usize, usize, usize)` derives the four widths from that count with a floor at today's hardcoded values and a clamp on the input. `watch()` calls it once per tick and uses the returned widths in place of the hardcoded `{:14}`/`{:24}`/`{:16}`/`{:12}` literals in the `status`, `devices`, and `dns` render blocks.

**Tech Stack:** Rust, one new dependency (`terminal_size`), same crate (`crates/cli`) as the rest of `traciumd`.

## Global Constraints

- Floor values match today's hardcoded widths exactly: label 14, host 24, ip 16, resolver 12 — so an 80-column terminal (or non-TTY/piped output) renders identically to before this change.
- `cols` (the raw terminal width reading) is clamped to `60..=200` before the formula is applied, so extremely narrow or extremely wide readings can't produce degenerate output.
- No layout restructuring — sections stay stacked vertically, one per row, exactly as today. Only the column widths inside `status`/`devices`/`dns` change.
- Re-read terminal size every tick (cheap, one function call) — no caching, no special-case resize handling; a live resize is picked up naturally on the next tick.
- Follow the existing file's test convention: add tests to the same `#[cfg(test)] mod tests` block already at the bottom of `crates/cli/src/main.rs`.

Reference spec: `docs/superpowers/specs/2026-08-09-watch-responsive-width-design.md`

---

## File Structure

Everything lives in the two existing files:

- `crates/cli/Cargo.toml`: add `terminal_size = "0.4"` to `[dependencies]`, alongside the existing `printpdf`/`dirs` entries (this crate adds deps directly rather than via `[workspace.dependencies]`, matching its existing pattern).
- `crates/cli/src/main.rs`: new pure function `column_widths`, plus updated `watch()` call sites in the `status`, `devices`, and `dns` render blocks.

---

### Task 1: Responsive column widths

**Files:**
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Produces: `fn column_widths(cols: usize) -> (usize, usize, usize, usize)` returning `(label_w, host_w, ip_w, resolver_w)` in that order — used by `watch()`'s `status`, `devices`, and `dns` render blocks.

- [ ] **Step 1: Add the dependency**

In `crates/cli/Cargo.toml`, in the `[dependencies]` section (alongside `printpdf`/`dirs`), add:

```toml
terminal_size = "0.4"
```

- [ ] **Step 2: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block at the bottom of `crates/cli/src/main.rs`:

```rust
    #[test]
    fn column_widths_floor_matches_todays_hardcoded_values_at_80_cols() {
        assert_eq!(column_widths(80), (14, 24, 16, 12));
    }

    #[test]
    fn column_widths_floor_holds_below_80_cols() {
        // Below the floor, widths never shrink past today's hardcoded values.
        assert_eq!(column_widths(60), (14, 24, 16, 12));
    }

    #[test]
    fn column_widths_grow_on_a_wide_terminal() {
        let (label_w, host_w, ip_w, resolver_w) = column_widths(160);
        assert!(label_w > 14, "label_w should grow past the floor at 160 cols, got {label_w}");
        assert!(host_w > 24, "host_w should grow past the floor at 160 cols, got {host_w}");
        assert!(ip_w > 16, "ip_w should grow past the floor at 160 cols, got {ip_w}");
        assert!(resolver_w > 12, "resolver_w should grow past the floor at 160 cols, got {resolver_w}");
    }

    #[test]
    fn column_widths_clamps_extreme_input() {
        // An absurdly narrow reading clamps up to 60 before the formula applies,
        // so it matches the 60-cols case exactly.
        assert_eq!(column_widths(10), column_widths(60));
        // An absurdly wide reading clamps down to 200 before the formula applies,
        // so it matches the 200-cols case exactly.
        assert_eq!(column_widths(10_000), column_widths(200));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p tracium-cli column_widths`
Expected: FAIL to compile — `column_widths` not defined.

- [ ] **Step 4: Implement `column_widths`**

Add above `watch()`, alongside the other pure helpers (`resolve_sections`, `sparkline`, `route_changed`):

```rust
/// Derive the target label/host, devices IP, and DNS resolver-name column
/// widths from the terminal's column count. Floors match the widths this
/// dashboard used before this feature existed (14/24/16/12), so an 80-column
/// terminal or a non-TTY (piped output, where `cols` is the 80-column
/// fallback) renders identically to before. `cols` is clamped to 60..=200
/// before the formula applies, so a degenerate reading can't produce
/// degenerate output.
fn column_widths(cols: usize) -> (usize, usize, usize, usize) {
    let cols = cols.clamp(60, 200);
    let label_w = (cols * 18 / 100).max(14);
    let host_w = (cols * 30 / 100).max(24);
    let ip_w = (cols * 20 / 100).max(16);
    let resolver_w = (cols * 15 / 100).max(12);
    (label_w, host_w, ip_w, resolver_w)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p tracium-cli column_widths`
Expected: PASS (4 tests)

- [ ] **Step 6: Wire it into `watch()`**

Near the top of `watch()`'s loop body — the same place the other per-tick values are computed, right after the existing `let avg_latency = { ... };` block — add:

```rust
        let term_cols = terminal_size::terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(80);
        let (label_w, host_w, ip_w, resolver_w) = column_widths(term_cols);
```

Then replace the three hardcoded-width format strings inside the loop:

In the `status` block, replace:
```rust
                buf.push_str(&format!("    {:14} {:24} {}\n", t.label, t.host, state));
```
with:
```rust
                buf.push_str(&format!(
                    "    {:label_w$} {:host_w$} {}\n",
                    t.label, t.host, state,
                ));
```

In the `devices` block, replace:
```rust
                buf.push_str(&format!(
                    "    {:16} {}\n",
                    d.ip.as_deref().unwrap_or("?"),
                    d.hostname.as_deref().unwrap_or_else(|| d.mac.as_deref().unwrap_or("")),
                ));
```
with:
```rust
                buf.push_str(&format!(
                    "    {:ip_w$} {}\n",
                    d.ip.as_deref().unwrap_or("?"),
                    d.hostname.as_deref().unwrap_or_else(|| d.mac.as_deref().unwrap_or("")),
                ));
```

In the `dns` block, replace:
```rust
                    buf.push_str(&format!(
                        "  dns: {:12} {:>8}  {} lookups, {} failures\n",
                        s.resolver,
                        s.avg_ms.map(|v| format!("{v:.1}ms")).unwrap_or_else(|| "—".into()),
                        s.count,
                        s.failures,
                    ));
```
with:
```rust
                    buf.push_str(&format!(
                        "  dns: {:resolver_w$} {:>8}  {} lookups, {} failures\n",
                        s.resolver,
                        s.avg_ms.map(|v| format!("{v:.1}ms")).unwrap_or_else(|| "—".into()),
                        s.count,
                        s.failures,
                    ));
```

(Rust's `format!` supports named-argument width specifiers like `{:label_w$}` directly against a local variable in scope — no extra positional arguments needed.)

- [ ] **Step 7: Build and run the full test suite**

Run: `cargo build -p tracium-cli`
Expected: builds cleanly, no warnings.

Run: `cargo test -p tracium-cli`
Expected: PASS — 17 tests total (13 pre-existing + 4 new `column_widths` tests).

- [ ] **Step 8: Manually verify at two terminal widths**

Run `cargo run -p tracium-cli --bin traciumd -- watch --interval 1` in a normal-width terminal (Ctrl-C after a couple of ticks) and confirm the target/devices/dns columns look the same as before this change. Then widen the terminal significantly (or run inside `tmux`/a resizable pane) and re-run — confirm the same columns visibly widen rather than staying cramped against the left edge.

- [ ] **Step 9: Commit**

```bash
git add crates/cli/Cargo.toml crates/cli/src/main.rs Cargo.lock
git commit -m "cli: scale watch's column widths to terminal width"
```

---

## Self-Review Notes

- **Spec coverage:** the spec's three scope items (dependency, `column_widths` derivation with floor/ceiling, wiring into `status`/`devices`/`dns`) are all in Step 1/4/6. The spec's explicit out-of-scope items (grid layout, resize special-casing, sparkline/banner changes) are untouched by this plan.
- **Placeholder scan:** none — every step has runnable code.
- **Type consistency:** `column_widths(cols: usize) -> (usize, usize, usize, usize)` is defined in Step 4 and consumed with the same tuple order in Step 6; the `label_w`/`host_w`/`ip_w`/`resolver_w` names are used consistently in both the destructuring and the format strings' named-argument syntax.

---

## Task 2 (revision): content-fit widths + alternate screen buffer

Manual testing on a real terminal showed Task 1's percentage-of-width
formula produces bad output on wide terminals (see the design spec's
2026-08-10 revision notes for the full rationale). This task replaces
Task 1's terminal-width-driven `column_widths` with a content-fit version,
and separately adds the alternate-screen-buffer fix for scrollback
pollution. Both are small enough to land as one task.

**Files:**
- Modify: `crates/cli/Cargo.toml` (remove `terminal_size` — no longer needed)
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Replaces: `fn column_widths(cols: usize) -> (usize, usize, usize, usize)` with `fn fit_width(values: impl Iterator<Item = usize>, floor: usize, margin: usize, cap: usize) -> usize`.
- Produces (new): alternate-screen enter/exit calls around `watch()`'s loop.

- [ ] **Step 1: Remove the now-unneeded dependency**

In `crates/cli/Cargo.toml`, delete the `terminal_size = "0.4"` line added by Task 1.

- [ ] **Step 2: Replace `column_widths`'s tests with `fit_width` tests**

Replace the four `column_widths_*` tests in `#[cfg(test)] mod tests` with:

```rust
    #[test]
    fn fit_width_uses_floor_when_content_is_short() {
        let values = ["1.1.1.1".len(), "8.8.8.8".len()].into_iter();
        assert_eq!(fit_width(values, 16, 2, 20), 16);
    }

    #[test]
    fn fit_width_grows_to_fit_a_long_value_plus_margin() {
        // "2606:4700:4700::1111" is 21 chars; +2 margin = 23, within the cap.
        let values = ["1.1.1.1".len(), "2606:4700:4700::1111".len()].into_iter();
        assert_eq!(fit_width(values, 24, 2, 45), 23_usize.max(24));
    }

    #[test]
    fn fit_width_caps_a_pathologically_long_value() {
        let values = [100usize].into_iter();
        assert_eq!(fit_width(values, 12, 2, 24), 24);
    }

    #[test]
    fn fit_width_falls_back_to_floor_on_empty_input() {
        let values: std::iter::Empty<usize> = std::iter::empty();
        assert_eq!(fit_width(values, 14, 2, 24), 14);
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p tracium-cli fit_width`
Expected: FAIL to compile — `fit_width` not defined (and the old `column_widths` tests you replaced are gone, so no conflict).

- [ ] **Step 4: Replace `column_widths` with `fit_width`**

Replace the entire `column_widths` function with:

```rust
/// Fit a column's width to the longest value currently being displayed in
/// it, not to terminal width — target labels, IPs, and resolver names are
/// short, bounded-length strings that don't need to grow just because the
/// terminal is wide. `margin` is a small fixed gap added after the longest
/// value; the result is clamped to `[floor, cap]` so short content doesn't
/// shrink below today's original widths and one pathologically long value
/// can't blow out the column.
fn fit_width(values: impl Iterator<Item = usize>, floor: usize, margin: usize, cap: usize) -> usize {
    let longest = values.max().unwrap_or(0);
    (longest + margin).clamp(floor, cap)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p tracium-cli fit_width`
Expected: PASS (4 tests)

- [ ] **Step 6: Rewire `watch()` to call `fit_width` per column, using this tick's actual data**

Replace:
```rust
        let term_cols = terminal_size::terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(80);
        let (label_w, host_w, ip_w, resolver_w) = column_widths(term_cols);
```
with:
```rust
        let label_w = fit_width(targets.iter().map(|t| t.label.chars().count()), 14, 2, 24);
        let host_w = fit_width(targets.iter().map(|t| t.host.chars().count()), 24, 2, 45);
        let active_devices: Vec<_> = devices.iter().filter(|d| d.is_active).collect();
        let ip_w = fit_width(active_devices.iter().map(|d| d.ip.as_deref().unwrap_or("?").chars().count()), 16, 2, 20);
        let resolver_w = fit_width(dns.iter().map(|s| s.resolver.chars().count()), 12, 2, 24);
```

Then, in the `devices` render block, since `active_devices` is now computed above (outside the block), replace the block's own `let active: Vec<_> = devices.iter().filter(|d| d.is_active).collect();` line with nothing (delete it) and use the outer `active_devices` binding in its place for the rest of that block (`active_devices.len()`, `active_devices.iter().take(8)`, `active_devices.len() > 8`).

- [ ] **Step 7: Add the alternate-screen buffer**

Right after the existing `print_banner(db, interval, &sections);` call and its `tokio::time::sleep(Duration::from_millis(700)).await;` line (both stay on the normal screen, so the banner remains visible in scrollback), add:

```rust
    print!("\x1b[?1049h"); // enter alternate screen — scrollback never sees the live loop
    std::io::stdout().flush().ok();
```

Immediately before both `break` points that end the loop — the `tokio::signal::ctrl_c()` arm's `{ println!(); break; }` — replace it with:

```rust
            _ = tokio::signal::ctrl_c() => {
                print!("\x1b[?1049l"); // leave alternate screen, restore the user's terminal
                std::io::stdout().flush().ok();
                break;
            }
```

Also wrap the function's `?`-propagating store calls so an early error return still restores the screen: this loop's `?` on `store.latest_per_target().await?` etc. would otherwise strand the terminal in the alternate buffer if a query fails after the buffer is entered. Add a small guard immediately after the `print!("\x1b[?1049h")` line:

```rust
    struct RestoreScreen;
    impl Drop for RestoreScreen {
        fn drop(&mut self) {
            print!("\x1b[?1049l");
            std::io::stdout().flush().ok();
        }
    }
    let _restore_screen = RestoreScreen;
```

Then remove the manual `print!("\x1b[?1049l")` from the Ctrl-C arm added above (the `Drop` guard now handles all exit paths — Ctrl-C, an early `?` return, or a panic — uniformly), leaving that arm as just:

```rust
            _ = tokio::signal::ctrl_c() => { println!(); break; }
```

(`println!()` still runs before the guard drops at function end, so the cursor lands on a fresh line on the normal screen after the alternate buffer closes.)

- [ ] **Step 8: Build and run the full test suite**

Run: `cargo build -p tracium-cli`
Expected: builds cleanly, no warnings (confirm `terminal_size` no longer appears in `cargo tree -p tracium-cli` output).

Run: `cargo test -p tracium-cli`
Expected: PASS — 17 tests total (13 from the original expansion + 4 new `fit_width` tests; the 4 old `column_widths` tests were replaced, not added to, so the count doesn't grow further).

- [ ] **Step 9: Manually verify**

Run `cargo run -p tracium-cli --bin traciumd -- watch --interval 1` in a real (wide) terminal, Ctrl-C after a few ticks, and confirm: (a) columns stay tight around actual IP/label lengths instead of stretching across the terminal; (b) the screen updates in place with no scrollback growth — scroll up after Ctrl-C and the alternate-screen content should be gone, with only the startup banner and your shell prompt visible; (c) the terminal is left in a normal, usable state after Ctrl-C (cursor visible, prompt behaves normally).

- [ ] **Step 10: Commit**

```bash
git add crates/cli/Cargo.toml crates/cli/src/main.rs Cargo.lock
git commit -m "cli: fit watch's columns to content and use the alternate screen buffer"
```
