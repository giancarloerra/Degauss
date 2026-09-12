<p align="center">
  <img src="assets/degauss-logo.png" alt="Degauss" width="360">
</p>

<p align="center"><strong>A blazing-fast, lightweight frontend for MiSTer FPGA.</strong></p>

<p align="center"><strong>Official website:</strong> <a href="https://misterdegauss.com">misterdegauss.com</a></p>

<p align="center">
  <a href="https://discord.com/channels/647909397477195803/1547377865983660072"><img src="https://img.shields.io/badge/Discord-Join%20the%20Degauss%20channel-5865F2?logo=discord&amp;logoColor=white" alt="Join the Degauss Discord channel"></a>
</p>

---

Degauss plays nice with the standard MiSTer setup, folders and scripts, instead of trying to replace it all.

It browses the games and folders on your card as they already are, with artwork and metadata
from EmulationStation gamelists or optional local MiSTer Game Artwork Databases, and builds a
beautiful, fast UI around them. Its optional ScreenScraper tool can add or update gamelist
artwork and metadata when you explicitly start it. It reads the same folders the stock menu
reads, so it agrees with the rest of your setup and scripts by construction.

It runs without background processes and without replacing the stock system. It is optimised for
speed and for CRTs, with several views, custom theming, different fonts, many features for favourites, browsing, lists, random discovery, a screensaver and a full set of options: it works out of the box, and can be tuned to your liking.

Source available, written in Rust and using Slint, and licensed for non-commercial use.

> **Slow exFAT folder scans after the September 7 MiSTer Linux update?**
> The [optional kernel fix guide](https://github.com/giancarloerra/Degauss/blob/e4b5d2a1d9c0e4196d934be8d0f5dcc0296fb2e7/support/kernel-fix/README.md) explains the
> separate installer and restoration procedure. Degauss's normal updater
> does not install or replace the kernel.

> ### Feature request or bug report? Star this repository as well to support it ⭐️
>
> Degauss is written and maintained by one person in his own time. A star is
> the whole of what it costs you and the clearest signal that the work is
> worth continuing! If you want something changed or something is broken,
> [open an issue](../../issues/new/choose) and if you starred it I'll know it matters to you beyond just requesting things :-)

## Screenshots

The Degauss 0.6.0 images below are captured from the actual MiSTer framebuffer,
mainly using the Blue-Yellow GE custom theme, with Standard-theme comparisons
labelled separately. CRT presentation is applied for the README. Older examples
remain labelled by release.

### Current interface (0.6.0)

| | | |
|---|---|---|
| <img src="docs/screenshots/0.6.0/01-home.png" alt="Home with its Capcom custom category image" height="200"> | <img src="docs/screenshots/0.6.0/02-details.png" alt="Details: DoDonPachi III" height="200"> | <img src="docs/screenshots/0.6.0/03-list.png" alt="List View: Arcade Favourites" height="200"> |
| Home, with its Capcom custom category image | Details: DoDonPachi III | List View |
| <img src="docs/screenshots/0.6.0/04-gallery.png" alt="Gallery View: Arcade Favourites" height="200"> | <img src="docs/screenshots/0.6.0/05-multi-list.png" alt="Multi List View: Arcade Favourites" height="200"> | <img src="docs/screenshots/0.6.0/06-carousel.png" alt="Carousel View: Actraiser" height="200"> |
| Gallery View | Multi List View | Carousel: Actraiser |
| <img src="docs/screenshots/0.6.0/07-tiled.png" alt="Tiled View: Amiga Favourites" height="200"> | <img src="docs/screenshots/0.6.0/08-options.png" alt="Extensive Options" height="200"> | <img src="docs/screenshots/0.6.0/09-navigation.png" alt="Navigation and Hold Shortcuts" height="200"> |
| Tiled: Amiga Favourites | Extensive Options | Navigation and Hold Shortcuts |
| <img src="docs/screenshots/0.6.0/10-appearance.png" alt="Appearance" height="200"> | <img src="docs/screenshots/0.6.0/11-library.png" alt="Library: Show Scripts Folder" height="200"> | <img src="docs/screenshots/0.6.0/12-display.png" alt="Display" height="200"> |
| Appearance | Library | Display |
| <img src="docs/screenshots/0.6.0/13-developer.png" alt="Developer" height="200"> | <img src="docs/screenshots/0.6.0/14-actions.png" alt="Grouped Actions" height="200"> | <img src="docs/screenshots/0.6.0/15-random-favourite.png" alt="Random Game and Random Favourite" height="200"> |
| Developer | Grouped Actions | Random Game and Random Favourite |
| <img src="docs/screenshots/0.6.0/16-place-view.png" alt="Per-place View" height="200"> | <img src="docs/screenshots/0.6.0/17-information.png" alt="Game Information: Steep Slope Sliders" height="200"> | <img src="docs/screenshots/0.6.0/32-standard-details.png" alt="Standard Theme: Actraiser Details" height="200"> |
| Per-place View | Game Information: Steep Slope Sliders | Standard Theme: Details |
| <img src="docs/screenshots/0.6.0/18-index-progress.png" alt="Indexing Progress: Running Rebuild" height="200"> | <img src="docs/screenshots/0.6.0/34-index-details.png" alt="Indexing Details: Running Rebuild" height="200"> | <img src="docs/screenshots/0.6.0/19-index-complete.png" alt="Indexing Complete" height="200"> |
| Indexing Progress | Indexing Details | Indexing Complete |

<p align="center">
  <img src="docs/screenshots/0.6.0/29-core-version-retroachievements.png" alt="RetroAchievements &amp; Unstable Cores" height="200"><br>
  <strong>RetroAchievements &amp; Unstable Cores</strong><br>
  <sub>Choose an installed Standard, RetroAchievements, or matching Unstable core for each system, and Degauss remembers that choice for its games and recognised favourites.</sub>
</p>

**Game Artwork Databases:** choose **Automatic**, **Gamelist** or **Artwork Pack**
for each supported system. Automatic is the default and prefers a `gamelist.xml` in any of that system's
library roots; otherwise it looks for a supported pack installed in the default
SD or USB locations. Explicit Gamelist and Artwork Pack choices stay saved until
Automatic is selected again. [How to install and select a pack](#using-mister-game-artwork-databases).

| | |
|---|---|
| <img src="docs/screenshots/0.6.0/28-artwork-source.png" alt="Game Data Source: Automatic, Gamelist or Artwork Pack" height="200"> | <img src="docs/screenshots/0.6.0/36-artwork-pack-location.png" alt="Select an Installed Artwork Pack Location" height="200"> |
| Automatic, Gamelist or Artwork Pack | Choose an Installed Pack Location |

| | | |
|---|---|---|
| <img src="docs/screenshots/0.6.0/20-manual-search.png" alt="Single-game Scraper Controls: Dandy" height="200"> | <img src="docs/screenshots/0.6.0/33-manual-query.png" alt="Manual Search: dandy" height="200"> | <img src="docs/screenshots/0.6.0/24-manual-match.png" alt="Manual Match: 1944 The Loop Master" height="200"> |
| Single-game Scraper Controls: Dandy | Manual Search: dandy | Manual Match: 1944 |

| | |
|---|---|
| <img src="docs/screenshots/0.6.0/21-scraper-progress.png" alt="Scraper Preparation: Arcade" height="200"> | <img src="docs/screenshots/0.6.0/35-scraper-complete.png" alt="Single-game Scrape Complete: 1944" height="200"> |
| Scraper Preparation: Arcade | Scrape Complete: 1944 |
| <img src="docs/screenshots/0.6.0/26-scripts.png" alt="Installed MiSTer Scripts" height="200"> | <img src="docs/screenshots/0.6.0/27-script-confirmation.png" alt="Confirmation Before Running a Script" height="200"> |
| Installed Scripts | Script Confirmation |

### Earlier release examples (0.4.0)

These genuine images retain earlier feature and theme examples. They show the
0.4.0 interface; the palettes and saved custom themes remain supported.

| | | |
|---|---|---|
| <img src="docs/screenshots/0.4.0/07-custom-theme.png" alt="Custom Theme" height="200"> | <img src="docs/screenshots/0.4.0/08-amber-theme.png" alt="Amber Theme" height="200"> | <img src="docs/screenshots/0.4.0/09-blue-orange-theme.png" alt="Blue-Orange Theme" height="200"> |
| Custom Theme | Amber Theme | Blue-Orange Theme |
| <img src="docs/screenshots/0.4.0/10-mono-theme.png" alt="Mono Theme" height="200"> | <img src="docs/screenshots/0.4.0/11-green-mono-theme.png" alt="Green Mono Theme" height="200"> | <img src="docs/screenshots/0.4.0/12-modern-theme.png" alt="Modern Theme" height="200"> |
| Mono Theme | Green Mono Theme | Modern Theme |
| <img src="docs/screenshots/0.4.0/13-neon-theme.png" alt="Neon Theme" height="200"> | <img src="docs/screenshots/0.4.0/16-theme-editor.png" alt="Theme Editor" height="200"> | <img src="docs/screenshots/0.4.0/17-category-system-image-picker.png" alt="Category &amp; System Image Picker" height="200"> |
| Neon Theme | Theme Editor | Category & System Image Picker |

| | |
|---|---|
| <img src="docs/screenshots/0.4.0/18-saved-custom-theme.png" alt="A Saved Custom Theme" height="200"> | <img src="docs/screenshots/0.4.0/26-screensaver-artwork-slideshow.png" alt="Screensaver Artwork Slideshow" height="200"> |
| A Saved Custom Theme | Screensaver Artwork Slideshow |

## Watch Degauss 0.4.0 in action

Earlier design. Same speed.

<p align="center">
  <a href="https://www.youtube.com/watch?v=aFnvkkhbPFY">
    <img src="https://img.youtube.com/vi/aFnvkkhbPFY/maxresdefault.jpg"
         alt="Degauss 0.4.0 in motion" width="640">
  </a>
</p>

Click the image to watch Degauss 0.4.0 on YouTube.

## Table of contents

- [Using it](#using-it)
  - [Running MiSTer scripts](#running-mister-scripts)
- [Why Degauss](#why-degauss)
- [Installing](#installing)
  - [Installing through Update All (recommended)](#installing-through-update-all-recommended)
  - [Installing through Downloader](#installing-through-downloader)
  - [Installing manually](#installing-manually)
  - [Upgrading from an earlier release](#upgrading-from-an-earlier-release)
  - [Returning to Degauss from a running core](#returning-to-degauss-from-a-running-core)
  - [Optional: starting Degauss from the stock menu on-demand](#optional-starting-degauss-from-the-stock-menu-on-demand)
  - [External storage support](#external-storage-support)
- [Views](#views)
- [Settings](#settings)
  - [Themes and colours](#themes-and-colours)
    - [Editing a theme on-device](#editing-a-theme-on-device)
    - [Proposing a theme for an official release](#proposing-a-theme-for-an-official-release)
- [Artwork and metadata](#artwork-and-metadata)
  - [Using MiSTer Game Artwork Databases](#using-mister-game-artwork-databases)
  - [Scraping with ScreenScraper](#scraping-with-screenscraper)
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
| **left / right** | scroll speed, 0.5x to 12x, or what **Left and Right Behaviour** says: letter jumps, page jumps, or plain movement. Inside an Options page, left chooses the previous ordered value and right chooses the next; either direction toggles two-choice values |
| **A** (enter) | open a folder, launch a game; in Options, choose the next value or run an action |
| **B** (escape) | back, out of the folder |
| **X** (tab) | **Actions** for the current selection and location: Game Information, random game, random favourite, keep or drop a favourite, jump to letter, search, hide a row, rebuild this system, change view, etc. With **Hold X (1s) to Add/Remove Fav** enabled, hold X for one second over a game to use the favourite shortcut |
| **Y** (space) | **Menu**: Options, Scripts, Help, About, Exit to MiSTer. Optional **Hold Y (1s) for Random Game** uses Random Game Behaviour while browsing inside a system; a short press still opens Menu |

**Actions** groups the available controls into **Game**, **Find**, **Library**
and **Appearance**. Groups without applicable actions are omitted. **A** opens
a group; **B** returns to the group list, then to browsing. Each selected
control has an explanation below the list. The Categories home screen keeps
its short, flat Actions menu for images and views.

**Game** contains Game Information, separate **Random Game** and **Random
Favourite** actions, and adding/removing favourites. Both random choices use
the currently open folder and **Random Game Behaviour**. **Find** contains
jump, search and hiding controls; **Library** contains scraping, core choice,
data source and rebuilding; **Appearance** contains view and image controls.

A gamepad needs no extra setup and browsing and settings need no keyboard. While Degauss
owns the screen, MiSTer sends the d-pad as arrows and the face buttons as
Enter, Escape, Space and Tab.

### Running MiSTer scripts

Open **Y Menu → Scripts** to browse the installed scripts and their subfolders.
The browser starts at `Scripts` under the configured `menu_root`, normally
`/media/fat/Scripts`. It lists `.sh` files, with folders first. Hidden files
and folders, and Degauss's own `degauss.sh`, are excluded.

Executable `.sh` files run directly, including compiled MiSTer utilities and
scripts with their own interpreter. Non-executable shell scripts run with Bash.

**A** opens a folder or asks for confirmation before running a script.
**B** goes to its parent folder, or returns to Menu at the Scripts root.
Choose **A Run** to execute the selected script or **B Cancel** to leave it
untouched. Only run scripts you trust: they retain their normal access to the
card and may require a keyboard or other interaction.

Degauss exits before the script runs, restoring its terminal rather than
remaining behind it. After completion, failure, or interruption with Ctrl-C,
the terminal keeps the result visible and asks for a key to return. Degauss
then reopens at the same Scripts folder and selection. A script that reboots
or shuts down MiSTer keeps that behaviour.

If a script reloads MiSTer's menu before returning, the new launcher restores
the same script selection. A later Menu reload, after Degauss has already
returned, uses the normal startup screen. Neither path starts a second
frontend. Scripts that load a game core keep that core on screen.

**Options → Library → Show Scripts Folder** is On by default. Switching it Off
hides this Menu entry without changing any script files; the choice is saved
with the other Options settings. Missing or unreadable script folders and
scripts report their underlying error.

## Why Degauss

- **Three pillars.** Performance (for large libraries and images), Simplicity (vs overengineering), Adherence to the MiSTer way (standards, scripts, folders).
- **Fast.** 0.47 s from launch to first frame. 0.57 s to open a folder of
  12,605 games.
- **Artwork instant browsing.** Artwork is read straight from the card as
  you scroll, with nothing pre-generated. Gallery reduces visible images to
  its cell size in memory only. Nothing is written beside the artwork.
- **Nothing resident.** No service, no daemon, no port, no background
  process, nothing at boot. One program, running only while you are
  looking at it.
- **Self-contained.** One program, with no runtime, toolkit or library to
  install beside it, and small footprint while it runs. The optional scraper
  uses the `curl` command already included in a normal MiSTer installation.
  The index Degauss builds
  costs about 300 bytes a game, so it stays in the low megabytes for an
  ordinary collection and is the only thing that grows with the size of
  yours.
- **CRT-optimised.** 352×240 with 1:1 pixel mapping to a 15 kHz analog output.
  Overscan margins and screen position are settings. Larger
  framebuffers are laid out from their own size, so HDMI works too.
- **The card is the truth.** Reorganise, rename or move files with any
  tool and the browser follows: nothing has to be re-imported or re-tagged.
  The index it keeps is only a copy of what the card already says, and
  **Options → Library → Rebuild All System Lists** updates all systems after an update,
  and **Actions → Library → Rebuild This System List** does one system alone.
- **Full metadata.** Read the complete description and available publisher,
  developer, release date, players, language and genre in Game Information,
  from the system's selected Gamelist or Artwork Pack source.
- **Favourites are MiSTer's favourites**, written into `_@Favorites` in
  MiSTer's own format. One made here works in the stock menu; one made
  anywhere else appears here.
- **Awkward systems handled** without hassle: AmigaVision, DOS,
  Neo Geo, Arcade, X68000, and cores that are several machines.

Measured on the DE10-Nano's own hardware, with a large multi-system
collection indexed.

## Installing

> **Slow scans after a MiSTer Linux update:** the September 7, 2026 Linux
> release can make exFAT folder reads much slower. The
> [upstream fix](https://github.com/MiSTer-devel/Linux-Kernel_MiSTer/commit/9854075c86455942c2ce57e0b7dc80e3e2c5b108)
> is merged, but a source-code merge is not a distributed kernel update.
> Degauss's normal updater does not replace the kernel. Any optional kernel
> workaround is separate from installing or updating Degauss. See the
> [optional kernel fix and restoration guide](https://github.com/giancarloerra/Degauss/blob/e4b5d2a1d9c0e4196d934be8d0f5dcc0296fb2e7/support/kernel-fix/README.md).


### Installing through Update All (recommended)

Degauss is available directly in the official
[Update All](https://github.com/theypsilon/Update_All_MiSTer) settings. To
install it and keep it updated:

1. Run **Scripts → update_all** on MiSTer.
2. Press **up** during the opening countdown to enter Settings.
3. Open **Frontends**.
4. Select **Degauss** so it shows **On**.
5. If Update All offers its optional Game Artwork DBs, choose the ones you want
   or select **No**.
6. Return to the main Settings screen, choose **SAVE**, then **EXIT and RUN
   UPDATE ALL**.

<p align="center">
  <img src="docs/screenshots/install/update-all-settings-frontends.png" alt="Update All Settings with Frontends highlighted" width="720">
</p>

<p align="center"><em>Open Frontends in Update All Settings.</em></p>

<p align="center">
  <img src="docs/screenshots/install/update-all-degauss.png" alt="Update All Frontends menu with Degauss enabled" width="720">
</p>

<p align="center"><em>Select Degauss so it shows On.</em></p>

Update All downloads the Degauss files, sets
`main=degauss/MiSTer_Degauss` in `MiSTer.ini`, and keeps the installation
current on later runs. MiSTer can use only one `main=` frontend, so selecting
Degauss switches any other frontend off.

To remove an installation managed by Update All, return to **Settings →
Frontends**, highlight **Degauss**, and choose **Uninstall**.

### Installing through Downloader

If you use MiSTer's Downloader without Update All, add these lines to the
bottom of `/media/fat/downloader.ini`:

```ini
[degauss]
db_url = 'https://github.com/giancarloerra/Degauss/releases/latest/download/degauss.json.zip'
```

Then run `downloader` as usual. Both binaries and the files beside them come
down and stay updated. Add this line to the `[MiSTer]` section of
`/media/fat/MiSTer.ini` once:

```ini
main=degauss/MiSTer_Degauss
```

### Installing manually

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

To remove Degauss, delete the `main=` line and the files above. If a return
shortcut was configured, also delete
`/media/fat/config/degauss/frontend_shortcut.bin` to remove its saved
assignment. Nothing else on the card is touched.

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

### Returning to Degauss from a running core

The recommended installation uses Degauss's `MiSTer_Degauss` Main binary.
While a game core is running, open the OSD and select **System → Frontend**.
Pressing A on **Frontend** keeps the direct route: it closes the current core
and returns to Degauss.

Select **System → Frontend shortcut**, directly below **Frontend**, and press A
to open the optional keyboard shortcut setting. It defaults to **Off**, so an
existing installation keeps its current input behaviour until it is configured.

Select **Keyboard** and press A, then press the physical keyboard key to
capture. Press X on the row to disable it. Menu or B cancels capture.

A configured shortcut works only while a non-menu core is running, the OSD
is unlocked, and no other framebuffer script owns the screen. The setting is
stored in `/media/fat/config/degauss/frontend_shortcut.bin`. A missing file
means the shortcut is Off. An invalid or unsupported file is identified in the
shortcut screen and can be replaced by selecting **Reset shortcut**.

This screen and shortcut belong to `MiSTer_Degauss`. They are not
available when Degauss is launched on demand from the stock Main binary as
described below.

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
full one of 97k+ games. Ordinary folder libraries then reuse their saved lists:
**Options → Library → Rebuild All System Lists** is how you tell Degauss the
card has changed (for example after adding new games). New images and metadata
are read on the fly. Automatic Artwork Pack preparation is described
[below](#using-mister-game-artwork-databases).
Adding games to one system does not need the whole card read again:
**X Actions → Library → Rebuild This System List** inside that system reads just its folders.
Running it from a subfolder still rebuilds the complete containing system, not only that subfolder.
A successful rebuild reflects additions and removals. An unreadable folder
reports its error and keeps that system's previous complete list. A malformed
archive, or a member inside one that MiSTer cannot launch, is skipped with a
warning and the rest of the system is published; the rebuild then finishes as
**Finished With Problems**, naming the system, the archive and the reason
(a system prepared from an Artwork Pack names them in its completion message,
as does a change of its game data source), with every skipped member written
to `/tmp/degauss.log`.

Global and single-system rebuilds show a progress dashboard with the active
system and folder, processed systems, folder/game counts and elapsed time.
**A Details** opens the full scrolling text report; **B Overview** returns to the dashboard.
**B Cancel** requests cancellation while the rebuild is active. Progress with
an unknown total is shown without a percentage; it never performs a second
scan just to count the work. Input and repaint stay responsive, and the
screensaver stays off during indexing. Completion, cancellation and errors
keep their report available until **B Back**. These operation controls remain
visible even when Bottom Bar While Browsing is Off.

### ZIP libraries

ZIP archives open as folders, including their internal subfolders, without extracting
the library. Systems that already launch ZIP files as individual games keep that
behaviour. ZIP64 is supported
within the documented bounds. Metadata for a multi-game archive identifies each game
by its full archive/member path; single-game archives retain legacy archive-level
metadata. A damaged archive is skipped whole; a member MiSTer cannot launch (an
inner archive, an encrypted entry or one using an unsupported compression
method, a legacy-encoded or ambiguous name) is skipped on its own while the
other members stay, as is a folder inside an archive that sits deeper than the
folder depth Degauss walks, with everything under it, and each skip is
reported. See [ZIP libraries](docs/zip-libraries.md) for details and launch
limits.

### RetroAchievements and Unstable cores

Installed RetroAchievements and Unstable cores can be browsed even when there is
no standard version of that core. Degauss discovers these installations; it does
not install the cores or configure a RetroAchievements account.

**X Actions → Library → Core Version** chooses
Default, Standard, RetroAchievements, or a matching Unstable build. The choice is
saved for that system and applies to its games and recognized favourites.
**Use Default Core Version** removes that override. Default follows **Options → Library → Core Preference**
and never automatically chooses an Unstable build. A missing explicitly selected
version produces an error; it does not launch another version.

The **Unstable** group browses installed `_Unstable` cores by their full build names.
RetroAchievements requires a compatible RA installation, including its Main profile.
Degauss releases include a separate RA Main that retains Frontend and the saved
shortcut without being overwritten by the upstream RA updater. Select it in the
RA Main profile as described in the [RA Main installation instructions](support/ra-main/README.md#installation-and-updates).
It uses the same saved Frontend shortcut as normal Degauss Main. Unstable cores
running under Degauss Main already have the Frontend menu and shortcut.

## Views

- **List**: plain text in one column.
- **Details**: the list beside a large picture, with a compact year/players
  and publisher summary. Game Information opens the complete metadata.
- **Tiled**: a grid of pictures with their titles underneath.
- **Carousel**: one large cover with its neighbours either side.
- **Multi list**: two text columns in reading order, showing twice as many entries
  as List. Only the selected long title scrolls.
- **Gallery**: a denser image grid with no permanent captions. The selected
  image has an outline and its title appears above the bottom bar for half a
  second. Entries without artwork remain visible as named text cells.

**Options → Appearance → View** is the global default. Every browse place can instead
keep its own custom view: the Categories screen, each category's Systems
screen, each system root, and every folder inside a system. Use **X Actions →
Appearance → Change View** at that place to create or update its custom view.
**Use Global View**, in the same group, removes only that place's custom view;
it then follows later global
changes again. A custom view remains custom even when it currently matches
the global setting. At the Categories home screen these view controls are
directly in its flat Actions menu.

View changes are saved when leaving the Actions page, before returning to
browsing or opening another action. A save error keeps the menu open with the
underlying problem, so the change is not silently lost after a restart.

For a playable game, **Game Information** is the first entry in **X Actions → Game**
from any browsing view. It shows the title, artwork, description, publisher,
developer, full release date, players, language and available genre. Scroll to
inspect long values and the full description; left/right moves by a page.
**B** or **X** returns to Actions / Game, and backing out restores the same game
without launching it or changing the view. Information comes from the system's
selected Gamelist or Artwork Pack source, including for favourites. Missing
fields remain empty. Details keeps the compact summary; Information is where
all available fields and the complete description can be read.

Complete descriptions are read only when Game Information opens, with a visible
loading or error state. Existing compact caches remain valid; no library rebuild
is required. This does not add description reads to ordinary browsing or indexing.

Folders appear in square brackets with the number of games inside them,
counted through every subfolder, and can sit before the games or after
them. Favourites carry a heart in every view and can be gathered at the
top of their folder.

**Hide This**, in **Actions → Find**, takes any row out of the list: a
game, a folder, or a whole system while you are looking at the system
list. That is separate from the folders and systems left out because they
hold no games at all, which **Show Systems with No Games** governs. **Show What
You Hid** shows the rows you hid without unhiding them, and **Unhide
Everything** puts them all back. These settings are in **Options → Library**.

The screensaver, after the set time, drifts through game images taken from your own
card.

## Settings

Open **Y Menu → Options**, then choose Navigation, Appearance, Library,
Display or Developer. **A** enters a page; **B** returns to the Options
categories. Left and right do nothing on the category list. Each page keeps
its selected row during the current session. Inside a page, left/right adjust
values and **A** adjusts a value or runs the selected action. Reset actions
require **A** and confirmation, never a sideways press.

| Page | Setting | Does |
|---|---|---|
| Navigation | Scroll Speed | How fast a held direction moves through the list. 3x out of the box |
| Navigation | Skip Artwork Faster Than | Above this speed, pictures wait until the list stops. 6x out of the box |
| Navigation | Left and Right Behaviour | What left and right do while browsing: Scroll Speed Change (the default), Letter, Page or Direction. Letter and Page repeat while held; in Direction, left and right move one entry and up and down move a whole row in Tiled, Multi List and Gallery |
| Navigation | Hold X (1s) to Add/Remove Fav | Off by default. Hold X for one second over a game to open the normal favourite-folder chooser, or to remove it when it is already a favourite. The shortcut does nothing in the master Favourites system |
| Navigation | Hold Y (1s) for Random Game | Off by default. Hold Y for one second while browsing inside a system to use Random Game. It follows Random Game Behaviour; a short Y still opens Menu. The hold shortcut is inactive in menus and during operations |
| Navigation | Random Game Behaviour | Whether either random action starts the game, or only moves to it so you can look first |
| Appearance | Theme | Left and right choose a palette. Press A to open the editor. Standard uses the colours in `degauss.toml`. See [Themes and colours](#themes-and-colours) |
| Appearance | View | The global default for places without a custom view: Details, Tiled, Carousel, List, Multi List or Gallery |
| Appearance | Reset All Custom Views | With A and confirmation, remove every place-specific view without changing the global View setting |
| Appearance | Text | The typeface: Smooth, Pixel (a DOS font on whole pixels), and the bolder Smooth 2 and Pixel 2 |
| Appearance | Artwork | Turn pictures off entirely |
| Appearance | Artwork Scale Factor | Framebuffer keeps the original square-pixel fit and is the default. 4:3 and 16:9 correct game artwork for that physical display shape in Details, Tiled, Carousel and Gallery. Category logos, system logos and the screensaver are unchanged |
| Appearance | Bottom Bar While Browsing | On by default. Show the time and button hints while browsing. A saved Off choice stays Off after updating or restarting; menus and operation controls remain visible |
| Appearance | Screensaver | How long with nothing pressed before pictures start |
| Library | Favourites First | Show favourites first in each folder, keeping them in alphabetical order |
| Library | Folders Before Games | On, folders lead a system's listing; off, the games come first |
| Library | Core Preference | Standard First (default) or RetroAchievements First. Used by systems whose Core Version is Default; the other version is used only when the preferred version is absent |
| Library | Show Other Folder | Show the Other group, the cores that are not games |
| Library | Show Utility Folder | Show the Utility group, test patterns and measurement cores |
| Library | Show Unstable Folder | Show installed Unstable cores. On by default |
| Library | Show Scripts Folder | On by default. Show Scripts in Menu to browse and run installed `.sh` files. Turning this Off hides the entry without changing the files |
| Library | Show Systems with No Games | Systems and folders holding nothing are left out on their own; this shows them. Off by default |
| Library | Show What You Hid | Show what you hid yourself with **Hide This** |
| Library | Unhide Everything | Press A and confirm to put back everything you hid yourself, in every folder and every system. Left and right do nothing |
| Library | Rebuild All System Lists | Press A to read the whole card again. Run it after adding games, cores or artwork; **Actions → Library → Rebuild This System List** rebuilds just the containing system. Left and right do nothing |
| Library | Scrape All Systems | Press A to open ScreenScraper settings for all supported systems. Artwork Pack systems are skipped; the master Favourites system is not a scrape target |
| Display | Edge Margin, Sides | Keep this much of each side clear of the bezel |
| Display | Edge Margin, Top and Bottom | The same, vertically |
| Display | Screen Position, Sideways | Nudge the picture, for a screen that sits off centre |
| Display | Screen Position, Up and Down | The same, vertically |
| Developer | Drawing Path | Draw into the screen directly, or into memory first |
| Developer | Performance Readout | Replace the key hints with frame timings |

`degauss.toml` is documentation as much as configuration: every value
explains itself. Changes are written to `settings.toml` beside it when leaving
an Options page, so your changes never overwrite those notes.
If saving fails, the page stays open and a message explains the problem.
Changes remain active for the current session; dismiss the message and press
**B** again after resolving the storage problem to retry saving.
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
three colours. The optional top-level `logo_opacity` is an integer from
`0` to `100`: `0` keeps the original three-colour artwork, `100` uses only
the selected `logo` colour, and values between them blend the two without
making the wordmark transparent. Leave it out and a named `logo` colour
keeps the fully monochrome result used before this setting existed.

The optional top-level `font` sets the theme's default typeface when that
theme is selected. Its values are `smooth`, `pixel`, `smooth 2` and
`pixel 2`. A theme without `font`, or with an unrecognised font, uses the
system-wide **Text** choice. It never inherits the font of the theme selected
before it. An unrecognised value is also reported. After selecting a theme,
**Text** remains an independent option: changing it overrides the current
theme default and that choice continues across restarts.

A theme can be five lines. `themes/Night.toml`:

```toml
font = "pixel 2"
background = "#101318"
accent = "#33FF33"
logo = "#33FF33"
logo_opacity = 80
```

picks a darker ground, a green selection bar and a wordmark blended 80%
towards green from its original colours, and
every other colour shows through from your `[colors]`. Copying the whole
`[colors]` block out of `degauss.toml` and pasting it in, header and
all, is also a valid theme, ready to be edited.

Six themes are available as starting points: `Amber`, `Mono`,
`Blue-Orange`, `Green Mono`, `Modern` and `Neon`. `Green Mono` defaults to
Pixel 2, `Neon` to Pixel, and `Modern` to Smooth 2. The three older file-based
themes do not set a font, so they use the system-wide Text choice. None of the six
names `favorite`, so the hearts keep your own favourite colour under every
palette: red unless you changed it in `[colors]`.

The updater manages the `Amber`, `Mono` and `Blue-Orange` files, so edits
to those files are overwritten on the next update. Copy one under a new
file name to keep your version. `Green Mono`, `Modern` and `Neon` are built into the
program, so an update does not write files with those names. A theme file
on the card with the same name takes precedence.

The card-theme folder is read once at startup, so a file added while Degauss
is running appears after the next start. `Green Mono`, `Modern` and `Neon` remain
available without files because they are built into Degauss. The chosen theme
is remembered by name in `settings.toml`. If that name resolves to neither a
built-in theme nor a valid card-theme file, Degauss uses the standard palette
and a message says so. A same-name card file takes precedence over a built-in;
if that file is broken, Degauss reports it instead of concealing it with the
built-in theme.

#### Editing a theme on-device

Highlight **Theme** in **Options → Appearance** and press A. The editor starts from the
currently selected palette and previews every change immediately.

| Control | Does |
|---|---|
| **up / down** | choose a colour role or control; in the continuous picker, choose the red, green or blue channel; in hexadecimal editing, change the selected digit with an immediate preview |
| **left / right** | choose another starting point, change the theme's Default text, change the selected RGB channel through its smooth gradient in five-unit steps, switch logo colour between Original and Selection, change logo colour mix in five-percent steps, or select a hexadecimal digit |
| **A** | open the continuous colour picker, apply the already-live colour, or activate Save as and other controls |
| **X** | choose another palette role and swap its exact colour with the selected role; switch between the continuous picker and exact hexadecimal editing; in the name grid, delete one character |
| **Y** | restore the colour that was present when the picker opened; in the name grid, clear the complete name |
| **B** | cancel the current edit or leave the editor; changed themes require explicit discard confirmation |

**Save as** opens a controller-operated name grid. Its green checkmark saves
and its red X cancels. Saving writes a complete
`.toml` file into `Scripts/.config/degauss/themes/`, selects it, and stores
its name in `settings.toml`; the default text choice remains part of the theme
file. Names use letters, numbers, spaces, hyphens and
underscores. An existing theme is never overwritten. A save error remains
on screen and the unsaved draft stays in the editor.

A theme saved by this editor also has a **Delete theme** control. Existing
canonical editor-saved themes are recognised too. Deletion requires
confirmation and removes only that selected custom theme; shipped themes do
not expose this control.

#### Proposing a theme for an official release

Start from the complete [`theme-template.toml`](docs/theme-template.toml),
change all ten colour roles, and test the file in Degauss. Put it in
`/media/fat/Scripts/.config/degauss/themes/` under a new file name and restart
Degauss. The file name without `.toml` is the name shown in **Theme** under
Options. The optional top-level `logo` key can colour the wordmark; leave it
out to keep the original three-colour wordmark.

Open a [Theme proposal](../../issues/new?template=theme.yml), paste the complete
TOML, and attach at least one screenshot showing the theme in Degauss. Do not
include private information in the screenshot.

A proposal becomes eligible for release review when at least three distinct
GitHub accounts other than the submitter each add a comment whose complete
voting line is:

```text
Vote: include
```

Reactions, repeated comments from the same account, the submitter's own comment,
and comments with different voting text do not count. Reaching three eligible
comments does not guarantee inclusion. The theme must still parse, keep every UI
role readable, and pass the project checks.

## Artwork and metadata

Degauss reads `gamelist.xml` in the EmulationStation format, in the same
folder as the games. Paths inside it are relative to that folder. Normal
browsing is read-only. Only the optional scraper described below writes a
gamelist, and only after you explicitly start a scrape.

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

### Using MiSTer Game Artwork Databases

Degauss can also read the local [MiSTer Game Artwork Databases](https://github.com/chipster6502/MiSTer_artwork_pack)
installed by MiSTer's Update All and Downloader tools. **Automatic** is the
default when no source choice has been saved. A `gamelist.xml` in any of the
system's library roots keeps the whole system on Gamelist. Otherwise Degauss
uses a valid installed pack from SD, then USB0 through USB7. With neither,
the usual filesystem/Gamelist presentation remains available. Broken or
unreadable candidates are reported, not silently skipped to another source.
Existing saved Artwork Pack locations retain their meaning.

Older settings did not record an explicit Gamelist choice. An existing settings
file without a saved Pack choice therefore uses Automatic, so an installed pack
can now appear when no root `gamelist.xml` exists. Choosing **Gamelist** now saves
that explicit choice. No migration or reset is needed.

Install and update a database through **Update All → Settings → Extra Content
→ Game Artwork DBs**. Update All also chooses its 2D, 3D or mixed artwork
style. Degauss does not download, update, repair, change the style of or
uninstall these databases; it reads whichever complete local style those
tools installed.

Databases can also be downloaded directly from the original
[MiSTer Game Artwork Databases repository](https://github.com/chipster6502/MiSTer_artwork_pack).
Degauss reads the installed database in place rather than importing or copying it.
**Game Data Source** is selected separately for each supported system from its
**X Actions → Library** menu, not from the global Options menu.

To choose a pack manually:

1. Highlight a supported system, or browse inside it, and press **X**.
2. Open **Library → Game Data Source**.
3. Choose **Artwork Pack**.
4. Confirm the detected `docs` location. If more than one installation is
   present, choose the one Degauss should use.

Degauss checks the normal SD, USB and network mount points and the mounts used
by configured game roots. A normal installation is under
`docs/<System>/Artwork`, for example
`/media/fat/docs/SuperGrafx/Artwork` or
`/media/usb0/docs/SuperGrafx/Artwork`. Games and artwork do not have to be on
the same device. A directory browser is available for a valid installation in
another permitted MiSTer storage location; it starts at `/media`, above the
normal SD, USB, network and CIFS mount directories.

The choice applies to the complete system, including all of its game folders.
Neo Geo and Neo Geo MVS share one choice because they use the same library and
database. Favourites have no separate choice: each one follows the current
source of the system that owns its game.

The source menu shows both the saved mode and its effective source. Automatic
is checked off-thread at startup, when reopening a system and when rebuilding
its list. Installing a pack or adding/removing a root gamelist can therefore
change an Automatic choice on the next check, without saving a manual path.
Automatic checks only standard SD/USB locations; the explicit Pack picker
continues to support network and custom locations.

A newly detected pack prepares its system lists as part of indexing, before
the completion report is shown. Its game counts and artwork are then available
to browsing, favourites and the screensaver without opening that system first.
No manual cache reset or extra rebuild is needed. Preparation uses the same
progress, Details and cancellation controls as the rest of indexing.

The two effective sources are deliberately exclusive:

- **Gamelist** is an explicit saved choice, even without XML, and uses the
  existing `gamelist.xml` name, artwork and metadata. It disables automatic
  pack selection until Automatic is chosen again.
- **Artwork Pack** uses only the selected local database for game artwork,
  display name, year, genre, developer, players and description.

If an individual game or field is absent from the database, Degauss keeps the
filesystem-derived game name and leaves that artwork or field empty. It never
fills the gap from the gamelist or ScreenScraper. Publisher and game language
also remain empty because the database format does not provide them. Folder
rows, system logos and category images remain independent of this choice.

MRA entries are matched by their `<setname>`, including MGLs that point to an
MRA. Large embedded hexadecimal ROM, patch and cheat payloads do not impose a
whole-file size limit on that lookup. XML identity metadata remains bounded to
1 MiB; this is separate from artwork image limits.

Selecting an Artwork Pack never edits or removes the existing gamelist or its
media. Choose **Gamelist** again to restore them immediately. While Artwork
Pack is selected, that system's scrape entries in Actions are hidden and
**Scrape All Systems** reports it as skipped before checking scraper login or
making a request.

Degauss rechecks a selected database when the system is entered. After Update
All replaces a style or updates the database, leave and reopen the system to
load the current files. A damaged database is reported as incomplete; a
missing or invalid database remains selected and produces no stale or
gamelist fallback data. Reconnect its storage, repair it through Update All,
or choose **Gamelist**. Games remain browseable and launchable from their
normal filesystem entries.

Only systems with a reviewed database mapping show **Game Data Source**.
Currently supported systems are 3DO, Amiga CD32, Arcade, Atari 2600, Atari
5200, Atari 7800, Atari Lynx, CD-i, ColecoVision, FDS, Game Boy, Game Boy 2P,
Game Boy Color, Super Game Boy, GBA, GBA 2P, Game Gear, Game Gear 2P, Genesis,
Intellivision, Jaguar, Mega CD, Nintendo 64, Neo Geo, Neo Geo MVS, Neo Geo CD,
Neo Geo Pocket, Neo Geo Pocket Color, NES, Odyssey 2, PlayStation, Sega 32X,
SG-1000, Master System, SNES, Saturn, SuperGrafx, TurboGrafx-16,
TurboGrafx-16 CD, Vectrex, Virtual Boy, WonderSwan and WonderSwan Color.

### Scraping with ScreenScraper

Degauss can create or update these same gamelists from
[ScreenScraper.fr](https://www.screenscraper.fr). A free ScreenScraper account
is required. Open **Y Menu → Options → Library → Scrape All Systems** for the whole card, or press
**X** to open **Actions → Library** and choose **Scrape This System**, **Scrape This Folder**
or **Scrape This Game**. The master Favourites shelf is never a scrape target.

Official Degauss binaries already contain the application authorization needed
to contact ScreenScraper. Users enter only their own ScreenScraper username and
password.

Enter the ScreenScraper account username and password, then choose separate
policies for pictures and metadata:

| Setting | Behaviour |
|---|---|
| **Images: Off** | Never downloads or changes artwork. |
| **Images: Missing only** | Keeps the effective `<image>`, `<screenshot>` or `<thumbnail>` when its file exists, and fetches a picture only when artwork is absent or broken. |
| **Images: Replace existing** | Downloads the selected ScreenScraper media and makes it the entry's `<image>`. The previous media file is not overwritten or deleted. |
| **Metadata: Off** | Never changes metadata. |
| **Metadata: Fill missing** | Fills empty fields and preserves every non-empty local or inherited value. |
| **Metadata: Replace existing** | Replaces only fields ScreenScraper actually returned. A missing upstream value never erases a local one. |

Both settings cannot be Off when a scrape starts. The metadata fields are
name, description, publisher, developer, release date, players, genre and
language.

When the selected image exists and every enabled metadata field is populated,
Degauss skips the game before making any ScreenScraper request. Fill missing
still checks all eight fields: a blank language, publisher or other field can
therefore cause another metadata lookup even when the picture and description
are present. A field ScreenScraper does not supply remains blank and may be
retried on a later run; existing artwork is not downloaded again for that
metadata lookup.

Ordinary ROM files are matched by CRC32, MD5 and SHA-1 when they are no more
than 64 MiB. Larger files, archives, `.mgl`, `.mra` and other wrappers use an
exact normalised-title search instead. A unique exact match is applied
automatically. If **Scrape This Game** finds no exact match or more than one,
Degauss shows the available candidates and previews the highlighted game's
artwork. Press **A** to confirm one, **X** to edit the pre-filled search title
and try again, or **B** to leave the game untouched. A search with no suitable
result stays editable so a shorter or alternative title can be tried.

For a direct title search without first running automatic matching, open
**Scrape This Game** and select **Search Manually**, immediately below
**Start Scraping**. It searches the selected game's system using its pre-filled
title. The same candidate picker lets you edit the title, preview a match and
confirm its use. **B** returns to the single-game scraper settings. The account,
password-storage confirmation and chosen Images and Metadata policies still
apply: manual matching does not force replacement of existing data. This action
is available only for an individual game, not folder, system or all-system jobs.

Use Search Manually when an automatic match was wrong. To correct existing
artwork or metadata, set the corresponding Images or Metadata policy to
**Replace existing** before confirming the replacement match.

Folder, system and all-systems scrapes never stop for a match choice. Missing
and ambiguous ScreenScraper matches are counted, skipped without changes, and the remaining
games continue. Unsupported systems are also skipped and counted. If two
systems use the same folder but require different ScreenScraper platform IDs,
the all-systems scrape skips that shared target; a per-system scrape remains
available.

Symlinked copies share a single scrape and gamelist update only when they point
to the same physical file and the same existing gamelist entry; extra paths
are counted as **Linked copies**. Conflicting mappings to different files and
ambiguous entries are reported as errors. During **Checking existing data**,
planning shows known totals and can be cancelled.

During a run, the progress dashboard shows current work, game progress,
written, unchanged, unresolved and failed counts. **A Details** opens a
scrollable report with status, scope, current title, completed/total, written,
unchanged, linked copies, unresolved and skipped/error breakdowns, allowance,
throughput and the last problem. **B Overview** returns without cancelling.
The report retains
worker counts, account limits reported by ScreenScraper and Degauss's allowance
estimate. Another
program using the same account can change the server's counters while the run
is in progress. The estimate covers API lookups, not the separate media-file
transfers. Media transfers obey the download-speed limit reported for the
account. On the running dashboard, press **B**, then confirm, to cancel. Cancellation stops card
enumeration and new requests, and safely finishes installing results that
already completed.

After results are saved, **Refreshing Lists** rebuilds each affected system's
complete list, including after a single-game scrape. This work runs in a
Degauss-owned worker, with the system/folder and read counts shown while the
interface remains responsive. It is not a targeted one-game refresh. The
dashboard shows safe finishing until that work completes; Details remains
available. An archive or member a refresh skips is shown as the last problem,
named by the system, and is not counted as a failed system. No helper or
background service stays running after Degauss exits.

Connection and server failures remain on the progress screen until they are
dismissed. The on-screen message is kept concise; technical curl and HTTP
details are written to `/tmp/degauss.log` without request URLs or login data.
Degauss allows 10 seconds to establish each connection and 60 seconds for an
ordinary API request. An image transfer receives 60 to 300 seconds according
to its size and the account's reported speed. Retryable failures receive up
to three attempts, and the whole operation remains cancellable while waiting.

Downloaded pictures go under `media/screenscraper/` beside the gamelist and
use content-based names. Degauss inspects each gamelist once before network
work and performs at most one replacement per scrape run, after validating
the complete proposed XML with its normal reader. Before changing an existing
gamelist it writes one timestamped
`gamelist.xml.degauss-scraper-*.bak` copy beside it. Backups are never
overwritten or removed, so they can be deleted manually after the result has
been checked. Existing comments, attributes, unknown fields and unrelated
entries are preserved. A malformed or ambiguous gamelist is reported and
left byte-for-byte unchanged. After a failed gamelist write, Degauss removes a
picture created by that run only when the current gamelist can be parsed and
proves the file is unreferenced. If that cannot be established safely, the
content-named file is retained for manual review.

The account password is stored as plain text in `screenscraper.toml` beside
`settings.toml`, because FAT and exFAT cards do not provide private Unix file
permissions. Degauss asks for confirmation before saving it, masks it on
screen, never puts it in logs or process arguments, and provides **Clear saved
login**. External USB and network storage need no special scraper setting:
the same resolved system folders used by the browser determine where each
gamelist and media directory are written.

Choose **Default Image** in Scrape All Systems to set the global artwork type:
Screenshot, Box Art (2D), or Box Art (3D). A system, folder or single-game
scraper instead offers **System Image**, which applies to that entire system.
Its default, **Use Global**, follows Default Image. For example, choose 2D
box art globally and Screenshot for Arcade. Scrape All and manual-match
previews respect each system's choice. Favourites display the artwork of
their original games, so they do not need a separate scraper preference.

Changing the image type does not replace existing pictures under Missing
Only. Choose Images: Replace existing when replacing already downloaded
artwork. If the chosen type is unavailable, Degauss reports missing media
instead of silently selecting another type. Artwork Pack systems remain
excluded from scraping.

Advanced settings can be edited directly in the same `screenscraper.toml`.
They are optional; omitting them keeps the defaults shown here:

```toml
# Omit region to use the account's preferred region.
region = "us"
language = "en"
hash_limit_mib = 64
max_media_mib = 32
media_type = "ss"

[system_media_types]
Arcade = "box-2D"

[system_ids]
"ExactDegaussSystemId" = 123
```

`hash_limit_mib` controls the largest ordinary ROM Degauss will hash (1–4096
MiB); larger and wrapper/disc files use title matching. `max_media_mib` limits
one downloaded picture (1–256 MiB). `media_type` is the ScreenScraper media
type, with `ss` meaning gameplay screenshot. The optional `system_media_types`
table overrides it for individual Degauss system IDs, matched
case-insensitively. Removing an entry restores the global type. Existing
custom media types remain supported in the file. A `system_ids` entry maps a
Degauss system ID, matched case-insensitively, to a positive ScreenScraper
platform ID. It is intended only for a missing or deliberately overridden
platform mapping; an invalid or ambiguous override can associate the wrong
game, so verify both IDs before using it.

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
Favorites.png
```

If no category image exists, Degauss chooses the logo of one of the systems
in that category at random. Lowercase `.png` and `.jpg` extensions are
supported. Favourites shows its heart when `Favorites.png` or `.jpg` is not
present. Restart Degauss after adding or replacing a directly named image.

You can also put additional PNG, JPG or JPEG images directly in this same `logos`
folder, using any filename. On the master Categories screen, press **X** and
choose **Change Image** directly. On a system such as **Computer → Amiga**,
press **X** and choose **Appearance → Change Image**. Then select any
shipped or user-added image in the list. Degauss copies the selection into its
managed image storage and leaves the source file untouched. **Clear Custom
Image** appears for a category or system that has such a selection; it removes
only the managed copy. Categories then return to their normal named image,
random system-logo choice, or Favourites heart. Systems return to their normal
image named after the system ID.

`Arcade.png` is already included as the default fixed image for the Arcade
category.

## Tips

### Making the gamelists

Degauss's built-in ScreenScraper tool can create and update gamelists directly
on MiSTer and is the simplest choice when MiSTer is online. The computer tools
below are alternatives for preparing or curating them while the card is mounted
in a computer, or through a local or network location.

**On a computer, with the card in it.** Any scraper written for
EmulationStation produces exactly the file Degauss reads.

- [Skraper](https://www.skraper.net) is free. It is a .NET application and
  Windows is the only native build; Linux and macOS go through WINE. Set its
  output to RecalBox or RetroPie mode, which is the setting that writes
  `gamelist.xml` rather than a frontend's own database. It already knows
  MiSTer's folder names, so you can point it straight at the card or at a
  share.
- [MiSTer Companion](https://mistercompanion.org/downloads/) includes
  ZapScraper for Windows, Linux and macOS. Select **Recalbox Compatible**, then
  point it at the inserted MiSTer card or a local or network MiSTer location.
  It writes the `gamelist.xml` and media files Degauss reads, with support for
  consoles, handhelds, Arcade and AmigaVision.
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

Degauss reads gamelist.xml files straight from the card. Its built-in scraper
can write them only when the user explicitly starts a scrape; do not edit
them while that progress screen is active.

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
- Degauss checks whether each named picture exists when it reads the system
  list. You still have to check that it is artwork for the correct game.
- Degauss caches what it found. Its built-in scraper refreshes every system
  it changed; after an external edit, rebuild from the menu.
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
**X Actions → Library → Rebuild This System List** while inside it. Artwork is not stored in the
persistent system-list index, so new pictures are read from the card when
their rows are shown. Favourites are shared with
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
| `--audit` | Every system, one line each: games found, artwork bound, folders and any selected Artwork Pack health problem. A Gamelist system with a `gamelist.xml` but no artwork bound, a usable Pack that resolves no pictures, or a system with no games is listed again underneath as a problem, as is an archive, or a member of one, that was skipped, with its reason. A whole card checked without opening a hundred systems by hand. |
| `--list-systems` | Which systems this card actually has, and where each one resolved. The answer to "why is my system missing". |
| `--check-install` | The installation itself, including every saved Artwork Pack root: what is present, missing, broken or left half-migrated. The first thing to run when something looks wrong. |
| `--report` | One system in detail, with `--system <id>`. It identifies Gamelist or Artwork Pack; for a Pack it also reports the selected root, health and the first game's local match method. A skipped archive or member is listed as `unreadable` with its reason. |
| `--dry-run-launch` | The MGL that *would* be written to start a game, printed instead of run. The answer to "why does this game not start". |

### Seeing it without the screen

`--render <file.bmp>` draws one frame to an image instead of the
framebuffer, which works while the frontend is running. `--screen`,
`--layout`, `--system`, `--select` and `--find` choose what that frame
shows. `--layout` accepts `details`, `tiled`, `list`, `carousel`,
`multi-list` and `gallery`. An explicit `--layout` is temporary and takes
precedence over saved global and custom views; without it, render, bench and
selftest use Details. This headless facility is useful for inspection and
diagnostics; it is separate from capturing the live MiSTer framebuffer.

`--screen options` shows the Options categories. Add
`--options-page navigation|appearance|library|display|developer` to render
a settings page directly. `--screen advanced` remains a direct alias for the
Developer page. `--screen actions` shows Actions, with the existing
`--screen context` spelling retained as an alias. `--screen information`
shows Game Information for the playable game chosen with `--system` and
`--select`. `--screen scripts` opens the Scripts browser under the configured
`menu_root`.

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

The MiSTer package script refuses to build unless the application credentials
issued to Degauss are supplied at compile time. Authorized maintainers can put
them in an untracked `screenscraper-developer.toml` at the repository root:

```toml
developer_id = "your_developer_id"
developer_password = "your_developer_password"
```

The same two values can instead be supplied as
`DEGAUSS_SCREENSCRAPER_DEVID` and `DEGAUSS_SCREENSCRAPER_DEVPASSWORD`, or the
file can be selected with `DEGAUSS_SCREENSCRAPER_CREDENTIAL_FILE`. The values
are embedded in the executable; the credentials file is never copied into
`deploy/`.

The repository ignores both that file and the local `screenscraper.toml`
account file. The scraper invokes `curl` for verified HTTPS; normal browsing
and every other feature remain in the static Rust binary.

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
