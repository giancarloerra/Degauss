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
# with it in Scripts/.config/degauss, in degauss.toml and settings.toml.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OLD="${HERE}/.degauss"
DIR="${HERE}/.config/degauss"

# Degauss used to live in Scripts/.degauss. It moved to Scripts/.config so
# it sits where the rest of the ecosystem keeps such folders, but the files
# Degauss writes for the user were never in the Downloader database, so no
# updater carries them over. This block does, file by file. It is permanent
# rather than for a transition period, because a card can upgrade straight
# from v0.1.0 or v0.2.0 at any future date and this launcher is what meets
# it.
if [ -d "${OLD}" ]; then
    # A migration that fails half way must not start the frontend: it would
    # come up with some of the user's files still in the old folder and
    # quietly regenerate the rest as defaults. Name what failed and stop.
    migrate_failed() {
        echo "Degauss: moving ${OLD}/$1 into ${DIR} failed; not starting." >&2
        exit 1
    }

    if ! mkdir -p "${DIR}"; then
        echo "Degauss: could not create ${DIR}; not starting." >&2
        exit 1
    fi

    # Moved only when the new folder does not have the file yet, so a rerun
    # after an interruption never overwrites what already arrived.
    carry() {
        if [ -e "${OLD}/$1" ] && [ ! -e "${DIR}/$1" ]; then
            mv "${OLD}/$1" "${DIR}/$1" || migrate_failed "$1"
        fi
    }

    # What Degauss wrote for the user: options, resume state and the index.
    carry settings.toml
    carry state.toml
    carry cache

    # Folders the user drops their own files into, merged one file at a
    # time: images already installed at the new path win, images only the
    # user has follow them over.
    for sub in logos themes; do
        if [ -d "${OLD}/${sub}" ]; then
            mkdir -p "${DIR}/${sub}" || migrate_failed "${sub}"
            for entry in "${OLD}/${sub}"/*; do
                [ -e "${entry}" ] || continue
                carry "${sub}/$(basename "${entry}")"
            done
            # Gone once everything in it moved; kept if anything stayed,
            # and the report below names it.
            rmdir "${OLD}/${sub}" 2>/dev/null
        fi
    done

    # The two shipped configuration files are documented as user editable,
    # so an edited copy is user data. An old copy identical to the new one
    # carries nothing and goes; a differing copy is left where it is rather
    # than guessed about, and the report below names it.
    for cfg in degauss.toml systems.toml; do
        if [ -e "${OLD}/${cfg}" ]; then
            if [ ! -e "${DIR}/${cfg}" ]; then
                mv "${OLD}/${cfg}" "${DIR}/${cfg}" || migrate_failed "${cfg}"
            elif cmp -s "${OLD}/${cfg}" "${DIR}/${cfg}"; then
                rm "${OLD}/${cfg}" || migrate_failed "${cfg}"
            fi
        fi
    done

    # The old folder goes only once it is empty. Anything still in it is
    # either a file this launcher does not own (the old binary, the licence
    # texts) or one it refused to move over an existing copy, and deleting
    # a folder that still holds something is how user data gets lost.
    if ! rmdir "${OLD}" 2>/dev/null; then
        echo "Degauss: kept ${OLD}, whose remaining files were not moved because they already exist in ${DIR} or are not user files: $(ls -A "${OLD}" | tr '\n' ' ')"
    fi
fi

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
