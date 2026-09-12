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

Cover every supported path form: an absolute target as the stock script writes it, a root-relative target as Degauss writes it, a bare target and a `./` or `../` target written by hand, and a linked core file. Relative targets are read the way MiSTer Main reads them, under the core's games folder (`games/<setname>` when the MGL carries a `<setname>` without `same_dir="1"`) in the storage search order, never beside the MGL; verify a decoy file beside the MGL is ignored and that a custom MGL still launches directly and unchanged. Place one bare target in two of the system's alias folders at once and verify the Favorite is listed and removable, receives no artwork, shows the ambiguity in Game Information, is named with both candidate paths in `/tmp/degauss.log`, and is refused at launch with that line rather than pointed at either file; remove one of the two files and verify it launches. Include a ZIP-member Favorite from the small library below. If the selected system has companion/reset actions, confirm they survive variant selection and actual launch.

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
2. Prepare through the actual Artwork Pack UI: under Automatic, with no `gamelist.xml` under any of the system's folders, enter the system and answer **Prepare** to the Artwork Pack Available question; with an explicit Artwork Pack choice made from a clean state instead, preparation starts without a question. Verify names and distinct artwork, then restart and verify the saved cache is reused without a preparation overlay.
3. Independently include one valid image larger than 1 MiB. Confirm it renders so image size is not confused with descriptor size.
4. Test the shared path in another mapped system using an MRA entry. Ordinary Gamelist browsing must remain unchanged.
5. Introduce a malformed descriptor before its identity and rebuild with **Actions → Library → Rebuild This System List** (every rebuild in this section, unless it names Rebuild All). Verify the preparation completes with problems, that the malformed descriptor's game stays listed with its filesystem name and no Pack data, that `/tmp/degauss.log` names it under `pack entry`, and that the summary and every other entry are unchanged. Restore it and rebuild again to verify it is matched.
6. Include a multi-component core descriptor for a core outside the systems table: an MGL under `_Arcade` naming a `<setname>` and several bare files, with those files under `games/<setname>` and decoy files of the same names beside the MGL. Verify the Pack match uses the descriptor's own name, that `--report` shows no failed match and `/tmp/degauss.log` no `MGL component` line, and that the descriptor launches directly. Remove one file from `games/<setname>` and verify the report names that file and folder, not the decoy beside the MGL.
7. Exercise cancellation while a descriptor is still being read, using a controlled slow reader in the isolated test folder if necessary. Verify that the reader actually started, cancellation closes it, the UI reports cancellation, and the previous cache remains unchanged. Restore a regular descriptor and rebuild successfully. A read that completes before cancellation is not a cancellation test. Host tests additionally cover native `part`, `patch` and `cheat` payloads, comma separators, small read boundaries, metadata limits and underlying read failures.
8. Disconnect the database location and rebuild the system: verify the rebuild fails as a whole, the previous complete cache and its three files under `cache/artwork-pack` are byte-identical, and the previous mapping is still shown. Reconnect, then replace the `cache/artwork-pack` folder with a regular file and rebuild: verify the same terminal failure with nothing installed. Restore the folder and rebuild successfully. Neither case may be reported as a skipped entry.
9. With the system accepted under Automatic, add one descriptor and run **Options → Library → Rebuild All System Lists**: verify no Pack preparation runs for it inside the rebuild, that `<id>.source.bin` and `<id>.prepared.bin` under `cache/artwork-pack` are byte-identical and `<id>.bin` is rewritten, and that the next entry asks **Artwork Pack Changed**. Answer **Keep Current** and verify the previous entries keep their Pack data, the added one has none, and the next entry does not ask; then **Rebuild This System List** and verify the added one is matched.

## Installation modes and restoration

Repeat the applicable launch/return sequence for Scripts-menu use and the Degauss Main installation. For the Degauss Main installation, verify Frontend and the existing saved shortcut from both RA and nightly gameplay. For Scripts-menu use, preserve its documented return controls. Also exercise the RA short Reboot action. Verify which executable is active after returning and that the next ordinary launch works.

RA's monitor-only operation without credentials can exercise core loading and executable switching. Verify certificate validation through the installed executable and adjacent CA bundle before credentials are used. Achievement login, game recognition and submission are separate account-dependent checks and must be recorded separately. The pinned RA Main uses its documented username/password configuration, not a web API key. [RA initialization](https://github.com/odelot/Main_MiSTer/blob/48e32b43ed85b046c7cd41bce756b7bb1679782b/achievements.cpp#L1666).

Restore saved configuration bytes and temporarily relocated cores. Remove only disposable Favorites, test libraries and other files introduced for this test. Recheck the original Main/core hashes and ordinary launch, return, metadata and Favorites behavior. Record each tested arrangement and any untested boundary without treating missing observation as a pass.
