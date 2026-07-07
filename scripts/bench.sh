#!/usr/bin/env bash
# Measure Tracium's runtime footprint (CPU + RAM) on Linux, RELEASE build.
#
# Builds the release binary (with the embedded frontend), launches it, lets the
# monitor settle, then samples CPU (summed across the app + its WebKit helper
# processes) and RAM as PSS (Proportional Set Size — the honest figure that
# accounts for shared libraries, unlike RSS which over-counts).
#
# Usage:  ./scripts/bench.sh [sample_secs=20] [settle_secs=25]
# Requires a display (it's a GUI app). See scripts/bench.ps1 for Windows.
set -euo pipefail
cd "$(dirname "$0")/.."

SAMPLE="${1:-20}"
SETTLE="${2:-25}"
BIN="target/release/tracium"

echo "building release (frontend + rust)…"
pnpm build >/dev/null 2>&1
cargo build --release -p tracium >/dev/null 2>&1
[ -x "$BIN" ] || { echo "ERROR: $BIN not found"; exit 1; }

echo "launching $BIN and settling ${SETTLE}s…"
"$BIN" >/tmp/tracium-bench.log 2>&1 &
APP=$!
trap 'kill $APP 2>/dev/null || true' EXIT
sleep "$SETTLE"

# Collect the whole process tree (main + WebKit web/network/GPU helpers).
pids() {
  local frontier="$APP" all="$APP" kids
  while :; do
    kids=$(for p in $frontier; do pgrep -P "$p" 2>/dev/null || true; done | tr '\n' ' ')
    [ -z "${kids// /}" ] && break
    all="$all $kids"; frontier="$kids"
  done
  echo "$all" | tr ' ' '\n' | grep -E '^[0-9]+$' | sort -un
}
PIDS=$(pids)
NPROC=$(nproc)
HZ=$(getconf CLK_TCK)

# RAM: sum PSS across the tree.
pss_kb=0
for p in $PIDS; do
  v=$(awk '/^Pss:/{s+=$2} END{print s+0}' "/proc/$p/smaps_rollup" 2>/dev/null || echo 0)
  pss_kb=$((pss_kb + ${v:-0}))
done

# CPU: sum utime+stime jiffies across the tree over the sample window.
jiffies() { local t=0 j; for p in $PIDS; do j=$(awk '{print $14+$15}' "/proc/$p/stat" 2>/dev/null || echo 0); t=$((t + ${j:-0})); done; echo "$t"; }
j1=$(jiffies); sleep "$SAMPLE"; j2=$(jiffies)
cpu=$(awk "BEGIN{printf \"%.2f\", ($j2-$j1)/$HZ/$SAMPLE*100}")

# DB footprint.
db="$HOME/.local/share/com.tracium.app/tracium.db"
db_kb=$(du -kc "$db" "$db-wal" 2>/dev/null | awk 'END{print $1}')

echo
echo "================ Tracium footprint (release, idle, window open) ================"
printf "processes      : %s (app + WebKit helpers)\n" "$(echo "$PIDS" | wc -l)"
printf "RAM (PSS total): %.0f MB\n" "$(awk "BEGIN{print $pss_kb/1024}")"
printf "CPU idle       : %s%% of one core (%s cores) over %ss\n" "$cpu" "$NPROC" "$SAMPLE"
printf "DB on disk     : %.1f MB\n" "$(awk "BEGIN{print ${db_kb:-0}/1024}")"
echo "================================================================================"
