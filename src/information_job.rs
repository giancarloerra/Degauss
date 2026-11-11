//! Selected-game description reads owned by the current process, not browsing.
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc,
};
use std::thread::JoinHandle;

use crate::{
    browse::Launch,
    config::SystemConfig,
    error::{DegaussError, Result},
};

pub enum Source {
    Gamelist,
    ArtworkPack(crate::artwork_pack::Provider),
}

pub struct Request {
    /// Resolve a Favorite to its owning system and original launch first.
    pub launch: Launch,
    pub config: SystemConfig,
    pub source: Source,
}

#[derive(Debug)]
pub enum Event {
    Ready(Option<String>),
    Failed(DegaussError),
    Cancelled,
}

pub struct Job {
    receiver: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn try_recv(&mut self) -> Option<Event> {
        let event = match self.receiver.try_recv() {
            Ok(event) => event,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) if self.handle.is_some() => {
                Event::Failed(DegaussError::unsupported(
                    "game information",
                    "description worker stopped without a result",
                ))
            }
            Err(mpsc::TryRecvError::Disconnected) => return None,
        };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Some(event)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn start(request: Request) -> Result<Job> {
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-info".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(&request, &flag)))
                    .unwrap_or_else(|_| {
                        Err(DegaussError::unsupported(
                            "game information",
                            "description worker panicked",
                        ))
                    });
            let event = if flag.load(Ordering::Relaxed) {
                Event::Cancelled
            } else {
                match result {
                    Ok(text) => Event::Ready(text),
                    Err(error) => Event::Failed(error),
                }
            };
            let _ = sender.send(event);
        })
        .map_err(|error| {
            DegaussError::unsupported(
                "game information",
                format!("starting description worker: {error}"),
            )
        })?;
    Ok(Job {
        receiver,
        cancelled,
        handle: Some(handle),
    })
}

pub(crate) fn read(request: &Request, cancelled: &AtomicBool) -> Result<Option<String>> {
    read_using(
        request,
        cancelled,
        crate::gamelist::Gamelist::full_description,
    )
}

/// The browser repeatedly opens different games without leaving the system.
/// Retain only the last gamelist, and check its file revision on every read.
#[derive(Default)]
pub(crate) struct DescriptionCache {
    gamelist: Option<(
        PathBuf,
        u64,
        std::time::SystemTime,
        crate::gamelist::Gamelist,
    )>,
}

impl DescriptionCache {
    pub fn read(&mut self, request: &Request, cancelled: &AtomicBool) -> Result<Option<String>> {
        read_using(request, cancelled, |path, folder, key, exact, cancelled| {
            let metadata = std::fs::metadata(path)
                .map_err(|error| DegaussError::io("reading game information", path, error))?;
            let modified = metadata.modified().map_err(|error| {
                DegaussError::io("reading game information revision", path, error)
            })?;
            if self
                .gamelist
                .as_ref()
                .is_none_or(|(saved, size, stamp, _)| {
                    saved != path || *size != metadata.len() || *stamp != modified
                })
            {
                self.gamelist = None;
                let list = crate::gamelist::Gamelist::load_descriptions(path, folder, cancelled)?;
                self.gamelist = Some((path.to_path_buf(), metadata.len(), modified, list));
            }
            let list = &self.gamelist.as_ref().expect("current gamelist").3;
            Ok(if exact {
                list.lookup_exact(key)
            } else {
                list.lookup(key)
            }
            .and_then(|(meta, _)| meta.desc.clone()))
        })
    }
}

fn read_using(
    request: &Request,
    cancelled: &AtomicBool,
    mut gamelist_description: impl FnMut(
        &Path,
        &Path,
        &str,
        bool,
        &AtomicBool,
    ) -> Result<Option<String>>,
) -> Result<Option<String>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if let Source::ArtworkPack(provider) = &request.source {
        return provider.full_description(&request.launch, cancelled);
    }
    let path = match &request.launch {
        Launch::File(path) => path.as_path(),
        Launch::AmigaVision { install, .. } => install.as_path(),
    };
    let root = std::iter::once(&request.config.path)
        .chain(request.config.extra_paths.iter())
        .map(PathBuf::from)
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.as_os_str().len())
        .ok_or_else(|| {
            DegaussError::unsupported(
                "game information",
                "selected game is outside its system's declared folders",
            )
        })?;
    let key = match &request.launch {
        Launch::File(path) => path
            .strip_prefix(&root)
            .expect("selected root")
            .to_string_lossy()
            .into_owned(),
        Launch::AmigaVision { title, .. } => title.clone(),
    };
    let exact = match crate::zip::split_member_path(path) {
        Some((archive, _)) => {
            let mut archives = crate::zip::ArchiveCache::default();
            let Some(contents) = archives.read_controlled(&archive, cancelled)? else {
                return Ok(None);
            };
            contents
                .entries
                .iter()
                .filter(|entry| request.config.accepts(Path::new(&entry.name)))
                .count()
                != 1
        }
        None => false,
    };
    let gamelist = root.join("gamelist.xml");
    match std::fs::metadata(&gamelist) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading game information",
                &gamelist,
                error,
            ))
        }
        Ok(_) => {}
    }
    gamelist_description(&gamelist, &root, &key, exact, cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, Request) {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "degauss-information-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let config = SystemConfig {
            name: "Information".into(),
            path: root.to_string_lossy().into_owned(),
            extensions: vec!["nes".into()],
            rbf: String::new(),
            launch: vec![],
            setname: None,
            same_dir: false,
            compatible_cores: Vec::new(),
            minimum_build: None,
            skip_folders: vec![],
            extra_paths: vec![],
            preserve_rbf_stem: false,
        };
        let request = Request {
            launch: Launch::File(root.join("Game.nes")),
            config,
            source: Source::Gamelist,
        };
        (root, request)
    }

    #[test]
    fn browser_description_cache_preserves_native_matching_and_refreshes_changed_files() {
        let (root, mut request) = fixture();
        let file = root.join("gamelist.xml");
        let xml = "<gameList><game id=\"p\"><desc>Inherited &amp; complete\nSecond paragraph.</desc></game><game parentid=\"p\"><path>Game.nes</path></game><game><path>Title.slug</path><desc>Slug\nFull</desc></game><game><path>Duplicate.nes</path><desc>Old</desc></game><game><path>Duplicate.nes</path><desc>New\nFull</desc></game><game><path>Empty.nes</path><name>No description</name></game></gameList>";
        std::fs::write(&file, xml).unwrap();
        let mut cache = DescriptionCache::default();
        let cancel = AtomicBool::new(false);
        for name in [
            "Game.nes",
            "Title.nes",
            "Duplicate.nes",
            "Empty.nes",
            "Missing.nes",
            "Game.nes",
        ] {
            request.launch = Launch::File(root.join(name));
            assert_eq!(
                cache.read(&request, &cancel).unwrap(),
                read(&request, &cancel).unwrap(),
                "{name}"
            );
        }
        let stored = cache
            .gamelist
            .as_ref()
            .unwrap()
            .3
            .lookup("Game.nes")
            .unwrap()
            .0
            .desc
            .as_ref()
            .unwrap()
            .as_ptr();
        assert!(cache.read(&request, &cancel).unwrap().is_some());
        assert_eq!(
            stored,
            cache
                .gamelist
                .as_ref()
                .unwrap()
                .3
                .lookup("Game.nes")
                .unwrap()
                .0
                .desc
                .as_ref()
                .unwrap()
                .as_ptr(),
            "Repeated game reads must reuse the parsed descriptions"
        );
        std::fs::write(&file, "<gameList><game><path>Game.nes</path><desc>Changed description</desc></game></gameList>").unwrap();
        assert_eq!(
            cache.read(&request, &cancel).unwrap().as_deref(),
            Some("Changed description")
        );
        std::fs::write(&file, "<gameList><game></bad>").unwrap();
        assert!(
            cache.read(&request, &cancel).is_err(),
            "A changed, broken source must not return cached text"
        );
        std::fs::remove_file(&file).unwrap();
        assert!(cache.read(&request, &cancel).unwrap().is_none());
        std::fs::write(&file, xml).unwrap();
        assert_eq!(
            cache.read(&request, &cancel).unwrap(),
            read(&request, &cancel).unwrap()
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(cache.read(&request, &cancel).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_description_worker_returns_complete_text_and_preserves_source() {
        let (root, request) = fixture();
        let xml = "<gameList><game><path>Game.nes</path><desc>First\nSecond paragraph.</desc></game></gameList>";
        std::fs::write(root.join("gamelist.xml"), xml).unwrap();
        let mut job = start(request).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Some(event) = job.try_recv() {
                assert!(
                    matches!(event, Event::Ready(Some(text)) if text == "First\nSecond paragraph.")
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read_to_string(root.join("gamelist.xml")).unwrap(),
            xml
        );
        drop(job);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn description_lookup_uses_deepest_root_and_reports_bad_xml() {
        let (root, mut request) = fixture();
        let inner = root.join("inner");
        std::fs::create_dir(&inner).unwrap();
        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><game><path>Game.nes</path><desc>Wrong</desc></game></gameList>",
        )
        .unwrap();
        request
            .config
            .extra_paths
            .push(inner.to_string_lossy().into_owned());
        request.launch = Launch::File(inner.join("Game.nes"));
        assert_eq!(read(&request, &AtomicBool::new(false)).unwrap(), None);
        std::fs::write(inner.join("gamelist.xml"), "<gameList><game></bad>").unwrap();
        assert!(read(&request, &AtomicBool::new(false)).is_err());
        assert_eq!(read(&request, &AtomicBool::new(true)).unwrap(), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_descriptions_use_exact_multi_game_and_legacy_single_game_matching() {
        let (root, mut request) = fixture();
        let mut cached = DescriptionCache::default();
        let archive = root.join("collection.zip");
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&["folder/Game.nes", "Other.nes"], false),
        )
        .unwrap();
        request.launch = Launch::File(archive.join("folder/Game.nes"));
        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><game><path>Game.nes</path><desc>Legacy\nFull text</desc></game></gameList>",
        )
        .unwrap();
        assert_eq!(
            read(&request, &AtomicBool::new(false)).unwrap(),
            None,
            "multi-game archives must not match another file's description"
        );
        assert_eq!(
            cached.read(&request, &AtomicBool::new(false)).unwrap(),
            None
        );
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&["folder/Game.nes"], false),
        )
        .unwrap();
        assert_eq!(
            read(&request, &AtomicBool::new(false)).unwrap().as_deref(),
            Some("Legacy\nFull text")
        );
        assert_eq!(
            cached.read(&request, &AtomicBool::new(false)).unwrap(),
            read(&request, &AtomicBool::new(false)).unwrap()
        );
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&["folder/Game.nes", "Other.nes"], true),
        )
        .unwrap();
        std::fs::write(root.join("gamelist.xml"), "<gameList><game><path>collection.zip/folder/Game.nes</path><desc>Exact\nFull text</desc></game></gameList>").unwrap();
        assert_eq!(
            read(&request, &AtomicBool::new(false)).unwrap().as_deref(),
            Some("Exact\nFull text")
        );
        assert_eq!(
            cached.read(&request, &AtomicBool::new(false)).unwrap(),
            read(&request, &AtomicBool::new(false)).unwrap()
        );
        std::fs::write(&archive, b"broken zip").unwrap();
        assert!(read(&request, &AtomicBool::new(false)).is_err());
        assert!(cached.read(&request, &AtomicBool::new(false)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
