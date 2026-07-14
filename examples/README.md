# Examples

Two tiny scripts that produce exactly the kind of output byte-level
snapshot tools struggle with. Run everything from this directory with a
built `ansisnap` on your `PATH` (or use `cargo run --`).

## greet.sh — progress bars and colors

```bash
ansisnap record greet -- sh greet.sh
cat .ansisnap/greet.snap     # two clean rows; the progress churn is gone
ansisnap check               # ok      greet
```

Now change `14 checks` to `13 checks` in `greet.sh` and run
`ansisnap check` again: it fails with a row-level diff and a caret under
the changed column. `ansisnap check --update` re-blesses.

## tui.sh — alternate-screen apps

```bash
ansisnap record tui -- sh tui.sh
cat .ansisnap/tui.snap       # only "session closed: ..." — the alt-screen
                             # frame was left behind on exit, like a real
                             # terminal
```

## Rendering arbitrary captures

`render` works on any captured byte stream, no snapshot needed:

```bash
sh greet.sh | ansisnap render --styles
```

Clean up with `rm -rf .ansisnap` when you are done experimenting.
