<p align="center">
  <img src="assets/degauss-logo.png" alt="Degauss" width="360">
</p>

<p align="center"><strong>A blazing-fast, lightweight frontend for MiSTer FPGA.</strong></p>

---

Degauss plays nice with the standard MiSTer setup, folders and scripts, instead of trying to replace it all.

It browses the games and folders on your card as they already are, with the artwork and
the metadata a scraper already wrote, and builds a beautiful, fast UI around them. It reads the
same folders the stock menu reads, so it agrees with the rest of your setup and scripts by
construction.

It runs without background processes and without replacing the stock system. It is optimised for
speed and for CRTs, with several views, custom theming, different fonts, many features for favourites, browsing, lists, random discovery, a screensaver and a full set of options: it works out of the box, and can be tuned to your liking.

Source available, written in Rust and using Slint, and licensed for non-commercial use.

> ### Feature request or bug report? Star this repository as well to support it ⭐️
>
> Degauss is written and maintained by one person in his own time. A star is
> the whole of what it costs you and the clearest signal that the work is
> worth continuing! If you want something changed or something is broken,
> [open an issue](../../issues/new/choose) and if you starred it I'll know it matters to you beyond just requesting things :-)

## Screenshots

| | | |
|---|---|---|
| <img src="docs/screenshots/crt-root.png" alt="The root menu" height="200"> | <img src="docs/screenshots/crt-details.png" alt="Details view" height="200"> | <img src="docs/screenshots/crt-tiled.png" alt="Tiled view" height="200"> |
| The root menu | Details view | Tiled view |
| <img src="docs/screenshots/crt-carousel.png" alt="Carousel view" height="200"> | <img src="docs/screenshots/crt-theme-blueorange-root.png" alt="Blue-Orange theme, Pixel font" height="200"> | <img src="docs/screenshots/crt-theme-blueorange-tiled.png" alt="Blue-Orange theme, tiled" height="200"> |
| Carousel view | Blue-Orange theme, Pixel font | Blue-Orange theme, tiled |
| <img src="docs/screenshots/crt-theme-amber-folders.png" alt="Amber theme example" height="200"> | <img src="docs/screenshots/crt-theme-amber-details.png" alt="Amber theme, Smooth 2 text" height="200"> | <img src="docs/screenshots/crt-favourites.png" alt="Inside the favourites" height="200"> |
| Amber theme, a system's folders | Amber theme, Smooth 2 text | Inside the favourites |
| <img src="docs/screenshots/crt-context.png" alt="Contextual actions" height="200"> | <img src="docs/screenshots/crt-options-top.png" alt="Options" height="200"> | <img src="docs/screenshots/crt-options-middle.png" alt="Options, further down" height="200"> |
| Contextual actions | Options | Options, further down |
| <img src="docs/screenshots/crt-letters.png" alt="Jump to letter" height="200"> | <img src="docs/screenshots/crt-search.png" alt="Search inside a folder" height="200"> | <img src="docs/screenshots/crt-saver.png" alt="The screensaver slideshow" height="200"> |
| Jump to letter | Search inside a folder | The screensaver slideshow |

## Watch Degauss speed in action (v0.2.0)

<p align="center">
  <a href="https://www.youtube.com/watch?v=w4dGS4kX35Q">
    <img src="https://img.youtube.com/vi/w4dGS4kX35Q/maxresdefault.jpg"
         alt="Degauss in motion" width="640">
  </a>
</p>

Video showing version 0.2.0 (themes/looks/options changed since then, but the speed is the same!). Click the image to watch it on YouTube.

## Table of contents

- [Using it](#using-it)
- [Why Degauss](#why-degauss)
- [Installing](#installing)
  - [Installing and staying up to date automatically via update_all script](#installing-and-staying-up-to-date-automatically-via-update_all-script)
  - [Upgrading from an earlier release](#upgrading-from-an-earlier-release)
  - [Optional: starting Degauss from the stock menu on-demand](#optional-starting-degauss-from-the-stock-menu-on-demand)
  - [External storage support](#external-storage-support)
- [Views](#views)
- [Settings](#settings)
  - [Themes and colours](#themes-and-colours)
- [Artwork and metadata](#artwork-and-metadata)
  - [Where the gamelist goes](#where-the-gamelist-goes)
  - [System and category images](#system-and-category-images)
- [Tips](#tips)
  - [Making the gamelists](#making-the-gamelists)
  - [Working with an agent](#working-with-an-agent)
    - [The one thing that catches everyone](#the-one-thing-that-catches-everyone)
    - [After any change](#after-any-change)
  - [Keeping the card in order (additional bonus!)](#keeping-the-card-in-order-additional-bonus)
- [The command line (CLI)](#the-command-line-cli)
  - [Checking a card](#checking-a-card)
  - [Seeing it without the screen](#seeing-it-without-the-screen)
  - [Measuring it](#measuring-it)
- [Building it yourself](#building-it-yourself)
- [Licence](#licence)

## Using it

| Control | Does |
|---|---|
| **up / down** | move through lists |
| **left / right** | scroll speed, 0.5x to 12x, or what the **Left and right** setting says: letter jumps, page jumps, or plain movement. In a menu, change the setting |
| **A** (enter) | open a folder, launch a game |
| **B** (escape) | back, out of the folder |
| **X** (tab) | context menu: random game, random favourite, keep or drop a favourite, jump to letter, search, hide a row, rebuild this system, change view, etc. With **Hold X (1s) to add/remove fav** enabled, hold X for one second over a game to use the favourite shortcut |
| **Y** (space) | menu: options, help, about, exit |

A gamepad needs no setup and a keyboard is never needed. While Degauss
owns the screen, MiSTer sends the d-pad as arrows and the face buttons as
Enter, Escape, Space and Tab.

## Why Degauss

- **Three pillars.** Performance (for large libraries and images), Simplicity (vs overengineering), Adherence to the MiSTer way (standards, scripts, folders).
- **Fast.** 0.47 s from launch to first frame. 0.57 s to open a folder of
  12,605 games.
- **Artwork instant browsing.** Full-size pictures are read straight from
  the card as you scroll, with no thumbnails and nothing pre-generated.
  They keep up at the fastest scroll speed.
- **Nothing resident.** No service, no daemon, no port, no background
  process, nothing at boot. One program, running only while you are
  looking at it.
- **Self-contained.** One program, with no runtime, toolkit or library to
  install beside it, and small footprint while it runs. The index it builds
  costs about 300 bytes a game, so it stays in the low megabytes for an
  ordinary collection and is the only thing that grows with the size of
  yours.
- **CRT-optimised.** 352x240, 1:1 with the analog output, no scaler in
  between. Overscan margins and screen position are settings. Larger
  framebuffers are laid out from their own size, so HDMI works too.
- **The card is the truth.** Reorganise, rename or move files with any
  tool and the browser follows: nothing has to be re-imported or re-tagged.
  The index it keeps is only a copy of what the card already says, and
  **Options -> Rebuild all system lists** quickly updates all systems after an update,
  and the contextual menu's **Rebuild this system list** does one system alone.
- **Full metadata.** Description, publisher, developer, release date,
  players and language, read from `gamelist.xml`.
- **Favourites are MiSTer's favourites**, written into `_@Favorites` in
  MiSTer's own format. One made here works in the stock menu; one made
  anywhere else appears here.
- **Awkward systems handled** without hassle: AmigaVision, DOS,
  Neo Geo, Arcade, X68000, and cores that are several machines.

Measured on the DE10-Nano's own hardware, with a large multi-system
collection indexed.

## Installing

Download the archive from the
[latest release](../../releases/latest) and copy its contents onto the card,
so the files land here:

```
/media/fat/degauss/MiSTer_Degauss
/media/fat/Scripts/degauss.sh
/media/fat/Scripts/.config/degauss/degauss
/media/fat/Scripts/.config/degauss/degauss.toml
/media/fat/Scripts/.config/degauss/systems.toml
/media/fat/Scripts/.config/degauss/logos/
/media/fat/Scripts/.config/degauss/themes/
```

Then add one line to the `[MiSTer]` section of `/media/fat/MiSTer.ini`:

```ini
main=degauss/MiSTer_Degauss
```

Reboot. Degauss comes up in place of the stock menu, and leaving a game
returns to it.

To remove Degauss, delete the `main=` line and the files above. Nothing
else on the card is touched. 

### Installing and staying up to date automatically via update_all script

Degauss publishes a Downloader database, so `update_all` can keep it current
with everything else on the card. Add two lines to the bottom of
`/media/fat/downloader.ini`:

```ini
[degauss]
db_url = 'https://github.com/giancarloerra/Degauss/releases/latest/download/degauss.json.zip'
```

Then run `update_all` or `downloader` as usual, and both binaries and the
files beside them come down and stay updated.

The `main=` line still has to be added by hand, once.

### Upgrading from an earlier release

Earlier releases lived in `/media/fat/Scripts/.degauss/`. When updating
over one, `degauss.sh` moves everything into the new folder on its next
start, says `Migrating Degauss data to Scripts/.config/degauss...` on the
console while it does,
and removes the old folder: your settings, resume state and index carry
over, a `degauss.toml`, `systems.toml` or logo you edited stays the
active copy, and any other file you kept in the folder follows the move
untouched. There is nothing to do.

If anything about an install or upgrade looks wrong, the binary can look
itself over:

```bash
/media/fat/Scripts/.config/degauss/degauss --check-install
```

It reports what is present, missing, broken or still waiting in the old
folder, and says ok when there is nothing to say.

### Optional: starting Degauss from the stock menu on-demand

The installation above is the recommended way to use Degauss. It opens Degauss automatically on boot, returns to it after leaving a game, and adds a **Frontend** entry to the System menu while a game is running.

If you prefer to keep the stock MiSTer menu as the default, leave the `main=` line as it is in `MiSTer.ini`. You can then start Degauss when wanted by opening the OSD and selecting **Scripts → degauss**.

This requires MiSTer's framebuffer terminal to be enabled:

```ini
fb_terminal=1
```

It is enabled by default in the standard MiSTer configuration.

Degauss can browse the collection and launch games normally when started this way. However, after leaving a game/core, MiSTer will return to the stock menu instead of reopening Degauss automatically. There's not going to be the Frontend option anymore in the cores menu, and you'll need to re-launch Degauss from the OSD if you want to go back to it.

Doing it in this way, the installed `/media/fat/degauss/MiSTer_Degauss` file is not used.

### External storage support

Games on a USB stick, the network share or the CIFS mount are found
without any setup, in the order MiSTer's own loader searches:
`/media/usb0` to `usb5`, then `/media/network`, then `/media/fat/cifs`,
then the card, each under its `games` folder. For each system folder the
first place that has it wins, so a system kept on both the stick and the
card browses from the stick, exactly as the stock menu would load it.
Different folders of one system may resolve in different places.

Two things to know. Storage is looked for when Degauss starts, so plug
the stick in first (or restart after); and moving a system between
storages changes its paths, so its listing is stale until **Rebuild this
system** or a full rebuild. A layout the defaults do not cover is one
`game_roots` edit in `degauss.toml` away.

The first run reads the card and writes an index, about a minute for a
full one of 97k+ games. It never does that again on its own: **Options → Rebuild all system lists**
is how you tell it the card has changed (for example after adding new games). New images and metadata are read on the fly.
Adding games to one system does not need the whole card read again:
**X → Rebuild this system list** inside that system reads just its folders.

## Views

- **List**: plain text, the most rows on screen.
- **Details**: the list beside a large picture, with what the gamelist
  knows underneath it.
- **Tiled**: a grid of pictures.
- **Carousel**: one large cover with its neighbours either side.

Folders appear in square brackets with the number of games inside them,
counted through every subfolder, and can sit before the games or after
them. Favourites carry a heart in every view and can be gathered at the
top of their folder.

**Hide this**, in the contextual menu, takes any row out of the list: a
game, a folder, or a whole system while you are looking at the system
list. That is separate from the folders and systems left out because they
hold no games at all, which **Show systems with no games** governs. **Show what
you hid** shows the rows you hid without unhiding them, and **Unhide
everything** puts them all back.

The screensaver, after the set time, drifts through game images taken from your own
card.

## Settings

**Options**

One screen holds everything, in groups a blank row apart, top to bottom:

| Setting | Does |
|---|---|
| Scroll speed | How fast a held direction moves through the list. 3x out of the box |
| Skip artwork faster than | Above this speed, pictures wait until the list stops. 6x out of the box |
| Left and right behaviour | What left and right do while browsing: Scroll speed change (the default), Letter, Page or Direction. Letter and Page repeat while held; Direction frees up and down to move a whole row in Tiled |
| Theme | A palette from the `themes/` folder, or Standard for the colours in `degauss.toml`. See [Themes and colours](#themes-and-colours) |
| View | Details, Tiled, List or Carousel |
| Text | The typeface: Smooth, Pixel (a DOS font on whole pixels), and the bolder Smooth 2 and Pixel 2 |
| Artwork | Turn pictures off entirely |
| Bottom bar while browsing | The strip with the time and the buttons. Menus always keep it |
| Favourites first | Gather a folder's favourites at its top, in the same alphabet |
| Hold X (1s) to add/remove fav | Off by default. Hold X for one second over a game to open the normal favourite-folder chooser, or to remove it when it is already a favourite. The shortcut does nothing in the master Favourites system |
| Folders before games | On, folders lead a system's listing; off, the games come first |
| Random game behaviour | Whether a random pick starts the game, or only moves to it so you can look first |
| Show Other folder | Show the Other group, the cores that are not games |
| Show Utility folder | Show the Utility group, test patterns and measurement cores |
| Show systems with no games | Systems and folders holding nothing are left out on their own; this shows them. Off by default |
| Show what you hid | Show what you hid yourself with **Hide this** |
| Unhide everything | Put back everything you hid yourself, in every folder and every system |
| Edge margin, sides | Keep this much of each side clear of the bezel |
| Edge margin, top and bottom | The same, vertically |
| Screen position, sideways | Nudge the picture, for a screen that sits off centre |
| Screen position, up and down | The same, vertically |
| Screensaver | How long with nothing pressed before pictures start |
| Rebuild all system lists | Read the whole card again. Run it after adding games, cores or artwork; for one system, the contextual menu's **Rebuild this system list** is quicker |
| Developer | The screen below |

**Developer**

| Setting | Does |
|---|---|
| Drawing path | Draw into the screen directly, or into memory first |
| Performance readout | Replace the key hints with frame timings |

`degauss.toml` is documentation as much as configuration: every value
explains itself. Anything changed from the Options screen is written to
`settings.toml` beside it, so your changes never overwrite those notes.
Delete `settings.toml` to go back to the documented defaults.

### Themes and colours

The ten palette colours Degauss draws with are roles in the `[colors]`
block of `degauss.toml`, validated as `#rrggbb` when the file is read
(the wordmark's `logo` colour, below, is the one colour that lives
outside it):

| Role | Does |
|---|---|
| `background` | The ground behind everything |
| `panel` | The title strip across the top of menu screens |
| `surface` | Raised surfaces: cards, the artwork plate, modal panels |
| `bar` | The strip along the bottom, darker so its small text reads |
| `text` | Ordinary text |
| `text_dim` | Secondary text: counts, hints, the clock |
| `accent` | The selection bar and focus ring |
| `accent_text` | Text drawn on top of `accent` |
| `state` | Toggles, progress, anything that is "on" |
| `favorite` | The favourite markers |

A theme is one `.toml` file in the `themes/` folder beside `degauss.toml`
(`Scripts/.config/degauss/themes/` on a card), naming any of those ten
roles, and its file stem is its name in the **Theme** row of Options.
The roles go in as bare keys or under a `[colors]` header, so the block
from `degauss.toml` pastes in unchanged. A theme overlays your
`[colors]`: roles it does not name show through from `degauss.toml`. One
more key, `logo`, at the top level, draws the wordmark as a flat
silhouette in that colour; leave it out and the wordmark keeps its own
three colours.

A complete theme can be three lines. `themes/Night.toml`:

```toml
background = "#101318"
accent = "#33FF33"
logo = "#33FF33"
```

picks a darker ground, a green selection bar and a green wordmark, and
every other colour shows through from your `[colors]`. Copying the whole
`[colors]` block out of `degauss.toml` and pasting it in, header and
all, is also a valid theme, ready to be edited.

Three themes ship as starting points: `Amber` after the amber phosphor,
`Mono`, the standard greys with the colour taken out, and `Blue-Orange`,
slate blue with an orange highlight. None of them names `favorite`, so
the hearts keep your own favourite colour under every palette: red,
unless you changed it in `[colors]`. The updater manages
those three files, so edits to them are overwritten on the next update;
copy one under a new file name to make it your own. The folder is read once at startup, so a file added while
Degauss is running appears after the next start, and the chosen theme is
remembered by name in `settings.toml`. If the file it names is missing or
broken, Degauss uses the standard palette and a message says so.

## Artwork and metadata

Degauss reads `gamelist.xml` in the EmulationStation format, in the same
folder as the games. Paths inside it are relative to that folder. Nothing
else is needed and nothing is written back.

The minimum useful entry is a path, a name and a picture:

```xml
<gameList>
  <game>
    <path>./Boulder Dash.d64</path>
    <name>Boulder Dash</name>
    <screenshot>./media/screenshot/Boulder Dash.png</screenshot>
  </game>
</gameList>
```

Everything Degauss reads, in one entry:

```xml
<game>
  <path>./Gran Turismo 2 (Arcade Mode).chd</path>
  <name>Gran Turismo 2 (Arcade Mode)</name>
  <desc>Gran Turismo 2 is fundamentally based on the racing game genre.</desc>
  <publisher>Sony Computer Entertainment</publisher>
  <developer>Polyphony Digital</developer>
  <releasedate>19991223T000000</releasedate>
  <players>1-2</players>
  <lang>en</lang>
  <genre>Racing</genre>
  <favorite>false</favorite>
  <image>./media/covers/gt2.png</image>
  <screenshot>./media/screenshot/gt2.png</screenshot>
  <thumbnail>./media/thumbs/gt2.png</thumbnail>
</game>
```

`<image>`, `<screenshot>` and `<thumbnail>` are all read, in that order of
preference. `<releasedate>` is the EmulationStation timestamp form and is
shown as a date; a month or day of zero means only the year is claimed.
Only `<path>` is required.

The same file works for every system, awkward ones included. What changes
is only what `<path>` points at:

```xml
<!-- AmigaVision: a title inside the disk image, not a file on the card -->
<game>
  <path>./Games/Zool 2 (AGA)[en]</path>
  <name>Zool 2 (AGA)[en]</name>
  <image>./media/screenshots/zool2.png</image>
</game>

<!-- Neo Geo: the games are folders of ROMs -->
<game>
  <path>./mslug</path>
  <name>Metal Slug</name>
  <screenshot>./media/screenshot/mslug.png</screenshot>
</game>

<!-- Arcade: an .mra names its own core and ROM set -->
<game>
  <path>./DoDonPachi (World, 1997 25 Master Ver.).mra</path>
  <name>DoDonPachi</name>
  <screenshot>./media/screenshot/ddonpach.png</screenshot>
</game>
```

### Where the gamelist goes

One `gamelist.xml` at the top of each folder a system uses.

Paths inside it are relative to that folder, and subfolders are covered by
the same file. Most systems use a single folder under `/media/fat/games`:

| System | Gamelist |
|---|---|
| Commodore 64 | `/media/fat/games/C64/gamelist.xml` |
| SNES | `/media/fat/games/SNES/gamelist.xml` |
| PlayStation | `/media/fat/games/PSX/gamelist.xml` |
| Amiga | `/media/fat/games/Amiga/gamelist.xml` |
| Neo Geo | `/media/fat/games/NEOGEO/gamelist.xml` |

Some sit outside that folder, and some are spread over several. Every
folder gets its own gamelist, and the system is still shown as one:

| System | Gamelist |
|---|---|
| Arcade | `/media/fat/_Arcade/gamelist.xml` |
| PC (DOS) | `/media/fat/games/AO486/gamelist.xml`<br>`/media/fat/_DOS Games/gamelist.xml` |
| Genesis | `/media/fat/games/MegaDrive/gamelist.xml`<br>`/media/fat/games/Genesis/gamelist.xml` |
| Neo Geo CD | `/media/fat/games/NeoGeo-CD/gamelist.xml`<br>`/media/fat/games/NEOGEO/gamelist.xml` |
| SG-1000 | `/media/fat/games/SG1000/gamelist.xml`<br>`/media/fat/games/Coleco/gamelist.xml`<br>`/media/fat/games/SMS/gamelist.xml` |

Systems that share a folder share its gamelist: Neo Geo and Neo Geo MVS
both read `/media/fat/games/NEOGEO`.

`/media/fat/Scripts/.config/degauss/degauss --list-systems` prints where every
system resolved on your own card, which is the answer for that card.

<details>
<summary>Every system and the folders it reads</summary>

| System | Folders holding its gamelist |
|---|---|
| 3DO | `/media/fat/games/3DO` |
| Adventure Vision | `/media/fat/games/AVision` |
| Amiga | `/media/fat/games/Amiga` |
| Amiga CD32 | `/media/fat/games/AmigaCD32` |
| Amstrad CPC | `/media/fat/games/Amstrad` |
| Amstrad PCW | `/media/fat/games/Amstrad PCW` |
| Apogee BK-01 | `/media/fat/games/APOGEE` |
| Apple I | `/media/fat/games/Apple-I` |
| Apple IIe | `/media/fat/games/Apple-II` |
| Apple IIGS | `/media/fat/games/Apple-IIgs` |
| Apple Lisa | `/media/fat/games/LISA` |
| Arcade | `/media/fat/_Arcade` |
| Arcadia 2001 | `/media/fat/games/Arcadia` |
| Arduboy | `/media/fat/games/Arduboy` |
| Atari 2600 | `/media/fat/games/ATARI7800`<br>`/media/fat/games/Atari2600` |
| Atari 5200 | `/media/fat/games/ATARI5200` |
| Atari 7800 | `/media/fat/games/ATARI7800` |
| Atari 800XL | `/media/fat/games/ATARI800` |
| Atari Lynx | `/media/fat/games/AtariLynx` |
| Atom | `/media/fat/games/AcornAtom` |
| Audio | `/media/fat/games/MegaVGMDrive` |
| Bally Astrocade | `/media/fat/games/Astrocade` |
| BBC Micro/Master | `/media/fat/games/BBCMicro` |
| BK0011M | `/media/fat/games/BK0011M` |
| Casio PV-1000 | `/media/fat/games/Casio_PV-1000` |
| Casio PV-2000 | `/media/fat/games/Casio_PV-2000` |
| CD-i | `/media/fat/games/CD-i` |
| Channel F | `/media/fat/games/ChannelF` |
| CHIP-8 | `/media/fat/games/Chip8` |
| ColecoVision | `/media/fat/games/Coleco` |
| Commodore 16 | `/media/fat/games/C16` |
| Commodore 64 | `/media/fat/games/C64` |
| Commodore PET 2001 | `/media/fat/games/PET2001` |
| Commodore VIC-20 | `/media/fat/games/VIC20` |
| EDSAC | `/media/fat/games/EDSAC` |
| Electron | `/media/fat/games/AcornElectron` |
| Famicom Disk System | `/media/fat/games/NES`<br>`/media/fat/games/FDS` |
| Galaksija | `/media/fat/games/Galaksija` |
| Gamate | `/media/fat/games/Gamate` |
| Game & Watch | `/media/fat/games/GameNWatch`<br>`/media/fat/games/Game and Watch` |
| Game Gear | `/media/fat/games/SMS`<br>`/media/fat/games/GameGear` |
| Game Gear (2 Player) | `/media/fat/games/GameGear2P` |
| Gameboy | `/media/fat/games/GAMEBOY` |
| Gameboy (2 Player) | `/media/fat/games/GAMEBOY2P` |
| Gameboy Advance | `/media/fat/games/GBA` |
| Gameboy Advance (2 Player) | `/media/fat/games/GBA2P` |
| Gameboy Color | `/media/fat/games/GAMEBOY`<br>`/media/fat/games/GBC` |
| Genesis | `/media/fat/games/MegaDrive`<br>`/media/fat/games/Genesis` |
| Genesis 32X | `/media/fat/games/S32X` |
| Groovy | `/media/fat/games/Groovy` |
| Intellivision | `/media/fat/games/Intellivision` |
| Interact | `/media/fat/games/Interact` |
| Jaguar | `/media/fat/games/Jaguar` |
| Jaguar CD | `/media/fat/games/Jaguar` |
| Jupiter Ace | `/media/fat/games/Jupiter` |
| Laser 350/500/700 | `/media/fat/games/Laser` |
| Lynx 48/96K | `/media/fat/games/Lynx48` |
| M5 | `/media/fat/games/Sord M5` |
| Macintosh Plus | `/media/fat/games/MACPLUS` |
| Magnavox Odyssey2 | `/media/fat/games/ODYSSEY2` |
| Master System | `/media/fat/games/SMS` |
| Mattel Aquarius | `/media/fat/games/AQUARIUS` |
| Mega Duck | `/media/fat/games/GAMEBOY`<br>`/media/fat/games/MegaDuck` |
| MSX | `/media/fat/games/MSX` |
| MSX1 | `/media/fat/games/MSX1` |
| MultiComp | `/media/fat/games/MultiComp` |
| Neo Geo | `/media/fat/games/NEOGEO` |
| Neo Geo CD | `/media/fat/games/NeoGeo-CD`<br>`/media/fat/games/NEOGEO` |
| Neo Geo MVS | `/media/fat/games/NEOGEO` |
| Neo Geo Pocket | `/media/fat/games/NGP` |
| Neo Geo Pocket Color | `/media/fat/games/NGPC` |
| NES | `/media/fat/games/NES` |
| NES Music | `/media/fat/games/NES` |
| Nintendo 64 | `/media/fat/games/N64` |
| OpenBOR | `/media/fat/games/OpenBOR` |
| Orao | `/media/fat/games/ORAO` |
| Oric | `/media/fat/games/Oric` |
| PC (DOS) | `/media/fat/games/AO486`<br>`/media/fat/_DOS Games` |
| PC/XT | `/media/fat/games/PCXT` |
| PDP-1 | `/media/fat/games/PDP1` |
| PICO-8 | `/media/fat/games/PICO-8` |
| Playstation | `/media/fat/games/PSX` |
| PMD 85-2A | `/media/fat/games/PMD85` |
| Pocket Challenge V2 | `/media/fat/games/WonderSwan`<br>`/media/fat/games/PocketChallengeV2` |
| Pokemon Mini | `/media/fat/games/PokemonMini` |
| RX-78 Gundam | `/media/fat/games/RX78` |
| SAM Coupe | `/media/fat/games/SAMCOUPE` |
| Saturn | `/media/fat/games/Saturn` |
| Sega CD | `/media/fat/games/MegaCD` |
| SG-1000 | `/media/fat/games/SG1000`<br>`/media/fat/games/Coleco`<br>`/media/fat/games/SMS` |
| Sinclair QL | `/media/fat/games/QL` |
| SNES | `/media/fat/games/SNES` |
| SNES Music | `/media/fat/games/SNES` |
| Specialist/MX | `/media/fat/games/SPMX` |
| Super Gameboy | `/media/fat/games/SGB` |
| SuperGrafx | `/media/fat/games/TGFX16` |
| SuperVision | `/media/fat/games/SuperVision` |
| SV-328 | `/media/fat/games/SVI328` |
| Tandy MC-10 | `/media/fat/games/AliceMC10` |
| Tatung Einstein | `/media/fat/games/TatungEinstein` |
| TI-99/4A | `/media/fat/games/TI-99_4A` |
| TRS-80 | `/media/fat/games/TRS-80` |
| TRS-80 CoCo 2 | `/media/fat/games/CoCo2` |
| TS-1500 | `/media/fat/games/ZX81` |
| TS-Config | `/media/fat/games/TSConf` |
| TurboGrafx-16 | `/media/fat/games/TGFX16` |
| TurboGrafx-16 CD | `/media/fat/games/TGFX16-CD` |
| Tutor | `/media/fat/games/TomyTutor` |
| UK101 | `/media/fat/games/UK101` |
| VC4000 | `/media/fat/games/VC4000` |
| Vector-06C | `/media/fat/games/VECTOR06` |
| Vectrex | `/media/fat/games/VECTREX` |
| Virtual Boy | `/media/fat/games/VirtualBoy` |
| VTech CreatiVision | `/media/fat/games/CreatiVision` |
| WonderSwan | `/media/fat/games/WonderSwan` |
| WonderSwan Color | `/media/fat/games/WonderSwan`<br>`/media/fat/games/WonderSwanColor` |
| X68000 | `/media/fat/games/X68000` |
| ZX Spectrum | `/media/fat/games/Spectrum` |
| ZX Spectrum Next | `/media/fat/games/ZXNext` |

</details>

System logos are read from the `logos` folder beside `degauss.toml`,
named after the system. The 89 files in `assets/logos/` are copied from
lehcimcramtrebor/es-theme-forever (`CUSTOMIZE/logos`). The marks themselves 
are the trademarks of their owners, used here only to identify the systems.

### System and category images

Degauss reads system and category images from the `logos` folder beside
`degauss.toml`. In a normal installation this is:

```text
/media/fat/Scripts/.config/degauss/logos/
```

A system image is named after its system ID, for example `C64.png` or
`PSX.jpg`.

To give a category a fixed image, name the file exactly after the category:

```text
Arcade.png
Console.png
Computer.png
Utility.png
Other.png
```

If no category image exists, Degauss chooses the logo of one of the systems
in that category at random. Lowercase `.png` and `.jpg` extensions are
supported. Restart Degauss after adding or replacing an image.

`Arcade.png` is already included as the default fixed image for the Arcade
category.

## Tips

### Making the gamelists

Degauss reads the gamelists; it does not write them. There are mainly two ways to get them, and the last one is what I actually use and works very well.

**On a computer, with the card in it.** Any scraper written for
EmulationStation produces exactly the file Degauss reads.

- [Skraper](https://www.skraper.net) is free. It is a .NET application and
  Windows is the only native build; Linux and macOS go through WINE. Set its
  output to RecalBox or RetroPie mode, which is the setting that writes
  `gamelist.xml` rather than a frontend's own database. It already knows
  MiSTer's folder names, so you can point it straight at the card or at a
  share.
- [Skyscraper](https://github.com/Gemba/skyscraper) is a C++ command line
  scraper, Linux first, and the one RetroPie uses. It caches everything it
  fetches and builds the gamelists from that cache, so changing your mind
  about the artwork costs nothing the second time.

**With an AI, over ssh.** This is what I do, and it is far and away the
best for me. Put an ssh key on the MiSTer, point an AI coding agent
at it, and ask. It reads the card as it is, works out which systems are
there, matches names against what it finds, writes a `gamelist.xml` per
folder, finds collections containing the images you need, and comes back with what it could not resolve instead of quietly skipping it (and almost always can have a good guess at it!).

The part AI is best at is also managing it all. A gamelist goes stale
the moment you add a game, and an agent can be told to look at what changed and only touch that, so keeping the lists current after adding games is super quick.

### Working with an agent

Put an ssh key on the MiSTer, point a coding agent at it, get the IP address for the agent, and tell the agent what
you want. There is nothing to install and nothing in Degauss to configure.
Any agent that can hold an ssh session will do. I use Claude Code.

The one rule that makes it safe: it looks, it tells you what it found, you
decide, then it acts. Never let it write to the card on its own initiative.

Everything else your agent can work out or ask you about. To save you both
the first hour, paste this into it at the start:

```
You are working on a MiSTer card for the Degauss frontend, over ssh.

https://github.com/giancarloerra/Degauss is the address to get the original README and code if needed.

Before you change anything on the card, tell me what you found and wait.

- Back up any gamelist.xml before you write it, next to the original.
- After writing one, read it back: check it still parses, and that every
  picture it names is really on the card.
- Re-read the card before telling me anything about it. Whatever you read
  earlier may have changed since.
- If you cannot find the right picture for a game, leave it without one.
  Never use a picture of a different game.
- Before telling me something is missing, search everything rather than the
  name you expected, and tell me what you searched.

Degauss reads gamelist.xml files straight from the card. It never writes
them, so keeping them right is your job.

Start from Degauss's own audit rather than forming your own view of the card:

  /media/fat/Scripts/degauss.sh --audit

That prints one line per system with how many games it found and how many
have artwork, then lists the problems underneath. Also useful:
--report --system <id> to expand one system, --list-systems when a system
is missing, --dry-run-launch when a game will not start.

Before editing a gamelist:
- Entries are either flat, or a parent holding the details with children
  pointing at it. A child inherits field by field, and its own value wins.
- Artwork is read as image, then screenshot, then thumbnail, and paths are
  relative to the folder the gamelist is in.
- Degauss does not check that those pictures exist. You have to.
- Degauss caches what it found, and only rebuilds when asked from its menu,
  so nothing you change appears until I rebuild it.
```

#### The one thing that catches everyone

Scrapers sometimes file a game under a different game's name. They group by
their own database record, so an arcade original and its clones can end up
sharing one entry, and the title you see is whichever one the scraper chose.
The game is not missing. It is sitting under a name you would never look
for, which is much harder to spot than an empty row.

If you hit it, ask your agent to detach that entry rather than rename it.
Renaming fixes the title and leaves the wrong description, publisher and
year behind it. Detached and left without a name, Degauss simply shows the
filename as it is on the card, which is usually clearer anyway.

It is worth asking your agent to check the whole card for this once.

#### After any change

Rebuild the cache from the Degauss menu, or just the changed system with
**X → Rebuild this system list** while inside it. Artwork is never cached, so
pictures appear as soon as they are on the card. Favourites are shared with
MiSTer's own `_@Favorites` folder, so an agent can add one and the stock
menu will agree with it, and vice versa.

### Keeping the card in order (additional bonus!)

The same thing works for the card itself. After every `update_all` I have an
agent go over it and tell me what it found: cores that arrived or vanished,
games with no artwork, artwork with no game, folders that ended up in the
wrong place, gamelists that no longer match what is on the card, and
anything a core needs that is missing. It reports; I decide; it executes.

## The command line (CLI)

Degauss is a normal program, so it can be run over SSH without taking the
screen. That is worth having for two things: finding out what it makes of a
card, and seeing what a change looks like without standing in front of the
machine.

```bash
/media/fat/Scripts/.config/degauss/degauss --help
```

`--config` and `--systems` default to `degauss.toml` and `systems.toml`
beside the binary, which is where they live, so the flags are only needed
to point somewhere else. `degauss.sh` passes them explicitly.

### Checking a card

| Flag | What it answers |
|---|---|
| `--audit` | Every system, one line each: games found, artwork bound, folders. A system with a `gamelist.xml` but no artwork bound, or with no games at all, is listed again underneath as a problem. A whole card checked without opening a hundred systems by hand. |
| `--list-systems` | Which systems this card actually has, and where each one resolved. The answer to "why is my system missing". |
| `--check-install` | The installation itself: what is present, missing, broken or left half-migrated. The first thing to run when something looks wrong. |
| `--report` | One system in detail, with `--system <id>`. |
| `--dry-run-launch` | The MGL that *would* be written to start a game, printed instead of run. The answer to "why does this game not start". |

### Seeing it without the screen

`--render <file.bmp>` draws one frame to an image instead of the
framebuffer, which works while the frontend is running. `--screen`,
`--layout`, `--system`, `--select` and `--find` choose what that frame
shows. Every screenshot in this README was made this way.

```bash
degauss --system PSX --layout tiled --render /tmp/shot.bmp --geometry 352x240
```

### Measuring it

`--bench <frames>` scrolls a folder in memory and reports frame times,
decode cost and cache behaviour. It never touches the framebuffer, so it is
safe to run while the frontend is up. Run it twice and read the second: the
first pays for a cold card.

`--import-favorites <file>` writes a favourite per line of a list, in
MiSTer's own format, for moving a collection over in one go.

## Building it yourself

Every release carries a ready binary, so building is optional.

The dependency tree is pure Rust, so a cross build needs nothing but
rustup: no Docker, no C cross-compiler, no `arm-linux-gnueabihf-gcc`. Rust's
own linker and its self-contained musl do the work, and `.cargo/config.toml`
already selects them.

```bash
rustup target add armv7-unknown-linux-musleabihf
./scripts/build-arm.sh
```

That writes the binary and everything beside it into `deploy/Scripts`, ready
to copy onto the card. It builds on Linux, macOS and Windows alike.

`MiSTer_Degauss` is built from its own repository, which carries the script
that does it. That one is C++ and does need a cross-compiler.

## Licence

Degauss is under PolyForm Noncommercial 1.0.0. See [`LICENSE`](LICENSE).

`MiSTer_Degauss`, shipped alongside it, is a separate program: a fork of
[MiSTer Main](https://github.com/MiSTer-devel/Main_MiSTer) under GPLv3, with
its source at
[giancarloerra/Degauss-Main](https://github.com/giancarloerra/Degauss-Main).

Four typefaces are baked into the binary:

- [DejaVu Sans](https://dejavu-fonts.github.io), under the Bitstream Vera and
  Arev licences ([text](assets/fonts/DejaVuSans-LICENSE.txt)).
- [Px437 DOS/V re. JPN12](https://int10h.org/oldschool-pc-fonts/), from The
  Ultimate Oldschool PC Font Pack, © 2016-2020 VileR, under
  [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/)
  ([text](assets/fonts/Px437-LICENSE.txt)). Unmodified; its glyphs are
  rasterised at fixed sizes.
- [Roboto Condensed Bold](https://github.com/googlefonts/roboto-2) v2.138,
  © Google, under the
  [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0)
  ([text](assets/fonts/RobotoCondensed-LICENSE.txt)). Unmodified.
- [Tamzen 6x12 Bold](https://github.com/sunaku/tamzen-font), © 2011 Suraj
  N. Kurapati, derived from Tamsyn © 2010 Scott Fial, free to use, copy,
  modify and distribute ([text](assets/fonts/Tamzen-LICENSE.txt)).
  Unmodified.

Names are drawn in the Latin, Greek and Cyrillic alphabets, with the accents
and marks of each. A name in Japanese or Chinese draws as a gap: neither
typeface has those characters.
