#!/bin/bash
# Rehearses the migration block of deploy/Scripts/degauss.sh against every
# upgrade shape a card can arrive in, on a desk, with fake binaries. Run
# from anywhere; it finds the repo from its own location. CI runs it, so a
# change that breaks an upgrade path fails before it ships.
#
# The contract being rehearsed: after any COMPLETED install, one start
# migrates Scripts/.degauss into Scripts/.config/degauss and removes it,
# silently. Pristine shipped files go, user-changed files stay active,
# unknown files follow the move verbatim. The only refusals are a failed
# move and a missing new binary, both of which stop the frontend starting.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SHIM="${HERE}/deploy/Scripts/degauss.sh"
BASE="$(mktemp -d "${TMPDIR:-/tmp}/degauss-rehearsal.XXXXXX")"
trap 'rm -rf "${BASE}"' EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

hash_of() {
    if command -v md5sum >/dev/null 2>&1; then
        md5sum "$1" | cut -c1-32
    else
        md5 -q "$1"
    fi
}

fake_bin() {
    printf '#!/bin/sh\nexit 0\n' > "$1"
    chmod +x "$1"
}

# The new tree exactly as an install lands it: binary, configs, licences,
# logos, themes. Taken from the repository, which is what both the release
# archive and the Downloader database serve.
stage_new() { # $1 = sandbox Scripts dir
    local d="$1/.config/degauss"
    mkdir -p "$d/logos" "$d/themes"
    fake_bin "$d/degauss"
    cp "${HERE}/degauss.toml" "$d/degauss.toml"
    cp "${HERE}/assets/systems.toml" "$d/systems.toml"
    cp "${HERE}/LICENSE" "$d/LICENSE"
    cp "${HERE}"/assets/fonts/*-LICENSE.txt "$d/"
    cp "${HERE}"/assets/logos/*.png "$d/logos/"
    # Guarded the way build-arm.sh guards it: shipped themes arrive in a
    # later change, and this rehearsal must hold on every commit.
    if compgen -G "${HERE}/assets/themes/*.toml" >/dev/null; then
        cp "${HERE}"/assets/themes/*.toml "$d/themes/"
    fi
}

# An old folder as the given release left it, plus what Degauss wrote for
# the user. $2 selects the release shape.
stock_old() { # $1 = sandbox Scripts dir, $2 = v0.1.0 | v0.2.0
    local o="$1/.degauss"
    mkdir -p "$o/cache" "$o/logos"
    echo 'font = "pixel"' > "$o/settings.toml"
    echo 'system = "SNES"' > "$o/state.toml"
    echo bin > "$o/cache/NES.bin"
    echo bin > "$o/cache/index.bin"
    printf '#!/bin/sh\nexit 9\n' > "$o/degauss"
    chmod +x "$o/degauss"
    cp "${HERE}"/assets/logos/*.png "$o/logos/"
    case "$2" in
    v0.1.0)
        cp "${HERE}/tests/fixtures/v0.1.0-degauss.toml" "$o/degauss.toml"
        cp "${HERE}/LICENSE" "$o/LICENSE"
        ;;
    v0.2.0)
        cp "${HERE}/tests/fixtures/v0.2.0-degauss.toml" "$o/degauss.toml"
        cp "${HERE}/LICENSE" "$o/LICENSE"
        cp "${HERE}/assets/fonts/DejaVuSans-LICENSE.txt" "$o/"
        cp "${HERE}/assets/fonts/Px437-LICENSE.txt" "$o/"
        ;;
    esac
    cp "${HERE}/tests/fixtures/v0.1.0-and-v0.2.0-systems.toml" "$o/systems.toml"
}

sandbox() { # $1 = name -> prints the Scripts dir
    local s="${BASE}/$1/Scripts"
    mkdir -p "$s"
    cp "${SHIM}" "$s/degauss.sh"
    echo "$s"
}

run_shim() { # $1 = Scripts dir; stdout+stderr captured separately
    ( cd "$1" && bash ./degauss.sh > "$1/.stdout" 2> "$1/.stderr" )
}

expect_silent_and_gone() { # $1 = Scripts dir, $2 = case name
    [ ! -e "$1/.degauss" ] || fail "$2: .degauss still exists: $(ls -A "$1/.degauss")"
    [ ! -s "$1/.stderr" ] || fail "$2: stderr not empty: $(cat "$1/.stderr")"
    grep -q "Starting Degauss" "$1/.stdout" || fail "$2: frontend did not start"
}

# ---- 0: the table in the shim is exactly the shipped set -----------------
# Recomputed from the committed fixtures and the repository's logos, so the
# shim's closed list can never drift from what the two old releases held.
expected="$( (
    hash_of "${HERE}/tests/fixtures/v0.1.0-degauss.toml"
    hash_of "${HERE}/tests/fixtures/v0.2.0-degauss.toml"
    hash_of "${HERE}/tests/fixtures/v0.1.0-and-v0.2.0-systems.toml"
    for png in "${HERE}"/assets/logos/*.png; do hash_of "$png"; done
) | sort -u )"
while IFS= read -r h; do
    grep -q "^        ${h}) return 0 ;;" "${SHIM}" || fail "table: missing shipped hash ${h}"
done <<< "${expected}"
arms=$(grep -c ") return 0 ;;" "${SHIM}")
want=$(printf '%s\n' "${expected}" | grep -c .)
[ "${arms}" = "${want}" ] || fail "table: ${arms} entries where the shipped set has ${want}"
echo "0 table matches the shipped set (${want} hashes): ok"

# ---- 1: fresh install ----------------------------------------------------
S=$(sandbox fresh); stage_new "$S"
run_shim "$S" || fail "fresh: exit $?"
expect_silent_and_gone "$S" fresh
echo "1 fresh install: ok"

# ---- 2: pristine v0.2.0 upgrade ------------------------------------------
S=$(sandbox pristine020); stage_new "$S"; stock_old "$S" v0.2.0
run_shim "$S" || fail "pristine020: exit $? ($(cat "$S/.stderr"))"
expect_silent_and_gone "$S" pristine020
d="$S/.config/degauss"
[ -e "$d/settings.toml" ] && [ -e "$d/state.toml" ] || fail "pristine020: user files missing"
[ -e "$d/cache/NES.bin" ] || fail "pristine020: cache missing"
cmp -s "$d/degauss.toml" "${HERE}/degauss.toml" || fail "pristine020: new config is not the shipped one"
[ "$(ls "$d/logos" | wc -l)" -eq "$(ls "${HERE}/assets/logos" | wc -l)" ] || fail "pristine020: logo count"
echo "2 pristine v0.2.0 converges silently: ok"

# ---- 3: pristine v0.1.0 upgrade ------------------------------------------
S=$(sandbox pristine010); stage_new "$S"; stock_old "$S" v0.1.0
run_shim "$S" || fail "pristine010: exit $?"
expect_silent_and_gone "$S" pristine010
cmp -s "$S/.config/degauss/degauss.toml" "${HERE}/degauss.toml" || fail "pristine010: new config not active"
echo "3 pristine v0.1.0 converges silently: ok"

# ---- 4: user-edited config stays the active one --------------------------
S=$(sandbox edited); stage_new "$S"; stock_old "$S" v0.2.0
printf '[app]\nfont = "pixel" # mine\n' > "$S/.degauss/degauss.toml"
run_shim "$S" || fail "edited: exit $?"
expect_silent_and_gone "$S" edited
grep -q "# mine" "$S/.config/degauss/degauss.toml" || fail "edited: user's config is not the active one"
echo "4 edited config stays active: ok"

# ---- 5: user's own art stays the active copy ------------------------------
S=$(sandbox art); stage_new "$S"; stock_old "$S" v0.2.0
echo my-art > "$S/.degauss/logos/NES.png"
echo extra-art > "$S/.degauss/logos/My System.png"
mkdir -p "$S/.degauss/themes"
echo 'text = "#33ff33"' > "$S/.degauss/themes/mine.toml"
run_shim "$S" || fail "art: exit $?"
expect_silent_and_gone "$S" art
grep -q my-art "$S/.config/degauss/logos/NES.png" || fail "art: custom logo lost"
grep -q extra-art "$S/.config/degauss/logos/My System.png" || fail "art: extra logo lost"
grep -q 33ff33 "$S/.config/degauss/themes/mine.toml" || fail "art: user theme lost"
echo "5 user art and themes stay active: ok"

# ---- 6: unknown files follow the move, nothing destroyed ------------------
S=$(sandbox unknown); stage_new "$S"; stock_old "$S" v0.2.0
echo backup > "$S/.degauss/frontend.backup.zst"
echo "same" > "$S/.degauss/notes.txt";  echo "same" > "$S/.config/degauss/notes.txt"
echo "old" > "$S/.degauss/clash.txt";   echo "new" > "$S/.config/degauss/clash.txt"
mkdir "$S/.degauss/mystuff"; echo keep > "$S/.degauss/mystuff/thing"
echo spaced > "$S/.degauss/my backup notes.txt"
# A card mounted on a Mac collects these in every folder it opens; one
# inside logos once left the folder undeletable and the launcher refusing
# to start for ever.
echo ds > "$S/.degauss/.DS_Store"
echo ds > "$S/.degauss/logos/.DS_Store"
run_shim "$S" || fail "unknown: exit $?"
expect_silent_and_gone "$S" unknown
d="$S/.config/degauss"
grep -q backup "$d/frontend.backup.zst" || fail "unknown: backup lost"
[ "$(cat "$d/notes.txt")" = "same" ] || fail "unknown: identical file mishandled"
[ "$(cat "$d/clash.txt")" = "new" ] || fail "unknown: collision clobbered the new file"
[ "$(cat "$d/clash.txt.old")" = "old" ] || fail "unknown: colliding file not kept aside"
grep -q keep "$d/mystuff/thing" || fail "unknown: user folder lost"
grep -q spaced "$d/my backup notes.txt" || fail "unknown: name with spaces lost"
[ -e "$d/.DS_Store" ] || fail "unknown: dotted name lost"
[ -e "$d/logos/.DS_Store" ] || fail "unknown: dotted name in logos lost"
echo "6 unknown files preserved through the move: ok"

# ---- 7: a rerun after an interruption converges ---------------------------
S=$(sandbox rerun); stage_new "$S"; stock_old "$S" v0.2.0
# Half-done state: the user files already arrived, some shipped files went.
mv "$S/.degauss/settings.toml" "$S/.config/degauss/settings.toml"
mv "$S/.degauss/state.toml" "$S/.config/degauss/state.toml"
rm "$S/.degauss/LICENSE" "$S/.degauss/degauss.toml"
run_shim "$S" || fail "rerun: exit $?"
expect_silent_and_gone "$S" rerun
run_shim "$S" || fail "rerun2: exit $?"
expect_silent_and_gone "$S" rerun2
echo "7 interruption rerun converges, then no-ops: ok"

# ---- 8: an interrupted install keeps the only binary ----------------------
S=$(sandbox nobinary); stage_new "$S"; stock_old "$S" v0.2.0
rm "$S/.config/degauss/degauss"
run_shim "$S" && fail "nobinary: started without a binary"
[ -e "$S/.degauss/degauss" ] || fail "nobinary: the only binary was deleted"
echo "8 interrupted install keeps the only binary and refuses: ok"

echo "ALL 9 REHEARSALS PASS"
