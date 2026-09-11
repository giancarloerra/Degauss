# Library compatibility hardware checks

Use one supported system and two or three small, known-working games in an isolated test folder. Keep the ordinary game files. Create only small copies for ZIP checks. Exercise artificial large offsets and malformed ZIP64 combinations on the host. An optional full collection check comes after the small checks, using a separate authorized test folder.

Use actual standard, RetroAchievements and nightly cores for the selected system. Dummy RBF files and renamed copies of a standard core do not establish variant behavior.

## Main switching and return controls

Both Degauss Main and RetroAchievements Main restart into the executable selected by the loaded core's INI configuration, retaining the core and MGL arguments. An RA profile can select `degauss/MiSTer_RA_Degauss` while the normal default continues to select Degauss Main. Follow the [profile setup instructions](../../support/ra-main/README.md) to preserve existing overrides and avoid an upstream installer appending a conflicting wildcard profile. The RA launcher must carry the corresponding `setname` and actual RA core path. Sources: [Degauss executable switching](https://github.com/giancarloerra/Degauss-Main/blob/0651979d53f570c29f8772e124ae603297830954/user_io.cpp#L1488), [RA executable switching](https://github.com/odelot/Main_MiSTer/blob/48e32b43ed85b046c7cd41bce756b7bb1679782b/user_io.cpp#L1482), [RA restart arguments](https://github.com/odelot/Main_MiSTer/blob/48e32b43ed85b046c7cd41bce756b7bb1679782b/fpga_io.cpp#L620), [RA launcher/profile construction](https://github.com/sage2050/MiSTer_RetroAchievements/blob/bc19dcab7c855dc55bf61e4025a2591cd9a9a5f5/Scripts/MiSTer_RA.sh#L485-L550).

**For a Degauss Main installation, RA gameplay must retain Frontend and the existing saved shortcut.** Use the separately named [RA Main integration](../../support/ra-main/README.md) with its adjacent CA bundle. Preserve the upstream RA Main and back up any profile being changed. Verify both the Frontend System-menu action and the already configured shortcut. Open shortcut capture, cancel it with the supported controller action, then capture and save the existing shortcut again; verify its configuration bytes remain unchanged. Return must reopen Degauss and allow another ordinary game launch. Upstream RA Main alone lacks these additions: its short System-menu Reboot action loads `menu.rbf`, while holding it requests a cold reboot. Preserve that existing return action as well. Loading the menu core should reselect the default Main, but actual return to Degauss requires a hardware check. Sources: [RA System menu](https://github.com/odelot/Main_MiSTer/blob/48e32b43ed85b046c7cd41bce756b7bb1679782b/menu.cpp#L2938), [RA short Reboot action](https://github.com/odelot/Main_MiSTer/blob/48e32b43ed85b046c7cd41bce756b7bb1679782b/menu.cpp#L3247), [Degauss menu startup](https://github.com/giancarloerra/Degauss-Main/blob/0651979d53f570c29f8772e124ae603297830954/menu.cpp#L1126).

## Prepare a reversible test

1. Record the active Main arrangement and the exact selected system's core/launcher paths. Save original bytes and hashes for files the test will change. Keep credentials and unrelated configuration out of diagnostic output.
2. Preserve standard Main and Degauss Main. Stage RA Main under its separate name, the selected actual RA core, its launcher and only the required profile override. Preserve existing achievement configuration and sound. A one-system check does not require an all-core installer.
3. Obtain the selected actual nightly from its official release and retain its complete filename. Compare downloaded bytes with the release digest before transfer. Reference releases include [RA Main](https://github.com/odelot/Main_MiSTer/releases), [RA NES](https://github.com/odelot/NES_MiSTer/releases) and [NES nightlies](https://github.com/MiSTer-unstable-nightlies/NES_MiSTer/releases/tag/unstable-builds).
4. Transfer through a unique temporary file, verify the complete bytes on the device, then rename into place. Keep an exact record of introduced files for rollback.
5. Use an isolated game directory and disposable Favorites. Never replace a real game library, rewrite existing Favorites or use real Favorites as destructive fixtures.

When the selected system already has every variant installed, exercise variant-only and missing-core cases by temporarily relocating only its relevant core/launcher files outside scanned directories. Keep each original file's name, bytes and destination recorded. Do not rename a different variant to impersonate the missing one. Restore the files after each arrangement.

## Core availability and saved choices

Verify the selected core and running Main, game boot, input, picture, sound and return behavior for each successful launch. A visible menu row, generated MGL or completed FIFO write proves only that stage.

| Arrangement | Checks |
|---|---|
| Standard only | Both global Core preference values use the installed standard core. Existing ordinary launches and Favorites remain unchanged. |
| RA only | The system remains in its normal category. Default selection can launch the installed RA core with either global preference. Return through Frontend and the existing shortcut when using the Degauss Main integration; preserve the upstream short Reboot action. |
| Standard and RA | Standard first and RetroAchievements first select their respective variants. Core Version can explicitly select each independently of the global preference. |
| Nightly only | The nightly remains reachable in Unstable when that group is enabled. An explicit saved nightly choice launches its exact path. |
| Standard alongside nightly | Ordinary default launch continues to use the default selection. An explicit nightly choice selects the full named build. Direct Unstable launch selects that exact row. |
| All selected variants present | Switch among Standard, RetroAchievements and the exact nightly through Core Version. Confirm each launch and return. |
| Saved variant missing | Temporarily remove the specifically selected core. Launch must report that selection as unavailable, without silently choosing a different core. |
| No selected system core | Launch reports the missing core before handoff. Restore one real variant and verify recovery. |

For each explicit per-system choice, leave the system, restart Degauss and verify that Core Version and the next launch retain the exact selection. A nightly choice must keep its complete path/build suffix, including when another nightly is added. Changing one system must not change another system's choice.

Use **Use Default Core Version**, restart again and verify that the saved override is removed and normal default selection applies. This reset must remain available when a previously selected core is missing.

Toggle **Show Unstable folder** off and on, then restart. Verify that visibility persists, build names stay distinct, and normal categories and ordinary game lists remain present. Visibility is a browsing preference; it must not rewrite a saved core path or silently change the chosen variant.

## Favorites and descriptions

Use the same test game through ordinary browsing, an existing Favorite, and a newly created disposable Favorite. Verify metadata, artwork, membership/removal identity, core choice and launch. Launch the newly created ordinary MGL from the stock menu as well. Existing Favorite file bytes must remain unchanged.

Cover a bare target and an absolute target, plus an established parent-relative or custom MGL when available. Include a ZIP-member Favorite from the small library below. If the selected system has companion/reset actions, confirm they survive variant selection and actual launch.

Provide synthetic metadata with a long first paragraph and later paragraphs containing `&amp;quot;`. Verify the first description line remains nonempty and retains the expected 160-character prefix plus truncation marker through source browsing, rebuilt cache, Favorites and restart. Double escaping in a later paragraph must not erase the description. Check the other details and external artwork independently.

## Small ZIP library

Use at most two or three games total, reusing the same permitted game bytes for these arrangements. Keep an uncompressed copy available as the baseline.

1. **Single member:** make a small stored ZIP with one supported game and optional readme. Existing archive-level metadata must still bind. Launch and return using the selected ordinary core.
2. **Multiple members:** make a small deflated ZIP with two supported members, including nested folders without explicit directory records. Use the same basename in two folders to verify distinct identities. Only exact `./archive.zip/folder/game.ext` metadata may bind these games; archive-level or basename-only metadata must not bleed across members.
3. **Navigation and state:** browse root/nested folders, search, favorite, hide/unhide and restart. Verify exact member identity and saved position. Launch a filtered result whose numeric position differs in the unfiltered list; returning must reset search and select the same member. Include distinct Unicode directory names with identical lowercase forms and verify stable order across restart. Launch both root and nested members and a ZIP-member Favorite.
4. **Replacement:** replace the disposable archive with a valid version that removes one member and adds another. Rebuild that system. The new contents replace the old list and unrelated systems remain unchanged.
5. **Stale member:** select a member, then replace the archive with a valid version lacking it before confirmation. Confirm a specific visible failure before handoff. Repeat with the outer archive absent.
6. **Corrupt archive:** truncate a disposable ZIP before rebuild. Expect the archive path and reason, with the previous valid cache and summary retained. Restore a valid archive and rebuild to verify recovery.
7. **Pass-through:** when testing a system already configured to accept `zip`, verify the whole archive remains one game. Do not change production extensions merely to force this case.
8. **Variants:** repeat a small supported ZIP-member launch with RA or a nightly alongside the standard core, and in the relevant variant-only arrangement. Verify the actual variant, not just the unchanged native target text.

ZIP64 parsing, large offsets, high entry counts and malformed record combinations are exercised on the host. The small hardware library verifies native ZIP launch and user-visible behavior. See [ZIP library limits and Main's archive-comment limitation](../zip-libraries.md).

## Large MRA artwork identity

Use an isolated artwork-pack library with two or three MRA/MGL entries and synthetic artwork. No arcade core or ROM archive needs to be installed or launched for this check.

1. Include a small descriptor and a valid descriptor larger than 1 MiB containing embedded hexadecimal payload. Cover both early and late `<setname>` positions and an MGL referencing the large descriptor.
2. Rebuild through the actual Artwork Pack UI. Verify names and distinct artwork, then restart and verify the saved cache.
3. Independently include one valid image larger than 1 MiB. Confirm it renders so image size is not confused with descriptor size.
4. Test the shared path in another mapped system using an MRA entry. Ordinary Gamelist browsing must remain unchanged.
5. Introduce a malformed descriptor before its identity, rebuild and verify an explicit failure with the previous complete cache and summary unchanged. Restore it and rebuild again.
6. Exercise cancellation while a descriptor is still being read, using a controlled slow reader in the isolated test folder if necessary. Verify that the reader actually started, cancellation closes it, the UI reports cancellation, and the previous cache remains unchanged. Restore a regular descriptor and rebuild successfully. A read that completes before cancellation is not a cancellation test. Host tests additionally cover native `part`, `patch` and `cheat` payloads, comma separators, small read boundaries, metadata limits and underlying read failures.

## Installation modes and restoration

Repeat the applicable launch/return sequence for Scripts-menu use and the Degauss Main installation. For the Degauss Main installation, verify Frontend and the existing saved shortcut from both RA and nightly gameplay. For Scripts-menu use, preserve its documented return controls. Also exercise the RA short Reboot action. Verify which executable is active after returning and that the next ordinary launch works.

RA's monitor-only operation without credentials can exercise core loading and executable switching. Verify certificate validation through the installed executable and adjacent CA bundle before credentials are used. Achievement login, game recognition and submission are separate account-dependent checks and must be recorded separately. The pinned RA Main uses its documented username/password configuration, not a web API key. [RA initialization](https://github.com/odelot/Main_MiSTer/blob/48e32b43ed85b046c7cd41bce756b7bb1679782b/achievements.cpp#L1666).

Restore saved configuration bytes and temporarily relocated cores. Remove only disposable Favorites, test libraries and other files introduced for this test. Recheck the original Main/core hashes and ordinary launch, return, metadata and Favorites behavior. Record each tested arrangement and any untested boundary without treating missing observation as a pass.

## Neo Geo ROM sets

Use an isolated Neo Geo folder with a synthetic `romsets.xml` of a few entries: one plain entry, one with a comma-separated alias pair, one carrying `hide`, and one without an `altname`. Keep the real catalogue and library untouched; small disposable copies of two or three permitted sets are enough, and the listing checks need no real ROM bytes at all.

1. **Zipped sets:** place `<setname>.zip` files for the plain entry and for both names of the alias pair. Each is one game, once, titled by the catalogue, with the second alias shown as `Title (alias)`. The ZIP must not open as an archive. Launch a real set and return.
2. **Unzipped sets:** repeat with `<setname>/` folders holding raw components. Each folder is one game and is never entered. Launch a real set and return.
3. **Sub-folder and own catalogue:** put sets inside an organisational sub-folder, which must remain navigable, and give one sub-folder its own `romsets.xml`; that folder must answer to its own file and not to the top-level one.
4. **Own `romset.xml`:** a folder carrying a valid `romset.xml` and no catalogue entry is a game titled by that file, including when the catalogue hides its name.
5. **Aliases, hidden and malformed:** name one set in a different case from its entry; it must still match. The hidden entry's ZIP and folder must not be listed, and a folder holding only hidden sets is empty. Break a copy of `romsets.xml` under a separate sub-folder: `--report` and `--audit` must name it, its ZIPs and folders fall back to archives and folders, and every `.neo` and `.mgl` beside them stays listed.
6. **Standalone and unrelated entries:** keep `.neo` and `.mgl` games beside the sets; their names and launches are unchanged. Add an unrelated ZIP and an ordinary folder; neither becomes a game.
7. **Metadata and artwork:** bind gamelist records to `./setname.zip`, `./setname` and a `.neo`, then repeat with an Artwork Pack and a custom image. Set artwork must resolve by set name without the frontend opening any ZIP.
8. **Favourites:** favourite one set of each format, restart, check the hearts, launch each favourite, and open the same favourite from the stock menu.
9. **Shared folder:** open the same folder through Neo Geo and Neo Geo MVS; both list the same rows and neither damages the other's cache or state.
10. **Storage and upgrade:** repeat the listing checks from USB and a mounted network path when available, and verify a 0.6.0 installation keeps its settings, positions and favourites after a single rebuild of the Neo Geo list.
