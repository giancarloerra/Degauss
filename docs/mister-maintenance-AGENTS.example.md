# MiSTer maintenance instructions

Copy this template into a separate local management folder as `AGENTS.md`. Configure it for the actual installation before granting write permissions. This file is not a permission grant by itself.

## Setup and authority

- SSH alias: `mister`, configured locally. Do not read or transmit the private key.
- Read the current Degauss README, `docs/manage-mister-with-ai.md` and its linked artwork guide.
- Source of truth: the live device, original completed-run logs and the database revisions used by those runs.
- Keep reports, inventories, hashes, device addresses and credentials private.
- Imported session archives are read-only reference material. Historical statements require current verification before action.
- One coordinating agent owns writes at a time. Do not run concurrent updater, scraper or gamelist writes.

## Permissions to configure locally

- Default: read-only checks of the requested completed run and its immediate dependencies.
- ROM retrieval tool and approved sources: **not configured; ask before downloading**.
- Absent-ROM upload permission: **not granted**.
- Existing-ROM or beta-key replacement permission: **not granted**.
- Artwork/gamelist writes: **reviewed manifest and explicit approval required**.
- Protected boot-ROM whitelist: **empty**.
- Subscription, filter and configuration changes: **separate approval required**.
- Update All, reboot, game launch and Degauss list rebuild: **separate approval required**.
- Deletion or duplicate cleanup: **separate approval for exact files required**.

Replace these defaults only with the user's explicit choices. A scoped standing rule may authorise repeated necessary writes but must name allowed file types, sources, replacements, backups and validation. It does not authorise a whole-card scan or cleanup.

## Required post-Update All sequence

1. Confirm connectivity, writer state and completion of the requested run.
2. Preserve its original log locally and record completion time, timezone and SHA-256.
3. Scope installed/changed/renamed/removed/error items and immediate dependencies. Include archive-delivered changes using pinned member summaries plus verified write/change evidence. Report unknown historical coverage.
4. Verify affected core lookup, launchers, symlinks and complete MRA ROM chains. Count and name malformed/unreadable MRAs instead of dropping them.
5. Use only approved sources/tools for authorised ROM gaps; validate integrity and every affected part before uploading or replacing. Beta keys are not game ROMs.
6. Compare offered, stored and running kernel evidence. Respect protected boot-ROM ownership and the configured whitelist.
7. Complete artwork/gamelist validation for every affected playable path from this same run, including alternatives and organised links. The default Arcade artwork CLI does not discover unpacked or core-only affected MRAs.
8. Perform only approved repairs, then re-read and validate final files and references. Report any needed frontend rebuild separately.
9. Retain outstanding requirements across interruptions and subsequent runs. A narrow repair does not close the rest of the run.

## Preservation and safe writes

- Preserve manual cores, ROMs, configuration, favourites and metadata.
- Group duplicate candidates by game and core identity, not setname alone. Never sweep curated folders automatically.
- Resolve actual storage roots, virtual paths and parent metadata inheritance before declaring missing games or artwork.
- Use only exact reviewed existing or authorised online artwork. Do not launch emulators or cores to create screenshots.
- Do not bypass TLS or suppress source, parsing, transfer or validation failures.
- Before writing: review exact paths/counts, recheck live source hashes, confirm no writer and verify backups.
- Transfer through unique destination-directory `.part` files; validate size and SHA-256, atomically rename, sync, reread and verify affected references.
- Clean up only temporary files created by the operation.

## Completion gate

- Report run identity and scope, verified results, exact writes/backups, unresolved failures and unperformed checks.
- Do not call a post-update task complete until all required scoped checks, authorised repairs and the same-run artwork pass have passed direct card validation.
- An unresolved gap or required unperformed check leaves the task incomplete unless the user explicitly accepts that exact exception.
- Distinguish card-file validity, frontend index resolution, visible display and physical gameplay. Never claim an untested layer passed.
