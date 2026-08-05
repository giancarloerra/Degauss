# Degauss

Degauss is a game frontend for
[Zaparoo Core](https://zaparoo.org), maintained as an independent fork of
[Zaparoo Frontend](https://github.com/ZaparooProject/zaparoo-frontend). It is
not affiliated with or endorsed by the Zaparoo Project or Wizzo Pty Ltd;
Zaparoo is a trademark of Wizzo Pty Ltd, and references to Zaparoo Core here
are factual statements of compatibility. The code remains under the upstream
PolyForm Noncommercial 1.0.0 license with all upstream copyright notices
preserved.

## Brand palette

The UI is deliberately monochrome (each grey preserves the relative
luminance of the upstream colour it replaced), so the wordmark's three
colours are the only chroma available and read as highlights wherever
they appear. Contrast ratios against the theme surfaces, for choosing
where each may carry text versus purely graphic accents:

| Colour | Hex | Dark panels / cards | CRT-light panels / cards | Safe uses |
|---|---|---|---|---|
| Yellow | `#FFCD09` | 11.2:1 / 10.3:1 | 7.6:1 / 6.0:1 | Anything, both themes, including small text |
| Teal | `#03A49D` | 5.5:1 / 5.0:1 | 3.7:1 / 2.9:1 | Text on dark; graphic accents (rings, bars, badges) on CRT-light |
| Red | `#FE2E1D` | 4.5:1 / 4.2:1 | 3.0:1 / 2.4:1 | Text on dark (borderline); graphic accents on CRT-light |

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
