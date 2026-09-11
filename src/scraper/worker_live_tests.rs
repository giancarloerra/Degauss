//! Opt-in checks against the real ScreenScraper service.
//!
//! These tests are ignored by default. Credentials and the ROM fixture are
//! supplied at execution time and are never printed, committed, or copied into
//! the release package. Every writable scenario runs in a disposable local
//! directory; the source ROM and the user's MiSTer card remain read-only.

use super::*;
use crate::browse::{DisplayNames, Launch, Place};
use crate::scraper::{ImagePolicy, MetadataPolicy};
use crate::systems::FoundSystem;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USERNAME_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_USERNAME";
const PASSWORD_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_PASSWORD";
const ROM_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_ROM";
const TITLE_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_TITLE";
const SYSTEM_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_SYSTEM";
const PLATFORM_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_PLATFORM";
const EXTRA_ROMS_ENV: &str = "DEGAUSS_SCREENSCRAPER_LIVE_EXTRA_ROMS";

struct LiveConfig {
    username: String,
    password: String,
    rom: PathBuf,
    title: String,
    system_id: String,
    platform_id: u32,
    extension: String,
    developer: DeveloperCredentials,
}

impl LiveConfig {
    fn from_environment() -> Self {
        let username = required(USERNAME_ENV);
        let password = required(PASSWORD_ENV);
        let rom = PathBuf::from(required(ROM_ENV));
        assert!(rom.is_file(), "{ROM_ENV} must name one readable ROM file");
        let title = required(TITLE_ENV);
        let system_id = required(SYSTEM_ENV);
        let platform_id = required(PLATFORM_ENV)
            .parse::<u32>()
            .expect("DEGAUSS_SCREENSCRAPER_LIVE_PLATFORM must be a positive number");
        assert!(platform_id > 0, "{PLATFORM_ENV} must be positive");
        let extension = rom
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .expect("DEGAUSS_SCREENSCRAPER_LIVE_ROM must have a file extension")
            .to_ascii_lowercase();
        let developer = DeveloperCredentials::embedded()
            .expect("compile the live test with the Degauss ScreenScraper credentials");
        Self {
            username,
            password,
            rom,
            title,
            system_id,
            platform_id,
            extension,
            developer,
        }
    }

    fn settings(
        &self,
        image_policy: ImagePolicy,
        metadata_policy: MetadataPolicy,
    ) -> ScraperSettings {
        ScraperSettings {
            username: self.username.clone(),
            password: self.password.clone(),
            accepted_plaintext_warning: true,
            image_policy,
            metadata_policy,
            system_ids: BTreeMap::from([(self.system_id.clone(), self.platform_id)]),
            ..Default::default()
        }
    }

    fn system(&self, root: &Path) -> FoundSystem {
        let definition = format!(
            "name = 'Live System'\nid = '{}'\nfolders = ['Live']\nrbf = '_Console/Live'\nextensions = ['{}']\n",
            self.system_id, self.extension
        );
        FoundSystem {
            def: toml::from_str(&definition).expect("the live system fixture must be valid"),
            paths: vec![root.to_path_buf()],
            logo_dir: None,
            menu_folder: Some("Console".into()),
        }
    }
}

struct Suite {
    root: PathBuf,
}

impl Suite {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "degauss-screenscraper-live-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create isolated live-test directory");
        Self { root }
    }

    fn case(&self, config: &LiveConfig, name: &str) -> PathBuf {
        let root = self.root.join(name);
        std::fs::create_dir(&root).expect("create isolated live-test case");
        std::fs::copy(
            &config.rom,
            root.join(format!("fixture.{}", config.extension)),
        )
        .expect("copy the read-only ROM fixture");
        root
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum ScopeKind {
    All,
    System,
    Folder,
    Game,
}

#[test]
#[ignore = "requires explicit credentials and three private local ROM fixtures"]
fn real_service_small_system_batch_is_non_destructive() {
    let config = LiveConfig::from_environment();
    let mut sources = vec![config.rom.clone()];
    sources.extend(
        required(EXTRA_ROMS_ENV)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from),
    );
    assert!(
        sources.len() >= 3,
        "{EXTRA_ROMS_ENV} must provide at least two newline-separated ROM paths"
    );
    for source in &sources {
        assert!(
            source.is_file(),
            "every {EXTRA_ROMS_ENV} path must be a file"
        );
    }

    let suite = Suite::new();
    let root = suite.root.join("small-system");
    std::fs::create_dir(&root).expect("create isolated small-system fixture");
    for (index, source) in sources.iter().enumerate() {
        std::fs::copy(
            source,
            root.join(format!("real-game-{index}.{}", config.extension)),
        )
        .expect("copy a read-only ROM into the isolated small system");
    }

    let request = Request {
        scope: Scope::System {
            system_id: config.system_id.clone(),
            place: Place::Dir(root.clone()),
            display_name: "Live Small System".into(),
        },
        scope_label: "Live Small System".into(),
        systems: vec![config.system(&root)],
        names: DisplayNames::default(),
        settings: config.settings(ImagePolicy::Off, MetadataPolicy::FillMissing),
        developer: Some(config.developer.clone()),
        artwork_pack_system_ids: HashSet::new(),
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(CurlTransport::new(Arc::clone(&cancelled)));
    let progress = finish_live(
        start_with_transport_and_cancel(request, transport, cancelled)
            .expect("start the small-system live ScreenScraper worker"),
    );
    assert_eq!(progress.completed, sources.len());
    assert_eq!(progress.updated, sources.len());
    assert_eq!(progress.failed, 0);
    assert_eq!(progress.not_found, 0);
    assert_eq!(progress.ambiguous, 0);
    assert!(progress.workers > 0);
    assert!(progress.account.is_some());
    assert!(progress.requests_started >= sources.len() as u64);
    let xml = read_gamelist(&root);
    for index in 0..sources.len() {
        assert!(xml.contains(&format!(
            "<path>./real-game-{index}.{}</path>",
            config.extension
        )));
    }
    assert_eq!(backup_count(&root), 0);
    assert!(!root.join("media").exists());
}

#[test]
#[ignore = "requires explicit credentials and one private local ROM fixture"]
fn real_service_policy_and_scope_matrix_is_non_destructive() {
    let config = LiveConfig::from_environment();
    let suite = Suite::new();

    let fresh = suite.case(&config, "fresh-game");
    let fresh_progress = run_case(
        &config,
        &fresh,
        ScopeKind::Game,
        ImagePolicy::MissingOnly,
        MetadataPolicy::FillMissing,
    );
    assert_success(&fresh_progress, "fresh game");
    let fresh_xml = read_gamelist(&fresh);
    assert!(fresh_xml.contains("<game"));
    assert!(fresh_xml.contains("<image>./media/screenscraper/"));
    assert!(fresh_xml.contains("<name>"));
    let downloaded = only_downloaded_image(&fresh);

    let partial = suite.case(&config, "partial-folder");
    std::fs::copy(&downloaded, partial.join("existing.png"))
        .expect("copy a real downloaded image into the partial fixture");
    write_gamelist(
        &partial,
        "<game id=\"legacy-id\"><path>./fixture.EXT</path><name>KEEP PARTIAL NAME</name><desc></desc><image>./existing.png</image><thumbnail>./legacy-thumb.png</thumbnail><video>./legacy-video.mp4</video><marquee>./legacy-marquee.png</marquee><rating>0.8</rating><favorite>true</favorite><comments>keep comments</comments><private-field>keep</private-field></game>",
        &config.extension,
    );
    // One chosen game fills its empty fields one by one; a folder run
    // would treat the stored name as complete and skip this entry.
    let partial_progress = run_case(
        &config,
        &partial,
        ScopeKind::Game,
        ImagePolicy::Off,
        MetadataPolicy::FillMissing,
    );
    assert_success(&partial_progress, "partial game");
    let partial_xml = read_gamelist(&partial);
    assert!(partial_xml.contains("<name>KEEP PARTIAL NAME</name>"));
    assert!(partial_xml.contains("<private-field>keep</private-field>"));
    assert!(partial_xml.contains("<image>./existing.png</image>"));
    assert!(partial_xml.contains("<thumbnail>./legacy-thumb.png</thumbnail>"));
    assert!(partial_xml.contains("<video>./legacy-video.mp4</video>"));
    assert!(partial_xml.contains("<marquee>./legacy-marquee.png</marquee>"));
    assert!(partial_xml.contains("<rating>0.8</rating>"));
    assert!(partial_xml.contains("<favorite>true</favorite>"));
    assert!(partial_xml.contains("<comments>keep comments</comments>"));
    assert!(partial_xml.contains("id=\"legacy-id\""));
    assert!(!partial_xml.contains("<desc></desc>"));
    assert_eq!(
        std::fs::read(partial.join("existing.png")).unwrap(),
        std::fs::read(&downloaded).unwrap()
    );
    assert_eq!(backup_count(&partial), 1);

    // The same partially filled entry is complete for a folder run: an
    // image plus one stored field makes no request and changes nothing.
    let skipped = suite.case(&config, "partial-folder");
    std::fs::copy(&downloaded, skipped.join("existing.png"))
        .expect("copy a real downloaded image into the skipped fixture");
    write_gamelist(
        &skipped,
        "<game><path>./fixture.EXT</path><name>KEEP PARTIAL NAME</name><image>./existing.png</image></game>",
        &config.extension,
    );
    let skipped_xml = read_gamelist(&skipped);
    let skipped_progress = run_case(
        &config,
        &skipped,
        ScopeKind::Folder,
        ImagePolicy::MissingOnly,
        MetadataPolicy::FillMissing,
    );
    assert_eq!(
        skipped_progress.failed, 0,
        "partial folder recorded a failed target"
    );
    assert_eq!(skipped_progress.completed, 1);
    assert_eq!(skipped_progress.unchanged, 1);
    assert_eq!(skipped_progress.updated, 0);
    assert_eq!(
        skipped_progress.requests_started, 0,
        "partial folder must not contact ScreenScraper"
    );
    assert_eq!(read_gamelist(&skipped), skipped_xml);
    assert_eq!(backup_count(&skipped), 0);

    let complete = suite.case(&config, "complete-game");
    std::fs::copy(&downloaded, complete.join("existing.png"))
        .expect("copy a real downloaded image into the complete fixture");
    write_gamelist(
        &complete,
        "<game><path>./fixture.EXT</path><name>REPLACE COMPLETE NAME</name><desc>complete</desc><publisher>complete</publisher><developer>complete</developer><releasedate>19990101T000000</releasedate><players>9</players><genre>complete</genre><lang>zz</lang><image>./existing.png</image></game>",
        &config.extension,
    );
    let complete_progress = run_case(
        &config,
        &complete,
        ScopeKind::Game,
        ImagePolicy::MissingOnly,
        MetadataPolicy::ReplaceExisting,
    );
    assert_success(&complete_progress, "complete game");
    let complete_xml = read_gamelist(&complete);
    assert!(!complete_xml.contains("<name>REPLACE COMPLETE NAME</name>"));
    assert!(complete_xml.contains("<image>./existing.png</image>"));
    for field in [
        "name",
        "desc",
        "publisher",
        "developer",
        "releasedate",
        "players",
        "genre",
        "lang",
    ] {
        if let Some(expected) = field_value(&fresh_xml, field) {
            assert_eq!(
                field_value(&complete_xml, field),
                Some(expected),
                "Replace existing did not apply the live {field} value"
            );
        }
    }
    assert_eq!(backup_count(&complete), 1);

    let replace_image = suite.case(&config, "replace-system-image");
    std::fs::copy(&downloaded, replace_image.join("existing.png"))
        .expect("copy a real downloaded image into the replacement fixture");
    write_gamelist(
        &replace_image,
        "<game><path>./fixture.EXT</path><name>KEEP COMPLETE NAME</name><desc>complete</desc><publisher>complete</publisher><developer>complete</developer><releasedate>19990101T000000</releasedate><players>9</players><genre>complete</genre><lang>zz</lang><image>./existing.png</image></game>",
        &config.extension,
    );
    let original_image = std::fs::read(replace_image.join("existing.png")).unwrap();
    let image_progress = run_case(
        &config,
        &replace_image,
        ScopeKind::System,
        ImagePolicy::ReplaceExisting,
        MetadataPolicy::Off,
    );
    assert_success(&image_progress, "system image replacement");
    let image_xml = read_gamelist(&replace_image);
    assert!(image_xml.contains("<name>KEEP COMPLETE NAME</name>"));
    assert!(image_xml.contains("<image>./media/screenscraper/"));
    assert_eq!(
        std::fs::read(replace_image.join("existing.png")).unwrap(),
        original_image
    );
    assert_eq!(backup_count(&replace_image), 1);

    let broken_image = suite.case(&config, "broken-all-image");
    write_gamelist(
        &broken_image,
        "<game><path>./fixture.EXT</path><name>KEEP BROKEN NAME</name><image>./missing.png</image></game>",
        &config.extension,
    );
    let broken_progress = run_case(
        &config,
        &broken_image,
        ScopeKind::All,
        ImagePolicy::MissingOnly,
        MetadataPolicy::Off,
    );
    assert_success(&broken_progress, "all-systems broken image repair");
    let broken_xml = read_gamelist(&broken_image);
    assert!(broken_xml.contains("<name>KEEP BROKEN NAME</name>"));
    assert!(broken_xml.contains("<image>./media/screenscraper/"));
    assert_eq!(backup_count(&broken_image), 1);

    // Once every requested field is complete, Missing only / Fill missing
    // must finish locally without spending even an account-preflight request.
    let complete_noop = suite.case(&config, "complete-noop");
    std::fs::copy(&downloaded, complete_noop.join("existing.png"))
        .expect("copy a real downloaded image into the no-op fixture");
    write_gamelist(
        &complete_noop,
        "<game><path>./fixture.EXT</path><name>complete</name><desc>complete</desc><publisher>complete</publisher><developer>complete</developer><releasedate>19990101T000000</releasedate><players>9</players><genre>complete</genre><lang>zz</lang><image>./existing.png</image><private-field>keep</private-field></game>",
        &config.extension,
    );
    let noop_before = read_gamelist(&complete_noop);
    let noop_image_before = std::fs::read(complete_noop.join("existing.png")).unwrap();
    let noop_progress = run_case(
        &config,
        &complete_noop,
        ScopeKind::Game,
        ImagePolicy::MissingOnly,
        MetadataPolicy::FillMissing,
    );
    assert_eq!(noop_progress.completed, 1);
    assert_eq!(noop_progress.unchanged, 1);
    assert_eq!(noop_progress.updated, 0);
    assert_eq!(noop_progress.requests_started, 0);
    assert!(noop_progress.account.is_none());
    assert_eq!(read_gamelist(&complete_noop), noop_before);
    assert_eq!(
        std::fs::read(complete_noop.join("existing.png")).unwrap(),
        noop_image_before
    );
    assert_eq!(backup_count(&complete_noop), 0);

    // Exercise a real server miss as an ordinary non-destructive result. The
    // deliberately synthetic bytes and title cannot identify a user's game.
    let no_match = suite.case(&config, "server-miss");
    std::fs::write(
        no_match.join(format!("fixture.{}", config.extension)),
        b"Degauss live ScreenScraper no-match fixture",
    )
    .expect("replace the isolated ROM copy with a no-match fixture");
    let no_match_progress = run_case_with_title(
        &config,
        &no_match,
        ScopeKind::Game,
        ImagePolicy::Off,
        MetadataPolicy::FillMissing,
        "Degauss Definitely Missing Live Test Entry 73F908",
    );
    assert_eq!(no_match_progress.completed, 1);
    assert_eq!(no_match_progress.not_found, 1);
    assert_eq!(no_match_progress.updated, 0);
    assert_eq!(no_match_progress.failed, 0);
    assert!(no_match_progress.requests_started >= 1);
    assert!(!no_match.join("gamelist.xml").exists());
    assert!(!no_match.join("media").exists());
}

#[test]
#[ignore = "requires explicit credentials and a real ScreenScraper account"]
fn real_service_distinguishes_an_invalid_user_login_without_exposing_it() {
    let config = LiveConfig::from_environment();
    let mut settings = config.settings(ImagePolicy::Off, MetadataPolicy::FillMissing);
    settings.password = "DegaussDeliberatelyInvalidPassword73F908".to_string();
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(CurlTransport::new(cancelled));
    let client = Client::new(transport, config.developer.clone(), &settings)
        .expect("construct the production ScreenScraper client");
    let error = client
        .account()
        .expect_err("ScreenScraper must reject the deliberately invalid user password");
    assert_eq!(error.kind, ErrorKind::Authentication);
    assert!(error.detail.contains("username and password"));
    for secret in [
        config.username.as_str(),
        config.password.as_str(),
        config.developer.developer_id.as_str(),
        config.developer.developer_password.as_str(),
    ] {
        assert!(!secret.is_empty());
        assert!(!error.detail.contains(secret));
    }
}

#[test]
#[ignore = "requires explicit credentials and a private NES ROM fixture"]
fn real_service_manual_match_preview_edit_and_batch_skip_are_non_destructive() {
    let config = LiveConfig::from_environment();
    assert_eq!(
        config.platform_id, 3,
        "the live manual-match query is a reviewed NES fixture"
    );
    let suite = Suite::new();
    let settings = config.settings(ImagePolicy::MissingOnly, MetadataPolicy::FillMissing);

    // A guaranteed miss exercises the same state from which the on-device
    // keyboard edits the term. It is a completed search, not a transport or
    // authentication failure.
    let missing = finish_live_search(
        start_search(SearchRequest {
            system_id: config.platform_id,
            term: "Degauss Definitely Missing Manual Search 73F908".into(),
            settings: settings.clone(),
            developer: config.developer.clone(),
        })
        .expect("start the real no-result manual search"),
    );
    let SearchEvent::Finished {
        matches: missing,
        failed_searches,
        ..
    } = missing
    else {
        panic!("the real no-result query did not complete normally");
    };
    assert!(missing.is_empty());
    assert_eq!(failed_searches, 1);

    // This edited broader NES term is known to return multiple real games.
    // Keep the service's relevance order and obtain preview media from the
    // same candidate record later applied to a disposable fixture.
    let search = finish_live_search(
        start_search(SearchRequest {
            system_id: config.platform_id,
            term: "Adventure".into(),
            settings: settings.clone(),
            developer: config.developer.clone(),
        })
        .expect("start the real broad manual search"),
    );
    let SearchEvent::Finished {
        matches,
        account,
        requests_started,
        ..
    } = search
    else {
        panic!("the edited real query did not complete");
    };
    assert!(matches.len() > 1, "the real query must exercise a chooser");
    assert!(
        requests_started >= 2,
        "preflight and search must be counted"
    );
    let selected = matches
        .iter()
        .find(|candidate| candidate.media.is_some())
        .cloned()
        .expect("at least one real candidate must provide screenshot media");
    let max_download_speed = account
        .max_download_speed
        .expect("the real account must report its media speed");
    let media = selected.media.clone().expect("selected preview media");
    let preview = finish_live_preview(
        start_preview(PreviewRequest {
            match_id: selected.id.clone(),
            media,
            settings: settings.clone(),
            developer: config.developer.clone(),
            max_download_speed,
            max_edge: 320,
            ground: [0, 0, 0],
        })
        .expect("start the real candidate preview"),
    );
    let PreviewEvent::Finished { match_id, image } = preview else {
        panic!("the real candidate preview did not download and decode");
    };
    assert_eq!(match_id, selected.id);
    assert!(image.width > 0 && image.height > 0);
    assert!(image.width <= 320 && image.height <= 320);

    let selected_root = suite.case(&config, "manual-selected");
    let selected_request = Request {
        scope: Scope::Game {
            system_id: config.system_id.clone(),
            launch: Launch::File(selected_root.join(format!("fixture.{}", config.extension))),
            title: "Manual Selection Fixture".into(),
        },
        scope_label: "Manual Selection Fixture".into(),
        systems: vec![config.system(&selected_root)],
        names: DisplayNames::default(),
        settings: settings.clone(),
        developer: Some(config.developer.clone()),
        artwork_pack_system_ids: HashSet::new(),
    };
    let selected_progress = finish_live(
        start_selected(selected_request, selected.clone())
            .expect("start applying the real selected match"),
    );
    assert_eq!(selected_progress.updated, 1);
    assert_eq!(selected_progress.not_found, 0);
    assert_eq!(selected_progress.ambiguous, 0);
    assert!(selected_progress.manual_matches.is_empty());
    let selected_xml = read_gamelist(&selected_root);
    assert_eq!(
        field_value(&selected_xml, "name"),
        Some(selected.name.as_str())
    );
    assert!(selected_xml.contains("<image>./media/screenscraper/"));

    // Multi-game scopes never retain candidates or stop for user input. A
    // real ROM must still be written after sharing the batch with a
    // deliberately synthetic miss, all inside the disposable directory.
    let batch_root = suite.root.join("batch-with-miss");
    std::fs::create_dir(&batch_root).expect("create disposable batch directory");
    std::fs::copy(
        &config.rom,
        batch_root.join(format!("real-game.{}", config.extension)),
    )
    .expect("copy the real ROM into the disposable batch");
    std::fs::write(
        batch_root.join(format!("Degauss Missing 73F908.{}", config.extension)),
        b"Degauss synthetic unresolved batch fixture 73F908",
    )
    .expect("write the disposable no-match fixture");
    let batch_request = Request {
        scope: Scope::System {
            system_id: config.system_id.clone(),
            place: Place::Dir(batch_root.clone()),
            display_name: "Live Mixed Batch".into(),
        },
        scope_label: "Live Mixed Batch".into(),
        systems: vec![config.system(&batch_root)],
        names: DisplayNames::default(),
        settings: config.settings(ImagePolicy::Off, MetadataPolicy::FillMissing),
        developer: Some(config.developer.clone()),
        artwork_pack_system_ids: HashSet::new(),
    };
    let batch_progress = finish_live(start(batch_request).expect("start the real mixed batch"));
    assert_eq!(batch_progress.total, 2);
    assert_eq!(batch_progress.completed, 2);
    assert_eq!(batch_progress.updated, 1);
    assert_eq!(batch_progress.not_found, 1);
    assert_eq!(batch_progress.failed, 0);
    assert!(batch_progress.manual_matches.is_empty());
    let batch_xml = read_gamelist(&batch_root);
    assert!(batch_xml.contains(&format!("<path>./real-game.{}</path>", config.extension)));
    assert!(!batch_xml.contains("Degauss Missing 73F908"));
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name} to run this ignored live test"))
}

fn run_case(
    config: &LiveConfig,
    root: &Path,
    scope_kind: ScopeKind,
    image_policy: ImagePolicy,
    metadata_policy: MetadataPolicy,
) -> Progress {
    run_case_with_title(
        config,
        root,
        scope_kind,
        image_policy,
        metadata_policy,
        &config.title,
    )
}

fn run_case_with_title(
    config: &LiveConfig,
    root: &Path,
    scope_kind: ScopeKind,
    image_policy: ImagePolicy,
    metadata_policy: MetadataPolicy,
    title: &str,
) -> Progress {
    let rom = root.join(format!("fixture.{}", config.extension));
    let scope = match scope_kind {
        ScopeKind::All => Scope::All,
        ScopeKind::System => Scope::System {
            system_id: config.system_id.clone(),
            place: Place::Dir(root.to_path_buf()),
            display_name: "Live System".into(),
        },
        ScopeKind::Folder => Scope::Folder {
            system_id: config.system_id.clone(),
            place: Place::Dir(root.to_path_buf()),
            display_name: "Live Folder".into(),
        },
        ScopeKind::Game => Scope::Game {
            system_id: config.system_id.clone(),
            launch: Launch::File(rom),
            title: title.to_string(),
        },
    };
    let scope_label = scope.label().to_string();
    let request = Request {
        scope,
        scope_label,
        systems: vec![config.system(root)],
        names: DisplayNames::default(),
        settings: config.settings(image_policy, metadata_policy),
        developer: Some(config.developer.clone()),
        artwork_pack_system_ids: HashSet::new(),
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(CurlTransport::new(Arc::clone(&cancelled)));
    let job = start_with_transport_and_cancel(request, transport, cancelled)
        .expect("start the live ScreenScraper worker");
    finish_live(job)
}

fn finish_live(job: Job) -> Progress {
    loop {
        match job
            .events
            .recv_timeout(Duration::from_secs(360))
            .expect("the live ScreenScraper worker stopped reporting progress")
        {
            Event::Finished(progress) => return progress,
            Event::Failed { error, progress } => panic!(
                "live ScreenScraper run failed ({:?}): {}; completed={}, failed={}, not_found={}, ambiguous={}",
                error.kind,
                error.detail,
                progress.completed,
                progress.failed,
                progress.not_found,
                progress.ambiguous
            ),
            Event::Cancelled(_) => panic!("live ScreenScraper run was cancelled"),
            Event::Progress(_) => {}
        }
    }
}

fn finish_live_search(job: SearchJob) -> SearchEvent {
    loop {
        match job
            .events
            .recv_timeout(Duration::from_secs(360))
            .expect("the live manual search stopped reporting progress")
        {
            event @ SearchEvent::Finished { .. } => return event,
            SearchEvent::Failed(error) => panic!(
                "live manual search failed ({:?}): {}",
                error.kind, error.detail
            ),
            SearchEvent::Cancelled => panic!("live manual search was cancelled"),
            SearchEvent::Activity(_) => {}
        }
    }
}

fn finish_live_preview(job: PreviewJob) -> PreviewEvent {
    job.events
        .recv_timeout(Duration::from_secs(360))
        .expect("the live preview stopped reporting progress")
}

fn assert_success(progress: &Progress, case: &str) {
    assert_eq!(progress.failed, 0, "{case} recorded a failed target");
    assert_eq!(progress.not_found, 0, "{case} did not match the real ROM");
    assert_eq!(progress.ambiguous, 0, "{case} matched more than one game");
    assert_eq!(
        progress.completed, 1,
        "{case} did not process exactly one game"
    );
    assert_eq!(
        progress.updated, 1,
        "{case} did not update exactly one game"
    );
    let account = progress
        .account
        .as_ref()
        .expect("the authenticated account limits must be present");
    assert!(account.level.is_some_and(|level| level > 0));
    assert!(account.max_threads.is_some_and(|workers| workers > 0));
    assert!(account
        .max_requests_per_minute
        .is_some_and(|limit| limit > 0));
    assert!(progress.requests_started > 0);
}

fn write_gamelist(root: &Path, game: &str, extension: &str) {
    let game = game.replace("EXT", extension);
    let document = format!("<?xml version=\"1.0\"?>\n<gameList>{game}</gameList>\n");
    std::fs::write(root.join("gamelist.xml"), document).expect("write isolated gamelist fixture");
}

fn read_gamelist(root: &Path) -> String {
    std::fs::read_to_string(root.join("gamelist.xml")).expect("read resulting gamelist")
}

fn field_value<'a>(document: &'a str, field: &str) -> Option<&'a str> {
    let start = format!("<{field}>");
    let end = format!("</{field}>");
    let content = document.split_once(&start)?.1;
    content.split_once(&end).map(|(value, _)| value)
}

fn only_downloaded_image(root: &Path) -> PathBuf {
    let directory = root.join("media/screenscraper");
    let files: Vec<_> = std::fs::read_dir(&directory)
        .expect("fresh scrape created the ScreenScraper media directory")
        .map(|entry| entry.expect("read downloaded image entry").path())
        .filter(|path| path.is_file())
        .collect();
    assert_eq!(files.len(), 1, "fresh scrape must create one image");
    files.into_iter().next().unwrap()
}

fn backup_count(root: &Path) -> usize {
    std::fs::read_dir(root)
        .expect("read isolated case directory")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("gamelist.xml.degauss-scraper-")
        })
        .count()
}
