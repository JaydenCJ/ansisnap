# The `.snap` format (v1)

A snapshot is a plain UTF-8 text file, designed to be committed and read in
code review. The first line is a magic/version marker; parsers must reject
anything else, and corrupt files fail with a **positioned** error
(`line N: ...`) rather than comparing garbage.

```text
ansisnap snapshot v1
cmd: ["sh","greet.sh"]
term: 80x24
exit: 0
--- screen: 24 rows x 80 cols ---
|   PASS src/lib.rs (14 checks)
|   PASS src/cli.rs (9 checks)
|
… (one |-prefixed line per grid row, 24 in total)
--- styles: 2 spans ---
r0 c0-c6 bold,fg=green
r1 c0-c6 bold,fg=green
--- stderr: 0 lines ---
```

## Header

| Line | Meaning |
|---|---|
| `ansisnap snapshot v1` | Magic + format version. Anything else is rejected at line 1. |
| `cmd: ["…","…"]` | The recorded argv, JSON-style string array (escapes: `\\`, `\"`, `\n`, `\t`, `\u00XX`). `check` re-runs exactly this. |
| `term: <cols>x<rows>` | Emulated terminal size, 1–1000 each. `check` re-renders at this size. |
| `exit: <code>` | The recorded exit code (`-1` = killed by a signal). Part of the assertion. |

## Screen section

`--- screen: R rows x C cols ---` is followed by exactly `R` rows, each
prefixed with `|`. Rows store *visible text only*: trailing cells that are a
plain space are trimmed (any non-default styling of those cells survives in
the styles section, so nothing observable is lost). A row wider than `C`
display columns is a parse error — wide (CJK/kana/fullwidth/emoji)
characters count as two columns.

## Styles section

`--- styles: N spans ---` (`1 span` when N is 1; parsers accept either
form) is followed by `N` lines of the form
`r<row> c<start>-c<end> <style>`: one line per maximal run of cells sharing
one non-default style, columns 0-based and inclusive. `<style>` is a
comma-joined list in fixed order — attributes (`bold`, `dim`, `italic`,
`underline`, `blink`, `reverse`, `hidden`, `strike`), then `fg=`, then
`bg=`. Colors are written as names (`red`, `bright-cyan`), palette indices
(`idx208`), or hex (`#ff8800`). Palette indices 0–15 normalize to names, so
`ESC[38;5;1m` and `ESC[31m` produce identical snapshots.

## Stderr section

`--- stderr: N lines ---` (`1 line` when N is 1; parsers accept either
form) is followed by `N` `|`-prefixed lines. Stderr is
stored as *stripped text*: escape sequences are parsed and dropped, `\r`
overwrites keep only the final state of the line (progress bars on stderr
are the norm), and trailing blank lines are trimmed. It is compared exactly
by `check`.

## Stability contract

- Two snapshots are equal iff their parsed structures are equal; byte-level
  differences in the *producing* program (SGR ordering, redundant resets,
  repaint strategy) never appear here, because the emulator already
  collapsed them.
- The format never contains machine-specific data: no timestamps, no paths,
  no hostnames — only what the command drew.
- Format changes bump the version marker; v1 files will keep parsing.
