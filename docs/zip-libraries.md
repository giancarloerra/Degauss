# ZIP libraries

A system that does not list `zip` among its launchable extensions opens each ZIP as a virtual folder. Root and nested games retain the native `archive.zip/folder/game.ext` target; neither browsing nor launch preparation extracts or modifies the archive. Directories can be explicit ZIP records or implied by member paths. A system already configured to launch `zip` keeps the whole archive as one game. An archive, or a virtual directory inside one, that holds a single supported game shows that game's artwork on its folder row before it is opened; it still opens as a folder.

The reader supports single-disk classic ZIP and ZIP64 with stored or deflated entries. Encryption, compressed-patch entries, other compression methods, nested archives, duplicate names and ASCII-case-ambiguous paths are rejected explicitly. Names must be exact UTF-8 and cannot contain traversal components, ambiguous separators or control characters. The complete native target must fit 1,023 bytes and the member filename must fit 260 bytes, matching the relevant Main buffers. An earlier `.zip` substring in the outer path is unsupported because Main splits its native ZIP target at the first such substring.

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

Replacing an archive is reflected by **Rebuild this system list** or a full rebuild. A successful scan replaces the current contents. An unreadable or malformed archive reports its path and reason; a failed rebuild preserves the previous valid cache and summary. Existing cache, Favorites and state formats remain readable.

Launch confirmation reopens and validates the outer archive and selected member. It reports a missing, renamed or unsupported member before handing control to Main. Reading the central directory does not establish the integrity of compressed payloads: decompression and payload checks remain Main's responsibility.

## Main archive-comment limitation

The Main reader at revision [`f8dc68e3dcf4694f5593e6552aea56cd852982af`](https://github.com/MiSTer-devel/Main_MiSTer/blob/f8dc68e3dcf4694f5593e6552aea56cd852982af/lib/miniz/miniz.c) selects the last end-record signature with enough following bytes, without first validating the ZIP comment length. A valid archive comment containing that signature can therefore make Main select the wrong end record and reject the archive.

Degauss correctly lists such an archive, but launch confirmation reports this Main limitation explicitly. It does not hand off a known incompatible target, extract a replacement ROM or rewrite the ZIP. Supporting that launch requires a Main reader that correctly validates end-record candidates; a Degauss-only parser change cannot repair Main's independent lookup.

## Local reader smoke test

With the pinned Main `lib/miniz` source already available locally:

```sh
python3 scripts/test-main-zip.py /path/to/Main_MiSTer/lib/miniz
```

The script verifies the source hashes, compiles Degauss's parser and Main's production iterator, and generates a few small synthetic archives locally. It compares exact member names, sizes and CRCs through stored, deflated and ZIP64 reads, verifies rejected unsupported archives, and verifies the explicit comment limitation. Temporary fixtures and binaries are removed automatically. There are no network requests or card writes.

This test exercises host filesystem and reader behavior. It does not prove ARM32 execution, a core launch or achievement operation. Large-entry-count and sparse-file tests belong on the host; the card smoke test needs only a few games in a small isolated library.
