//! Process-owned orchestration around the released scanner and publisher.
//! The UI must paint the current scope before dispatch and serialize writers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::browse::{DisplayNames, Library};
use crate::cache::{Index, Summary, SystemCache};
use crate::config::SystemConfig;
use crate::error::{DegaussError, Result};

const EVENT_CAPACITY: usize = 4;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub struct DiscoveryRequest {
    pub menu_root: PathBuf,
    pub roots: Vec<PathBuf>,
    pub table: Vec<crate::systems::SystemDef>,
    pub logo_dir: Option<PathBuf>,
}

type DiscoveryResult = Result<Option<Vec<crate::systems::FoundSystem>>>;

pub struct DiscoveryJob {
    result: Option<Receiver<DiscoveryResult>>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DiscoveryJob {
    pub fn start(request: DiscoveryRequest) -> Result<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let handle = std::thread::Builder::new()
            .name("degauss-discover".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if worker_cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let cores = crate::systems::CoreIndex::read(&request.menu_root);
                    if worker_cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let found = crate::systems::discover_checked(
                        &request.table,
                        &request.roots,
                        request.logo_dir.as_deref(),
                        &cores,
                    )?;
                    Ok((!worker_cancelled.load(Ordering::Relaxed)).then_some(found))
                }))
                .unwrap_or_else(|_| {
                    Err(DegaussError::unsupported(
                        "system discovery",
                        "discovery worker panicked",
                    ))
                });
                let _ = sender.send(result);
            })
            .map_err(|error| {
                DegaussError::unsupported("system discovery", format!("starting worker: {error}"))
            })?;
        Ok(Self {
            result: Some(result),
            cancelled,
            handle: Some(handle),
        })
    }

    pub fn try_recv(&mut self) -> Option<DiscoveryResult> {
        let result = match self.result.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DegaussError::unsupported(
                "system discovery",
                "discovery worker stopped without returning its result",
            )),
        };
        self.result = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Some(result)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for DiscoveryJob {
    fn drop(&mut self) {
        self.cancel();
        self.result.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub struct Request {
    pub id: String,
    pub config: SystemConfig,
    pub names: DisplayNames,
    pub cache_dir: PathBuf,
    pub forced: bool,
    pub artwork_pack: bool,
    pub index: Index,
    pub retain_cache: bool,
}

#[derive(Debug)]
pub enum Event {
    Progress {
        folders: usize,
        games: usize,
        folder: String,
    },
    Ready {
        index: Index,
        summary: Option<Summary>,
        cache: Option<SystemCache>,
        warnings: Vec<String>,
        folders: usize,
        games: usize,
        elapsed: Duration,
    },
    Cancelled {
        index: Index,
    },
    Failed {
        error: DegaussError,
        index: Index,
    },
}

#[derive(Debug)]
pub struct StartError {
    pub error: DegaussError,
    pub index: Index,
}

pub struct Job {
    events: Option<Receiver<Event>>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn try_recv(&mut self) -> Option<Event> {
        match self.events.as_ref()?.try_recv() {
            Ok(event) => {
                if !matches!(event, Event::Progress { .. }) {
                    if let Some(handle) = self.handle.take() {
                        // The terminal send is the worker's final operation.
                        let _ = handle.join();
                    }
                    self.events = None;
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                // Worker panics are converted to Failed before disconnect.
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                self.events = None;
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        // Release a potentially full terminal channel before joining. This
        // also guarantees there is no detached indexing work after UI exit.
        self.events.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn start(request: Request) -> std::result::Result<Job, StartError> {
    let (sender, events) = mpsc::sync_channel(EVENT_CAPACITY);
    let (input, receive_input) = mpsc::sync_channel::<Request>(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = match std::thread::Builder::new()
        .name("degauss-index".to_string())
        .spawn(move || {
            let Ok(mut request) = receive_input.recv() else {
                return;
            };
            let began = Instant::now();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&mut request, &sender, &worker_cancelled)
            }))
            .unwrap_or_else(|_| {
                Err(DegaussError::unsupported(
                    "indexing",
                    "index worker panicked",
                ))
            });
            let event = match result {
                Ok(Some(ready)) => Event::Ready {
                    index: request.index,
                    summary: ready.summary,
                    cache: ready.cache,
                    warnings: ready.warnings,
                    folders: ready.folders,
                    games: ready.games,
                    elapsed: began.elapsed(),
                },
                Ok(None) => Event::Cancelled {
                    index: request.index,
                },
                Err(error) => Event::Failed {
                    error,
                    index: request.index,
                },
            };
            let _ = sender.send(event);
        }) {
        Ok(handle) => handle,
        Err(error) => {
            return Err(StartError {
                error: DegaussError::unsupported(
                    "indexing",
                    format!("starting index worker: {error}"),
                ),
                index: request.index,
            })
        }
    };
    if let Err(error) = input.send(request) {
        let _ = handle.join();
        return Err(StartError {
            error: DegaussError::unsupported(
                "indexing",
                "index worker stopped before receiving work",
            ),
            index: error.0.index,
        });
    }
    Ok(Job {
        events: Some(events),
        cancelled,
        handle: Some(handle),
    })
}

struct Ready {
    summary: Option<Summary>,
    cache: Option<SystemCache>,
    warnings: Vec<String>,
    folders: usize,
    games: usize,
}

fn run(
    request: &mut Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
) -> Result<Option<Ready>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let start = crate::browse::start_for(&request.config);
    let cached = if request.artwork_pack {
        crate::cache::load_artwork_pack_system(&request.cache_dir, &request.id)
    } else if !request.forced {
        crate::cache::load_system(&request.cache_dir, &request.id)
    } else {
        None
    };
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if request.artwork_pack && cached.is_none() {
        return Err(DegaussError::unsupported(
            "Artwork Pack cache",
            format!(
                "{}: the prepared system cache is missing or unreadable",
                request.id
            ),
        ));
    }
    if cached.is_some() {
        let summary = cached.as_ref().map(|cache| cache.summary(&start));
        if let Some(summary) = summary {
            request.index.systems.insert(request.id.clone(), summary);
        }
        return Ok(Some(Ready {
            summary,
            folders: cached.as_ref().map_or(0, |cache| cache.folders.len()),
            games: summary.map_or(0, |summary| summary.games),
            cache: if request.retain_cache { cached } else { None },
            warnings: Vec::new(),
        }));
    }
    let library = Library::open_with_names(&request.config, std::mem::take(&mut request.names))?;
    let mut last_progress = Instant::now() - PROGRESS_INTERVAL;
    let mut warnings = Vec::new();
    let Some(cache) = crate::cache::build_system_observed(
        &library,
        cancelled,
        &mut warnings,
        &mut |place, folders, games, _| {
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                let folder = match place {
                    crate::browse::Place::Roots => "System Folders".to_string(),
                    crate::browse::Place::ArchiveDirectory { archive, prefix } => {
                        format!("{}/{}", archive.display(), prefix)
                    }
                    _ => place.path().display().to_string(),
                };
                let _ = events.try_send(Event::Progress {
                    folders,
                    games,
                    folder,
                });
                last_progress = Instant::now();
            }
        },
    )?
    else {
        return Ok(None);
    };
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let summary = cache.summary(&start);
    let previous = request.index.systems.insert(request.id.clone(), summary);
    // Keep the released complete-system transaction unchanged. Cancellation
    // during publication waits for its result, rather than interrupting it.
    let publication = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::cache::save_system_with_index(
            &request.cache_dir,
            &request.id,
            &cache,
            &request.index,
        )
    }))
    .unwrap_or_else(|_| {
        Err(DegaussError::unsupported(
            "indexing",
            "index publication panicked; inspect the cache before retrying",
        ))
    });
    match publication {
        Ok(published) => warnings.extend(published),
        Err(error) => {
            if let Some(previous) = previous {
                request.index.systems.insert(request.id.clone(), previous);
            } else {
                request.index.systems.remove(&request.id);
            }
            return Err(error);
        }
    }
    Ok(Some(Ready {
        summary: Some(summary),
        folders: cache.folders.len(),
        games: summary.games,
        cache: request.retain_cache.then_some(cache),
        warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, Request) {
        let root = (0..)
            .find_map(|serial| {
                let path = std::env::temp_dir()
                    .join(format!("degauss-ui-index-{}-{serial}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => Some(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("fixture: {error}"),
                }
            })
            .unwrap();
        let games = root.join("games");
        std::fs::create_dir(&games).unwrap();
        std::fs::write(games.join("One.rom"), b"fixture").unwrap();
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
            index: Index::new(),
            retain_cache: true,
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
            assert!(Instant::now() < deadline, "worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn publishes_matching_released_cache_and_index_and_returns_retained_rows() {
        let (root, mut request) = fixture();
        request.index.systems.insert(
            "Other".into(),
            Summary {
                games: 7,
                folders: 1,
            },
        );
        let dir = request.cache_dir.clone();
        let mut job = start(request).unwrap();
        let Event::Ready {
            index,
            summary: Some(summary),
            cache: Some(cache),
            warnings,
            folders,
            games,
            ..
        } = terminal(&mut job)
        else {
            panic!("expected complete cache");
        };
        assert_eq!(summary.games, 1);
        assert_eq!(games, 1);
        assert_eq!(folders, cache.folders.len());
        assert_eq!(index.systems["Other"].games, 7);
        assert_eq!(
            crate::cache::load_index(&dir).unwrap().systems,
            index.systems
        );
        assert!(crate::cache::load_system(&dir, "Test").is_some());
        assert!(warnings.is_empty());
        assert!(job.handle.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_cancelled_request_preserves_index_and_does_not_write() {
        let (root, mut request) = fixture();
        request.index.systems.insert(
            "Test".into(),
            Summary {
                games: 9,
                folders: 1,
            },
        );
        let (sender, _) = mpsc::sync_channel(EVENT_CAPACITY);
        assert!(run(&mut request, &sender, &AtomicBool::new(true))
            .unwrap()
            .is_none());
        assert_eq!(request.index.systems["Test"].games, 9);
        assert!(!request.cache_dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// One corrupt archive must not cost the system its healthy games: the
    /// scan completes, the warning names the archive, and what is published
    /// is the healthy subset with the same count the worker reports.
    #[test]
    fn corrupt_zip_is_skipped_and_the_healthy_subset_is_published() {
        let (root, mut request) = fixture();
        let (sender, _) = mpsc::sync_channel(EVENT_CAPACITY);
        assert!(run(&mut request, &sender, &AtomicBool::new(false))
            .unwrap()
            .is_some());
        let broken = root.join("games/Broken.zip");
        std::fs::write(&broken, b"broken").unwrap();
        let ready = run(&mut request, &sender, &AtomicBool::new(false))
            .unwrap()
            .expect("not cancelled");
        assert_eq!(ready.games, 1);
        assert_eq!(ready.summary.unwrap().games, 1);
        assert_eq!(ready.warnings.len(), 1, "{:?}", ready.warnings);
        assert!(
            ready.warnings[0].starts_with(&format!("{}: skipped: ", broken.display())),
            "{:?}",
            ready.warnings
        );
        assert_eq!(request.index.systems["Test"].games, 1);
        let published = crate::cache::load_system(&request.cache_dir, "Test").unwrap();
        assert_eq!(
            published
                .summary(&crate::browse::start_for(&request.config))
                .games,
            1
        );
        assert!(!published
            .folders
            .contains_key(&crate::browse::Place::Archive(broken).key()));
        assert_eq!(
            crate::cache::load_index(&request.cache_dir)
                .unwrap()
                .systems["Test"]
                .games,
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A system whose root cannot be read is a failed transaction, not an
    /// empty success: the previous cache and index bytes stay exactly as
    /// they were, and the index still carries the previous count.
    #[test]
    fn root_read_failure_keeps_the_previous_cache_and_index() {
        let (root, mut request) = fixture();
        let (sender, _) = mpsc::sync_channel(EVENT_CAPACITY);
        assert!(run(&mut request, &sender, &AtomicBool::new(false))
            .unwrap()
            .is_some());
        let cache_path = crate::cache::system_path(&request.cache_dir, "Test");
        let before = std::fs::read(&cache_path).unwrap();
        let index_before = std::fs::read(crate::cache::index_path(&request.cache_dir)).unwrap();
        std::fs::remove_dir_all(root.join("games")).unwrap();
        std::fs::write(root.join("games"), b"not a directory").unwrap();
        let error = match run(&mut request, &sender, &AtomicBool::new(false)) {
            Err(error) => error,
            Ok(_) => panic!("an unreadable system root must fail the transaction"),
        };
        assert!(error.to_string().contains("reading folder"), "{error}");
        assert_eq!(request.index.systems["Test"].games, 1);
        assert_eq!(std::fs::read(cache_path).unwrap(), before);
        assert_eq!(
            std::fs::read(crate::cache::index_path(&request.cache_dir)).unwrap(),
            index_before
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_failure_returns_previous_summary() {
        let (root, mut request) = fixture();
        request.index.systems.insert(
            "Test".into(),
            Summary {
                games: 9,
                folders: 1,
            },
        );
        std::fs::write(&request.cache_dir, b"not a directory").unwrap();
        let mut job = start(request).unwrap();
        let Event::Failed { index, .. } = terminal(&mut job) else {
            panic!("publication must fail");
        };
        assert_eq!(index.systems["Test"].games, 9);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_pack_cache_fails_without_scanning_or_publishing_gamelist_data() {
        let (root, mut request) = fixture();
        request.artwork_pack = true;
        std::fs::remove_dir_all(root.join("games")).unwrap();
        let dir = request.cache_dir.clone();
        let mut job = start(request).unwrap();
        let Event::Failed { error, .. } = terminal(&mut job) else {
            panic!("missing Pack cache must not be reported as a completed empty system");
        };
        assert!(error
            .to_string()
            .contains("prepared system cache is missing or unreadable"));
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn complete_pack_cache_returns_actual_counts_without_ordinary_scanning() {
        let (root, mut request) = fixture();
        let library = Library::open_source_neutral(&request.config, Default::default()).unwrap();
        let cache = crate::cache::build_system(&library);
        crate::cache::stage_transactional(
            &request.cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            vec![crate::cache::StagedSystemCache {
                id: request.id.clone(),
                cache,
                fingerprints: Default::default(),
                fingerprints_complete: true,
            }],
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap()
        .install()
        .unwrap();
        request.artwork_pack = true;
        std::fs::remove_dir_all(root.join("games")).unwrap();
        let dir = request.cache_dir.clone();
        let mut job = start(request).unwrap();
        let Event::Ready {
            summary: Some(summary),
            cache: Some(cache),
            games,
            index,
            ..
        } = terminal(&mut job)
        else {
            panic!("complete Pack cache must retain its game counts");
        };
        assert_eq!(summary.games, 1);
        assert_eq!(games, 1);
        assert_eq!(index.systems["Test"], summary);
        assert!(!cache.folders.is_empty());
        assert!(!crate::cache::system_path(&dir, "Test").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn drop_unblocks_a_full_event_channel_and_joins_the_worker() {
        let (sender, events) = mpsc::sync_channel(1);
        sender
            .send(Event::Progress {
                folders: 1,
                games: 1,
                folder: "System Folders".into(),
            })
            .unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let handle = std::thread::spawn(move || {
            let _ = sender.send(Event::Cancelled {
                index: Index::new(),
            });
            worker_finished.store(true, Ordering::Relaxed);
        });
        let job = Job {
            events: Some(events),
            cancelled: Arc::new(AtomicBool::new(false)),
            handle: Some(handle),
        };
        drop(job);
        assert!(finished.load(Ordering::Relaxed));
    }
}
