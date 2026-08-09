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
