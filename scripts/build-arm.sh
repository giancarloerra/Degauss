#!/bin/bash
# Build Degauss for the MiSTer and assemble the folder that goes on the card.
#
# No Docker and no C cross-compiler. The dependency tree is pure Rust, so
# rustup's own linker and its self-contained musl are enough; the linker is
# selected in .cargo/config.toml.

set -euo pipefail

TARGET="armv7-unknown-linux-musleabihf"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${HERE}"

if ! rustup target list --installed | grep -q "^${TARGET}$"; then
    echo "installing target ${TARGET}"
    rustup target add "${TARGET}"
fi

echo "building"
cargo build --release --target "${TARGET}"

mkdir -p deploy/Scripts/.degauss/logos
cp "target/${TARGET}/release/degauss" deploy/Scripts/.degauss/degauss
cp degauss.toml deploy/Scripts/.degauss/degauss.toml
cp assets/systems.toml deploy/Scripts/.degauss/systems.toml
# A glob that matches nothing is passed through literally, and the copy then
# fails the whole script under set -e. Guarded so an empty folder is simply
# an empty folder.
if compgen -G "assets/logos/*.png" >/dev/null; then
    cp assets/logos/*.png deploy/Scripts/.degauss/logos/
fi
# The licence travels with the program, so a copy on a card is never a copy
# with no terms attached. The typefaces are baked into the binary and carry
# their own terms, so those travel with it too.
cp LICENSE deploy/Scripts/.degauss/LICENSE
cp assets/fonts/DejaVuSans-LICENSE.txt assets/fonts/Px437-LICENSE.txt \
   assets/fonts/RobotoCondensed-LICENSE.txt assets/fonts/Tamzen-LICENSE.txt \
   deploy/Scripts/.degauss/

echo
echo "deploy/ is ready. Copy its contents onto the card:"
echo "  deploy/Scripts/          ->  /media/fat/Scripts/"
