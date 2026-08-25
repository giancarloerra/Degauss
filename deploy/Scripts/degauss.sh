#!/bin/bash
# Degauss: a fast frontend for MiSTer FPGA.
#
# Appears in the MiSTer Scripts menu.
#
#   up / down      move
#   left / right   scroll speed, 0.5x to 12x
#   enter          open a system, or launch a game
#   escape / back  go back
#   tab            this folder: random, favourites, letter, search, view
#   space          menu: options, help, about, exit
#
# A gamepad needs no setup: while Degauss owns the screen, MiSTer sends the
# d-pad as arrows and the face buttons as Enter, Escape, Space and Tab.
#
# It installs nothing and changes no MiSTer settings. Its own settings live
# beside it in degauss.toml and settings.toml.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIR="${HERE}/.degauss"

BIN="${DIR}/degauss"

if [ ! -x "${BIN}" ]; then
    echo "Degauss binary missing or not executable: ${BIN}" >&2
    exit 1
fi

# The console is visible for the moment before the frontend takes the
# screen, and a bare cursor on a black screen looks like a machine that has
# stopped. One line is enough to say what is happening.
echo "Starting Degauss frontend..."

# Deliberately no `set -e` around this: Degauss's own error message and exit
# code are the useful output, and aborting here would hide them.
"${BIN}" --config "${DIR}/degauss.toml" --systems "${DIR}/systems.toml" "$@"
status=$?

if [ "${status}" != "0" ]; then
    echo
    echo "Degauss exited with status ${status}"
fi

exit "${status}"
