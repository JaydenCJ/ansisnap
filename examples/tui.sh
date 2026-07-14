#!/bin/sh
# A minimal full-screen "TUI": switches to the alternate screen, draws a
# cursor-addressed frame, then exits back to the main screen and prints a
# summary — the pattern every ratatui/bubbletea app follows. The snapshot
# records what is visible after exit: only the summary line.
printf '\033[?1049h\033[2J\033[1;1H+------------+'
printf '\033[2;1H| dashboard  |'
printf '\033[3;1H+------------+'
printf '\033[?1049l'
printf 'session closed: 3 widgets rendered\n'
