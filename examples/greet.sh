#!/bin/sh
# A deliberately noisy CLI: progress-bar overdraws, an erase-line, bold
# colored output. Byte-level snapshots of this churn on every cosmetic
# tweak; the rendered screen is just two stable rows.
printf 'linting 12%%\rlinting 68%%\rlinting 100%%\r\033[2K'
printf '\033[1;32m   PASS\033[0m src/lib.rs (14 checks)\n'
printf '\033[1;32m   PASS\033[0m src/cli.rs (9 checks)\n'
