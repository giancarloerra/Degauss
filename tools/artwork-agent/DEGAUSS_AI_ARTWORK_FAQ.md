# FAQ: Managing Degauss artwork with an AI coding agent

Degauss reads EmulationStation-compatible `gamelist.xml` files and image files directly from the MiSTer card. An AI coding agent can audit and maintain those files over SSH without using Degauss's interactive scraper. The card remains the source of truth.

The safe sequence is:

1. Audit the live card without changing it.
2. Classify genuine gaps.
3. Find exact artwork candidates.
4. Review every candidate.
5. Pin the approved scope in a manifest.
6. Perform a dry run.
7. Write through verified temporary files.
8. Re-read and validate the result.
9. Rebuild the affected Degauss system list from the Degauss menu.

Never let an agent modify a gamelist or image merely because a filename looks similar.

## What this can do

- Audit every Degauss system and report real artwork coverage.
- Prioritise systems represented in MiSTer's `_@Favorites` folder.
- Fill only the gaps introduced by the latest Update All run.
- Repair a missing entry, an empty artwork field, or a broken image reference.
- Reuse one exact image for genuine regional or organised variants of the same game.
- Build a new `gamelist.xml` from scratch without using a scraper.
- Leave uncertain games unchanged and produce an explicit unresolved report.

## Requirements

- Degauss installed on the MiSTer.
- Passwordless SSH access to the MiSTer from the computer running the agent.
- Python 3 on that computer.
- Pillow for full PNG and JPEG validation when images are downloaded.
- Enough free card space for the images, temporary files, and gamelist backups.
- No Update All, downloader, scraper, or other gamelist writer running during a write.

External artwork lookup is optional. Direct ScreenScraper API access requires legitimate ScreenScraper developer credentials as well as the user's ScreenScraper account. Do not extract or reuse credentials bundled with another application. Without authorised API credentials, the agent can still audit, reuse exact local artwork, build gamelists, and use an explicitly approved public collection.

## Setting up passwordless SSH

Run every command in this section on the computer that will run the agent. Replace `<MISTER_IP>` with the MiSTer's current IP address. The initial key installation asks for the MiSTer root password once. Never put that password in a command, script, configuration file, or agent prompt.

### 1. Create a dedicated SSH key if one does not already exist

Check first:

```bash
test -f "$HOME/.ssh/id_ed25519" && test -f "$HOME/.ssh/id_ed25519.pub" && echo "key exists"
```

If `key exists` is not printed, create the key:

```bash
ssh-keygen -t ed25519 -f "$HOME/.ssh/id_ed25519"
```

Protect the private key:

```bash
chmod 700 "$HOME/.ssh"
chmod 600 "$HOME/.ssh/id_ed25519"
chmod 644 "$HOME/.ssh/id_ed25519.pub"
```

Never copy, display, transmit, or ask an agent to read the private key.

### 2. Install only the public key on the MiSTer

```bash
cat "$HOME/.ssh/id_ed25519.pub" | \
  ssh root@<MISTER_IP> \
  'umask 077; mkdir -p /root/.ssh /media/fat/config; touch /root/.ssh/authorized_keys /media/fat/config/authorized_keys; key=$(cat); for file in /root/.ssh/authorized_keys /media/fat/config/authorized_keys; do grep -qxF "$key" "$file" || printf "%s\n" "$key" >> "$file"; done; chown root:root /root/.ssh /root/.ssh/authorized_keys; chmod 700 /root/.ssh; chmod 600 /root/.ssh/authorized_keys; sync'
```

Enter the MiSTer root password when SSH asks for it. This command adds the public key only and avoids adding a duplicate line. It installs the active copy under `/root/.ssh` and a persistent copy at `/media/fat/config/authorized_keys` so a MiSTer Linux update can restore it.

### 3. Add a stable local SSH alias

Add this block to `$HOME/.ssh/config` on the computer:

```sshconfig
Host mister
  HostName <MISTER_IP>
  User root
  IdentityFile ~/.ssh/id_ed25519
  IdentitiesOnly yes
```

Then protect the file:

```bash
chmod 600 "$HOME/.ssh/config"
```

If the MiSTer receives a changing address from the router, reserve its current address in the router or update `HostName` when it changes.

### 4. Verify key authentication from the computer

```bash
ssh -o BatchMode=yes mister true && echo "key works"
```

Success requires the literal `key works` output. If the command asks for a password or reports an authentication error, stop and repair SSH before giving the agent card-write work.

After this succeeds, the remaining examples can use `ssh mister ...`.

### 5. Restore the active key after a MiSTer Linux update

A Linux or root-filesystem update can remove `/root/.ssh/authorized_keys` while leaving the persistent FAT-partition copy intact. If batch-mode authentication stops working, connect from the computer with password authentication:

```bash
ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no mister
```

Enter the current MiSTer password only at the interactive prompt. At the MiSTer shell, run:

```sh
test -s /media/fat/config/authorized_keys || exit 1
mkdir -p /root/.ssh
cp /media/fat/config/authorized_keys /root/.ssh/authorized_keys
chown root:root /root/.ssh /root/.ssh/authorized_keys
chmod 700 /root/.ssh
chmod 600 /root/.ssh/authorized_keys
sync
exit
```

Back on the computer, verify again:

```bash
ssh -o BatchMode=yes mister true && echo "key works"
```

Do not replace or expose the private key during recovery.

## Is this ready to give to an agent?

Yes, when the complete kit is supplied. The distributable kit contains:

- `DEGAUSS_AI_ARTWORK_FAQ.md`
- `README.md`
- `arcade_artwork_audit.py`
- `arcade_artwork_source.py`
- `degauss_artwork_apply.py`
- `test_arcade_artwork_audit.py`
- `test_arcade_artwork_source.py`
- `test_degauss_artwork_apply.py`
- `requirements.txt`
- `LICENSE`

The scripts contain no user or developer credentials. The ScreenScraper helper reads a separately protected developer TOML selected through `--developer-config` or `SCREENSCRAPER_DEVELOPER_CONFIG`, and reads the user's own protected Degauss ScreenScraper configuration on the MiSTer. Neither credential file belongs in the kit.

The Arcade audit and source tools are intentionally limited to Arcade MRAs named by the latest Update All log. `degauss_artwork_apply.py` is the generic final-write tool for any Degauss system. It accepts only an explicit, hash-pinned, reviewed manifest; its default mode is a dry run. Discovery and matching remain system-specific and must be completed before preparing that manifest.

Before use, install Pillow and run the bundled tests:

```bash
python3 -m pip install -r requirements.txt
python3 -m unittest -v test_arcade_artwork_audit.py test_arcade_artwork_source.py test_degauss_artwork_apply.py
```

## Degauss's artwork data model

Each library root can contain one `gamelist.xml`. Paths inside that file are relative to that library root.

```xml
<gameList>
  <game>
    <path>./Boulder Dash.d64</path>
    <name>Boulder Dash</name>
    <screenshot>./media/screenshot/Boulder Dash.png</screenshot>
  </game>
</gameList>
```

Degauss accepts these artwork fields in this order:

1. `<image>`
2. `<screenshot>`
3. `<thumbnail>`

The first present field wins. The referenced file must still exist and be a valid image.

Entries can inherit metadata and artwork from a parent:

```xml
<game id="parent-game">
  <name>Example Game</name>
  <screenshot>./media/screenshot/example.png</screenshot>
</game>
<game parentid="parent-game">
  <path>./Example Game (Europe).rom</path>
</game>
```

A child's own field overrides the corresponding parent field. An audit must resolve inheritance field by field before declaring the child incomplete.

Current Degauss gamelist matching relevant to this workflow checks:

1. Exact relative path.
2. Filename.
3. Filename stem.
4. Normalised `.slug` candidates.

`.slug` entries are virtual keys, not files that must exist on the card. An exact path entry should take precedence over a broad filename, stem, or slug match.

Some systems use multiple roots, and some systems share a root and gamelist. Run `--list-systems` on the live card rather than assuming a folder. Examples include PC DOS, Genesis, Neo Geo CD, and SG-1000.

## Importing an existing EmulationStation collection

An existing `gamelist.xml` and its media can usually be reused. Do not copy the XML blindly. The source paths describe the source frontend's filesystem, while Degauss must resolve each game and artwork path from the corresponding live MiSTer library root.

Common source frontends include:

| Source | What can be reused | Required conversion |
|---|---|---|
| Batocera | EmulationStation `gamelist.xml`, image references, and media files | Rebase each game and artwork path to the live MiSTer system root |
| RetroPie | EmulationStation metadata, image references, and downloaded media | Locate the actual source gamelist and media directories, then rebase paths |
| Recalbox | EmulationStation `gamelist.xml` and referenced images | Rebase paths and retain only entries that map to live MiSTer games |
| EmuELEC | EmulationStation `gamelist.xml` and referenced images | Rebase paths and validate supported fields and files |
| ES-DE | Metadata from `gamelist.xml` and media from `downloaded_media` | Current ES-DE normally matches media by filename instead of storing image paths in the gamelist, so pair the media with each game and add explicit Degauss artwork references |

Relevant source documentation:

- [ES-DE user guide](https://gitlab.com/es-de/emulationstation-de/-/blob/master/USERGUIDE.md)
- [Batocera EmulationStation gamelist format](https://github.com/batocera-linux/batocera-emulationstation/blob/master/GAMELISTS.md)
- [RetroPie EmulationStation documentation](https://github.com/RetroPie/RetroPie-Docs/blob/master/docs/EmulationStation.md)
- [Recalbox EmulationStation gamelist format](https://github.com/recalbox/recalbox-emulationstation/blob/master/GAMELISTS.md)
- [EmuELEC EmulationStation gamelist format](https://github.com/EmuELEC/emuelec-emulationstation/blob/EmuELEC/GAMELISTS.md)

The import sequence is:

1. Run Degauss `--list-systems` and `--report --system` to identify the exact target roots and browse rules.
2. Copy the source `gamelist.xml` and media to a local staging directory. Do not upload them yet.
3. Parse the source XML. Resolve parent inheritance and the first present artwork field in Degauss order: `image`, `screenshot`, then `thumbnail`.
4. Inventory the target MiSTer system through its real Degauss rules. Do not assume every source ROM exists or that every card file is a game.
5. Match source games to target games by exact relative path first, then exact filename, exact stem, or a verified normalised title key. Preserve region, revision, edition, disc, and volume distinctions.
6. For ES-DE, match media according to ES-DE's documented game-relative filename rules. Do not expect current ES-DE gamelists to contain image tags.
7. Fully decode and visually inspect each selected source image. Existing covers, screenshots, title screens, and miximages are different media types. Choose deliberately rather than silently substituting one for another.
8. Copy approved images below the target MiSTer library root, normally under `media/screenshot/`, using collision-safe names.
9. Write relative Degauss references such as `./media/screenshot/Game.png`. Do not retain absolute source-computer paths, home-directory paths, or paths that escape the target library root.
10. Preserve compatible metadata and existing parent relationships. Degauss uses `name`, `image`, `screenshot`, `thumbnail`, `genre`, `favorite`, `desc`, `publisher`, `developer`, `releasedate`, `players`, and `lang`. Unrecognised source fields may remain only when a targeted edit can preserve them safely.
11. Present a hash-pinned import manifest and dry run before writing.
12. Apply the normal backup, unique `.part`, size/hash, atomic rename, `sync`, final reread, XML parse, and Degauss report checks.

An imported image is not valid merely because its source XML points to it. The source file must exist, decode successfully, show the correct game, and remain inside the copied target media tree.

### Copyable collection-import brief

```text
Import this existing EmulationStation-compatible collection into Degauss. Read DEGAUSS_AI_ARTWORK_FAQ.md and the current Degauss README first. Work from the live MiSTer --list-systems and --report output, not assumed folder names.

Stage the source gamelist.xml and artwork locally. Parse parent inheritance and resolve image, screenshot, then thumbnail. If the source is current ES-DE, also inspect downloaded_media and use ES-DE's filename-matching rules because its gamelist may contain no media paths.

Map each source entry to a real target game by exact relative path, exact filename, exact stem, or a verified normalised title. Do not merge regions, revisions, discs, editions, sequels, or volumes unless they are proven to be the same visual game. Fully decode and visually inspect every selected image.

Prepare a manifest containing the source and target paths, image source, size, SHA-256, intended relative Degauss artwork path, operation type, and expected count. Do not write until the manifest is reviewed and approved.

For an approved write, confirm no updater or scraper is active. Back up the live gamelist, copy through unique .part files, verify size and SHA-256, atomically rename, sync, and reread. Preserve existing XML and parent relationships. Parse the final XML, validate every reference, run the Degauss report, and state which system list needs rebuilding. Never upload the source collection or local inventory to an external service.
```

## Applying a reviewed manifest to any system

`degauss_artwork_apply.py` is the bundled final-write tool for a reviewed system-specific manifest. It does not discover games, choose images, or contact an artwork service. Its default mode is read-only, and it refuses a changed gamelist, a changed source image, an unsafe path, a duplicate operation, an active updater or scraper, and an unexpected existing destination image.

Each operation either references an image already below the live system root or uploads one locally reviewed image:

```json
{
  "version": 1,
  "system": "Example System",
  "root": "/media/fat/games/Example",
  "gamelist_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "expected_operations": 1,
  "operations": [
    {
      "path": "./Example Game.rom",
      "name": "Example Game",
      "artwork": "./media/screenshot/Example Game.png",
      "image": {
        "kind": "local",
        "path": "./reviewed/Example Game.png",
        "size": 123456,
        "sha256": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        "source": "Recorded public source or existing collection identity"
      }
    }
  ]
}
```

For an existing image already under the system root, set `kind` to `existing` and set `image.path` to the same relative path used by `artwork`. Its size and SHA-256 remain mandatory.

Run the pinned dry run, inspect every listed game and image target, then apply the unchanged manifest:

```bash
python3 degauss_artwork_apply.py reviewed-manifest.json
python3 degauss_artwork_apply.py reviewed-manifest.json --apply
```

The apply step verifies each source, uploads new images through unique temporary files, creates a timestamped gamelist backup, changes only the targeted game fragments or appends the approved entries, atomically replaces the gamelist, and rereads every resulting reference. Run Degauss's report afterward and rebuild only the affected system list from Degauss.

## First step: use Degauss's own audit

Run the read-only audit through the installed launcher:

```bash
ssh mister '/media/fat/Scripts/degauss.sh --audit'
```

Useful companion commands:

```bash
ssh mister '/media/fat/Scripts/degauss.sh --list-systems'
ssh mister '/media/fat/Scripts/degauss.sh --report --system "<system-id>"'
```

`--audit` uses Degauss's actual browse and artwork rules. Its numbers are browse rows, not necessarily unique commercial games. A collection can include:

- Regional versions.
- Revisions and prototypes.
- Hacks, translations, demos, and homebrew.
- Multiple file formats for one title.
- Support files and explicitly labelled non-games.
- Backing disk images that are also exposed as browsable rows.
- Virtual games inside an archive or disk image.

Do not send the complete inventory to a third party. Only public game titles and public platform identifiers are needed for an external artwork query.

## Prioritising MiSTer Favorites correctly

Do not count the `_@Favorites` directory as a normal system. Its launchers usually have no artwork of their own. They point to games owned by other systems.

Resolve every favourite to its actual target:

- Follow MRA symlinks to the real Arcade MRA.
- Parse each MGL and read its core and game path.
- Resolve relative paths from the MGL's real location.
- Map the resolved target to Degauss's live system roots.
- Do not trust the Favorites subfolder name. A launcher can be filed under the wrong heading.

After resolving the actual systems, audit each of those systems completely. This gives useful priority without mistaking launcher rows for missing screenshots.

## Classifying a gap

Every reported row should end in exactly one category:

| Category | Meaning | Action |
|---|---|---|
| Valid | The effective artwork field resolves to a valid image for the correct game | Keep it |
| Missing entry | No gamelist entry matches the browsed item | Add a minimal exact-path entry after approval |
| Empty artwork | An exact entry exists but has no inherited or local artwork | Add one artwork field after approval |
| Broken reference | The field exists but the target is missing or invalid | Repair only after identifying the intended image |
| Inherited | The child has no field but inherits valid art from a parent | Keep it |
| Virtual | The entry is a `.slug`, archive member, or another non-file game key | Validate with that system's actual browse rules |
| Shared variant | A regional or organised variant is genuinely the same visual game | It may share reviewed artwork |
| Distinct edition | A sequel, volume, edition, conversion, or materially different release | Find separate artwork |
| Non-game/support | BIOS, browser, cleaning disc, test tool, backing disk, or other support item | Report separately; do not scrape blindly |
| Unresolved | Identity or image cannot be proved | Leave unchanged |

An image file existing on the card does not prove it is correct. Decode it fully and inspect it.

## Reference automation used for a scoped Arcade update

The reference workflow separates read/write auditing from external candidate discovery:

- `arcade_artwork_audit.py` reads the live card, mirrors Degauss matching, checks image references, validates an approved manifest, performs a dry run, and applies only that pinned manifest.
- `arcade_artwork_source.py` groups genuine title gaps, obtains candidate screenshots, validates downloads, records hashes and sources, and creates a manifest from reviewed approvals. It never changes the card.

These bundled scripts are intentionally scoped to Arcade MRAs named by the latest Update All downloader log, plus organised MRA symlinks that resolve to those exact MRAs. They are not a generic whole-card scraper.

### 1. Audit the current Update All Arcade scope

```bash
python3 arcade_artwork_audit.py
python3 arcade_artwork_audit.py --json > audit.json
```

The audit records:

- The SHA-256 of the downloader log snapshot.
- The SHA-256 of the live Arcade `gamelist.xml`.
- Every changed MRA and its real target.
- Organised symlink copies of only those MRAs.
- Exact, filename, stem, and slug matches.
- Parent-inherited artwork.
- Existing candidate images on the card.
- Broken or missing image references.

### 2. List public titles that need candidates

```bash
python3 arcade_artwork_source.py --folder ./artwork-candidates
```

This is read-only. It groups only genuinely identical titles. Different editions and volumes retain separate groups.

### 3. Query ScreenScraper, if authorised

```bash
python3 arcade_artwork_source.py \
  --folder ./artwork-candidates \
  --fetch \
  --limit 10
```

The reference source script:

- Reads developer and user credentials from protected files inside the process.
- Never places credentials in shell arguments.
- Never prints an authenticated URL.
- Sends only the public title, public platform ID, and a public application label.
- Requires an exact normalised title match.
- Rejects results that resolve to multiple game IDs.
- Accepts only HTTPS media hosted by ScreenScraper.
- Prefers world, US, Europe, then Japan screenshots when several exact screenshots exist.
- Limits response and image sizes.
- Fully decodes the image with Pillow.
- Accepts PNG or JPEG only.
- Records the source, byte size, and SHA-256.

The user or agent must visually inspect every candidate. A technically valid image can still show the wrong game, wrong edition, title screen, black frame, border, or unrelated content.

### 4. Use the explicit Arcade fallback only after a recorded miss

For Arcade only, the reference workflow can try the public Libretro MAME 2010 Named Snaps source by exact safe set name:

```bash
python3 arcade_artwork_source.py \
  --folder ./artwork-candidates \
  --libretro \
  --limit 10
```

This command is allowed only after the ScreenScraper attempt has recorded `no_exact_title`, `ambiguous_exact_title`, or `no_screenshot`. It must not hide an API, credential, TLS, or network failure.

Source details:

| Source | Used for | Matching boundary |
|---|---|---|
| [ScreenScraper](https://www.screenscraper.fr/) | Exact screenshots for supported console, computer, and Arcade platforms through its official API | Public title plus platform; exact returned identity; authorised user and developer credentials |
| [Libretro MAME 2010 thumbnail sources](https://github.com/libretro-thumbnails/mame2010-thumbnail-sources) | Arcade `Named_Snaps` when ScreenScraper records an exact-title miss, ambiguity, or no screenshot | Exact safe MAME set name only |
| [Libretro thumbnail repositories](https://github.com/libretro-thumbnails/libretro-thumbnails) | Locating the public system-specific Libretro thumbnail repository and its Named Snaps collection | Exact system and exact repository filename; visually inspect before approval |
| Official game, test-suite, homebrew, or core repository | Items that artwork databases do not catalogue, such as test tools and homebrew | Directly attributable image for the exact public item and version |
| Existing EmulationStation-compatible collection | Reusing already-owned images from ES-DE, Batocera, RetroPie, Recalbox, or EmuELEC | Exact imported game identity after path rebasing and visual review |

Libretro is a public repository fallback, not a title-search service. ScreenScraper is the primary external title-and-platform lookup. An official project source is preferable for a test suite, demo, homebrew release, or support disc when it publishes an exact screenshot. A search engine result is not an artwork source by itself.

Check the source's current terms before redistributing downloaded images. This procedure records provenance but does not grant redistribution rights.

## Artwork source order

Use sources in this order:

1. An exact, already present image on the MiSTer card.
2. Exact artwork imported from the user's own existing frontend collection.
3. An official game, homebrew, test-suite, or core project page when it publishes a directly attributable image for that exact item.
4. ScreenScraper's official API, when authorised credentials are available and an exact game identity resolves.
5. For Arcade, Libretro's public MAME Named Snaps by exact safe set name, but only after ScreenScraper records no exact title, an ambiguous exact title, or no screenshot.
6. Another public collection only after its identity, provenance, terms, and exact image have been reviewed and explicitly approved.

Do not use a fallback after an authentication, API, TLS, or network failure. Do not use a random search-engine image as an unrecorded substitute. Record the source URL or repository identity, exact game identity, media type, byte size, and SHA-256 for every externally obtained file.

### 5. Approve candidates by content hash

The simplest approval file maps a public title to the SHA-256 of the visually reviewed candidate:

```json
{
  "Example Game": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}
```

A reviewed local image can specify its complete destination and provenance:

```json
{
  "Example Game": {
    "image": "./reviewed/example.png",
    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "target": "media/screenshot/example.png",
    "source": "Existing local artwork, visually verified"
  }
}
```

Generate the pinned manifest:

```bash
python3 arcade_artwork_source.py \
  --folder ./artwork-candidates \
  --approved approvals.json \
  --manifest-output manifest.json
```

The manifest pins:

- The downloader log hash.
- The gamelist hash.
- Exact MRA set names.
- The reviewed image path and hash.
- The card destination.
- The expected number of missing paths.
- Any exact existing entries that may be edited.
- The recorded source.

If the card, log, gamelist, image, or expected count changes, the installation must stop and require a new review.

### 6. Dry-run the exact write set

```bash
python3 arcade_artwork_audit.py --manifest manifest.json
```

Review every printed path, set name, target image, append, and existing-entry edit. Do not use `--apply` until this output is correct.

### 7. Apply only after explicit approval

```bash
python3 arcade_artwork_audit.py --manifest manifest.json --apply
```

The reference script then:

1. Confirms that no updater, scraper, or gamelist writer is running.
2. Confirms the live gamelist still matches the reviewed hash.
3. Refuses to replace a different existing image.
4. Uploads each new image to a unique `.part` path.
5. Verifies transferred size and SHA-256.
6. Atomically renames the image and runs `sync`.
7. Creates a timestamped gamelist backup beside the original.
8. Verifies that the backup matches the reviewed original.
9. Preserves existing XML text and inserts only the approved fields or minimal entries.
10. Uploads the new XML to a unique `.part` path.
11. Verifies size and SHA-256 before the atomic rename.
12. Runs `sync`, reads the final file back, and compares every byte.
13. Parses the final XML.
14. Re-runs the Degauss lookup for every changed game.
15. Confirms every new image reference resolves to a valid image.

The script does not rebuild Degauss automatically.

## Extending the method beyond Arcade

Do not make the Arcade scripts generic by changing only a directory constant. Each system can have different roots, browse extensions, archives, virtual games, shared gamelists, and support files.

For each new system adapter, define and test:

- The system ID and all live roots from `--list-systems`.
- The exact playable file extensions and ignored files from the current Degauss system definition.
- Whether directories, archives, archive members, shortcuts, or virtual titles are browsed.
- How the displayed title is derived.
- Whether multiple roots share a gamelist.
- The external platform ID, if external lookup is authorised.
- Which variants may share artwork.
- Which support rows must remain separate from playable games.
- A read-only audit and fixtures for exact path, filename, stem, slug, inheritance, broken image, and distinct-volume cases.

Keep the same two-stage boundary:

1. A source tool can discover and download candidates but cannot write to the card.
2. An installer can write only a reviewed, hash-pinned manifest and must not contact an artwork service.

## Building a gamelist from scratch without a scraper

A scraper is not required to make a usable gamelist. Artwork and rich metadata are optional.

### 1. Resolve the live system roots

```bash
ssh mister '/media/fat/Scripts/degauss.sh --list-systems'
```

Read the current Degauss system definition for the chosen system. Do not treat every file in the directory as a game.

### 2. Build a deterministic inventory

For every item Degauss can actually browse, record:

- Library root.
- Exact relative path.
- File or virtual-entry type.
- Display name derived without deleting meaningful edition or volume text.
- Size and hash when a stable local file exists.
- Whether it is a game, variant, demo, hack, tool, BIOS, or support item.

Sort entries deterministically. Preserve case in paths. Reject paths that escape the library root.

### 3. Reuse only exact local images

Search the entire relevant card media area by:

- Exact relative path identity.
- Exact filename stem.
- Verified set name for Arcade.
- Existing parent entry.
- A previously reviewed regional variant of the same visual game.

Fully decode the candidate and inspect it. Do not infer identity only from a similar filename.

### 4. Create minimal entries

```xml
<gameList>
  <game>
    <path>./Exact Game Filename.ext</path>
    <name>Exact Game Filename</name>
  </game>
</gameList>
```

Add an artwork field only when a reviewed image exists:

```xml
<screenshot>./media/screenshot/exact-game.png</screenshot>
```

Do not invent descriptions, dates, publishers, regions, genres, or player counts. Missing metadata is better than fabricated metadata.

### 5. Dry-run and safe-write

Before writing:

- Hash the live root inventory and any existing gamelist.
- Print the exact new and changed entries.
- Confirm no concurrent writer is running.
- Obtain explicit approval.

Then use the same backup, unique `.part`, size/hash verification, atomic rename, `sync`, final reread, XML parse, and Degauss resolution checks used by the scoped Arcade installer.

If no gamelist existed when review began but one appears before installation, stop. Do not overwrite it.

## Validation after every approved write

Run all applicable checks:

```bash
ssh mister '/media/fat/Scripts/degauss.sh --audit'
ssh mister '/media/fat/Scripts/degauss.sh --report --system "<system-id>"'
```

Verify:

- The XML parses.
- The edited path occurs exactly once unless the existing format intentionally uses parent records.
- Every effective artwork reference stays inside the card and exists.
- Every referenced PNG or JPEG fully decodes.
- The changed row resolves through the intended exact match.
- Previously valid references remain valid.
- Shared regional or organised entries still resolve correctly.
- Distinct editions and volumes have not been merged.
- The expected gap count changed by exactly the approved amount.

External edits do not automatically rebuild Degauss's persistent system-list index. In Degauss, use:

- **Actions → Library → Rebuild This System List** for one changed system.
- **Options → Library → Rebuild All System Lists** after changing several systems.

The agent should report that a rebuild is needed. It should not trigger one without separate authorisation.

## Failure rules

Stop without writing when:

- The live gamelist or source scope changed after review.
- A candidate is ambiguous.
- The image is corrupt, truncated, or for another game.
- The destination contains a different file.
- ScreenScraper returns no exact match.
- An external API, credential, TLS, or network operation fails.
- A fallback was not explicitly authorised.
- Another writer is active.
- A backup, transfer, hash, XML parse, or final reread check fails.

Never convert these failures into an empty result or apparent success. Report the exact unresolved title and failed layer.

## Copyable agent brief

```text
Manage Degauss artwork directly through the MiSTer card's gamelist.xml files and image files over passwordless SSH. Treat the card as the source of truth.

Start read-only. Run the installed Degauss --audit and --list-systems commands. Resolve Favorites launchers to their actual systems before prioritising them. Treat audit counts as browse rows, not unique commercial games.

For every candidate gap, resolve exact-path, filename, stem, .slug, and parent-inherited matches using current Degauss behaviour. Distinguish a missing entry, empty artwork field, broken reference, virtual item, shared regional variant, distinct edition, and non-game/support row. Do not write yet.

Reuse exact local art first. If external lookup is authorised, send only a public game title and platform ID. Read credentials from protected files inside the process and never print them or authenticated URLs. Require an exact identity match, fully decode the image, inspect it, and record its source, size, and SHA-256. Do not use a fallback to hide a source or network failure.

Group only genuinely identical visual games. Never merge sequels, editions, volumes, conversions, or materially different releases. Leave every uncertain item unchanged.

Before a write, present a manifest pinned to the current inventory, gamelist hash, exact paths, candidate hashes, destinations, operation types, and expected counts. Wait for explicit approval. Re-read the live state and abort if it changed.

For an approved write, confirm no updater or scraper is running. Back up the gamelist. Upload images and XML to unique .part files, validate size and SHA-256, atomically rename, sync, and reread. Preserve existing XML structure and change only approved entries. Reparse the final XML, validate all affected images, rerun Degauss resolution, and ensure previously valid references still work.

Do not rebuild Degauss automatically. Report which system needs Actions > Library > Rebuild This System List, or whether Options > Library > Rebuild All System Lists is required.
```

## What not to do

- Do not use the Favorites pseudo-system as an artwork backlog.
- Do not count XML rows without applying Degauss matching and inheritance.
- Do not assume one system equals one folder.
- Do not assume every file in a game root is a playable game.
- Do not overwrite a valid image because a scraper offers another one.
- Do not share one screenshot across different games or volumes.
- Do not expose credentials in commands, URLs, logs, reports, or screenshots.
- Do not upload a gamelist, card inventory, local path, or private log to an external service.
- Do not bypass TLS validation.
- Do not rewrite the entire XML when a targeted insertion is sufficient.
- Do not run a whole-card write when only the latest Update All additions need work.
- Do not report completion until the final bytes and Degauss resolution have been verified on the card.
