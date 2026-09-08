//! One system's Index All work, owned by the running frontend process.
//! Reads and complete cache replacement stay off the rendering thread.
//! The UI owns the final summary index and prevents concurrent cache writers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::browse::{DisplayNames, Library};
use crate::cache::Summary;
use crate::config::SystemConfig;
use crate::error::{DegaussError, Result};

pub struct Request {
    pub id: String,
    pub config: SystemConfig,
    pub names: DisplayNames,
    pub cache_dir: PathBuf,
    pub forced: bool,
    pub artwork_pack: bool,
}

#[derive(Debug)]
pub enum Event {
    Progress { folders: usize, games: usize },
    Ready { summary: Option<Summary> },
    Cancelled,
    Failed(DegaussError),
}

pub struct Job {
    events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn try_recv(&mut self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(event) => {
                if !matches!(event, Event::Progress { .. }) {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => self.handle.take().map(|handle| {
                let _ = handle.join();
                Event::Failed(DegaussError::unsupported(
                    "indexing",
                    "the index worker stopped without a result",
                ))
            }),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start(request: Request) -> Result<Job> {
    let (sender, events) = mpsc::sync_channel(4);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-index".to_string())
        .spawn(move || {
            let result = run(request, &sender, &worker_cancelled);
            let event = match result {
                Ok(event) => event,
                Err(error) => Event::Failed(error),
            };
            let _ = sender.send(event);
        })
        .map_err(|error| {
            DegaussError::unsupported("indexing", format!("could not start index worker: {error}"))
        })?;
    Ok(Job {
        events,
        cancelled,
        handle: Some(handle),
    })
}

fn run(request: Request, events: &SyncSender<Event>, cancelled: &AtomicBool) -> Result<Event> {
    let began = Instant::now();
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Event::Cancelled);
    }
    let start = crate::browse::start_for(&request.config);
    // Pack rebuilding remains the responsibility of the existing group
    // worker, queued after this pass. Never replace it with gamelist data.
    let cached = if request.artwork_pack {
        crate::cache::load_artwork_pack_system(&request.cache_dir, &request.id)
    } else if !request.forced {
        crate::cache::load_system(&request.cache_dir, &request.id)
    } else {
        None
    };
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Event::Cancelled);
    }
    if cached.is_some() || request.artwork_pack {
        return Ok(Event::Ready {
            summary: cached.map(|cache| cache.summary(&start)),
        });
    }
    let library = Library::open_with_names(&request.config, request.names)?;
    let open_ms = began.elapsed().as_millis();
    let walking = Instant::now();
    let mut last_progress = Instant::now();
    let Some(cache) =
        crate::cache::build_system_controlled(&library, cancelled, &mut |folders, games| {
            if last_progress.elapsed() >= Duration::from_millis(100) {
                let _ = events.try_send(Event::Progress { folders, games });
                last_progress = Instant::now();
            }
        })?
    else {
        return Ok(Event::Cancelled);
    };
    let walk_ms = walking.elapsed().as_millis();
    let summary = cache.summary(&start);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Event::Cancelled);
    }
    // Once this complete cache is installed, return its summary even if
    // cancellation arrives during the write. The UI retains completed work
    // and writes the matching final index before releasing the writer.
    let saving = Instant::now();
    crate::cache::save_system(&request.cache_dir, &request.id, &cache)?;
    crate::note(&format!(
        "index        {}: open={}ms walk={}ms save={}ms total={}ms folders={} games={}",
        request.id,
        open_ms,
        walk_ms,
        saving.elapsed().as_millis(),
        began.elapsed().as_millis(),
        cache.folders.len(),
        summary.games,
    ));
    Ok(Event::Ready {
        summary: Some(summary),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn fixture() -> (PathBuf, Request) {
        let root = (0..)
            .find_map(|serial| {
                let path = std::env::temp_dir()
                    .join(format!("degauss-index-{}-{serial}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => Some(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("fixture: {error}"),
                }
            })
            .unwrap();
        let games = root.join("games");
        std::fs::create_dir(&games).unwrap();
        std::fs::write(games.join("one.rom"), b"fixture").unwrap();
        let request = Request {
            id: "Test".into(),
            config: SystemConfig {
                preserve_rbf_stem: false,
                name: "Test".into(),
                path: games.to_string_lossy().into_owned(),
                extensions: vec!["rom".into()],
                rbf: "Test".into(),
                launch: vec![],
                setname: None,
                skip_folders: vec![],
                extra_paths: vec![],
            },
            names: Default::default(),
            cache_dir: root.join("cache"),
            forced: true,
            artwork_pack: false,
        };
        (root, request)
    }

    fn terminal(job: &mut Job) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(event) = job.try_recv() {
                if !matches!(event, Event::Progress { .. }) {
                    return event;
                }
            }
            assert!(Instant::now() < deadline, "index worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn worker_installs_complete_cache_and_leaves_summary_index_to_ui() {
        let (root, request) = fixture();
        let cache_dir = request.cache_dir.clone();
        let mut job = start(request).unwrap();
        let Event::Ready {
            summary: Some(summary),
        } = terminal(&mut job)
        else {
            panic!("complete scan must be saved");
        };
        assert_eq!(summary.games, 1);
        assert!(crate::cache::load_index(&cache_dir).is_none());
        assert!(crate::cache::load_system(&cache_dir, "Test").is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_scan_preserves_previous_cache() {
        let (root, request) = fixture();
        let cache_dir = request.cache_dir.clone();
        let mut first = start(Request {
            config: request.config.clone(),
            names: request.names.clone(),
            id: request.id.clone(),
            cache_dir: cache_dir.clone(),
            forced: true,
            artwork_pack: false,
        })
        .unwrap();
        assert!(matches!(terminal(&mut first), Event::Ready { .. }));
        let before = std::fs::read(crate::cache::system_path(&cache_dir, "Test")).unwrap();
        std::fs::write(root.join("games/two.rom"), b"fixture").unwrap();
        let (sender, _) = mpsc::sync_channel(4);
        assert!(matches!(
            run(request, &sender, &AtomicBool::new(true)).unwrap(),
            Event::Cancelled
        ));
        assert_eq!(
            std::fs::read(crate::cache::system_path(&cache_dir, "Test")).unwrap(),
            before
        );
        assert_eq!(std::fs::read_dir(&cache_dir).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_pack_pass_never_scans_or_installs_gamelist_data() {
        let (root, mut request) = fixture();
        let cache_dir = request.cache_dir.clone();
        request.artwork_pack = true;
        let mut job = start(request).unwrap();
        assert!(matches!(terminal(&mut job), Event::Ready { summary: None }));
        assert!(!cache_dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_cancelled_scan_does_not_touch_the_cache() {
        let (root, request) = fixture();
        let cache_dir = request.cache_dir.clone();
        let (sender, _) = mpsc::sync_channel(4);
        assert!(matches!(
            run(request, &sender, &AtomicBool::new(true)).unwrap(),
            Event::Cancelled
        ));
        assert!(!cache_dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
