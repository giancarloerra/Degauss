//! Acknowledged display-state handover with the owning Degauss Main process.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::error::{DegaussError, Result};

pub const CAPABILITY_ENV: &str = "DEGAUSS_SCRIPT_HANDOVER";
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
pub enum Mode {
    Frontend,
    Script,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Frontend => "frontend",
            Self::Script => "script",
        }
    }
}

pub fn supported() -> bool {
    std::env::var_os(CAPABILITY_ENV).as_deref() == Some(std::ffi::OsStr::new("1"))
}

fn remove_reply(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DegaussError::io(
            "remove terminal-handover reply",
            path,
            error,
        )),
    }
}

pub fn request(fifo: &Path, directory: &Path, mode: Mode) -> Result<()> {
    request_with_timeout(fifo, directory, mode, Duration::from_secs(2))
}

fn request_with_timeout(
    fifo: &Path,
    directory: &Path,
    mode: Mode,
    timeout: Duration,
) -> Result<()> {
    let token = 100_000_000
        + std::process::id()
            .wrapping_mul(1_000_003)
            .wrapping_add(SEQUENCE.fetch_add(1, Ordering::Relaxed))
            % 900_000_000;
    let status = directory.join(format!("degauss-terminal-{token}.status"));
    remove_reply(&status)?;
    let mut options = fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options
        .open(fifo)
        .and_then(|mut file| {
            file.write_all(format!("fb_degauss {token} {}\n", mode.label()).as_bytes())
        })
        .map_err(|error| DegaussError::io("request script terminal handover", fifo, error))?;
    let deadline = Instant::now() + timeout;
    loop {
        match fs::read_to_string(&status) {
            Ok(reply) => {
                remove_reply(&status)?;
                return match reply.trim_end() {
                    "ok" => Ok(()),
                    error if error.starts_with("error ") => Err(DegaussError::unsupported(
                        "script terminal handover",
                        &error[6..],
                    )),
                    _ => Err(DegaussError::malformed(
                        "terminal-handover reply",
                        &status,
                        "unrecognised response",
                    )),
                };
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DegaussError::io(
                    "read terminal-handover reply",
                    &status,
                    error,
                ))
            }
        }
        if Instant::now() >= deadline {
            return Err(DegaussError::unsupported(
                "script terminal handover",
                "MiSTer Main did not acknowledge the requested terminal mode",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    #[test]
    fn real_fifo_acknowledgement_precedes_success_and_preserves_main_errors() {
        for (mode, reply, success) in [
            (Mode::Script, "ok\n", true),
            (Mode::Frontend, "ok\n", true),
            (Mode::Script, "error terminal unavailable\n", false),
            (Mode::Script, "unexpected\n", false),
        ] {
            let directory = std::env::temp_dir().join(format!(
                "degauss-terminal-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let fifo = directory.join("command");
            let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            let mut reader = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .unwrap();
            let parent = directory.clone();
            let main = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let deadline = Instant::now() + Duration::from_secs(3);
                while !bytes.ends_with(b"\n") {
                    match reader.read_to_end(&mut bytes) {
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(error) => panic!("{error}"),
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                let command = String::from_utf8(bytes).unwrap();
                let parts: Vec<_> = command.split_whitespace().collect();
                assert_eq!(parts[0], "fb_degauss");
                assert_eq!(parts[2], mode.label());
                let status = parent.join(format!("degauss-terminal-{}.status", parts[1]));
                let temporary = status.with_extension("part");
                fs::write(&temporary, reply).unwrap();
                fs::rename(&temporary, status).unwrap();
            });
            let result = request(&fifo, &directory, mode);
            main.join().unwrap();
            assert_eq!(result.is_ok(), success);
            if reply.starts_with("error") {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("terminal unavailable"));
            }
            assert_eq!(
                fs::read_dir(&directory).unwrap().count(),
                1,
                "reply is consumed"
            );
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn unavailable_main_does_not_report_a_successful_handover() {
        let directory =
            std::env::temp_dir().join(format!("degauss-terminal-timeout-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let fifo = directory.join("command");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(request(&fifo, &directory, Mode::Script).is_err());
        let _reader = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let error =
            request_with_timeout(&fifo, &directory, Mode::Script, Duration::from_millis(20))
                .unwrap_err();
        assert!(error.to_string().contains("did not acknowledge"));
        fs::remove_dir_all(directory).unwrap();
    }
}
