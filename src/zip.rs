//! Read the ZIP central directory without extracting file data.
//!
//! MiSTer opens the native `archive.zip/member` target. Its reader supports
//! stored and deflated single-disk classic/ZIP64 archives. Names must survive
//! our UTF-8 state/XML paths and MiSTer's ASCII-insensitive lookup unchanged.
//! A central directory that does not hold together fails the whole archive;
//! a member Main cannot use is left out with its reason, and the rest of the
//! archive stays available.

use std::collections::hash_map::Entry as MapEntry;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{DegaussError, Result};

const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const CENTRAL_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const MAX_TRAILER: u64 = 22 + u16::MAX as u64;
/// Bound both Main's central-directory allocation and Degauss's entry table.
/// Main itself uses 32-bit directory sizes/counts; these lower bounds leave
/// room for the frontend, metadata and Main on a 512 MiB MiSTer.
pub const MAX_ENTRIES: u64 = 100_000;
pub const MAX_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;
/// Main's fileTYPE path buffer is 1024 bytes including the terminator.
pub const MAX_NATIVE_PATH_BYTES: usize = 1023;

fn bad(path: &Path, reason: impl AsRef<str>) -> DegaussError {
    DegaussError::malformed("zip archive", path, reason.as_ref())
}
fn word(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("checked record"),
    )
}
fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked record"),
    )
}
fn qword(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("checked record"),
    )
}
fn read_at(file: &mut std::fs::File, path: &Path, offset: u64, bytes: &mut [u8]) -> Result<()> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| DegaussError::io("seeking archive", path, e))?;
    file.read_exact(bytes)
        .map_err(|e| DegaussError::io("reading archive directory", path, e))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub crc32: u32,
    pub size: u64,
}

/// A member the archive holds that cannot be offered, and why. The name is
/// the raw central-directory bytes, escaped when they are not UTF-8, so the
/// log identifies the member exactly without a lossy decode; a UTF-8 name is
/// kept exact, so a favourite naming the member still finds it. The reason
/// is the same text for every member it applies to, so a summary can count
/// them; anything that identifies one member further goes in the detail,
/// which only the log prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub member: String,
    pub reason: String,
    pub detail: Option<String>,
}

impl Skipped {
    /// The name as a diagnostic prints it: exact, unless it holds a control
    /// character, which is itself a reason to be here and would split the
    /// line it is printed on.
    pub fn shown(&self) -> String {
        if self.member.chars().any(char::is_control) {
            self.member.escape_debug().to_string()
        } else {
            self.member.clone()
        }
    }

    /// The complete log line body for this member.
    pub fn describe(&self) -> String {
        let member = self.shown();
        match &self.detail {
            Some(detail) => format!("member {member}: {}; {detail}", self.reason),
            None => format!("member {member}: {}", self.reason),
        }
    }
}

#[derive(Debug)]
pub struct Contents {
    pub entries: Vec<Entry>,
    pub directories: Vec<String>,
    pub skipped: Vec<Skipped>,
    // Current Main picks the last signature without checking comment length.
    main_compatible: bool,
}

/// A bounded, operation-local archive cache. Opening the file and checking its
/// identity on every use keeps ordinary browsing responsive to replacement;
/// launch confirmation deliberately calls `entries` directly instead.
#[derive(Debug, Default)]
pub struct ArchiveCache {
    last: Option<(PathBuf, ArchiveStamp, std::sync::Arc<Contents>)>,
}

#[derive(Debug, PartialEq, Eq)]
struct ArchiveStamp {
    size: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl ArchiveCache {
    pub fn read(&mut self, path: &Path) -> Result<std::sync::Arc<Contents>> {
        self.read_controlled(path, &AtomicBool::new(false))?
            .ok_or_else(|| bad(path, "archive listing cancelled unexpectedly"))
    }

    pub fn read_controlled(
        &mut self,
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<std::sync::Arc<Contents>>> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let file =
            std::fs::File::open(path).map_err(|e| DegaussError::io("opening archive", path, e))?;
        let metadata = file
            .metadata()
            .map_err(|e| DegaussError::io("reading archive size", path, e))?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let stamp = ArchiveStamp {
            size: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|e| DegaussError::io("reading archive modification time", path, e))?,
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        };
        if let Some((previous, previous_stamp, entries)) = &self.last {
            if previous == path && previous_stamp == &stamp {
                return Ok(Some(entries.clone()));
            }
        }
        let Some(contents) = contents_controlled(path, cancelled)? else {
            return Ok(None);
        };
        let entries = std::sync::Arc::new(contents);
        self.last = Some((path.to_path_buf(), stamp, entries.clone()));
        Ok(Some(entries))
    }
}

/// Split only a native ZIP member target. The member string is not normalized.
pub fn split_member_path(path: &Path) -> Option<(PathBuf, String)> {
    let raw = path.to_str()?;
    let at = raw.to_ascii_lowercase().find(".zip/")? + 4;
    Some((PathBuf::from(&raw[..at]), raw[at + 1..].to_string()))
}

/// Re-read the actual archive, including all ambiguity and format checks.
pub fn validate_member(path: &Path) -> Result<Entry> {
    let (archive, member) =
        split_member_path(path).ok_or_else(|| bad(path, "not a native archive/member target"))?;
    find_member(contents(&archive)?, &archive, &member)
}

/// Validate the selected member and reject a known Main reader limitation
/// before handoff. Listing can remain correct for these valid containers.
pub fn validate_member_for_launch(path: &Path) -> Result<Entry> {
    let (archive, member) =
        split_member_path(path).ok_or_else(|| bad(path, "not a native archive/member target"))?;
    let contents = contents(&archive)?;
    if !contents.main_compatible {
        return Err(DegaussError::unsupported("zip launch", format!(
            "{}: MiSTer Main's ZIP reader selects a different end record inside the archive comment; member {member:?} cannot be launched",
            archive.display())));
    }
    find_member(contents, &archive, &member)
}

/// The selected member, or the reason it was left out: a favourite or a
/// gamelist can still name a member the archive holds but Main cannot use,
/// and that is a different message from one that was renamed.
fn find_member(contents: Contents, archive: &Path, member: &str) -> Result<Entry> {
    let Contents {
        entries, skipped, ..
    } = contents;
    if let Some(entry) = entries.into_iter().find(|entry| entry.name == member) {
        return Ok(entry);
    }
    Err(
        match skipped.iter().find(|skipped| skipped.member == member) {
            Some(skipped) => DegaussError::unsupported(
                "zip launch",
                format!(
                    "{}: member {member:?} skipped: {}",
                    archive.display(),
                    skipped.reason
                ),
            ),
            None => bad(
                archive,
                format!("member {member:?} is missing or was renamed"),
            ),
        },
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn list(path: &Path) -> Result<Vec<String>> {
    entries(path).map(|entries| entries.into_iter().map(|entry| entry.name).collect())
}
pub fn entries(path: &Path) -> Result<Vec<Entry>> {
    contents(path).map(|contents| contents.entries)
}
fn contents(path: &Path) -> Result<Contents> {
    contents_controlled(path, &AtomicBool::new(false))?
        .ok_or_else(|| bad(path, "archive listing was cancelled unexpectedly"))
}

#[derive(Clone, Copy)]
struct Directory {
    count: u64,
    offset: u64,
    size: u64,
    disk: u32,
}

fn layout(file: &mut std::fs::File, path: &Path, end: &[u8], end_offset: u64) -> Result<Directory> {
    let disk = u32::from(word(end, 4));
    let cd_disk = u32::from(word(end, 6));
    let on_disk = u64::from(word(end, 8));
    let count = u64::from(word(end, 10));
    let size = u64::from(dword(end, 12));
    let offset = u64::from(dword(end, 16));
    let needs64 = [on_disk, count].contains(&u64::from(u16::MAX))
        || [size, offset].contains(&u64::from(u32::MAX));
    let mut directory = Directory {
        count,
        offset,
        size,
        disk,
    };
    let mut records_start = end_offset;
    let mut locator = [0u8; 20];
    let has_locator = if end_offset >= 20 {
        read_at(file, path, end_offset - 20, &mut locator)?;
        locator[..4] == [0x50, 0x4b, 0x06, 0x07]
    } else {
        false
    };
    if needs64 && !has_locator {
        return Err(bad(path, "missing ZIP64 locator for sentinel fields"));
    }
    if has_locator {
        let record_offset = qword(&locator, 8);
        if dword(&locator, 4) != 0 || dword(&locator, 16) != 1 {
            return Err(bad(path, "multi-disk ZIP64 locator is unsupported"));
        }
        if record_offset
            .checked_add(56)
            .is_none_or(|end| end > end_offset - 20)
        {
            return Err(bad(path, "ZIP64 end record is outside the archive"));
        }
        let mut record = [0u8; 56];
        read_at(file, path, record_offset, &mut record)?;
        if record[..4] != [0x50, 0x4b, 0x06, 0x06]
            || qword(&record, 4) < 44
            || record_offset
                .checked_add(12)
                .and_then(|v| v.checked_add(qword(&record, 4)))
                != Some(end_offset - 20)
        {
            return Err(bad(
                path,
                "invalid ZIP64 end record signature, size or locator chain",
            ));
        }
        let disk64 = dword(&record, 16);
        if disk64 > 1 || dword(&record, 20) != disk64 || qword(&record, 24) != qword(&record, 32) {
            return Err(bad(path, "multi-disk or inconsistent ZIP64 counts"));
        }
        directory = Directory {
            count: qword(&record, 32),
            size: qword(&record, 40),
            offset: qword(&record, 48),
            disk: disk64,
        };
        for (classic, sentinel, actual) in [
            (count, u64::from(u16::MAX), directory.count),
            (on_disk, u64::from(u16::MAX), directory.count),
            (size, u64::from(u32::MAX), directory.size),
            (offset, u64::from(u32::MAX), directory.offset),
        ] {
            if classic != sentinel && classic != actual {
                return Err(bad(path, "classic and ZIP64 end records disagree"));
            }
        }
        if (disk != u32::from(u16::MAX) && disk != disk64)
            || (cd_disk != u32::from(u16::MAX) && cd_disk != disk64)
        {
            return Err(bad(path, "classic and ZIP64 disk fields disagree"));
        }
        records_start = record_offset;
    } else if disk > 1 || cd_disk != disk || count != on_disk {
        return Err(bad(path, "multi-disk archive or inconsistent entry counts"));
    }
    if directory.count > MAX_ENTRIES || directory.size > MAX_DIRECTORY_BYTES {
        return Err(bad(path, format!("resource limit exceeded: at most {MAX_ENTRIES} entries and {MAX_DIRECTORY_BYTES} central-directory bytes")));
    }
    if directory
        .count
        .checked_mul(46)
        .is_none_or(|minimum| minimum > directory.size)
        || directory.offset.checked_add(directory.size) != Some(records_start)
    {
        return Err(bad(
            path,
            "central-directory size, count or offset is inconsistent with end records",
        ));
    }
    Ok(directory)
}

/// Main splits its native target at the first `.zip`, so the archive path
/// itself has to end there: no member of an archive at any other path can be
/// addressed, which makes this an archive-level failure.
fn outer_path(path: &Path) -> Result<&str> {
    let outer = path
        .to_str()
        .ok_or_else(|| bad(path, "archive path is not UTF-8"))?;
    if outer.to_ascii_lowercase().find(".zip") != outer.len().checked_sub(4)
        || outer.chars().any(char::is_control)
    {
        return Err(bad(
            path,
            "archive path holds an earlier `.zip` or a control character, which Main cannot address",
        ));
    }
    Ok(outer)
}

/// Why one member cannot become a byte-exact native path, or `None` when it
/// can: Main's separators, its 260-byte filename and 1024-byte path buffers,
/// and its refusal of a second `.zip` in the target.
fn member_name_problem(name: &str, outer_len: usize) -> Option<String> {
    let body = name.strip_suffix('/').unwrap_or(name);
    if body
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Some("name holds a traversal or empty path segment".to_string());
    }
    if body.contains('\\') || body.contains(':') {
        return Some(
            "name holds a backslash or colon, which is not a native separator".to_string(),
        );
    }
    if body.chars().any(char::is_control) {
        return Some("name holds a control character".to_string());
    }
    if body.trim() != body {
        return Some("name starts or ends with whitespace".to_string());
    }
    if body.rsplit('/').next().is_some_and(|part| part.len() > 260) {
        return Some("filename exceeds Main's 260-byte limit".to_string());
    }
    if body
        .split('/')
        .any(|part| part.to_ascii_lowercase().ends_with(".zip"))
    {
        return Some("nested archive member is unsupported by MiSTer Main".to_string());
    }
    if outer_len
        .checked_add(1)
        .and_then(|n| n.checked_add(body.len()))
        .is_none_or(|n| n > MAX_NATIVE_PATH_BYTES)
    {
        return Some(format!(
            "native target exceeds {MAX_NATIVE_PATH_BYTES} bytes"
        ));
    }
    None
}

/// The standard ZIP CRC-32, a bit at a time. Only member names pass through
/// it, and this file is also compiled on its own by scripts/test-main-zip.py,
/// so it cannot lean on a crate.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// The Info-ZIP Unicode Path field (APPNOTE 4.6.9): version 1, the CRC-32
/// of the raw name it describes, then the UTF-8 name. Another version or a
/// stale CRC means the field is ignored, as APPNOTE says. The name it
/// carries only identifies a member in a diagnostic: Main looks members up
/// by their raw central-directory bytes and never reads this field, so it
/// is only decoded for a member that is being left out.
fn unicode_path<'a>(value: &'a [u8], raw: &[u8]) -> Option<&'a str> {
    if value.len() < 5 || value[0] != 1 || dword(value, 1) != crc32(raw) {
        return None;
    }
    std::str::from_utf8(&value[5..]).ok()
}

/// A member name for a diagnostic: quoted when it is UTF-8, otherwise the
/// raw bytes escaped, exact either way.
fn shown(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(name) => format!("{name:?}"),
        Err(_) => format!("\"{}\"", raw.escape_ascii()),
    }
}

/// Central-directory-only read with cancellation. Payload CRC/decompression
/// failures remain Main's responsibility; this never reads compressed data.
fn contents_controlled(path: &Path, cancelled: &AtomicBool) -> Result<Option<Contents>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let outer_len = outer_path(path)?.len();
    let mut file =
        std::fs::File::open(path).map_err(|e| DegaussError::io("opening archive", path, e))?;
    let file_size = file
        .metadata()
        .map_err(|e| DegaussError::io("reading archive size", path, e))?
        .len();
    let tail_len = usize::try_from(file_size.min(MAX_TRAILER))
        .map_err(|_| bad(path, "archive trailer does not fit this target"))?;
    let tail_start = file_size - tail_len as u64;
    let mut tail = vec![0; tail_len];
    read_at(&mut file, path, tail_start, &mut tail)?;
    let mut found = None;
    let mut failure = bad(path, "no valid end-of-directory record");
    for at in (0..tail.len().saturating_sub(21)).rev() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if tail[at..at + 4] != EOCD_SIGNATURE
            || at + 22 + usize::from(word(&tail, at + 20)) != tail.len()
        {
            continue;
        }
        match layout(&mut file, path, &tail[at..at + 22], tail_start + at as u64) {
            Ok(directory) => {
                found = Some((directory, at));
                break;
            }
            Err(error) => failure = error,
        }
    }
    let (directory, eocd) = found.ok_or(failure)?;
    let main_eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&at| tail[at..at + 4] == EOCD_SIGNATURE);
    let main_compatible = main_eocd == Some(eocd);
    let length = usize::try_from(directory.size)
        .map_err(|_| bad(path, "central directory does not fit this target"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| bad(path, "cannot allocate bounded central directory"))?;
    bytes.resize(length, 0);
    read_at(&mut file, path, directory.offset, &mut bytes)?;
    let count = usize::try_from(directory.count)
        .map_err(|_| bad(path, "entry count does not fit this target"))?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(count)
        .map_err(|_| bad(path, "cannot allocate bounded entry table"))?;
    let mut directories = Vec::new();
    let mut names = HashMap::<String, (String, bool)>::new();
    names
        .try_reserve(count)
        .map_err(|_| bad(path, "cannot allocate archive name table"))?;
    let mut skipped = Vec::new();
    let mut conflicts = HashSet::new();
    let mut cursor = 0usize;
    for _ in 0..count {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let header = bytes
            .get(cursor..cursor + 46)
            .ok_or_else(|| bad(path, "truncated central-directory header"))?;
        if header[..4] != CENTRAL_SIGNATURE {
            return Err(bad(path, "invalid central-directory signature"));
        }
        let name_len = usize::from(word(header, 28));
        let extra_len = usize::from(word(header, 30));
        let comment_len = usize::from(word(header, 32));
        let end = cursor + 46 + name_len + extra_len + comment_len;
        if end > length {
            return Err(bad(
                path,
                "truncated central-directory name, extra field or comment",
            ));
        }
        let raw = &bytes[cursor + 46..cursor + 46 + name_len];
        let mut extra = &bytes[cursor + 46 + name_len..cursor + 46 + name_len + extra_len];
        cursor = end;
        let flags = word(header, 8);
        let method = word(header, 10);
        let mut zip64 = None;
        let mut unicode = None;
        while !extra.is_empty() {
            if extra.len() < 4 {
                return Err(bad(path, "truncated extra-field header"));
            }
            let field_len = usize::from(word(extra, 2));
            if field_len + 4 > extra.len() {
                return Err(bad(path, "truncated extra-field value"));
            }
            match word(extra, 0) {
                1 => {
                    if zip64.is_some() {
                        return Err(bad(path, "duplicate ZIP64 extra field"));
                    }
                    zip64 = Some(&extra[4..4 + field_len]);
                }
                0x7075 => unicode = Some(&extra[4..4 + field_len]),
                _ => {}
            }
            extra = &extra[4 + field_len..];
        }
        // Main checks every record's ZIP64 values, disk and local-header
        // bounds when it opens the archive, whatever else is wrong with the
        // member, and refuses the whole archive when one fails: the same
        // checks run here before any member can be left out on its own.
        let mut size = u64::from(dword(header, 24));
        let mut compressed = u64::from(dword(header, 20));
        let mut local = u64::from(dword(header, 42));
        let disk = u32::from(word(header, 34));
        let mut zip64 = zip64.unwrap_or(&[]);
        for value in [&mut size, &mut compressed, &mut local] {
            if *value == u64::from(u32::MAX) {
                if zip64.len() < 8 {
                    return Err(bad(path, "missing or truncated ZIP64 entry value"));
                }
                *value = qword(zip64, 0);
                zip64 = &zip64[8..];
            }
        }
        // The disk number is never resolved from the ZIP64 field: Main
        // refuses the record outright when it is the sentinel, without
        // reading that value, so the sentinel is another disk here too.
        // Main also lets a record saying disk 1 into a disk-0 archive;
        // here every record has to say the directory's disk, which only
        // ever leaves an archive out, never offers one Main refuses.
        if disk != directory.disk {
            return Err(bad(path, "member starts on another disk"));
        }
        // PKWARE APPNOTE 4.5.3 permits these fields only when the corresponding
        // central-directory value is its ZIP64 sentinel.
        if !zip64.is_empty() {
            return Err(bad(path, "inconsistent ZIP64 extra-field values"));
        }
        // An encrypted stored member carries its 12-byte encryption header
        // in the compressed size, so the stored-size rule would fail it.
        // Main reads the method and the DOS time as one 32-bit word for
        // that rule and applies it only when both are zero, so such a
        // member opens in Main unless its time is zero: the rule is applied
        // to an encrypted member exactly when Main applies it.
        let stored_rule = method == 0 && (flags & 1 == 0 || dword(header, 10) == 0);
        if (stored_rule && size != compressed)
            || (size != 0 && compressed == 0)
            || local
                .checked_add(30)
                .and_then(|v| v.checked_add(name_len as u64))
                .and_then(|v| v.checked_add(compressed))
                .is_none_or(|v| v > directory.offset)
        {
            return Err(bad(
                path,
                format!(
                    "member {} has inconsistent size or local-header bounds",
                    shown(raw)
                ),
            ));
        }
        // A masked local header (bit 13) fails Main's central-directory
        // read for the whole archive; the encryption and patch bits below
        // are only refused when that member is extracted.
        if flags & 8192 != 0 {
            return Err(bad(
                path,
                format!(
                    "member {} has a masked local header, which Main refuses for the whole archive",
                    shown(raw)
                ),
            ));
        }
        // From here on a problem belongs to this member alone: it is left
        // out with its reason and the rest of the archive still stands.
        // Only raw bytes that are valid UTF-8 can reach Main unchanged
        // through the cache and the MGL; the UTF-8 flag (bit 11) says
        // whether the writer claimed that, and a decoded Unicode Path name
        // only identifies the member in the log.
        let name = match std::str::from_utf8(raw) {
            Ok(name) => name,
            Err(_) => {
                let reason = if flags & 2048 != 0 {
                    "member name is flagged UTF-8 but is not valid UTF-8"
                } else {
                    "unsupported legacy ZIP filename encoding"
                };
                skipped.push(Skipped {
                    member: raw.escape_ascii().to_string(),
                    reason: reason.to_string(),
                    detail: unicode
                        .and_then(|field| unicode_path(field, raw))
                        .map(|decoded| format!("Unicode Path {decoded:?}")),
                });
                continue;
            }
        };
        // Every addressable name takes part in the conflict pass whether or
        // not its member stays: Main's lookup lands on whichever record
        // matches the requested bytes, so a member cannot be offered beside
        // one it could be mistaken for, even one already left out.
        let is_dir = name.ends_with('/');
        let key = name.trim_end_matches('/').to_ascii_lowercase();
        match names.entry(key) {
            MapEntry::Occupied(taken) => {
                conflicts.insert(taken.key().clone());
            }
            MapEntry::Vacant(free) => {
                free.insert((name.to_string(), is_dir));
            }
        }
        // Main's extract iterator refuses bits 0, 6 and 5 together as
        // unsupported encryption; the two reasons are kept apart here
        // because the issue asks for the exact one, in the order Main's
        // directory scan tests them (encryption, then a compressed patch).
        let problem = if flags & (1 | 64) != 0 {
            Some("encrypted member is unsupported".to_string())
        } else if flags & 32 != 0 {
            Some("compressed-patch member is unsupported".to_string())
        } else if method != 0 && method != 8 {
            Some(format!("unsupported compression method {method}"))
        } else {
            member_name_problem(name, outer_len)
        };
        if let Some(reason) = problem {
            skipped.push(Skipped {
                member: name.to_string(),
                reason,
                detail: None,
            });
            continue;
        }
        if is_dir {
            // The slash stays until the conflict pass has seen the name.
            directories.push(name.to_string());
        } else {
            entries.push(Entry {
                name: name.to_string(),
                crc32: dword(header, 16),
                size,
            });
        }
    }
    if cursor != length {
        return Err(bad(
            path,
            "central-directory size does not match its entries",
        ));
    }
    // Check implied directories against explicit files and directory casing.
    // Sort once rather than retaining every prefix of every long member name.
    // A conflict is never resolved by choosing a side: the lowercase key of
    // the ambiguous path is noted and everything at or under it is left out.
    let mut paths: Vec<_> = names.iter().collect();
    paths.sort_unstable_by_key(|(left, _)| *left);
    for (_, (name, _)) in &paths {
        for (slash, _) in name.match_indices('/') {
            let prefix = &name[..slash];
            let prefix_key = prefix.to_ascii_lowercase();
            if let Some((existing, directory)) = names.get(&prefix_key) {
                if !directory || existing.trim_end_matches('/') != prefix {
                    conflicts.insert(prefix_key);
                }
            }
        }
    }
    for pair in paths.windows(2) {
        let (_, (left, _)) = pair[0];
        let (_, (right, _)) = pair[1];
        let mut end = 0;
        for (a, b) in left.split('/').zip(right.split('/')) {
            if !a.eq_ignore_ascii_case(b) {
                break;
            }
            end += a.len();
            if a != b {
                conflicts.insert(left[..end].to_ascii_lowercase());
                break;
            }
            end += 1;
        }
    }
    // Keys are only worked out again when there is a conflict to match.
    let conflict_of = |name: &str| -> Option<String> {
        let key = name.trim_end_matches('/').to_ascii_lowercase();
        key.match_indices('/')
            .map(|(slash, _)| slash)
            .chain(std::iter::once(key.len()))
            .map(|at| &key[..at])
            .find(|prefix| conflicts.contains(*prefix))
            .map(str::to_string)
    };
    if !conflicts.is_empty() {
        let conflicting = |name: String| {
            let group = conflict_of(&name).expect("matched by the same test");
            Skipped {
                member: name,
                reason: format!("duplicate or case-ambiguous member paths under {group:?}"),
                detail: None,
            }
        };
        skipped.extend(
            directories
                .extract_if(.., |name| conflict_of(name).is_some())
                .map(conflicting),
        );
        skipped.extend(
            entries
                .extract_if(.., |entry| conflict_of(&entry.name).is_some())
                .map(|entry| conflicting(entry.name)),
        );
    }
    for directory in &mut directories {
        directory.truncate(directory.trim_end_matches('/').len());
    }
    // Browse uses a stable case-insensitive sort. Exact Unicode directory names
    // can share that sort key, so their input order must not come from HashMap.
    directories.sort_unstable();
    // Every member left out goes to the log here, where the archive is
    // read, so whatever reads it (an index, an audit, a listing straight
    // from the card, a launch check) leaves the diagnostic once per read,
    // in one write however many members there are.
    if !skipped.is_empty() {
        let lines: Vec<String> = skipped
            .iter()
            .map(|skipped| format!("zip          {}: {}", path.display(), skipped.describe()))
            .collect();
        crate::note(&lines.join("\n"));
    }
    Ok(Some(Contents {
        entries,
        directories,
        skipped,
        main_compatible,
    }))
}

/// Real stored ZIP bytes shared by boundary/compatibility tests.
#[cfg(test)]
pub fn tests_archive(names: &[&str], zip64: bool) -> Vec<u8> {
    let entries: Vec<_> = names
        .iter()
        .map(|name| TestEntry {
            name: name.as_bytes(),
            flags: 0,
            method: 0,
            extra: &[],
        })
        .collect();
    tests_archive_entries(&entries, zip64)
}

/// One member of a test archive: its raw name bytes, general-purpose flags,
/// compression method and central-directory extra field.
#[cfg(test)]
pub struct TestEntry<'a> {
    pub name: &'a [u8],
    pub flags: u16,
    pub method: u16,
    pub extra: &'a [u8],
}

#[cfg(test)]
pub fn tests_archive_entries(entries: &[TestEntry<'_>], zip64: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut directory = Vec::new();
    for entry in entries {
        let offset = bytes.len() as u64;
        let name = entry.name;
        let mut local = [0u8; 30];
        local[..4].copy_from_slice(&[0x50, 0x4b, 3, 4]);
        local[4..6].copy_from_slice(&20u16.to_le_bytes());
        local[6..8].copy_from_slice(&entry.flags.to_le_bytes());
        local[8..10].copy_from_slice(&entry.method.to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&local);
        bytes.extend_from_slice(name);
        let mut central = [0u8; 46];
        central[..4].copy_from_slice(&CENTRAL_SIGNATURE);
        central[4..6].copy_from_slice(&20u16.to_le_bytes());
        central[6..8].copy_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
        central[8..10].copy_from_slice(&entry.flags.to_le_bytes());
        central[10..12].copy_from_slice(&entry.method.to_le_bytes());
        central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        let extra_len = entry.extra.len() + if zip64 { 28 } else { 0 };
        central[30..32].copy_from_slice(&(extra_len as u16).to_le_bytes());
        if zip64 {
            central[20..28].fill(0xff);
            central[42..46].fill(0xff);
        } else {
            central[42..46].copy_from_slice(&(offset as u32).to_le_bytes());
        }
        directory.extend_from_slice(&central);
        directory.extend_from_slice(name);
        directory.extend_from_slice(entry.extra);
        if zip64 {
            directory.extend_from_slice(&1u16.to_le_bytes());
            directory.extend_from_slice(&24u16.to_le_bytes());
            directory.extend_from_slice(&0u64.to_le_bytes());
            directory.extend_from_slice(&0u64.to_le_bytes());
            directory.extend_from_slice(&offset.to_le_bytes());
        }
    }
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(&directory);
    if zip64 {
        let record_offset = bytes.len() as u64;
        bytes.extend_from_slice(&[0x50, 0x4b, 6, 6]);
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&45u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(directory.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&[0x50, 0x4b, 6, 7]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&record_offset.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
    }
    bytes.extend_from_slice(&EOCD_SIGNATURE);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let count = if zip64 {
        u16::MAX
    } else {
        entries.len() as u16
    };
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(
        &(if zip64 {
            u32::MAX
        } else {
            directory.len() as u32
        })
        .to_le_bytes(),
    );
    bytes.extend_from_slice(&(if zip64 { u32::MAX } else { offset as u32 }).to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes
}

/// The test archive, shared with the catalog's tests so both exercise the
/// same real bytes rather than two hand-made approximations.
#[cfg(test)]
pub fn tests_fixture() -> &'static [u8] {
    tests::ZIP_FIXTURE
}

#[cfg(test)]
mod tests {
    use super::*;

    // Three files, one of them in a subfolder, built with a real zip writer.
    pub(super) const ZIP_FIXTURE: &[u8] = &[
        0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x6a, 0xaa, 0x17, 0x5d, 0xd2,
        0x8f, 0x34, 0xb5, 0x07, 0x00, 0x00, 0x00, 0x2c, 0x01, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00,
        0x4d, 0x65, 0x74, 0x61, 0x6c, 0x20, 0x53, 0x6c, 0x75, 0x67, 0x2e, 0x6e, 0x65, 0x6f, 0x63,
        0x60, 0x18, 0x05, 0xc4, 0x02, 0x00, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x6a, 0xaa, 0x17, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x07, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x72, 0x65, 0x61, 0x64, 0x6d, 0x65, 0x2e, 0x74, 0x78,
        0x74, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00,
        0x00, 0x08, 0x00, 0x6a, 0xaa, 0x17, 0x5d, 0x15, 0x96, 0x2c, 0x6a, 0x06, 0x00, 0x00, 0x00,
        0x78, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x73, 0x75, 0x62, 0x2f, 0x41, 0x6e, 0x6f,
        0x74, 0x68, 0x65, 0x72, 0x20, 0x47, 0x61, 0x6d, 0x65, 0x2e, 0x6e, 0x65, 0x6f, 0x63, 0x64,
        0x1c, 0x18, 0x00, 0x00, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x6a, 0xaa, 0x17, 0x5d, 0xd2, 0x8f, 0x34, 0xb5, 0x07, 0x00, 0x00, 0x00, 0x2c, 0x01,
        0x00, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x4d, 0x65, 0x74, 0x61, 0x6c, 0x20, 0x53, 0x6c, 0x75, 0x67,
        0x2e, 0x6e, 0x65, 0x6f, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x6a, 0xaa, 0x17, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x07, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
        0x01, 0x33, 0x00, 0x00, 0x00, 0x72, 0x65, 0x61, 0x64, 0x6d, 0x65, 0x2e, 0x74, 0x78, 0x74,
        0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x6a, 0xaa, 0x17,
        0x5d, 0x15, 0x96, 0x2c, 0x6a, 0x06, 0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x14, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x62, 0x00, 0x00,
        0x00, 0x73, 0x75, 0x62, 0x2f, 0x41, 0x6e, 0x6f, 0x74, 0x68, 0x65, 0x72, 0x20, 0x47, 0x61,
        0x6d, 0x65, 0x2e, 0x6e, 0x65, 0x6f, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x03,
        0x00, 0x03, 0x00, 0xb6, 0x00, 0x00, 0x00, 0x9a, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    fn write_fixture(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("degauss-zip-{tag}-{}.zip", std::process::id()));
        std::fs::write(&path, bytes).expect("fixture written");
        path
    }

    #[test]
    fn every_file_in_an_archive_is_listed_including_ones_in_subfolders() {
        let path = write_fixture("list", ZIP_FIXTURE);
        let names = list(&path).expect("archive lists");

        assert_eq!(
            names,
            vec![
                "Metal Slug.neo".to_string(),
                "readme.txt".to_string(),
                "sub/Another Game.neo".to_string()
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_pre_cancelled_archive_read_does_not_open_the_file() {
        let missing = std::env::temp_dir().join(format!(
            "degauss-zip-cancelled-missing-{}.zip",
            std::process::id()
        ));
        std::fs::remove_file(&missing).ok();
        let cancelled = AtomicBool::new(true);
        assert!(contents_controlled(&missing, &cancelled).unwrap().is_none());
    }

    #[test]
    fn a_file_that_is_not_an_archive_is_an_error_not_an_empty_list() {
        // An empty list would look like an archive with nothing in it, and
        // the games inside would silently disappear from the library.
        let path = write_fixture("garbage", b"this is not a zip file at all");
        let err = list(&path).expect_err("must not accept garbage");
        assert!(err.to_string().contains("end-of-directory"), "got: {err}");
        std::fs::remove_file(&path).ok();
    }

    /// The offset of the central directory is a number taken straight out of
    /// the file. A corrupt one used to be sliced with, which panics, and a
    /// panic here is not an error the caller can handle: the release profile
    /// aborts, and this process is the whole menu. A file on the card must
    /// never be able to take the screen away.
    #[test]
    fn a_directory_offset_past_the_end_is_an_error_not_a_crash() {
        let mut bytes = ZIP_FIXTURE.to_vec();
        let eocd = bytes
            .windows(4)
            .rposition(|w| w == EOCD_SIGNATURE)
            .expect("fixture has an end-of-directory record");
        // Point the directory a byte past the end of the file.
        let past = (bytes.len() as u32) + 1;
        bytes[eocd + 16..eocd + 20].copy_from_slice(&past.to_le_bytes());
        let path = write_fixture("bad-offset", &bytes);
        let outcome = list(&path);
        let _ = std::fs::remove_file(&path);
        assert!(
            outcome.is_err(),
            "a directory offset past the end must be reported, got {outcome:?}"
        );
    }

    /// The same number, at its maximum, on a small file.
    #[test]
    fn a_wildly_wrong_directory_offset_is_an_error_not_a_crash() {
        let mut bytes = ZIP_FIXTURE.to_vec();
        let eocd = bytes
            .windows(4)
            .rposition(|w| w == EOCD_SIGNATURE)
            .expect("fixture has an end-of-directory record");
        bytes[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        let path = write_fixture("huge-offset", &bytes);
        let outcome = list(&path);
        let _ = std::fs::remove_file(&path);
        assert!(outcome.is_err(), "got {outcome:?}");
    }

    /// The module promises a damaged archive is an error, never an empty
    /// list: a folder of games must not quietly become a folder of nothing.
    #[test]
    fn a_damaged_directory_is_an_error_not_an_empty_list() {
        let mut bytes = ZIP_FIXTURE.to_vec();
        let first = bytes
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE)
            .expect("fixture has a central directory");
        bytes[first] = 0x00; // break the first header's signature
        let path = write_fixture("damaged-directory", &bytes);
        let outcome = list(&path);
        let _ = std::fs::remove_file(&path);
        assert!(
            outcome.is_err(),
            "a damaged directory must be reported, got {outcome:?}"
        );
    }

    #[test]
    fn a_truncated_archive_is_reported() {
        let path = write_fixture("truncated", &ZIP_FIXTURE[..120]);
        assert!(list(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_archive_names_the_file_it_could_not_open() {
        let err = list(Path::new("/definitely/not/here.zip")).expect_err("must fail");
        assert!(err.to_string().contains("here.zip"), "got: {err}");
    }
    #[test]
    fn explicit_directory_order_is_stable_when_browse_sort_keys_tie() {
        let path = write_fixture(
            "directory-order",
            &tests_archive(&["ä/", "Ä/", "ö/", "Ö/", "ü/", "Ü/"], false),
        );
        for _ in 0..8 {
            let contents = contents_controlled(&path, &AtomicBool::new(false))
                .unwrap()
                .unwrap();
            let mut names = contents.directories;
            names.sort_by_key(|name| name.to_lowercase());
            assert_eq!(names, ["Ä", "ä", "Ö", "ö", "Ü", "ü"]);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn zip64_entries_preserve_exact_identity_size_and_crc() {
        let path = write_fixture(
            "zip64",
            &tests_archive(&["root.neo", "folder/nested.neo"], true),
        );
        let entries = entries(&path).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.name.as_str(), e.size, e.crc32))
                .collect::<Vec<_>>(),
            vec![("root.neo", 0, 0), ("folder/nested.neo", 0, 0)]
        );
        assert_eq!(
            validate_member(&path.join("folder/nested.neo")).unwrap(),
            entries[1]
        );
        assert!(validate_member(&path.join("missing.neo"))
            .unwrap_err()
            .to_string()
            .contains("missing.neo"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn comment_signatures_do_not_hide_the_structural_end_record() {
        let mut bytes = tests_archive(&["game.neo"], false);
        let at = bytes.len() - 2;
        let comment = b"comment PK\x05\x06 with a false signature";
        bytes[at..].copy_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend_from_slice(comment);
        let path = write_fixture("comment", &bytes);
        assert_eq!(list(&path).unwrap(), ["game.neo"]);
        assert!(validate_member(&path.join("game.neo")).is_ok());
        let error = validate_member_for_launch(&path.join("game.neo"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("archive comment"));
        assert!(error.contains(path.to_str().unwrap()));
        std::fs::remove_file(path).unwrap();
    }

    fn skipped_of(contents: &Contents) -> Vec<(&str, &str)> {
        contents
            .skipped
            .iter()
            .map(|skipped| (skipped.member.as_str(), skipped.reason.as_str()))
            .collect()
    }

    fn names_of(contents: &Contents) -> Vec<&str> {
        contents
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    /// A name Main cannot address, or one that could pick the wrong member,
    /// is left out on its own with its reason. The healthy sibling stays: one
    /// bad name must not empty an archive of games, and a conflicting group
    /// is left out whole rather than resolved by choosing a side.
    #[test]
    fn invalid_names_cannot_change_identity_or_choose_the_wrong_member() {
        let overlong = format!("{}.neo", "n".repeat(257));
        let deep = format!("{}game.neo", "a/".repeat(520));
        for (index, (names, left_out, reason)) in [
            (
                vec!["../game.neo", "ok.neo"],
                1,
                "traversal or empty path segment",
            ),
            (
                vec!["/game.neo", "ok.neo"],
                1,
                "traversal or empty path segment",
            ),
            (
                vec!["a//game.neo", "ok.neo"],
                1,
                "traversal or empty path segment",
            ),
            (
                vec!["a/./game.neo", "ok.neo"],
                1,
                "traversal or empty path segment",
            ),
            (vec!["a\\game.neo", "ok.neo"], 1, "backslash or colon"),
            (vec!["a:b.neo", "ok.neo"], 1, "backslash or colon"),
            (vec!["game.neo\n", "ok.neo"], 1, "control character"),
            (
                vec![" game.neo", "ok.neo"],
                1,
                "starts or ends with whitespace",
            ),
            (
                vec!["game.neo ", "ok.neo"],
                1,
                "starts or ends with whitespace",
            ),
            (vec![overlong.as_str(), "ok.neo"], 1, "260-byte"),
            (
                vec![deep.as_str(), "ok.neo"],
                1,
                "native target exceeds 1023 bytes",
            ),
            (
                vec!["nested.zip/game.neo", "ok.neo"],
                1,
                "nested archive member is unsupported by MiSTer Main",
            ),
            (
                vec!["nested.zip", "ok.neo"],
                1,
                "nested archive member is unsupported by MiSTer Main",
            ),
            (
                vec!["game.neo", "game.neo", "ok.neo"],
                2,
                "duplicate or case-ambiguous member paths under \"game.neo\"",
            ),
            (
                vec!["game.neo", "GAME.neo", "ok.neo"],
                2,
                "duplicate or case-ambiguous member paths under \"game.neo\"",
            ),
            (
                vec!["Folder/a.neo", "Folder/b.neo", "folder/z.neo", "ok.neo"],
                3,
                "duplicate or case-ambiguous member paths under \"folder\"",
            ),
            (
                vec!["folder", "folder/game.neo", "ok.neo"],
                2,
                "duplicate or case-ambiguous member paths under \"folder\"",
            ),
            (
                vec!["Folder/", "folder/game.neo", "ok.neo"],
                2,
                "duplicate or case-ambiguous member paths under \"folder\"",
            ),
            (
                vec!["x/Folder/a.neo", "x/folder/b.neo", "x/other.neo", "ok.neo"],
                2,
                "duplicate or case-ambiguous member paths under \"x/folder\"",
            ),
        ]
        .iter()
        .enumerate()
        {
            let path = write_fixture(&format!("names-{index}"), &tests_archive(names, false));
            let contents = contents(&path).unwrap_or_else(|error| panic!("{names:?}: {error}"));
            assert_eq!(
                names_of(&contents),
                names[*left_out..].to_vec(),
                "retained members for {names:?}"
            );
            let skipped = skipped_of(&contents);
            let mut left_out_members: Vec<_> = skipped.iter().map(|(member, _)| *member).collect();
            left_out_members.sort_unstable();
            let mut expected = names[..*left_out].to_vec();
            expected.sort_unstable();
            assert_eq!(left_out_members, expected, "skipped members for {names:?}");
            for (member, why) in &skipped {
                assert!(why.contains(reason), "{member}: {why}");
            }
            std::fs::remove_file(path).unwrap();
        }
    }

    /// A name left out for a control character keeps that character in the
    /// record, so a favourite naming the member is told why it was skipped
    /// rather than that it was renamed; the log and audit lines escape it,
    /// because a newline in a name would otherwise split the line.
    #[test]
    fn a_control_character_in_a_skipped_name_is_escaped_only_where_it_is_printed() {
        let path = write_fixture(
            "control-name",
            &tests_archive(&["game.neo\n", "ok.neo"], false),
        );
        let contents = contents(&path).unwrap();
        assert_eq!(names_of(&contents), ["ok.neo"]);
        let skipped = &contents.skipped[0];
        assert_eq!(skipped.member, "game.neo\n");
        assert_eq!(
            skipped.describe(),
            "member game.neo\\n: name holds a control character"
        );
        let error = validate_member(&path.join("game.neo\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("control character"), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    /// Main finds a member by comparing the requested bytes with every
    /// record, ASCII case-insensitively, and a member already left out is
    /// still a record it can land on. A supported member that shares a
    /// name with an encrypted or unusually compressed one is therefore a
    /// conflict too, and the whole group goes, never one side of it.
    #[test]
    fn a_retained_member_cannot_share_a_name_with_a_skipped_one() {
        for (index, (flags, method, reason)) in [
            (1u16, 0u16, "encrypted member is unsupported"),
            (0, 12, "unsupported compression method 12"),
        ]
        .into_iter()
        .enumerate()
        {
            let entries = [
                TestEntry {
                    name: b"Game.neo",
                    flags,
                    method,
                    extra: &[],
                },
                TestEntry {
                    name: b"game.neo",
                    flags: 0,
                    method: 0,
                    extra: &[],
                },
                TestEntry {
                    name: b"ok.neo",
                    flags: 0,
                    method: 0,
                    extra: &[],
                },
            ];
            let path = write_fixture(
                &format!("skipped-conflict-{index}"),
                &tests_archive_entries(&entries, false),
            );
            let contents = contents(&path).unwrap();
            assert_eq!(names_of(&contents), ["ok.neo"], "case {index}");
            let skipped = skipped_of(&contents);
            assert_eq!(skipped.len(), 2, "case {index}: {skipped:?}");
            assert_eq!(skipped[0], ("Game.neo", reason), "case {index}");
            assert_eq!(
                skipped[1],
                (
                    "game.neo",
                    "duplicate or case-ambiguous member paths under \"game.neo\""
                ),
                "case {index}"
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    /// Main checks every record's local-header bounds and stored sizes when
    /// it opens an archive and refuses the whole archive on a failure, even
    /// for a member it could never extract. A member Degauss leaves out for
    /// its own reason therefore still fails the archive when its record is
    /// inconsistent: otherwise the siblings would be offered as launchable
    /// from an archive Main cannot open.
    #[test]
    fn a_skipped_member_with_an_inconsistent_record_still_fails_the_archive() {
        for (index, (name, flags, method)) in [
            (&b"game.neo"[..], 1u16, 0u16),
            (b"game.neo", 0, 12),
            (b"leg\xe9.neo", 0, 0),
            (b"inner.zip", 0, 0),
        ]
        .into_iter()
        .enumerate()
        {
            let members = [
                TestEntry {
                    name,
                    flags,
                    method,
                    extra: &[],
                },
                TestEntry {
                    name: b"plain.neo",
                    flags: 0,
                    method: 0,
                    extra: &[],
                },
            ];
            let original = tests_archive_entries(&members, false);
            let central = original
                .windows(4)
                .position(|b| b == CENTRAL_SIGNATURE)
                .unwrap();
            // The local header offset past the directory; an uncompressed
            // size with no stored bytes; and, for a stored member, the
            // compressed size an encryption header would give it.
            let mut mutations = vec![("offset", 42, 0xf0u8), ("size", 24, 12)];
            if method == 0 {
                mutations.push(("header", 20, 12));
            }
            for (tag, offset, value) in mutations {
                let mut bytes = original.clone();
                bytes[central + offset] = value;
                let path = write_fixture(&format!("skipped-record-{index}-{tag}"), &bytes);
                let error = entries(&path).unwrap_err().to_string();
                assert!(
                    error.contains("inconsistent size or local-header bounds"),
                    "case {index} {tag}: {error}"
                );
                std::fs::remove_file(path).unwrap();
            }
        }
    }

    /// Launch confirmation and favourites name the real cause for a member
    /// the archive holds but Main cannot use, which is a different repair
    /// from a member that was renamed away.
    #[test]
    fn a_skipped_member_reports_its_reason_instead_of_looking_renamed() {
        let path = write_fixture(
            "skipped-launch",
            &tests_archive(&["inner.zip", "game.neo"], false),
        );
        assert_eq!(list(&path).unwrap(), ["game.neo"]);
        assert!(validate_member_for_launch(&path.join("game.neo")).is_ok());
        for outcome in [
            validate_member(&path.join("inner.zip")),
            validate_member_for_launch(&path.join("inner.zip")),
        ] {
            let error = outcome.unwrap_err().to_string();
            assert!(
                error.contains("nested archive member is unsupported"),
                "{error}"
            );
            assert!(error.contains("inner.zip"), "{error}");
            assert!(!error.contains("missing or was renamed"), "{error}");
        }
        let error = validate_member(&path.join("gone.neo"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing or was renamed"), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_zip64_chains_and_entry_fields_fail_explicitly() {
        let original = tests_archive(&["game.neo"], true);
        let central = original
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        let end64 = original
            .windows(4)
            .position(|b| b == [0x50, 0x4b, 6, 6])
            .unwrap();
        let locator = end64 + 56;
        for (index, (offset, value)) in [
            (end64, 0),
            (end64 + 4, 43),
            (end64 + 16, 2),
            (end64 + 32, 2),
            (locator, 0),
            (locator + 16, 2),
            (central + 46 + "game.neo".len(), 2),
            (central + 46 + "game.neo".len() + 2, 23),
        ]
        .into_iter()
        .enumerate()
        {
            let mut bytes = original.clone();
            bytes[offset] = value;
            let path = write_fixture(&format!("broken64-{index}"), &bytes);
            assert!(entries(&path).is_err(), "accepted mutation {index}");
            std::fs::remove_file(path).unwrap();
        }
    }

    /// Main opens a classic archive only when its end record says disk 0
    /// or disk 1 throughout, and refuses any record whose disk number is
    /// the ZIP64 sentinel, which it never resolves, or is neither that
    /// disk nor 1. A member Main would not open the archive for cannot be
    /// offered, so each of those is the whole archive's failure here. A
    /// record saying disk 1 in a disk-0 archive, which Main lets through,
    /// is a multi-disk structure all the same and fails here too: that
    /// leaves out an archive, never offers one Main refuses. The
    /// disk-1-throughout layout still lists.
    #[test]
    fn multi_disk_end_records_and_member_disk_numbers_fail_the_archive() {
        for (index, (zip64, at_end, value, reason)) in [
            (
                false,
                Some(4),
                1u8,
                "multi-disk archive or inconsistent entry counts",
            ),
            (
                false,
                Some(6),
                1,
                "multi-disk archive or inconsistent entry counts",
            ),
            (false, None, 1, "member starts on another disk"),
            (false, None, 0xff, "member starts on another disk"),
            (true, None, 0xff, "member starts on another disk"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut bytes = tests_archive(&["game.neo", "plain.neo"], zip64);
            let eocd = bytes.len() - 22;
            let central = bytes
                .windows(4)
                .position(|b| b == CENTRAL_SIGNATURE)
                .unwrap();
            match at_end {
                Some(offset) => bytes[eocd + offset] = value,
                None => bytes[central + 34..central + 36].fill(value),
            }
            let path = write_fixture(&format!("multi-disk-{index}"), &bytes);
            let error = entries(&path).unwrap_err().to_string();
            assert!(error.contains(reason), "mutation {index}: {error}");
            std::fs::remove_file(path).unwrap();
        }
        let mut bytes = tests_archive(&["game.neo"], false);
        let eocd = bytes.len() - 22;
        bytes[eocd + 4] = 1;
        bytes[eocd + 6] = 1;
        let path = write_fixture("disk-one-throughout", &bytes);
        let error = entries(&path).unwrap_err().to_string();
        assert!(error.contains("member starts on another disk"), "{error}");
        let central = bytes
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        bytes[central + 34] = 1;
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(list(&path).unwrap(), ["game.neo"]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn zip64_values_without_corresponding_sentinels_are_rejected() {
        let mut bytes = tests_archive(&["game.neo"], true);
        let central = bytes
            .windows(4)
            .position(|header| header == CENTRAL_SIGNATURE)
            .unwrap();
        // The empty member fits in the classic compressed-size field. Keeping
        // its ZIP64 value anyway violates APPNOTE 4.5.3's conditional layout.
        bytes[central + 20..central + 24].fill(0);
        let path = write_fixture("surplus64", &bytes);
        assert!(entries(&path)
            .unwrap_err()
            .to_string()
            .contains("inconsistent ZIP64 extra-field values"));
        std::fs::remove_file(path).unwrap();
    }

    /// The stored-size rule is one Main applies at open to every record,
    /// including an encrypted one, whose compressed size holds a 12-byte
    /// encryption header, but only through a 32-bit read of the method and
    /// the DOS time together. With a zero time Main refuses the archive and
    /// so must Degauss; with any other time Main opens it and the member is
    /// a skip of its own while the plain sibling stays launchable.
    #[test]
    fn an_encrypted_stored_member_fails_the_archive_only_when_main_refuses_it() {
        let original = tests_archive(&["game.neo", "plain.neo"], false);
        let central = original
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        let mut bytes = original.clone();
        bytes[central + 8] = 1;
        bytes[central + 20] = 12;
        let path = write_fixture("encrypted-stored-time-zero", &bytes);
        let error = entries(&path).unwrap_err().to_string();
        assert!(
            error.contains("inconsistent size or local-header bounds"),
            "{error}"
        );
        std::fs::remove_file(path).unwrap();
        bytes[central + 12] = 1;
        let path = write_fixture("encrypted-stored-timed", &bytes);
        let contents = contents(&path).unwrap();
        assert_eq!(names_of(&contents), ["plain.neo"]);
        assert_eq!(
            skipped_of(&contents),
            [("game.neo", "encrypted member is unsupported")]
        );
        std::fs::remove_file(path).unwrap();
    }

    /// Encryption, patched data and other compression methods are Main
    /// reader limits of one member: that member is left out with the exact
    /// limit named, and the plain member beside it is still listed.
    #[test]
    fn unsupported_methods_and_flags_skip_only_that_member() {
        let original = tests_archive(&["game.neo", "plain.neo"], false);
        let central = original
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        for (index, (offset, value, reason)) in [
            (8, 1, "encrypted member is unsupported"),
            (8, 32, "compressed-patch member is unsupported"),
            (8, 64, "encrypted member is unsupported"),
            (8, 33, "encrypted member is unsupported"),
            (10, 12, "unsupported compression method 12"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut bytes = original.clone();
            bytes[central + offset] = value;
            let path = write_fixture(&format!("unsupported-{index}"), &bytes);
            let contents = contents(&path).unwrap();
            assert_eq!(names_of(&contents), ["plain.neo"], "mutation {index}");
            assert_eq!(
                skipped_of(&contents),
                [("game.neo", reason)],
                "mutation {index}"
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    /// Bit 13 of the general-purpose flags (a masked local header) is the
    /// one member flag Main's central-directory read refuses the whole
    /// archive for, so the plain sibling cannot be offered either: Main
    /// would not open the archive to launch it.
    #[test]
    fn a_masked_local_header_is_an_archive_error() {
        let mut bytes = tests_archive(&["game.neo", "plain.neo"], false);
        let central = bytes
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        bytes[central + 9] = 32;
        let path = write_fixture("masked", &bytes);
        let error = entries(&path).unwrap_err().to_string();
        assert!(error.contains("masked local header"), "{error}");
        assert!(error.contains("\"game.neo\""), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    /// A comment length that misplaces the next header is the directory not
    /// holding together, which is the whole archive's problem: no member of
    /// it is trusted, however healthy the others look.
    #[test]
    fn a_malformed_directory_length_is_an_archive_error() {
        let mut bytes = tests_archive(&["game.neo", "plain.neo"], false);
        let central = bytes
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        bytes[central + 32] = 1;
        let path = write_fixture("bad-comment-length", &bytes);
        let error = entries(&path).unwrap_err().to_string();
        assert!(error.contains("central-directory"), "{error}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn more_than_65535_entries_are_reachable_within_the_resource_limit() {
        let names: Vec<_> = (0..65_536).map(|n| format!("{n}.neo")).collect();
        let refs: Vec<_> = names.iter().map(String::as_str).collect();
        let path = write_fixture("count64", &tests_archive(&refs, true));
        assert_eq!(entries(&path).unwrap().len(), 65_536);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sparse_archive_directory_above_four_gib_uses_real_large_file_seeks() {
        use std::io::Write;
        let bytes = tests_archive(&["game.neo"], true);
        let central = bytes
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        let mut ending = bytes[central..].to_vec();
        let end64 = ending
            .windows(4)
            .position(|b| b == [0x50, 0x4b, 6, 6])
            .unwrap();
        let offset = u64::from(u32::MAX) + 65;
        ending[end64 + 48..end64 + 56].copy_from_slice(&offset.to_le_bytes());
        ending[end64 + 64..end64 + 72].copy_from_slice(&(offset + end64 as u64).to_le_bytes());
        let path = write_fixture("sparse64", &bytes[..central]);
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(&ending).unwrap();
        drop(file);
        assert_eq!(list(&path).unwrap(), ["game.neo"]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn archive_cache_observes_replacement_and_never_uses_a_missing_archive() {
        let path = write_fixture("cache-replace", &tests_archive(&["old.neo"], false));
        let mut cache = ArchiveCache::default();
        assert_eq!(cache.read(&path).unwrap().entries[0].name, "old.neo");
        std::fs::remove_file(&path).unwrap();
        assert!(cache.read(&path).is_err());
        std::fs::write(&path, tests_archive(&["new.neo"], false)).unwrap();
        assert_eq!(cache.read(&path).unwrap().entries[0].name, "new.neo");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn resource_limits_are_explicit_before_any_large_allocation() {
        let original = tests_archive(&["game.neo"], true);
        let at = original
            .windows(4)
            .position(|b| b == [0x50, 0x4b, 6, 6])
            .unwrap();
        for (tag, offset, value) in [
            ("count-limit", 32, MAX_ENTRIES + 1),
            ("directory-limit", 40, MAX_DIRECTORY_BYTES + 1),
        ] {
            let mut bytes = original.clone();
            bytes[at + offset..at + offset + 8].copy_from_slice(&value.to_le_bytes());
            if offset == 32 {
                bytes[at + 24..at + 32].copy_from_slice(&value.to_le_bytes());
            }
            let path = write_fixture(tag, &bytes);
            let error = entries(&path).unwrap_err().to_string();
            assert!(error.contains("resource limit"), "{error}");
            assert!(error.contains(path.to_str().unwrap()));
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn duplicate_zip64_extra_fields_are_rejected() {
        let mut bytes = tests_archive(&["game.neo"], true);
        let central = bytes
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        let end64 = bytes
            .windows(4)
            .position(|b| b == [0x50, 0x4b, 6, 6])
            .unwrap();
        let extra_start = central + 46 + "game.neo".len();
        let duplicate = bytes[extra_start..extra_start + 28].to_vec();
        bytes.splice(end64..end64, duplicate);
        bytes[central + 30..central + 32].copy_from_slice(&56u16.to_le_bytes());
        let end64 = end64 + 28;
        let size = qword(&bytes, end64 + 40) + 28;
        bytes[end64 + 40..end64 + 48].copy_from_slice(&size.to_le_bytes());
        bytes[end64 + 64..end64 + 72].copy_from_slice(&(end64 as u64).to_le_bytes());
        let path = write_fixture("duplicate64", &bytes);
        assert!(entries(&path)
            .unwrap_err()
            .to_string()
            .contains("duplicate ZIP64"));
        std::fs::remove_file(path).unwrap();
    }

    /// Main finds a member by its raw central-directory bytes, so only raw
    /// bytes that are valid UTF-8 can travel through the cache and the MGL
    /// unchanged. A legacy encoding is left out and named as such, never
    /// decoded into a name that would not resolve; the Unicode Path field is
    /// checked and used only to say which member that was; a name the writer
    /// flagged as UTF-8 that is not gets its own reason; and none of this
    /// fails the archive or produces a replacement character.
    #[test]
    fn filename_encodings_are_honoured_without_lossy_decoding() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926, "CRC-32 check value");
        let field = |version: u8, raw: &[u8], decoded: &str, stale: bool| -> Vec<u8> {
            let mut value = vec![version];
            value.extend_from_slice(&(crc32(raw) ^ if stale { 1 } else { 0 }).to_le_bytes());
            value.extend_from_slice(decoded.as_bytes());
            let mut extra = vec![0x75, 0x70];
            extra.extend_from_slice(&(value.len() as u16).to_le_bytes());
            extra.extend_from_slice(&value);
            extra
        };
        let described = field(1, b"two\xe9.neo", "two\u{e9}.neo", false);
        let stale = field(1, b"three\xe9.neo", "three\u{e9}.neo", true);
        let unknown_version = field(2, b"four\xe9.neo", "four\u{e9}.neo", false);
        let redundant = field(1, b"plain.neo", "plain.neo", false);
        let entries = [
            TestEntry {
                name: b"one\xe9.neo",
                flags: 0,
                method: 0,
                extra: &[],
            },
            TestEntry {
                name: "\u{e4}.neo".as_bytes(),
                flags: 2048,
                method: 0,
                extra: &[],
            },
            TestEntry {
                name: b"two\xe9.neo",
                flags: 0,
                method: 0,
                extra: &described,
            },
            TestEntry {
                name: b"three\xe9.neo",
                flags: 0,
                method: 0,
                extra: &stale,
            },
            TestEntry {
                name: b"four\xe9.neo",
                flags: 0,
                method: 0,
                extra: &unknown_version,
            },
            TestEntry {
                name: b"five\xe9.neo",
                flags: 2048,
                method: 0,
                extra: &[],
            },
            TestEntry {
                name: b"plain.neo",
                flags: 0,
                method: 0,
                extra: &redundant,
            },
            TestEntry {
                name: b"kept.neo",
                flags: 0,
                method: 0,
                extra: &[0x75, 0x70, 2, 0, 1, 0],
            },
        ];
        let path = write_fixture("encodings", &tests_archive_entries(&entries, false));
        let contents = contents(&path).unwrap();
        assert_eq!(
            names_of(&contents),
            ["\u{e4}.neo", "plain.neo", "kept.neo"],
            "the flagged UTF-8 name is byte-exact, and a Unicode Path field a valid name does not need never removes it"
        );
        assert_eq!(
            skipped_of(&contents),
            [
                ("one\\xe9.neo", "unsupported legacy ZIP filename encoding"),
                ("two\\xe9.neo", "unsupported legacy ZIP filename encoding"),
                ("three\\xe9.neo", "unsupported legacy ZIP filename encoding"),
                ("four\\xe9.neo", "unsupported legacy ZIP filename encoding"),
                (
                    "five\\xe9.neo",
                    "member name is flagged UTF-8 but is not valid UTF-8"
                ),
            ],
            "one reason per class, so a summary can count the members under it"
        );
        assert_eq!(
            contents
                .skipped
                .iter()
                .map(|skipped| skipped.detail.as_deref())
                .collect::<Vec<_>>(),
            [
                None,
                Some("Unicode Path \"two\u{e9}.neo\""),
                None,
                None,
                None
            ],
            "only the valid Unicode Path field identifies its member, and only in the log"
        );
        assert_eq!(
            contents.skipped[1].describe(),
            "member two\\xe9.neo: unsupported legacy ZIP filename encoding; Unicode Path \"two\u{e9}.neo\""
        );
        for text in contents
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .chain(
                contents
                    .skipped
                    .iter()
                    .map(|skipped| skipped.member.as_str()),
            )
        {
            assert!(!text.contains('\u{fffd}'), "{text}");
            assert!(!text.contains("malformed"), "{text}");
        }
        std::fs::remove_file(path).unwrap();
    }

    /// The archive path is Main's first `.zip` split, so an earlier `.zip`
    /// in it or a control character makes every member unaddressable: that
    /// is the archive's problem, not any member's.
    #[test]
    fn an_ambiguous_outer_path_is_an_archive_error() {
        let dir =
            std::env::temp_dir().join(format!("degauss-zip-outer-{}.zip", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("inner.zip");
        std::fs::write(&path, tests_archive(&["game.neo"], false)).unwrap();
        let error = entries(&path).unwrap_err().to_string();
        assert!(error.contains("earlier `.zip`"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
