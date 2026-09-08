//! Read the ZIP central directory without extracting file data.
//!
//! MiSTer opens the native `archive.zip/member` target. Its reader supports
//! stored and deflated single-disk classic/ZIP64 archives. Names must survive
//! our UTF-8 state/XML paths and MiSTer's ASCII-insensitive lookup unchanged.

use std::collections::HashMap;
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

#[derive(Debug)]
pub struct Contents {
    pub entries: Vec<Entry>,
    pub directories: Vec<String>,
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
    entries(&archive)?
        .into_iter()
        .find(|entry| entry.name == member)
        .ok_or_else(|| {
            bad(
                &archive,
                format!("member {member:?} is missing or was renamed"),
            )
        })
}

/// Validate the selected member and reject a known Main reader limitation
/// before handoff. Listing can remain correct for these valid containers.
pub fn validate_member_for_launch(path: &Path) -> Result<Entry> {
    let (archive, member) =
        split_member_path(path).ok_or_else(|| bad(path, "not a native archive/member target"))?;
    let contents = contents_controlled(&archive, &AtomicBool::new(false))?
        .ok_or_else(|| bad(&archive, "archive listing was cancelled unexpectedly"))?;
    if !contents.main_compatible {
        return Err(DegaussError::unsupported("zip launch", format!(
            "{}: MiSTer Main's ZIP reader selects a different end record inside the archive comment; member {member:?} cannot be launched",
            archive.display())));
    }
    contents
        .entries
        .into_iter()
        .find(|entry| entry.name == member)
        .ok_or_else(|| {
            bad(
                &archive,
                format!("member {member:?} is missing or was renamed"),
            )
        })
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn list(path: &Path) -> Result<Vec<String>> {
    entries(path).map(|entries| entries.into_iter().map(|entry| entry.name).collect())
}
pub fn entries(path: &Path) -> Result<Vec<Entry>> {
    entries_controlled(path, &AtomicBool::new(false))?
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

/// Enforce a byte-exact path, including Main's first `.zip` split and buffers.
fn validate_name(path: &Path, name: &str) -> Result<()> {
    let body = name.strip_suffix('/').unwrap_or(name);
    if body.is_empty()
        || body.trim() != body
        || body.contains('\\')
        || body.contains(':')
        || body.chars().any(char::is_control)
        || body
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(bad(
            path,
            format!("member {name:?} cannot round-trip as a native path"),
        ));
    }
    if body.rsplit('/').next().is_some_and(|part| part.len() > 260) {
        return Err(bad(
            path,
            format!("member {name:?} exceeds Main's 260-byte filename limit"),
        ));
    }
    if body
        .split('/')
        .any(|part| part.to_ascii_lowercase().ends_with(".zip"))
    {
        return Err(bad(
            path,
            format!("nested archive member {name:?} is unsupported"),
        ));
    }
    let outer = path
        .to_str()
        .ok_or_else(|| bad(path, "archive path is not UTF-8"))?;
    let lower = outer.to_ascii_lowercase();
    if lower.find(".zip") != outer.len().checked_sub(4)
        || outer.chars().any(char::is_control)
        || outer
            .len()
            .checked_add(1)
            .and_then(|n| n.checked_add(body.len()))
            .is_none_or(|n| n > MAX_NATIVE_PATH_BYTES)
    {
        return Err(bad(
            path,
            format!(
                "native target for {name:?} is ambiguous or exceeds {MAX_NATIVE_PATH_BYTES} bytes"
            ),
        ));
    }
    Ok(())
}

/// Central-directory-only read with cancellation. Payload CRC/decompression
/// failures remain Main's responsibility; this never reads compressed data.
pub fn entries_controlled(path: &Path, cancelled: &AtomicBool) -> Result<Option<Vec<Entry>>> {
    contents_controlled(path, cancelled).map(|contents| contents.map(|contents| contents.entries))
}

fn contents_controlled(path: &Path, cancelled: &AtomicBool) -> Result<Option<Contents>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
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
    let mut names = HashMap::<String, (String, bool)>::new();
    names
        .try_reserve(count)
        .map_err(|_| bad(path, "cannot allocate archive name table"))?;
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
        let name = std::str::from_utf8(&bytes[cursor + 46..cursor + 46 + name_len])
            .map_err(|_| bad(path, "member name is not exact UTF-8"))?;
        validate_name(path, name)?;
        let flags = word(header, 8);
        if flags & (1 | 32 | 64 | 8192) != 0 {
            return Err(bad(
                path,
                format!("encrypted or compressed-patch member {name:?} is unsupported"),
            ));
        }
        let method = word(header, 10);
        if method != 0 && method != 8 {
            return Err(bad(
                path,
                format!("member {name:?} has unsupported compression method {method}"),
            ));
        }
        let mut size = u64::from(dword(header, 24));
        let mut compressed = u64::from(dword(header, 20));
        let mut local = u64::from(dword(header, 42));
        let mut disk = u64::from(word(header, 34));
        let mut extra = &bytes[cursor + 46 + name_len..cursor + 46 + name_len + extra_len];
        let mut zip64 = None;
        while !extra.is_empty() {
            if extra.len() < 4 {
                return Err(bad(path, "truncated extra-field header"));
            }
            let field_len = usize::from(word(extra, 2));
            if field_len + 4 > extra.len() {
                return Err(bad(path, "truncated extra-field value"));
            }
            if word(extra, 0) == 1 {
                if zip64.is_some() {
                    return Err(bad(path, "duplicate ZIP64 extra field"));
                }
                zip64 = Some(&extra[4..4 + field_len]);
            }
            extra = &extra[4 + field_len..];
        }
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
        if disk == u64::from(u16::MAX) {
            if zip64.len() < 4 {
                return Err(bad(path, "missing ZIP64 disk value"));
            }
            disk = u64::from(dword(zip64, 0));
            zip64 = &zip64[4..];
        }
        // PKWARE APPNOTE 4.5.3 permits these fields only when the corresponding
        // central-directory value is its ZIP64 sentinel.
        if !zip64.is_empty() {
            return Err(bad(path, "inconsistent ZIP64 extra-field values"));
        }
        if disk != u64::from(directory.disk) {
            return Err(bad(path, "member starts on another disk"));
        }
        if (method == 0 && size != compressed)
            || (size != 0 && compressed == 0)
            || local
                .checked_add(30)
                .and_then(|v| v.checked_add(name_len as u64))
                .and_then(|v| v.checked_add(compressed))
                .is_none_or(|v| v > directory.offset)
        {
            return Err(bad(
                path,
                format!("member {name:?} has inconsistent size or local-header bounds"),
            ));
        }
        let is_dir = name.ends_with('/');
        let key = name.trim_end_matches('/').to_ascii_lowercase();
        if let Some((previous, _)) = names.get(&key) {
            return Err(bad(
                path,
                format!("duplicate or case-ambiguous members {previous:?} and {name:?}"),
            ));
        }
        names.insert(key, (name.to_string(), is_dir));
        if !is_dir {
            entries.push(Entry {
                name: name.to_string(),
                crc32: dword(header, 16),
                size,
            });
        }
        cursor = end;
    }
    if cursor != length {
        return Err(bad(
            path,
            "central-directory size does not match its entries",
        ));
    }
    // Check implied directories against explicit files and directory casing.
    // Sort once rather than retaining every prefix of every long member name.
    let mut paths: Vec<_> = names.iter().collect();
    paths.sort_unstable_by_key(|(left, _)| *left);
    for (_, (name, _)) in &paths {
        for (slash, _) in name.match_indices('/') {
            let prefix = &name[..slash];
            if let Some((existing, directory)) = names.get(&prefix.to_ascii_lowercase()) {
                if !directory || existing.trim_end_matches('/') != prefix {
                    return Err(bad(
                        path,
                        format!("ambiguous file/directory prefix in member {name:?}"),
                    ));
                }
            }
        }
    }
    for pair in paths.windows(2) {
        let (_, (left, _)) = pair[0];
        let (_, (right, _)) = pair[1];
        for (a, b) in left.split('/').zip(right.split('/')) {
            if !a.eq_ignore_ascii_case(b) {
                break;
            }
            if a != b {
                return Err(bad(
                    path,
                    format!("case-ambiguous directory names in {left:?} and {right:?}"),
                ));
            }
        }
    }
    let mut directories: Vec<String> = names
        .into_values()
        .filter_map(|(name, directory)| directory.then(|| name.trim_end_matches('/').to_string()))
        .collect();
    // Browse uses a stable case-insensitive sort. Exact Unicode directory names
    // can share that sort key, so their input order must not come from HashMap.
    directories.sort_unstable();
    Ok(Some(Contents {
        entries,
        directories,
        main_compatible,
    }))
}

/// Real stored ZIP bytes shared by boundary/compatibility tests.
#[cfg(test)]
pub fn tests_archive(names: &[&str], zip64: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut directory = Vec::new();
    for name in names {
        let offset = bytes.len() as u64;
        let name = name.as_bytes();
        let mut local = [0u8; 30];
        local[..4].copy_from_slice(&[0x50, 0x4b, 3, 4]);
        local[4..6].copy_from_slice(&20u16.to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&local);
        bytes.extend_from_slice(name);
        let mut central = [0u8; 46];
        central[..4].copy_from_slice(&CENTRAL_SIGNATURE);
        central[4..6].copy_from_slice(&20u16.to_le_bytes());
        central[6..8].copy_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes());
        central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        if zip64 {
            central[20..28].fill(0xff);
            central[42..46].fill(0xff);
            central[30..32].copy_from_slice(&28u16.to_le_bytes());
        } else {
            central[42..46].copy_from_slice(&(offset as u32).to_le_bytes());
        }
        directory.extend_from_slice(&central);
        directory.extend_from_slice(name);
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
        bytes.extend_from_slice(&(names.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(names.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&(directory.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&[0x50, 0x4b, 6, 7]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&record_offset.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
    }
    bytes.extend_from_slice(&EOCD_SIGNATURE);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let count = if zip64 { u16::MAX } else { names.len() as u16 };
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
        assert_eq!(entries_controlled(&missing, &cancelled).unwrap(), None);
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

    #[test]
    fn invalid_names_cannot_change_identity_or_choose_the_wrong_member() {
        for (index, names) in [
            vec!["../game.neo"],
            vec!["/game.neo"],
            vec!["a//game.neo"],
            vec!["a/./game.neo"],
            vec!["a\\game.neo"],
            vec!["game.neo\n"],
            vec!["nested.zip/game.neo"],
            vec!["nested.zip"],
            vec!["game.neo", "game.neo"],
            vec!["game.neo", "GAME.neo"],
            vec!["Folder/a.neo", "folder/b.neo"],
            vec!["folder", "folder/game.neo"],
        ]
        .iter()
        .enumerate()
        {
            let path = write_fixture(&format!("names-{index}"), &tests_archive(names, false));
            assert!(entries(&path).is_err(), "accepted {names:?}");
            std::fs::remove_file(path).unwrap();
        }
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

    #[test]
    fn unsupported_methods_flags_and_malformed_directory_lengths_are_errors() {
        let original = tests_archive(&["game.neo"], false);
        let central = original
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        for (index, (offset, value)) in [(8, 1), (8, 32), (8, 64), (9, 32), (10, 12), (32, 1)]
            .into_iter()
            .enumerate()
        {
            let mut bytes = original.clone();
            bytes[central + offset] = value;
            let path = write_fixture(&format!("unsupported-{index}"), &bytes);
            assert!(entries(&path).is_err(), "accepted mutation {index}");
            std::fs::remove_file(path).unwrap();
        }
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

    #[test]
    fn entry_names_are_never_decoded_lossily() {
        let mut bytes = tests_archive(&["game.neo"], false);
        let central = bytes
            .windows(4)
            .position(|b| b == CENTRAL_SIGNATURE)
            .unwrap();
        bytes[central + 46] = 0xff;
        let path = write_fixture("bad-utf8", &bytes);
        assert!(entries(&path).unwrap_err().to_string().contains("UTF-8"));
        std::fs::remove_file(path).unwrap();
    }
}
