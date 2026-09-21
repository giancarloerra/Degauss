//! Wait for MiSTer's boot-managed CIFS library before startup discovery.
//!
//! The official CIFS script starts in the background. A cold boot can
//! therefore reach Degauss before the configured game mount exists. This
//! module reads only the script's non-secret mount settings, observes the
//! kernel mount table, and keeps discovery off the card until the relevant
//! mount is ready or the bounded wait is explicitly cancelled.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::error::{DegaussError, Result};
use crate::index_job::{Discovered, DiscoveryRequest};
use crate::systems::{FoundSystem, SystemDef};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SCRIPT_RELATIVE_PATH: &str = "Scripts/cifs_mount.sh";
const MOUNTINFO_PATH: &str = "/proc/self/mountinfo";

const SAFE_KEYS: [&str; 9] = [
    "MOUNT_AT_BOOT",
    "LOCAL_DIR",
    "BASE_PATH",
    "BOOT_START_DELAY_SECONDS",
    "NETWORK_READY_TIMEOUT_SECONDS",
    "DEFAULT_ROUTE_READY_TIMEOUT_SECONDS",
    "DUAL_INTERFACE_SETTLE_SECONDS",
    "SERVER_WAIT_TIMEOUT_SECONDS",
    "BOOT_LOG_PATH",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Targets {
    Explicit(Vec<PathBuf>),
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct Plan {
    targets: Targets,
    base_path: PathBuf,
    log_path: PathBuf,
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
                "Waiting for the CIFS boot mount".into()
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
            .name("degauss-network-library".into())
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
                        "network game library",
                        "startup worker panicked",
                    ))
                });
                let _ = sender.send(result);
            })
            .map_err(|error| {
                DegaussError::unsupported(
                    "network game library",
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
                "network game library",
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

pub fn detect(
    menu_root: &Path,
    game_roots: &[PathBuf],
    table: &[SystemDef],
) -> Result<Option<Plan>> {
    detect_with_mountinfo(menu_root, game_roots, table, PathBuf::from(MOUNTINFO_PATH))
}

fn detect_with_mountinfo(
    menu_root: &Path,
    game_roots: &[PathBuf],
    table: &[SystemDef],
    mountinfo_path: PathBuf,
) -> Result<Option<Plan>> {
    let script = menu_root.join(SCRIPT_RELATIVE_PATH);
    let script_text = match std::fs::read_to_string(&script) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading CIFS boot settings",
                script,
                error,
            ))
        }
    };
    let mut values = parse_settings(
        script_text
            .split("#=========CODE STARTS HERE=========")
            .next()
            .unwrap_or(&script_text),
    );
    let ini = script.with_extension("ini");
    match std::fs::read_to_string(&ini) {
        Ok(text) => values.extend(parse_settings(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DegaussError::io("reading CIFS boot settings", ini, error)),
    }
    if !values
        .get("MOUNT_AT_BOOT")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return Ok(None);
    }

    let base_path = PathBuf::from(
        values
            .get("BASE_PATH")
            .map(String::as_str)
            .unwrap_or("/media/fat"),
    );
    let local_dir = values
        .get("LOCAL_DIR")
        .map(String::as_str)
        .unwrap_or("cifs")
        .trim();
    let targets = if local_dir == "*" {
        let relevant = game_roots
            .iter()
            .any(|root| root == &base_path || root.starts_with(&base_path));
        if !relevant {
            return Ok(None);
        }
        Targets::Dynamic
    } else {
        let targets = local_dir
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| base_path.join(part))
            .filter(|target| target_is_relevant(target, game_roots, table))
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return Ok(None);
        }
        Targets::Explicit(targets)
    };
    let timeout = configured_timeout(&values);
    let log_path = PathBuf::from(
        values
            .get("BOOT_LOG_PATH")
            .map(String::as_str)
            .unwrap_or("/tmp/cifs_mount.log"),
    );
    let plan = Plan {
        targets,
        base_path,
        log_path,
        mountinfo_path,
        timeout,
    };
    let mountinfo = std::fs::read_to_string(&plan.mountinfo_path).map_err(|error| {
        DegaussError::io("reading mounted filesystems", &plan.mountinfo_path, error)
    })?;
    let log = read_optional(&plan.log_path)?.unwrap_or_default();
    if plan.ready(&mountinfo, &log) {
        return Ok(None);
    }
    Ok(Some(plan))
}

fn configured_timeout(values: &BTreeMap<String, String>) -> Duration {
    let configured_seconds = [
        ("BOOT_START_DELAY_SECONDS", 8),
        ("NETWORK_READY_TIMEOUT_SECONDS", 45),
        ("DEFAULT_ROUTE_READY_TIMEOUT_SECONDS", 30),
        ("DUAL_INTERFACE_SETTLE_SECONDS", 5),
        ("SERVER_WAIT_TIMEOUT_SECONDS", 60),
    ]
    .into_iter()
    .fold(0_u64, |total, (key, default)| {
        total.saturating_add(positive_seconds(values, key, default))
    });
    Duration::from_secs(configured_seconds.saturating_add(30).clamp(30, 300))
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
            let mountinfo = std::fs::read_to_string(&plan.mountinfo_path).map_err(|error| {
                DegaussError::io("reading mounted filesystems", &plan.mountinfo_path, error)
            })?;
            let log = read_optional(&plan.log_path)?;
            if plan.ready(&mountinfo, log.as_deref().unwrap_or_default()) {
                break;
            }
            if let Some(problem) = log.as_deref().and_then(terminal_problem) {
                return Err(DegaussError::unsupported("network game library", problem));
            }
            if started.elapsed() >= plan.timeout {
                let activity = log
                    .as_deref()
                    .and_then(last_activity)
                    .unwrap_or_else(|| "the CIFS boot mount did not report progress".into());
                return Err(DegaussError::unsupported(
                    "network game library",
                    format!(
                        "CIFS storage did not become ready within {} seconds. {activity}",
                        plan.timeout.as_secs()
                    ),
                ));
            }
            if let Ok(mut current) = progress.lock() {
                current.elapsed = started.elapsed();
                current.activity = log
                    .as_deref()
                    .and_then(last_activity)
                    .unwrap_or_else(|| "Waiting for the CIFS boot mount".into());
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
            "Network storage ready; discovering systems".into()
        };
    }
    let cores = crate::systems::CoreIndex::read(&discovery.menu_root);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let (mountinfo, log, excluded) = if local_only {
        let mountinfo = read_optional(&plan.mountinfo_path)?.unwrap_or_default();
        let log = read_optional(&plan.log_path)?.unwrap_or_default();
        let excluded = plan.local_exclusions(&discovery, &mountinfo, &log);
        (mountinfo, log, excluded)
    } else {
        (String::new(), String::new(), Vec::new())
    };
    let mut systems = crate::systems::discover_checked_excluding(
        &discovery.table,
        &discovery.roots,
        discovery.logo_dir.as_deref(),
        &cores,
        &excluded,
    )?;
    if local_only {
        plan.remove_network_system_paths(&mut systems, &mountinfo, &log);
    }
    let catalogue = cores.catalogue(&discovery.table);
    Ok((!cancelled.load(Ordering::Relaxed)).then_some(Discovered {
        systems,
        cores: catalogue,
    }))
}

impl Plan {
    #[cfg(test)]
    pub(crate) fn fixture(
        targets: Vec<PathBuf>,
        base_path: PathBuf,
        log_path: PathBuf,
        mountinfo_path: PathBuf,
        timeout: Duration,
    ) -> Self {
        Self {
            targets: Targets::Explicit(targets),
            base_path,
            log_path,
            mountinfo_path,
            timeout,
        }
    }

    fn excluded_targets(&self, mountinfo: &str, log: &str) -> Vec<PathBuf> {
        let mut targets = match &self.targets {
            Targets::Explicit(targets) => targets.clone(),
            Targets::Dynamic => dynamic_targets(&self.base_path, mountinfo, log),
        };
        targets.sort();
        targets.dedup();
        targets
    }

    fn local_exclusions(
        &self,
        discovery: &DiscoveryRequest,
        mountinfo: &str,
        log: &str,
    ) -> Vec<PathBuf> {
        let mut targets = self.excluded_targets(mountinfo, log);
        if self.targets == Targets::Dynamic {
            for root in &discovery.roots {
                let Ok(relative) = root.strip_prefix(&self.base_path) else {
                    continue;
                };
                if let Some(component) = first_normal_component(relative) {
                    targets.push(self.base_path.join(component));
                } else {
                    targets.extend(
                        discovery
                            .table
                            .iter()
                            .flat_map(|system| &system.folders)
                            .filter_map(|folder| {
                                let folder = Path::new(folder);
                                (!folder.is_absolute())
                                    .then(|| first_normal_component(folder))
                                    .flatten()
                            })
                            .map(|component| self.base_path.join(component)),
                    );
                }
            }
            targets.sort();
            targets.dedup();
        }
        targets
    }

    fn ready(&self, mountinfo: &str, log: &str) -> bool {
        match &self.targets {
            Targets::Explicit(targets) => {
                targets.iter().all(|target| mounted_at(mountinfo, target))
            }
            Targets::Dynamic => {
                let section = last_boot_section(log);
                section.lines().any(|line| line.trim() == "Done!")
                    && logged_targets(&self.base_path, section)
                        .iter()
                        .all(|target| mounted_at(mountinfo, target))
            }
        }
    }

    fn remove_network_system_paths(
        &self,
        systems: &mut Vec<FoundSystem>,
        mountinfo: &str,
        log: &str,
    ) {
        let targets = self.excluded_targets(mountinfo, log);
        for system in systems.iter_mut() {
            system
                .paths
                .retain(|path| !targets.iter().any(|target| path.starts_with(target)));
        }
        systems.retain(|system| !system.paths.is_empty());
    }
}

fn parse_settings(text: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim().trim_end_matches('\r');
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !SAFE_KEYS.contains(&key) {
            continue;
        }
        let value = value.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value.split('#').next().unwrap_or(value).trim()
        };
        values.insert(key.to_string(), value.to_string());
    }
    values
}

fn positive_seconds(values: &BTreeMap<String, String>, key: &str, default: u64) -> u64 {
    values
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn target_is_relevant(target: &Path, roots: &[PathBuf], table: &[SystemDef]) -> bool {
    roots.iter().any(|root| {
        root == target
            || root.starts_with(target)
            || target.strip_prefix(root).ok().is_some_and(|relative| {
                first_normal_component(relative).is_some_and(|component| {
                    table
                        .iter()
                        .flat_map(|system| &system.folders)
                        .any(|folder| {
                            let folder = Path::new(folder);
                            !folder.is_absolute()
                                && first_normal_component(folder)
                                    .is_some_and(|known| known.eq_ignore_ascii_case(component))
                        })
                })
            })
    })
}

fn first_normal_component(path: &Path) -> Option<&str> {
    path.components().find_map(|component| match component {
        Component::Normal(part) => part.to_str(),
        _ => None,
    })
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DegaussError::io("reading CIFS boot status", path, error)),
    }
}

fn last_boot_section(log: &str) -> &str {
    let offset = log
        .match_indices("==== ")
        .filter(|(at, _)| *at == 0 || log.as_bytes().get(at - 1) == Some(&b'\n'))
        .map(|(at, _)| at)
        .last()
        .unwrap_or(0);
    &log[offset..]
}

fn last_activity(log: &str) -> Option<String> {
    last_boot_section(log)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("==== "))
        .map(|line| {
            line.chars()
                .filter(|ch| !ch.is_control())
                .take(160)
                .collect()
        })
}

fn terminal_problem(log: &str) -> Option<String> {
    let section = last_boot_section(log);
    let lower = section.to_ascii_lowercase();
    if lower.contains("done with ") && lower.contains(" failure") {
        return last_activity(section);
    }
    if lower.contains("network not ready after")
        || lower.contains("default route not ready after")
        || lower.contains(" not found after ")
        || lower.contains(" not reachable after ")
    {
        return last_activity(section);
    }
    if lower.contains("doesn't\nsupport cifs") || lower.contains("doesn't support cifs") {
        return Some("The current MiSTer Linux kernel does not support CIFS.".into());
    }
    if lower.contains("please configure\nthis script") {
        return Some("The CIFS boot script is enabled but not configured.".into());
    }
    None
}

fn mounted_at(mountinfo: &str, target: &Path) -> bool {
    mount_points(mountinfo).iter().any(|path| path == target)
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

fn dynamic_targets(base: &Path, mountinfo: &str, log: &str) -> Vec<PathBuf> {
    let mut targets = mount_points(mountinfo)
        .into_iter()
        .filter(|path| path != base && path.starts_with(base))
        .collect::<Vec<_>>();
    targets.extend(logged_targets(base, last_boot_section(log)));
    targets
}

fn logged_targets(base: &Path, section: &str) -> Vec<PathBuf> {
    let mut targets = Vec::new();
    for line in section.lines().map(str::trim) {
        let label = line
            .strip_suffix(" already mounted")
            .or_else(|| line.strip_suffix(" not mounted"))
            .or_else(|| line.strip_suffix(" mounted"));
        let Some(label) = label else {
            continue;
        };
        let path = Path::new(label);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || path.components().count() != 1
        {
            continue;
        }
        targets.push(base.join(path));
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("degauss-network-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn request(root: &Path, roots: Vec<PathBuf>) -> DiscoveryRequest {
        DiscoveryRequest {
            menu_root: root.to_path_buf(),
            roots,
            table: vec![system("NES", "NES")],
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

    fn system(id: &str, folder: &str) -> SystemDef {
        crate::systems::parse_table(
            &format!(
                "[[systems]]\nname = \"{id}\"\nid = \"{id}\"\nfolders = [\"{folder}\"]\nrbf = \"_Console/{id}\"\nextensions = [\"rom\"]\ncategory = \"Console\"\n"
            ),
            Path::new("systems.toml"),
        )
        .unwrap()
        .remove(0)
    }

    #[test]
    fn settings_parser_never_retains_credentials() {
        let values = parse_settings(
            "MOUNT_AT_BOOT=\"true\"\nLOCAL_DIR='cifs'\nUSERNAME=private\nPASSWORD=secret\n",
        );
        assert_eq!(
            values.get("MOUNT_AT_BOOT").map(String::as_str),
            Some("true")
        );
        assert_eq!(values.get("LOCAL_DIR").map(String::as_str), Some("cifs"));
        assert!(!values.contains_key("USERNAME"));
        assert!(!values.contains_key("PASSWORD"));
    }

    #[test]
    fn configured_timeout_is_bounded_even_for_extreme_values() {
        let mut values = BTreeMap::new();
        for key in [
            "BOOT_START_DELAY_SECONDS",
            "NETWORK_READY_TIMEOUT_SECONDS",
            "DEFAULT_ROUTE_READY_TIMEOUT_SECONDS",
            "DUAL_INTERFACE_SETTLE_SECONDS",
            "SERVER_WAIT_TIMEOUT_SECONDS",
        ] {
            values.insert(key.into(), u64::MAX.to_string());
        }
        assert_eq!(configured_timeout(&values), Duration::from_secs(300));
    }

    #[test]
    fn configured_boot_mount_is_detected_once_and_skipped_when_already_mounted() {
        let root = temp_dir("detect");
        std::fs::create_dir_all(root.join("Scripts")).unwrap();
        std::fs::write(
            root.join("Scripts/cifs_mount.sh"),
            "MOUNT_AT_BOOT=\"false\"\nLOCAL_DIR=\"cifs\"\nBASE_PATH=\"/media/fat\"\n#=========CODE STARTS HERE=========\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Scripts/cifs_mount.ini"),
            "MOUNT_AT_BOOT=\"true\"\n",
        )
        .unwrap();
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let roots = vec![PathBuf::from("/media/fat/cifs/games")];
        let table = vec![system("NES", "NES")];

        assert!(
            detect_with_mountinfo(&root, &roots, &table, mountinfo.clone())
                .unwrap()
                .is_some()
        );

        std::fs::write(
            &mountinfo,
            "31 22 0:30 / /media/fat/cifs rw - cifs //nas/MiSTer rw\n",
        )
        .unwrap();
        assert!(detect_with_mountinfo(&root, &roots, &table, mountinfo)
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn explicit_mount_is_relevant_only_to_a_configured_library_path() {
        let table = vec![system("NES", "NES")];
        let roots = vec![
            PathBuf::from("/media/fat/cifs/games"),
            PathBuf::from("/media/fat"),
        ];
        assert!(target_is_relevant(
            Path::new("/media/fat/cifs"),
            &roots,
            &table
        ));
        assert!(target_is_relevant(
            Path::new("/media/fat/NES"),
            &roots,
            &table
        ));
        assert!(!target_is_relevant(
            Path::new("/media/fat/Documents"),
            &roots,
            &table
        ));
    }

    #[test]
    fn readiness_uses_mount_table_not_directory_existence() {
        let plan = Plan {
            targets: Targets::Explicit(vec![PathBuf::from("/media/fat/cifs")]),
            base_path: PathBuf::from("/media/fat"),
            log_path: PathBuf::from("/tmp/log"),
            mountinfo_path: PathBuf::from("/proc/self/mountinfo"),
            timeout: Duration::from_secs(1),
        };
        assert!(!plan.ready("", "Done!"));
        assert!(plan.ready(
            "31 22 0:30 / /media/fat/cifs rw - cifs //nas/MiSTer rw\n",
            ""
        ));
    }

    #[test]
    fn mountinfo_decodes_spaces_and_dynamic_completion_uses_current_boot() {
        let mountinfo = "31 22 0:30 / /media/fat/Amiga\\040CD32 rw - cifs //nas/MiSTer rw\n";
        assert!(mounted_at(mountinfo, Path::new("/media/fat/Amiga CD32")));
        let plan = Plan {
            targets: Targets::Dynamic,
            base_path: PathBuf::from("/media/fat"),
            log_path: PathBuf::from("/tmp/log"),
            mountinfo_path: PathBuf::from("/proc/self/mountinfo"),
            timeout: Duration::from_secs(1),
        };
        assert!(!plan.ready(
            "",
            "==== old boot start ====\nDone!\n==== new boot start ====\nWaiting for network\n"
        ));
        assert!(plan.ready("", "==== boot start ====\nDone!\n"));
    }

    #[test]
    fn terminal_failure_reports_the_script_cause() {
        let log = "==== boot start ====\nWaiting for NAS\nNAS not reachable after 60 seconds.\n";
        assert_eq!(
            terminal_problem(log).as_deref(),
            Some("NAS not reachable after 60 seconds.")
        );
    }

    #[test]
    fn local_only_filter_removes_explicit_and_partial_dynamic_mounts() {
        let mut found = vec![FoundSystem {
            def: system("NES", "NES"),
            paths: vec![
                PathBuf::from("/media/fat/NES"),
                PathBuf::from("/media/usb0/games/NES"),
            ],
            logo_dir: None,
            menu_folder: None,
        }];
        let plan = Plan {
            targets: Targets::Dynamic,
            base_path: PathBuf::from("/media/fat"),
            log_path: PathBuf::from("/tmp/log"),
            mountinfo_path: PathBuf::from("/proc/self/mountinfo"),
            timeout: Duration::from_secs(1),
        };
        plan.remove_network_system_paths(
            &mut found,
            "31 22 0:30 / /media/fat/NES rw - cifs //nas/MiSTer rw\n",
            "==== boot start ====\nNES mounted\n",
        );
        assert_eq!(found[0].paths, vec![PathBuf::from("/media/usb0/games/NES")]);
    }

    #[test]
    fn local_only_discovery_uses_a_later_local_copy_instead_of_a_stale_mountpoint() {
        let root = temp_dir("local-fallback");
        let network = root.join("fat/cifs");
        let local = root.join("usb0/games");
        std::fs::create_dir_all(network.join("games/NES")).unwrap();
        std::fs::create_dir_all(local.join("NES")).unwrap();
        let mountinfo = root.join("mountinfo");
        let log = root.join("cifs.log");
        std::fs::write(&mountinfo, "").unwrap();
        let plan = Plan {
            targets: Targets::Explicit(vec![network.clone()]),
            base_path: root.join("fat"),
            log_path: log,
            mountinfo_path: mountinfo,
            timeout: Duration::ZERO,
        };

        let discovered = run_now(
            &plan,
            request(&root, vec![network.join("games"), local.clone()]),
            true,
        )
        .unwrap()
        .unwrap();

        assert_eq!(discovered.systems.len(), 1);
        assert_eq!(discovered.systems[0].paths, vec![local.join("NES")]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn wildcard_local_only_discovery_excludes_unmounted_card_targets_without_a_log() {
        let root = temp_dir("dynamic-local-fallback");
        let card = root.join("fat");
        let local = root.join("usb0/games");
        std::fs::create_dir_all(card.join("NES")).unwrap();
        std::fs::create_dir_all(local.join("NES")).unwrap();
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let plan = Plan {
            targets: Targets::Dynamic,
            base_path: card.clone(),
            log_path: root.join("cifs.log"),
            mountinfo_path: mountinfo,
            timeout: Duration::ZERO,
        };

        let discovered = run_now(&plan, request(&root, vec![card, local.clone()]), true)
            .unwrap()
            .unwrap();

        assert_eq!(discovered.systems[0].paths, vec![local.join("NES")]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancelled_wait_stops_without_discovery_and_timeout_keeps_the_real_failure() {
        let root = temp_dir("cancel-timeout");
        let mountinfo = root.join("mountinfo");
        std::fs::write(&mountinfo, "").unwrap();
        let plan = Plan {
            targets: Targets::Explicit(vec![root.join("fat/cifs")]),
            base_path: root.join("fat"),
            log_path: root.join("cifs.log"),
            mountinfo_path: mountinfo,
            timeout: Duration::ZERO,
        };
        let roots = vec![root.join("fat/cifs/games")];

        let cancelled = AtomicBool::new(true);
        assert!(run(
            &plan,
            request(&root, roots.clone()),
            false,
            &Mutex::new(Progress::waiting(false)),
            &cancelled,
        )
        .unwrap()
        .is_none());

        let error = match run_now(&plan, request(&root, roots), false) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("an unavailable zero-timeout mount must fail"),
        };
        assert!(error.contains("CIFS storage did not become ready"));
        std::fs::remove_dir_all(root).ok();
    }
}
