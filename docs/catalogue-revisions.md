# Independent system catalogue revisions

**Options → Library → Check for System Catalogue Updates** is manual. It reads
the official repository's `catalogue-v1-<number>` tags, not the development
branch's systems file. A newer compatible revision offers **Install** or
**Back**. Installing affects the next Degauss start, not the currently open
library. Reopen Check to rebuild only an affected library present on the card.
This does not update cores, games, MiSTer Main or Degauss itself.

The packaged systems table is never overwritten. A manually edited table
or explicitly selected alternative remains authoritative, as do the user's
folder and compatible-core overrides. A failed or offline check leaves the
working table in place; the diagnostic is in `/tmp/degauss.log`.

## Publishing a catalogue-only revision

1. Make the system-specific changes in `assets/systems.toml` and its identical
   installation copy. Verify the affected media actually index and launch on
   the oldest supported released frontend **and its corresponding Main**. If
   software changes are necessary, ship those in a frontend release instead.
2. Choose a revision greater than all previously published catalogue revisions.
   Run `python3 scripts/prepare-catalogue.py --revision NUMBER --minimum X.Y.Z
   --summary "Short description"`. Commit the exact two systems files and
   `catalogue/revision.json` together. Complete normal review and testing.
3. Publish a **lightweight** tag `catalogue-v1-NUMBER` on that approved commit.
   Do not move existing tags. Degauss looks up the tag, pins both metadata
   and content to its commit, and checks format, minimum release, byte count,
   SHA-256 and the parsed table before activating a single atomic record.
4. Record the revision, minimum Degauss version and verified entries in #204.
   Keep that tracker open. The next frontend release must bundle the approved
   table and set its bundled catalogue revision accordingly.
