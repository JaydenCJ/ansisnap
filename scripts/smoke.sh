#!/usr/bin/env bash
# Smoke test: builds ansisnap, records a snapshot of a script that spews
# progress-bar overdraws and colored output, checks it, breaks it, watches
# the check fail with a rendered-screen diff, re-blesses it, and exercises
# render/diff/list. Self-contained: temp dirs only, no network.
set -euo pipefail

cd "$(dirname "$0")/.."

fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }

echo "[smoke] building..."
cargo build --quiet
BIN=$(pwd)/target/debug/ansisnap

WORK=$(mktemp -d "${TMPDIR:-/tmp}/ansisnap-smoke.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

# --- 1. version/help sanity --------------------------------------------------
"$BIN" --version | grep -q '^ansisnap 0\.1\.0$' || fail "--version mismatch"
"$BIN" --help | grep -q 'COMMANDS:' || fail "--help missing sections"

# --- 2. record: progress noise collapses to the final screen -----------------
cat > app.sh <<'EOF'
#!/bin/sh
printf 'compiling 1%%\rcompiling 57%%\rcompiling 99%%\r\033[2K'
printf '\033[1;32m   PASS\033[0m 42 checks, 0 failed\n'
printf 'artifacts: 3\n'
EOF

echo "[smoke] ansisnap record"
"$BIN" record app -- sh app.sh | tee record.out
grep -q 'recorded app -> .ansisnap/app.snap (exit 0, 80x24' record.out \
  || fail "record summary line wrong"
grep -q '|   PASS 42 checks, 0 failed' .ansisnap/app.snap \
  || fail "snapshot missing final screen row"
grep -q 'compiling' .ansisnap/app.snap && fail "progress overdraw leaked into snapshot"
grep -q 'r0 c0-c6 bold,fg=green' .ansisnap/app.snap || fail "style span not recorded"

# --- 3. check passes, then fails readably after a real change ----------------
echo "[smoke] ansisnap check (expect ok)"
"$BIN" check | tee check1.out
grep -q '^ok      app$' check1.out || fail "clean check did not pass"
grep -q '1 snapshot(s): 1 ok' check1.out || fail "check summary wrong"

sed -i.bak 's/42 checks/41 checks/' app.sh
echo "[smoke] ansisnap check (expect FAIL with screen diff)"
if "$BIN" check > check2.out; then fail "check passed after behavior change"; fi
grep -q '^FAIL    app$' check2.out || fail "failing snapshot not marked FAIL"
grep -q 'row 0 text differs' check2.out || fail "diff lacks row-level detail"
grep -q 'expected |   PASS 42 checks' check2.out || fail "diff lacks expected row"
grep -q 'actual   |   PASS 41 checks' check2.out || fail "diff lacks actual row"

echo "[smoke] ansisnap check --update (re-bless)"
"$BIN" check --update | grep -q '^updated app' || fail "--update did not re-record"
"$BIN" check | grep -q '^ok      app$' || fail "check still failing after --update"

# --- 4. render: emulate a byte capture from stdin ----------------------------
echo "[smoke] ansisnap render"
printf 'step 1/3\rstep 2/3\rstep 3/3\r\033[2K\033[7m DONE \033[0m\n' > raw.bin
"$BIN" render --cols 40 --rows 5 raw.bin > render.out
[ "$(head -1 render.out)" = " DONE" ] || fail "render did not collapse overdraws"
"$BIN" render --styles raw.bin | grep -q 'r0 c0-c5 reverse' || fail "--styles span missing"

# --- 5. diff: different bytes, same screen => identical ----------------------
echo "[smoke] ansisnap diff"
printf '\033[1;31mboom\033[0m\n' > a.bin
printf 'zzzz\r\033[31m\033[1mboom\033[0m\n' > b.bin
printf '\033[1;31mbeep\033[0m\n' > c.bin
"$BIN" diff a.bin b.bin | grep -q 'screens identical' \
  || fail "equivalent escape streams reported as different"
if "$BIN" diff a.bin c.bin > diff.out; then fail "diff missed a text change"; fi
grep -q 'row 0 text differs' diff.out || fail "diff output lacks row detail"

# --- 6. list + guardrails -----------------------------------------------------
"$BIN" list | grep -qE '^app	80x24	exit 0	\["sh","app.sh"\]$' || fail "list row wrong"
if "$BIN" record '../evil' -- true 2> err.out; then fail "path-escaping name accepted"; fi
grep -q 'invalid snapshot name' err.out || fail "name rejection message missing"

echo "SMOKE OK"
