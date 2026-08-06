# Degauss

Degauss is a speed-first fork of
[Zaparoo Frontend](https://github.com/ZaparooProject/zaparoo-frontend).
It features:

- The direct-read speed stack below.
- Favorites as a full view: the complete set loads at once, with sort
  modes, system and category scope filters.
- True random game across any folder and favourites.
- A `crt-light` theme that lifts surfaces for analog CRT output.
- Fast list travel: holding Left/Right pages the list at a fast,
  steady cadence, with cover work paused until you stop; favorite
  hearts stay visible while you scroll.
- Jump-to-letter, computed locally the moment a folder is fully
  loaded.

**The speed philosophy: read locally.** Everything the
interface shows is read directly from the local media database the
moment you ask for it, instead of being paged over RPC round trips.
Folder listings, cover art, favorites, play history, detail metadata,
the jump-to-letter index, and alternate-version discovery all resolve
on-device; in steady-state browsing the frontend performs **zero API
round trips**: the always-on connection to the local Zaparoo Core
service carries only writes, launches, and notifications. Measured on
a MiSTer over a large library:

- entering a folder of roughly two thousand entries: **~4.5 s → ~0.5 s**
- a visible page of covers: **~13x faster**
- favorites and recents: from about a second to **effectively instant**
- detail metadata: **~2 ms** median lookup against ~60–70 ms over RPC,
  letting the detail pane track the cursor essentially live

Every direct read keeps the RPC as its fallback behind a schema guard:
if the database ever changes shape, the affected read turns itself off
loudly and the frontend behaves exactly like upstream until it is
updated. Writes never touch the database directly.

**The look.** An opinionated more neutral monochrome interface where the only chroma is the
Degauss wordmark's own palette, one meaning per colour: yellow
(`#FFCD09`) is selection, teal (`#03A49D`) is state and activity (the
home tiles' focus, the status pill's progress), red (`#FE2E1D`) marks
favorites and errors. Yellow holds contrast for any use in both
themes; teal and red carry text on the dark theme and act as graphic
accents on `crt-light`, the dedicated theme that lifts the surfaces
for analog tubes.

Degauss requires [Zaparoo Core](https://zaparoo.org) on the device. It
is not affiliated with or endorsed by the Zaparoo Project or Wizzo Pty
Ltd; see the Trademarks section. The code remains under the upstream
PolyForm Noncommercial 1.0.0 license with all upstream copyright
notices preserved.

## Install on MiSTer

Degauss replaces the stock frontend binary; Zaparoo Core stays exactly
as installed. One-time setup, from SSH or the MiSTer console:

```bash
wget -O /media/fat/Scripts/update_degauss.sh https://raw.githubusercontent.com/giancarloerra/degauss/main/scripts/mister/update_degauss.sh
```

Then run **update_degauss** from the MiSTer Scripts menu. The script:

- checks the installed Zaparoo Core version against the release's
  declared minimum (and maximum, when one is declared) and refuses to
  install an incompatible build,
- backs up the stock frontend once as `frontend.zaparoo-original`, and
  the previously installed Degauss as `frontend.degauss-prev` on every
  update,
- verifies the download's checksum before touching anything, then
  swaps binaries with a staged move that is safe while the frontend is
  running.

Run it again anytime to update to the latest release. If a Zaparoo
update ever reinstalls the stock frontend, run the script again and
Degauss is back. To return to the stock frontend:

```bash
/media/fat/Scripts/update_degauss.sh -restore
```

Every Degauss release tracks an upstream Zaparoo Frontend release and
states the Core versions it was built and tested against.

## Build

Start with [docs/building.md](docs/building.md). It covers the packages you
need on a fresh machine and the MiSTer cross-build path.

Most commands go through the [`justfile`](justfile). Run `just --list` if you
need the full menu.

```bash
just build && just run    # desktop
./scripts/build-arm32.sh  # MiSTer ARM32 cross-build (Docker-only)
just test                 # ctest + cargo nextest
just lint                 # clang-format, clang-tidy, qmllint, rustfmt, clippy, cargo-deny
```

The MiSTer ARM32 path uses the official Docker Buildx toolchain image and does
not need Qt, CMake, Rust, or `just` installed on the host.

## Customize

You can override system artwork, the Hub menu icons, and system display names
without rebuilding. See [docs/customization.md](docs/customization.md).

`just test` and `just lint` need `cargo-nextest` and `cargo-deny`:

```bash
cargo install --locked cargo-nextest cargo-deny
```

## Trademarks

Degauss does not use the Zaparoo name or logo in its branding, and this
repository bundles no Zaparoo brand assets. Zaparoo is a trademark of Wizzo
Pty Ltd; where the interface or documentation says "Zaparoo Core", that is a
factual statement of compatibility with the separately distributed Zaparoo
Core service, not an affiliation or endorsement. See the Zaparoo
[Terms of Use](https://zaparoo.org/terms/) before reusing any Zaparoo marks
in derivative work.

## License

Copyright 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
Source available under the [PolyForm Noncommercial License 1.0.0](COPYING).
Non-commercial use only. For commercial licensing, contact
[legal@zaparoo.org](mailto:legal@zaparoo.org).

Third-party components:

- **Qt framework**: LGPLv3. Dynamically linked on desktop builds; statically
  linked on MiSTer ARM32. Object files for re-linking against a modified Qt
  are available on request at
  [legal@zaparoo.org](mailto:legal@zaparoo.org).
  See [`src/LICENSES/Qt-LGPL-NOTICE.txt`](src/LICENSES/Qt-LGPL-NOTICE.txt)
  and [`src/LICENSES/LGPLv3.txt`](src/LICENSES/LGPLv3.txt).
- **zaparoo-update**: optional third-party update integration (compiled out of
  this fork's MiSTer build; desktop dev presets keep upstream defaults),
  separately owned
  and licensed under PolyForm Noncommercial License 1.0.0. Commercial licenses
  for Zaparoo Frontend do not grant commercial rights to zaparoo-update;
  commercial use, distribution, or bundling with the update integration enabled
  requires a separate license from the zaparoo-update copyright holder,
  José Manuel Barroso Galindo <theypsilon@gmail.com>. See
  [`src/LICENSES/zaparoo-update-NOTICE.txt`](src/LICENSES/zaparoo-update-NOTICE.txt).
- **Noto Sans** fonts: SIL Open Font License 1.1, © The Noto Project Authors.
  See [`src/LICENSES/NotoSans-ATTRIBUTION.txt`](src/LICENSES/NotoSans-ATTRIBUTION.txt)
  and [`src/LICENSES/NotoSans-OFL.txt`](src/LICENSES/NotoSans-OFL.txt).
- **MxPlus HP 100LX 6x8** font: Creative Commons Attribution-ShareAlike 4.0
  International, © VileR. See
  [`src/LICENSES/MxPlus-ATTRIBUTION.txt`](src/LICENSES/MxPlus-ATTRIBUTION.txt).
- **Iconoir** UI icons: MIT License, © Luca Burgio and contributors.
  See [`src/LICENSES/Iconoir-ATTRIBUTION.txt`](src/LICENSES/Iconoir-ATTRIBUTION.txt).
- **Lucide** UI icons: ISC License, © 2024 Lucide Contributors (fork of Feather
  Icons by Cole Bemis). See
  [`src/LICENSES/Lucide-ATTRIBUTION.txt`](src/LICENSES/Lucide-ATTRIBUTION.txt).
- **Streamline** Core line icon (Handheld category): © Webalys LLC, used
  under the Streamline Free License — <https://streamlinehq.com>. See
  [`src/LICENSES/Streamline-ATTRIBUTION.txt`](src/LICENSES/Streamline-ATTRIBUTION.txt).
- **Controller Input Icons** by ElDuderino, released into the public domain.
  See [`src/LICENSES/controller-icons-ATTRIBUTION.txt`](src/LICENSES/controller-icons-ATTRIBUTION.txt).
- **Console logos** redrawn by Dan Patrick (MIT-licensed compilation; platform
  marks remain trademarks of their respective owners). See
  [`src/LICENSES/console-logos-ATTRIBUTION.txt`](src/LICENSES/console-logos-ATTRIBUTION.txt).
- **Wikimedia Commons public-domain text/logo assets** used for specific
  missing system logos. See
  [`src/LICENSES/wikimedia-public-domain-ATTRIBUTION.txt`](src/LICENSES/wikimedia-public-domain-ATTRIBUTION.txt).
- **Noun Project icons** used in 2-player system logo composites. See
  [`src/LICENSES/NounProject-ATTRIBUTION.txt`](src/LICENSES/NounProject-ATTRIBUTION.txt).

See all bundled asset and third-party notices in [`src/LICENSES/`](src/LICENSES/).
