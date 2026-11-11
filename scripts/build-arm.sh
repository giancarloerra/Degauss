#!/bin/bash
# Build Degauss for the MiSTer and assemble the folder that goes on the card.
#
# Rust's linker and self-contained musl produce the static binary. HTTPS's
# ring dependency also needs Clang to compile its ARM cryptography routines.

set -euo pipefail

TARGET="armv7-unknown-linux-musleabihf"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${HERE}"

if [[ -z "${CC_armv7_unknown_linux_musleabihf:-}" ]]; then
    if [[ "$(uname -s)" == Darwin && -x /Library/Developer/CommandLineTools/usr/bin/clang ]]; then
        export CC_armv7_unknown_linux_musleabihf=/Library/Developer/CommandLineTools/usr/bin/clang
    elif command -v clang >/dev/null 2>&1; then
        export CC_armv7_unknown_linux_musleabihf=clang
    else
        echo "Install Clang with ARM support, or set CC_armv7_unknown_linux_musleabihf to a suitable Clang compiler." >&2
        exit 1
    fi
fi

# Apple's ar/ranlib only index Mach-O objects and can silently discard ARM ELF
# members. Use LLVM's cross-target archiver on macOS for ring's static library.
if [[ "$(uname -s)" == Darwin && -z "${AR_armv7_unknown_linux_musleabihf:-}" ]]; then
    degauss_rust_host="$(rustc -vV | sed -n 's/^host: //p')"
    degauss_llvm_ar="$(rustc --print sysroot)/lib/rustlib/${degauss_rust_host}/bin/llvm-ar"
    if [[ -x "${degauss_llvm_ar}" ]]; then
        export AR_armv7_unknown_linux_musleabihf="${degauss_llvm_ar}"
    elif command -v llvm-ar >/dev/null 2>&1; then
        export AR_armv7_unknown_linux_musleabihf=llvm-ar
    else
        echo "Install Rust's LLVM tools with: rustup component add llvm-tools" >&2
        exit 1
    fi
fi

# Official MiSTer packages always contain the application credentials issued
# to Degauss. They remain outside Git and are never copied into deploy/. A
# maintainer can either export both values or point this script at the private
# two-key TOML file.
degauss_developer_id="${DEGAUSS_SCREENSCRAPER_DEVID:-}"
degauss_developer_password="${DEGAUSS_SCREENSCRAPER_DEVPASSWORD:-}"
degauss_credential_file="${DEGAUSS_SCREENSCRAPER_CREDENTIAL_FILE:-${HERE}/screenscraper-developer.toml}"

if [[ -f "${degauss_credential_file}" ]]; then
    if [[ -z "${degauss_developer_id}" ]]; then
        degauss_developer_id="$(sed -nE 's/^[[:space:]]*developer_id[[:space:]]*=[[:space:]]*"([^"]+)"[[:space:]]*$/\1/p' "${degauss_credential_file}" | head -n 1)"
    fi
    if [[ -z "${degauss_developer_password}" ]]; then
        degauss_developer_password="$(sed -nE 's/^[[:space:]]*developer_password[[:space:]]*=[[:space:]]*"([^"]+)"[[:space:]]*$/\1/p' "${degauss_credential_file}" | head -n 1)"
    fi
fi

if [[ -z "${degauss_developer_id}" || -z "${degauss_developer_password}" ]]; then
    echo "Degauss MiSTer packages require both ScreenScraper credential fields." >&2
    exit 1
fi
export DEGAUSS_SCREENSCRAPER_DEVID="${degauss_developer_id}"
export DEGAUSS_SCREENSCRAPER_DEVPASSWORD="${degauss_developer_password}"

if ! rustup target list --installed | grep -q "^${TARGET}$"; then
    echo "installing target ${TARGET}"
    rustup target add "${TARGET}"
fi

# Rust keeps source locations used by panic messages even in a stripped release
# binary. Remap the checkout and toolchain roots so a package never discloses the
# build machine's local paths. CARGO_ENCODED_RUSTFLAGS preserves paths containing
# spaces; RUSTFLAGS, when supplied by the caller, is split the same way Cargo
# documents before being carried into the encoded form.
degauss_cargo_root="$(cd "$(dirname "$(command -v cargo)")/.." && pwd -P)"
degauss_rust_root="$(rustc --print sysroot)"
degauss_target_root="${CARGO_TARGET_DIR:-${HERE}/target}"
if [[ "${degauss_target_root}" != /* ]]; then
    degauss_target_root="${HERE}/${degauss_target_root}"
fi
mkdir -p "${degauss_target_root}"
degauss_target_root="$(cd "${degauss_target_root}" && pwd -P)"
degauss_encoded_rustflags="${CARGO_ENCODED_RUSTFLAGS:-}"
if [[ -z "${degauss_encoded_rustflags}" && -n "${RUSTFLAGS:-}" ]]; then
    read -r -a degauss_existing_rustflags <<< "${RUSTFLAGS}"
    for degauss_rustflag in "${degauss_existing_rustflags[@]}"; do
        if [[ -n "${degauss_encoded_rustflags}" ]]; then
            degauss_encoded_rustflags+=$'\x1f'
        fi
        degauss_encoded_rustflags+="${degauss_rustflag}"
    done
fi
for degauss_rustflag in \
    "-C" \
    "linker-flavor=ld.lld" \
    "--remap-path-prefix=${HERE}=." \
    "--remap-path-prefix=${degauss_target_root}=/target" \
    "--remap-path-prefix=${degauss_cargo_root}=/cargo" \
    "--remap-path-prefix=${degauss_rust_root}=/rust"
do
    if [[ -n "${degauss_encoded_rustflags}" ]]; then
        degauss_encoded_rustflags+=$'\x1f'
    fi
    degauss_encoded_rustflags+="${degauss_rustflag}"
done
export CARGO_ENCODED_RUSTFLAGS="${degauss_encoded_rustflags}"
unset RUSTFLAGS

echo "building"
cargo build --release --target "${TARGET}"

degauss_binary="${degauss_target_root}/${TARGET}/release/degauss"
if ! LC_ALL=C grep -aFq -f <(printf '%s\n' "${degauss_developer_id}") "${degauss_binary}" \
    || ! LC_ALL=C grep -aFq -f <(printf '%s\n' "${degauss_developer_password}") "${degauss_binary}"; then
    echo "The release binary did not retain both ScreenScraper credential fields." >&2
    exit 1
fi
if LC_ALL=C grep -aFq "${HERE}" "${degauss_binary}" \
    || LC_ALL=C grep -aFq "${degauss_target_root}" "${degauss_binary}" \
    || LC_ALL=C grep -aFq "${degauss_cargo_root}" "${degauss_binary}" \
    || LC_ALL=C grep -aFq "${degauss_rust_root}" "${degauss_binary}"; then
    echo "The release binary retained a private build-machine path." >&2
    exit 1
fi

# The staged asset folders are rebuilt from scratch: a plain copy over
# last time's staging keeps files the repository no longer ships, and a
# stale theme or logo would ride every local deploy from then on.
rm -rf deploy/Scripts/.config/degauss/logos deploy/Scripts/.config/degauss/themes deploy/Scripts/.config/degauss/masks
mkdir -p deploy/Scripts/.config/degauss/logos
cp "${degauss_binary}" deploy/Scripts/.config/degauss/degauss
cp degauss.toml deploy/Scripts/.config/degauss/degauss.toml
cp assets/systems.toml deploy/Scripts/.config/degauss/systems.toml
# A glob that matches nothing is passed through literally, and the copy then
# fails the whole script under set -e. Guarded so an empty folder is simply
# an empty folder.
if compgen -G "assets/logos/*.png" >/dev/null; then
    cp assets/logos/*.png deploy/Scripts/.config/degauss/logos/
fi
# The shipped themes, staged beside the configuration where Degauss reads
# them from.
mkdir -p deploy/Scripts/.config/degauss/themes
cp assets/themes/*.toml deploy/Scripts/.config/degauss/themes/
# Native MiSTer shadow-mask files, including supplied presets. A copy over
# an installed card leaves user-added mask files in that folder intact.
mkdir -p deploy/Scripts/.config/degauss/masks
cp assets/masks/*.txt deploy/Scripts/.config/degauss/masks/
# The licence travels with the program, so a copy on a card is never a copy
# with no terms attached. The typefaces are baked into the binary and carry
# their own terms, so those travel with it too.
cp LICENSE deploy/Scripts/.config/degauss/LICENSE
cp assets/licenses/MessagePack-MIT.txt deploy/Scripts/.config/degauss/
cp assets/fonts/DejaVuSans-LICENSE.txt assets/fonts/Px437-LICENSE.txt \
   assets/fonts/RobotoCondensed-LICENSE.txt assets/fonts/Tamzen-LICENSE.txt \
   deploy/Scripts/.config/degauss/

unset degauss_developer_id degauss_developer_password
unset DEGAUSS_SCREENSCRAPER_DEVID DEGAUSS_SCREENSCRAPER_DEVPASSWORD
unset degauss_cargo_root degauss_rust_root degauss_target_root degauss_encoded_rustflags
unset degauss_existing_rustflags degauss_rustflag CARGO_ENCODED_RUSTFLAGS

echo
echo "deploy/ is ready. Copy its contents onto the card:"
echo "  deploy/Scripts/          ->  /media/fat/Scripts/"
