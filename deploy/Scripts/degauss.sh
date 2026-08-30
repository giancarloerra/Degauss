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

# Degauss used to live in Scripts/.degauss; only v0.1.0 and v0.2.0 ever
# installed there, and every later version installs to Scripts/.config so
# it sits where the rest of the ecosystem keeps such folders. The files
# Degauss writes for the user were never in the Downloader database, so no
# updater carries them over. This block does, file by file, and removes the
# old folder once it is empty. It is permanent rather than for a transition
# period, because a card can upgrade straight from v0.1.0 or v0.2.0 at any
# future date and this launcher is what meets it.
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

    # The console is what is on screen while this runs, and a blank pause
    # with no words looks like a machine that stopped.
    echo "Migrating Degauss data to Scripts/.config/degauss..."

    # md5sum on the MiSTer's busybox; md5 where this is rehearsed on a desk.
    hash_of() {
        if command -v md5sum >/dev/null 2>&1; then
            md5sum "$1" | cut -c1-32
        else
            md5 -q "$1"
        fi
    }

    # Every byte v0.1.0 or v0.2.0 ever shipped into Scripts/.degauss, by
    # md5: each version's degauss.toml, their shared systems.toml, and the
    # logos, identical in both releases and deduplicated where two systems
    # share one image. A CLOSED list, never appended to: those two
    # releases are the only ones that ever installed to the old folder, so
    # a hash in here is certainly a pristine shipped copy and a miss is
    # certainly something the user changed or added. The rehearsal in
    # scripts/rehearse-migration.sh fails if this table disagrees with the
    # committed copies of what those releases shipped.
    shipped() {
        case "$1" in
        0b0a76c7cc2190a1a7b28beea7083d92) return 0 ;;
        0fd9f2c9d0ac607fc30b2ace3a8cbc2b) return 0 ;;
        13d248dadf4ab4d8ca197092b529dd2b) return 0 ;;
        14b7aa08041d3df0096b4bf2d58319e7) return 0 ;;
        156f646779e2de23af86fe364f8a6045) return 0 ;;
        1a5b5b09c44a9723931477db165f8f48) return 0 ;;
        1cf697b3adbfb5762fedea45d4fe845e) return 0 ;;
        1daacf1ae98648e5829475fbad86766a) return 0 ;;
        1ec19d5d256ea8e02fd717ce5e9cd625) return 0 ;;
        2567c02493b5c1413c53395c0fa8ee80) return 0 ;;
        293f70170055fef8d809b08fab89f59b) return 0 ;;
        2ca30ad4009d344a2220ca76237594bc) return 0 ;;
        2ccfa18d54b44f2cdbe06d86f24b28ce) return 0 ;;
        2f1bbfb7edffb447b1b4317073866220) return 0 ;;
        302b3d0ce25196193416717e97366cee) return 0 ;;
        332fca290928906d611c7f01eda81b05) return 0 ;;
        3be1ed16e98127f2dc9cb43a0de65e4b) return 0 ;;
        3f2d0171b404341502ff4277c2a8f88a) return 0 ;;
        42a787d38abd11393fb2617795d55552) return 0 ;;
        4471d30e3a7ff74f9fdad7703ecb2d28) return 0 ;;
        46d17dbe17d63626c48adb672ca8e817) return 0 ;;
        46d1f8cb28cb7827077529bc91e40fea) return 0 ;;
        47d67e47fadc7739c2b516a949864c8c) return 0 ;;
        47f26ffcd8c01ec953abbafcfe2d1c2d) return 0 ;;
        4cedfb2e213d40a42b23462d3aeb6b05) return 0 ;;
        4d956c339bc6f296df8f52599984a7db) return 0 ;;
        55485c06974d66168f2fb1cc176436e0) return 0 ;;
        55a698240639e1fb69d3d9c110cd8700) return 0 ;;
        5c62ee45b08fb5879c2c13c072e5dbee) return 0 ;;
        5cc6090675d8516460b4eaae2d640129) return 0 ;;
        61c6e1088568add2e4cc1d96cc4ac533) return 0 ;;
        69e537e0ea5835419af73b9294fe88fb) return 0 ;;
        6a0e38f8a459e713c252c0df5c212e76) return 0 ;;
        6e448fd04c586ec5066628d61bec51e6) return 0 ;;
        714a385faf3ccdb57811f7606012fa0b) return 0 ;;
        71ddef1d7257dc4b48033dbe2cfd1943) return 0 ;;
        726c445acff52e1d1eba774e52ad8867) return 0 ;;
        75f30f25472f340fbd7c99c4eef495d4) return 0 ;;
        771814d1c26a47b8bea6eba18b2a0142) return 0 ;;
        77230bc6675d1785523d1c153bc1f633) return 0 ;;
        7b4d7ea4446ae71273773265e0dd851a) return 0 ;;
        7b563210a7f4991b65b1deeea75236dd) return 0 ;;
        7dfee53b8f45826d9caf614ed079cc99) return 0 ;;
        8049ac518ead0a76da7a0e374912e787) return 0 ;;
        83beaa52a20480ffd624b66fa43819e3) return 0 ;;
        83e7932ae1b8bbc40bc20c5b45ec43f0) return 0 ;;
        849739bb314e6c9b08c8d99df01151ad) return 0 ;;
        885429bd450640fc44d49eeef7bede58) return 0 ;;
        8a4290e8898b0fd06c18a4fc61571afa) return 0 ;;
        9004e7073c0ea9e721b0b3787fbba51b) return 0 ;;
        9373ce70089b2939014645a62d5b0b76) return 0 ;;
        961efa6cd36621739581c43083ce7b3e) return 0 ;;
        9889443774feff62a03126332ee36a59) return 0 ;;
        9d54be57819b633be402e83621829481) return 0 ;;
        9f29d036ec324114f831d50f4b221db0) return 0 ;;
        9f84d9332d76940c8a5542b0461d6fda) return 0 ;;
        a264c410d782a370a5450f142b6ec99a) return 0 ;;
        a55786f97e3ca773139bd5cf2478ff12) return 0 ;;
        a584c02c98a0f9f16255f0a3279c1745) return 0 ;;
        a5d47ddebcf2902152e6a5619fdd7aa2) return 0 ;;
        aa8407df7c8becdbd00d84486e9679e3) return 0 ;;
        ab343fb4ac762033a0090e5a3aad0235) return 0 ;;
        b2d4c217cb94d9be0596458805788cf9) return 0 ;;
        b2ed1f6fd37fb2d98bd71fed8c495610) return 0 ;;
        bbd19b4083e9185c2029193d78b3e90d) return 0 ;;
        be0dfdefa380cc084abca662a5bf5f91) return 0 ;;
        c4d199b4e8acbcb0d84b139b1c60efad) return 0 ;;
        c562eec35c93471c55e339718ee15e7e) return 0 ;;
        c87ad1eeb20e748999a039e9f654db0a) return 0 ;;
        cb81da04e79c1703e75934598602a19c) return 0 ;;
        ceedbc2fbbe212d8256868657652a96a) return 0 ;;
        cf63f5352871377cd3d42a4f2ea9c988) return 0 ;;
        d157725dfbbac6b24db2c5a7e66d1e28) return 0 ;;
        d206c7e85fab2e850436c71e17923dfe) return 0 ;;
        d2b0098f38de0f38b2e88cf9590d2e3d) return 0 ;;
        d320a8df744e8f17148805f1381332f8) return 0 ;;
        d58ea9ce06d6e62e93b8dfe4a4b5987f) return 0 ;;
        da9cf47a95cc4c66baee9d5b30f87f30) return 0 ;;
        dbc4d370cde46ba9e758cef0a4d0c3df) return 0 ;;
        dffe197c31f58a798cd1c4368e602c55) return 0 ;;
        e0329c1f7a48b058cd68b392d6b752d0) return 0 ;;
        e63dd47f8d5482873723fec433189846) return 0 ;;
        e919dab52c0896985cec382559369051) return 0 ;;
        f1321727db532ecd94552545b69d62e7) return 0 ;;
        esac
        return 1
    }

    # A file only the user's Degauss ever writes cannot collide with the
    # new install, which never ships it: moved, unless a rerun after an
    # interruption finds it already arrived.
    carry() {
        if [ -e "${OLD}/$1" ] && [ ! -e "${DIR}/$1" ]; then
            mv "${OLD}/$1" "${DIR}/$1" || migrate_failed "$1"
        fi
    }
    carry settings.toml
    carry state.toml
    carry cache

    # The two editable configs: a pristine shipped copy carries nothing
    # and goes; anything else is the user's edited file, and it stays the
    # ACTIVE one, moved over the copy the new version shipped. Every key
    # the old file lacks has a default in the binary, and keys are never
    # renamed or removed (pinned by the compat fixtures in tests/), so an
    # old edited config keeps working in every future version.
    for cfg in degauss.toml systems.toml; do
        if [ -f "${OLD}/${cfg}" ]; then
            if shipped "$(hash_of "${OLD}/${cfg}")"; then
                rm "${OLD}/${cfg}" || migrate_failed "${cfg}"
            else
                mv -f "${OLD}/${cfg}" "${DIR}/${cfg}" || migrate_failed "${cfg}"
            fi
        fi
    done

    # The licence texts are nobody's edits and the current copies are
    # already at the new path: the old ones carry nothing.
    for lic in LICENSE DejaVuSans-LICENSE.txt Px437-LICENSE.txt \
        RobotoCondensed-LICENSE.txt Tamzen-LICENSE.txt; do
        if [ -f "${OLD}/${lic}" ]; then
            rm "${OLD}/${lic}" || migrate_failed "${lic}"
        fi
    done

    # Logos: a pristine shipped logo goes, and the user's own art, shipped
    # name or not, stays the active copy at the new path. Themes were
    # never shipped to the old folder, so everything there is the user's.
    for sub in logos themes; do
        if [ -d "${OLD}/${sub}" ]; then
            mkdir -p "${DIR}/${sub}" || migrate_failed "${sub}"
            # Dotted names included: a card mounted on a Mac grows a
            # .DS_Store in every folder it opens, and a name the globs
            # missed would leave the folder undeletable for ever.
            for entry in "${OLD}/${sub}"/* "${OLD}/${sub}"/.[!.]* "${OLD}/${sub}"/..?*; do
                [ -e "${entry}" ] || continue
                name="${sub}/$(basename "${entry}")"
                if [ -f "${entry}" ] && shipped "$(hash_of "${entry}")"; then
                    rm "${entry}" || migrate_failed "${name}"
                elif [ ! -e "${DIR}/${name}" ]; then
                    mv "${entry}" "${DIR}/${name}" || migrate_failed "${name}"
                elif [ -f "${entry}" ] && [ -f "${DIR}/${name}" ]; then
                    mv -f "${entry}" "${DIR}/${name}" || migrate_failed "${name}"
                else
                    aside="${name}.old"
                    n=2
                    while [ -e "${DIR}/${aside}" ]; do
                        aside="${name}.old.${n}"
                        n=$((n + 1))
                    done
                    mv "${entry}" "${DIR}/${aside}" || migrate_failed "${name}"
                fi
            done
            # Empty in every real case now. If something landed in it
            # while this ran, the sweep below carries the whole folder
            # aside verbatim rather than refusing to start.
            rmdir "${OLD}/${sub}" 2>/dev/null
        fi
    done

    # The superseded binary is the one thing here that is nobody's data:
    # it is the program this move replaces. It goes only once its
    # replacement is confirmed present and runnable at the new path, so an
    # interrupted install can never delete the only binary a card has.
    if [ -e "${OLD}/degauss" ] && [ -x "${DIR}/degauss" ]; then
        rm "${OLD}/degauss" || migrate_failed "degauss"
    fi

    # Whatever remains is not Degauss's: backups, notes, another tool's
    # files. Their content is unknown but the safe disposition is not:
    # they follow the move verbatim, never deleted unless byte-identical
    # to a copy already at the new path, renamed aside on a collision.
    for entry in "${OLD}"/* "${OLD}"/.[!.]* "${OLD}"/..?*; do
        [ -e "${entry}" ] || continue
        name="$(basename "${entry}")"
        if [ "${name}" = "degauss" ]; then
            # Still here means the rule above kept it: there is no runnable
            # replacement, and promoting the superseded binary to the new
            # path would dress an interrupted install up as a finished one.
            continue
        fi
        if [ ! -e "${DIR}/${name}" ]; then
            mv "${entry}" "${DIR}/${name}" || migrate_failed "${name}"
        elif [ -f "${entry}" ] && [ -f "${DIR}/${name}" ] && cmp -s "${entry}" "${DIR}/${name}"; then
            rm "${entry}" || migrate_failed "${name}"
        else
            aside="${name}.old"
            n=2
            while [ -e "${DIR}/${aside}" ]; do
                aside="${name}.old.${n}"
                n=$((n + 1))
            done
            mv "${entry}" "${DIR}/${aside}" || migrate_failed "${name}"
        fi
    done

    # Empty now in every completed install; if something appeared in the
    # folder while this ran, the next start picks it up.
    if ! rmdir "${OLD}" 2>/dev/null; then
        echo "Degauss: ${OLD} not empty, migrating the rest on the next start." >&2
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
