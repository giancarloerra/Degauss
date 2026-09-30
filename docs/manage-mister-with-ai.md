# Managing MiSTer with AI after Update All

An AI coding agent with SSH access can check what an Update All run changed, verify the affected files, repair approved gaps, and maintain Degauss artwork. Start with the completed run, not a whole-card scan. The live device and the manifests used by that run are the sources of truth.

This guide is a maintenance procedure, not a replacement updater. It does not grant permission to download games, replace files, change subscriptions, run Update All, reboot, or rebuild Degauss lists.

## Set up once

1. Follow the [passwordless SSH setup and recovery instructions](../tools/artwork-agent/DEGAUSS_AI_ARTWORK_FAQ.md#setting-up-passwordless-ssh). Commands are labelled for the computer or the MiSTer shell. Never give an agent the private key or put a password in a command.
2. Create a separate local management folder for instructions and private reports. Keep development checkouts and imported chat archives outside the write scope.
3. Copy [the generic instruction template](mister-maintenance-AGENTS.example.md) into that folder as `AGENTS.md`. Set permissions for this installation explicitly. Tell the agent to read it at the start of every task; if the agent uses a different instruction-file convention, point it to the same file.
4. Supply this guide, the [Degauss README](../README.md), and the [artwork guide](../tools/artwork-agent/DEGAUSS_AI_ARTWORK_FAQ.md). For artwork work, download the [credential-free artwork kit](../tools/artwork-agent/Degauss-AI-Artwork-Agent-Kit.zip), install its requirements and run its tests.
5. Identify any approved local ROM-retrieval tool and its permitted sources separately. The artwork kit does **not** include a ROM downloader, game data or access keys.

The general log, core and ROM checks also apply without Degauss. The gamelist and artwork sections use Degauss's matching rules and must not be assumed to describe another frontend.

## Define permissions before repairs

Read-only inspection is the starting point. A request to check a run is not permission to change it.

Record these choices locally:

| Action | Required permission |
|---|---|
| Read the log, enabled databases and affected card files | Read-only task scope |
| Retrieve absent ROMs | Named tool, permitted sources and exact scoped archives |
| Replace an existing ROM ZIP or beta key | Explicit replacement permission, backup and complete compatibility verification |
| Add artwork or edit gamelists | Approved reviewed manifest, or an explicit standing rule with the same scope and validation requirements |
| Replace a protected boot ROM | An exact approved path and supplying database |
| Enable a database, change filters or remove files | Separate approval for the exact change |
| Run Update All, reboot, launch games or rebuild frontend lists | Separate approval |

Standing permission can cover repeated in-scope repairs. It must specify what may change and what must be preserved. It must not become permission for whole-card cleanup or arbitrary downloads.

Keep credentials, inventories, logs and reports private. An external artwork query needs only a public game title and platform identifier, plus authorised service credentials handled inside the process. Never send a complete card inventory, local paths or ROM contents to a third-party service.

## 1. Confirm access and a completed run

Run on the computer:

```sh
ssh -o BatchMode=yes -o ConnectTimeout=10 mister true
ssh mister 'tail -n 28 /media/fat/Scripts/.config/downloader/downloader.log'
```

The second command is a starting point, not a fixed audit boundary. Increase the tail only enough to read the complete summary. If it refers to a stage whose outcome is outside the downloader log, inspect the corresponding Update All log or current process state.

Do not repair files while Update All, Downloader, a scraper or another writer is active. Inspect the actual process command lines; a terminal that stopped repainting does not prove the updater stopped. Do not kill processes or rerun the updater to diagnose a frozen shell.

Record locally:

- completion timestamp, timezone, duration and completion status;
- log size and SHA-256;
- processed database identities;
- installed files, explicit removals and renames;
- errors, skipped files and declined boot-ROM replacements;
- the updater's relevant settings and filters.

Keep a local copy of the original log before another run rotates it. Do not rewrite the card log to manufacture an audit scope. If the log is incomplete or the relevant run cannot be identified, report that limitation instead of saying the run was checked.

## 2. Build the exact affected scope

Start with the run's installed, changed, renamed, removed and error entries. Include their immediate dependencies: MRA core references, ROM chains, disk/video data, organised links and effective gamelist artwork.

For an updated RBF, check the MRAs that use that core even if those MRAs were not rewritten. For a removed or renamed source MRA, check the organised links and indexed paths that depended on it. Limit these checks to the affected identities.

### Changes delivered inside archives

An installed archive can add or update MRAs without naming each member in the log. A log-only MRA list is incomplete in that case.

1. Identify the archive and its member summary from the database revision actually used by the completed run.
2. Pin the summary URL or repository revision. Verify its declared size and hash, and ZIP integrity when applicable.
3. Compare member identities and hashes with the previously recorded managed state when available.
4. Correlate the candidate changes with the run's write evidence and current card hashes. A timestamp alone does not identify the supplying database, and a current hash alone does not prove when a file changed.
5. Add only the verified affected members and their organised links to the scope.

If a later run or manual edit has overwritten this evidence, state which historical changes cannot be reconstructed. Do not silently use today's manifest as yesterday's installed manifest, or include every unchanged archive member just to produce a larger audit.

Downloader's [archive specification](https://github.com/MiSTer-devel/Downloader_MiSTer/blob/main/docs/custom-databases-archives.md) describes member summaries and archive installation. The agent should read the current specification rather than assume a particular archive name or destination.

### Storage and file identity

Locate the affected item by identity if its location is uncertain. Check the effective storage root, mounted USB/network storage and launcher's real target before declaring it absent. MiSTer's [path-resolution documentation](https://mister-devel.github.io/MkDocs_MiSTer/cores/paths/) describes storage precedence.

Do not assume that a display name equals a folder name. A targeted filename or extension search is not permission to read and checksum every file on the card.

## 3. Verify cores, launchers and ROM requirements

### Core and launcher resolution

- Read each affected MRA's `<rbf>` and `<setname>`.
- Resolve the core using the installed Main binary's actual lookup rules. Do not require the RBF filename to equal the MRA filename.
- Validate managed files against the supplying manifest's size and hash. Equal sizes are not evidence that two cores are identical.
- Resolve affected symlinks and MGL core/game paths. Report dangling links and unreadable files explicitly.
- Distinguish managed copies from intentionally separate manual implementations before proposing a repair.

The public [MiSTer MRA documentation](https://mister-devel.github.io/MkDocs_MiSTer/developer/mra/) and [Main MRA loader](https://github.com/MiSTer-devel/Main_MiSTer/blob/master/support/arcade/mra_loader.cpp) define the format and lookup behaviour. Inspect the corresponding fork if the installed Main differs.

### Complete ROM-chain verification

For every affected MRA:

1. Parse all ROM blocks, including part-level ZIP overrides. Respect the loader's archive search order and the `|`-separated archive alternatives.
2. Locate the relevant archives at their actual resolved storage location.
3. Open each used ZIP and validate integrity, for example with Python's `ZipFile.testzip()`. Handle and report read or decompression errors.
4. Compare every external part against the entries in the complete applicable archive chain. With a valid nonzero CRC, use CRC matching; without a usable CRC, use the declared filename. A `name="none"` part with a CRC must not require a literal file named `none`.
5. Account separately for inline data and assembly directives. Do not count embedded constants as missing archive files.
6. Report `found/required` external parts and name every unresolved part, required CRC and searched archive.

Use an XML parser so both quote styles work. If strict parsing fails, report the affected MRA; a carefully verified tolerant extraction may aid diagnosis but does not establish that the installed Main can load malformed XML. Never silently drop a parse failure from the totals.

Test the verifier with a known present part and a deliberate non-match before trusting a zero-missing result. BusyBox commands on the device do not necessarily support desktop flags, so use a controlled Python verifier when shell tools cannot express the check reliably.

A downloader error is not proof that a ROM is missing. The existing ZIP may still pass integrity and satisfy every affected part. Conversely, a successful download is not proof that the MRA is complete.

### Retrieve only approved gaps

Use the user's approved tool and source. Read its argument parser first: an empty name list must not start a default bulk download, and unknown arguments must fail. Prepare its manifest from the current scoped MRA requirements and retrieve only the exact missing archives.

Before uploading, validate ZIP integrity and all affected MRA parts. For an existing incomplete ZIP, check every consumer affected by replacement, including known shared dependencies. Back up the old ZIP and use the safe-write sequence below. Do not replace a valid archive merely because a downloader retry failed.

If source access, TLS validation or part verification fails, preserve the card file and report the actual failure. Do not disable certificate validation or claim that a different ROM revision is equivalent.

This guide supplies no game files or general download source. Use only sources the user is entitled and authorised to use.

### Beta keys and disk/video files

`jtbeta.zip` contains Jotego's beta access key, not game ROM data. Use a legitimately supplied key. Before a replacement, compare its integrity and `beta.bin` CRC against all affected installed references and, for a requested preview, the incoming manifest's MRAs. Do not install an incoming key that would break the current installed set. Report conflicting requirements by MRA rather than cycling backups between incompatible sets.

For CHD, VHD, DLV or other additional data, read the exact core's current README and release requirements. Verify the documented location, format, full length and authoritative checksum when provided. A file extension or four-byte header alone does not prove completeness. Check structural bounds and indexes where relevant, and compare local and card hashes after transfer.

File verification does not prove gameplay. Launch, audio, controls, seeking and restart checks require an authorised real-device test. Record those checks separately from the structural result.

## 4. Check databases, Linux and boot ROMs

### What was installed versus what is offered

[Update All](https://github.com/theypsilon/Update_All_MiSTer) uses Downloader and selects additional databases. Its current distribution choices include stable Linux and Edge Linux. Inspect this installation's selected sources and filters; do not assume every release announcement is subscribed or eligible.

For an optional update preview, compare a pinned current manifest with the live managed state. Report adds, updates, removals, excluded content and unmet dependencies separately. A developer release, a database offer and a verified installed file are three different states. Adding a source or changing a filter requires approval.

### Three kernel states

Compare:

1. **Offered:** the Linux/kernel payload in the enabled, eligible database.
2. **Stored:** the corresponding payload actually present on the device.
3. **Running:** the active kernel reported by the device.

Run on the computer:

```sh
ssh mister 'uname -r; cat /proc/version'
```

Locate the stored payload by its manifest role and the updater's install mapping, not an assumed directory. A Linux image archive can contain the kernel; its outer filename need not expose a version. Verify the supplying database, pinned payload URL, size and declared hash. Use authoritative release/build evidence to compare the payload with the running kernel. Do not claim that the running kernel is newer merely because a download completed.

Report `unchanged`, `newer`, `unavailable` or `unverified`, with exact evidence and whether the normal configured updater would install it. A verified stored update can still require a reboot to become active. Do not reboot or alter updater settings during an analysis-only check.

### Declined boot-ROM replacements

A database entry with `overwrite: false` protects an existing file from replacement. A declined upgrade is not automatically an error, and a different hash is not by itself proof of a newer or preferable BIOS. See Downloader's [manifest specification](https://github.com/MiSTer-devel/Downloader_MiSTer/blob/main/docs/custom-databases.md).

If the user wants such files kept current, define an exact path/database whitelist. Compare the card file with the last privately recorded verified version before overwriting it. If it changed independently, stop for a decision. Fetch only the offered manifest URL over valid HTTPS, validate its size and hash, back up the existing file, and use the same safe-write procedure. Do not generalise permission for one boot ROM to all BIOS files.

## 5. Complete artwork for the same run

This is a required step when maintaining a Degauss library after Update All, not an optional follow-up to ROM repair.

Use the [complete artwork guide](../tools/artwork-agent/DEGAUSS_AI_ARTWORK_FAQ.md). Check every affected playable path from step 2, including archive-delivered members and organised or regional alternatives.

- Resolve actual system roots, exact-path/filename/stem/virtual-slug matches and parent metadata inheritance before counting gaps.
- Keep valid art. Share images only for verified variants of the same visual game, not sequels, editions or volumes.
- Reuse an exact, reviewed existing image first. Otherwise use authorised ScreenScraper access or an explicitly permitted, recorded public source such as Libretro Named Snaps.
- Fully decode and visually inspect each candidate. Do not launch an emulator or core to manufacture artwork.
- Create a missing entry or repair only the approved artwork field. Preserve unrelated XML and existing inheritance.
- Confirm no concurrent writer, make backups, safe-write and re-read the files, then verify every affected reference and previously valid references.

The kit's default `arcade_artwork_audit.py` CLI covers log-listed MRAs and their organised links. It does **not** automatically discover archive-delivered changes or MRAs affected only by an RBF update. A zero-gap result from that subset is not a complete post-update artwork result.

For additional verified paths, perform the expanded path audit described in the [artwork guide](../tools/artwork-agent/DEGAUSS_AI_ARTWORK_FAQ.md#archive-delivered-and-other-indirect-changes), then use `degauss_artwork_apply.py` with an explicit reviewed manifest if repairs are needed. Do not invent a `--scope` flag, rewrite the real log, or pretend the default CLI covers those paths.

Report which Degauss system lists need rebuilding. Rebuild only with authorisation. Card XML/image validity, index resolution and visible frontend display are distinct checks.

## 6. Preserve manual content and validate writes

A shared `<setname>` is not enough to identify a duplicate: two MRAs can use different cores intentionally. Compare the normalized setname **and** core identity, affected location and managed/manual ownership. Keep empty setnames separate. Report suspected duplicates; do not automatically sweep curated folders.

For every approved file write:

1. Resolve the exact destination and confirm no concurrent writer.
2. Back up an existing file and verify the backup against the original.
3. Upload to a uniquely named `.part` file in the destination directory.
4. Verify transferred size and SHA-256 against the staged source.
5. Atomically rename the validated temporary file to the final destination.
6. Run `sync` on the MiSTer.
7. Re-read and validate the final bytes and all affected references.
8. Remove only temporary files created by that operation.

Use the manifest's hash algorithm as well as SHA-256 where the manifest requires it. A matching transfer hash proves the copy, not that the original candidate was suitable. If a failure occurs after a rename, report the exact state and preserve the verified backup for authorised recovery; do not conceal it with an apparent success.

## Completion and interrupted work

The local report must contain:

| Section | Required evidence |
|---|---|
| Run | Completion time, log hash and pinned supplying revisions |
| Scope | Direct changes, unpacked changes, affected dependencies and exact counts |
| Verified | Core resolution, archive/part results, kernel states and artwork references |
| Changes | Exact paths, candidate hashes, backups and final read-back results |
| Unresolved | Every missing part, failed operation and unperformed required check |
| Frontend | Affected lists needing a rebuild; whether rebuild/display were actually tested |

Do not declare the post-update task complete until every required scoped check, authorised repair and **that run's artwork pass** has passed direct validation. An unresolved gap or unavailable required check leaves the task incomplete unless the user explicitly accepts that exact exception. A repair attempt can be finished without the original problem being resolved; report both states accurately.

After an interruption or a narrow follow-up, retain all unaffected outstanding work. If another Update All runs, preserve both run identities, reassess affected live state and finish the combined outstanding scope. Completing one named game's screenshot or one ROM repair never closes the rest of the run.

Use a separate status for physical gameplay and visible display if those have not been tested. Do not convert a file check into a claim that every game works.

## Copyable agent brief

```text
Read my local AGENTS.md, the Degauss README, Managing MiSTer with AI after Update All, and the linked artwork guide. Use passwordless SSH and keep logs, inventory and credentials local.

Check the latest completed Update All run. Start with its summary and pin the original log and supplying database revisions. Do not run a whole-card scan. Include exact direct changes, verified changed archive members, and immediate core/ROM/launcher/artwork dependencies. Do not treat today's manifest as the one used by an older run.

Verify affected core lookup and links. For each affected MRA, verify complete applicable ZIP chains, part-level overrides, integrity, CRC/name matching and inline-data distinctions. Report parse failures and missing parts explicitly. Use only my authorised retrieval tool and sources for approved gaps. Verify all affected consumers before an existing ZIP or beta-key replacement.

Compare offered, stored and running Linux/kernel evidence separately. Respect protected boot ROMs and the exact permissions in AGENTS.md. Do not change subscriptions, filters, configurations or manual content without approval.

Complete the same-run artwork pass for every affected playable path, including archive-delivered alternatives and organised links. Reuse only exact reviewed art, preserve gamelist metadata and inheritance, and verify final XML and images on the card. Do not generate screenshots through an emulator. The default Arcade artwork CLI is only a log-listed subset; explicitly audit additional verified paths.

For authorised repairs, confirm no concurrent writer, review the exact change set, back up existing files, transfer through unique .part paths, validate size and hashes, atomically rename, sync and reread. Never disable TLS checks or conceal a failed source or validation step.

Keep outstanding requirements across interruptions and later Update All runs. Report run identity, scope, verified results, writes, backups, unresolved items and any needed Degauss list rebuild. Do not run Update All, reboot, launch games or reindex unless separately authorised. Do not claim completion while a required scoped check or the artwork pass remains incomplete.
```
