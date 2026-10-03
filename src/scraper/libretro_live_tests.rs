//! Opt-in public upstream checks through the production transport.
//! Writable scenarios use only the explicitly supplied isolated local root.

use super::*;
use crate::browse::{DisplayNames, Launch, Place};
use crate::scraper::libretro::{self, Artwork, Database};
use crate::scraper::{ImagePolicy, MetadataPolicy, ScraperSource};

fn finish_live(mut job: Job) -> Progress {
    let until = Instant::now() + Duration::from_secs(180);
    loop {
        assert!(
            Instant::now() < until,
            "public Libretro scrape exceeded its deadline"
        );
        match job.try_recv() {
            Some(Event::Finished(progress)) => return progress,
            Some(Event::Failed { error, .. }) => panic!("public Libretro scrape failed: {error}"),
            Some(Event::Cancelled(_)) => {
                panic!("public Libretro scrape was unexpectedly cancelled")
            }
            _ => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

#[test]
#[ignore = "opt-in public HTTPS/database/artwork check with an isolated local destination"]
fn live_libretro_database_artwork_and_xml_roundtrip() {
    let root = std::env::var_os("DEGAUSS_LIBRETRO_TEST_ROOT")
        .map(PathBuf::from)
        .expect("set DEGAUSS_LIBRETRO_TEST_ROOT to an isolated project test directory");
    std::fs::create_dir_all(&root).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = CurlTransport::new(cancelled.clone());
    for name in [
        "MAME",
        "Atari - 2600",
        "Nintendo - Nintendo Entertainment System",
        "Sony - PlayStation",
        "Commodore - CD32",
    ] {
        let before = Instant::now();
        let database = Database::load(
            name,
            &root.join("cache"),
            &transport,
            &cancelled,
            &mut |_| {},
        )
        .unwrap();
        assert!(
            !database.search("a", Artwork::Screenshot).is_empty(),
            "real {name} returned no records"
        );
        eprintln!(
            "public database {name}: {} ms",
            before.elapsed().as_millis()
        );
        if name == "MAME" {
            let Lookup::Found(matched) = database
                .lookup("Battle K-Road", None, Artwork::Screenshot)
                .lookup
            else {
                panic!("canonical MAME entry was not an exact match")
            };
            let image = libretro::download_image(
                &libretro::thumbnail_candidates(&matched, None),
                &transport,
                16 * 1024 * 1024,
                &cancelled,
            )
            .unwrap();
            let decoded = decode_media_image(&image, [0; 3], 320).unwrap();
            assert!(decoded.width > 1 && decoded.height > 1);
        }
    }
    let missing = transport
        .get_libretro(
            "https://thumbnails.libretro.com/MAME/Named_Snaps/Degauss_Nonexistent_Test_73F908.png",
            1024 * 1024,
        )
        .unwrap();
    assert_eq!(
        libretro::checked_response(missing, "image")
            .err()
            .unwrap()
            .kind,
        ErrorKind::NotFound
    );
    for artwork in Artwork::ALL {
        let db = Database::load(
            "Atari - 2600",
            &root.join("cache"),
            &transport,
            &cancelled,
            &mut |_| {},
        )
        .unwrap();
        let Lookup::Found(matched) = db.lookup("Adventure (USA)", None, artwork).lookup else {
            panic!("Atari title was not an exact match")
        };
        let image = libretro::download_image(
            &libretro::thumbnail_candidates(&matched, None),
            &transport,
            16 * 1024 * 1024,
            &cancelled,
        )
        .unwrap();
        assert!(decode_media_image(&image, [0; 3], 320).unwrap().width > 1);
    }
    let fixture = root.join(format!("isolated-card-{}", std::process::id()));
    let card = fixture.join("_Arcade");
    std::fs::create_dir_all(&card).unwrap();
    let mra = card.join("Battle K-Road.mra");
    std::fs::write(
        &mra,
        "<misterromdescription><name>Battle K-Road</name><rbf>Psikyo</rbf></misterromdescription>",
    )
    .unwrap();
    let system = FoundSystem { def: toml::from_str("name = 'Arcade'\nid = 'Arcade'\nfolders = ['_Arcade']\nextensions = ['mra']\nrbf = '_Arcade/cores'\n").unwrap(), paths: vec![card.clone()], logo_dir: None, menu_folder: Some("Arcade".into()) };
    let scopes = [
        Scope::Game {
            system_id: "Arcade".into(),
            launch: Launch::File(mra.clone()),
            title: "Battle K-Road".into(),
        },
        Scope::Folder {
            system_id: "Arcade".into(),
            place: Place::Dir(card.clone()),
            display_name: "Arcade".into(),
        },
        Scope::System {
            system_id: "Arcade".into(),
            place: Place::Dir(card.clone()),
            display_name: "Arcade".into(),
        },
        Scope::All,
    ];
    for (index, scope) in scopes.into_iter().enumerate() {
        let request = Request {
            scope_label: scope.label().to_string(),
            scope,
            systems: vec![system.clone()],
            names: DisplayNames::default(),
            settings: ScraperSettings {
                source: ScraperSource::Libretro,
                image_policy: ImagePolicy::ReplaceExisting,
                metadata_policy: MetadataPolicy::ReplaceExisting,
                ..Default::default()
            },
            developer: None,
            cache_dir: root.join("cache"),
            artwork_pack_system_ids: HashSet::new(),
        };
        let progress = finish_live(start(request).unwrap());
        assert_eq!(progress.total, 1);
        assert_eq!(progress.completed, 1);
        assert_eq!(progress.updated, usize::from(index == 0));
        assert_eq!(progress.unchanged, usize::from(index != 0));
        assert_eq!(progress.failed, 0);
        assert_eq!(progress.not_found, 0);
        assert_eq!(progress.ambiguous, 0);
        assert_eq!(progress.no_media, 0);
        assert!(progress.account.is_none());
        let list = crate::gamelist::Gamelist::load(&card.join("gamelist.xml"), &card).unwrap();
        let (meta, _) = list.lookup_exact("./Battle K-Road.mra").unwrap();
        assert_eq!(meta.name.as_deref(), Some("Battle K-Road"));
        assert!(std::fs::read_to_string(card.join("gamelist.xml"))
            .unwrap()
            .contains("./media/libretro/"));
    }
    struct NoNetwork;
    impl Transport for NoNetwork {
        fn get(&self, _: &str, _: &[(String, String)], _: u64) -> Result<HttpResponse> {
            panic!("unexpected account request")
        }
        fn get_media(&self, _: &str, _: u64, _: Option<u64>) -> Result<HttpResponse> {
            panic!("valid cached data caused a network request")
        }
    }
    Database::load(
        "MAME",
        &root.join("cache"),
        &NoNetwork,
        &cancelled,
        &mut |_| {},
    )
    .unwrap();
    std::fs::remove_dir_all(fixture).unwrap();
}
