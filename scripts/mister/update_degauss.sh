#!/bin/bash
# Degauss installer / updater for MiSTer.
#
# Drop this file into /media/fat/Scripts and run "update_degauss" from
# the MiSTer Scripts menu. It installs or updates the Degauss frontend
# over the stock Zaparoo Frontend, safely:
#
#   - refuses to install a release whose minimum Zaparoo Core version
#     is newer than the Core installed on this MiSTer (and, when the
#     release declares one, a maximum it has not been tested past),
#   - backs up the stock frontend ONCE as frontend.zaparoo-original,
#     and the previously installed Degauss as frontend.degauss-prev on
#     every update,
#   - verifies the download's checksum before touching anything,
#   - swaps binaries with a staged move, which is safe while the
#     frontend is running,
#   - restores the stock frontend with:  update_degauss.sh -restore
#
# If a Zaparoo update ever reinstalls the stock frontend, simply run
# this script again.
#
# Release asset contract (produced by the Degauss release workflow):
#   frontend              the MiSTer ARM32 binary
#   frontend.md5          its md5 checksum ("<md5>  frontend")
#   min_core_version.txt  minimum compatible Zaparoo Core version;
#                         optional second line: maximum tested version

REPO="giancarloerra/degauss"
FRONTEND="/media/fat/zaparoo/frontend"
ORIGINAL_BACKUP="/media/fat/zaparoo/frontend.zaparoo-original"
PREV_BACKUP="/media/fat/zaparoo/frontend.degauss-prev"
CORE_SCRIPT="/media/fat/Scripts/zaparoo.sh"
API_LATEST="https://api.github.com/repos/${REPO}/releases/latest"
STAGE="${FRONTEND}.new"

die() {
    echo ""
    echo "ERROR: $1"
    exit 1
}

restore_original() {
    [ -f "${ORIGINAL_BACKUP}" ] || die "no original backup at ${ORIGINAL_BACKUP} - nothing to restore."
    cp "${ORIGINAL_BACKUP}" "${STAGE}" || die "staging the original failed."
    mv -f "${STAGE}" "${FRONTEND}" || die "restoring the original failed."
    echo "Stock frontend restored from ${ORIGINAL_BACKUP}."
    echo "Reboot to run it."
    exit 0
}

# Compare two dotted versions. Echoes -1, 0, or 1 for a<b, a=b, a>b.
vercmp() {
    local a b i x y
    IFS='.' read -r -a a <<< "$1"
    IFS='.' read -r -a b <<< "$2"
    for i in 0 1 2; do
        x="${a[$i]:-0}"
        y="${b[$i]:-0}"
        if [ "$x" -lt "$y" ] 2> /dev/null; then echo -1; return; fi
        if [ "$x" -gt "$y" ] 2> /dev/null; then echo 1; return; fi
    done
    echo 0
}

[ "$1" = "-restore" ] && restore_original

echo "Degauss installer"
echo "================="

[ -d /media/fat/zaparoo ] || die "no /media/fat/zaparoo - install Zaparoo Core first (zaparoo.org)."

# ── Core compatibility gate ─────────────────────────────────────────
core_version=""
if [ -x "${CORE_SCRIPT}" ]; then
    core_version="$("${CORE_SCRIPT}" -version 2> /dev/null | sed -n 's/^Zaparoo v\([0-9][0-9.]*\).*/\1/p')"
fi
if [ -z "${core_version}" ]; then
    echo "Could not detect the installed Zaparoo Core version."
    echo "Degauss checks compatibility again at startup, but installing"
    echo "blind is your call."
    read -r -p "Continue anyway? (y/N) " reply
    [ "${reply}" = "y" ] || [ "${reply}" = "Y" ] || exit 1
else
    echo "Installed Zaparoo Core: v${core_version}"
fi

echo "Fetching latest Degauss release..."
release_json="$(curl -fsSL "${API_LATEST}")" || die "could not reach GitHub (network down?)."
tag="$(echo "${release_json}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
assets="$(echo "${release_json}" | grep -o '"browser_download_url": *"[^"]*"' | sed 's/.*"\(https[^"]*\)"/\1/')"
bin_url="$(echo "${assets}" | grep '/frontend$' | head -1)"
md5_url="$(echo "${assets}" | grep '/frontend\.md5$' | head -1)"
core_req_url="$(echo "${assets}" | grep '/min_core_version\.txt$' | head -1)"
[ -n "${bin_url}" ] || die "release ${tag} carries no frontend binary."
[ -n "${md5_url}" ] || die "release ${tag} carries no checksum."
echo "Latest release: ${tag}"

if [ -n "${core_req_url}" ] && [ -n "${core_version}" ]; then
    core_req="$(curl -fsSL "${core_req_url}")" || die "could not download the compatibility file."
    min_core="$(echo "${core_req}" | sed -n 1p | tr -d '[:space:]')"
    max_core="$(echo "${core_req}" | sed -n 2p | tr -d '[:space:]')"
    if [ -n "${min_core}" ] && [ "$(vercmp "${core_version}" "${min_core}")" = "-1" ]; then
        die "this Degauss release needs Zaparoo Core v${min_core} or newer (installed: v${core_version}). Update Zaparoo Core first, then run this script again."
    fi
    if [ -n "${max_core}" ] && [ "$(vercmp "${core_version}" "${max_core}")" = "1" ]; then
        die "installed Zaparoo Core v${core_version} is newer than this Degauss release was tested with (v${max_core}). Wait for the next Degauss release."
    fi
    echo "Core compatibility: ok (needs >= v${min_core}${max_core:+, tested <= v${max_core}})"
fi

# ── Download and verify ─────────────────────────────────────────────
echo "Downloading..."
curl -fL --progress-bar -o "${STAGE}" "${bin_url}" || die "download failed."
want_md5="$(curl -fsSL "${md5_url}" | awk '{print $1}')"
have_md5="$(md5sum "${STAGE}" | awk '{print $1}')"
if [ -z "${want_md5}" ] || [ "${want_md5}" != "${have_md5}" ]; then
    rm -f "${STAGE}"
    die "checksum mismatch - download corrupted, nothing was changed."
fi
chmod +x "${STAGE}"

# ── Backups, then the staged swap ───────────────────────────────────
if [ -f "${FRONTEND}" ]; then
    if [ ! -f "${ORIGINAL_BACKUP}" ]; then
        cp "${FRONTEND}" "${ORIGINAL_BACKUP}" || die "could not back up the current frontend."
        echo "Current frontend backed up as ${ORIGINAL_BACKUP}."
    else
        cp "${FRONTEND}" "${PREV_BACKUP}" || die "could not back up the current frontend."
        echo "Previous frontend backed up as ${PREV_BACKUP}."
    fi
fi
mv -f "${STAGE}" "${FRONTEND}" || die "installing the new binary failed."

echo ""
echo "Degauss ${tag} installed."
read -r -p "Reboot now to run it? (y/N) " reply
if [ "${reply}" = "y" ] || [ "${reply}" = "Y" ]; then
    reboot
fi
echo "Reboot later to run it."
