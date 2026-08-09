# `traciumd watch` responsive column widths — design

## Why

`traciumd watch` (expanded in the prior `2026-08-09-cli-watch-expansion` work)
formats several sections with hardcoded fixed-width columns: the target
list's label/host columns (`{:14} {:24}`), the devices section's IP column
(`{:16}`), and the DNS section's resolver-name column (`{:12}`). These
widths were sized for an ~80-column terminal. On a wider terminal the text
stays left-crammed at that width, leaving the right side of the window
empty — the dashboard doesn't use the space it has.

## Scope

1. Add the `terminal_size` crate to `crates/cli` to read the terminal's
   column count.
2. Compute four column widths once per tick in `watch()`, derived from the
   terminal width with a floor at today's values (so an 80-column terminal
   or a non-TTY renders identically to before) and a ceiling so a very wide
   terminal doesn't stretch columns absurdly:
   - `label_w = max(14, cols * 18 / 100)` — target label column
   - `host_w = max(24, cols * 30 / 100)` — target host column
   - `ip_w = max(16, cols * 20 / 100)` — devices IP column
   - `resolver_w = max(12, cols * 15 / 100)` — DNS resolver-name column
   - `cols` itself is `terminal_size()`'s reported width, falling back to
     `80` when not a TTY (e.g. piped output), clamped to `60..=200`.
3. Use these computed widths in place of the hardcoded literals in the
   `status` and `devices` and `dns` render blocks.

Explicitly out of scope: any multi-column/grid layout (sections stay
stacked vertically, one per row, as today); resizing mid-run without
re-querying (each tick re-reads the terminal size, so a live resize is
picked up naturally — no special-case handling needed); changing anything
about the sparkline, banner, or single-line sections, which don't use
fixed-width columns.

## Implementation notes

`terminal_size::terminal_size()` returns `Option<(Width, Height)>`, `None`
when stdout isn't a TTY. Read once per tick (cheap — no polling, no thread)
immediately before rendering, alongside the existing per-tick store
queries. This one function call is the entire per-tick cost this feature
adds.

## Testing

- Unit test the width-derivation function (a pure `fn column_widths(cols: usize) -> (usize, usize, usize, usize)` or similar) at: the floor case (`cols` below or at 80, all four widths equal today's hardcoded values), a wide case (e.g. `cols = 160`, all four widths larger than the floor), and the clamp boundaries (`cols` far below 60 and far above 200 both clamp before the formula applies).
- No integration/DB test needed — this only changes string formatting.
