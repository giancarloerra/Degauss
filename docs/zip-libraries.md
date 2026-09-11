# ZIP libraries

A system that does not list `zip` among its launchable extensions opens each ZIP as a virtual folder. Root and nested games retain the native `archive.zip/folder/game.ext` target; neither browsing nor launch preparation extracts or modifies the archive. Directories can be explicit ZIP records or implied by member paths. A system already configured to launch `zip` keeps the whole archive as one game.

The reader supports single-disk classic ZIP and ZIP64 with stored or deflated entries. Problems are handled at two levels.

A central directory that does not hold together fails the whole archive: the archive is skipped with its path and reason and nothing from it is listed. That is any of:

- a truncated, unreadable or inconsistent directory, or an invalid signature, count, size or offset;
- invalid ZIP64 metadata, or a multi-disk archive;
- a member whose disk number is the ZIP64 sentinel (which Main refuses without resolving) or is not the archive's disk (Main also lets a record saying disk 1 into a disk-0 archive; Degauss is stricter there, which only ever leaves an archive out);
- a member record whose sizes or local-header offset do not fit the archive;
- a member with a masked local header (general-purpose bit 13, which Main refuses for the whole archive when it reads the directory);
- an archive path holding an earlier `.zip` substring, which Main splits its native target at.

Every record is checked this way, including one that is about to be skipped on its own, because Main checks them all before it will open the archive.

An archive one of whose member paths goes deeper than the folder depth the index walks (twelve levels below where the system starts, the folders holding the archive included) is also skipped whole, with `a member path exceeds the maximum folder depth of 12`: that is the walk's own limit rather than anything Main checks, and an archive the walk cannot finish is not listed in part. `--report` and `--audit` count the members they reach before that depth and name the archive with the same reason. A folder that deep remains the system's failure.

A problem confined to one member skips only that member, with its exact reason, and the other members stay. That is any of:

- an encrypted entry, or a compressed-patch entry;
- another compression method, named by its number;
- a nested archive member (Main refuses a second `.zip` in a target);
- a traversal or empty path segment, a backslash or colon, a control character, or leading or trailing whitespace in the name;
- a member filename over 260 bytes or a native target over 1,023 bytes, matching the relevant Main buffers.

An encrypted stored member carries its encryption header in its compressed size; Main's stored-size check reads the method and the DOS time as one word and only fails such a member when its time is zero, so Degauss fails the archive in exactly that case and otherwise skips the member.

Duplicate or ASCII-case-ambiguous paths, including a file whose name is also a directory, skip the whole conflicting group rather than choosing one of them; a member already skipped for its own reason still counts as part of a group, because Main's lookup can land on its record.

Member names are used exactly as their raw central-directory bytes. Main locates a member by comparing the requested bytes with those raw bytes, ASCII case-insensitively, without decoding them and without reading the UTF-8 flag or the Info-ZIP Unicode Path extra field ([Degauss-Main lib/miniz/miniz.c, `mz_zip_reader_locate_file_v2`](https://github.com/giancarloerra/Degauss-Main/blob/0651979d53f570c29f8772e124ae603297830954/lib/miniz/miniz.c#L4312-L4373)). A raw name that is valid UTF-8 is therefore the only representation that travels unchanged through the cache, the generated MGL and Main, and it is accepted whether or not the archive sets the UTF-8 flag.

A name that is not valid UTF-8 is skipped: without the flag as `unsupported legacy ZIP filename encoding`, with the flag as a name flagged UTF-8 that is not.

The Unicode Path extra field (APPNOTE 4.6.9) is validated (version 1 and a matching CRC-32 of the raw name; otherwise it is ignored, as APPNOTE requires) and its decoded name is written after the reason in the log line so the member can be identified; the reason itself stays the same for every such member so the summary can count them. It is never used as the launch name, because it would not resolve through Main. No name is ever decoded lossily.

Archives are bounded to 100,000 central-directory entries and 16 MiB of central-directory data. Both limits apply before allocation, regardless of overall archive size. The archive can exceed 4 GiB when these limits are met. Entry sizes and archive offsets are parsed with checked 64-bit arithmetic. These bounds constrain both frontend memory use and the central directory Main reads when opening a member.

Use complete relative metadata paths for individual games:

```xml
<game>
  <path>./library.zip/folder/game.rom</path>
  <name>Game title</name>
  <image>./images/game.png</image>
</game>
```

An archive containing more than one game supported by the system uses only exact member metadata paths. A single supported game retains existing archive/title metadata matching. Scraping a single member preserves Keep/Fill policies but writes a separate exact member entry when an update is needed; it never overwrites the archive-level metadata row. Artwork remains an external file alongside the library.

Replacing an archive is reflected by **Rebuild this system list** or a full rebuild. A successful scan replaces the current contents. A malformed archive or an unsupported member is skipped with a warning and the healthy remainder of the system is published in the same transaction; the rebuild finishes as **Finished With Problems**, and the summary names the system, the archive and the reason, one line per archive and reason with a count of members. For a system prepared from an Artwork Pack, **Rebuild this system list**, the preparation on opening and a change of its game data source show the same archive and reason lines in their completion message.

A system holding only rejected content completes with zero games and that warning. An unreadable system folder, a cancellation or a failure to publish remains a failed transaction that preserves the previous valid cache and summary. Existing cache, Favorites and state formats remain readable.

Every skipped member is written to `/tmp/degauss.log` as `zip <archive>: member <name>: <reason>` (a name holding a control character is escaped there) by the reader whenever it reads the archive, once per read: a rebuild, `--report`, `--audit`, a listing straight from the card of a system that is not indexed, and the check before a launch all leave the lines. A skipped archive is written by the scan as `zip <archive>: skipped: <error>` at the moment it is skipped.

Launch confirmation reopens and validates the outer archive and selected member. It reports a missing or renamed member before handing control to Main, and for a member the archive holds but the reader skipped (a favourite or gamelist entry can still name one) it reports the skip reason instead. Reading the central directory does not establish the integrity of compressed payloads: decompression and payload checks remain Main's responsibility.

`--report` and `--audit` list skipped members and skipped archives as `unreadable` lines with their reason: `--report` prints the first five, `--audit` the first three per system, and the log holds all of them. When `--report` has to prepare a system's Artwork Pack cache first, the scan's warnings are printed as `source note` lines.

## Main archive-comment limitation

The Main reader at revision [`f8dc68e3dcf4694f5593e6552aea56cd852982af`](https://github.com/MiSTer-devel/Main_MiSTer/blob/f8dc68e3dcf4694f5593e6552aea56cd852982af/lib/miniz/miniz.c) selects the last end-record signature with enough following bytes, without first validating the ZIP comment length. A valid archive comment containing that signature can therefore make Main select the wrong end record and reject the archive.

Degauss correctly lists such an archive, but launch confirmation reports this Main limitation explicitly. It does not hand off a known incompatible target, extract a replacement ROM or rewrite the ZIP. Supporting that launch requires a Main reader that correctly validates end-record candidates; a Degauss-only parser change cannot repair Main's independent lookup.

## Local reader smoke test

With the pinned Main `lib/miniz` source already available locally:

```sh
python3 scripts/test-main-zip.py /path/to/Main_MiSTer/lib/miniz
```

The script verifies the source hashes, compiles Degauss's parser and Main's production iterator, and generates a few small synthetic archives locally.

It compares exact member names, sizes and CRCs through stored, deflated and ZIP64 reads, verifies that an encrypted or unusually compressed member is skipped and refused for launch while its supported sibling is still listed and Main's iterator rejects the archive, verifies that a masked local header and a ZIP64 sentinel member disk number are refused whole by both readers, and verifies the explicit comment limitation. Temporary fixtures and binaries are removed automatically. There are no network requests or card writes.

This test exercises host filesystem and reader behavior. It does not prove ARM32 execution, a core launch or achievement operation. Large-entry-count and sparse-file tests belong on the host; the card smoke test needs only a few games in a small isolated library.
