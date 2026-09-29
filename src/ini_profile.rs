//! The INI slots reported by the running MiSTer Main.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub slot: u8,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub current: u8,
    pub profiles: Vec<Profile>,
}

fn invalid_response() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid INI profile reply from MiSTer Main",
    )
}

fn send_request(fifo: &Path, request: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(fifo)?.write_all(request)
}

fn parse_reply(bytes: &[u8]) -> io::Result<Snapshot> {
    if bytes.len() < 2 || bytes[0] > 3 || !(1..=4).contains(&bytes[1]) {
        return Err(invalid_response());
    }
    let current = bytes[0];
    let count = usize::from(bytes[1]);
    let mut at = 2;
    let mut profiles = Vec::with_capacity(count);
    for _ in 0..count {
        if at + 2 > bytes.len() {
            return Err(invalid_response());
        }
        let slot = bytes[at];
        let length = usize::from(bytes[at + 1]);
        at += 2;
        if slot > 3 || length == 0 || at + length > bytes.len() {
            return Err(invalid_response());
        }
        let filename = &bytes[at..at + length];
        at += length;
        let name = if slot == 0 && filename == b"Main" {
            "Main".to_string()
        } else if slot > 0
            && filename.len() > 11
            && filename[..7].eq_ignore_ascii_case(b"MiSTer_")
            && filename[filename.len() - 4..].eq_ignore_ascii_case(b".ini")
        {
            String::from_utf8_lossy(&filename[7..filename.len() - 4]).into_owned()
        } else {
            return Err(invalid_response());
        };
        if profiles
            .last()
            .is_some_and(|previous: &Profile| previous.slot >= slot)
        {
            return Err(invalid_response());
        }
        profiles.push(Profile { slot, name });
    }
    if at != bytes.len() || profiles.first().is_none_or(|profile| profile.slot != 0) {
        return Err(invalid_response());
    }
    Ok(Snapshot { current, profiles })
}

pub fn discover(fifo: &Path) -> io::Result<Snapshot> {
    let token = ((SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos()
        % 900_000_000) as u32)
        + 100_000_000;
    let status = format!("/tmp/degauss-ini-profiles-{token}.status");
    if Path::new(&status).exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "INI profile reply path is already in use",
        ));
    }
    send_request(fifo, format!("ini_profiles {token}\n").as_bytes())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match fs::read(&status) {
            Ok(reply) => {
                fs::remove_file(&status)?;
                return parse_reply(&reply);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "MiSTer Main did not report its selectable INI profiles",
    ))
}

fn marker_slot(marker: [u8; 4]) -> io::Result<u8> {
    if marker[..3] != [0x34, 0x99, 0xBA] {
        return Ok(0);
    }
    if marker[3] <= 3 {
        Ok(marker[3])
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "MiSTer INI marker contains an unsupported slot",
        ))
    }
}

#[cfg(target_os = "linux")]
pub fn current_slot() -> io::Result<u8> {
    use std::os::fd::AsRawFd;

    let mem = fs::OpenOptions::new().read(true).open("/dev/mem")?;
    // Main's altcfg() reads bytes at 0x1FFFFF04 within this shared page.
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            0x1000,
            libc::PROT_READ,
            libc::MAP_SHARED,
            mem.as_raw_fd(),
            0x1FFFF000,
        )
    };
    if page == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    let mut marker = [0; 4];
    for (index, byte) in marker.iter_mut().enumerate() {
        *byte = unsafe { std::ptr::read_volatile((page as *const u8).add(0xF04 + index)) };
    }
    let unmap = unsafe { libc::munmap(page, 0x1000) };
    if unmap != 0 {
        return Err(io::Error::last_os_error());
    }
    marker_slot(marker)
}

#[cfg(target_os = "linux")]
pub fn select_slot(slot: u8) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    if slot > 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "MiSTer INI slot must be 0..3",
        ));
    }
    let mem = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/mem")?;
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            0x1000,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            mem.as_raw_fd(),
            0x1FFFF000,
        )
    };
    if page == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    for (index, byte) in [0x34, 0x99, 0xBA, slot].into_iter().enumerate() {
        unsafe { std::ptr::write_volatile((page as *mut u8).add(0xF04 + index), byte) };
    }
    if unsafe { libc::munmap(page, 0x1000) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn current_slot() -> io::Result<u8> {
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn request_does_not_block_when_fifo_has_no_reader() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let path = std::env::temp_dir().join(format!(
            "degauss-ini-no-reader-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let native = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
        let started = Instant::now();
        let error = send_request(&path, b"ini_profiles 123456789\n").unwrap_err();
        fs::remove_file(&path).unwrap();

        assert_eq!(error.raw_os_error(), Some(libc::ENXIO));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn marker_matches_main_semantics() {
        assert_eq!(marker_slot([0, 0, 0, 7]).unwrap(), 0);
        assert_eq!(marker_slot([0x34, 0x99, 0xBA, 2]).unwrap(), 2);
        assert!(marker_slot([0x34, 0x99, 0xBA, 4]).is_err());
    }

    #[test]
    fn main_reply_preserves_mains_slots_and_custom_labels() {
        let reply = [
            2, 3, 0, 4, b'M', b'a', b'i', b'n', 1, 17, b'M', b'i', b'S', b'T', b'e', b'r', b'_',
            b'C', b'u', b's', b't', b'o', b'm', b'.', b'i', b'n', b'i', 2, 14, b'M', b'i', b'S',
            b'T', b'e', b'r', b'_', b'A', b'l', b't', b'.', b'i', b'n', b'i',
        ];
        let snapshot = parse_reply(&reply).unwrap();
        assert_eq!(snapshot.current, 2);
        assert_eq!(snapshot.profiles[0].name, "Main");
        assert_eq!(snapshot.profiles[1].name, "Custom");
        assert_eq!(snapshot.profiles[2].name, "Alt");
        assert_eq!(snapshot.profiles[2].slot, 2);
    }

    #[test]
    fn malformed_or_unsorted_reply_is_not_accepted_as_available() {
        assert!(parse_reply(&[0, 1, 1, 4, b'M', b'a', b'i', b'n']).is_err());
        assert!(parse_reply(&[0, 1, 0, 4, b'M', b'a', b'i']).is_err());
        assert!(parse_reply(&[0, 1, 0, 4, b'M', b'a', b'i', b'n', 0]).is_err());
    }

    #[test]
    fn main_can_report_a_gap_after_an_alternate_disappears() {
        let reply = [
            0, 2, 0, 4, b'M', b'a', b'i', b'n', 3, 14, b'M', b'i', b'S', b'T', b'e', b'r', b'_',
            b'A', b'l', b't', b'.', b'i', b'n', b'i',
        ];
        let snapshot = parse_reply(&reply).unwrap();
        assert_eq!(snapshot.profiles.len(), 2);
        assert!(!snapshot.profiles.iter().any(|profile| profile.slot == 1));
        assert!(snapshot.profiles.iter().any(|profile| profile.slot == 3));
    }
}
