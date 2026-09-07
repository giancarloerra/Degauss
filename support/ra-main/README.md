# RA Main frontend integration

`../../scripts/build-ra-main.sh NEW_OUTPUT_DIRECTORY` builds RetroAchievements
Main with the existing Degauss Frontend menu and keyboard shortcut. The output
directory must not exist. The helper installs nothing on a device. The release
workflow builds it and includes the binary, CA bundle, and source notice in the
normal archive and Downloader database. Corresponding source is attached to the
same release as `MiSTer_RA_Degauss-source.tar.gz`.

## Installation and updates

The release installs these separate files:

```text
degauss/MiSTer_RA_Degauss
degauss/MiSTer_RA_Degauss.cacert.pem
degauss/MiSTer_RA_Degauss.SOURCE.txt
```

After installing the RA cores with their compatible RA setup, select this Main
in the existing `[RA_*]` section of `MiSTer.ini`:

```ini
[RA_*]
main=degauss/MiSTer_RA_Degauss
```

Change only that section's `main=` value, preserving its position and other
settings. If it is absent, add it before any more-specific RA sections. Matching
sections apply in file order: preserve deliberate per-core Main overrides and
update an affected override only when that core should use this integration.
The package and Downloader never rewrite INI files or credentials.

The [upstream RA installer](https://github.com/sage2050/MiSTer_RetroAchievements/blob/bc19dcab7c855dc55bf61e4025a2591cd9a9a5f5/Scripts/MiSTer_RA.sh#L371)
updates its own root `MiSTer_RA` and leaves an existing `[RA_*]` block untouched. The separate Degauss executable therefore survives that
update. Degauss's normal Downloader database updates the integrated binary and
its CA bundle. No executable or updater is renamed or impersonated.

This preserves files from upstream updates; it does not promise that an older
RA Main understands future core protocols. Degauss releases deliberately pin,
build and test the RA backend. When upstream cores require a newer Main, update
the pinned backend and validate it before shipping another Degauss release.

## Sources and compatibility

- RA Main v1.12.2: commit `48e32b43ed85b046c7cd41bce756b7bb1679782b`
  from https://github.com/odelot/Main_MiSTer/tree/48e32b43ed85b046c7cd41bce756b7bb1679782b.
- Source archive SHA-256:
  `77b2f284a675f4217be27b3fb700c06536e07f3600e14149f2231f2dceb95edb`.
- Degauss integration: commit `0651979d53f570c29f8772e124ae603297830954`
  relative to `0a8fb44`, from https://github.com/giancarloerra/Degauss-Main.
  The patch omits its unrelated input.cpp blank-line removal.
- rcheevos 12.4.0 is already bundled in this exact RA source. Its bytes remain
  unchanged; the build fails if the dependency or its compiled object is missing.
- Mozilla CA bundle from https://curl.se/ca/cacert-2026-08-13.pem (MPL-2.0),
  SHA-256 `f66dff1bdf8f96060b8177976f8b7d9254bc89bc4db933d769f7384d28480bc9`.
  Its published checksum is available at the same URL with `.sha256` appended.

The patch adds the same helper implementation, input hook, menu actions, and
host tests used by Degauss Main. It preserves RA achievement code and its build
configuration. The `controller.patch` sends both joystick words from the first update and
sends initial controller states even before any button is pressed. This clears
FPGA input bits retained across a Main restart, including the RA NES input-disable
bit, while preserving player swaps and suppressing unchanged polling traffic.
The existing protocol already sends both words after any high-bit button input.
A separate `tls.patch` changes only the active HTTP transport and
adds a real-curl host test. It removes certificate-verification bypass, requires
HTTPS even for redirects, ignores curl startup configuration, and reports
transport errors without URLs or credential contents. Curl is started directly;
request values pass through an anonymous stdin connection, never shell/process
arguments or request files. This uses curl 7.43 or newer (`data-raw`). The existing 16-byte version 1 shortcut configuration at
`config/degauss/frontend_shortcut.bin` is shared unchanged. There is no second
shortcut setting. Frontend and the configured keyboard key load `menu.rbf`;
normal Main profile selection determines the executable used after that load.
When a frontend exits from RA Main Scripts mode, cleanup explicitly returns to
VT1 before disabling its script framebuffer.

Unstable cores using Degauss Main already use that integration. This build is
needed for an RA Main executable selected through an RA profile. It does not
modify any FPGA core.

## Build and validation

Requires Bash, curl, tar, patch, shasum, Python 3, OpenSSL, a host C++ compiler,
and Docker. Host TLS tests bind a temporary loopback server. The build
builds `degauss-ra-main-build:bookworm-gcc10` from the included Dockerfile,
allowing Docker to reuse matching layers but never trusting an arbitrary
existing tag. Debian Bookworm provides the maintained host libraries and tools;
low-priority Bullseye packages supply GCC 10 and the ARM glibc 2.31 sysroot for
MiSTer compatibility. It compiles for Cortex-A9 with NEON and the ARM hard-float
ABI, then rejects GLIBC requirements above 2.31 or GLIBCXX requirements above
3.4.28 before packaging. Container compilation has networking disabled. The
source archive is checksum-verified and the integration must apply with zero fuzz.

Controller tests compile the actual serialization function and initial polling
block against the HPS word-update contract, covering stale high bits, zero-input
startup, Start, release, player swaps and unchanged polls.

Host tests validate persisted shortcut records, disabled and invalid settings,
key consumption, repeat/release handling, extended Linux keys, runtime gates,
and placement before keyboard remapping. The build checks that achievement
support was enabled and that achievement and frontend implementations were
linked into the final executable. Real host curl tests exercise trusted TLS,
hostname rejection even with an insecure curlrc, plaintext/redirect rejection,
missing/invalid CA files, connection errors, quoted POST bodies and paths, long
quoted content types, and request-log redaction.
On macOS only the `/proc/self/exe` lookup is adapted to the host executable API;
actual Linux sidecar discovery and target curl remain device acceptance checks.

The output retains patched source, upstream archive, integration patch,
Dockerfile, build log, source pins, `MiSTer_RA_Degauss`, and its `.cacert.pem`
sidecar. To rebuild from the
output bundle, run `rebuild/scripts/build-ra-main.sh NEW_OUTPUT_DIRECTORY`.
Before distribution,
include corresponding source and build instructions: Main and this integration
are GPL-3.0, separate from the Rust frontend's licence. Upstream bundled
components retain their own licences and notices.

On-device acceptance must additionally verify the Frontend OSD action and saved
keyboard shortcut from an RA game, reopening the frontend, another RA launch,
and subsequent standard and Unstable launches. Preserve the original RA Main
and restore it when removing the test installation.

## Updating

Pin a new RA release and its archive checksum deliberately. Regenerate the
Degauss patch from the two recorded integration revisions when that integration
changes. Check RA input/menu additions before resolving a conflict; never replace
whole RA files with standard Main files. Run the host tests and full ARM build,
then repeat the device acceptance flow. A source update is not implicitly
validated by a previously tested binary.

## TLS installation and CA updates

Return to the menu so RA Main is inactive before replacing its executable.
Install the binary and CA bundle together, then launch the replacement. The bundle must have the executable's
full pathname plus `.cacert.pem`. For example, a binary installed as
`/media/fat/MiSTer_RA` needs `/media/fat/MiSTer_RA.cacert.pem`. The normal release instead
uses `/media/fat/degauss/MiSTer_RA_Degauss` and the adjacent
`MiSTer_RA_Degauss.cacert.pem`. Deriving this from
the running executable preserves custom installation locations. Back up both
exact destinations, verify staged hashes, and replace each through a temporary
file in its destination directory followed by an atomic rename. No system CA
store, credential file, or curl configuration is modified. Missing or invalid
bundles fail explicitly, with no insecure fallback.

The bundle comes from curl's [Mozilla CA extraction service](https://curl.se/docs/caextract.html).
For updates, select a dated official bundle, verify its published SHA-256, and
update the URL and expected digest in the build script together. A verified
bundle can also replace the sidecar without rebuilding Main. Retain its MPL-2.0
notice and official source URL when distributing it. Do not pin individual
server certificates, which would break legitimate certificate rotation.

Before using credentials, verify an anonymous HTTPS request with the target
curl and this exact bundle. Then verify the new binary's actual RA request path
and ensure failures identify certificate, CA-file, connection, and timeout
errors without printing credentials. An HTTP error response still establishes
TLS transport when curl succeeds; it does not establish authenticated login.
