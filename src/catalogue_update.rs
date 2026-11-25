//! Manual, immutable, independently tagged system-catalogue revisions.
use crate::systems::SystemDef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
const RELEASES: &str =
    "https://api.github.com/repos/giancarloerra/Degauss/git/matching-refs/tags/catalogue-v1-";
const PREFIX: &str = "refs/tags/catalogue-v1-";
const RAW: &str = "https://raw.githubusercontent.com/giancarloerra/Degauss/";
pub const BUNDLED: u64 = 2026112501;
const LIMIT: u64 = 4 * 1024 * 1024;
// The install worker and completed foreground rebuilds update the same record.
static RECORD_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
type Result<T> = std::result::Result<T, String>;
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(Clone)]
pub struct Location {
    config: PathBuf,
    shipped: PathBuf,
    normal: PathBuf,
    installed: PathBuf,
    opened_revision: u64,
    pub pending: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub revision: u64,
    format: u32,
    min_app: String,
    bytes: u64,
    sha256: String,
    pub summary: String,
}
#[derive(Clone)]
pub struct Release {
    pub meta: Revision,
    sha: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    meta: Revision,
    body: String,
    rescan: Vec<String>,
}
pub enum Check {
    Current,
    Custom,
    Incompatible(String),
    Update(Release),
}
fn version(s: &str) -> Result<(u32, u32, u32)> {
    let mut v = s.split('.');
    let mut n = || {
        v.next()
            .and_then(|n| n.parse::<u32>().ok())
            .ok_or_else(|| "invalid minimum Degauss version".to_string())
    };
    let out = (n()?, n()?, n()?);
    if v.next().is_some() {
        return Err("invalid Degauss version".into());
    }
    Ok(out)
}
fn bounded(path: &Path, n: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("read catalogue: {e}"))?;
    let mut bytes = vec![];
    f.take(n + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read catalogue: {e}"))?;
    if bytes.len() as u64 > n {
        return Err("catalogue exceeds the download safety limit".into());
    }
    Ok(bytes)
}
impl Revision {
    fn compatible(&self) -> Result<bool> {
        Ok(self.format == 1 && version(&self.min_app)? <= version(env!("CARGO_PKG_VERSION"))?)
    }
    fn verify(&self, body: &[u8], path: &Path) -> Result<Vec<SystemDef>> {
        if !self.compatible()? {
            return Err("catalogue needs another Degauss version".into());
        }
        if self.bytes == 0 || self.bytes > LIMIT || body.len() as u64 != self.bytes {
            return Err("catalogue byte count mismatch".into());
        }
        if self.sha256 != digest(body) {
            return Err("catalogue SHA-256 mismatch".into());
        }
        let txt = std::str::from_utf8(body).map_err(|_| "catalogue is not UTF-8")?;
        let table = crate::systems::parse_table(txt, path)
            .map_err(|_| "invalid systems table or unsupported launch declaration")?;
        let mut ids = std::collections::HashSet::new();
        for system in &table {
            if system.id.is_empty()
                || system.name.trim().is_empty()
                || !ids.insert(system.id.to_lowercase())
            {
                return Err("invalid or duplicate system identifier".into());
            }
            for rule in system
                .launch
                .iter()
                .chain(system.compatible_cores.iter().flat_map(|p| &p.launch))
            {
                if !matches!(rule.kind.as_str(), "s" | "f") || rule.extensions.is_empty() {
                    return Err("unsupported media launch rule".into());
                }
            }
        }
        Ok(table)
    }
}
impl Location {
    pub fn new(shipped: &Path, config: &Path) -> Self {
        Self {
            config: config.to_owned(),
            shipped: shipped.to_owned(),
            normal: config.with_file_name("systems.toml"),
            installed: config.with_file_name("catalogue-installed.json"),
            opened_revision: BUNDLED,
            pending: Vec::new(),
        }
    }
    fn stock(&self) -> Result<bool> {
        if self.shipped != self.normal {
            return Ok(false);
        }
        let info = std::fs::symlink_metadata(&self.shipped)
            .map_err(|e| format!("checking stock catalogue: {e}"))?;
        if !info.is_file() || info.len() != include_bytes!("../assets/systems.toml").len() as u64 {
            return Ok(false);
        }
        let bytes = bounded(&self.shipped, LIMIT)?;
        Ok(bytes.as_slice() == include_bytes!("../assets/systems.toml"))
    }
    fn stored(&self) -> Result<Option<Stored>> {
        match self.installed.try_exists() {
            Ok(false) => return Ok(None),
            Err(e) => return Err(format!("checking stored catalogue: {e}")),
            Ok(true) => {}
        }
        let bytes = bounded(&self.installed, LIMIT * 6 + 4096)?;
        let stored: Stored =
            serde_json::from_slice(&bytes).map_err(|_| "invalid installed catalogue record")?;
        if stored.meta.revision <= BUNDLED || !stored.meta.compatible()? {
            return Ok(None);
        }
        Ok(Some(stored))
    }
    fn read_active(&self) -> crate::error::Result<(Vec<SystemDef>, u64, Vec<String>)> {
        // No network; the released startup path stays intact unless there is a local update.
        let result = (|| {
            let Some(stored) = self.stored()? else {
                return Ok(None);
            };
            if !self.stock()? {
                return Ok(None);
            }
            let table = stored
                .meta
                .verify(stored.body.as_bytes(), &self.installed)?;
            Ok::<_, String>(Some((table, stored.meta.revision, stored.rescan)))
        })();
        match result {
            Ok(Some(active)) => Ok(active),
            other => {
                if let Err(e) = other {
                    crate::note(&format!("catalogue: using shipped table: {e}"));
                }
                crate::systems::load_table(&self.shipped).map(|table| (table, BUNDLED, vec![]))
            }
        }
    }
    pub fn open(&mut self) -> crate::error::Result<Vec<SystemDef>> {
        let (table, revision, pending) = self.read_active()?;
        self.opened_revision = revision;
        self.pending = pending;
        Ok(table)
    }
    #[cfg(test)]
    fn load(&self) -> crate::error::Result<Vec<SystemDef>> {
        self.read_active().map(|active| active.0)
    }
    fn installed_revision(&self) -> u64 {
        match self.stored().and_then(|s| {
            s.map(|s| {
                s.meta
                    .verify(s.body.as_bytes(), &self.installed)
                    .map(|_| s.meta.revision)
            })
            .transpose()
        }) {
            Ok(n) => n.unwrap_or(BUNDLED),
            Err(e) => {
                crate::note(&format!("catalogue: ignoring damaged stored revision: {e}"));
                BUNDLED
            }
        }
    }
    #[cfg(test)]
    fn changed(&self) -> Result<Vec<String>> {
        let Some(s) = self.stored()? else {
            return Ok(vec![]);
        };
        if !self.stock()? {
            return Ok(vec![]);
        }
        s.meta.verify(s.body.as_bytes(), &self.installed)?;
        Ok(s.rescan)
    }
    pub fn rebuilt(&mut self, id: &str) -> Result<()> {
        if !self.pending.iter().any(|p| p == id) {
            return Ok(());
        }
        let _writing = RECORD_WRITE
            .lock()
            .map_err(|_| "catalogue record lock failed")?;
        if let Some(mut record) = self.stored()? {
            if record.meta.revision == self.opened_revision && self.stock()? {
                record
                    .meta
                    .verify(record.body.as_bytes(), &self.installed)?;
                record.rescan.retain(|p| p != id);
                self.save_record(&record)?;
            }
        }
        self.pending.retain(|p| p != id);
        Ok(())
    }
    fn save_record(&self, stored: &Stored) -> Result<()> {
        let bytes = serde_json::to_vec(stored).map_err(|_| "cannot encode catalogue record")?;
        crate::scraper::atomic_write(&self.installed, &bytes, "catalogue update")
            .map_err(|e| e.detail)
    }
    fn check(
        &self,
        stop: &Arc<AtomicBool>,
        get: &impl Fn(&str, u64) -> Result<Vec<u8>>,
    ) -> Result<Check> {
        if !self.stock()? {
            return Ok(Check::Custom);
        }
        let current = self.installed_revision();
        let data = get(RELEASES, 512 * 1024)?;
        #[derive(Deserialize)]
        struct Ref {
            #[serde(rename = "ref")]
            name: String,
            object: Object,
        }
        #[derive(Deserialize)]
        struct Object {
            #[serde(rename = "type")]
            kind: String,
            sha: String,
        }
        let refs: Vec<Ref> =
            serde_json::from_slice(&data).map_err(|_| "invalid GitHub catalogue tag list")?;
        let mut releases = vec![];
        for r in refs {
            let Some(suffix) = r.name.strip_prefix(PREFIX) else {
                continue;
            };
            let revision = suffix.parse::<u64>().map_err(|_| "invalid catalogue tag")?;
            if revision <= current {
                continue;
            }
            if r.object.kind != "commit"
                || r.object.sha.len() != 40
                || !r.object.sha.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("catalogue tag is not an immutable commit reference".into());
            }
            releases.push((revision, r.object.sha));
        }
        releases.sort_unstable_by_key(|release| std::cmp::Reverse(release.0));
        let mut unsupported = None;
        for (n, sha) in releases {
            cancelled(stop)?;
            let meta: Revision = serde_json::from_slice(&get(
                &format!("{RAW}{sha}/catalogue/revision.json"),
                16 * 1024,
            )?)
            .map_err(|_| "invalid catalogue revision metadata")?;
            if meta.revision != n
                || meta.bytes == 0
                || meta.bytes > LIMIT
                || meta.summary.trim().is_empty()
                || meta.summary.chars().count() > 160
                || meta.summary.chars().any(char::is_control)
                || meta.sha256.len() != 64
                || !meta.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("invalid published revision/size/checksum".into());
            }
            if meta.compatible()? {
                return Ok(Check::Update(Release { meta, sha }));
            }
            if unsupported.is_none() {
                unsupported = Some(meta.min_app);
            }
        }
        Ok(unsupported.map_or(Check::Current, Check::Incompatible))
    }
    fn install(
        &self,
        r: Release,
        stop: &Arc<AtomicBool>,
        get: &impl Fn(&str, u64) -> Result<Vec<u8>>,
    ) -> Result<Vec<String>> {
        if !self.stock()? || r.meta.revision <= self.installed_revision() {
            return Err("catalogue selection changed; reopen Check".into());
        }
        let bytes = get(&format!("{RAW}{}/assets/systems.toml", r.sha), r.meta.bytes)?;
        let table = r.meta.verify(&bytes, &self.installed)?;
        let (current, current_revision, mut pending) = self
            .read_active()
            .map_err(|e| format!("reading current catalogue: {e}"))?;
        let user = crate::config::Config::load_for_systems(&self.config, &current)
            .map_err(|e| format!("reading current settings: {e}"))?;
        user.validate_catalogue_overrides(&table, &self.config.with_file_name("degauss-user.toml"))
            .map_err(|e| format!("existing overrides would stop working: {e}"))?;
        let changed = changed(&current, &table)?;
        let body = String::from_utf8(bytes).map_err(|_| "invalid catalogue encoding")?;
        let _writing = RECORD_WRITE
            .lock()
            .map_err(|_| "catalogue record lock failed")?;
        cancelled(stop)?;
        if !self.stock()? || self.installed_revision() != current_revision {
            return Err("local catalogue changed; not installing".into());
        }
        if current_revision > BUNDLED {
            pending = self
                .stored()?
                .ok_or("previous catalogue disappeared")?
                .rescan;
        }
        pending.extend(changed.iter().cloned());
        pending.sort_unstable();
        pending.dedup();
        self.save_record(&Stored {
            meta: r.meta,
            body,
            rescan: pending,
        })?;
        Ok(changed)
    }
}
fn changed(old: &[SystemDef], new: &[SystemDef]) -> Result<Vec<String>> {
    let incoming: std::collections::BTreeMap<_, _> =
        new.iter().map(|s| (s.id.as_str(), s)).collect();
    for system in old {
        let next = incoming
            .get(system.id.as_str())
            .ok_or_else(|| format!("update removes saved system ID {}", system.id))?;
        if system
            .compatible_cores
            .iter()
            .any(|p| !next.compatible_cores.iter().any(|n| n.id == p.id))
        {
            return Err(format!(
                "update removes a saved core choice for {}",
                system.id
            ));
        }
    }
    let old: std::collections::BTreeMap<_, _> = old.iter().map(|s| (s.id.as_str(), s)).collect();
    Ok(incoming
        .into_iter()
        .filter(|(id, n)| {
            old.get(id).is_none_or(|o| {
                n.folders != o.folders
                    || n.extensions != o.extensions
                    || n.skip_folders != o.skip_folders
            })
        })
        .map(|(id, _)| id.to_owned())
        .collect())
}
fn cancelled(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Relaxed) {
        Err("cancelled".into())
    } else {
        Ok(())
    }
}
pub enum Outcome {
    Checked(Check),
    Installed(Vec<String>),
}
pub struct Job {
    pub rx: mpsc::Receiver<Result<Outcome>>,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn start(loc: Location, release: Option<Release>) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        std::thread::Builder::new()
            .name("catalogue-update".into())
            .spawn(move || {
                let client = crate::scraper::CurlTransport::new(signal.clone());
                let get = |url: &str, limit: u64| {
                    let res = client.catalogue_get(url, limit).map_err(|e| e.detail)?;
                    if res.status != 200 {
                        return Err(format!("GitHub HTTP {}", res.status));
                    }
                    Ok(res.body)
                };
                let result = match release {
                    Some(r) => loc.install(r, &signal, &get).map(Outcome::Installed),
                    None => loc.check(&signal, &get).map(Outcome::Checked),
                };
                let _ = tx.send(result);
            })
            .map_err(|e| format!("starting catalogue job: {e}"))?;
        Ok(Self { rx, cancel })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const STOCK: &[u8] = include_bytes!("../assets/systems.toml");
    #[test]
    fn publication_metadata_describes_the_exact_shipped_catalogue() {
        let r: Revision = serde_json::from_str(include_str!("../catalogue/revision.json")).unwrap();
        assert_eq!(r.revision, BUNDLED);
        r.verify(STOCK, Path::new("systems.toml")).unwrap();
        assert_eq!(
            STOCK,
            include_bytes!("../deploy/Scripts/.config/degauss/systems.toml").as_slice()
        );
    }
    fn setup(name: &str) -> (PathBuf, Location) {
        let dir = std::env::temp_dir().join(format!(
            "degauss-cat-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let p = dir.join("systems.toml");
        std::fs::write(&p, STOCK).unwrap();
        std::fs::write(dir.join("degauss.toml"), "[app]\n").unwrap();
        let l = Location::new(&p, &dir.join("degauss.toml"));
        (dir, l)
    }
    fn data() -> (Vec<u8>, Release) {
        let mut bytes = STOCK.to_vec();
        bytes.extend_from_slice(b"\n[[systems]]\nname='New system'\nid='NewSystem'\nrbf='_Computer/NewSystem'\nfolders=['NewSystem']\nextensions=['mgl']\n");
        let meta = Revision {
            revision: BUNDLED + 1,
            format: 1,
            min_app: env!("CARGO_PKG_VERSION").into(),
            bytes: bytes.len() as u64,
            sha256: digest(&bytes),
            summary: "New indexed library".into(),
        };
        (
            bytes,
            Release {
                meta,
                sha: "a".repeat(40),
            },
        )
    }
    #[test]
    fn check_is_not_install_and_an_offline_restart_uses_only_a_verified_new_revision() {
        let (d, loc) = setup("journey");
        let (bytes, r) = data();
        let stop = Arc::new(AtomicBool::new(false));
        let transport = |url: &str, _: u64| -> Result<Vec<u8>> {
            if url == RELEASES {
                return Ok(serde_json::to_vec(&serde_json::json!([{"ref":format!("{PREFIX}{}",r.meta.revision),"object":{"type":"commit","sha":r.sha}}])).unwrap());
            }
            if url == format!("{RAW}{}/catalogue/revision.json", r.sha) {
                return Ok(serde_json::to_vec(&r.meta).unwrap());
            }
            if url == format!("{RAW}{}/assets/systems.toml", r.sha) {
                return Ok(bytes.clone());
            }
            panic!("Unexpected request {url}")
        };
        let Check::Update(found) = loc.check(&stop, &transport).unwrap() else {
            panic!("update unavailable")
        };
        assert!(!loc.installed.exists());
        assert!(!loc.load().unwrap().iter().any(|s| s.id == "NewSystem"));
        assert_eq!(
            loc.install(found, &stop, &transport).unwrap(),
            ["NewSystem"]
        );
        assert_eq!(std::fs::read(&loc.shipped).unwrap(), STOCK);
        assert!(loc.load().unwrap().iter().any(|s| s.id == "NewSystem"));
        assert!(matches!(loc.check(&stop, &transport), Ok(Check::Current)));
        // A user edit after installation remains authoritative on the next open.
        std::fs::write(&loc.shipped, [STOCK, b"\n# custom preference\n"].concat()).unwrap();
        assert!(!loc.load().unwrap().iter().any(|s| s.id == "NewSystem"));
        assert!(matches!(
            loc.check(&stop, &|_, _| panic!(
                "custom file must not contact network"
            )),
            Ok(Check::Custom)
        ));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn a_catalogue_revision_preserves_ids_overrides_and_only_offers_rescans_for_index_changes() {
        let (d, mut loc) = setup("overrides");
        let (bytes, mut release) = data();
        let stop = Arc::new(AtomicBool::new(false));
        std::fs::write(
            d.join("degauss-user.toml"),
            "[library_folders]\nNES=['My NES']\n",
        )
        .unwrap();
        loc.install(release.clone(), &stop, &|_, _| Ok(bytes.clone()))
            .unwrap();
        assert_eq!(loc.changed().unwrap(), ["NewSystem"]);
        let before = std::fs::read(&loc.installed).unwrap();
        let mut broken = release.clone();
        broken.meta.revision += 1;
        let truncated = bytes[1..].to_vec();
        assert!(loc
            .install(broken.clone(), &stop, &|_, _| Ok(truncated.clone()))
            .is_err());
        assert!(loc
            .install(broken, &stop, &|_, _| {
                stop.store(true, Ordering::Relaxed);
                Ok(bytes.clone())
            })
            .is_err());
        stop.store(false, Ordering::Relaxed);
        assert_eq!(
            std::fs::read(&loc.installed).unwrap(),
            before,
            "a failed or cancelled subsequent update must preserve the previous valid revision"
        );
        let amended = String::from_utf8(bytes.clone())
            .unwrap()
            .replace("name='New system'", "name='Renamed system'");
        release.meta.revision += 1;
        let make = |mut r: Release, s: &str| {
            r.meta.bytes = s.len() as u64;
            r.meta.sha256 = digest(s.as_bytes());
            r
        };
        let losing = amended.replace("id = \"NES\"", "id = \"DifferentNES\"");
        assert_ne!(losing, amended);
        assert!(loc
            .install(make(release.clone(), &losing), &stop, &|_, _| Ok(losing
                .as_bytes()
                .to_vec()))
            .is_err());
        assert_eq!(std::fs::read(&loc.installed).unwrap(), before);
        assert!(
            loc.install(make(release, &amended), &stop, &|_, _| Ok(amended
                .as_bytes()
                .to_vec()))
                .unwrap()
                .is_empty(),
            "a label change does not need a library rebuild"
        );
        assert_eq!(
            loc.changed().unwrap(),
            ["NewSystem"],
            "a label-only revision must preserve an outstanding rescan"
        );
        loc.rebuilt("NewSystem").unwrap();
        assert_eq!(
            loc.changed().unwrap(),
            ["NewSystem"],
            "the still-running old table cannot clear a new revision's reminder"
        );
        loc.open().unwrap();
        assert_eq!(loc.pending, ["NewSystem"]);
        loc.rebuilt("NewSystem").unwrap();
        assert!(loc.changed().unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(d.join("degauss-user.toml")).unwrap(),
            "[library_folders]\nNES=['My NES']\n"
        );
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn corrupt_partial_incompatible_or_cancelled_transfers_do_not_replace_working_data() {
        let (d, l) = setup("failure");
        let (bytes, r) = data();
        let stop = Arc::new(AtomicBool::new(false));
        for damaged in [vec![], bytes[1..].to_vec(), vec![b'x'; bytes.len()]] {
            assert!(l
                .install(r.clone(), &stop, &|_, _| Ok(damaged.clone()))
                .is_err());
            assert!(!l.installed.exists());
        }
        assert!(l.check(&stop, &|_, _| Err("DNS failure".into())).is_err());
        let mut future = r.clone();
        future.meta.min_app = "99.0.0".into();
        assert!(future.meta.verify(&bytes, &l.shipped).is_err());
        stop.store(true, Ordering::Relaxed);
        assert!(l
            .install(r.clone(), &stop, &|_, _| Ok(bytes.clone()))
            .is_err());
        assert!(!l.installed.exists());
        stop.store(false, Ordering::Relaxed);
        std::fs::write(&l.installed, b"{").unwrap();
        assert_eq!(
            l.load().unwrap().len(),
            crate::systems::load_table(&l.shipped).unwrap().len()
        );
        let alternate = Location::new(&d.join("alternate.toml"), &d.join("degauss.toml"));
        std::fs::write(&alternate.shipped, STOCK).unwrap();
        assert!(!alternate.stock().unwrap());
        std::fs::remove_file(&l.installed).unwrap();
        std::fs::create_dir(&l.installed).unwrap();
        assert!(l.install(r, &stop, &|_, _| Ok(bytes.clone())).is_err());
        assert!(l.installed.is_dir() && std::fs::read(&l.shipped).unwrap() == STOCK);
        std::fs::remove_dir_all(d).unwrap();
    }
}
