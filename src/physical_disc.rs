//! Session-scoped handoff for Degauss Main's physical-disc reader.
//!
//! Main owns the Linux CD-ROM and SCSI work. This process only enables that
//! worker while the option is on and consumes its atomic launch/error event.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::error::{DegaussError, Result};

const CONTROL_FILE: &str = "/tmp/degauss_physical_disc.pid";
const EVENT_FILE: &str = "/tmp/degauss_physical_disc_event";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
static NEXT_TEMPORARY: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Launch(PathBuf),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    Idle,
    Checked(Option<Event>),
}

#[derive(Debug)]
pub struct Session {
    control: PathBuf,
    event: PathBuf,
    owner: String,
    requested: bool,
    active: bool,
    last_poll: Option<Instant>,
}

impl Session {
    pub fn new() -> Self {
        Self::with_paths(PathBuf::from(CONTROL_FILE), PathBuf::from(EVENT_FILE))
    }

    fn with_paths(control: PathBuf, event: PathBuf) -> Self {
        Self {
            control,
            event,
            owner: format!("1 {}\n", std::process::id()),
            requested: false,
            active: false,
            last_poll: None,
        }
    }

    /// Change the requested state once. A failed transition is not retried
    /// every frame; switching Off and On again makes a deliberate retry.
    pub fn sync_enabled(&mut self, enabled: bool) -> Result<()> {
        if self.requested == enabled {
            return Ok(());
        }
        self.requested = enabled;
        if enabled {
            remove_if_present(&self.event, "clearing a stale physical-disc event")?;
            match atomic_write(&self.control, self.owner.as_bytes()) {
                Ok(()) => {
                    self.active = true;
                    self.last_poll = None;
                    Ok(())
                }
                Err(error) => {
                    self.active = false;
                    Err(error)
                }
            }
        } else {
            self.active = false;
            self.last_poll = None;
            self.remove_owned_control()
        }
    }

    pub fn poll(&mut self, now: Instant) -> Result<Poll> {
        if !self.active
            || self
                .last_poll
                .is_some_and(|last| now.duration_since(last) < POLL_INTERVAL)
        {
            return Ok(Poll::Idle);
        }
        self.last_poll = Some(now);
        self.read_event().map(Poll::Checked)
    }

    fn read_event(&self) -> Result<Option<Event>> {
        let text = match fs::read_to_string(&self.event) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(DegaussError::io(
                    "reading the physical-disc event",
                    &self.event,
                    error,
                ));
            }
        };
        fs::remove_file(&self.event).map_err(|error| {
            DegaussError::io("consuming the physical-disc event", &self.event, error)
        })?;
        parse_event(&self.event, &text).map(Some)
    }

    fn remove_owned_control(&self) -> Result<()> {
        let current = match fs::read_to_string(&self.control) {
            Ok(current) => current,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(DegaussError::io(
                    "reading the physical-disc control",
                    &self.control,
                    error,
                ));
            }
        };
        if current == self.owner {
            remove_if_present(&self.control, "disabling physical-disc detection")?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = self.remove_owned_control() {
            crate::note(&format!("physical disc  control not removed: {error}"));
        }
    }
}

fn parse_event(path: &Path, text: &str) -> Result<Event> {
    if !text.ends_with('\n') {
        return Err(DegaussError::malformed(
            "physical-disc event",
            path,
            "missing final newline",
        ));
    }
    let mut lines = text.lines();
    let kind = lines.next().unwrap_or_default();
    let value = lines.next().unwrap_or_default();
    if value.is_empty() || lines.next().is_some() {
        return Err(DegaussError::malformed(
            "physical-disc event",
            path,
            "expected exactly a type and value",
        ));
    }
    match kind {
        "launch" => {
            let launcher = PathBuf::from(value);
            if !launcher.is_absolute() {
                return Err(DegaussError::malformed(
                    "physical-disc event",
                    path,
                    "launcher path is not absolute",
                ));
            }
            Ok(Event::Launch(launcher))
        }
        "error" => Ok(Event::Error(value.to_string())),
        other => Err(DegaussError::malformed(
            "physical-disc event",
            path,
            format!("unknown event type {other:?}"),
        )),
    }
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DegaussError::unsupported(
            "physical-disc control",
            format!("{} has no parent directory", path.display()),
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        DegaussError::unsupported(
            "physical-disc control",
            format!("{} has no file name", path.display()),
        )
    })?;
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{}.{}.{}.part",
        name.to_string_lossy(),
        std::process::id(),
        sequence
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| {
                DegaussError::io("creating the physical-disc control", &temporary, error)
            })?;
        file.write_all(body).map_err(|error| {
            DegaussError::io("writing the physical-disc control", &temporary, error)
        })?;
        fs::rename(&temporary, path)
            .map_err(|error| DegaussError::io("enabling physical-disc detection", path, error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn remove_if_present(path: &Path, what: &'static str) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DegaussError::io(what, path, error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "degauss-physical-disc-{tag}-{}",
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir(&root).unwrap();
        (root.clone(), root.join("control"), root.join("event"))
    }

    #[test]
    fn disabled_is_the_compatible_default() {
        let (root, control, event) = fixture("default-off");
        let mut session = Session::with_paths(control.clone(), event);
        session.sync_enabled(false).unwrap();
        assert!(!control.exists());
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enable_clears_stale_event_and_disable_removes_owned_control() {
        let (root, control, event) = fixture("lifecycle");
        fs::write(&event, "launch\n/old.mgl\n").unwrap();
        let mut session = Session::with_paths(control.clone(), event.clone());
        session.sync_enabled(true).unwrap();
        assert_eq!(fs::read_to_string(&control).unwrap(), session.owner);
        assert!(!event.exists());
        session.sync_enabled(false).unwrap();
        assert!(!control.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drop_preserves_another_session_control() {
        let (root, control, event) = fixture("foreign-control");
        let mut session = Session::with_paths(control.clone(), event);
        session.sync_enabled(true).unwrap();
        fs::write(&control, "1 99999\n").unwrap();
        drop(session);
        assert_eq!(fs::read_to_string(&control).unwrap(), "1 99999\n");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_and_error_events_are_consumed_once() {
        let (root, control, event) = fixture("events");
        let mut session = Session::with_paths(control, event.clone());
        session.sync_enabled(true).unwrap();
        fs::write(&event, "launch\n/media/fat/_Disc_Cores/Saturn.mgl\n").unwrap();
        assert_eq!(
            session.read_event().unwrap(),
            Some(Event::Launch(PathBuf::from(
                "/media/fat/_Disc_Cores/Saturn.mgl"
            )))
        );
        assert_eq!(session.read_event().unwrap(), None);
        fs::write(&event, "error\nNo provider is installed.\n").unwrap();
        assert_eq!(
            session.read_event().unwrap(),
            Some(Event::Error("No provider is installed.".to_string()))
        );
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_event_is_removed_and_reported() {
        let (root, control, event) = fixture("malformed");
        let mut session = Session::with_paths(control, event.clone());
        session.sync_enabled(true).unwrap();
        fs::write(&event, "launch\nrelative.mgl\n").unwrap();
        let error = session.read_event().unwrap_err().to_string();
        assert!(error.contains("launcher path is not absolute"));
        assert!(!event.exists());
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn poll_distinguishes_throttling_from_a_successful_check() {
        let (root, control, event) = fixture("poll-state");
        let mut session = Session::with_paths(control, event.clone());
        session.sync_enabled(true).unwrap();
        let now = Instant::now();
        assert_eq!(session.poll(now).unwrap(), Poll::Checked(None));
        assert_eq!(session.poll(now).unwrap(), Poll::Idle);
        fs::write(&event, "error\nProvider unavailable.\n").unwrap();
        assert_eq!(
            session.poll(now + POLL_INTERVAL).unwrap(),
            Poll::Checked(Some(Event::Error("Provider unavailable.".to_string())))
        );
        drop(session);
        fs::remove_dir_all(root).unwrap();
    }
}
