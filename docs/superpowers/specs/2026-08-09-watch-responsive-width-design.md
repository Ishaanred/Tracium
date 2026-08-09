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

## Revision (2026-08-10): percentage-of-terminal-width abandoned

Manual testing on a real (wide) terminal showed the percentage-of-width
formula above produces bad output: target labels, IPs, and DNS resolver
names are short, bounded-length strings that don't need to grow just
because the terminal is wide. At a clamped-max 200-column reading, `host_w`
alone was 60 characters — stretching a 7-character IP address like
`1.1.1.1` across a 60-column field produces large, ugly dead gaps between
columns, which is worse than the original cramped-at-80-columns problem
this work set out to fix.

**Corrected approach:** column widths are now fit to the *content* actually
being displayed each tick, not to terminal width at all:

```
column_width = clamp(longest_current_value + margin, floor, cap)
```

where `margin` is a small fixed gap (2 chars) and `floor`/`cap` bound the
result per column (floor = today's original hardcoded widths, unchanged;
cap = a generous but finite ceiling so one pathologically long value can't
blow out the column). This self-adjusts to whatever data is actually on
screen — short IPs stay tight, a long hostname or resolver name gets the
room it needs — and is independent of terminal width entirely. The
`terminal_size` dependency and the `cols` parameter are dropped; this
function no longer needs to know the terminal's size.

Revised columns and bounds:
- target label: floor 14, cap 24
- target host: floor 24, cap 45 (room for a full IPv6 address + margin)
- devices IP: floor 16, cap 20
- DNS resolver name: floor 12, cap 24

## Revision (2026-08-10): alternate screen buffer

Separately, manual testing surfaced a second, unrelated point of confusion:
`watch`'s clear-screen-per-tick approach (`\x1b[2J\x1b[H`) does overwrite in
place when watched live, but every tick still lands in the terminal's
scrollback — so copy-pasting or scrolling up shows every past frame
stacked, which reads as "it keeps printing new screens instead of
updating." The fix is the standard terminal-dashboard idiom (used by
`htop`, `less`, `vim`): switch into the **alternate screen buffer** for the
duration of `watch`, and switch back on exit. This is a separate screen
that scrollback never sees; exiting restores the original screen exactly
as it was. No new dependency needed — this is two more ANSI escape
sequences (`\x1b[?1049h` to enter, `\x1b[?1049l` to leave), following the
same string-based approach `watch` already uses for `\x1b[2J\x1b[H`.
Entering happens once, right after the startup banner (which stays on the
normal screen, so it's still visible in scrollback afterward); leaving
happens once, on both the normal Ctrl-C exit path and — via a cleanup guard
— any early return, so a crash mid-loop can't strand the user's terminal
in the alternate buffer.
