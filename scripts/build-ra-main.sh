#!/usr/bin/env bash
# Build release-pinned RA Main with the existing Degauss frontend integration.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -ne 1 || -e "$1" ]]; then
    echo "Usage: $0 NEW_OUTPUT_DIRECTORY (must not already exist)" >&2
    exit 1
fi
mkdir -p "$1"
build_root="$(cd "$1" && pwd)"
source_pin=48e32b43ed85b046c7cd41bce756b7bb1679782b
archive_sha=77b2f284a675f4217be27b3fb700c06536e07f3600e14149f2231f2dceb95edb
curl --fail --location --silent --show-error \
    "https://codeload.github.com/odelot/Main_MiSTer/tar.gz/${source_pin}" \
    -o "$build_root/upstream.tar.gz"
actual_sha="$(shasum -a 256 "$build_root/upstream.tar.gz" | awk '{print $1}')"
[[ "$actual_sha" == "$archive_sha" ]] || { echo "RA source checksum mismatch" >&2; exit 1; }
ca_sha=f66dff1bdf8f96060b8177976f8b7d9254bc89bc4db933d769f7384d28480bc9
curl --fail --location --silent --show-error \
    https://curl.se/ca/cacert-2026-08-13.pem \
    -o "$build_root/MiSTer_RA_Degauss.cacert.pem"
actual_ca_sha="$(shasum -a 256 "$build_root/MiSTer_RA_Degauss.cacert.pem" | awk '{print $1}')"
[[ "$actual_ca_sha" == "$ca_sha" ]] || { echo "CA bundle checksum mismatch" >&2; exit 1; }
mkdir "$build_root/source"
tar -xzf "$build_root/upstream.tar.gz" --strip-components=1 -C "$build_root/source"
cd "$build_root/source"
# The pinned release contains its achievement dependency. Never build the
# upstream Makefile's permitted no-rcheevos variant as an RA replacement.
test -s lib/rcheevos/include/rc_client.h
test -s lib/rcheevos/src/rc_client.c
patch --fuzz=0 -p1 < "$here/support/ra-main/frontend.patch"
patch --fuzz=0 -p1 < "$here/support/ra-main/tls.patch"
patch --fuzz=0 -p1 < "$here/support/ra-main/controller.patch"
python3 tests/run-ra-controller-tests.py
bash tests/run-degauss-shortcut-tests.sh
image=degauss-ra-main-build:bullseye
# Rebuild from the checked-in recipe (Docker may reuse matching layers), rather
# than trusting whatever an existing local image tag happens to reference.
docker build -t "$image" "$here/support/ra-main"
python3 tests/run-ra-http-tests.py
docker run --rm --network none -v "$PWD:/src" -w /src \
    -u "$(id -u):$(id -g)" "$image" \
    make BASE=arm-linux-gnueabihf \
    CC="arm-linux-gnueabihf-gcc -mcpu=cortex-a9 -mfpu=neon -mfloat-abi=hard" \
    V=1 2>&1 | tee "$build_root/build.log"
grep -q -- '-DHAS_RCHEEVOS=1' "$build_root/build.log"
test -s bin/lib/rcheevos/src/rc_client.c.o
docker run --rm --network none -v "$PWD:/src:ro" "$image" \
    arm-linux-gnueabihf-nm -C /src/bin/MiSTer.elf > "$build_root/symbols.txt"
for symbol in ' T rc_client_create$' ' T achievements_init\(\)$' \
    ' T degauss_should_take_menu\(' ' T degauss_shortcut_handle_keyboard_event\('; do
    grep -Eq "$symbol" "$build_root/symbols.txt"
done
cp bin/MiSTer "$build_root/MiSTer_RA_Degauss"
shasum -a 256 "$build_root/MiSTer_RA_Degauss"
printf 'RA Main source: %s\nDegauss integration: 0651979d53f570c29f8772e124ae603297830954\n' \
    "$source_pin" > "$build_root/SOURCE-PINS.txt"
printf 'CA bundle: curl Mozilla 2026-08-13 (MPL-2.0), SHA256 %s\n' "$ca_sha" >> "$build_root/SOURCE-PINS.txt"
# Keep corresponding patched source and its build instructions with the binary.
mkdir -p "$build_root/rebuild/scripts" "$build_root/rebuild/support"
cp "$here/scripts/build-ra-main.sh" "$here/scripts/package-ra-main.py" "$build_root/rebuild/scripts/"
cp -R "$here/support/ra-main" "$build_root/rebuild/support/ra-main"
python3 "$here/scripts/package-ra-main.py" "$build_root"
printf 'Built %s\n' "$build_root/MiSTer_RA_Degauss"
