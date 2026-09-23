//! Wait for explicitly required game mounts before startup discovery.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{DegaussError, Result};
use crate::index_job::{Discovered, DiscoveryRequest};
use crate::systems::FoundSystem;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const WAIT_TIMEOUT: Duration = Duration::from_secs(180);
const MOUNTINFO_PATH: &str = "/proc/self/mountinfo";

#[derive(Debug, Clone)]
pub struct Plan {
    targets: Vec<PathBuf>,
    mountinfo_path: PathBuf,
    timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Waiting,
    Discovering,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub elapsed: Duration,
    pub activity: String,
    pub local_only: bool,
    pub cancelling: bool,
}

impl Progress {
    fn waiting(local_only: bool) -> Self {
        Self {
            phase: if local_only {
                Phase::Discovering
            } else {
                Phase::Waiting
            },
            elapsed: Duration::ZERO,
            activity: if local_only {
                "Discovering available local systems".into()
            } else {
                "Waiting for required game storage".into()
            },
            local_only,
            cancelling: false,
        }
    }
}

type JobResult = Result<Option<Discovered>>;

pub struct Job {
    result: Option<Receiver<JobResult>>,
    progress: Arc<Mutex<Progress>>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn start(plan: Plan, discovery: DiscoveryRequest, local_only: bool) -> Result<Self> {
        let (sender, result) = mpsc::sync_channel(1);
        let progress = Arc::new(Mutex::new(Progress::waiting(local_only)));
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_progress = Arc::clone(&progress);
        let worker_cancelled = Arc::clone(&cancelled);
        let handle = std::thread::Builder::new()
            .name("degauss-game-mount-wait".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(
                        &plan,
                        discovery,
                        local_only,
                        &worker_progress,
                        &worker_cancelled,
                    )
                }))
                .unwrap_or_else(|_| {
                    Err(DegaussError::unsupported(
                        "required game storage",
                        "startup worker panicked",
                    ))
                });
                let _ = sender.send(result);
            })
            .map_err(|error| {
                DegaussError::unsupported(
                    "required game storage",
                    format!("starting worker: {error}"),
                )
            })?;
        Ok(Self {
            result: Some(result),
            progress,
            cancelled,
            handle: Some(handle),
        })
    }

    pub fn progress(&self) -> Progress {
        self.progress
            .lock()
            .map(|progress| progress.clone())
            .unwrap_or_else(|_| Progress::waiting(false))
    }

    pub fn try_recv(&mut self) -> Option<JobResult> {
        let result = match self.result.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(DegaussError::unsupported(
                "required game storage",
                "startup worker stopped without returning its result",
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
        if let Ok(mut progress) = self.progress.lock() {
            progress.cancelling = true;
            progress.activity = "Stopping safely".into();
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        self.result.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn detect(required_mounts: &[String]) -> Result<Option<Plan>> {
    detect_with_mountinfo(required_mounts, PathBuf::from(MOUNTINFO_PATH))
}

fn detect_with_mountinfo(
    required_mounts: &[String],
    mountinfo_path: PathBuf,
) -> Result<Option<Plan>> {
    if required_mounts.is_empty() {
        return Ok(None);
    }
    let plan = Plan {
        targets: required_mounts.iter().map(PathBuf::from).collect(),
        mountinfo_path,
        timeout: WAIT_TIMEOUT,
    };
    let mountinfo = read_mountinfo(&plan.mountinfo_path)?;
    Ok((!plan.missing(&mountinfo).is_empty()).then_some(plan))
}

fn read_mountinfo(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| DegaussError::io("reading mounted filesystems", path, error))
}

fn run(
    plan: &Plan,
    discovery: DiscoveryRequest,
    local_only: bool,
    progress: &Mutex<Progress>,
    cancelled: &AtomicBool,
) -> JobResult {
    let started = Instant::now();
    if !local_only {
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let mountinfo = read_mountinfo(&plan.mountinfo_path)?;
            let missing = plan.missing(&mountinfo);
            if missing.is_empty() {
                break;
            }
            let missing_names = missing
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            if started.elapsed() >= plan.timeout {
                return Err(DegaussError::unsupported(
                    "required game storage",
                    format!(
                        "Required mountpoint(s) did not appear within {} seconds: {missing_names}",
                        plan.timeout.as_secs()
                    ),
                ));
            }
            if let Ok(mut current) = progress.lock() {
                current.elapsed = started.elapsed();
                current.activity = format!("Waiting for {missing_names}");
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if let Ok(mut current) = progress.lock() {
        current.phase = Phase::Discovering;
        current.elapsed = started.elapsed();
        current.activity = if local_only {
            "Discovering available local systems".into()
        } else {
            "Required storage ready; discovering systems".into()
        };
    }
    let cores = crate::systems::CoreIndex::read(&discovery.menu_root);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let excluded = if local_only {
        plan.missing(&read_mountinfo(&plan.mountinfo_path)?)
            .into_iter()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut systems = crate::systems::discover_checked_excluding(
        &discovery.table,
        &discovery.roots,
        discovery.logo_dir.as_deref(),
        &cores,
        &excluded,
    )?;
    if local_only {
        remove_excluded_paths(&mut systems, &excluded);
    }
    let catalogue = cores.catalogue(&discovery.table);
    Ok((!cancelled.load(Ordering::Relaxed)).then_some(Discovered {
        systems,
        cores: catalogue,
    }))
}

fn remove_excluded_paths(systems: &mut Vec<FoundSystem>, excluded: &[PathBuf]) {
    for system in systems.iter_mut() {
        system
            .paths
            .retain(|path| !excluded.iter().any(|target| path.starts_with(target)));
    }
    systems.retain(|system| !system.paths.is_empty());
}

impl Plan {
    #[cfg(test)]
    pub(crate) fn fixture(
        targets: Vec<PathBuf>,
        mountinfo_path: PathBuf,
        timeout: Duration,
    ) -> Self {
        Self {
            targets,
            mountinfo_path,
            timeout,
        }
    }

    fn missing<'a>(&'a self, mountinfo: &str) -> Vec<&'a Path> {
        let mounted = mount_points(mountinfo);
        self.targets
            .iter()
            .filter(|target| !mounted.contains(target))
            .map(PathBuf::as_path)
            .collect()
    }
}

fn mount_points(mountinfo: &str) -> Vec<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|line| line.split_whitespace().nth(4))
        .map(decode_mount_field)
        .map(PathBuf::from)
        .collect()
}

fn decode_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\' && at + 3 < bytes.len() {
            let octal = &bytes[at + 1..at + 4];
            if octal.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
                decoded.push((octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + (octal[2] - b'0'));
                at += 4;
                continue;
            }
        }
        decoded.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        (0..)
            .find_map(|attempt| {
                let path = std::env::temp_dir().join(format!(
                    "degauss-mount-{tag}-{}-{attempt}",
                    std::process::id()
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => Some(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => panic!("creating test directory: {error}"),
                }
            })
            .expect("test directory counter exhausted")
    }

    fn request(root: &Path, roots: Vec<PathBuf>) -> DiscoveryRequest {
        DiscoveryRequest {
            menu_root: root.to_path_buf(),
            roots,
            table: crate::systems::parse_table(
                "[[systems]]\nname = \"NES\"\nid = \"NES\"\nfolders = [\"NES\"]\nrbf = \"_Console/NES\"\nextensions = [\"rom\"]\ncategory = \"Console\"\n",
                Path::new("systems.toml"),
            )
            .unwrap(),
            logo_dir: None,
        }
    }

    fn run_now(plan: &Plan, discovery: DiscoveryRequest, local_only: bool) -> JobResult {
        run(
            plan,
            discovery,
            local_only,
            &Mutex::new(Progress::waiting(local_only)),
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn empty_configuration_never_reads_mounts_or_stock_script() {
        assert!(
            detect_with_mountinfo(&[], PathBuf::from("/missing/mountinfo"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn required_mounts_use_the_mount_table_not_directory_existence_or_fs_type() {
        let root = temp_dir("detect");
        let target = root.join("games");
        std::fs::create_dir(&target).unwrap();
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let required = vec![target.to_string_lossy().into_owned()];
        assert!(detect_with_mountinfo(&required, mountinfo.clone())
            .unwrap()
            .is_some());
        std::fs::write(
            &mountinfo,
            format!(
                "31 22 0:30 / {} rw - nfs server:/games rw\n",
                target.display()
            ),
        )
        .unwrap();
        assert!(detect_with_mountinfo(&required, mountinfo)
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_late_required_mount_starts_discovery_without_a_script_or_log() {
        let root = temp_dir("late-mount");
        let parent = root.join("share");
        std::fs::create_dir_all(parent.join("games/NES")).unwrap();
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let plan = Plan::fixture(vec![parent.clone()], mountinfo.clone(), WAIT_TIMEOUT);
        let mut job = Job::start(plan, request(&root, vec![parent.join("games")]), false).unwrap();
        assert_eq!(job.progress().phase, Phase::Waiting);
        std::fs::write(
            &mountinfo,
            format!(
                "31 22 0:30 / {} rw - nfs server:/games rw\n",
                parent.display()
            ),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let discovered = loop {
            if let Some(result) = job.try_recv() {
                break result.unwrap().unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "discovery did not follow the mount"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(discovered.systems[0].paths, vec![parent.join("games/NES")]);
        drop(job);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_mounts_decode_spaces_and_identify_each_missing_path() {
        let plan = Plan::fixture(
            vec![
                PathBuf::from("/media/fat/Amiga CD32"),
                PathBuf::from("/media/fat/cifs"),
            ],
            PathBuf::from("/proc/self/mountinfo"),
            WAIT_TIMEOUT,
        );
        let mountinfo = "31 22 0:30 / /media/fat/Amiga\\040CD32 rw - cifs //nas/share rw\n";
        assert_eq!(plan.missing(mountinfo), vec![Path::new("/media/fat/cifs")]);
    }

    #[test]
    fn local_only_skips_an_unmounted_parent_and_uses_a_later_local_copy() {
        let root = temp_dir("local-fallback");
        let parent = root.join("fat/cifs");
        let local = root.join("fat/games");
        std::fs::create_dir_all(parent.join("games/NES")).unwrap();
        std::fs::create_dir_all(local.join("NES")).unwrap();
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let plan = Plan::fixture(vec![parent.clone()], mountinfo, WAIT_TIMEOUT);

        let discovered = run_now(
            &plan,
            request(&root, vec![parent.join("games"), local.clone()]),
            true,
        )
        .unwrap()
        .unwrap();

        assert_eq!(discovered.systems.len(), 1);
        assert_eq!(discovered.systems[0].paths, vec![local.join("NES")]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_only_keeps_already_mounted_roots_when_another_is_missing() {
        let root = temp_dir("partial-mounts");
        let mounted = root.join("mounted");
        let missing = root.join("missing");
        std::fs::create_dir_all(mounted.join("NES")).unwrap();
        std::fs::create_dir_all(missing.join("NES")).unwrap();
        let mountinfo = root.join("mountinfo");
        std::fs::write(
            &mountinfo,
            format!(
                "31 22 0:30 / {} rw - cifs //nas/share rw\n",
                mounted.display()
            ),
        )
        .unwrap();
        let plan = Plan::fixture(
            vec![mounted.clone(), missing.clone()],
            mountinfo,
            WAIT_TIMEOUT,
        );
        let discovered = run_now(&plan, request(&root, vec![missing, mounted.clone()]), true)
            .unwrap()
            .unwrap();
        assert_eq!(discovered.systems[0].paths, vec![mounted.join("NES")]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_stops_before_discovery_and_timeout_names_missing_mount() {
        let root = temp_dir("cancel-timeout");
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let missing = root.join("games");
        let plan = Plan::fixture(vec![missing.clone()], mountinfo, Duration::ZERO);
        let cancelled = AtomicBool::new(true);
        assert!(run(
            &plan,
            request(&root, vec![missing.clone()]),
            false,
            &Mutex::new(Progress::waiting(false)),
            &cancelled,
        )
        .unwrap()
        .is_none());
        let error = run_now(&plan, request(&root, vec![missing.clone()]), false)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("0 seconds"));
        assert!(error.contains(&missing.to_string_lossy().to_string()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
