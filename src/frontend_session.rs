pub const OWNER_ENV: &str = "DEGAUSS_MENU_OWNER";

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OwnerIdentity {
    pid: i32,
    started: u64,
}

#[cfg(any(target_os = "linux", test))]
impl OwnerIdentity {
    fn parse(token: &str) -> std::result::Result<Self, &'static str> {
        let (pid, started) = token
            .split_once(':')
            .ok_or("expected a process ID and start time separated by ':'")?;
        let pid = pid.parse::<i32>().map_err(|_| "invalid owner process ID")?;
        let started = started
            .parse::<u64>()
            .map_err(|_| "invalid owner start time")?;
        if pid <= 1 || started == 0 {
            return Err("owner must identify a running MiSTer process, not init");
        }
        Ok(Self { pid, started })
    }

    fn token(self) -> String {
        format!("{}:{}", self.pid, self.started)
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessStat {
    pid: i32,
    parent: i32,
    started: u64,
}

#[cfg(any(target_os = "linux", test))]
fn parse_stat(text: &str) -> std::result::Result<ProcessStat, &'static str> {
    let open = text.find('(').ok_or("missing process name")?;
    let close = text.rfind(')').ok_or("unterminated process name")?;
    if close < open {
        return Err("invalid process name");
    }
    let pid = text[..open]
        .trim()
        .parse::<i32>()
        .map_err(|_| "invalid process ID")?;
    let mut fields = text[close + 1..].split_whitespace();
    fields.next().ok_or("missing process state")?;
    let parent = fields
        .next()
        .ok_or("missing parent process ID")?
        .parse::<i32>()
        .map_err(|_| "invalid parent process ID")?;
    let started = fields
        .nth(17)
        .ok_or("missing process start time")?
        .parse::<u64>()
        .map_err(|_| "invalid process start time")?;
    if pid <= 0 || parent < 0 {
        return Err("invalid process ancestry");
    }
    Ok(ProcessStat {
        pid,
        parent,
        started,
    })
}

#[cfg(any(target_os = "linux", test))]
fn is_menu_script(command: &[u8]) -> bool {
    command
        .split(|byte| *byte == 0)
        .any(|arg| arg == b"/tmp/script")
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::{is_menu_script, parse_stat, OwnerIdentity, ProcessStat, OWNER_ENV};
    use crate::error::{DegaussError, Result};

    const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
    const LOCK_RETRY: Duration = Duration::from_millis(20);

    fn process_stat(pid: i32) -> Result<Option<ProcessStat>> {
        let path = PathBuf::from(format!("/proc/{pid}/stat"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(DegaussError::io("read menu owner", path, error)),
        };
        let stat = parse_stat(&text)
            .map_err(|detail| DegaussError::malformed("menu owner process", &path, detail))?;
        if stat.pid != pid {
            return Err(DegaussError::malformed(
                "menu owner process",
                path,
                "process ID does not match the requested process",
            ));
        }
        Ok(Some(stat))
    }

    enum Origin {
        Unmanaged,
        Owner(OwnerIdentity),
        Expired,
    }

    fn discover_owner() -> Result<Origin> {
        let mut pid = unsafe { libc::getppid() };
        let mut visited = Vec::new();
        for _ in 0..64 {
            if pid <= 1 {
                return Ok(Origin::Unmanaged);
            }
            if visited.contains(&pid) {
                return Err(DegaussError::malformed(
                    "menu owner ancestry",
                    "/proc",
                    "process ancestry contains a cycle",
                ));
            }
            visited.push(pid);
            let Some(stat) = process_stat(pid)? else {
                return Ok(Origin::Expired);
            };
            let path = PathBuf::from(format!("/proc/{pid}/cmdline"));
            let command = match std::fs::read(&path) {
                Ok(command) => command,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(Origin::Expired);
                }
                Err(error) => return Err(DegaussError::io("read menu launcher", path, error)),
            };
            if is_menu_script(&command) {
                if stat.parent <= 1 {
                    return Ok(Origin::Expired);
                }
                let Some(owner) = process_stat(stat.parent)? else {
                    return Ok(Origin::Expired);
                };
                let Some(launcher) = process_stat(pid)? else {
                    return Ok(Origin::Expired);
                };
                if launcher.started != stat.started || launcher.parent != owner.pid {
                    return Ok(Origin::Expired);
                }
                return Ok(Origin::Owner(OwnerIdentity {
                    pid: owner.pid,
                    started: owner.started,
                }));
            }
            pid = stat.parent;
        }
        Err(DegaussError::malformed(
            "menu owner ancestry",
            "/proc",
            "process ancestry exceeds 64 levels",
        ))
    }

    struct Owner {
        identity: OwnerIdentity,
        process: File,
    }

    impl Owner {
        fn open(identity: OwnerIdentity) -> Result<Option<Self>> {
            let matches = || {
                process_stat(identity.pid)
                    .map(|stat| stat.is_some_and(|stat| stat.started == identity.started))
            };
            if !matches()? {
                return Ok(None);
            }
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) };
            if fd == -1 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(None);
                }
                return Err(DegaussError::io(
                    "observe MiSTer process lifetime with pidfd_open",
                    format!("/proc/{}", identity.pid),
                    error,
                ));
            }
            let owner = Self {
                identity,
                process: unsafe { File::from_raw_fd(fd as i32) },
            };
            if !matches()? || !owner.alive()? {
                return Ok(None);
            }
            Ok(Some(owner))
        }

        fn alive(&self) -> Result<bool> {
            let mut descriptor = libc::pollfd {
                fd: self.process.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
                if result == 0 {
                    return Ok(true);
                }
                if result < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(DegaussError::io(
                        "check MiSTer process lifetime",
                        format!("/proc/{}", self.identity.pid),
                        error,
                    ));
                }
                if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                    return Err(DegaussError::io(
                        "check MiSTer process lifetime",
                        format!("/proc/{}", self.identity.pid),
                        io::Error::other(format!(
                            "process descriptor reported flags {:#x}",
                            descriptor.revents
                        )),
                    ));
                }
                if descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                    return Ok(false);
                }
                return Err(DegaussError::io(
                    "check MiSTer process lifetime",
                    format!("/proc/{}", self.identity.pid),
                    io::Error::other(format!(
                        "unexpected process descriptor flags {:#x}",
                        descriptor.revents
                    )),
                ));
            }
        }
    }

    pub struct UiSession {
        _framebuffer: File,
        owner: Option<Owner>,
    }

    impl UiSession {
        pub fn acquire(device: &Path) -> Result<Option<Self>> {
            let origin = match std::env::var_os(OWNER_ENV) {
                Some(value) => {
                    let token = value.to_str().ok_or_else(|| {
                        DegaussError::malformed(
                            "menu owner token",
                            OWNER_ENV,
                            "value is not valid UTF-8",
                        )
                    })?;
                    Origin::Owner(OwnerIdentity::parse(token).map_err(|detail| {
                        DegaussError::malformed("menu owner token", OWNER_ENV, detail)
                    })?)
                }
                None => discover_owner()?,
            };
            let owner = match origin {
                Origin::Owner(identity) => match Owner::open(identity)? {
                    Some(owner) => Some(owner),
                    None => return Ok(None),
                },
                Origin::Expired => return Ok(None),
                Origin::Unmanaged => None,
            };
            Self::lock(device, owner, LOCK_TIMEOUT)
        }

        fn lock(device: &Path, owner: Option<Owner>, timeout: Duration) -> Result<Option<Self>> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(device)
                .map_err(|error| {
                    DegaussError::io("open framebuffer ownership lock", device, error)
                })?;
            let began = Instant::now();
            loop {
                if let Some(owner) = &owner {
                    if !owner.alive()? {
                        return Ok(None);
                    }
                }
                let result =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    let session = Self {
                        _framebuffer: file,
                        owner,
                    };
                    return if session.owner_alive()? {
                        Ok(Some(session))
                    } else {
                        Ok(None)
                    };
                }
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() != io::ErrorKind::WouldBlock {
                    return Err(DegaussError::io(
                        "lock framebuffer ownership",
                        device,
                        error,
                    ));
                }
                if began.elapsed() >= timeout {
                    return Err(DegaussError::io(
                        "lock framebuffer ownership",
                        device,
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "another Degauss instance still owns this framebuffer; close that instance before starting another",
                        ),
                    ));
                }
                std::thread::sleep(LOCK_RETRY.min(timeout.saturating_sub(began.elapsed())));
            }
        }

        pub fn owner_token(&self) -> Option<String> {
            self.owner.as_ref().map(|owner| owner.identity.token())
        }

        pub fn owner_alive(&self) -> Result<bool> {
            self.owner.as_ref().map_or(Ok(true), Owner::alive)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::{Child, Command, Stdio};
        use std::sync::atomic::{AtomicU64, Ordering};

        struct LockFile(PathBuf);

        impl LockFile {
            fn new() -> Self {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let path = std::env::temp_dir().join(format!(
                    "degauss-ui-session-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .unwrap();
                Self(path)
            }
        }

        impl Drop for LockFile {
            fn drop(&mut self) {
                std::fs::remove_file(&self.0).unwrap();
            }
        }

        struct ChildOwner(Child);

        impl ChildOwner {
            fn new() -> Self {
                Self(
                    Command::new("/bin/sh")
                        .arg("-c")
                        .arg("read ignored")
                        .stdin(Stdio::piped())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap(),
                )
            }

            fn identity(&self) -> OwnerIdentity {
                let stat = process_stat(self.0.id() as i32).unwrap().unwrap();
                OwnerIdentity {
                    pid: stat.pid,
                    started: stat.started,
                }
            }

            fn stop(&mut self) {
                self.0.kill().unwrap();
                self.0.wait().unwrap();
            }
        }

        impl Drop for ChildOwner {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        #[test]
        fn framebuffer_lock_is_exclusive_and_released_with_ui() {
            let device = LockFile::new();
            let first = UiSession::lock(&device.0, None, Duration::ZERO)
                .unwrap()
                .unwrap();
            assert!(first.owner_alive().unwrap());
            assert_eq!(first.owner_token(), None);
            let error = UiSession::lock(&device.0, None, Duration::ZERO)
                .err()
                .unwrap();
            assert!(
                matches!(error, DegaussError::Io { source, .. } if source.kind() == io::ErrorKind::TimedOut)
            );
            drop(first);
            assert!(UiSession::lock(&device.0, None, Duration::ZERO)
                .unwrap()
                .is_some());
        }

        #[test]
        fn new_ui_waits_for_previous_ui_to_release_framebuffer() {
            let device = LockFile::new();
            let first = UiSession::lock(&device.0, None, Duration::ZERO)
                .unwrap()
                .unwrap();
            let release = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                drop(first);
            });
            let second = UiSession::lock(&device.0, None, Duration::from_secs(2))
                .unwrap()
                .unwrap();
            release.join().unwrap();
            assert!(second.owner_alive().unwrap());
        }

        #[test]
        fn pidfd_reports_owner_exit_without_following_a_reused_pid() {
            let mut child = ChildOwner::new();
            let identity = child.identity();
            let owner = Owner::open(identity).unwrap().unwrap();
            assert!(owner.alive().unwrap());
            let reused = OwnerIdentity {
                started: identity.started + 1,
                ..identity
            };
            assert!(Owner::open(reused).unwrap().is_none());
            child.stop();
            assert!(!owner.alive().unwrap());
            assert!(Owner::open(identity).unwrap().is_none());
        }

        #[test]
        fn owner_exit_cancels_waiting_for_another_frontend() {
            let device = LockFile::new();
            let _first = UiSession::lock(&device.0, None, Duration::ZERO)
                .unwrap()
                .unwrap();
            let mut child = ChildOwner::new();
            let owner = Owner::open(child.identity()).unwrap().unwrap();
            let stop = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                child.stop();
            });
            assert!(
                UiSession::lock(&device.0, Some(owner), Duration::from_secs(2))
                    .unwrap()
                    .is_none()
            );
            stop.join().unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::UiSession;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_token_pins_pid_and_start_time() {
        let owner = OwnerIdentity::parse("1234:567890").unwrap();
        assert_eq!(owner.pid, 1234);
        assert_eq!(owner.started, 567890);
        assert_eq!(owner.token(), "1234:567890");
        for invalid in [
            "", "1234", "1:12", "0:12", "-2:12", "1234:0", "1234:x", "2:3:4",
        ] {
            assert!(OwnerIdentity::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn process_stat_parses_names_with_spaces_and_closing_parentheses() {
        let text = "1234 (MiSTer (custom) name)) S 4321 1 1 1026 1 0 0 0 0 0 0 0 0 0 20 0 1 0 567890 1024 20";
        assert_eq!(
            parse_stat(text).unwrap(),
            ProcessStat {
                pid: 1234,
                parent: 4321,
                started: 567890,
            }
        );
        for invalid in ["", "1234 missing", "1234 (name) S 1", "1234 )name( S 1"] {
            assert!(parse_stat(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn menu_launcher_requires_the_exact_script_argument() {
        assert!(is_menu_script(b"/bin/bash\0/tmp/script\0-f\0root\0"));
        assert!(is_menu_script(b"/tmp/script\0"));
        assert!(!is_menu_script(b"/bin/bash\0/tmp/script-copy\0"));
        assert!(!is_menu_script(b"/bin/bash\0/tmp/scripts/script\0"));
    }
}
