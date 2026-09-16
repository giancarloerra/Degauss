use super::*;
use slint::Model;

fn information_row() -> browse::Row {
    browse::Row {
        name: "Example Game / 日本語".into(),
        sort_key: "EXAMPLE GAME".into(),
        kind: browse::Kind::Play(browse::Launch::File(PathBuf::from("example.nes"))),
        cover: None,
        genre: Some("Puzzle / Action".into()),
        favorite: false,
        below: None,
        details: browse::Details {
            desc: format!(
                "First paragraph.\n\n{}\nFinal paragraph.",
                "Long description. ".repeat(80)
            ),
            publisher: "Example Publisher".into(),
            developer: "Example Developer".into(),
            released: "1993-11-21T00:00:00".into(),
            players: "1–2".into(),
            lang: "English, 日本語".into(),
        },
    }
}

fn action_labels(entries: &[String]) -> Vec<&str> {
    entries
        .iter()
        .filter(|entry| !entry.is_empty())
        .map(String::as_str)
        .collect()
}

#[test]
fn information_preserves_every_metadata_field_and_the_complete_description() {
    let row = information_row();
    let expected = format!(
        "{}\n\nGenre: Puzzle / Action\n\nPublisher: Example Publisher\n\nDeveloper: Example Developer\n\nReleased: 1993-11-21T00:00:00\n\nPlayers: 1–2\n\nLanguage: English, 日本語\n\nDescription\n{}",
        row.name, row.details.desc
    );
    assert_eq!(game_information(&row), expected);
    assert_eq!(row.details.values().len(), browse::Details::LABELS.len());
}

#[test]
fn information_keeps_missing_provider_fields_blank() {
    let mut row = information_row();
    row.genre = None;
    row.details = browse::Details::default();
    let expected = format!(
        "{}\n\nGenre: \n\nPublisher: \n\nDeveloper: \n\nReleased: \n\nPlayers: \n\nLanguage: \n\nDescription\n",
        row.name
    );
    assert_eq!(game_information(&row), expected);
    row.favorite = true;
    assert_eq!(
        game_information(&row),
        expected,
        "favourites reuse the resolved provider data"
    );
}

#[test]
fn manual_search_is_single_game_only_and_preserves_every_bulk_setting() {
    use crate::scraper::Scope;

    let bulk = [
        ScraperRow::Username,
        ScraperRow::Password,
        ScraperRow::Images,
        ScraperRow::ImageType,
        ScraperRow::Metadata,
        ScraperRow::Start,
        ScraperRow::ClearLogin,
        ScraperRow::Back,
    ];
    for scope in [
        Scope::All,
        Scope::System {
            system_id: "NES".into(),
            place: Place::Roots,
            display_name: "Fixture System".into(),
        },
        Scope::Folder {
            system_id: "NES".into(),
            place: Place::Dir(PathBuf::from("fixture-folder")),
            display_name: "Fixture Folder".into(),
        },
    ] {
        assert_eq!(scraper_rows(&scope), bulk);
    }
    let scope = Scope::Game {
        system_id: "NES".into(),
        launch: browse::Launch::File(PathBuf::from("fixture-game.nes")),
        title: "Fixture Game".into(),
    };
    assert_eq!(
        scraper_rows(&scope),
        [
            ScraperRow::Username,
            ScraperRow::Password,
            ScraperRow::Images,
            ScraperRow::ImageType,
            ScraperRow::Metadata,
            ScraperRow::Start,
            ScraperRow::Search,
            ScraperRow::ClearLogin,
            ScraperRow::Back,
        ]
    );
    assert_eq!(ScraperRow::Search.label(), "Search Manually");
}

#[test]
fn manual_search_preserves_title_words_punctuation_and_non_ascii_letters() {
    for (input, expected) in [
        ("Adventure Island", "Adventure Island"),
        ("  Ghosts 'n\tGoblins  ", "Ghosts 'n Goblins"),
        ("Super Mario Bros. 3", "Super Mario Bros. 3"),
        ("Pokémon: 日本語", "Pokémon: 日本語"),
        (" \t\n", ""),
    ] {
        assert_eq!(scraper_search_title(input), expected);
    }
    assert_eq!(squashed("Adventure Island"), "ADVENTUREISLAND");
}

#[test]
fn categories_offer_only_applicable_image_and_exact_place_view_actions() {
    let entries = context_entries(
        Browsing::Categories,
        false,
        None,
        None,
        true,
        ContextActions {
            image_override: Some(true),
            ..Default::default()
        },
    );
    assert_eq!(
        action_labels(&entries),
        [
            CHANGE_CATEGORY_IMAGE,
            CLEAR_CATEGORY_IMAGE,
            CHANGE_VIEW,
            USE_GLOBAL_VIEW
        ]
    );
    let entries = context_entries(
        Browsing::Categories,
        false,
        None,
        None,
        false,
        ContextActions {
            image_override: Some(false),
            ..Default::default()
        },
    );
    assert_eq!(
        action_labels(&entries),
        [CHANGE_CATEGORY_IMAGE, CHANGE_VIEW]
    );
}

#[test]
fn playable_actions_keep_both_random_modes_and_the_correct_favourite_action() {
    for (favorite, expected, absent) in [
        (false, ADD_FAVORITE, REMOVE_FAVORITE),
        (true, REMOVE_FAVORITE, ADD_FAVORITE),
    ] {
        let entries = context_entries(
            Browsing::Games,
            false,
            Some(favorite),
            Some(false),
            false,
            ContextActions::default(),
        );
        let labels = action_labels(&entries);
        assert_eq!(labels.first(), Some(&GAME_INFORMATION));
        assert!(labels
            .windows(2)
            .any(|pair| pair == [RANDOM, RANDOM_FAVORITE]));
        assert!(labels.contains(&expected));
        assert!(!labels.contains(&absent));
        assert!(!labels.contains(&USE_GLOBAL_VIEW));
        for scrape in [SCRAPE_SYSTEM, SCRAPE_FOLDER, SCRAPE_GAME] {
            assert!(
                !labels.contains(&scrape),
                "scraping requires source eligibility"
            );
        }
    }
    let entries = context_entries(
        Browsing::Games,
        false,
        None,
        Some(false),
        false,
        ContextActions::default(),
    );
    let labels = action_labels(&entries);
    assert!(
        !labels.contains(&GAME_INFORMATION),
        "folders are not playable games"
    );
    assert!(!labels.contains(&ADD_FAVORITE));
    assert!(!labels.contains(&REMOVE_FAVORITE));
    assert!(
        labels.contains(&REBUILD_SYSTEM),
        "a folder rebuild still targets the containing system"
    );
}

#[test]
fn every_existing_action_remains_reachable_without_a_placeholder_submenu() {
    let full = ContextActions {
        scrape_scope: true,
        scrape_game: true,
        image_override: None,
        game_data_source: true,
        launch_core: true,
        core_version: true,
        core_version_override: true,
        favorite_folder: false,
        metadata_filters: true,
        rebuild_system: true,
    };
    let cases = [
        context_entries(Browsing::Games, true, Some(false), Some(false), true, full),
        context_entries(
            Browsing::Systems,
            true,
            None,
            Some(false),
            true,
            ContextActions {
                scrape_game: false,
                image_override: Some(true),
                ..full
            },
        ),
        context_entries(
            Browsing::Games,
            false,
            Some(true),
            Some(true),
            false,
            ContextActions::default(),
        ),
    ];
    let mut actual = std::collections::BTreeSet::new();
    for entries in &cases {
        assert!(entries.first().is_some_and(|entry| !entry.is_empty()));
        assert!(entries.last().is_some_and(|entry| !entry.is_empty()));
        assert!(!entries
            .windows(2)
            .any(|pair| pair[0].is_empty() && pair[1].is_empty()));
        let labels = action_labels(entries);
        for action in &labels {
            assert_eq!(
                ContextPage::ALL
                    .into_iter()
                    .filter(|page| page.contains(action))
                    .count(),
                1,
                "{action} belongs to exactly one group"
            );
            assert!(
                context_help(action).len() > 20,
                "{action} has its own explanation"
            );
            assert!(!context_help(action).contains("more actions"));
        }
        assert_eq!(labels.len(), labels.iter().collect::<HashSet<_>>().len());
        actual.extend(labels);
    }
    let expected: std::collections::BTreeSet<_> = [
        GAME_INFORMATION,
        RANDOM,
        RANDOM_FAVORITE,
        ADD_FAVORITE,
        REMOVE_FAVORITE,
        SCRAPE_SYSTEM,
        SCRAPE_FOLDER,
        SCRAPE_GAME,
        LAUNCH_CORE,
        CORE_VERSION,
        USE_DEFAULT_CORE_VERSION,
        GAME_DATA_SOURCE,
        JUMP,
        SEARCH,
        CLEAR_SEARCH,
        FILTER_GAMES,
        CLEAR_FILTERS,
        HIDE_THIS,
        SHOW_THIS,
        REBUILD_SYSTEM,
        CHANGE_CATEGORY_IMAGE,
        CLEAR_CATEGORY_IMAGE,
        CHANGE_VIEW,
        USE_GLOBAL_VIEW,
    ]
    .into();
    assert_eq!(actual, expected);
}

fn fixture_directory() -> PathBuf {
    for attempt in 0_u64.. {
        let path = std::env::temp_dir().join(format!(
            "degauss-ui-acceptance-{}-{attempt}",
            std::process::id()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("creating UI fixture: {error}"),
        }
    }
    unreachable!("fixture directory counter exhausted")
}

fn unopened_fixture_app(root: &Path, window: Rc<MinimalSoftwareWindow>, settings: Settings) -> App {
    unopened_fixture_app_with_systems(root, window, settings, &["NES"], "games/NES")
}

/// The given systems, in that order, all over one games directory under
/// the root: the members of a shared source group share their folder on
/// the card.
fn unopened_fixture_app_with_systems(
    root: &Path,
    window: Rc<MinimalSoftwareWindow>,
    settings: Settings,
    ids: &[&str],
    games: &str,
) -> App {
    let mut config = Config::parse("[app]", &root.join("degauss.toml")).unwrap();
    config.menu_root = root.to_string_lossy().into_owned();
    config.game_roots = vec![root.join("games").to_string_lossy().into_owned()];
    let table = crate::systems::parse_table(
        include_str!("../assets/systems.toml"),
        Path::new("systems.toml"),
    )
    .unwrap();
    let defs: Vec<_> = ids
        .iter()
        .map(|id| {
            table
                .iter()
                .find(|system| system.id == *id)
                .unwrap()
                .clone()
        })
        .collect();
    let loaded = Loaded {
        config,
        settings,
        settings_path: root.join("settings.toml"),
        core_catalogue: Default::default(),
        systems: defs
            .iter()
            .map(|def| FoundSystem {
                def: def.clone(),
                paths: vec![root.join(games)],
                logo_dir: None,
                menu_folder: None,
            })
            .collect(),
        table: defs,
        names: Default::default(),
        logo_dir: None,
        themes_dir: root.join("themes"),
        themes: crate::theme::load_available(&root.join("themes")),
    };
    App::new(
        loaded,
        window,
        DegaussWindow::new().unwrap(),
        StartupTimings::default(),
        352,
        240,
    )
}

fn fixture_app(root: &Path, window: Rc<MinimalSoftwareWindow>, settings: Settings) -> App {
    let mut app = unopened_fixture_app(root, window, settings);
    app.open_system_by_index(0);
    assert!(app.build.is_none(), "fixture indexing must actually finish");
    assert!(
        app.message.is_none(),
        "fixture startup failed: {:?}",
        app.message
    );
    assert_eq!(app.here.len(), 2);
    app.leave_splash();
    assert_eq!(app.screen, Screen::Browse);
    app
}

fn select_option(app: &mut App, page: OptionsPage, option: OptionId) {
    app.open_options_page(page);
    let selected = page
        .ids()
        .iter()
        .position(|candidate| *candidate == option)
        .unwrap();
    app.active_list_mut().select(selected);
}

fn run_browse_bar_settings_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    for saved in [None, Some(false), Some(true)] {
        let mut app = fixture_app(
            root,
            window.clone(),
            Settings {
                show_bar: saved,
                ..Default::default()
            },
        );
        app.refresh();
        assert_eq!(app.show_bar, saved.unwrap_or(true));
        assert_eq!(app.ui.get_bottom_bar_visible(), saved.unwrap_or(true));
        assert_eq!(
            app.settings.show_bar, saved,
            "startup must not rewrite a choice"
        );
        for _ in 0..2 {
            let previous = app.show_bar;
            select_option(&mut app, OptionsPage::Appearance, OptionId::ShowBar);
            app.handle(Action::Accept);
            app.handle(Action::Quit);
            app.handle(Action::Quit);
            assert_eq!(app.show_bar, !previous);
            assert_eq!(app.settings.show_bar, Some(!previous));
            let reloaded = Settings::load(&app.settings_path).unwrap();
            assert_eq!(reloaded.show_bar, Some(!previous));
            app.ui.hide().unwrap();
            drop(app);
            app = fixture_app(root, window.clone(), reloaded);
            assert_eq!(app.show_bar, !previous, "the saved choice survives restart");
        }
        app.ui.hide().unwrap();
    }
}

fn run_cores_browser_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("cores-browser");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let console = root.join("_Console");
    let ra = root.join("_RA_Cores");
    let unstable = root.join("_Unstable");
    std::fs::create_dir_all(&console).unwrap();
    std::fs::create_dir_all(&ra).unwrap();
    std::fs::create_dir_all(&unstable).unwrap();
    let standard = console.join("NES_20260916.rbf");
    let ra_launcher = ra.join("RA_NES.mgl");
    let unstable_launcher = unstable.join("NES_unstable_20260916_ab12.rbf");
    std::fs::write(&standard, b"fixture").unwrap();
    std::fs::write(&unstable_launcher, b"fixture").unwrap();
    std::fs::write(
        &ra_launcher,
        "<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir=\"1\">RA_NES</setname></mistergamedescription>",
    )
    .unwrap();
    std::fs::create_dir_all(ra.join("Cores")).unwrap();
    std::fs::write(ra.join("Cores/NES.rbf"), b"fixture").unwrap();

    let mut app = fixture_app(&root, window, Settings::default());
    app.open_system = None;
    app.library = None;
    app.system_cache = None;
    app.opened_config = None;
    app.open_category = None;
    app.trail.clear();
    app.here.clear();
    app.browsing = Browsing::Categories;
    app.rebuild_system_list();
    assert!(
        !app.categories
            .iter()
            .any(|(name, _)| name == CORES_CATEGORY),
        "the backwards-compatible default leaves Cores hidden"
    );

    select_option(&mut app, OptionsPage::Library, OptionId::ShowCores);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Options);
    assert_eq!(app.options_page, OptionsPage::Library);
    assert_eq!(
        app.option_ids()[app.active_list().selected()],
        OptionId::ShowCores,
        "the targeted catalogue build keeps the user on the option they changed"
    );
    assert_eq!(app.core_catalogue.entries.len(), 3);
    assert_eq!(
        crate::cache::load_core_catalogue(&app.cache_dir),
        Some(app.core_catalogue.clone()),
        "the first opt-in builds and persists the catalogue explicitly"
    );

    // The Cores catalogue is an optional companion to the main index. A
    // failure to persist it must be reported, but must not discard a complete
    // system discovery or the newly built system lists.
    let catalogue_path = crate::cache::core_catalogue_path(&app.cache_dir);
    std::fs::remove_file(&catalogue_path).unwrap();
    std::fs::create_dir(&catalogue_path).unwrap();
    std::fs::write(root.join("games/NES/Third Game.nes"), b"fixture").unwrap();
    app.rebuild_all_systems();
    app.finish_background_work_for_headless();
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 3);
    assert_eq!(
        app.index_terminal.as_ref().unwrap().state,
        "Finished With Problems"
    );
    assert!(app
        .index_terminal
        .as_ref()
        .unwrap()
        .problem
        .contains("Core catalogue not saved"));
    std::fs::remove_dir(&catalogue_path).unwrap();
    crate::cache::save_core_catalogue(&app.cache_dir, &app.core_catalogue).unwrap();

    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Options);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::OptionsRoot);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Menu);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.show_cores);
    let cores = app
        .categories
        .iter()
        .position(|(name, _)| name == CORES_CATEGORY)
        .expect("the enabled Cores category is on Home");
    app.category_list.select(cores);
    app.handle(Action::Accept);
    assert_eq!(app.browsing, Browsing::Systems);
    assert!(app.in_cores_browser());
    assert_eq!(app.core_categories(), vec![("Console".into(), 3)]);

    app.handle(Action::Accept);
    assert_eq!(app.browsing, Browsing::Games);
    assert_eq!(app.here.len(), 3);
    assert_eq!(app.here[0].name, "NES [Standard]");
    assert_eq!(app.here[1].name, "NES [RA]");
    assert_eq!(app.here[2].name, "NES [Unstable: 20260916_ab12]");
    app.open_context();
    assert_eq!(
        app.context_actions,
        vec![JUMP, SEARCH, REBUILD_CORES, CHANGE_VIEW]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    );
    app.handle(Action::Quit);
    app.filter = squashed("[RA]");
    app.apply_filter();
    assert_eq!(app.here.len(), 1);
    assert!(app.here[0].name.ends_with("[RA]"));
    app.clear_filter();

    app.category_system
        .insert("Console".to_string(), "NES".to_string());
    app.left_at.push(crate::state::LeftAt {
        system: "NES".to_string(),
        place: "dir:/media/fat/games/NES".to_string(),
        row: "game:remembered.nes".to_string(),
    });
    app.game_list.select(1);
    let saved = app.position();
    assert_eq!(saved.system, CORES_SYSTEM_ID);
    app.category_system.clear();
    app.left_at.clear();
    app.open_category = None;
    app.core_category = None;
    app.browsing = Browsing::Categories;
    app.rebuild_system_list();
    app.restore_position(&saved);
    assert!(app.in_cores_browser());
    assert_eq!(app.browsing, Browsing::Games);
    assert_eq!(app.game_list.selected(), 1);
    assert!(app.here[1].name.ends_with("[RA]"));
    assert_eq!(app.category_system, saved.category_system);
    assert_eq!(app.left_at, saved.left_at);

    let Outcome::Launch { plan, history, .. } = app.handle(Action::Accept).expect("core launch")
    else {
        panic!("unexpected core launch outcome")
    };
    assert!(history.is_none(), "launching a core is not game history");
    assert_eq!(
        plan.command,
        format!(
            "load_core {}\n",
            ra_launcher.canonicalize().unwrap().display()
        )
    );
    std::fs::remove_file(&standard).unwrap();
    app.game_list.select(0);
    assert!(app.handle(Action::Accept).is_none());
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("no longer installed")));

    let previous = app.core_catalogue.clone();
    let menu_root = app.config.menu_root.clone();
    app.config.menu_root = root
        .join("missing-menu-root")
        .to_string_lossy()
        .into_owned();
    app.rebuild_cores_catalogue();
    assert_eq!(app.core_catalogue, previous);
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("Cores list was not changed")));
    app.config.menu_root = menu_root;
    app.ui.hide().unwrap();
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

fn run_misterzine_browser_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("misterzine-browser");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let core = root.join("_Console/NES_20260916.rbf");
    std::fs::create_dir_all(core.parent().unwrap()).unwrap();
    std::fs::write(&core, b"fixture").unwrap();

    let mut app = fixture_app(&root, window.clone(), Settings::default());
    app.open_system = None;
    app.library = None;
    app.system_cache = None;
    app.opened_config = None;
    app.open_category = None;
    app.trail.clear();
    app.here.clear();
    app.browsing = Browsing::Categories;
    app.rebuild_system_list();
    assert_eq!(app.settings.show_misterzine, None);
    assert!(!app.show_misterzine);
    assert!(app.misterzine_job.is_none());
    assert!(!crate::misterzine::cache_path(&app.cache_dir).exists());
    assert!(!app
        .categories
        .iter()
        .any(|(name, _)| name == MISTERZINE_CATEGORY));

    select_option(&mut app, OptionsPage::Library, OptionId::ShowMisterZine);
    app.handle(Action::Accept);
    assert_eq!(app.settings.show_misterzine, Some(true));
    assert!(app.show_misterzine);
    assert!(app.misterzine_job.is_none());
    assert!(!crate::misterzine::cache_path(&app.cache_dir).exists());
    assert!(app
        .categories
        .iter()
        .any(|(name, _)| name == MISTERZINE_CATEGORY));

    app.screen = Screen::Browse;
    app.open_category = Some(MISTERZINE_CATEGORY.into());
    app.browsing = Browsing::Games;
    app.misterzine_items = vec![
        crate::misterzine::Item::fixture(
            "Installed Release",
            crate::misterzine::LocalState::Current,
            Some(core.clone()),
        ),
        crate::misterzine::Item::fixture(
            "Missing Release",
            crate::misterzine::LocalState::NotInstalled,
            None,
        ),
    ];
    app.rebuild_misterzine_rows();
    assert_eq!(app.here.len(), 2);
    app.open_context();
    assert_eq!(
        app.context_actions,
        [REFRESH_MISTERZINE, INSTALLED_ONLY, ABOUT_MISTERZINE]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    );
    let installed_only = app
        .menu
        .iter()
        .position(|entry| entry == INSTALLED_ONLY)
        .unwrap();
    app.menu_list.select(installed_only);
    app.handle(Action::Accept);
    assert!(app.misterzine_installed_only);
    assert_eq!(app.here.len(), 1);
    let Outcome::Launch { plan, .. } = app.handle(Action::Accept).expect("local core launch")
    else {
        panic!("unexpected MiSTerZine launch outcome")
    };
    assert_eq!(
        plan.command,
        format!("load_core {}\n", core.canonicalize().unwrap().display())
    );

    std::fs::remove_file(&core).unwrap();
    assert!(app.handle(Action::Accept).is_none());
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("no longer installed")));
    app.message = None;
    app.open_context();
    let installed_only = app
        .menu
        .iter()
        .position(|entry| entry == INSTALLED_ONLY)
        .unwrap();
    app.menu_list.select(installed_only);
    app.handle(Action::Accept);
    assert!(!app.misterzine_installed_only);
    app.game_list.select(1);
    assert!(app.handle(Action::Accept).is_none());
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message == "Missing Release is not installed on this MiSTer."));
    app.message = None;

    app.open_context();
    let about = app
        .menu
        .iter()
        .position(|entry| entry == ABOUT_MISTERZINE)
        .unwrap();
    app.menu_list.select(about);
    app.handle(Action::Accept);
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("CC BY 4.0")));
    assert!(app.save_settings());
    let settings = Settings::load(&app.settings_path).unwrap();
    app.ui.hide().unwrap();
    drop(app);

    let mut app = fixture_app(&root, window, settings);
    assert!(app.show_misterzine);
    assert!(
        !app.misterzine_installed_only,
        "the view filter is not persisted"
    );
    assert!(
        app.misterzine_job.is_none(),
        "startup does not contact MiSTerZine"
    );
    select_option(&mut app, OptionsPage::Library, OptionId::ShowMisterZine);
    app.handle(Action::Accept);
    assert!(!app.show_misterzine);
    assert!(!app
        .categories
        .iter()
        .any(|(name, _)| name == MISTERZINE_CATEGORY));
    app.ui.hide().unwrap();
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

fn run_handheld_category_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let mut app = fixture_app(root, window, Settings::default());
    let mut handheld = app.all_systems[0].clone();
    handheld.def.handheld = true;
    let mut console = handheld.clone();
    console.def.id = "SNES".into();
    console.def.name = "Super Nintendo".into();
    console.def.handheld = false;
    console.paths = vec![root.join("games/SNES")];
    app.all_systems = vec![handheld, console];
    app.open_category = Some("Console".into());
    app.category_system.insert("Console".into(), "NES".into());
    app.rebuild_system_list();
    app.browsing = Browsing::Games;
    app.game_list.select(1);

    assert_eq!(app.settings.separate_handheld_category, None);
    assert_eq!(app.open_category.as_deref(), Some("Console"));
    assert_eq!(
        app.categories
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["Console"]
    );
    let saved = app.position();
    let selected_game = row_key(&app.here[app.game_list.selected()]);
    let cache_before = cache_snapshot(&app.cache_dir);
    let launch_before = app
        .all_systems
        .iter()
        .find(|system| system.def.id == "NES")
        .unwrap()
        .to_config();
    app.settings
        .custom_views
        .systems
        .insert(HANDHELD_CATEGORY.into(), "gallery".into());

    select_option(
        &mut app,
        OptionsPage::Library,
        OptionId::SeparateHandheldCategory,
    );
    app.handle(Action::Accept);

    assert_eq!(app.settings.separate_handheld_category, Some(true));
    assert_eq!(app.open_category.as_deref(), Some(HANDHELD_CATEGORY));
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected_game);
    assert_eq!(
        app.categories
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["Console", HANDHELD_CATEGORY]
    );
    assert_eq!(app.systems.len(), 1);
    assert_eq!(app.systems[0].def.id, "NES");
    assert_eq!(cache_snapshot(&app.cache_dir), cache_before);
    assert_eq!(
        app.settings
            .custom_views
            .systems
            .get(HANDHELD_CATEGORY)
            .map(String::as_str),
        Some("gallery")
    );
    app.browsing = Browsing::Systems;
    app.resolve_view();
    assert_eq!(app.layout, Layout::Gallery);
    app.browsing = Browsing::Games;
    app.resolve_view();

    app.restore_position(&saved);
    app.finish_background_work_for_headless();
    assert_eq!(app.open_category.as_deref(), Some(HANDHELD_CATEGORY));
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert!(
        app.message.is_none(),
        "restoring the saved position must not leave an overlay: {:?}",
        app.message
    );
    let selected_after_restore = row_key(&app.here[app.game_list.selected()]);

    select_option(
        &mut app,
        OptionsPage::Library,
        OptionId::SeparateHandheldCategory,
    );
    app.handle(Action::Accept);
    assert_eq!(app.settings.separate_handheld_category, Some(false));
    assert_eq!(app.open_category.as_deref(), Some("Console"));
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert_eq!(
        row_key(&app.here[app.game_list.selected()]),
        selected_after_restore
    );
    assert_eq!(cache_snapshot(&app.cache_dir), cache_before);
    assert_eq!(
        app.settings
            .custom_views
            .systems
            .get(HANDHELD_CATEGORY)
            .map(String::as_str),
        Some("gallery"),
        "the optional category's saved view remains dormant when disabled"
    );

    let launch_after = app
        .all_systems
        .iter()
        .find(|system| system.def.id == "NES")
        .unwrap()
        .to_config();
    assert_eq!(launch_after.path, launch_before.path);
    assert_eq!(launch_after.extensions, launch_before.extensions);
    assert_eq!(launch_after.rbf, launch_before.rbf);
    assert_eq!(launch_after.launch, launch_before.launch);
    assert_eq!(launch_after.setname, launch_before.setname);

    app.handle(Action::Quit);
    let reloaded = Settings::load(&app.settings_path).unwrap();
    assert_eq!(reloaded.separate_handheld_category, Some(false));
    assert_eq!(
        reloaded
            .custom_views
            .systems
            .get(HANDHELD_CATEGORY)
            .map(String::as_str),
        Some("gallery")
    );
    app.ui.hide().unwrap();
}

fn run_scripts_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let scripts_root = root.join("Scripts");
    let nested = scripts_root.join("Tools and Tests");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(scripts_root.join("Empty Folder")).unwrap();
    std::fs::write(
        scripts_root.join("Example Script.sh"),
        "#!/bin/bash\nexit 0\n",
    )
    .unwrap();
    std::fs::write(nested.join("Example Tool.sh"), "#!/bin/bash\nexit 0\n").unwrap();
    std::fs::write(scripts_root.join("degauss.sh"), "#!/bin/bash\nexit 0\n").unwrap();
    std::fs::write(scripts_root.join(".hidden.sh"), "#!/bin/bash\nexit 0\n").unwrap();

    for saved in [None, Some(false), Some(true)] {
        let mut app = fixture_app(
            root,
            window.clone(),
            Settings {
                show_scripts: saved,
                ..Default::default()
            },
        );
        app.open_menu();
        assert_eq!(
            app.menu.iter().any(|entry| entry == "Scripts"),
            saved.unwrap_or(true)
        );
        assert_eq!(
            app.settings.show_scripts, saved,
            "startup preserves an absent or explicit choice"
        );
        for _ in 0..2 {
            let was_visible = app.settings.show_scripts.unwrap_or(true);
            select_option(&mut app, OptionsPage::Library, OptionId::ShowScripts);
            app.handle(Action::Accept);
            app.handle(Action::Quit);
            app.handle(Action::Quit);
            assert_eq!(app.screen, Screen::Menu);
            assert_eq!(
                app.menu.iter().any(|entry| entry == "Scripts"),
                !was_visible
            );
            let settings = Settings::load(&app.settings_path).unwrap();
            assert_eq!(settings.show_scripts, Some(!was_visible));
            app.ui.hide().unwrap();
            drop(app);
            app = fixture_app(root, window.clone(), settings);
        }
        app.ui.hide().unwrap();
    }

    let mut app = fixture_app(root, window.clone(), Settings::default());
    app.game_list.select(1);
    let selected = row_key(&app.here[app.game_list.selected()]);
    let games = app.here.len();
    app.open_menu();
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == "Scripts")
            .unwrap(),
    );
    assert_eq!(app.handle(Action::Accept), None);
    assert_eq!(app.screen, Screen::Scripts);
    assert_eq!(
        app.menu,
        [
            "[ Empty Folder ]",
            "[ Tools and Tests ]",
            "Example Script.sh"
        ]
    );
    assert!(app.build.is_none());
    assert!(app.available_hold_shortcuts().iter().all(Option::is_none));
    app.refresh();
    assert!(app.ui.get_status().contains("folder"));
    app.handle(Action::Accept);
    assert!(app.scripts_entries.is_empty());
    app.refresh();
    assert!(app.ui.get_status().contains("No scripts"));
    app.handle(Action::Quit);
    assert_eq!(app.menu_list.selected(), 0);
    app.menu_list.select(1);
    app.handle(Action::Accept);
    assert_eq!(app.menu, ["Example Tool.sh"]);
    app.handle(Action::Accept);
    assert!(matches!(app.pending, Some(Pending::RunScript(_))));
    assert_eq!(app.handle(Action::Quit), None);
    assert!(app.pending.is_none());
    assert_eq!(app.screen, Screen::Scripts);
    let settings_path = app.settings_path.clone();
    app.settings_path = scripts_root.join("Example Script.sh/settings.toml");
    app.handle(Action::Accept);
    assert_eq!(
        app.handle(Action::Accept),
        None,
        "failed settings writes cannot hand off"
    );
    assert!(app.message.is_some());
    assert_eq!(app.screen, Screen::Scripts);
    app.handle(Action::Quit);
    app.settings_path = settings_path;
    app.handle(Action::Accept);
    assert!(matches!(
        app.handle(Action::Accept),
        Some(Outcome::Script(_))
    ));
    app.resume_scripts(&nested.join("Example Tool.sh"));
    assert_eq!(app.screen, Screen::Scripts);
    assert_eq!(app.menu[app.menu_list.selected()], "Example Tool.sh");
    app.handle(Action::Quit);
    assert_eq!(app.menu[app.menu_list.selected()], "[ Tools and Tests ]");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Menu);
    assert_eq!(app.menu[app.menu_list.selected()], "Scripts");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected);
    assert_eq!(app.here.len(), games);
    assert!(app.build.is_none(), "scripts must not start game indexing");
    app.ui.hide().unwrap();
}

fn run_artwork_visibility_flow(app: &mut App) {
    let layout = app.layout;
    let selected = app.game_list.selected();
    let cover = app.here[selected].cover.clone();
    app.here[selected].cover =
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos/NES.png"));
    app.layout = Layout::Details;
    app.load_art();
    assert!(
        app.ui.get_has_art(),
        "start with a genuinely loaded picture"
    );
    let loads = app.art.loads;
    for visible in [false, true, false, true] {
        select_option(app, OptionsPage::Appearance, OptionId::ShowArt);
        app.handle(Action::Faster);
        assert_eq!(app.show_art, visible);
        for _ in 0..3 {
            if app.screen == Screen::Browse {
                break;
            }
            app.handle(Action::Quit);
        }
        assert_eq!(app.screen, Screen::Browse);
        app.layout = Layout::Details;
        app.load_art();
        assert_eq!(
            app.ui.get_has_art(),
            visible,
            "return from Artwork toggle must update the loaded image"
        );
        assert_eq!(
            Settings::load(&app.settings_path).unwrap().show_art,
            Some(visible)
        );
        if !visible {
            let size = app.ui.get_art().size();
            assert_eq!((size.width, size.height), (0, 0));
            assert!(app.ui.get_art_caption().is_empty());
            assert!(!app.ui.get_art_heart());
        }
    }
    assert_eq!(
        app.art.loads,
        loads + 2,
        "disabled artwork must not cause an image load"
    );
    app.here[selected].cover = cover;
    app.layout = layout;
}

fn assert_current_art_matte(app: &mut App, path: &Path, ground: [u8; 3]) {
    app.load_art();
    app.refresh();
    let image = if matches!(app.screen, Screen::Information)
        || (app.screen == Screen::Browse && app.layout == Layout::Details)
    {
        assert!(app.ui.get_has_art());
        app.ui.get_art()
    } else {
        let selected = if app.screen == Screen::Screensaver {
            0
        } else {
            app.ui.get_selected() as usize
        };
        let row = app.rows.row_data(selected).unwrap();
        assert!(row.has_cover, "the actual rendered row must contain art");
        row.cover
    };
    let edge = if app.screen == Screen::Browse && app.layout == Layout::Gallery {
        app.gallery_covers.max_edge()
    } else {
        app.covers.max_edge()
    };
    let expected = crate::covers::load_scaled(path, edge, ground, &mut Default::default()).unwrap();
    assert!(
        expected.rgb.as_chunks::<3>().0.contains(&ground),
        "the real PNG must contain transparent pixels to exercise the matte"
    );
    assert_eq!(
        image.to_rgb8().unwrap().as_bytes(),
        expected.rgb.as_slice(),
        "{:?}/{} must composite transparent artwork onto its actual background",
        app.screen,
        app.layout.label()
    );
}

fn run_artwork_matte_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("artwork-matte");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(games.join(name), b"fixture").unwrap();
    }
    let path = games.join("transparent.png");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos/NES.png"),
        &path,
    )
    .unwrap();
    let mut app = fixture_app(&root, window, Settings::default());
    for row in &mut app.here {
        row.cover = Some(path.clone());
    }
    assert_eq!(app.covers.capacity(), app.config.app.art_cache.max(8));
    assert_eq!(
        app.group_covers.capacity(),
        8,
        "group previews must not duplicate a full game-art cache"
    );
    app.themes = vec![Theme {
        name: "Matte Check".into(),
        file: crate::theme::ThemeFile::parse("background = \"#735b83\"\nsurface = \"#172f47\"")
            .unwrap(),
    }];
    for (theme, changed) in [("standard", false), ("changed", true)] {
        if changed {
            select_option(&mut app, OptionsPage::Appearance, OptionId::Theme);
            app.handle(Action::Faster);
            assert_eq!(app.active_theme, Some(0));
            assert_eq!(app.covers.len(), 0, "theme changes invalidate old mattes");
            assert_eq!(app.group_covers.capacity(), 8);
        }
        let palette = app.effective_palette();
        let background = [
            palette.background.r,
            palette.background.g,
            palette.background.b,
        ];
        let surface = [palette.surface.r, palette.surface.g, palette.surface.b];
        assert_ne!(background, surface);
        for layout in [
            Layout::Carousel,
            Layout::Details,
            Layout::Tiled,
            Layout::Gallery,
        ] {
            app.screen = Screen::Browse;
            app.set_layout(layout);
            let ground = if layout == Layout::Carousel {
                background
            } else {
                surface
            };
            assert_current_art_matte(&mut app, &path, ground);
            let decoded = (app.covers.stats.decoded, app.gallery_covers.stats.decoded);
            for _ in 0..3 {
                assert_current_art_matte(&mut app, &path, ground);
            }
            assert_eq!(
                (app.covers.stats.decoded, app.gallery_covers.stats.decoded),
                decoded,
                "repeated use of the same matte must remain cached"
            );
            assert_eq!(
                app.group_covers.len(),
                0,
                "game art uses the existing full-size pool"
            );
            capture_live_if_requested(&mut app, &format!("matte-{theme}-{}", layout.label()));
        }
        app.screen = Screen::Information;
        app.apply_geometry();
        app.ui
            .set_information_text(game_information(&app.here[app.game_list.selected()]).into());
        assert_current_art_matte(&mut app, &path, background);
        capture_live_if_requested(&mut app, &format!("matte-{theme}-information"));
        app.saver_pool = vec![SaverPicture {
            path: path.clone(),
            caption: "Fixture Game - NES".into(),
        }];
        app.screen = Screen::Screensaver;
        app.apply_geometry();
        assert_current_art_matte(&mut app, &path, [0, 0, 0]);
        capture_live_if_requested(&mut app, &format!("matte-{theme}-screensaver"));
    }
}

fn run_selected_controls_flow(app: &mut App) {
    let width = app.width;
    for size in [352, 640] {
        app.width = size;
        for page in OptionsPage::ALL {
            app.open_options_page(page);
            for (index, id) in page.ids().iter().enumerate() {
                app.select(index);
                app.update_chrome();
                let adjustable = matches!(
                    option_operation(*id, OptionInput::Next),
                    OptionOperation::Adjust(_)
                );
                assert_eq!(
                    app.ui.get_plain_help().contains("←→"),
                    adjustable,
                    "{id:?} footer must follow actual activation behaviour"
                );
            }
        }
        app.open_scraper(
            crate::scraper::Scope::Game {
                system_id: "NES".into(),
                launch: browse::Launch::File(PathBuf::from("fixture.nes")),
                title: "Fixture Game".into(),
            },
            Screen::Browse,
        );
        for (index, row) in scraper_rows(&app.scraper_scope).iter().enumerate() {
            app.select(index);
            app.update_chrome();
            let help = app.ui.get_plain_help();
            let adjustable = matches!(
                row,
                ScraperRow::Images | ScraperRow::ImageType | ScraperRow::Metadata
            );
            assert_eq!(
                help.contains("←→") || help.contains("Left/Right"),
                adjustable,
                "{row:?} footer must not promise an inert horizontal action"
            );
        }
        for action in [CHANGE_VIEW, GAME_INFORMATION, REBUILD_SYSTEM] {
            app.reopen_context_for(action);
            app.update_chrome();
            assert_eq!(
                app.ui.get_plain_help().contains("←→"),
                action == CHANGE_VIEW
            );
        }
        app.open_context();
        app.update_chrome();
        assert_eq!(app.ui.get_plain_help(), "A Open   B Back");
    }
    app.width = width;
    app.screen = Screen::Browse;
    app.apply_geometry();
}

fn run_hold_shortcut_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("hold-shortcuts");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    std::fs::create_dir(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NES.rbf"), b"fixture core").unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture game").unwrap();
    }
    let mut app = fixture_app(&root, window, Settings::default());
    assert_eq!(
        app.hold_shortcuts,
        [HoldShortcut::None; 4],
        "existing settings retain immediate face buttons"
    );
    app.handle(Action::Menu);
    assert_eq!(app.screen, Screen::Menu);
    app.handle(Action::Quit);
    select_option(&mut app, OptionsPage::Shortcuts, OptionId::HoldY);
    app.handle(Action::Accept);
    app.handle(Action::Accept);
    assert_eq!(
        app.hold_shortcuts[HoldButton::Y.index()],
        HoldShortcut::RandomGame
    );
    app.handle(Action::Quit);
    let saved = Settings::load(&app.settings_path).unwrap();
    assert_eq!(saved.hold_y_shortcut, Some(HoldShortcut::RandomGame));
    assert_eq!(saved.hold_y_random, Some(true));
    app.screen = Screen::Browse;
    assert_eq!(
        app.available_hold_shortcuts()[HoldButton::Y.index()],
        Some(HoldShortcut::RandomGame)
    );
    let held = Instant::now();
    let mut repeater = Repeater::new(RepeatConfig::default());
    repeater.set_hold_shortcuts(app.available_hold_shortcuts());
    assert_eq!(repeater.press(Action::Menu, held), None);
    let short = repeater
        .release(Action::Menu, held + Duration::from_millis(999))
        .unwrap();
    assert_eq!(short, Action::Menu);
    app.handle(short);
    assert_eq!(app.screen, Screen::Menu);
    app.handle(Action::Quit);

    for launches in [false, true] {
        app.random_launches = launches;
        app.message = None;
        app.screen = Screen::Browse;
        repeater.set_hold_shortcuts(app.available_hold_shortcuts());
        let pressed = Instant::now();
        assert_eq!(repeater.press(Action::Menu, pressed), None);
        let due = repeater.tick(pressed + Duration::from_secs(1));
        assert_eq!(due, vec![Action::HoldShortcut(HoldShortcut::RandomGame)]);
        let outcome = app.handle(due[0]);
        assert_eq!(
            matches!(outcome, Some(Outcome::Launch { .. })),
            launches,
            "Hold Y follows Random Game Behaviour: {:?}",
            app.message
        );
        assert_eq!(app.screen, Screen::Browse);
        assert!(repeater.tick(pressed + Duration::from_secs(2)).is_empty());
        assert_eq!(
            repeater.release(Action::Menu, pressed + Duration::from_secs(3)),
            None
        );
    }

    app.random_launches = false;
    app.message = None;
    app.screen = Screen::Browse;
    let place = app.current_view_place().unwrap();
    let previous_layout = app.layout;
    assert!(app.perform_hold_shortcut(HoldShortcut::CycleView).is_none());
    assert_eq!(app.layout, previous_layout.next());
    assert_eq!(
        place.get(&app.settings.custom_views),
        Some(app.layout.label()),
        "Cycle View saves the exact browse place"
    );
    let saved = Settings::load(&app.settings_path).unwrap();
    assert_eq!(
        place.get(&saved.custom_views),
        Some(app.layout.label()),
        "Cycle View is durable immediately"
    );

    app.browsing = Browsing::Systems;
    app.open_category = Some("Console".into());
    app.layout = Layout::List;
    app.perform_hold_shortcut(HoldShortcut::CycleView);
    assert_eq!(
        app.settings
            .custom_views
            .systems
            .get("Console")
            .map(String::as_str),
        Some(Layout::MultiList.label())
    );
    app.browsing = Browsing::Categories;
    app.layout = Layout::Gallery;
    app.perform_hold_shortcut(HoldShortcut::CycleView);
    assert_eq!(
        app.settings.custom_views.categories.as_deref(),
        Some(Layout::Details.label())
    );
    app.browsing = Browsing::Games;

    for (shortcut, mode) in [
        (HoldShortcut::SearchThisFolder, FindMode::Search),
        (HoldShortcut::JumpToLetter, FindMode::Jump),
    ] {
        app.screen = Screen::Browse;
        assert!(app.perform_hold_shortcut(shortcut).is_none());
        assert_eq!(app.screen, Screen::Find);
        assert_eq!(app.find_mode, mode);
        app.handle(Action::Quit);
        assert_eq!(app.screen, Screen::Browse);
    }

    app.screen = Screen::Browse;
    assert!(app
        .perform_hold_shortcut(HoldShortcut::GameInformation)
        .is_none());
    assert_eq!(app.screen, Screen::Information);
    app.finish_background_work_for_headless();
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Context);
    assert!(app.menu.iter().any(|entry| entry == GAME_INFORMATION));
    app.handle(Action::Quit);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);

    assert!(app
        .perform_hold_shortcut(HoldShortcut::AddRemoveFavourite)
        .is_none());
    assert_eq!(app.screen, Screen::FavoriteFolder);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);

    assert!(app
        .perform_hold_shortcut(HoldShortcut::RandomFavourite)
        .is_none());
    assert_eq!(
        app.message.as_deref(),
        Some("No favourites under this folder.")
    );
    app.handle(Action::Quit);
    assert!(app.message.is_none());
    assert_eq!(app.screen, Screen::Browse);

    app.hold_shortcuts = [
        HoldShortcut::CycleView,
        HoldShortcut::SearchThisFolder,
        HoldShortcut::GameInformation,
        HoldShortcut::RandomGame,
    ];

    for screen in [
        Screen::Splash,
        Screen::Menu,
        Screen::Scripts,
        Screen::Context,
        Screen::OptionsRoot,
        Screen::Options,
        Screen::Information,
        Screen::ThemeEditor,
        Screen::Advanced,
        Screen::Help,
        Screen::About,
        Screen::Screensaver,
        Screen::Find,
        Screen::FavoriteFolder,
        Screen::Scraper,
        Screen::ScraperKeyboard,
        Screen::ScraperProgress,
        Screen::CategoryImage,
        Screen::ScraperMatches,
        Screen::GameDataSource,
        Screen::ArtworkPackLocation,
        Screen::ArtworkPackDirectory,
        Screen::SourceProgress,
    ] {
        app.screen = screen;
        assert!(
            app.available_hold_shortcuts().iter().all(Option::is_none),
            "{screen:?} must never delay a face button or run a browsing shortcut"
        );
    }
    app.screen = Screen::Browse;
    for browsing in [Browsing::Systems, Browsing::Categories] {
        app.browsing = browsing;
        assert!(app.available_hold_shortcuts()[HoldButton::Y.index()].is_none());
    }
    app.browsing = Browsing::Games;
    app.message = Some("Fixture message".into());
    assert!(app.available_hold_shortcuts()[HoldButton::Y.index()].is_none());
    app.message = None;
    app.pending = Some(Pending::Exit);
    assert!(app.available_hold_shortcuts()[HoldButton::Y.index()].is_none());
    app.pending = None;
    app.index_terminal = Some(IndexOverview::default());
    assert!(app.available_hold_shortcuts()[HoldButton::Y.index()].is_none());
    app.index_terminal = None;
    app.scraper_pending_terminal = Some(ScraperTerminal::Finished);
    assert!(app.available_hold_shortcuts()[HoldButton::Y.index()].is_none());
    app.scraper_pending_terminal = None;

    app.saver_return = Screen::Browse;
    app.screen = Screen::Screensaver;
    repeater.set_hold_shortcuts(app.available_hold_shortcuts());
    let wake = Instant::now();
    assert_eq!(repeater.press(Action::Menu, wake), Some(Action::Menu));
    let selected = app.game_list.selected();
    assert!(app.handle(Action::Menu).is_none());
    assert_eq!(app.screen, Screen::Browse);
    repeater.set_hold_shortcuts(app.available_hold_shortcuts());
    assert!(repeater.tick(wake + Duration::from_secs(2)).is_empty());
    assert_eq!(
        repeater.release(Action::Menu, wake + Duration::from_secs(2)),
        None
    );
    assert_eq!(
        app.game_list.selected(),
        selected,
        "wake must not choose another game"
    );
    app.ui.hide().unwrap();
}

/// Open Actions over the selected game and accept one entry of its Game
/// group, the way a controller reaches it.
fn accept_game_action(app: &mut App, action: &str) {
    assert_eq!(app.screen, Screen::Browse);
    app.handle(Action::Context);
    assert_eq!(app.screen, Screen::Context);
    assert_eq!(
        app.menu.first().map(String::as_str),
        Some(ContextPage::Game.label())
    );
    app.menu_list.select(0);
    app.handle(Action::Accept);
    let index = app
        .menu
        .iter()
        .position(|entry| entry == action)
        .unwrap_or_else(|| panic!("{action} is offered: {:?}", app.menu));
    app.menu_list.select(index);
    app.handle(Action::Accept);
}

fn select_row_named(app: &mut App, name: &str) {
    let index = app
        .here
        .iter()
        .position(|row| row.name == name)
        .unwrap_or_else(|| panic!("{name} is listed: {:?}", app.here));
    app.game_list.select(index);
}

fn run_last_played_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("last-played");
    let games = root.join("games/NES");
    let favorites_root = root.join("_@Favorites");
    let media = games.join("media");
    std::fs::create_dir_all(&media).unwrap();
    std::fs::create_dir(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NES.rbf"), b"fixture core").unwrap();
    let first = games.join("First Game.nes");
    let second = games.join("Second Game.nes");
    let missing = games.join("Missing Game.nes");
    std::fs::write(&first, b"fixture game").unwrap();
    std::fs::write(&second, b"fixture game").unwrap();
    std::fs::write(media.join("first.png"), b"fixture image").unwrap();
    std::fs::write(media.join("second.png"), b"fixture image").unwrap();
    std::fs::write(
        games.join("gamelist.xml"),
        "<gameList>\
         <game><path>First Game.nes</path><name>Current First</name><image>./media/first.png</image><genre>Puzzle</genre><publisher>Current Publisher</publisher></game>\
         <game><path>Second Game.nes</path><name>Current Second</name><image>./media/second.png</image></game>\
         </gameList>",
    )
    .unwrap();

    // Existing installations are unchanged: no category and no history
    // update is attached to a successful launch while the option is absent.
    let mut app = fixture_app(&root, window.clone(), Settings::default());
    assert!(app
        .categories
        .iter()
        .all(|(name, _)| name != LAST_PLAYED_CATEGORY));
    select_row_named(&mut app, "Current First");
    let Some(Outcome::Launch { history, .. }) = app.confirm_launch() else {
        panic!("the ordinary fixture game launches: {:?}", app.message);
    };
    assert!(history.is_none());
    assert!(!crate::history::path_beside(&app.settings_path).exists());
    app.ui.hide().unwrap();
    drop(app);

    // Retained order is newest first. Recorded names are only fallbacks:
    // current cache presentation must replace them when a target resolves.
    let history_path = crate::history::path_beside(&root.join("settings.toml"));
    let mut history = crate::history::History::default();
    history.remember(crate::history::Entry {
        system: "NES".into(),
        launch: browse::Launch::File(first.clone()),
        name: "Recorded First".into(),
    });
    history.remember(crate::history::Entry {
        system: "NES".into(),
        launch: browse::Launch::File(missing.clone()),
        name: "Recorded Missing".into(),
    });
    history.remember(crate::history::Entry {
        system: "NES".into(),
        launch: browse::Launch::File(second.clone()),
        name: "Recorded Second".into(),
    });
    history.save(&history_path).unwrap();

    let mut app = unopened_fixture_app(
        &root,
        window.clone(),
        Settings {
            last_played: Some(2),
            show_cores: Some(true),
            ..Settings::default()
        },
    );
    app.leave_splash();
    app.finish_background_work_for_headless();
    app.core_catalogue = crate::systems::CoreIndex::read(&root).catalogue(&app.table);
    std::fs::create_dir_all(&favorites_root).unwrap();
    let mut favorites = crate::systems::parse_table(
        include_str!("../assets/systems.toml"),
        Path::new("systems.toml"),
    )
    .unwrap()
    .into_iter()
    .find(|system| system.id == "Favorites")
    .unwrap();
    favorites.folders = vec![favorites_root.to_string_lossy().into_owned()];
    app.all_systems.push(FoundSystem {
        def: favorites,
        paths: vec![favorites_root.clone()],
        logo_dir: None,
        menu_folder: None,
    });
    app.rebuild_system_list();
    let categories: Vec<&str> = app
        .categories
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    let recent_at = categories
        .iter()
        .position(|name| *name == LAST_PLAYED_CATEGORY)
        .unwrap();
    let cores_at = categories
        .iter()
        .position(|name| *name == CORES_CATEGORY)
        .unwrap();
    let favorites_at = categories
        .iter()
        .position(|name| is_favorites(name))
        .unwrap();
    assert_eq!(cores_at + 1, recent_at);
    assert_eq!(recent_at + 1, favorites_at);
    assert_eq!(app.categories[recent_at].1, 2);
    assert!(
        app.last_played_rows.is_empty(),
        "building Home previews does not populate the open collection's row map"
    );
    assert_eq!(
        app.category_picks.get(LAST_PLAYED_CATEGORY),
        Some(&media.join("second.png")),
        "the newest resolvable current cover previews the collection"
    );

    app.category_list.select(recent_at);
    app.open_selected_category();
    assert!(app.last_played_open);
    assert_eq!(app.here_label(), LAST_PLAYED_CATEGORY);
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Current Second", "Recorded Missing"]
    );
    assert_eq!(
        app.here[0].cover.as_deref(),
        Some(media.join("second.png").as_path())
    );
    assert!(app.message.is_none());

    // A background rebuild can replace the current cache while this mixed
    // collection is open. Its visible rows and launch map must move to the
    // same completed cache together rather than requiring a leave/re-enter.
    let original_gamelist = std::fs::read_to_string(games.join("gamelist.xml")).unwrap();
    std::fs::write(
        games.join("gamelist.xml"),
        original_gamelist.replace("Current Second", "Rebuilt Second"),
    )
    .unwrap();
    app.start_build(true);
    app.finish_background_work_for_headless();
    assert_eq!(app.here[0].name, "Rebuilt Second");
    assert!(app.selected_last_played().is_some());
    std::fs::write(games.join("gamelist.xml"), &original_gamelist).unwrap();
    app.start_build(true);
    app.finish_background_work_for_headless();
    assert_eq!(app.here[0].name, "Current Second");
    assert!(app.selected_last_played().is_some());

    // The collection is one stable place. It has game-list actions, but no
    // action that pretends the mixed collection is a single system.
    let place = app.current_view_place().unwrap();
    assert_eq!(
        place,
        ViewPlace::Games {
            system: LAST_PLAYED_SYSTEM.into(),
            place: LAST_PLAYED_PLACE.into(),
        }
    );
    app.open_context();
    for absent in [
        REBUILD_SYSTEM,
        GAME_DATA_SOURCE,
        CORE_VERSION,
        SCRAPE_SYSTEM,
    ] {
        assert!(
            !app.context_actions.iter().any(|action| action == absent),
            "{absent} does not apply to a mixed-system collection"
        );
    }
    for present in [
        GAME_INFORMATION,
        ADD_FAVORITE,
        SEARCH,
        JUMP,
        FILTER_GAMES,
        CHANGE_VIEW,
    ] {
        assert!(
            app.context_actions.iter().any(|action| action == present),
            "{present} remains useful in Last Played"
        );
    }
    app.handle(Action::Quit);

    // Raising and lowering the visible amount changes the open list at once
    // without deleting the retained third entry.
    app.adjust_option_value(OptionId::LastPlayed, 1);
    assert_eq!(app.option_value(OptionId::LastPlayed), "3");
    assert_eq!(app.here.len(), 3);
    assert_eq!(app.here[2].name, "Current First");
    assert_eq!(app.here[2].genre.as_deref(), Some("Puzzle"));
    assert_eq!(app.here[2].details.publisher, "Current Publisher");
    app.adjust_option_value(OptionId::LastPlayed, -1);
    assert_eq!(app.here.len(), 2);
    assert_eq!(
        crate::history::History::load(&history_path)
            .unwrap()
            .entries
            .len(),
        3
    );
    app.adjust_option_value(OptionId::LastPlayed, 1);

    // Refreshing the collection while a search is active restores the
    // selected history entry in the filtered rows, not the first match.
    app.search_for("CURRENT");
    select_row_named(&mut app, "Current First");
    app.relist_here();
    assert_eq!(
        app.here[app.game_list.selected()].name,
        "Current First",
        "the selected filtered history entry survives a refresh"
    );
    app.clear_filter();

    app.layout = app.layout.next();
    app.remember_view();
    assert_eq!(
        place.get(&app.settings.custom_views),
        Some(app.layout.label())
    );
    assert_eq!(app.settings.custom_views.games.len(), 1);
    assert!(
        app.settings
            .custom_views
            .games
            .contains_key(LAST_PLAYED_SYSTEM),
        "the custom view belongs only to Last Played"
    );
    app.search_for("FIRST");
    assert_eq!(app.here.len(), 1);
    assert_eq!(app.here[0].name, "Current First");
    app.clear_filter();
    app.game_filters.choose(
        GameFilterField::Genre,
        &GameFilterChoice::Known {
            label: "Puzzle".into(),
            key: "puzzle".into(),
        },
    );
    app.apply_filter();
    assert_eq!(app.here.len(), 1);
    assert_eq!(app.here[0].name, "Current First");
    app.adjust_option_value(OptionId::LastPlayed, -1);
    assert!(app.here.is_empty());
    assert!(app.game_filters.is_active());
    app.adjust_option_value(OptionId::LastPlayed, 1);
    assert_eq!(app.here.len(), 1);
    assert_eq!(app.here[0].name, "Current First");
    app.clear_game_filters();

    select_row_named(&mut app, "Recorded Missing");
    let before_failure = std::fs::read(&history_path).unwrap();
    assert!(app.confirm_launch().is_none());
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("file is gone")));
    assert_eq!(std::fs::read(&history_path).unwrap(), before_failure);
    app.handle(Action::Quit);

    // Launch planning records the originating system and exact target, then
    // the durable history update moves an existing entry rather than adding
    // a duplicate. The actual FIFO-before-history boundary is exercised on
    // MiSTer in the physical test plan.
    select_row_named(&mut app, "Current First");
    let Some(Outcome::Launch {
        history: Some(update),
        ..
    }) = app.confirm_launch()
    else {
        panic!("a resolved Last Played row launches: {:?}", app.message);
    };
    assert_eq!(update.entry.system, "NES");
    assert_eq!(update.entry.launch, browse::Launch::File(first.clone()));
    crate::history::record(&update.path, update.entry).unwrap();
    let moved = crate::history::History::load(&history_path).unwrap();
    assert_eq!(moved.entries.len(), 3);
    assert_eq!(moved.entries[0].launch, browse::Launch::File(first.clone()));

    // Favourite state and writes use the original target rather than a
    // synthetic Last Played path.
    app.last_played = moved;
    app.relist_here();
    select_row_named(&mut app, "Current First");
    let information = app
        .information_request(&app.here[app.game_list.selected()])
        .unwrap();
    assert_eq!(information.launch, browse::Launch::File(first.clone()));
    assert!(matches!(
        information.source,
        crate::information_job::Source::Gamelist
    ));
    std::fs::create_dir_all(root.join("_Arcade")).unwrap();
    std::fs::write(root.join("_Arcade/LegacyNES.rbf"), b"alternate core").unwrap();
    app.all_systems
        .iter_mut()
        .find(|system| system.def.id == "NES")
        .unwrap()
        .def
        .compatible_cores
        .push(crate::config::CoreProfile {
            id: "legacy-nes".into(),
            label: "Legacy NES".into(),
            rbf: "_Arcade/LegacyNES".into(),
            setname: Some("LegacyNES".into()),
        });
    app.settings
        .launch_cores
        .insert("NES".into(), "legacy-nes".into());
    app.add_favorite_in(&favorites_root);
    assert!(app.favorites.holds(&first));
    let favorite = std::fs::read_to_string(favorites_root.join("Current First.mgl")).unwrap();
    assert!(
        favorite.contains("<rbf>_Arcade/LegacyNES</rbf>")
            && favorite.contains("<setname>LegacyNES</setname>"),
        "a Favourite created from Last Played uses its originating system's Launch Core: {favorite}"
    );
    assert!(app
        .here
        .iter()
        .any(|row| row.name == "Current First" && row.favorite));
    app.remove_favorite();
    assert!(!app.favorites.holds(&first));
    assert!(app
        .here
        .iter()
        .any(|row| row.name == "Current First" && !row.favorite));

    // The same game launched from the Favourites shelf resolves back to the
    // originating system and target, so it cannot create a second history
    // identity for the .mgl file on the shelf.
    select_row_named(&mut app, "Current First");
    app.add_favorite_in(&favorites_root);
    let favorites_at = app
        .systems
        .iter()
        .position(|system| system.def.id == "Favorites")
        .unwrap();
    app.open_system_by_index(favorites_at);
    assert!(app.in_favorites());
    select_row_named(&mut app, "Current First");
    let Some(Outcome::Launch {
        history: Some(from_favorite),
        ..
    }) = app.confirm_launch()
    else {
        panic!(
            "the favourite resolves to its original game: {:?}",
            app.message
        );
    };
    assert_eq!(from_favorite.entry.system, "NES");
    assert_eq!(
        from_favorite.entry.launch,
        browse::Launch::File(first.clone())
    );
    crate::history::record(&from_favorite.path, from_favorite.entry).unwrap();
    let after_favorite = crate::history::History::load(&history_path).unwrap();
    assert_eq!(after_favorite.entries.len(), 3);
    assert_eq!(
        after_favorite.entries[0].launch,
        browse::Launch::File(first.clone())
    );
    app.last_played = after_favorite;
    app.open_last_played();
    select_row_named(&mut app, "Current First");

    let saved_position = app.position();
    select_row_named(&mut app, "Current Second");
    app.ui.hide().unwrap();
    drop(app);

    let mut restarted = unopened_fixture_app(
        &root,
        window.clone(),
        Settings {
            last_played: Some(3),
            ..Settings::default()
        },
    );
    restarted.leave_splash();
    restarted.finish_background_work_for_headless();
    restarted.restore_position(&saved_position);
    assert!(restarted.last_played_open);
    assert_eq!(
        restarted.here[restarted.game_list.selected()].name,
        "Current First"
    );
    assert!(restarted.message.is_none(), "{:?}", restarted.message);
    restarted
        .game_filters
        .choose(GameFilterField::Genre, &GameFilterChoice::Unknown);
    restarted.apply_filter();
    restarted.handle(Action::Quit);
    assert_eq!(restarted.browsing, Browsing::Categories);
    assert!(!restarted.last_played_open);
    assert!(!restarted.game_filters.is_active());
    restarted.ui.hide().unwrap();
    drop(restarted);

    // A damaged history is reported, left byte-for-byte intact and never
    // attached to a launch, while ordinary browsing and launch planning work.
    std::fs::write(&history_path, "not = [valid").unwrap();
    let malformed = std::fs::read(&history_path).unwrap();
    let mut restarted = fixture_app(
        &root,
        window,
        Settings {
            last_played: Some(3),
            ..Settings::default()
        },
    );
    assert!(restarted.last_played_problem.is_some());
    select_row_named(&mut restarted, "Current First");
    let Some(Outcome::Launch { history, .. }) = restarted.confirm_launch() else {
        panic!("a malformed optional history must not block ordinary launches");
    };
    assert!(history.is_none());
    assert_eq!(std::fs::read(&history_path).unwrap(), malformed);
    restarted.ui.hide().unwrap();
}

fn run_main_favourites_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("main-favourites");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    std::fs::create_dir(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NES.rbf"), b"fixture core").unwrap();
    let game = root.join("games/NES/First Game.nes");
    std::fs::write(&game, b"fixture game").unwrap();
    std::fs::write(
        root.join("games/NES/gamelist.xml"),
        "<gameList><game><path>First Game.nes</path><name>First Game</name></game></gameList>",
    )
    .unwrap();
    // A ready-made descriptor is a core file to the favourites code: it is
    // linked to rather than described again.
    let core_file = root.join("games/NES/Core Game.mgl");
    std::fs::write(
        &core_file,
        "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"/media/fat/games/NES/Core Game.nes\"/></mistergamedescription>",
    )
    .unwrap();
    let favorites_root = root.join("_@Favorites");
    let mut app = fixture_app(&root, window, Settings::default());
    assert!(
        !favorites_root.exists(),
        "the flow starts on a card that has no favourites yet"
    );
    assert!(
        !app.all_systems
            .iter()
            .any(|system| crate::systems::is_favorites(system.category())),
        "a folder absent at startup is a system discovery never found"
    );

    // The chooser offers the root first even before the root exists, and
    // leaving it without choosing drops its rows.
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.screen, Screen::FavoriteFolder);
    assert_eq!(app.menu, vec![MAIN_FAVORITES, NEW_FOLDER]);
    assert_eq!(app.favorite_destinations[0], FavoriteDestination::Root);
    assert_eq!(app.menu_list.selected(), 0);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.favorite_destinations.is_empty());
    assert!(!favorites_root.exists(), "looking does not make the folder");

    // The first favourite makes the root and goes straight into it. With
    // no Favorites system discovered there is no shelf to refresh, and
    // the save stays press-free, the way a folder save on such a card has
    // always been: the shelf is listed by the next full rebuild.
    let root_favorite = favorites_root.join("First Game.mgl");
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.menu_list.selected(), 0);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.favorite_destinations.is_empty());
    assert!(std::fs::symlink_metadata(&root_favorite).unwrap().is_file());
    assert!(app.favorites.holds(&game));
    assert!(app
        .here
        .iter()
        .any(|row| row.name == "First Game" && row.favorite));
    assert!(
        app.message.is_none(),
        "a save with no shelf to refresh costs no press: {:?}",
        app.message
    );
    assert!(
        crate::cache::load_system(&app.cache_dir, "Favorites").is_none(),
        "no shelf was discovered, so none was written"
    );
    assert!(app
        .systems
        .iter()
        .all(|system| system.def.id != "Favorites"));
    // The existing removal takes it away again, as press-free as before.
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, REMOVE_FAVORITE);
    assert_eq!(app.screen, Screen::Browse);
    assert!(!root_favorite.exists());
    assert!(!app.favorites.holds(&game));
    assert!(
        favorites_root.is_dir(),
        "removing a favourite keeps the root"
    );
    assert!(app.message.is_none(), "{:?}", app.message);

    // The master shelf, declared the way the shipped table declares it and
    // pointed at this card's root: what the full rebuild discovers now the
    // folder is there, so the refresh after a write has a system to
    // rewrite.
    let mut favorites = crate::systems::parse_table(
        include_str!("../assets/systems.toml"),
        Path::new("systems.toml"),
    )
    .unwrap()
    .into_iter()
    .find(|system| system.id == "Favorites")
    .unwrap();
    favorites.folders = vec![favorites_root.to_string_lossy().into_owned()];
    app.all_systems.push(FoundSystem {
        def: favorites,
        paths: vec![favorites_root.clone()],
        logo_dir: None,
        menu_folder: None,
    });

    // Choosing the root writes the .mgl straight into _@Favorites and
    // makes no folder named after the row.
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.screen, Screen::FavoriteFolder);
    assert_eq!(app.menu, vec![MAIN_FAVORITES, NEW_FOLDER]);
    assert_eq!(app.menu_list.selected(), 0);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(
        std::fs::symlink_metadata(&root_favorite).unwrap().is_file(),
        "a game favourite in the root is a plain .mgl"
    );
    assert!(std::fs::read_to_string(&root_favorite)
        .unwrap()
        .contains("<mistergamedescription>"));
    assert!(
        !favorites_root.join(MAIN_FAVORITES).exists(),
        "Main Favourites is the root, not a folder"
    );
    assert!(app.favorites.holds(&game));
    assert!(app
        .here
        .iter()
        .any(|row| row.name == "First Game" && row.favorite));
    // The refresh rewrote the shelf's cache with the root-level file, and
    // said nothing: a shelf listed live would look the same on screen while
    // a failed refresh went unreported.
    assert!(app.message.is_none(), "{:?}", app.message);
    let shelf_cache = crate::cache::load_system(&app.cache_dir, "Favorites")
        .expect("the Favorites cache is written by the refresh");
    assert!(
        shelf_cache
            .folders
            .values()
            .flat_map(|folder| &folder.rows)
            .any(|row| {
                matches!(
                    &row.kind,
                    browse::Kind::Play(browse::Launch::File(path)) if path == &root_favorite
                )
            }),
        "the refreshed cache lists the root-level favourite: {:?}",
        shelf_cache.folders
    );

    // The shelf shows it after the existing refresh, and the existing
    // removal there takes a root-level favourite away.
    app.open_system_by_index(1);
    assert_eq!(app.open_system.as_deref(), Some("Favorites"));
    assert!(app.in_favorites());
    assert!(
        app.here.iter().any(|row| {
            row.name == "First Game"
                && matches!(
                    &row.kind,
                    browse::Kind::Play(browse::Launch::File(path)) if path == &root_favorite
                )
        }),
        "the shelf lists the root-level favourite: {:?}",
        app.here
    );
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, REMOVE_FAVORITE);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(!root_favorite.exists());
    assert!(!app.here.iter().any(|row| row.name == "First Game"));
    assert!(!app.favorites.holds(&game));

    // The held-X shortcut reaches the same chooser, and a core file kept
    // in the root is the standard link.
    app.open_system_by_index(0);
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.hold_shortcuts[HoldButton::X.index()] = HoldShortcut::AddRemoveFavourite;
    app.settings
        .set_hold_shortcut(HoldButton::X, HoldShortcut::AddRemoveFavourite);
    select_row_named(&mut app, "Core Game");
    assert_eq!(app.favorite_change(), Some(FavoriteChange::Add));
    app.handle(Action::HoldShortcut(HoldShortcut::AddRemoveFavourite));
    assert_eq!(app.screen, Screen::FavoriteFolder);
    assert_eq!(app.menu.first().map(String::as_str), Some(MAIN_FAVORITES));
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    let root_link = favorites_root.join("Core Game.mgl");
    assert!(std::fs::symlink_metadata(&root_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&root_link).unwrap(), core_file);
    assert!(app.favorites.holds(&core_file));
    assert!(app
        .here
        .iter()
        .any(|row| row.name == "Core Game" && row.favorite));

    // A root-level favourite made outside Degauss, for another file but
    // under this game's name, is not this game's favourite, so the game is
    // still offered Add to Favourites. The write is refused by name, and
    // the refusal is shown rather than lost to the redraw that follows it.
    std::fs::write(
        &root_favorite,
        "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"/media/fat/games/NES/Other Game.nes\"/></mistergamedescription>",
    )
    .unwrap();
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.screen, Screen::FavoriteFolder);
    assert_eq!(app.menu_list.selected(), 0);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    let refusal = app.message.clone().expect("a failed write is reported");
    assert!(
        refusal.contains("already has a favourite called First Game"),
        "the real cause is shown: {refusal}"
    );
    assert!(
        !app.favorites.holds(&game),
        "a refused write does not mark the game"
    );
    // The next press dismisses the message and is spent on that.
    app.handle(Action::Quit);
    assert!(app.message.is_none());
    assert_eq!(app.screen, Screen::Browse);

    // When the redraw after the refusal cannot list the folder either,
    // both causes are shown: the refusal alone would leave the emptied
    // list looking like an empty folder once it was dismissed.
    let place = app.trail.last().unwrap().place.clone();
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.screen, Screen::FavoriteFolder);
    app.trail.last_mut().unwrap().place = Place::Dir(root.join("games/Gone"));
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    let both = app.message.clone().expect("both failures are reported");
    assert!(
        both.contains("already has a favourite called First Game"),
        "the refusal comes first: {both}"
    );
    assert!(
        both.contains("reading folder"),
        "the listing failure is kept under it: {both}"
    );
    assert!(both.find("already has").unwrap() < both.find("reading folder").unwrap());
    assert!(app.here.is_empty());
    app.handle(Action::Quit);
    assert!(app.message.is_none());
    app.trail.last_mut().unwrap().place = place;
    app.relist_here();
    assert_eq!(app.here.len(), 2);
    std::fs::remove_file(&root_favorite).unwrap();

    // A folder really called Main Favourites shows the same words as the
    // root and stays a destination of its own, chosen by row rather than
    // by label.
    let named_folder = favorites_root.join(MAIN_FAVORITES);
    std::fs::create_dir(&named_folder).unwrap();
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.screen, Screen::FavoriteFolder);
    assert_eq!(app.menu, vec![MAIN_FAVORITES, MAIN_FAVORITES, NEW_FOLDER]);
    assert_eq!(
        app.favorite_destinations,
        vec![
            FavoriteDestination::Root,
            FavoriteDestination::Folder(MAIN_FAVORITES.to_string()),
            FavoriteDestination::NewFolder,
        ]
    );
    app.menu_list.select(1);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(named_folder.join("First Game.mgl").is_file());
    assert!(
        !root_favorite.exists(),
        "the folder's row must not write into the root"
    );
    assert!(app.favorites.holds(&game));
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, REMOVE_FAVORITE);
    assert!(!named_folder.join("First Game.mgl").exists());
    assert!(!app.favorites.holds(&game));
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    assert_eq!(app.menu.len(), 3);
    app.menu_list.select(0);
    app.handle(Action::Accept);
    assert!(root_favorite.is_file());
    assert!(
        !named_folder.join("First Game.mgl").exists(),
        "the root's row must not write into the folder of the same name"
    );
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, REMOVE_FAVORITE);
    assert!(!root_favorite.exists());
    assert!(!app.favorites.holds(&game));

    // Naming a new folder is still the last row and still makes the folder
    // before writing into it.
    select_row_named(&mut app, "First Game");
    accept_game_action(&mut app, ADD_FAVORITE);
    app.menu_list.select(app.menu.len() - 1);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::NameKeyboard);
    assert_eq!(app.name_keyboard_draft, "_");
    app.handle(Action::Context);
    assert!(
        app.name_keyboard_draft.is_empty(),
        "X removes the native-menu prefix"
    );
    app.name_keyboard_draft = "Arcade!".into();
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert!(favorites_root.join("Arcade!/First Game.mgl").is_file());
    assert!(
        !favorites_root.join("First Game.mgl").exists(),
        "the new folder's row must not write into the root"
    );
    assert!(app.favorites.holds(&game));

    // A real folder selected inside Favourites can be renamed in place.
    // Only the Favourites cache changes and the renamed row remains selected.
    app.open_system_by_index(1);
    assert_eq!(app.open_system.as_deref(), Some("Favorites"));
    select_row_named(&mut app, "Arcade!");
    let nes_cache = std::fs::read(crate::cache::system_path(&app.cache_dir, "NES")).unwrap();
    accept_game_action(&mut app, RENAME_FAVORITE_FOLDER);
    assert_eq!(app.screen, Screen::NameKeyboard);
    assert_eq!(app.name_keyboard_draft, "Arcade!");
    assert_eq!(app.name_keyboard_page, NamePage::Lower);
    app.handle(Action::Menu);
    assert_eq!(app.name_keyboard_page, NamePage::Upper);
    app.handle(Action::Menu);
    assert_eq!(app.name_keyboard_page, NamePage::Symbols);
    app.handle(Action::Menu);
    assert_eq!(app.name_keyboard_page, NamePage::Lower);
    app.name_keyboard_draft = "Renamed Folder!".into();
    app.handle(Action::Quit);
    let renamed = favorites_root.join("Renamed Folder!");
    assert_eq!(app.screen, Screen::Browse);
    assert!(!favorites_root.join("Arcade!").exists());
    assert!(renamed.join("First Game.mgl").is_file());
    assert_eq!(app.here[app.game_list.selected()].name, "Renamed Folder!");
    assert!(
        app.build.is_none(),
        "rename must not start a full-library build"
    );
    assert_eq!(
        std::fs::read(crate::cache::system_path(&app.cache_dir, "NES")).unwrap(),
        nes_cache,
        "rename must not rewrite another system cache"
    );

    // Delete first honours cancellation, then removes an empty directory.
    // Empty folders are deliberately absent under the default browse option,
    // so expose them through the existing setting before selecting this one.
    app.show_empty = true;
    app.relist_here();
    select_row_named(&mut app, MAIN_FAVORITES);
    accept_game_action(&mut app, DELETE_FAVORITE_FOLDER);
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains(MAIN_FAVORITES)));
    app.handle(Action::Quit);
    assert!(named_folder.is_dir(), "cancel keeps the empty folder");
    app.handle(Action::Quit);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    accept_game_action(&mut app, DELETE_FAVORITE_FOLDER);
    app.handle(Action::Accept);
    assert!(!named_folder.exists());
    assert!(!app.here.iter().any(|row| row.name == MAIN_FAVORITES));
    assert!(
        app.build.is_none(),
        "delete must not start a full-library build"
    );

    // A non-empty folder reaches the same named confirmation but the final
    // non-recursive filesystem operation refuses it and keeps every entry.
    select_row_named(&mut app, "Renamed Folder!");
    accept_game_action(&mut app, DELETE_FAVORITE_FOLDER);
    app.handle(Action::Accept);
    assert!(renamed.is_dir());
    assert!(renamed.join("First Game.mgl").is_file());
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("not empty")));
    app.handle(Action::Quit);
    app.ui.hide().unwrap();
}

fn run_scraper_cache_refresh_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("scraper-refresh");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(games.join(name), b"fixture game").unwrap();
    }
    let image = games.join("cover.png");
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos");
    std::fs::copy(assets.join("NES.png"), &image).unwrap();
    let metadata = games.join("gamelist.xml");
    std::fs::write(&metadata, "<gameList><game><path>First Game.nes</path><desc>Before scrape</desc><image>cover.png</image></game></gameList>").unwrap();
    let mut app = fixture_app(&root, window, Settings::default());
    let original_image = app.covers.get(&image).unwrap().rgb.clone();
    let original_gallery = app.gallery_covers.get(&image).unwrap().rgb.clone();
    let original_cache = cache_snapshot(&app.cache_dir);
    std::fs::copy(assets.join("SNES.png"), &image).unwrap();
    std::fs::write(&metadata, "<gameList><game><path>First Game.nes</path><desc>Updated description</desc><publisher>Updated Publisher</publisher><image>cover.png</image></game></gameList>").unwrap();

    let begin = |app: &mut App| {
        app.screen = Screen::ScraperProgress;
        app.scraper_return = Screen::Browse;
        app.scraper_details = false;
        app.scraper_terminal = None;
        app.scraper_progress = crate::scraper::Progress {
            phase: crate::scraper::Phase::Finishing,
            scope: "Fixture System".into(),
            total: 1,
            completed: 1,
            updated: 1,
            updated_systems: vec!["NES".into()],
            ..Default::default()
        };
        app.begin_scraper_finish(ScraperTerminal::Finished);
        assert!(app.scraper_cache_refresh_active);
        let id = app.refreshing.take().unwrap();
        app.start_scraper_cache_refresh(id);
    };
    let finish = |app: &mut App| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.scraper_refresh_job.is_some() {
            assert!(
                Instant::now() < deadline,
                "real cache refresh worker did not finish"
            );
            app.poll_scraper_cache_refresh();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.scraper_cache_refresh_active);
        assert!(app.scraper_pending_terminal.is_none());
    };
    begin(&mut app);
    assert!(
        app.scraper_refresh_job.is_some(),
        "refresh uses the actual worker"
    );
    app.hold_shortcuts[HoldButton::Y.index()] = HoldShortcut::RandomGame;
    assert!(app.available_hold_shortcuts()[HoldButton::Y.index()].is_none());
    capture_live_if_requested(&mut app, "scraper-refresh-real-worker");
    app.handle(Action::Accept);
    assert!(app.scraper_details);
    capture_live_if_requested(&mut app, "scraper-refresh-real-worker-details");
    app.handle(Action::Quit);
    assert!(!app.scraper_details);
    app.handle(Action::Quit);
    assert_eq!(
        app.screen,
        Screen::ScraperProgress,
        "Back cannot leave before refresh has completed"
    );
    assert!(app.scraper_pending_terminal.is_some());
    finish(&mut app);
    assert_eq!(app.scraper_progress_rows().len(), SCRAPER_PROGRESS_ROWS);
    assert_eq!(app.scraper_terminal, Some(ScraperTerminal::Finished));
    assert_eq!(app.scraper_progress.system_errors, 0);
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 2);
    let updated = app
        .here
        .iter()
        .find(|row| row_target(row).as_deref() == Some(games.join("First Game.nes").as_path()))
        .unwrap();
    assert_eq!(updated.details.desc, "Updated description");
    assert_eq!(updated.details.publisher, "Updated Publisher");
    assert_eq!(updated.cover.as_deref(), Some(image.as_path()));
    assert_ne!(cache_snapshot(&app.cache_dir), original_cache);
    let written = crate::cache::load_system(&app.cache_dir, "NES").unwrap();
    assert!(written
        .folders
        .values()
        .flat_map(|folder| &folder.rows)
        .any(|row| row.details.desc == "Updated description"));
    assert_eq!(
        crate::cache::load_index(&app.cache_dir).unwrap().systems,
        app.index.as_ref().unwrap().systems
    );
    assert_ne!(
        app.covers.get(&image).unwrap().rgb,
        original_image,
        "same-path artwork replacement must invalidate the decoded cache"
    );
    assert_ne!(
        app.gallery_covers.get(&image).unwrap().rgb,
        original_gallery
    );
    capture_live_if_requested(&mut app, "scraper-refresh-real-complete");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    app.load_art();

    begin(&mut app);
    finish(&mut app);
    assert_eq!(app.scraper_progress.system_errors, 0, "refresh is reusable");

    // A member the refresh leaves out is the scrape's last problem, named
    // by the system, the archive and the reason the way a rebuild names
    // it, and written to the log. The list was replaced, so it is not a
    // failed refresh: the dashboard would otherwise count a system whose
    // list is there as a failed one and call the refresh failed.
    let outer = games.join("Outer.zip");
    std::fs::write(
        &outer,
        crate::zip::tests_archive(&["Inner Game.nes", "inner.zip"], false),
    )
    .unwrap();
    let before_outer = cache_snapshot(&app.cache_dir);
    let log_since = |from: usize| {
        let log = std::fs::read_to_string(crate::LOG_PATH).unwrap_or_default();
        log[from.min(log.len())..].to_string()
    };
    let log_before = log_since(0).len();
    begin(&mut app);
    finish(&mut app);
    assert_eq!(app.scraper_terminal, Some(ScraperTerminal::Finished));
    assert_eq!(
        app.scraper_progress.system_errors, 0,
        "a skipped member is not a failed refresh"
    );
    let problem = format!(
        "NES: {}: 1 member skipped: nested archive member is unsupported by MiSTer Main",
        outer.display()
    );
    assert_eq!(
        app.scraper_progress.last_problem.as_deref(),
        Some(problem.as_str())
    );
    assert!(
        log_since(log_before).lines().any(|line| line == problem),
        "no line {problem:?} in the log"
    );
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 3);
    assert_ne!(
        cache_snapshot(&app.cache_dir),
        before_outer,
        "the list holding the retained member replaced the previous one"
    );
    let published = crate::cache::load_system(&app.cache_dir, "NES").unwrap();
    assert_eq!(
        published.folders[&Place::Archive(outer.clone()).key()]
            .rows
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Inner Game.nes"],
        "the inner archive is not a row"
    );
    std::fs::remove_file(&outer).unwrap();
    begin(&mut app);
    finish(&mut app);
    assert_eq!(app.scraper_progress.system_errors, 0);
    assert_eq!(app.scraper_progress.last_problem, None);
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 2);
    let stable_cache = cache_snapshot(&app.cache_dir);
    let stable_index = app.index.as_ref().unwrap().systems.clone();
    std::fs::rename(&games, root.join("games/NES-moved")).unwrap();
    begin(&mut app);
    assert!(
        app.scraper_refresh_job.is_none(),
        "a missing source must fail before starting the worker"
    );
    assert_eq!(app.scraper_progress.system_errors, 1);
    assert!(app
        .scraper_progress
        .last_problem
        .as_deref()
        .unwrap()
        .contains("no folder for NES is on the card"));
    assert_eq!(cache_snapshot(&app.cache_dir), stable_cache);
    assert_eq!(app.index.as_ref().unwrap().systems, stable_index);
    capture_live_if_requested(&mut app, "scraper-refresh-missing-root");

    let blocked = root.join("not-a-directory");
    std::fs::write(&blocked, b"fixture file").unwrap();
    app.all_systems[0].def.folders = vec![blocked.join("NES").to_string_lossy().into_owned()];
    begin(&mut app);
    assert!(app.scraper_refresh_job.is_none());
    assert_eq!(app.scraper_progress.system_errors, 1);
    let cause = app.scraper_progress.last_problem.as_deref().unwrap();
    assert!(cause.contains("checking system folder") && cause.contains("not-a-directory"));
    assert_eq!(cache_snapshot(&app.cache_dir), stable_cache);
    assert_eq!(app.index.as_ref().unwrap().systems, stable_index);
    capture_live_if_requested(&mut app, "scraper-refresh-unreadable-root");
    app.ui.hide().unwrap();
}

fn run_scraper_unresolved_report_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    // A batch never stops for a game it cannot write, so the report after
    // the run is the only place the user can learn which games those were.
    let mut app = fixture_app(root, window, Settings::default());
    app.screen = Screen::ScraperProgress;
    app.scraper_return = Screen::Browse;
    app.scraper_details = true;
    app.scraper_terminal = None;
    app.handle(Action::End);
    assert_eq!(
        app.scraper_progress_list.selected(),
        SCRAPER_PROGRESS_ROWS - 1
    );
    app.scraper_progress = crate::scraper::Progress {
        phase: crate::scraper::Phase::Finishing,
        scope: "Fixture System".into(),
        total: 3,
        completed: 3,
        unchanged: 1,
        not_found: 1,
        failed: 1,
        unresolved_games: vec![
            crate::scraper::UnresolvedGame {
                label: "Fixture System: Missing Game".into(),
                reason: "no match".into(),
            },
            crate::scraper::UnresolvedGame {
                label: "Fixture System: Rejected Game".into(),
                reason: "ScreenScraper rejected this request".into(),
            },
        ],
        ..Default::default()
    };
    app.begin_scraper_finish(ScraperTerminal::Finished);
    assert!(
        !app.scraper_cache_refresh_active,
        "nothing was written, so no list refresh runs"
    );
    assert_eq!(app.scraper_terminal, Some(ScraperTerminal::Finished));
    assert_eq!(app.scraper_progress_row_count(), SCRAPER_PROGRESS_ROWS + 3);
    assert_eq!(
        app.scraper_progress_list.count(),
        SCRAPER_PROGRESS_ROWS + 3,
        "the report can scroll to every unresolved game"
    );
    assert_eq!(
        app.scraper_progress_list.selected(),
        SCRAPER_PROGRESS_ROWS - 1,
        "a Details view read during the run is not snapped back to the top when the run ends"
    );
    assert_eq!(app.scraper_progress_rows().len(), SCRAPER_PROGRESS_ROWS);
    app.handle(Action::Quit);
    assert!(!app.scraper_details);
    capture_live_if_requested(&mut app, "scraper-unresolved-overview");
    app.handle(Action::Accept);
    assert!(app.scraper_details);
    app.handle(Action::End);
    assert_eq!(
        app.scraper_progress_list.selected(),
        SCRAPER_PROGRESS_ROWS + 2
    );
    app.refresh();
    let selected = app.rows.row_data(app.ui.get_selected() as usize).unwrap();
    assert_eq!(selected.title, "Fixture System: Rejected Game");
    assert_eq!(selected.value, "ScreenScraper rejected this request");
    capture_live_if_requested(&mut app, "scraper-unresolved-details-end");
    app.handle(Action::Up);
    app.handle(Action::Up);
    app.refresh();
    let header = app.rows.row_data(app.ui.get_selected() as usize).unwrap();
    assert_eq!(header.title, "Unresolved games");
    assert_eq!(header.value, "2");
    capture_live_if_requested(&mut app, "scraper-unresolved-details-header");
    app.handle(Action::Quit);
    assert!(!app.scraper_details);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert!(
        app.scraper_progress.unresolved_games.is_empty(),
        "the closed report cannot be reopened, so its list is released rather than kept until the next scrape"
    );

    let job = crate::scraper::start(crate::scraper::Request {
        scope: crate::scraper::Scope::All,
        scope_label: "All Systems".into(),
        systems: Vec::new(),
        names: browse::DisplayNames::default(),
        settings: crate::scraper::ScraperSettings::default(),
        developer: None,
        artwork_pack_system_ids: std::collections::HashSet::new(),
    })
    .unwrap();
    app.scraper_scope = crate::scraper::Scope::All;
    app.begin_scraper_job(job, false);
    assert!(
        app.scraper_progress.unresolved_games.is_empty(),
        "a new job starts with an empty report"
    );
    assert_eq!(
        app.scraper_progress_list.count(),
        SCRAPER_PROGRESS_ROWS,
        "a new job resets the report to its fixed rows"
    );
    app.scraper_job = None;
    app.ui.hide().unwrap();
}

fn run_scraper_image_choice_flow(app: &mut App) {
    use crate::scraper::{Scope, ScraperSettings};
    let original = app.scraper_settings.clone();
    app.open_scraper(Scope::All, Screen::Browse);
    let image_row = scraper_rows(&app.scraper_scope)
        .iter()
        .position(|row| *row == ScraperRow::ImageType)
        .unwrap();
    app.scraper_list.select(image_row);
    assert_eq!(app.scraper_value(ScraperRow::ImageType), "Screenshot");
    app.handle(Action::Faster);
    assert_eq!(app.scraper_settings.media_type, "box-2D");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    app.open_scraper(
        Scope::System {
            system_id: "Arcade".into(),
            place: Place::Roots,
            display_name: "Arcade".into(),
        },
        Screen::Browse,
    );
    app.scraper_list.select(image_row);
    assert_eq!(app.scraper_value(ScraperRow::ImageType), "Use Global");
    assert_eq!(app.scraper_search_settings().media_type, "box-2D");
    app.handle(Action::Accept);
    assert_eq!(app.scraper_value(ScraperRow::ImageType), "Screenshot");
    assert_eq!(app.scraper_settings.media_type_for("arcade"), "ss");
    assert_eq!(app.scraper_settings.media_type_for("NES"), "box-2D");
    assert_eq!(app.scraper_search_settings().media_type, "ss");
    app.handle(Action::Quit);
    app.scraper_settings = ScraperSettings::load(&app.scraper_settings_path).unwrap();
    assert_eq!(app.scraper_settings.media_type_for("Arcade"), "ss");
    for scope in [
        Scope::Folder {
            system_id: "NES".into(),
            place: Place::Roots,
            display_name: "Folder".into(),
        },
        Scope::Game {
            system_id: "NES".into(),
            launch: browse::Launch::File(PathBuf::from("Example.nes")),
            title: "Example".into(),
        },
    ] {
        app.open_scraper(scope, Screen::Browse);
        app.scraper_list.select(image_row);
        assert_eq!(app.scraper_search_settings().media_type, "box-2D");
        app.handle(Action::Slower);
        assert_eq!(app.scraper_search_settings().media_type, "box-3D");
        assert_eq!(app.scraper_settings.media_type_for("Arcade"), "ss");
        app.handle(Action::Faster);
        assert_eq!(app.scraper_value(ScraperRow::ImageType), "Use Global");
        app.handle(Action::Quit);
    }
    assert_eq!(
        ScraperSettings::load(&app.scraper_settings_path)
            .unwrap()
            .media_type_override("NES"),
        None
    );
    assert!(app.replace_scraper_settings(original));
    assert!(app.scraper_job.is_none());
    assert!(app.scraper_search_job.is_none());
}

fn run_manual_search_ui_flow(app: &mut App, root: &Path) {
    use crate::scraper::{ImagePolicy, MetadataPolicy};

    let original_settings = app.scraper_settings.clone();
    let original_path = app.scraper_settings_path.clone();
    let original_bytes = std::fs::read(&original_path).unwrap();
    app.reopen_context_for(SCRAPE_GAME);
    let action_row = app
        .menu
        .iter()
        .position(|entry| entry == SCRAPE_GAME)
        .unwrap();
    app.menu_list.select(action_row);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scraper);
    let crate::scraper::Scope::Game { title, .. } = &app.scraper_scope else {
        panic!("Scrape This Game must open single-game settings");
    };
    assert_eq!(&app.scraper_search_term, title);
    assert!(!app.scraper_manual_resolution_eligible);
    assert_eq!(app.scraper_progress.completed, 0);
    assert_eq!(app.scraper_progress.not_found, 0);
    assert_eq!(app.scraper_progress.ambiguous, 0);
    app.scraper_search_term.clear();
    let search_row = scraper_rows(&app.scraper_scope)
        .iter()
        .position(|row| *row == ScraperRow::Search)
        .unwrap();
    app.scraper_list.select(search_row);
    let speed = app.speed;
    for action in [Action::Slower, Action::Faster] {
        app.handle(action);
        assert_eq!(app.screen, Screen::Scraper);
        assert_eq!(app.scraper_list.selected(), search_row);
        assert_eq!(app.speed, speed);
        assert!(app.scraper_settings == original_settings);
        assert!(app.message.is_none());
        assert!(app.pending.is_none());
        assert!(app.scraper_job.is_none());
        assert!(app.scraper_search_job.is_none());
    }

    for (username, password) in [("", ""), ("fixture-user", ""), ("", "fixture-password")] {
        app.scraper_settings.username = username.into();
        app.scraper_settings.password = password.into();
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::Scraper);
        assert_eq!(
            app.message.as_deref(),
            Some("Set the ScreenScraper username and password first.")
        );
        assert!(app.scraper_search_job.is_none());
        app.handle(Action::Quit);
    }
    app.scraper_settings.username = "fixture-user".into();
    app.scraper_settings.password = "fixture-password".into();
    app.scraper_settings.image_policy = ImagePolicy::Off;
    app.scraper_settings.metadata_policy = MetadataPolicy::Off;
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scraper);
    assert_eq!(
        app.message.as_deref(),
        Some("Enable images, metadata, or both first.")
    );
    assert!(app.scraper_search_job.is_none());
    app.handle(Action::Quit);
    app.scraper_settings.image_policy = ImagePolicy::MissingOnly;
    app.scraper_settings.metadata_policy = MetadataPolicy::FillMissing;
    app.scraper_settings.accepted_plaintext_warning = false;
    app.handle(Action::Accept);
    assert_eq!(app.pending, Some(Pending::AcceptScraperStorageForSearch));
    app.handle(Action::Quit);
    assert!(app.pending.is_none());
    assert!(!app.scraper_settings.accepted_plaintext_warning);
    assert_eq!(std::fs::read(&original_path).unwrap(), original_bytes);

    let blocked = root.join("manual-search-blocked");
    std::fs::write(&blocked, b"fixture file, not a settings directory").unwrap();
    app.handle(Action::Accept);
    assert_eq!(app.pending, Some(Pending::AcceptScraperStorageForSearch));
    app.scraper_settings_path = blocked.join("screenscraper.toml");
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scraper);
    assert!(!app.scraper_settings.accepted_plaintext_warning);
    assert!(app
        .message
        .as_ref()
        .is_some_and(|message| message.contains("manual-search-blocked")));
    assert!(app.scraper_search_job.is_none());
    assert_eq!(std::fs::read(&original_path).unwrap(), original_bytes);
    app.handle(Action::Quit);
    app.scraper_settings_path = original_path.clone();
    app.handle(Action::Accept);
    assert_eq!(app.pending, Some(Pending::AcceptScraperStorageForSearch));
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::ScraperMatches);
    assert_eq!(app.scraper_matches_return, Screen::Scraper);
    assert!(app.scraper_settings.accepted_plaintext_warning);
    assert_eq!(app.scraper_search_status, "Enter a game title");
    assert!(app.scraper_job.is_none());
    assert!(app.scraper_search_job.is_none());
    assert!(app.scraper_preview_job.is_none());
    assert_eq!(app.scraper_progress.completed, 0);
    assert!(!app.scraper_manual_resolution_eligible);
    app.apply_geometry();
    assert_eq!(app.ui.get_plain_help().as_str(), "B Back   X Search");
    let saved = crate::scraper::ScraperSettings::load(&original_path).unwrap();
    assert!(saved == app.scraper_settings);

    let original_scope = app.scraper_scope.clone();
    app.scraper_scope = crate::scraper::Scope::All;
    app.open_scraper_keyboard(ScraperField::SearchTerm);
    app.scraper_keyboard_draft = "  Adventure Island: 日本語  ".into();
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::ScraperMatches);
    assert_eq!(app.scraper_search_term, "Adventure Island: 日本語");
    assert!(app.scraper_search_job.is_none());
    app.scraper_scope = original_scope;
    app.scraper_search_term.clear();

    app.set_scraper_matches(vec![crate::scraper::Match {
        id: "fixture-match".into(),
        name: "Fixture Match".into(),
        names: vec!["Fixture Match".into()],
        metadata: Default::default(),
        media: None,
        rom_crc32: None,
        rom_md5: None,
        rom_sha1: None,
    }]);
    assert_eq!(app.scraper_search_status, "1 Match");
    app.apply_geometry();
    assert_eq!(
        app.ui.get_plain_help().as_str(),
        "A Use   B Back   X Search"
    );
    app.handle(Action::Accept);
    assert!(matches!(app.pending, Some(Pending::UseScraperMatch(_))));
    assert!(app
        .message
        .as_ref()
        .is_some_and(|message| message.contains("A Use, B Cancel")));
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::ScraperMatches);
    assert!(app.scraper_job.is_none());
    assert!(
        app.scraper_settings == saved,
        "manual selection must not override write policies"
    );
    app.scraper_preview_image = Some(crate::covers::RgbImage::new(1, 1, vec![255, 0, 0]).unwrap());
    app.load_art();
    assert!(
        app.ui.get_has_art(),
        "manual previews ignore the browse artwork preference"
    );
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Scraper);
    assert_eq!(app.scraper_list.selected(), search_row);
    assert!(
        !app.ui.get_has_art(),
        "leaving the picker must clear the shared preview"
    );
    let cleared_image = app.ui.get_art().size();
    assert_eq!((cleared_image.width, cleared_image.height), (0, 0));
    assert!(app.scraper_preview_image.is_none());
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Context);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Context);
    assert!(app.context_is_root());
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    app.load_art();
    assert!(
        !app.ui.get_has_art(),
        "hidden browse artwork must not retain the preview"
    );
    app.reopen_context_for(SCRAPE_GAME);
    app.menu_list.select(action_row);
    app.handle(Action::Accept);
    app.scraper_search_term.clear();
    app.scraper_list.select(search_row);

    app.scraper_progress.completed = 1;
    app.scraper_progress.updated = 1;
    app.scraper_progress.not_found = 0;
    app.scraper_progress.ambiguous = 0;
    app.scraper_manual_resolution_eligible = false;
    app.handle(Action::Accept);
    assert_eq!(
        app.screen,
        Screen::ScraperMatches,
        "manual search remains available after a successful automatic scrape"
    );
    assert_eq!(app.scraper_matches_return, Screen::Scraper);
    assert_eq!(app.scraper_search_status, "Enter a game title");
    assert!(app.scraper_job.is_none());
    assert!(app.scraper_search_job.is_none());
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Scraper);
    assert_eq!(app.scraper_list.selected(), search_row);

    app.scraper_settings_path = blocked.join("screenscraper.toml");
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scraper);
    assert!(app
        .message
        .as_ref()
        .is_some_and(|message| message.contains("manual-search-blocked")));
    assert!(app.scraper_search_job.is_none());
    app.handle(Action::Quit);
    app.scraper_settings_path = original_path;
    app.open_scraper_matches(Vec::new());
    assert_eq!(app.scraper_matches_return, Screen::ScraperProgress);
    app.handle(Action::Quit);
    assert_eq!(
        app.screen,
        Screen::ScraperProgress,
        "automatic unresolved matches retain their original return path"
    );
    app.close_scraper_progress();
    assert_eq!(app.screen, Screen::Context);
    assert_eq!(app.menu_list.selected(), action_row);
    assert!(app.replace_scraper_settings(original_settings));
}

fn capture_frame(app: &mut App, directory: &Path, name: &str, width: u32, height: u32) {
    capture_frame_mode(app, directory, name, width, height, false);
}

fn capture_live_frame(app: &mut App, directory: &Path, name: &str, width: u32, height: u32) {
    capture_frame_mode(app, directory, name, width, height, true);
}

fn capture_frame_mode(
    app: &mut App,
    directory: &Path,
    name: &str,
    width: u32,
    height: u32,
    live: bool,
) {
    let path = directory.join(format!("fixture-{width}x{height}-{name}.bmp"));
    assert!(
        !path.exists(),
        "capture must not overwrite {}",
        path.display()
    );
    app.width = width;
    app.height = height;
    app.window.set_size(slint::PhysicalSize::new(width, height));
    app.apply_geometry();
    app.ui.show().unwrap();
    app.window.request_redraw();
    let mut surface =
        crate::surface::MemorySurface::new(width, height, crate::surface::PixelFormat::Rgb565);
    let mut presenter = Presenter::new(surface.geometry(), PresentMode::Direct);
    presenter.force_repaint(&app.window);
    if live {
        app.refresh();
        assert!(presenter.draw(&app.window, &mut surface).unwrap().is_some());
        surface.present().unwrap();
    } else {
        app.render_once(&mut surface, &mut presenter).unwrap();
    }
    assert!(
        surface.presents > 0,
        "capture must render actual Slint frames"
    );
    surface.write_bmp(&path).unwrap();
    let quote = |text: &str| {
        let mut escaped = String::from("\"");
        for character in text.chars() {
            match character {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                character if character.is_control() => {
                    escaped.push_str(&format!("\\u{:04x}", character as u32))
                }
                character => escaped.push(character),
            }
        }
        escaped.push('"');
        escaped
    };
    let option = if matches!(app.screen, Screen::Options | Screen::Advanced) {
        app.option_ids()
            .get(app.active_list().selected())
            .map(|id| format!("{id:?}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let action = if app.screen == Screen::Context {
        app.menu
            .get(app.menu_list.selected())
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    };
    let theme = app
        .active_theme
        .map(|index| app.themes[index].name.as_str())
        .unwrap_or("Standard");
    let record = format!("{{\"file\":{},\"scenario\":{},\"screen\":{},\"optionId\":{},\"actionId\":{},\"selected\":{},\"font\":{},\"theme\":{},\"scope\":{},\"help\":{},\"width\":{width},\"height\":{height}}}\n", quote(path.file_name().unwrap().to_str().unwrap()), quote(name), quote(&format!("{:?}", app.screen)), quote(&option), quote(&action), app.active_list().selected(), quote(app.font.label()), quote(theme), quote(app.ui.get_plain_scope().as_str()), quote(app.ui.get_status().as_str()));
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("captures.jsonl"))
        .unwrap()
        .write_all(record.as_bytes())
        .unwrap();
}

fn capture_every_menu_row(app: &mut App, directory: &Path) {
    let full = ContextActions {
        scrape_scope: true,
        scrape_game: true,
        image_override: None,
        game_data_source: true,
        launch_core: true,
        core_version: true,
        core_version_override: true,
        favorite_folder: false,
        metadata_filters: true,
        rebuild_system: true,
    };
    let scenarios = [
        (
            "game",
            Browsing::Games,
            true,
            Some(false),
            Some(false),
            true,
            full,
        ),
        (
            "favourite-hidden",
            Browsing::Games,
            false,
            Some(true),
            Some(true),
            false,
            ContextActions {
                core_version: true,
                ..ContextActions::default()
            },
        ),
        (
            "folder",
            Browsing::Games,
            false,
            None,
            Some(false),
            false,
            ContextActions {
                scrape_game: false,
                ..full
            },
        ),
        (
            "zip-folder",
            Browsing::Games,
            false,
            None,
            Some(false),
            true,
            ContextActions {
                scrape_game: false,
                ..full
            },
        ),
        (
            "pack",
            Browsing::Games,
            false,
            Some(false),
            Some(false),
            false,
            ContextActions {
                scrape_scope: false,
                scrape_game: false,
                ..full
            },
        ),
        (
            "empty",
            Browsing::Games,
            false,
            None,
            None,
            false,
            ContextActions::default(),
        ),
        (
            "systems",
            Browsing::Systems,
            true,
            None,
            Some(false),
            true,
            ContextActions {
                scrape_game: false,
                image_override: Some(true),
                ..full
            },
        ),
        (
            "home-custom",
            Browsing::Categories,
            false,
            None,
            None,
            true,
            ContextActions {
                image_override: Some(true),
                ..ContextActions::default()
            },
        ),
        (
            "home-global",
            Browsing::Categories,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        ),
    ];
    for font in Font::ALL {
        while app.font != font {
            app.adjust_option_value(OptionId::Font, 1);
        }
        let font_name = font.label().replace(' ', "-");
        app.set_screen(Screen::OptionsRoot);
        for page in OptionsPage::ALL {
            app.select(page.index());
            capture_frame(
                app,
                directory,
                &format!("row-{font_name}-options-root-{}", page.key()),
                352,
                240,
            );
            assert_eq!(app.ui.get_status().as_str(), page.help());
        }
        for page in OptionsPage::ALL {
            app.open_options_page(page);
            for (index, option) in page.ids().iter().enumerate() {
                app.select(index);
                capture_frame(
                    app,
                    directory,
                    &format!("row-{font_name}-options-{}-{option:?}", page.key()),
                    352,
                    240,
                );
                assert_eq!(app.ui.get_status().as_str(), option.help());
            }
        }
        for (scenario, browsing, searching, favorite, hidden, custom, flags) in scenarios {
            app.browsing = browsing;
            app.context_actions =
                context_entries(browsing, searching, favorite, hidden, custom, flags);
            app.context_root_selection = 0;
            app.context_page_selections = [0; 4];
            app.show_context_page(None);
            if browsing == Browsing::Categories {
                for index in 0..app.menu.len() {
                    app.select(index);
                    capture_frame(
                        app,
                        directory,
                        &format!("row-{font_name}-actions-{scenario}-{index:02}"),
                        352,
                        240,
                    );
                    assert_eq!(app.ui.get_status().as_str(), context_help(&app.menu[index]));
                }
                continue;
            }
            let pages = ContextPage::ALL
                .into_iter()
                .filter(|page| {
                    app.context_actions
                        .iter()
                        .any(|action| page.contains(action))
                })
                .collect::<Vec<_>>();
            for page in pages {
                app.show_context_page(None);
                app.select(
                    app.menu
                        .iter()
                        .position(|label| label == page.label())
                        .unwrap(),
                );
                capture_frame(
                    app,
                    directory,
                    &format!("row-{font_name}-actions-{scenario}-group-{}", page.label()),
                    352,
                    240,
                );
                app.handle(Action::Accept);
                assert_eq!(app.context_page, Some(page));
                for index in 0..app.menu.len() {
                    app.select(index);
                    capture_frame(
                        app,
                        directory,
                        &format!(
                            "row-{font_name}-actions-{scenario}-{}-{index:02}",
                            page.label()
                        ),
                        352,
                        240,
                    );
                    assert_eq!(app.ui.get_status().as_str(), context_help(&app.menu[index]));
                }
                app.handle(Action::Quit);
                assert!(app.context_is_root());
                assert_eq!(app.menu[app.menu_list.selected()], page.label());
            }
        }
        app.browsing = Browsing::Games;
        for (scope_name, scope) in [
            ("all", crate::scraper::Scope::All),
            (
                "system",
                crate::scraper::Scope::System {
                    system_id: "NES".into(),
                    place: Place::Roots,
                    display_name: "Fixture System".into(),
                },
            ),
            (
                "folder",
                crate::scraper::Scope::Folder {
                    system_id: "NES".into(),
                    place: Place::Roots,
                    display_name: "Fixture Folder".into(),
                },
            ),
            (
                "game",
                crate::scraper::Scope::Game {
                    system_id: "NES".into(),
                    launch: browse::Launch::File(PathBuf::from("fixture.nes")),
                    title: "Fixture Game".into(),
                },
            ),
        ] {
            app.open_scraper(scope, Screen::Browse);
            for (index, row) in scraper_rows(&app.scraper_scope).iter().enumerate() {
                app.select(index);
                assert!(!app.scraper_selected_help().is_empty());
                capture_frame(
                    app,
                    directory,
                    &format!("row-{font_name}-scraper-{scope_name}-{row:?}"),
                    352,
                    240,
                );
                assert_eq!(app.ui.get_status().as_str(), app.scraper_selected_help());
            }
        }
    }
    app.browsing = Browsing::Games;
}

fn capture_scraper_progress_states(app: &mut App, directory: &Path) {
    for scenario in [
        "account",
        "enumerating",
        "planning",
        "running",
        "zero-complete",
        "complete",
        "cancelled",
        "failed",
        "refreshing",
    ] {
        app.screen = Screen::ScraperProgress;
        app.scraper_details = false;
        app.scraper_terminal = None;
        app.scraper_pending_terminal = None;
        app.scraper_cancelling = false;
        app.scraper_progress = crate::scraper::Progress {
            phase: crate::scraper::Phase::Scraping,
            scope: "Fixture System".into(),
            current: "Fixture Game".into(),
            total: 10,
            completed: 4,
            updated: 2,
            unchanged: 1,
            not_found: 1,
            ambiguous: 1,
            no_media: 1,
            failed: 1,
            unsupported_systems: 1,
            skipped_artwork_pack: 1,
            system_errors: 1,
            ambiguous_targets: 1,
            ..Default::default()
        };
        match scenario {
            "account" | "enumerating" => {
                app.scraper_progress.phase = if scenario == "account" {
                    crate::scraper::Phase::Account
                } else {
                    crate::scraper::Phase::Enumerating
                };
                app.scraper_progress.total = 0;
                app.scraper_progress.completed = 0;
                app.scraper_progress.current.clear();
            }
            "zero-complete" => {
                app.scraper_progress = crate::scraper::Progress {
                    phase: crate::scraper::Phase::Finishing,
                    scope: "Fixture System".into(),
                    ..Default::default()
                };
                app.scraper_terminal = Some(ScraperTerminal::Finished);
            }
            "planning" => {
                app.scraper_progress.phase = crate::scraper::Phase::Planning;
                app.scraper_progress.completed = 0;
            }
            "complete" => {
                app.scraper_progress.completed = 10;
                app.scraper_terminal = Some(ScraperTerminal::Finished);
            }
            "cancelled" => app.scraper_terminal = Some(ScraperTerminal::Cancelled),
            "failed" => app.scraper_terminal = Some(ScraperTerminal::Failed(
                "Fixture timeout: the request did not complete. Check the connection and retry."
                    .into(),
            )),
            "refreshing" => {
                app.scraper_pending_terminal = Some(ScraperTerminal::Finished);
                app.scraper_refresh_folder = "Fixture Folder".into();
                app.scraper_refresh_folders = 4;
                app.scraper_refresh_games = 10;
            }
            "running" => {}
            _ => unreachable!(),
        }
        if matches!(scenario, "running" | "complete" | "cancelled") {
            app.scraper_progress.deduplicated_aliases = 2;
        }
        assert_eq!(app.scraper_progress_rows().len(), SCRAPER_PROGRESS_ROWS);
        capture_live_frame(
            app,
            directory,
            &format!("scraper-{scenario}-overview"),
            352,
            240,
        );
        assert_eq!(app.ui.get_operation_kind(), 2);
        assert!(!app.ui.get_operation_details());
        assert_eq!(app.ui.get_operation_subject(), app.scraper_scope_label());
        if app.scraper_progress.deduplicated_aliases > 0 {
            assert_eq!(app.ui.get_operation_note(), "Linked Copies Skipped: 2");
            assert!(app
                .scraper_progress_rows()
                .contains(&("Linked copies".into(), "2".into())));
        }
        if scenario == "planning" {
            assert_eq!(app.ui.get_operation_state(), "Checking existing data");
            assert!(app.ui.get_operation_determinate());
            assert_eq!(app.ui.get_operation_progress(), "0 / 10 Games");
        }
        assert!(
            app.ui.get_operation_activity().is_empty()
                || app.ui.get_operation_activity() != app.ui.get_operation_subject(),
            "single-game and refresh dashboards must not repeat their subject"
        );
        if scenario == "failed" {
            assert_eq!(app.ui.get_operation_state(), "Failed");
        }
        if matches!(
            scenario,
            "account" | "enumerating" | "zero-complete" | "refreshing"
        ) {
            assert!(
                !app.ui.get_operation_determinate(),
                "unknown totals must not invent a percentage"
            );
        }
        app.handle(Action::Accept);
        assert!(app.scraper_details);
        for index in 0..SCRAPER_PROGRESS_ROWS {
            app.scraper_progress_list.select(index);
            capture_live_frame(
                app,
                directory,
                &format!("scraper-{scenario}-details-{index:02}"),
                352,
                240,
            );
        }
        app.handle(Action::Quit);
        assert!(!app.scraper_details);
        assert_eq!(
            app.screen,
            Screen::ScraperProgress,
            "Back returns from Details to the overview"
        );
    }
    app.screen = Screen::ScraperProgress;
    app.scraper_details = true;
    app.scraper_pending_terminal = None;
    app.scraper_terminal = None;
    app.scraper_progress.scope =
        "Fixture Collection / A Very Long System Folder / Final Scope Name".into();
    app.scraper_progress_list.go_first();
    app.ui.set_marquee_end(true);
    app.handle(Action::Down);
    assert_eq!(app.scraper_progress_list.selected(), 1);
    assert!(
        !app.ui.get_marquee_end(),
        "changing report rows restarts reading from the beginning"
    );
    capture_live_frame(app, directory, "scraper-long-report-start", 352, 240);
    let selected = app.rows.row_data(app.ui.get_selected() as usize).unwrap();
    assert_eq!(selected.value, app.scraper_progress.scope);
    app.marquee.stop();
    app.ui.set_marquee_end(true);
    capture_live_frame(app, directory, "scraper-long-report-moving", 352, 240);
    std::thread::sleep(Duration::from_millis(1500));
    slint::platform::update_timers_and_animations();
    capture_live_frame(app, directory, "scraper-long-report-end", 352, 240);
    app.scraper_pending_terminal = None;
    app.scraper_terminal = None;
    app.scraper_details = false;
    app.screen = Screen::Browse;
}

fn paint_index_frame(app: &mut App) {
    app.ui.show().unwrap();
    app.refresh();
    let mut surface =
        crate::surface::MemorySurface::new(352, 240, crate::surface::PixelFormat::Rgb565);
    let mut presenter = Presenter::new(surface.geometry(), PresentMode::Direct);
    presenter.force_repaint(&app.window);
    assert!(presenter.draw(&app.window, &mut surface).unwrap().is_some());
    surface.present().unwrap();
    assert!(surface.presents > 0);
    app.index_frame_presented();
}

fn capture_live_if_requested(app: &mut App, name: &str) {
    if let Some(directory) = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR") {
        capture_live_frame(app, Path::new(&directory), name, 352, 240);
    }
}

fn finish_discovery(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app
        .build
        .as_ref()
        .is_some_and(|build| build.discovery.is_some())
        || app.source_resolution.is_some()
    {
        assert!(
            Instant::now() < deadline,
            "fixture discovery did not finish"
        );
        app.build_one_system();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn cache_snapshot(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(cache_snapshot(&entry.path()));
        } else {
            files.push((entry.path(), std::fs::read(entry.path()).unwrap()));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn run_indexing_ui_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("indexing");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let mut app = fixture_app(&root, window, Settings::default());
    select_option(&mut app, OptionsPage::Library, OptionId::RebuildCache);
    app.refresh();
    let initial_cache = cache_snapshot(&app.cache_dir);
    let original_open = app.open_system.clone();
    let original_table = app.table.clone();

    app.rebuild_all_systems();
    let original_start = app.build.as_ref().unwrap().started;
    app.rebuild_all_systems();
    assert_eq!(app.build.as_ref().unwrap().started, original_start);
    app.build_one_system();
    assert!(
        matches!(
            app.build.as_ref().unwrap().discovery,
            Some(Discovery::Queued(_))
        ),
        "discovery must wait for a real Preparing frame"
    );
    assert_eq!(app.ui.get_operation_state(), "Preparing");
    capture_live_if_requested(&mut app, "index-all-preparing");
    app.handle(Action::Accept);
    assert!(app.index_details);
    capture_live_if_requested(&mut app, "index-all-preparing-details");
    app.handle(Action::Quit);
    assert!(!app.index_details);
    assert!(
        !app.build.as_ref().unwrap().cancelling,
        "Back from Details does not cancel"
    );
    assert!(!app.ui.get_bottom_bar_visible());
    app.handle(Action::Quit);
    app.build_one_system();
    assert!(
        app.build.is_none(),
        "queued cancellation must not spawn a worker"
    );
    assert_eq!(app.open_system, original_open);
    assert_eq!(cache_snapshot(&app.cache_dir), initial_cache);
    capture_live_if_requested(&mut app, "index-all-cancelled-before-worker");

    app.message = None;
    app.rebuild_all_systems();
    paint_index_frame(&mut app);
    let detail_model = app.ui.get_detail_lines();
    let mut marker = app.rows.row_data(0).unwrap();
    marker.title = SharedString::from("Unchanged covered row");
    app.rows.set_row_data(0, marker);
    app.dirty = false;
    {
        let build = app.build.as_mut().unwrap();
        build.started = Instant::now();
        build.folders = 12;
        build.games = 34;
    }
    assert!(app.publish_index_progress());
    assert!(!app.dirty, "progress must not schedule a model rebuild");
    assert!(app.ui.get_detail_lines() == detail_model);
    assert_eq!(app.rows.row_data(0).unwrap().title, "Unchanged covered row");
    for _ in 0..100 {
        assert!(
            !app.publish_index_progress(),
            "unchanged idle progress must not reformat"
        );
    }
    app.build.as_mut().unwrap().started -= Duration::from_secs(2);
    assert!(
        app.publish_index_progress(),
        "a new displayed second updates the overlay"
    );
    assert!(!app.dirty);
    assert!(app.ui.get_detail_lines() == detail_model);
    assert_eq!(app.rows.row_data(0).unwrap().title, "Unchanged covered row");
    app.build_one_system();
    assert!(matches!(
        app.build.as_ref().unwrap().discovery,
        Some(Discovery::Running(_))
    ));
    assert!(
        !app.dirty,
        "dispatching an already painted phase needs no extra frame"
    );
    capture_live_if_requested(&mut app, "index-all-discovery-active");
    app.handle(Action::Quit);
    assert!(app.build.as_ref().unwrap().cancelling);
    capture_live_if_requested(&mut app, "index-all-cancelling");
    finish_discovery(&mut app);
    assert!(app.build.is_none());
    capture_live_if_requested(&mut app, "index-all-cancelled");
    assert_eq!(app.open_system, original_open);
    assert_eq!(cache_snapshot(&app.cache_dir), initial_cache);

    // An actionable discovery error must not replace the previous snapshot.
    let blocked = root.join("not-a-directory");
    std::fs::write(&blocked, b"fixture").unwrap();
    app.table[0].folders = vec![blocked.join("NES").to_string_lossy().into_owned()];
    app.rebuild_all_systems();
    paint_index_frame(&mut app);
    finish_discovery(&mut app);
    assert!(app.build.is_none());
    let failure = app.index_terminal.as_ref().unwrap();
    assert_eq!(failure.state, "Failed");
    assert!(
        failure.problem.contains("checking system folder")
            && failure.problem.contains("not-a-directory")
    );
    assert!(
        app.message.is_none(),
        "the failed overview owns the error until Details is requested"
    );
    capture_live_if_requested(&mut app, "index-all-discovery-failed");
    app.handle(Action::Accept);
    assert!(app.message.as_ref().unwrap().contains("not-a-directory"));
    capture_live_if_requested(&mut app, "index-all-discovery-failed-details");
    app.handle(Action::Quit);
    assert!(app.index_terminal.is_some());
    app.handle(Action::Quit);
    assert!(app.index_terminal.is_none());
    assert_eq!(app.open_system, original_open);
    assert_eq!(app.all_systems.len(), 1);
    assert_eq!(cache_snapshot(&app.cache_dir), initial_cache);
    app.table = original_table;

    // A successful read picks up new folders and core locations before scanning.
    std::fs::create_dir_all(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NES.rbf"), b"fixture core").unwrap();
    std::fs::write(root.join("_Console/Added.rbf"), b"fixture core").unwrap();
    std::fs::create_dir(root.join("games/Added")).unwrap();
    std::fs::write(root.join("games/Added/Added.nes"), b"fixture").unwrap();
    let mut added = app.table[0].clone();
    added.id = "Added".into();
    added.name = "Added System".into();
    added.rbf = "Added".into();
    added.folders = vec!["Added".into()];
    app.table.push(added);
    app.rebuild_all_systems();
    let started = Instant::now() - Duration::from_secs(2);
    app.build.as_mut().unwrap().started = started;
    paint_index_frame(&mut app);
    finish_discovery(&mut app);
    assert_eq!(app.build.as_ref().unwrap().started, started);
    assert_eq!(app.index_return_screen, Screen::Options);
    assert_eq!(app.screen, Screen::Options);
    assert_eq!(app.all_systems.len(), 2);
    assert_eq!(app.all_systems[0].menu_folder.as_deref(), Some("Console"));
    assert_eq!(app.all_systems[1].def.id, "Added");
    assert!(app.build.as_ref().unwrap().awaiting_frame);
    app.build_one_system();
    assert!(
        app.build.as_ref().unwrap().job.is_none(),
        "system name must paint before its scan"
    );
    capture_live_if_requested(&mut app, "index-all-system-before-worker");
    {
        let build = app.build.as_mut().unwrap();
        let original = (
            build.folders,
            build.games,
            build.done,
            build.total,
            build.folder.clone(),
        );
        build.folders = 23_456;
        build.games = 123_456;
        build.done = 75;
        build.total = 115;
        build.folder = "Fixture Arcade/Publisher Collections/A Long Directory Name".into();
        capture_live_if_requested(&mut app, "index-all-large-counter-layout-fixture");
        let build = app.build.as_mut().unwrap();
        (
            build.folders,
            build.games,
            build.done,
            build.total,
            build.folder,
        ) = original;
        app.dirty = true;
    }
    app.finish_background_work_for_headless();
    assert_eq!(app.index.as_ref().unwrap().systems["Added"].games, 1);
    assert_eq!(app.index_terminal.as_ref().unwrap().state, "Complete");
    capture_live_if_requested(&mut app, "index-all-complete");
    let completed = app.index_terminal.clone();
    app.index_terminal = Some(IndexOverview {
        title: "Index All Systems".into(),
        state: "Finished With Problems".into(),
        subject: "All Systems".into(),
        done: 115,
        total: 115,
        folders: 23_456,
        games: 123_456,
        elapsed: 219,
        determinate: true,
        problem: "Fixture System: /media/fat/games/Fixture/Fixture Archive.zip: skipped: zip archive is malformed: truncated central-directory header\nFixture System: /media/fat/games/Fixture/Fixture Set.zip: 2 members skipped: nested archive member is unsupported by MiSTer Main".into(),
        report: "Indexing finished\n115 / 115 systems processed in 219.0s\n23456 folders   123456 games read\n\nFixture System: /media/fat/games/Fixture/Fixture Archive.zip: skipped: zip archive is malformed: truncated central-directory header\nFixture System: /media/fat/games/Fixture/Fixture Set.zip: 2 members skipped: nested archive member is unsupported by MiSTer Main".into(),
        ..Default::default()
    });
    capture_live_if_requested(&mut app, "index-large-problems-layout-fixture");
    app.index_terminal = completed;
    app.handle(Action::Accept);
    capture_live_if_requested(&mut app, "index-all-complete-details");
    app.handle(Action::Quit);
    assert!(app.index_terminal.is_some());
    app.handle(Action::Quit);
    assert!(app.index_terminal.is_none());

    // A corrupt archive and an inner archive member cost only themselves:
    // the healthy games publish, the report says which system, archive and
    // reason, and neither can be reached as a row.
    let outer = root.join("games/NES/Outer.zip");
    std::fs::write(
        &outer,
        crate::zip::tests_archive(&["Inner Game.nes", "inner.zip"], false),
    )
    .unwrap();
    let broken = root.join("games/NES/Broken.zip");
    std::fs::write(&broken, b"not an archive").unwrap();
    app.rebuild_all_systems();
    paint_index_frame(&mut app);
    finish_discovery(&mut app);
    app.finish_background_work_for_headless();
    let problems = app.index_terminal.clone().unwrap();
    assert_eq!(problems.state, "Finished With Problems");
    assert!(
        problems.problem.contains(&format!(
            "NES: {}: skipped: zip archive is malformed: no valid end-of-directory record",
            broken.display()
        )),
        "{}",
        problems.problem
    );
    assert!(
        problems.problem.contains(&format!(
            "NES: {}: 1 member skipped: nested archive member is unsupported by MiSTer Main",
            outer.display()
        )),
        "{}",
        problems.problem
    );
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 3);
    assert_eq!(app.index.as_ref().unwrap().systems["Added"].games, 1);
    let published = crate::cache::load_system(&app.cache_dir, "NES").unwrap();
    assert_eq!(
        published.summary(&Place::Dir(root.join("games/NES"))).games,
        3
    );
    assert!(!published
        .folders
        .contains_key(&Place::Archive(broken.clone()).key()));
    capture_live_if_requested(&mut app, "index-all-finished-with-problems");
    app.handle(Action::Accept);
    assert!(app.message.as_ref().is_some_and(|report| {
        report.contains("Broken.zip")
            && report.contains("Outer.zip")
            && report.contains("nested archive member is unsupported by MiSTer Main")
    }));
    capture_live_if_requested(&mut app, "index-all-finished-with-problems-details");
    app.handle(Action::Quit);
    app.handle(Action::Quit);
    assert!(app.index_terminal.is_none());
    assert_eq!(app.open_system, original_open);
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Outer", "First Game.nes", "Second Game.nes"],
        "the rejected archive is not a row"
    );
    app.enter(Place::Archive(outer.clone()));
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Inner Game.nes"],
        "the inner archive is not a row"
    );
    assert!(app.leave());
    std::fs::remove_file(&outer).unwrap();
    std::fs::remove_file(&broken).unwrap();

    std::fs::remove_file(root.join("_Console/Added.rbf")).unwrap();
    std::fs::rename(root.join("games/NES"), root.join("games/NES-removed")).unwrap();
    app.rebuild_all_systems();
    paint_index_frame(&mut app);
    finish_discovery(&mut app);
    assert_eq!(app.all_systems.len(), 1);
    assert_eq!(app.all_systems[0].def.id, "Added");
    assert!(
        app.all_systems[0].menu_folder.is_none(),
        "removed cores are not retained"
    );
    assert!(app.open_system.is_none());
    assert!(app.trail.is_empty());
    assert_eq!(app.browsing, Browsing::Categories);
    app.finish_background_work_for_headless();
    assert!(!app.index.as_ref().unwrap().systems.contains_key("NES"));
    assert!(app
        .index_terminal
        .as_ref()
        .is_some_and(|view| view.problem.contains("no longer on the card")));
    app.ui.hide().unwrap();
}

fn capture_ui_if_requested(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let Some(directory) = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    assert!(
        directory.is_absolute() && directory.is_dir(),
        "DEGAUSS_UI_CAPTURE_DIR must name an existing absolute directory"
    );
    let directory = directory.canonicalize().unwrap();
    let mut app = fixture_app(root, window, Settings::default());
    let artwork = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos/NES.png");
    assert!(artwork.is_file());
    let mut rows: Vec<_> = (0..32)
        .map(|index| {
            let mut row = information_row();
            row.name = format!("Fixture Game {:02}", index + 1);
            row.sort_key = row.name.to_ascii_uppercase();
            row.kind =
                browse::Kind::Play(browse::Launch::File(root.join("games/NES/First Game.nes")));
            row.cover = (index % 5 != 0).then(|| artwork.clone());
            row.favorite = index % 4 == 0;
            row
        })
        .collect();
    rows.insert(
        0,
        browse::Row {
            name: "Fixture Folder".into(),
            sort_key: "FIXTURE FOLDER".into(),
            kind: browse::Kind::Enter(Place::Dir(root.join("games/NES"))),
            cover: None,
            genre: None,
            favorite: false,
            below: Some(32),
            details: browse::Details::default(),
        },
    );
    app.here = rows.clone();
    app.all_here = rows;
    app.game_list = ListState::new(app.here.len(), app.geometry.visible);
    app.game_list.select(5);
    app.show_bar = true;
    app.settings.show_bar = Some(true);
    app.browsing = Browsing::Categories;
    let original_categories = app.categories.clone();
    app.categories = ["Arcade", "Console", "Computer", "Favorites"]
        .into_iter()
        .map(|name| (name.to_string(), 1))
        .collect();
    app.category_list = ListState::new(app.categories.len(), app.geometry.visible);
    app.category_list.select(1);
    app.category_picks.insert("Console".into(), artwork.clone());
    app.touch_selection();
    for (width, height) in [(352, 240), (640, 480)] {
        for layout in Layout::ALL {
            app.set_screen(Screen::Browse);
            app.set_layout(layout);
            capture_frame(
                &mut app,
                &directory,
                &format!("home-{}", layout.label()),
                width,
                height,
            );
            assert!(app.ui.get_show_brand());
            assert_eq!(app.ui.get_heading_detail().as_str(), "Game Browser");
            assert_eq!(app.ui.get_group_title().as_str(), "Console");
            assert_eq!(app.ui.get_group_subtitle().as_str(), "Browse Systems");
            assert!(
                app.ui.get_heading().is_empty(),
                "the Home logo must not have a duplicate Degauss label"
            );
        }
    }
    app.categories = original_categories;
    app.category_list = ListState::new(app.categories.len(), app.geometry.visible);
    app.browsing = Browsing::Games;
    for (width, height) in [(352, 240), (640, 480)] {
        for layout in Layout::ALL {
            app.set_screen(Screen::Browse);
            app.set_layout(layout);
            assert_eq!(app.chrome_here(), layout != Layout::Gallery);
            capture_frame(&mut app, &directory, layout.label(), width, height);
            assert_eq!(app.ui.get_heading().as_str(), app.here_label());
        }
    }
    app.browsing = Browsing::Systems;
    app.open_category = Some("Console".into());
    app.set_layout(Layout::Details);
    capture_frame(&mut app, &directory, "systems", 352, 240);
    capture_frame(&mut app, &directory, "systems", 640, 480);
    assert_eq!(app.ui.get_heading().as_str(), "Console");
    app.browsing = Browsing::Games;
    app.game_list.select(6);
    for layout in [Layout::Details, Layout::Carousel] {
        app.set_layout(layout);
        app.touch_selection();
        capture_frame(
            &mut app,
            &directory,
            &format!("selected-missing-art-{}", layout.label()),
            352,
            240,
        );
    }
    app.game_list.select(5);
    app.show_bar = false;
    app.settings.show_bar = Some(false);
    app.set_screen(Screen::Browse);
    app.set_layout(Layout::Gallery);
    capture_frame(&mut app, &directory, "gallery-hidden-bar", 352, 240);
    app.show_bar = true;
    app.settings.show_bar = Some(true);
    app.set_layout(Layout::Details);

    app.set_screen(Screen::OptionsRoot);
    capture_frame(&mut app, &directory, "options-root", 352, 240);
    for page in OptionsPage::ALL {
        app.open_options_page(page);
        app.select(0);
        capture_frame(
            &mut app,
            &directory,
            &format!("options-{}", page.key()),
            352,
            240,
        );
        assert_eq!(
            app.ui.get_heading_detail().as_str(),
            format!("1/{}", page.ids().len())
        );
        assert_eq!(app.ui.get_plain_scope().as_str(), "Global Settings");
    }
    app.open_options_page(OptionsPage::Library);
    app.select(OptionsPage::Library.ids().len() - 1);
    capture_frame(&mut app, &directory, "options-library-last", 352, 240);

    app.set_screen(Screen::Browse);
    app.game_list.select(5);
    app.set_screen(Screen::Context);
    capture_frame(&mut app, &directory, "actions-game", 352, 240);
    assert!(app.ui.get_plain_help_height() > 0.0);
    assert_eq!(app.ui.get_status().as_str(), ContextPage::Game.help());
    assert_eq!(
        app.ui.get_heading_detail().as_str(),
        format!("1/{}", action_labels(&app.menu).len())
    );
    app.handle(Action::End);
    capture_frame(&mut app, &directory, "actions-game-last", 352, 240);
    app.set_screen(Screen::Browse);
    app.game_list.select(0);
    app.set_screen(Screen::Context);
    capture_frame(&mut app, &directory, "actions-folder", 352, 240);
    app.browsing = Browsing::Categories;
    app.set_screen(Screen::Context);
    capture_frame(&mut app, &directory, "actions-category", 352, 240);
    app.browsing = Browsing::Games;
    app.game_list.select(5);
    app.set_screen(Screen::Information);
    capture_frame(&mut app, &directory, "information-top", 352, 240);
    assert!(
        app.ui.get_information_max_scroll() > 0.0,
        "long fixture metadata must be scrollable"
    );
    app.handle(Action::PageDown);
    capture_frame(&mut app, &directory, "information-middle", 352, 240);
    app.handle(Action::End);
    assert!(app.ui.get_information_offset() > 0.0);
    capture_frame(&mut app, &directory, "information-bottom", 352, 240);

    app.set_screen(Screen::Browse);
    app.message = Some(format!(
        "Indexing failed\n{}Final actionable cause",
        "Detailed archive error.\n".repeat(30)
    ));
    capture_frame(&mut app, &directory, "long-error-top", 352, 240);
    app.handle(Action::End);
    capture_frame(&mut app, &directory, "long-error-bottom", 352, 240);
    app.handle(Action::Quit);

    app.set_screen(Screen::Help);
    capture_frame(&mut app, &directory, "help", 352, 240);
    app.open_options_page(OptionsPage::Appearance);
    app.set_screen(Screen::ThemeEditor);
    capture_frame(&mut app, &directory, "theme-editor", 352, 240);
    assert_eq!(
        app.ui.get_heading_detail().as_str(),
        app.theme_editor.as_ref().unwrap().source_name()
    );
    app.select(1);
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Picker);
    capture_frame(&mut app, &directory, "theme-editor-picker", 352, 240);
    app.handle(Action::Context);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Hex);
    capture_frame(&mut app, &directory, "theme-editor-hex", 352, 240);
    app.handle(Action::Quit);
    app.handle(Action::Context);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Swap);
    capture_frame(&mut app, &directory, "theme-editor-swap", 352, 240);
    app.handle(Action::Quit);
    app.select(14);
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Name);
    capture_frame(&mut app, &directory, "theme-editor-save", 352, 240);
    capture_frame(
        &mut app,
        &directory,
        "theme-editor-name-lowercase",
        352,
        240,
    );
    app.handle(Action::Menu);
    capture_frame(
        &mut app,
        &directory,
        "theme-editor-name-uppercase",
        352,
        240,
    );
    app.handle(Action::Menu);
    capture_frame(&mut app, &directory, "theme-editor-name-symbols", 352, 240);
    app.handle(Action::Quit);
    app.close_theme_editor();
    app.open_options_page(OptionsPage::Appearance);
    app.select(
        OptionsPage::Appearance
            .ids()
            .iter()
            .position(|id| *id == OptionId::ResetCustomViews)
            .unwrap(),
    );
    app.handle(Action::Accept);
    assert!(matches!(app.pending, Some(Pending::ResetCustomViews)));
    capture_frame(
        &mut app,
        &directory,
        "reset-custom-views-confirmation",
        352,
        240,
    );
    app.handle(Action::Quit);
    app.open_options_page(OptionsPage::Library);
    app.open_scraper(crate::scraper::Scope::All, Screen::Options);
    assert!(app.scraper_job.is_none());
    capture_frame(&mut app, &directory, "scraper-settings", 352, 240);
    let scope = app.scraper_scope_from_context(SCRAPE_GAME).unwrap();
    app.open_scraper(scope, Screen::Context);
    app.scraper_list.select(
        scraper_rows(&app.scraper_scope)
            .iter()
            .position(|row| *row == ScraperRow::Search)
            .unwrap(),
    );
    assert!(app.scraper_job.is_none());
    assert!(app.scraper_search_job.is_none());
    capture_frame(&mut app, &directory, "scraper-single-game", 352, 240);
    for field in [
        ScraperField::Username,
        ScraperField::Password,
        ScraperField::SearchTerm,
    ] {
        app.open_scraper_keyboard(field);
        for page in [
            ScraperKeyboardPage::Lower,
            ScraperKeyboardPage::Upper,
            ScraperKeyboardPage::Symbols,
        ] {
            capture_frame(
                &mut app,
                &directory,
                &format!("scraper-keyboard-{field:?}-{page:?}"),
                352,
                240,
            );
            app.cycle_scraper_keyboard();
        }
    }
    app.set_scraper_match_visual_fixture(
        "Fixture Game",
        vec![crate::scraper::Match {
            id: "fixture-match".into(),
            name: "Fixture Match".into(),
            names: vec!["Fixture Match".into()],
            metadata: Default::default(),
            media: None,
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        }],
        0,
        "One Match",
        None,
        "No Image Available",
    );
    capture_frame(&mut app, &directory, "scraper-manual-match", 352, 240);
    app.scraper_terminal = None;

    let pack_art = root.join("docs/NES/Artwork");
    std::fs::create_dir_all(&pack_art).unwrap();
    std::fs::write(
        pack_art.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nFixture\tbox-2D\t3\n",
    )
    .unwrap();
    std::fs::write(
        pack_art.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nFirst Game\t\t\tFixture\n",
    )
    .unwrap();
    std::fs::write(pack_art.join("gameinfo.tsv"), "#key\tname\tyear\tgenre\tdeveloper\tplayers\nFixture\tFixture Game\t1990\tPuzzle\tFixture Studio\t1\n").unwrap();
    std::fs::write(pack_art.join("Fixture.jpg"), crate::covers::JPEG_16).unwrap();
    assert!(
        crate::artwork_pack::Provider::load("NES", &root.join("docs"), None)
            .health
            .usable()
    );
    for (screen, name) in [
        (Screen::Menu, "main-menu"),
        (Screen::Scripts, "scripts"),
        (Screen::Advanced, "legacy-advanced"),
        (Screen::About, "about"),
        (Screen::Splash, "splash"),
        (Screen::Find, "jump-to-letter"),
        (Screen::GameDataSource, "game-data-source"),
        (Screen::ArtworkPackLocation, "artwork-pack-location"),
        (Screen::ArtworkPackDirectory, "artwork-pack-directory"),
        (Screen::SourceProgress, "source-progress"),
    ] {
        app.set_screen(Screen::Browse);
        app.set_screen(screen);
        assert_eq!(
            app.screen, screen,
            "{name} must be reached through its real entry path: {:?}",
            app.message
        );
        capture_frame(&mut app, &directory, name, 352, 240);
    }
    app.set_screen(Screen::Scripts);
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == "Example Script.sh")
            .unwrap(),
    );
    app.handle(Action::Accept);
    assert!(matches!(app.pending, Some(Pending::RunScript(_))));
    capture_frame(&mut app, &directory, "scripts-confirmation", 352, 240);
    capture_frame(&mut app, &directory, "scripts-confirmation", 640, 480);
    app.handle(Action::Quit);
    app.set_screen(Screen::Browse);
    app.search_for("FIXTURE");
    capture_frame(&mut app, &directory, "search", 352, 240);
    capture_frame(&mut app, &directory, "search", 640, 480);
    app.search_for("A VERY LONG SEARCH QUERY THAT MUST REMAIN READABLE");
    capture_frame(&mut app, &directory, "search-long-query", 352, 240);
    app.handle(Action::Menu);
    capture_frame(&mut app, &directory, "search-empty", 352, 240);
    app.search_for("FIXTURE");
    app.handle(Action::Quit);
    app.open_favorite_folders();
    assert_eq!(app.screen, Screen::FavoriteFolder);
    capture_frame(&mut app, &directory, "favorite-folder", 352, 240);
    app.handle(Action::Quit);
    app.open_name_keyboard(NamePurpose::NewFavoriteFolder, "_".to_string());
    capture_frame(&mut app, &directory, "new-favorite-folder", 352, 240);
    capture_frame(
        &mut app,
        &directory,
        "new-favorite-folder-lowercase",
        352,
        240,
    );
    app.handle(Action::Menu);
    capture_frame(
        &mut app,
        &directory,
        "new-favorite-folder-uppercase",
        352,
        240,
    );
    app.handle(Action::Menu);
    capture_frame(
        &mut app,
        &directory,
        "new-favorite-folder-symbols",
        352,
        240,
    );
    app.name_keyboard_draft.clear();
    app.set_screen(Screen::Browse);
    app.apply_filter();
    app.game_list.select(5);
    app.logo_dir = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos"));
    app.browsing = Browsing::Categories;
    app.set_screen(Screen::CategoryImage);
    assert_eq!(app.screen, Screen::CategoryImage);
    capture_frame(&mut app, &directory, "category-image", 352, 240);
    app.handle(Action::Quit);
    app.browsing = Browsing::Games;
    app.set_screen(Screen::Browse);
    app.saver_pool.push(SaverPicture {
        path: artwork.clone(),
        caption: "Fixture Game - NES".into(),
    });
    app.set_screen(Screen::Screensaver);
    assert_eq!(app.screen, Screen::Screensaver);
    capture_frame(&mut app, &directory, "screensaver", 352, 240);
    app.handle(Action::Quit);
    app.open_scraper(crate::scraper::Scope::All, Screen::Browse);
    app.set_screen(Screen::ScraperProgress);
    capture_frame(&mut app, &directory, "scraper-progress-empty", 352, 240);

    for font in Font::ALL {
        while app.font != font {
            app.adjust_option_value(OptionId::Font, 1);
        }
        let key = font.label().replace(' ', "-");
        app.set_screen(Screen::Browse);
        app.set_layout(Layout::Details);
        capture_frame(
            &mut app,
            &directory,
            &format!("font-{key}-details"),
            352,
            240,
        );
        app.open_options_page(OptionsPage::Appearance);
        app.select(
            OptionsPage::Appearance
                .ids()
                .iter()
                .position(|id| *id == OptionId::Font)
                .unwrap(),
        );
        capture_frame(
            &mut app,
            &directory,
            &format!("font-{key}-options"),
            352,
            240,
        );
    }
    app.themes = vec![
        Theme {
            name: "Amber".into(),
            file: crate::theme::ThemeFile::parse(include_str!("../assets/themes/Amber.toml"))
                .unwrap(),
        },
        Theme {
            name: "Mono".into(),
            file: crate::theme::ThemeFile::parse(include_str!("../assets/themes/Mono.toml"))
                .unwrap(),
        },
    ];
    for name in ["amber", "mono"] {
        app.adjust_option_value(OptionId::Theme, 1);
        assert!(app.themes[app.active_theme.unwrap()].file.font.is_none());
        assert_eq!(
            app.font,
            Font::Pixel2,
            "old themes preserve the selected system font"
        );
        app.set_screen(Screen::Browse);
        app.set_layout(Layout::Details);
        capture_frame(
            &mut app,
            &directory,
            &format!("legacy-theme-{name}"),
            352,
            240,
        );
    }
    app.active_theme = None;
    app.apply_palette();
    capture_every_menu_row(&mut app, &directory);
    capture_scraper_progress_states(&mut app, &directory);
    app.ui.hide().unwrap();
}

fn run_favorite_information_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let mut app = fixture_app(root, window, Settings::default());
    let original = app.here[0].clone();
    let game = row_target(&original).unwrap();
    let favorite_root = root.join("_@Favorites");
    std::fs::create_dir(&favorite_root).unwrap();
    let favorite = favorite_root.join("First Game.mgl");
    let mgl = crate::launch::favorite_mgl(&app.all_systems[0].to_config(), &game)
        .unwrap()
        .unwrap();
    std::fs::write(&favorite, mgl).unwrap();
    let mut favorites = app.all_systems[0].clone();
    favorites.def.id = "Favorites".into();
    favorites.def.name = "Favorites".into();
    favorites.def.category = Some("Favorites".into());
    favorites.paths = vec![favorite_root];
    app.all_systems.push(favorites);
    app.open_system = Some("Favorites".into());
    app.here = vec![browse::Row {
        kind: browse::Kind::Play(browse::Launch::File(favorite)),
        ..original
    }];
    app.game_list = ListState::new(1, app.geometry.visible);
    assert!(app.in_favorites());
    let request = app.information_request(&app.here[0]).unwrap();
    assert_eq!(request.launch, browse::Launch::File(game));
    assert!(matches!(
        request.source,
        crate::information_job::Source::Gamelist
    ));
    let cache_before = cache_snapshot(&app.cache_dir);
    app.open_context();
    app.open_information();
    app.finish_background_work_for_headless();
    assert!(app.ui.get_information_text().contains("Final paragraph."));
    assert_eq!(cache_snapshot(&app.cache_dir), cache_before);
    app.handle(Action::Quit);
    app.settings.artwork_pack_roots.insert(
        "NES".into(),
        root.join("docs").to_string_lossy().into_owned(),
    );
    app.effective_artwork_pack_roots = crate::artwork_source::resolve(
        &app.all_systems,
        &app.settings,
        &app.cache_dir,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .unwrap()
    .roots;
    app.open_information();
    assert!(app
        .ui
        .get_information_text()
        .contains("Artwork Pack metadata is unavailable"));
    assert!(
        !app.ui.get_information_text().contains("Final paragraph."),
        "a favourite whose owner selects Pack must never fall back to gamelist text"
    );
    app.ui.hide().unwrap();
}

/// The three files a prepared Pack system leaves beside the cache: its
/// rows, its decision and its prepared presentation, each as bytes when
/// it is there.
fn pack_files(cache_dir: &Path, id: &str) -> [Option<Vec<u8>>; 3] {
    [
        crate::cache::artwork_pack_system_path(cache_dir, id),
        crate::cache::artwork_pack_source_path(cache_dir, id),
        crate::cache::artwork_pack_prepared_path(cache_dir, id),
    ]
    .map(|path| std::fs::read(path).ok())
}

/// What the log has said since `from`.
fn log_since(from: u64) -> String {
    let logged = std::fs::read(crate::LOG_PATH).unwrap_or_default();
    String::from_utf8_lossy(&logged[(from as usize).min(logged.len())..]).into_owned()
}

fn log_len() -> u64 {
    std::fs::metadata(crate::LOG_PATH)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Leave the open system, folder by folder, with nothing on screen in
/// the way of the presses.
fn leave_system(app: &mut App) {
    app.message = None;
    app.pending = None;
    for _ in 0..4 {
        if app.open_system.is_none() {
            return;
        }
        app.handle(Action::Quit);
    }
    assert!(
        app.open_system.is_none(),
        "B on the top folder leaves the system"
    );
}

/// An Automatic system with an installed Pack is prepared when its user
/// says so, for that system alone, and from then on opens on what was
/// written down: no worker, no table, no row walk. Every other outcome
/// of the question, and every way the Pack or the answer can change, is
/// walked through the interface here, on the fixture the eager index
/// used to prepare unasked.
fn run_automatic_pack_consent_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("automatic-pack-consent");
    let games = root.join("games/NES");
    let extra = root.join("games/NES-extra");
    let docs = root.join("docs");
    let artwork = docs.join("NES/Artwork");
    for directory in [&games, &extra, &artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(games.join("Known.nes"), b"first rom").unwrap();
    std::fs::write(extra.join("Second.nes"), b"second rom").unwrap();
    let write_manifest = |keys: &[&str]| {
        let rows: String = keys
            .iter()
            .map(|key| format!("{key}\tbox-2D\t3\n"))
            .collect();
        std::fs::write(
            artwork.join("manifest.tsv"),
            format!("#key\tstyle\tss_system_id\n{rows}"),
        )
        .unwrap();
    };
    write_manifest(&["Known", "Second"]);
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\nSecond\t\t\tSecond\n",
    )
    .unwrap();
    let write_gameinfo = |first: &str| {
        std::fs::write(
            artwork.join("gameinfo.tsv"),
            format!("#key\tname\tyear\tgenre\tdeveloper\tplayers\nKnown\t{first}\t1990\tAction\tStudio\t1\nSecond\tPack Second\t1991\tPuzzle\tStudio\t2\n"),
        )
        .unwrap();
    };
    write_gameinfo("Pack First");
    for key in ["Known", "Second"] {
        std::fs::write(artwork.join(format!("{key}.jpg")), crate::covers::JPEG_16).unwrap();
    }
    let prepare = |app: &mut App| {
        app.all_systems[0].paths = vec![games.clone(), extra.clone()];
        app.systems = app.all_systems.clone();
        app.pack_candidate_bases = vec![root.clone()];
    };
    let available = "Artwork Pack Available\n\nAn installed Artwork Pack was found for NES. Prepare its artwork and metadata now?\n\nA Prepare   B Not Now";
    let changed = "Artwork Pack Changed\n\nThe installed Artwork Pack data for NES has changed. Update its prepared artwork and metadata now?\n\nA Update   B Keep Current";
    let assert_prompt = |app: &App, text: &str| {
        assert_eq!(app.message.as_deref(), Some(text));
        assert!(
            matches!(
                (&app.pending, text.starts_with("Artwork Pack Available")),
                (Some(Pending::PrepareArtworkPack(_)), true)
                    | (Some(Pending::UpdateArtworkPack(_)), false)
            ),
            "{:?}",
            app.pending
        );
        assert!(app.source_job.is_none() && app.provider_job.is_none());
    };
    let assert_no_pack_work = |app: &App| {
        assert!(
            !app.cache_dir.join("artwork-pack").exists(),
            "no Pack file is written before the user says so"
        );
        assert!(app.artwork_provider_cache.is_empty());
        assert!(app.provider_job.is_none() && app.provider_requests.is_empty());
        assert!(app.source_job.is_none() && app.source_recovery_queue.is_empty());
    };
    let assert_pack_rows = |app: &App| {
        assert_eq!(app.here[0].name, "Pack First", "{:?}", app.here);
        assert_eq!(app.here[0].cover, Some(artwork.join("Known.jpg")));
        assert_eq!(app.here[0].genre.as_deref(), Some("Action"));
    };
    let assert_ordinary_rows = |app: &App| {
        assert_eq!(app.here[0].name, "Known.nes", "{:?}", app.here);
        assert_eq!(app.here[0].cover, None);
    };
    let open = |app: &mut App| {
        app.open_system_by_index(0);
        if app.open_system.is_some() {
            app.enter(Place::Dir(games.clone()));
        }
    };
    let assert_operation_controls = |app: &mut App, details: bool, controls: &str| {
        app.refresh();
        assert!(!app.show_bar);
        assert!(!app.ui.get_show_bar());
        assert!(app.geometry.bar > 0.0);
        assert!(app.ui.get_bar_height() >= app.ui.get_bar_glyph());
        assert_eq!(app.ui.get_operation_details(), details);
        assert_eq!(app.ui.get_operation_controls(), controls);
        if details {
            assert!(!app.ui.get_overlay().is_empty());
            assert_eq!(app.ui.get_operation_kind(), 1);
        } else {
            assert!(app.ui.get_operation_footer_visible());
        }
    };
    let assert_complete = |app: &App, expected_games: usize| {
        assert!(app.build.is_none());
        assert!(app.source_job.is_none());
        assert!(app.source_recovery_queue.is_empty());
        assert_eq!(
            app.index.as_ref().unwrap().systems["NES"].games,
            expected_games
        );
        assert_eq!(
            crate::cache::load_index(&app.cache_dir).unwrap().systems["NES"].games,
            expected_games
        );
        assert_eq!(app.total_games, expected_games);
        assert!(app
            .ui
            .get_about_line()
            .contains(&format!("{expected_games} games")));
        assert!(!app.empty_systems.as_ref().unwrap().contains("NES"));
        assert_eq!(
            crate::artwork_source::mode(&app.settings, "NES"),
            crate::artwork_source::Mode::Automatic
        );
        assert!(app.settings.artwork_pack_roots.is_empty());
        let data = crate::cache::load_artwork_pack_data(&app.cache_dir, "NES").unwrap();
        assert_eq!(data.cache.summary(&Place::Roots).games, expected_games);
    };
    let browse_bar_off = Settings {
        show_bar: Some(false),
        ..Default::default()
    };

    // 1, 2. The card is read into the index; the installed Pack is not.
    let mut app = unopened_fixture_app(&root, window.clone(), browse_bar_off.clone());
    prepare(&mut app);
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 2);
    assert!(crate::cache::system_path(&app.cache_dir, "NES").exists());
    assert_eq!(app.total_games, 2);
    assert!(app.ui.get_about_line().contains("2 games"));
    assert_eq!(
        app.source_label("NES"),
        format!("Automatic (Using: {SOURCE_GAMELIST})"),
        "an installed Pack nobody accepted is not the source"
    );
    assert_no_pack_work(&app);
    // A never-entered system is looked into by the screensaver and by
    // the favourites shelf with its ordinary data, not its Pack's.
    app.refill_saver();
    assert!(
        app.saver_pool.is_empty(),
        "the ordinary index holds no pictures for this system: {:?}",
        app.saver_pool
    );
    let favorite = root.join("Known.mgl");
    std::fs::write(
        &favorite,
        crate::launch::favorite_mgl(&app.all_systems[0].to_config(), &games.join("Known.nes"))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let favorite_row = || browse::Row {
        kind: browse::Kind::Play(browse::Launch::File(favorite.clone())),
        ..information_row()
    };
    let mut favorite_rows = [favorite_row()];
    enrich_favorite_rows(
        &mut favorite_rows,
        &app.all_systems,
        &app.homes(),
        &app.effective_artwork_pack_roots,
        &app.cache_dir,
        |id, _, _| app.artwork_provider_cache.get(id).cloned(),
    );
    assert_eq!(favorite_rows[0].name, "Known.nes");
    assert_eq!(favorite_rows[0].cover, None);
    assert_no_pack_work(&app);

    // 3. A gamelist under the system's own folder decides first.
    std::fs::write(games.join("gamelist.xml"), "<gameList/>").unwrap();
    open(&mut app);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(app.artwork_provider.is_none());
    assert_ordinary_rows(&app);
    assert_no_pack_work(&app);
    leave_system(&mut app);

    // The opt-in priority affects only systems left on Automatic. It is a
    // source-order setting, not a per-system rewrite or an eager Pack read.
    let source_settings_before = (
        app.settings.artwork_pack_roots.clone(),
        app.settings.gamelist_sources.clone(),
    );
    app.adjust_option_value(OptionId::AutomaticDataSource, 1);
    assert_eq!(
        app.option_value(OptionId::AutomaticDataSource),
        "Artwork Pack First"
    );
    assert_eq!(
        (
            app.settings.artwork_pack_roots.clone(),
            app.settings.gamelist_sources.clone(),
        ),
        source_settings_before,
        "the global priority must not rewrite per-system choices"
    );
    assert_no_pack_work(&app);
    open(&mut app);
    assert_prompt(&app, available);
    assert!(
        games.join("gamelist.xml").exists(),
        "Artwork Pack First asks despite an available gamelist"
    );
    app.handle(Action::Up);
    assert_no_pack_work(&app);
    app.adjust_option_value(OptionId::AutomaticDataSource, 1);
    assert_eq!(
        app.option_value(OptionId::AutomaticDataSource),
        "Gamelist First"
    );
    assert!(app.save_settings());
    assert_eq!(
        Settings::load(&app.settings_path)
            .unwrap()
            .automatic_data_source,
        Some(crate::settings::AutomaticDataSource::GamelistFirst)
    );
    open(&mut app);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert_ordinary_rows(&app);
    leave_system(&mut app);
    std::fs::remove_file(games.join("gamelist.xml")).unwrap();

    // 4. No Pack where one would be looked for: opened as it is.
    app.pack_candidate_bases = vec![root.join("nowhere")];
    open(&mut app);
    assert!(app.pending.is_none());
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert_ordinary_rows(&app);
    assert_no_pack_work(&app);
    leave_system(&mut app);
    app.pack_candidate_bases = vec![root.clone()];

    // 5. The Pack is there: asked before any of it is read.
    let logged_from = log_len();
    open(&mut app);
    assert_prompt(&app, available);
    assert!(app.open_system.is_none());
    assert_no_pack_work(&app);
    assert!(log_since(logged_from).contains("artwork pack NES: prompt shown: available"));
    app.refresh();
    assert_eq!(app.ui.get_overlay().as_str(), available);
    assert!(!app.ui.get_overlay_dismissible());
    capture_live_if_requested(&mut app, "source-auto-pack-available");
    // A press that is not an answer takes the question down and decides
    // nothing: it is asked again.
    app.handle(Action::Up);
    assert!(app.pending.is_none() && app.message.is_none());
    assert!(app.open_system.is_none());
    assert_no_pack_work(&app);
    // Nothing in the Pack is read before the answer, the manifest
    // included: with it unreadable the question is still asked, and only
    // the No then finds it cannot be remembered, says so, and opens the
    // system without the Pack all the same.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let manifest = artwork.join("manifest.tsv");
        let mode = std::fs::metadata(&manifest).unwrap().permissions().mode();
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o000)).unwrap();
        open(&mut app);
        assert_prompt(&app, available);
        app.handle(Action::Quit);
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
        let message = app
            .message
            .clone()
            .expect("a No that cannot be remembered is said");
        assert!(
            message.starts_with("Artwork Pack decision for NES was not saved"),
            "{message}"
        );
        assert!(
            crate::cache::load_pack_source_state(&app.cache_dir, "NES")
                .unwrap()
                .is_none(),
            "nothing is written down for a No without a signature"
        );
        leave_system(&mut app);
    }
    open(&mut app);
    assert_prompt(&app, available);

    // 6. Not Now: opened without the Pack, and remembered for this Pack.
    app.handle(Action::Quit);
    assert!(app.pending.is_none() && app.message.is_none());
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_ordinary_rows(&app);
    let [rows, decision, prepared] = pack_files(&app.cache_dir, "NES");
    assert!(rows.is_none() && prepared.is_none());
    assert!(decision.is_some(), "Not Now is written down");
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_none());
    assert_eq!(
        state
            .declined
            .as_ref()
            .map(|declined| declined.docs_root.as_str()),
        Some(docs.to_str().unwrap())
    );
    assert!(log_since(logged_from).contains("artwork pack NES: declined recorded"));
    assert_eq!(
        app.source_label("NES"),
        format!("Automatic (Using: {SOURCE_GAMELIST})")
    );
    for _ in 0..2 {
        leave_system(&mut app);
        open(&mut app);
        assert!(
            app.pending.is_none(),
            "the same Pack is not asked about again"
        );
        assert_eq!(app.open_system.as_deref(), Some("NES"));
        assert_ordinary_rows(&app);
    }
    assert!(app.artwork_provider_cache.is_empty());
    app.ui.hide().unwrap();
    drop(app);
    let mut app = unopened_fixture_app(&root, window.clone(), browse_bar_off.clone());
    prepare(&mut app);
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none(), "the index is read, not rebuilt");
    open(&mut app);
    assert!(app.pending.is_none(), "Not Now survives a restart");
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert_ordinary_rows(&app);
    leave_system(&mut app);
    // A decision file that is there but cannot be read is said, not read
    // as no decision: nothing is asked, nothing is prepared and nothing
    // is written over it.
    let decision = crate::cache::artwork_pack_source_path(&app.cache_dir, "NES");
    let decision_bytes = std::fs::read(&decision).unwrap();
    std::fs::remove_file(&decision).unwrap();
    std::fs::create_dir(&decision).unwrap();
    open(&mut app);
    assert!(app.pending.is_none());
    assert!(app.open_system.is_none());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.starts_with("NES: game data source could not be resolved")),
        "{:?}",
        app.message
    );
    assert!(app.provider_job.is_none() && app.source_job.is_none());
    std::fs::remove_dir(&decision).unwrap();
    std::fs::write(&decision, decision_bytes).unwrap();
    app.message = None;

    // 7. A changed Pack is another Pack: asked about again.
    write_manifest(&["Known", "Second", "Third"]);
    open(&mut app);
    assert_prompt(&app, available);
    app.handle(Action::Quit);
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    leave_system(&mut app);
    open(&mut app);
    assert!(app.pending.is_none());

    // 21. Rebuild This System List is the deliberate retry: a declined
    // Pack is asked about again; Not Now rebuilds the ordinary list.
    // The No given before is set aside for the question only: a press
    // that is not an answer leaves it standing, on disk and at the next
    // entry.
    app.rebuild_open_system();
    assert_prompt(&app, available);
    app.handle(Action::Up);
    assert!(app.pending.is_none() && app.build.is_none());
    assert!(
        crate::cache::load_pack_source_state(&app.cache_dir, "NES")
            .unwrap()
            .unwrap()
            .declined
            .is_some(),
        "a question nobody answered erases nothing"
    );
    leave_system(&mut app);
    open(&mut app);
    assert!(app.pending.is_none(), "the No given before still stands");
    write_manifest(&["Known", "Second"]);
    // A No the rebuild cannot write down is said with the rebuild's
    // result, not lost under its progress.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let store = app.cache_dir.join("artwork-pack");
        let mode = std::fs::metadata(&store).unwrap().permissions().mode();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555)).unwrap();
        app.rebuild_open_system();
        assert_prompt(&app, available);
        app.handle(Action::Quit);
        assert!(app.build.as_ref().is_some_and(|build| build.single));
        app.finish_background_work_for_headless();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(mode)).unwrap();
        let overview = app.index_terminal.as_ref().expect("the rebuild finished");
        assert_eq!(
            overview.state, "Finished With Problems",
            "{:?}",
            overview.problem
        );
        assert!(
            overview
                .problem
                .contains("Artwork Pack decision for NES was not saved"),
            "{}",
            overview.problem
        );
        app.handle(Action::Quit);
        prepare(&mut app);
    }
    app.rebuild_open_system();
    assert_prompt(&app, available);
    app.handle(Action::Quit);
    assert!(app.build.as_ref().is_some_and(|build| build.single));
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    assert_eq!(
        app.index_terminal
            .as_ref()
            .map(|overview| overview.state.as_str()),
        Some("Complete"),
        "{:?}",
        app.message
    );
    assert_eq!(
        app.index_terminal
            .as_ref()
            .map(|overview| overview.subject.as_str()),
        Some("NES")
    );
    app.handle(Action::Quit);
    assert!(pack_files(&app.cache_dir, "NES")[0].is_none());
    leave_system(&mut app);
    open(&mut app);
    assert!(app.pending.is_none(), "the No given at the rebuild stands");
    leave_system(&mut app);
    // The ordinary rebuild found the system's folders again under the
    // root, which holds one of the two the fixture gives it.
    prepare(&mut app);

    // 16. Prepare that is cancelled: nothing is written, the ordinary
    // system opens, and the next entry asks again. The Pack is changed
    // first so that the No given above is not the answer any more.
    std::fs::write(
        artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nKnown\tbox-3D\t3\nSecond\tbox-3D\t3\n",
    )
    .unwrap();
    open(&mut app);
    assert_prompt(&app, available);
    app.handle(Action::Accept);
    assert!(app.source_job.is_some(), "{:?}", app.message);
    assert_eq!(app.screen, Screen::SourceProgress);
    app.refresh();
    assert_eq!(app.ui.get_heading(), "Preparing Artwork Pack");
    assert_eq!(app.ui.get_status(), "B Cancel");
    capture_live_if_requested(&mut app, "source-auto-pack-preparing");
    app.handle(Action::Quit);
    assert!(app.source_cancelling);
    app.refresh();
    assert_eq!(app.ui.get_status(), "Stopping safely");
    app.finish_background_work_for_headless();
    assert!(app.source_job.is_none());
    assert_eq!(
        app.message.as_deref(),
        Some("Artwork Pack preparation for NES cancelled."),
    );
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_ordinary_rows(&app);
    let [rows, _, prepared] = pack_files(&app.cache_dir, "NES");
    assert!(
        rows.is_none() && prepared.is_none(),
        "a cancelled preparation writes nothing down"
    );
    assert!(crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap()
        .accepted
        .is_none());
    assert!(!app.effective_artwork_pack_roots.contains_key("NES"));
    leave_system(&mut app);
    open(&mut app);
    assert_prompt(&app, available);
    app.handle(Action::Up);

    // 16. Prepare that fails: the same, with the reason.
    let good_manifest = std::fs::read(artwork.join("manifest.tsv")).unwrap();
    std::fs::write(artwork.join("manifest.tsv"), "not-a-valid-manifest\n").unwrap();
    open(&mut app);
    assert_prompt(&app, available);
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert!(app.source_job.is_none());
    let message = app.message.clone().expect("a failed preparation says so");
    assert_eq!(
        message,
        "NES Artwork Pack was not prepared.\nArtwork Pack invalid: repair it and try again.",
        "the category the worker found is on screen, without its detail"
    );
    assert!(!message.contains("previous complete cache"));
    assert!(!message.contains('/'), "no path on screen: {message}");
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_ordinary_rows(&app);
    let [rows, _, prepared] = pack_files(&app.cache_dir, "NES");
    assert!(rows.is_none() && prepared.is_none());
    assert!(crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap()
        .accepted
        .is_none());
    assert!(!app.effective_artwork_pack_roots.contains_key("NES"));
    assert!(
        app.source_recovery_suppressed.is_empty(),
        "a later retry is not suppressed"
    );
    std::fs::write(artwork.join("manifest.tsv"), &good_manifest).unwrap();
    app.message = None;
    app.rebuild_open_system();
    assert_prompt(&app, available);
    app.handle(Action::Up);
    leave_system(&mut app);

    // 8. Prepare: one system, then the system opens on its Pack.
    let other_cache = crate::cache::system_path(&app.cache_dir, "Other");
    std::fs::write(&other_cache, b"another system's list").unwrap();
    open(&mut app);
    assert_prompt(&app, available);
    app.handle(Action::Accept);
    assert!(app.source_job.is_some());
    assert!(
        !app.effective_artwork_pack_roots.contains_key("NES"),
        "the root is not taken into use before the result is installed"
    );
    app.finish_background_work_for_headless();
    assert!(app.source_job.is_none());
    assert_eq!(app.source_progress.systems, 1, "one system, not the group");
    assert_eq!(app.message.as_deref(), Some("Artwork Pack prepared."));
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    assert_eq!(
        app.source_label("NES"),
        format!("Automatic (Using: {SOURCE_ARTWORK_PACK})")
    );
    let [rows, decision, prepared] = pack_files(&app.cache_dir, "NES");
    assert!(rows.is_some() && decision.is_some() && prepared.is_some());
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    let accepted = state.accepted.expect("Prepare is written down");
    assert!(state.declined.is_none());
    assert_eq!(accepted.docs_root, docs.to_str().unwrap());
    assert_eq!(accepted.skipped_entries, 0);
    assert_eq!(
        accepted.cache_marker,
        crc32fast::hash(rows.as_ref().unwrap()),
        "the marker is the rows the mapping was prepared from"
    );
    assert_eq!(
        std::fs::read(&other_cache).unwrap(),
        b"another system's list",
        "another system's list is not touched"
    );
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 2);
    assert_eq!(app.total_games, 2);
    let prepared_files = pack_files(&app.cache_dir, "NES");

    // With both sources available, changing the global priority makes the
    // already-prepared Pack dormant or active without rewriting, deleting or
    // rebuilding it. Returning to Pack First reuses the same prepared state.
    leave_system(&mut app);
    std::fs::write(games.join("gamelist.xml"), "<gameList/>").unwrap();
    open(&mut app);
    assert!(app.pending.is_none());
    assert_ordinary_rows(&app);
    assert_eq!(pack_files(&app.cache_dir, "NES"), prepared_files);
    leave_system(&mut app);
    app.adjust_option_value(OptionId::AutomaticDataSource, 1);
    open(&mut app);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_pack_rows(&app);
    assert!(app.source_job.is_none() && app.provider_job.is_none());
    assert_eq!(pack_files(&app.cache_dir, "NES"), prepared_files);
    leave_system(&mut app);
    app.adjust_option_value(OptionId::AutomaticDataSource, 1);
    open(&mut app);
    assert!(app.pending.is_none());
    assert_ordinary_rows(&app);
    assert_eq!(pack_files(&app.cache_dir, "NES"), prepared_files);
    leave_system(&mut app);
    std::fs::remove_file(games.join("gamelist.xml")).unwrap();

    // 9. Left and entered again: no worker, no progress, the same rows.
    for _ in 0..3 {
        leave_system(&mut app);
        let logged_from = log_len();
        open(&mut app);
        assert!(app.pending.is_none());
        assert_ne!(app.screen, Screen::SourceProgress);
        assert!(app.provider_job.is_none() && app.provider_pending_open.is_none());
        assert!(app.source_job.is_none());
        assert_pack_rows(&app);
        assert!(
            log_since(logged_from).contains("artwork pack NES: state reused"),
            "{}",
            log_since(logged_from)
        );
    }
    assert_eq!(pack_files(&app.cache_dir, "NES"), prepared_files);

    // The favourite and the screensaver now find the Pack through what
    // was written down, without a worker.
    let mut favorite_rows = [favorite_row()];
    let (systems, homes, roots, cache_dir) = (
        app.all_systems.clone(),
        app.homes(),
        app.effective_artwork_pack_roots.clone(),
        app.cache_dir.clone(),
    );
    app.artwork_provider_cache.clear();
    enrich_favorite_rows(
        &mut favorite_rows,
        &systems,
        &homes,
        &roots,
        &cache_dir,
        |id, root, _| app.provider_from_state(id, root).unwrap(),
    );
    assert_eq!(favorite_rows[0].name, "Pack First");
    assert_eq!(favorite_rows[0].cover, Some(artwork.join("Known.jpg")));
    app.saver_candidates = None;
    app.saver_pool.clear();
    app.refill_saver();
    assert_eq!(
        app.saver_pool
            .iter()
            .map(|picture| picture.path.clone())
            .collect::<HashSet<_>>(),
        [artwork.join("Known.jpg"), artwork.join("Second.jpg")]
            .into_iter()
            .collect()
    );
    assert!(app.provider_job.is_none());

    // 10. A new process, the Artwork directory unlistable and a ROM gone:
    // opened on the state alone, with its Pack data.
    app.ui.hide().unwrap();
    drop(app);
    let mut app = unopened_fixture_app(&root, window.clone(), browse_bar_off.clone());
    prepare(&mut app);
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    assert_eq!(
        app.effective_artwork_pack_roots
            .get("NES")
            .map(String::as_str),
        Some(docs.to_str().unwrap()),
        "the accepted root is known at startup from its state file"
    );
    assert!(app.artwork_provider_cache.is_empty(), "known, not read");
    assert!(app.provider_job.is_none() && app.provider_requests.is_empty());
    std::fs::remove_file(extra.join("Second.nes")).unwrap();
    #[cfg(unix)]
    let original_mode = {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&artwork).unwrap().permissions().mode();
        std::fs::set_permissions(&artwork, std::fs::Permissions::from_mode(0o111)).unwrap();
        mode
    };
    let logged_from = log_len();
    open(&mut app);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artwork, std::fs::Permissions::from_mode(original_mode)).unwrap();
    }
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert!(app.provider_job.is_none() && app.source_job.is_none());
    assert_pack_rows(&app);
    assert_eq!(app.here.len(), 1);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    assert_eq!(pack_files(&app.cache_dir, "NES"), prepared_files);
    std::fs::write(extra.join("Second.nes"), b"second rom").unwrap();

    // 13, 14. A table edited: asked once; Keep Current keeps the rows and
    // is remembered, in this process and the next.
    leave_system(&mut app);
    write_gameinfo("Pack First Revised");
    let logged_from = log_len();
    open(&mut app);
    assert_prompt(&app, changed);
    assert!(log_since(logged_from).contains("artwork pack NES: prompt shown: changed"));
    assert_eq!(pack_files(&app.cache_dir, "NES"), prepared_files);
    app.refresh();
    capture_live_if_requested(&mut app, "source-auto-pack-changed");
    app.handle(Action::Quit);
    assert!(app.pending.is_none() && app.message.is_none());
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(
        state.accepted.is_some(),
        "Keep Current keeps the acceptance"
    );
    assert!(state.declined.is_some(), "and remembers the change");
    assert_eq!(pack_files(&app.cache_dir, "NES")[0], prepared_files[0]);
    assert_eq!(pack_files(&app.cache_dir, "NES")[2], prepared_files[2]);
    let request = app.information_request(&app.here[0]).unwrap();
    assert!(matches!(
        request.source,
        crate::information_job::Source::ArtworkPack(_)
    ));
    leave_system(&mut app);
    open(&mut app);
    assert!(
        app.pending.is_none(),
        "the kept change is not asked about again"
    );
    assert_pack_rows(&app);
    let kept_files = pack_files(&app.cache_dir, "NES");
    app.ui.hide().unwrap();
    drop(app);
    let mut app = unopened_fixture_app(&root, window.clone(), browse_bar_off.clone());
    prepare(&mut app);
    app.leave_splash();
    app.finish_background_work_for_headless();
    open(&mut app);
    assert!(app.pending.is_none(), "Keep Current survives a restart");
    assert_pack_rows(&app);

    // 13, 14. The language changed: asked; Keep Current keeps the rows
    // prepared for the previous language in use under the new one, with
    // no worker and no row walk, now, at the next entry and after a
    // restart. The acceptance still records the language they were
    // prepared for.
    leave_system(&mut app);
    app.scraper_settings.language = Some("it".into());
    open(&mut app);
    assert_prompt(&app, changed);
    // While the question stands, nothing is kept about the new language:
    // the rows prepared for the previous one are handed to the favourites
    // shelf without being held in memory, and the shelf draws with them
    // all the same, once per owning system rather than once per row.
    {
        let mut favorites = app.all_systems[0].clone();
        favorites.def.id = "Favorites".into();
        favorites.def.name = "Favorites".into();
        favorites.def.category = Some("Favorites".into());
        favorites.paths = Vec::new();
        app.all_systems.push(favorites);
        let (was_open, was_pending, was_message) = (
            app.open_system.replace("Favorites".into()),
            app.pending.take(),
            app.message.take(),
        );
        app.artwork_provider_cache.clear();
        let mut shelf = [favorite_row(), favorite_row()];
        app.enrich_favorites(&mut shelf);
        assert!(
            !app.artwork_provider_cache.contains_key("NES"),
            "rows prepared for another language are not held while the question stands"
        );
        for row in &shelf {
            assert_eq!(
                row.name, "Pack First",
                "the favourite draws with the rows the shelf was handed"
            );
            assert_eq!(row.cover, Some(artwork.join("Known.jpg")));
        }
        assert!(app.provider_job.is_none() && app.provider_requests.is_empty());
        app.all_systems.pop();
        app.open_system = was_open;
        app.pending = was_pending;
        app.message = was_message;
    }
    app.handle(Action::Quit);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(
        app.provider_job.is_none()
            && app.provider_pending_open.is_none()
            && app.source_job.is_none(),
        "a kept change of language reads nothing: {:?}",
        app.message
    );
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert_eq!(
        state.accepted.as_ref().unwrap().language,
        None,
        "the rows stay recorded as prepared for the previous language"
    );
    assert_eq!(
        state.declined.as_ref().unwrap().language.as_deref(),
        Some("it")
    );
    assert_eq!(pack_files(&app.cache_dir, "NES")[0], kept_files[0]);
    assert_eq!(pack_files(&app.cache_dir, "NES")[2], kept_files[2]);
    let kept_files = pack_files(&app.cache_dir, "NES");
    for _ in 0..2 {
        leave_system(&mut app);
        let logged_from = log_len();
        open(&mut app);
        assert!(app.pending.is_none(), "{:?}", app.message);
        assert!(app.provider_job.is_none() && app.provider_pending_open.is_none());
        assert_pack_rows(&app);
        assert!(
            log_since(logged_from).contains("artwork pack NES: state reused"),
            "{}",
            log_since(logged_from)
        );
    }
    assert_eq!(pack_files(&app.cache_dir, "NES"), kept_files);
    app.ui.hide().unwrap();
    drop(app);
    let mut app = unopened_fixture_app(&root, window.clone(), browse_bar_off.clone());
    prepare(&mut app);
    app.scraper_settings.language = Some("it".into());
    app.leave_splash();
    app.finish_background_work_for_headless();
    let logged_from = log_len();
    open(&mut app);
    assert!(
        app.pending.is_none(),
        "the kept change of language survives a restart: {:?}",
        app.message
    );
    assert!(app.provider_job.is_none() && app.provider_pending_open.is_none());
    assert_pack_rows(&app);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    assert_eq!(pack_files(&app.cache_dir, "NES"), kept_files);

    // 13. A table changed under the kept language: asked; 15. Update
    // replaces the result after success, for this system alone, and
    // records the language in use.
    leave_system(&mut app);
    write_gameinfo("Pack First");
    open(&mut app);
    assert_prompt(&app, changed);
    app.handle(Action::Accept);
    assert!(app.source_job.is_some());
    app.finish_background_work_for_headless();
    assert_eq!(app.source_progress.systems, 1);
    assert_eq!(app.message.as_deref(), Some("Artwork Pack prepared."));
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert_eq!(
        state.accepted.as_ref().unwrap().language.as_deref(),
        Some("it")
    );
    assert!(state.declined.is_none());
    assert_ne!(pack_files(&app.cache_dir, "NES")[1], kept_files[1]);
    leave_system(&mut app);
    open(&mut app);
    assert!(app.pending.is_none());

    // 13. The rows the mapping was prepared from replaced: asked. 15.
    // Update: the mapping follows the new rows.
    leave_system(&mut app);
    std::fs::write(games.join("Third.nes"), b"third rom").unwrap();
    let library =
        Library::open_source_neutral(&app.all_systems[0].to_config(), Default::default()).unwrap();
    crate::cache::install_transactional(
        &app.cache_dir,
        crate::cache::CacheKind::ArtworkPack,
        &[crate::cache::StagedSystemCache {
            id: "NES".into(),
            cache: crate::cache::build_system(&library),
            fingerprints: Default::default(),
            fingerprints_complete: true,
        }],
    )
    .unwrap();
    let before_update = pack_files(&app.cache_dir, "NES");
    open(&mut app);
    assert_prompt(&app, changed);

    // 17. An Update that is cancelled, then one that fails: the previous
    // result is untouched, byte for byte, and still in use.
    app.handle(Action::Accept);
    assert!(app.source_job.is_some());
    app.handle(Action::Quit);
    assert!(app.source_cancelling);
    app.finish_background_work_for_headless();
    assert_eq!(
        app.message.as_deref(),
        Some("Artwork Pack preparation for NES cancelled. The previous complete cache remains in use.")
    );
    assert_eq!(pack_files(&app.cache_dir, "NES"), before_update);
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    leave_system(&mut app);
    std::fs::write(artwork.join("manifest.tsv"), "not-a-valid-manifest\n").unwrap();
    open(&mut app);
    assert_prompt(&app, changed);
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    let message = app.message.clone().unwrap();
    assert!(
        message.starts_with("NES Artwork Pack was not prepared.")
            && message.ends_with("The previous complete cache remains in use."),
        "{message}"
    );
    assert_eq!(pack_files(&app.cache_dir, "NES"), before_update);
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    std::fs::write(artwork.join("manifest.tsv"), &good_manifest).unwrap();
    leave_system(&mut app);
    open(&mut app);
    assert_prompt(&app, changed);
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Artwork Pack prepared."));
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    assert_eq!(app.here.len(), 2, "{:?}", app.here);
    assert_ne!(pack_files(&app.cache_dir, "NES")[0], before_update[0]);
    assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, 3);

    // 21. Rebuild This System List on the accepted system: the deliberate
    // preparation, one system, no question.
    app.rebuild_open_system();
    assert!(app.source_job.is_some(), "{:?}", app.message);
    assert!(app.pending.is_none());
    app.finish_background_work_for_headless();
    assert_eq!(app.source_progress.systems, 1);
    assert_eq!(app.message.as_deref(), Some("NES list rebuilt."));
    assert_pack_rows(&app);
    assert_eq!(
        std::fs::read(&other_cache).unwrap(),
        b"another system's list"
    );

    // Rebuild All reads the accepted system the ordinary way, into its
    // Pack cache and without its Pack: nothing is prepared unasked, and
    // what was written down about the Pack is left as it was. The rows
    // being other rows than the mapping was prepared from, the next
    // entry asks; Keep Current keeps browsing on the previous mapping
    // over them, and is remembered; Rebuild This System List then
    // prepares the current Pack whatever was kept.
    app.message = None;
    let prepared_before = pack_files(&app.cache_dir, "NES");
    let ordinary_path = crate::cache::system_path(&app.cache_dir, "NES");
    let ordinary_before = std::fs::read(&ordinary_path).unwrap();
    app.start_build(true);
    assert!(
        app.source_recovery_queue.is_empty(),
        "an Automatic acceptance is not prepared inside the rebuild"
    );
    app.finish_background_work_for_headless();
    assert_eq!(app.index_terminal.as_ref().unwrap().state, "Complete");
    assert_eq!(app.index_terminal.as_ref().unwrap().games, 3);
    // A preparation inside the rebuild would write the decision and the
    // mapping afresh; the files below prove it did not run.
    let rebuilt = pack_files(&app.cache_dir, "NES");
    assert_ne!(rebuilt[0], prepared_before[0], "the rows are read again");
    assert_eq!(rebuilt[1], prepared_before[1], "the decision is untouched");
    assert_eq!(rebuilt[2], prepared_before[2], "the mapping is untouched");
    assert!(
        !crate::cache::load_artwork_pack_data(&app.cache_dir, "NES")
            .unwrap()
            .fingerprints_complete,
        "rows read without the Pack are not a preparation"
    );
    assert_eq!(
        std::fs::read(&ordinary_path).unwrap(),
        ordinary_before,
        "the rows belong to the Pack cache, not the ordinary file"
    );
    app.handle(Action::Quit);
    leave_system(&mut app);
    open(&mut app);
    assert_prompt(&app, changed);
    app.handle(Action::Quit);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert!(
        app.source_job.is_none() && app.provider_job.is_none(),
        "Keep Current prepares nothing: {:?}",
        app.message
    );
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    app.enter(Place::Dir(games.clone()));
    assert_pack_rows(&app);
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_some());
    assert_eq!(
        state
            .declined
            .as_ref()
            .and_then(|declined| declined.cache_marker),
        Some(crc32fast::hash(rebuilt[0].as_ref().unwrap())),
        "the kept change is the rows the rebuild wrote"
    );
    assert_eq!(pack_files(&app.cache_dir, "NES")[0], rebuilt[0]);
    assert_eq!(pack_files(&app.cache_dir, "NES")[2], rebuilt[2]);
    leave_system(&mut app);
    let logged_from = log_len();
    open(&mut app);
    assert!(
        app.pending.is_none(),
        "the kept rows are not asked about again"
    );
    assert!(app.source_job.is_none() && app.provider_job.is_none());
    assert_pack_rows(&app);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    app.rebuild_open_system();
    assert!(app.source_job.is_some(), "{:?}", app.message);
    assert!(app.pending.is_none());
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("NES list rebuilt."));
    assert_pack_rows(&app);
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_some() && state.declined.is_none());
    assert!(
        crate::cache::load_artwork_pack_data(&app.cache_dir, "NES")
            .unwrap()
            .fingerprints_complete
    );
    app.message = None;
    let art_deadline = Instant::now() + Duration::from_secs(3);
    while app.art_pending {
        assert!(
            Instant::now() < art_deadline,
            "the normal artwork loader did not finish"
        );
        slint::platform::update_timers_and_animations();
        if app.art_has_settled(Instant::now()) {
            app.load_art();
            app.refresh();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        app.ui.get_has_art(),
        "the rebuilt browse capture must display its real Pack JPEG"
    );
    let artwork_size = app.ui.get_art().size();
    assert_eq!((artwork_size.width, artwork_size.height), (16, 16));
    assert!(!app.show_bar);
    assert!(!app.ui.get_show_bar());
    assert!(!app.ui.get_bottom_bar_visible());
    capture_live_if_requested(&mut app, "source-auto-pack-open-rebuilt");
    let rebuilt_files = pack_files(&app.cache_dir, "NES");
    assert!(rebuilt_files.iter().all(Option::is_some));

    // 12. An image replaced at its path: read on demand, no rebuild. The
    // picture decoded from the old file is dropped so the new one is read.
    leave_system(&mut app);
    assert!(app.covers.knows(&artwork.join("Known.jpg")));
    let replacement = artwork.join("Known.replacement");
    std::fs::write(&replacement, crate::covers::JPEG_16).unwrap();
    std::fs::rename(&replacement, artwork.join("Known.jpg")).unwrap();
    let logged_from = log_len();
    open(&mut app);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_pack_rows(&app);
    assert!(log_since(logged_from).contains("artwork pack NES: images changed"));
    assert!(
        !app.covers.knows(&artwork.join("Known.jpg")),
        "the decoded picture of the replaced file is dropped"
    );
    assert_ne!(
        pack_files(&app.cache_dir, "NES")[1],
        rebuilt_files[1],
        "the refreshed signature is written down"
    );
    assert_eq!(pack_files(&app.cache_dir, "NES")[0], rebuilt_files[0]);
    assert_eq!(pack_files(&app.cache_dir, "NES")[2], rebuilt_files[2]);
    let logged_from = log_len();
    leave_system(&mut app);
    open(&mut app);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    let rebuilt_files = pack_files(&app.cache_dir, "NES");

    // 18. The Pack's storage gone: said, nothing changed, nothing read as
    // unchanged; back again, the state is used as before.
    leave_system(&mut app);
    std::fs::rename(&docs, root.join("docs-away")).unwrap();
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert!(app.pending.is_none());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("is unavailable")),
        "{:?}",
        app.message
    );
    assert_eq!(
        app.artwork_provider.as_ref().map(|p| p.health),
        Some(crate::artwork_pack::ProviderHealth::Unavailable)
    );
    assert_eq!(pack_files(&app.cache_dir, "NES"), rebuilt_files);
    assert!(log_since(logged_from).contains("artwork pack NES: unavailable at"));
    app.handle(Action::Accept);
    app.enter(Place::Dir(games.clone()));
    assert_eq!(app.here[0].name, "Known.nes", "no Pack data is substituted");
    assert_eq!(app.here[0].cover, None);
    // Said again at every entry while the storage is missing: opening
    // the system without its Pack data needs the press each time.
    leave_system(&mut app);
    app.open_system_by_index(0);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("is unavailable")),
        "{:?}",
        app.message
    );
    app.handle(Action::Accept);
    leave_system(&mut app);
    std::fs::rename(root.join("docs-away"), &docs).unwrap();
    let logged_from = log_len();
    open(&mut app);
    assert!(
        app.pending.is_none() && app.message.is_none(),
        "{:?}",
        app.message
    );
    assert_pack_rows(&app);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    assert_eq!(pack_files(&app.cache_dir, "NES"), rebuilt_files);
    // The favourites and the screensaver find the prepared mapping again
    // too, not the stand-in held while the storage was gone.
    let mut favorite_rows = [favorite_row()];
    let (systems, homes, roots, cache_dir) = (
        app.all_systems.clone(),
        app.homes(),
        app.effective_artwork_pack_roots.clone(),
        app.cache_dir.clone(),
    );
    leave_system(&mut app);
    std::fs::rename(&docs, root.join("docs-away")).unwrap();
    app.open_system_by_index(0);
    assert_eq!(
        app.artwork_provider_cache.get("NES").map(|p| p.health),
        Some(crate::artwork_pack::ProviderHealth::Unavailable)
    );
    app.handle(Action::Accept);
    leave_system(&mut app);
    std::fs::rename(root.join("docs-away"), &docs).unwrap();
    enrich_favorite_rows(
        &mut favorite_rows,
        &systems,
        &homes,
        &roots,
        &cache_dir,
        |id, root, _| app.provider_from_state(id, root).unwrap(),
    );
    assert_eq!(
        favorite_rows[0].name, "Pack First",
        "the stand-in for the missing storage is not kept once it is back"
    );
    assert_eq!(favorite_rows[0].cover, Some(artwork.join("Known.jpg")));
    app.saver_candidates = None;
    app.saver_pool.clear();
    app.refill_saver();
    assert!(
        app.saver_pool
            .iter()
            .any(|picture| picture.path == artwork.join("Known.jpg")),
        "{:?}",
        app.saver_pool
    );
    assert!(app.provider_job.is_none());

    // 13, 14. The rows the mapping was prepared from gone: asked, never
    // written again unasked. Keep Current opens the ordinary system,
    // there being no previous result to keep, and is remembered; the
    // rebuild asks again, and Update writes the rows again.
    let rows_path = crate::cache::artwork_pack_system_path(&app.cache_dir, "NES");
    std::fs::remove_file(&rows_path).unwrap();
    open(&mut app);
    assert_prompt(&app, changed);
    app.handle(Action::Quit);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert!(
        app.source_job.is_none() && app.provider_job.is_none(),
        "nothing is prepared unasked: {:?}",
        app.message
    );
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(app.artwork_provider.is_none());
    app.enter(Place::Dir(games.clone()));
    assert_ordinary_rows(&app);
    assert!(!rows_path.exists(), "a No writes no rows");
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_some());
    assert!(state
        .declined
        .as_ref()
        .is_some_and(|declined| declined.cache_marker.is_none()));
    assert_eq!(
        app.source_label("NES"),
        format!("Automatic (Using: {SOURCE_GAMELIST})")
    );
    leave_system(&mut app);
    open(&mut app);
    assert!(
        app.pending.is_none(),
        "the No stands for the rows being gone"
    );
    assert!(app.source_job.is_none() && app.provider_job.is_none());
    assert_ordinary_rows(&app);
    app.rebuild_open_system();
    assert_prompt(&app, changed);
    app.handle(Action::Accept);
    assert!(app.source_job.is_some(), "{:?}", app.message);
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Artwork Pack prepared."));
    assert_pack_rows(&app);
    assert!(rows_path.exists());
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_some() && state.declined.is_none());
    assert_complete(&app, 3);
    // A corrupt archive in a Pack system is reported as the ZIP problem it
    // is, with the healthy games still prepared; it must never come back as
    // a missing prepared cache, which would hide the cause.
    let broken = games.join("Broken.zip");
    std::fs::write(&broken, b"not an archive").unwrap();
    let warning = format!(
        "NES: {}: skipped: zip archive is malformed: no valid end-of-directory record",
        broken.display()
    );
    // The log is shared and kept across runs: only what it gains here
    // counts, and the warning is the only line of its kind for this
    // fixture's archive path.
    let log_since = |from: usize| {
        let log = std::fs::read_to_string(crate::LOG_PATH).unwrap_or_default();
        log[from.min(log.len())..].to_string()
    };
    let recovery_line = format!("cache        recovery warning: {warning}");
    let log_before = log_since(0).len();
    app.start_build(true);
    app.finish_background_work_for_headless();
    let problems = app.index_terminal.clone().unwrap();
    assert_eq!(problems.state, "Finished With Problems");
    assert!(problems.problem.contains(&warning), "{}", problems.problem);
    assert!(
        !problems
            .problem
            .contains("prepared system cache is missing"),
        "{}",
        problems.problem
    );
    // A full build logs the warning once, as the line the terminal shows;
    // the recovery step must not log it a second time on the way there.
    let log = log_since(log_before);
    assert_eq!(
        log.lines().filter(|line| *line == warning).count(),
        1,
        "{log}"
    );
    assert!(
        !log.lines().any(|line| line == recovery_line),
        "a full build logged the warning twice: {log}"
    );
    assert_complete(&app, 3);
    assert_operation_controls(&mut app, false, "A Details   B Back");
    capture_live_if_requested(&mut app, "source-auto-pack-finished-with-problems");
    app.handle(Action::Quit);
    // Rebuild This System List on a Pack-prepared system runs through the
    // source worker rather than the index terminal, and has to say the
    // same thing: the archive and its reason, not a pointer to the log.
    app.message = None;
    let log_before = log_since(0).len();
    app.rebuild_open_system_resolved();
    app.finish_background_work_for_headless();
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.starts_with("NES list rebuilt with problems:\n"),
        "{message}"
    );
    assert!(message.contains(&warning), "{message}");
    // Without a terminal to drain them, the recovery step is the one
    // place this path logs the warning.
    let log = log_since(log_before);
    assert_eq!(
        log.lines().filter(|line| *line == recovery_line).count(),
        1,
        "{log}"
    );
    assert_complete(&app, 3);
    capture_live_if_requested(&mut app, "source-auto-pack-rebuilt-with-problems");
    std::fs::remove_file(&broken).unwrap();
    app.message = None;

    // An ordinary rebuild that fails leaves the prepared result alone.
    leave_system(&mut app);
    let before_failure = cache_snapshot(&app.cache_dir);
    let held = root.join("held-games");
    std::fs::rename(&games, &held).unwrap();
    std::fs::write(&games, b"not a directory").unwrap();
    app.start_build(true);
    app.finish_background_work_for_headless();
    assert_eq!(
        app.index_terminal.as_ref().unwrap().state,
        "Finished With Problems"
    );
    assert_eq!(cache_snapshot(&app.cache_dir), before_failure);
    app.handle(Action::Quit);
    std::fs::remove_file(&games).unwrap();
    std::fs::rename(&held, &games).unwrap();

    // 20. Gamelist chosen: no check and no question, even where a check
    // would fail.
    app.source_system_id = Some("NES".into());
    app.begin_source_switch(crate::source_cache::Target::Gamelist);
    app.finish_background_work_for_headless();
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        crate::artwork_source::Mode::Gamelist
    );
    assert!(!app.effective_artwork_pack_roots.contains_key("NES"));
    std::fs::rename(&docs, root.join("docs-away")).unwrap();
    std::fs::write(&docs, b"not a directory").unwrap();
    app.message = None;
    open(&mut app);
    assert!(
        app.pending.is_none() && app.message.is_none(),
        "{:?}",
        app.message
    );
    assert_ordinary_rows(&app);
    std::fs::remove_file(&docs).unwrap();
    std::fs::rename(root.join("docs-away"), &docs).unwrap();
    leave_system(&mut app);

    // Automatic chosen again: the acceptance written down comes back
    // without preparing anything. Chosen from inside the system while
    // the Pack's storage is gone, the system is shown on its ordinary
    // rows behind the unavailable message, as an entry shows it.
    open(&mut app);
    assert_ordinary_rows(&app);
    std::fs::rename(&docs, root.join("docs-away")).unwrap();
    app.open_game_data_source();
    app.menu_list.select(0);
    app.handle(Action::Accept);
    assert!(app.source_job.is_none() && app.provider_job.is_none());
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        crate::artwork_source::Mode::Automatic
    );
    assert!(app.effective_artwork_pack_roots.contains_key("NES"));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("is unavailable")),
        "{:?}",
        app.message
    );
    assert_eq!(
        app.artwork_provider.as_ref().map(|p| p.health),
        Some(crate::artwork_pack::ProviderHealth::Unavailable)
    );
    assert_eq!(app.here[0].name, "Known.nes", "no Pack data is substituted");
    std::fs::rename(root.join("docs-away"), &docs).unwrap();
    app.screen = Screen::Browse;
    leave_system(&mut app);
    app.open_game_data_source();
    assert_eq!(
        app.menu[0],
        format!("Automatic (Using: {SOURCE_ARTWORK_PACK})")
    );
    app.screen = Screen::Browse;
    open(&mut app);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_pack_rows(&app);

    // 19. Artwork Pack chosen explicitly is the consent: prepared as the
    // switch, no question, and written down like any preparation.
    leave_system(&mut app);
    std::fs::remove_dir_all(app.cache_dir.join("artwork-pack")).unwrap();
    app.effective_artwork_pack_roots.clear();
    app.artwork_provider_cache.clear();
    // A switch whose settings cannot be saved after its result was
    // installed says what stands: the prepared data, used under the
    // Automatic that remains in force.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&root).unwrap().permissions().mode();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        app.source_system_id = Some("NES".into());
        app.begin_source_switch(crate::source_cache::Target::ArtworkPack {
            docs_root: docs.clone(),
        });
        assert!(app.source_job.is_some());
        app.finish_background_work_for_headless();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
        let message = app.message.clone().expect("a failed save is said");
        assert!(message.starts_with(
            "Artwork Pack could not be enabled for NES.\n\nAutomatic remains active and is currently using Gamelist."
        ), "{message}");
        assert!(
            message.ends_with("The prepared Artwork Pack data was saved for a later retry."),
            "{message}"
        );
        assert_eq!(
            crate::artwork_source::mode(&app.settings, "NES"),
            crate::artwork_source::Mode::Automatic
        );
        assert!(pack_files(&app.cache_dir, "NES")
            .iter()
            .all(Option::is_some));
        app.screen = Screen::Browse;
        app.message = None;
        open(&mut app);
        assert!(app.pending.is_none(), "{:?}", app.message);
        assert_pack_rows(&app);
        leave_system(&mut app);
        std::fs::remove_dir_all(app.cache_dir.join("artwork-pack")).unwrap();
        app.effective_artwork_pack_roots.clear();
        app.artwork_provider_cache.clear();
    }
    app.source_system_id = Some("NES".into());
    app.begin_source_switch(crate::source_cache::Target::ArtworkPack {
        docs_root: docs.clone(),
    });
    assert!(app.source_job.is_some());
    assert!(app.pending.is_none());
    app.finish_background_work_for_headless();
    assert_eq!(
        app.message.as_deref(),
        Some(&*format!("Now using {SOURCE_ARTWORK_PACK}."))
    );
    assert!(pack_files(&app.cache_dir, "NES")
        .iter()
        .all(Option::is_some));
    app.message = None;
    open(&mut app);
    assert!(app.pending.is_none());
    assert_pack_rows(&app);
    app.ui.hide().unwrap();
    drop(app);

    // 11. Two systems on one catalogue: each is asked about on its own,
    // and only the one that said yes is prepared.
    let shared = root.join("shared-group");
    let shared_games = shared.join("games/NEOGEO");
    let shared_docs = shared.join("docs");
    let shared_artwork = shared_docs.join("NEOGEO/Artwork");
    for directory in [&shared_games, &shared_artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(shared_games.join("One.neo"), b"first rom").unwrap();
    std::fs::write(
        shared_artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nOne\tbox-2D\t142\n",
    )
    .unwrap();
    std::fs::write(
        shared_artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nOne\t\t\tOne\n",
    )
    .unwrap();
    std::fs::write(
        shared_artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nOne\tPack One\t1990\tAction\tStudio\t1\n",
    )
    .unwrap();
    std::fs::write(shared_artwork.join("One.jpg"), crate::covers::JPEG_16).unwrap();
    let mut app = unopened_fixture_app_with_systems(
        &shared,
        window.clone(),
        Settings::default(),
        &["NeoGeoMVS", "NeoGeo"],
        "games/NEOGEO",
    );
    app.pack_candidate_bases = vec![shared.clone()];
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(!app.cache_dir.join("artwork-pack").exists());
    app.open_system_by_index(1);
    assert_eq!(
        app.message.as_deref(),
        Some("Artwork Pack Available\n\nAn installed Artwork Pack was found for Neo Geo. Prepare its artwork and metadata now?\n\nA Prepare   B Not Now")
    );
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert_eq!(app.source_progress.systems, 1);
    assert_eq!(
        app.open_system.as_deref(),
        Some("NeoGeo"),
        "{:?}",
        app.message
    );
    app.enter(Place::Dir(shared_games.clone()));
    assert_eq!(app.here[0].name, "Pack One");
    assert!(pack_files(&app.cache_dir, "NeoGeo")
        .iter()
        .all(Option::is_some));
    assert!(
        pack_files(&app.cache_dir, "NeoGeoMVS")
            .iter()
            .all(Option::is_none),
        "the other member is not prepared on this one's yes"
    );
    assert!(app.effective_artwork_pack_roots.contains_key("NeoGeo"));
    assert!(!app.effective_artwork_pack_roots.contains_key("NeoGeoMVS"));
    assert_eq!(
        app.source_label("NeoGeoMVS"),
        format!("Automatic (Using: {SOURCE_GAMELIST})")
    );
    leave_system(&mut app);
    app.open_system_by_index(0);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Artwork Pack Available") && m.contains("Neo Geo MVS")),
        "{:?}",
        app.message
    );
    assert!(matches!(app.pending, Some(Pending::PrepareArtworkPack(_))));
    app.handle(Action::Quit);
    assert_eq!(app.open_system.as_deref(), Some("NeoGeoMVS"));
    app.enter(Place::Dir(shared_games.clone()));
    assert_eq!(app.here[0].name, "One.neo");
    // Rebuild All reads both members the ordinary way: the accepted one
    // into its Pack cache without its Pack, the state and mapping left
    // alone, the other one into its ordinary file. The accepted member's
    // next entry asks about its rows; the other one is not asked again
    // for the Pack it declined.
    leave_system(&mut app);
    let prepared_neogeo = pack_files(&app.cache_dir, "NeoGeo");
    app.start_build(true);
    assert!(app.source_recovery_queue.is_empty());
    app.finish_background_work_for_headless();
    assert_eq!(app.index_terminal.as_ref().unwrap().state, "Complete");
    assert_eq!(app.index_terminal.as_ref().unwrap().done, 2);
    let rebuilt_neogeo = pack_files(&app.cache_dir, "NeoGeo");
    assert!(rebuilt_neogeo.iter().all(Option::is_some));
    assert_ne!(rebuilt_neogeo[0], prepared_neogeo[0]);
    assert_eq!(rebuilt_neogeo[1], prepared_neogeo[1]);
    assert_eq!(rebuilt_neogeo[2], prepared_neogeo[2]);
    assert!(pack_files(&app.cache_dir, "NeoGeoMVS")[1].is_some());
    assert!(pack_files(&app.cache_dir, "NeoGeoMVS")[0].is_none());
    assert!(crate::cache::system_path(&app.cache_dir, "NeoGeoMVS").exists());
    app.handle(Action::Quit);
    app.open_system_by_index(1);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.starts_with("Artwork Pack Changed") && m.contains("Neo Geo")),
        "{:?}",
        app.message
    );
    assert!(matches!(app.pending, Some(Pending::UpdateArtworkPack(_))));
    app.handle(Action::Up);
    app.open_system_by_index(0);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_eq!(app.open_system.as_deref(), Some("NeoGeoMVS"));
    app.ui.hide().unwrap();
}

/// The table's entry for one system, found over one games folder under
/// the root, as `unopened_fixture_app` builds it.
fn fixture_system(root: &Path, id: &str, games: &str) -> FoundSystem {
    let def = crate::systems::parse_table(
        include_str!("../assets/systems.toml"),
        Path::new("systems.toml"),
    )
    .unwrap()
    .into_iter()
    .find(|system| system.id == id)
    .unwrap();
    FoundSystem {
        def,
        paths: vec![root.join(games)],
        logo_dir: None,
        menu_folder: None,
    }
}

/// A Pack cache as the previous release wrote it: complete rows and no
/// state beside them. Installed with the index so the start reads the
/// card as an upgraded installation does.
fn install_legacy_pack_cache(cache_dir: &Path, system: &FoundSystem) -> Vec<u8> {
    let library = Library::open_source_neutral(&system.to_config(), Default::default()).unwrap();
    let cache = crate::cache::build_system(&library);
    let mut index = crate::cache::Index::new();
    index
        .systems
        .insert(system.def.id.clone(), cache.summary(&library.start()));
    crate::cache::save_index(cache_dir, &index).unwrap();
    crate::cache::install_transactional(
        cache_dir,
        crate::cache::CacheKind::ArtworkPack,
        &[crate::cache::StagedSystemCache {
            id: system.def.id.clone(),
            cache,
            fingerprints: Default::default(),
            fingerprints_complete: true,
        }],
    )
    .unwrap();
    std::fs::read(crate::cache::artwork_pack_system_path(
        cache_dir,
        &system.def.id,
    ))
    .unwrap()
}

/// An installation upgraded from the release that kept no state beside
/// its Pack caches: nothing is done to them at startup, each is adopted
/// by the one system entry that first needs it, and a cache that cannot
/// be tied to the Pack that is installed now is kept and asked about.
fn run_legacy_pack_cache_adoption_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let write_pack = |docs: &Path| {
        let artwork = docs.join("NES/Artwork");
        std::fs::create_dir_all(&artwork).unwrap();
        std::fs::write(
            artwork.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nKnown\tbox-2D\t3\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\n",
        )
        .unwrap();
        std::fs::write(
            artwork.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nKnown\tPack First\t1990\tAction\tStudio\t1\n",
        )
        .unwrap();
        std::fs::write(artwork.join("Known.jpg"), crate::covers::JPEG_16).unwrap();
        artwork
    };

    // 23. An explicit choice from before: the cache validates against
    // the live files once, on the worker, and the state is written; from
    // then on the entry reads the state alone. No question: the choice
    // was the consent.
    let root = root.join("legacy-explicit");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    std::fs::write(games.join("Known.nes"), b"first rom").unwrap();
    let docs = root.join("docs");
    let artwork = write_pack(&docs);
    let settings_path = root.join("settings.toml");
    let mut settings = Settings::default();
    settings
        .artwork_pack_roots
        .insert("NES".into(), docs.to_string_lossy().into_owned());
    settings.save(&settings_path).unwrap();
    let legacy_bytes = install_legacy_pack_cache(
        &crate::cache::dir_for(&settings_path),
        &fixture_system(&root, "NES", "games/NES"),
    );
    let mut app = unopened_fixture_app(
        &root,
        window.clone(),
        Settings::load(&settings_path).unwrap(),
    );
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    assert!(
        pack_files(&app.cache_dir, "NES")[1].is_none(),
        "startup writes no state for a cache it has not been asked to open"
    );
    assert!(app.provider_job.is_none() && app.provider_requests.is_empty());
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(
        app.pending.is_none(),
        "an explicit choice is not asked again"
    );
    assert!(app.message.is_none(), "{:?}", app.message);
    assert_eq!(app.here[0].name, "Pack First");
    assert_eq!(app.here[0].cover, Some(artwork.join("Known.jpg")));
    assert!(log_since(logged_from).contains("artwork pack NES: state written"));
    let [rows, decision, prepared] = pack_files(&app.cache_dir, "NES");
    assert_eq!(
        rows,
        Some(legacy_bytes),
        "the cache is adopted, not rewritten"
    );
    assert!(decision.is_some() && prepared.is_some());
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert_eq!(
        state
            .accepted
            .as_ref()
            .map(|accepted| accepted.docs_root.as_str()),
        Some(docs.to_str().unwrap())
    );
    leave_system(&mut app);
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    assert!(
        app.provider_job.is_none(),
        "the reused state opens the system without a worker read"
    );
    assert_eq!(app.here[0].name, "Pack First");
    // A worker read started to open the system opens it when it is done,
    // whatever an earlier read in the process was started for.
    leave_system(&mut app);
    app.artwork_provider_cache.clear();
    app.provider_pending_then = OfferFollowUp::RebuildSystem;
    app.wait_for_artwork_provider("NES", OfferFollowUp::OpenSystem);
    assert!(app.provider_job.is_some());
    app.finish_background_work_for_headless();
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(
        app.source_job.is_none() && app.build.is_none(),
        "no rebuild was asked for"
    );
    app.ui.hide().unwrap();
    drop(app);

    // 24. Automatic from before, with a cache that still matches its
    // installed Pack: adopted on the first entry, without a question.
    let root = root.with_file_name("legacy-automatic");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    std::fs::write(games.join("Known.nes"), b"first rom").unwrap();
    let docs = root.join("docs");
    let artwork = write_pack(&docs);
    let legacy_bytes = install_legacy_pack_cache(
        &root.join("cache"),
        &fixture_system(&root, "NES", "games/NES"),
    );
    let mut app = unopened_fixture_app(&root, window.clone(), Settings::default());
    app.pack_candidate_bases = vec![root.clone()];
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    assert!(pack_files(&app.cache_dir, "NES")[1].is_none());
    assert!(!app.effective_artwork_pack_roots.contains_key("NES"));
    // A read that ends without the state written, here because the
    // store cannot be written, gives the root back: the system is not
    // held as Pack-selected on a Pack nothing validated, and the next
    // entry reads it again.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let store = app.cache_dir.join("artwork-pack");
        let mode = std::fs::metadata(&store).unwrap().permissions().mode();
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555)).unwrap();
        app.open_system_by_index(0);
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(app.open_system.is_none(), "{:?}", app.message);
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.starts_with("Artwork Pack read failed; system not opened.")),
            "{:?}",
            app.message
        );
        assert!(
            !app.effective_artwork_pack_roots.contains_key("NES"),
            "the root taken for the adoption is given back"
        );
        assert_eq!(
            app.source_label("NES"),
            format!("Automatic (Using: {SOURCE_GAMELIST})")
        );
        assert!(pack_files(&app.cache_dir, "NES")[1].is_none());
        app.message = None;
    }
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(app.pending.is_none(), "{:?}", app.message);
    assert_eq!(app.here[0].name, "Pack First");
    assert_eq!(app.here[0].cover, Some(artwork.join("Known.jpg")));
    assert!(log_since(logged_from).contains("artwork pack NES: adopting legacy cache"));
    assert!(log_since(logged_from).contains("artwork pack NES: state written"));
    assert_eq!(pack_files(&app.cache_dir, "NES")[0], Some(legacy_bytes));
    assert_eq!(
        app.source_label("NES"),
        format!("Automatic (Using: {SOURCE_ARTWORK_PACK})")
    );
    leave_system(&mut app);
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert!(log_since(logged_from).contains("artwork pack NES: state reused"));
    assert_eq!(app.here[0].name, "Pack First");
    app.ui.hide().unwrap();
    drop(app);

    // 24. Automatic from before, with a cache that no longer tells the
    // truth about its games: kept as it is and asked about. There is no
    // current result to keep, so Keep Current opens the ordinary system
    // and is remembered; Rebuild This System List is the way to prepare
    // it, and it asks once more first.
    let root = root.with_file_name("legacy-automatic-changed");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    std::fs::write(games.join("Known.nes"), b"first rom").unwrap();
    std::fs::write(games.join("Extra.nes"), b"a game the pack does not know").unwrap();
    let docs = root.join("docs");
    let artwork = write_pack(&docs);
    let legacy_bytes = install_legacy_pack_cache(
        &root.join("cache"),
        &fixture_system(&root, "NES", "games/NES"),
    );
    let mut app = unopened_fixture_app(&root, window.clone(), Settings::default());
    app.pack_candidate_bases = vec![root.clone()];
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert!(
        matches!(app.pending, Some(Pending::UpdateArtworkPack(_))),
        "{:?}",
        app.message
    );
    assert_eq!(
        app.message.as_deref(),
        Some("Artwork Pack Changed\n\nThe installed Artwork Pack data for NES has changed. Update its prepared artwork and metadata now?\n\nA Update   B Keep Current"),
        "the same question a changed Pack raises"
    );
    assert!(log_since(logged_from).contains("artwork pack NES: prompt shown: changed"));
    assert_eq!(
        pack_files(&app.cache_dir, "NES")[0].as_deref(),
        Some(legacy_bytes.as_slice()),
        "the old cache is preserved while the question is up"
    );
    assert!(pack_files(&app.cache_dir, "NES")[1].is_none());
    app.handle(Action::Quit);
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(
        app.artwork_provider.is_none(),
        "Keep Current on a cache that cannot be tied to the Pack opens without it"
    );
    assert_eq!(app.here.len(), 2, "{:?}", app.here);
    assert_eq!(app.here[0].name, "Extra.nes");
    assert_eq!(app.here[1].name, "Known.nes");
    assert!(app.here.iter().all(|row| row.cover.is_none()));
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_none());
    assert!(state.declined.is_some());
    assert_eq!(
        pack_files(&app.cache_dir, "NES")[0],
        Some(legacy_bytes.clone())
    );
    leave_system(&mut app);
    app.open_system_by_index(0);
    assert!(
        app.pending.is_none(),
        "the kept cache is not asked about again"
    );
    assert!(app.provider_job.is_none());
    assert_eq!(app.here.len(), 2);
    // An Update that fails leaves the system as it was, on its ordinary
    // rows, and says so without claiming a previous result is in use:
    // none was.
    let manifest = artwork.join("manifest.tsv");
    let good_manifest = std::fs::read(&manifest).unwrap();
    std::fs::write(&manifest, "not-a-valid-manifest\n").unwrap();
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert!(
        matches!(app.pending, Some(Pending::UpdateArtworkPack(_))),
        "{:?}",
        app.message
    );
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    let message = app.message.clone().expect("a failed Update is said");
    assert!(
        message.starts_with("NES Artwork Pack was not prepared."),
        "{message}"
    );
    assert!(
        !message.contains("previous complete cache"),
        "no Pack result was in use: {message}"
    );
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    assert!(app.artwork_provider.is_none());
    assert_eq!(
        pack_files(&app.cache_dir, "NES")[0],
        Some(legacy_bytes.clone())
    );
    std::fs::write(&manifest, &good_manifest).unwrap();
    app.message = None;
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert!(
        matches!(app.pending, Some(Pending::UpdateArtworkPack(_))),
        "the rebuild is the retry and asks once more: {:?}",
        app.message
    );
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Artwork Pack prepared."));
    assert!(app.artwork_provider.is_some());
    assert!(pack_files(&app.cache_dir, "NES")
        .iter()
        .all(Option::is_some));
    assert_ne!(pack_files(&app.cache_dir, "NES")[0], Some(legacy_bytes));
    let known = app
        .here
        .iter()
        .find(|row| row.cover.is_some())
        .expect("the known game is enriched");
    assert_eq!(known.name, "Pack First");
    assert_eq!(known.cover, Some(artwork.join("Known.jpg")));
    assert!(app
        .here
        .iter()
        .any(|row| row.name == "Extra.nes" && row.cover.is_none()));
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "NES")
        .unwrap()
        .unwrap();
    assert!(state.accepted.is_some() && state.declined.is_none());
    app.ui.hide().unwrap();
    drop(app);

    // 24. Two systems on one catalogue: one prepared under this release,
    // one carrying a cache from before. Adopting the second reads
    // nothing for the first and writes nothing over its state, so the
    // Pack change made in between is still asked about when the first
    // is entered. Read on a fresh start, as an upgrade is: no provider
    // is in memory.
    let root = root.with_file_name("legacy-shared-group");
    let games = root.join("games/NEOGEO");
    let artwork = root.join("docs/NEOGEO/Artwork");
    for directory in [&games, &artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(games.join("One.neo"), b"first rom").unwrap();
    std::fs::write(
        artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nOne\tbox-2D\t142\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nOne\t\t\tOne\n",
    )
    .unwrap();
    let gameinfo = artwork.join("gameinfo.tsv");
    std::fs::write(
        &gameinfo,
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nOne\tPack One\t1990\tAction\tStudio\t1\n",
    )
    .unwrap();
    std::fs::write(artwork.join("One.jpg"), crate::covers::JPEG_16).unwrap();
    let mut app = unopened_fixture_app_with_systems(
        &root,
        window.clone(),
        Settings::default(),
        &["NeoGeoMVS", "NeoGeo"],
        "games/NEOGEO",
    );
    app.pack_candidate_bases = vec![root.clone()];
    app.leave_splash();
    app.finish_background_work_for_headless();
    app.open_system_by_index(0);
    assert!(matches!(app.pending, Some(Pending::PrepareArtworkPack(_))));
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert_eq!(
        app.open_system.as_deref(),
        Some("NeoGeoMVS"),
        "{:?}",
        app.message
    );
    let prepared_mvs = pack_files(&app.cache_dir, "NeoGeoMVS");
    assert!(prepared_mvs.iter().all(Option::is_some));
    let cache_dir = app.cache_dir.clone();
    app.ui.hide().unwrap();
    drop(app);
    // The other member's cache from before, complete and without state,
    // and a Pack table edited after the first member was prepared.
    let neogeo = fixture_system(&root, "NeoGeo", "games/NEOGEO");
    let library = Library::open_source_neutral(&neogeo.to_config(), Default::default()).unwrap();
    crate::cache::install_transactional(
        &cache_dir,
        crate::cache::CacheKind::ArtworkPack,
        &[crate::cache::StagedSystemCache {
            id: "NeoGeo".into(),
            cache: crate::cache::build_system(&library),
            fingerprints: Default::default(),
            fingerprints_complete: true,
        }],
    )
    .unwrap();
    std::fs::write(
        &gameinfo,
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nOne\tPack One Renamed\t1990\tAction\tStudio\t1\n",
    )
    .unwrap();
    let mut app = unopened_fixture_app_with_systems(
        &root,
        window.clone(),
        Settings::default(),
        &["NeoGeoMVS", "NeoGeo"],
        "games/NEOGEO",
    );
    app.pack_candidate_bases = vec![root.clone()];
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none());
    assert!(app.artwork_provider_cache.is_empty());
    let logged_from = log_len();
    app.open_system_by_index(1);
    assert_eq!(
        app.open_system.as_deref(),
        Some("NeoGeo"),
        "{:?}",
        app.message
    );
    assert!(app.pending.is_none(), "{:?}", app.message);
    let logged = log_since(logged_from);
    assert!(logged.contains("artwork pack NeoGeo: adopting legacy cache"));
    assert!(logged.contains("artwork pack NeoGeo: state written"));
    assert!(
        !logged.contains("artwork pack NeoGeoMVS"),
        "the prepared member is not read on the other one's adoption: {logged}"
    );
    assert_eq!(
        pack_files(&app.cache_dir, "NeoGeoMVS"),
        prepared_mvs,
        "the prepared member's files are not written over"
    );
    app.enter(Place::Dir(games.clone()));
    assert_eq!(app.here[0].name, "Pack One Renamed");
    leave_system(&mut app);
    app.open_system_by_index(0);
    assert!(
        matches!(app.pending, Some(Pending::UpdateArtworkPack(_))),
        "the prepared member's own entry sees the change: {:?}",
        app.message
    );
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.starts_with("Artwork Pack Changed")));
    app.ui.hide().unwrap();
}

/// An arcade folder with one of every broken descriptor beside the
/// healthy ones and a set whose bare components live under the core's
/// home: the Pack is prepared for every healthy entry, each broken one
/// stays listed as it is, the report on screen counts them without a
/// path, and the fast path is silent about them. A repaired descriptor
/// is matched by Rebuild This System List; a Pack root that is gone or
/// a cache folder that cannot be written fails the rebuild whole and
/// leaves every file as it was. The ROM repository beside the
/// descriptors is never traversed.
#[cfg(unix)]
fn run_arcade_descriptor_failures_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    use std::os::unix::fs::PermissionsExt;
    let root = root.join("arcade-descriptors");
    let arcade = root.join("_Arcade");
    let home = root.join("games/Battletoads");
    let mame = root.join("games/mame");
    let docs = root.join("docs");
    let artwork = docs.join("Arcade/Artwork");
    for directory in [&arcade, &home, &mame, &artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(
        mame.join("healthy.zip"),
        crate::zip::tests_archive(&["healthy.rom"], false),
    )
    .unwrap();
    std::fs::set_permissions(&mame, std::fs::Permissions::from_mode(0o000)).unwrap();
    for name in ["btc0-p0.bin", "btc0-p1.bin", "btc0-s.bin"] {
        std::fs::write(home.join(name), b"payload").unwrap();
    }
    std::fs::write(arcade.join("btc0-s.bin"), b"decoy beside the descriptor").unwrap();
    let set = arcade.join("Battletoads.mgl");
    std::fs::write(
        &set,
        "<mistergamedescription>\n\t<rbf>_Arcade/cores/Battletoads</rbf>\n\t<setname>Battletoads</setname>\n\t\
         <file delay=\"1\" type=\"f\" index=\"0\" path=\"btc0-p0.bin\"/>\n\t\
         <file delay=\"1\" type=\"f\" index=\"1\" path=\"btc0-p1.bin\"/>\n\t\
         <file delay=\"1\" type=\"f\" index=\"2\" path=\"btc0-s.bin\"/>\n\
         </mistergamedescription>\n",
    )
    .unwrap();
    let healthy = arcade.join("Healthy.mra");
    let healthy_xml = "<misterromdescription><setname>healthy</setname></misterromdescription>";
    std::fs::write(&healthy, healthy_xml).unwrap();
    // A descriptor whose game is not there: the descriptor reads, the
    // game it names does not.
    let gone = arcade.join("Gone.mgl");
    std::fs::write(
        &gone,
        format!(
            "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
            root.join("nowhere.rom").display()
        ),
    )
    .unwrap();
    let dangling = arcade.join("Dangling.mra");
    std::os::unix::fs::symlink(root.join("gone.mra"), &dangling).unwrap();
    let sealed = arcade.join("Sealed.mra");
    std::fs::write(&sealed, healthy_xml).unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let sealed_readable = std::fs::read(&sealed).is_ok();
    let broken = arcade.join("Broken.mra");
    std::fs::write(&broken, "<misterromdescription><rom></wrong>").unwrap();
    let cycle = arcade.join("Cycle.mgl");
    std::fs::write(
        &cycle,
        format!(
            "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
            cycle.display()
        ),
    )
    .unwrap();
    let chain: Vec<PathBuf> = (0..=8)
        .map(|index| arcade.join(format!("Chain-{index}.mgl")))
        .collect();
    for (index, mgl) in chain.iter().enumerate() {
        let target = chain.get(index + 1).unwrap_or(&healthy);
        std::fs::write(
            mgl,
            format!(
                "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
                target.display()
            ),
        )
        .unwrap();
    }
    std::fs::write(
        artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nhealthy\tbox-2D\t75\nBattletoads\tbox-2D\t75\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nhealthy\t\t\thealthy\nBattletoads\t\t\tBattletoads\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nhealthy\tPack Healthy\t1990\tShooter\tStudio\t2\nBattletoads\tPack Battletoads\t1994\tBeat 'em up\tStudio\t2\n",
    )
    .unwrap();
    for key in ["healthy", "Battletoads"] {
        std::fs::write(artwork.join(format!("{key}.jpg")), crate::covers::JPEG_16).unwrap();
    }

    let mut app = unopened_fixture_app_with_systems(
        &root,
        window,
        Settings::default(),
        &["Arcade"],
        "_Arcade",
    );
    app.pack_candidate_bases = vec![root.clone()];
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(app.build.is_none(), "{:?}", app.message);
    let listed = app.index.as_ref().unwrap().systems["Arcade"].games;
    assert_eq!(
        listed, 16,
        "every descriptor is a game to the ordinary index"
    );

    let name_of = |app: &App, path: &Path| -> browse::Row {
        app.here
            .iter()
            .find(|row| row_target(row).as_deref() == Some(path))
            .unwrap_or_else(|| panic!("{} is listed: {:?}", path.display(), app.here))
            .clone()
    };
    let assert_enriched = |app: &App, path: &Path, name: &str, key: &str| {
        let row = name_of(app, path);
        assert_eq!(row.name, name, "{row:?}");
        assert_eq!(row.cover, Some(artwork.join(format!("{key}.jpg"))));
    };
    let assert_plain = |app: &App, path: &Path| {
        let row = name_of(app, path);
        assert_eq!(
            row.name,
            path.file_stem().unwrap().to_string_lossy(),
            "a skipped row keeps its own name: {row:?}"
        );
        assert_eq!(row.cover, None);
    };

    // 26. Prepared with problems: the healthy entries enriched, the broken
    // ones counted on screen without a path, every row still listed.
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert!(matches!(app.pending, Some(Pending::PrepareArtworkPack(_))));
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert!(app.source_job.is_none());
    assert_eq!(
        app.open_system.as_deref(),
        Some("Arcade"),
        "{:?}",
        app.message
    );
    let message = app.message.clone().expect("the report");
    assert!(
        message.starts_with("Artwork Pack prepared with problems:\n"),
        "{message}"
    );
    capture_live_if_requested(&mut app, "source-auto-pack-prepared-with-problems");
    assert!(
        !message.contains('/'),
        "no path reaches the screen: {message}"
    );
    assert!(message.contains("Arcade: 2 games left without Pack data: missing file"));
    assert!(message.contains("Arcade: 1 game left without Pack data: malformed descriptor"));
    assert!(message.contains("Arcade: 2 games left without Pack data: invalid redirect chain"));
    assert_eq!(
        message.contains("inaccessible file"),
        !sealed_readable,
        "a sealed file is inaccessible to anyone but root: {message}"
    );
    let logged = log_since(logged_from);
    for (path, category) in [
        (&gone, "missing file"),
        (&dangling, "missing file"),
        (&broken, "malformed descriptor"),
        (&cycle, "invalid redirect chain"),
        (&chain[0], "invalid redirect chain"),
    ] {
        assert!(
            logged.contains(&format!(
                "pack entry   {}: skipped: {category}:",
                path.display()
            )),
            "{}: {logged}",
            path.display()
        );
    }
    assert!(
        !logged.contains("_Arcade/btc0-s.bin"),
        "nothing is looked for beside the set descriptor: {logged}"
    );
    assert_eq!(app.here.len(), listed, "every row is still listed");
    assert_enriched(&app, &healthy, "Pack Healthy", "healthy");
    assert_enriched(&app, &set, "Pack Battletoads", "Battletoads");
    assert_enriched(&app, &chain[1], "Pack Healthy", "healthy");
    for path in [&gone, &dangling, &broken, &cycle, &chain[0]] {
        assert_plain(&app, path);
    }
    let files = pack_files(&app.cache_dir, "Arcade");
    assert!(files.iter().all(Option::is_some));
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "Arcade")
        .unwrap()
        .unwrap();
    let expected_skipped = if sealed_readable { 5 } else { 6 };
    assert_eq!(
        state.accepted.as_ref().unwrap().skipped_entries,
        expected_skipped
    );
    assert_eq!(
        state.accepted.as_ref().unwrap().health,
        crate::artwork_pack::ProviderHealth::Ready,
        "a broken descriptor is not a degraded Pack"
    );

    // The fast path says nothing about them again.
    leave_system(&mut app);
    let logged_from = log_len();
    app.open_system_by_index(0);
    assert!(
        app.message.is_none() && app.pending.is_none(),
        "{:?}",
        app.message
    );
    assert!(app.source_job.is_none() && app.provider_job.is_none());
    assert!(log_since(logged_from).contains("artwork pack Arcade: state reused"));
    assert!(!log_since(logged_from).contains(&format!("pack entry   {}", arcade.display())));
    assert_plain(&app, &broken);
    assert_enriched(&app, &healthy, "Pack Healthy", "healthy");

    // Repaired and rebuilt: matched like any other.
    std::fs::write(root.join("nowhere.rom"), b"rom").unwrap();
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nhealthy\t\t\thealthy\nBattletoads\t\t\tBattletoads\nnowhere\t\t\thealthy\n",
    )
    .unwrap();
    std::fs::remove_file(&dangling).unwrap();
    std::fs::write(&dangling, healthy_xml).unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&broken, healthy_xml).unwrap();
    std::fs::write(
        &cycle,
        format!(
            "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
            healthy.display()
        ),
    )
    .unwrap();
    std::fs::write(
        &chain[0],
        format!(
            "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
            healthy.display()
        ),
    )
    .unwrap();
    app.rebuild_open_system();
    assert!(app.source_job.is_some(), "{:?}", app.message);
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Arcade list rebuilt."));
    for path in [&gone, &dangling, &sealed, &broken, &cycle, &chain[0]] {
        assert_enriched(&app, path, "Pack Healthy", "healthy");
    }
    let state = crate::cache::load_pack_source_state(&app.cache_dir, "Arcade")
        .unwrap()
        .unwrap();
    assert_eq!(state.accepted.as_ref().unwrap().skipped_entries, 0);

    // 27. One component of the set actually gone from its home: only that
    // entry is left without Pack data, at the home path; back, matched.
    std::fs::remove_file(home.join("btc0-p1.bin")).unwrap();
    app.message = None;
    let logged_from = log_len();
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert_eq!(
        app.message.as_deref(),
        Some("Arcade list rebuilt with problems:\nArcade: 1 game left without Pack data: missing file")
    );
    let logged = log_since(logged_from);
    assert!(
        logged.contains(&format!(
            "pack entry   {}: skipped: missing file:",
            set.display()
        )) && logged.contains(&home.join("btc0-p1.bin").display().to_string()),
        "{logged}"
    );
    assert!(!logged.contains("_Arcade/btc0"), "{logged}");
    assert_plain(&app, &set);
    assert_enriched(&app, &healthy, "Pack Healthy", "healthy");
    std::fs::write(home.join("btc0-p1.bin"), b"payload").unwrap();
    app.message = None;
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Arcade list rebuilt."));
    assert_enriched(&app, &set, "Pack Battletoads", "Battletoads");
    let complete = pack_files(&app.cache_dir, "Arcade");

    // The Pack root gone is the whole system's failure: nothing replaced.
    std::fs::rename(&docs, root.join("docs-away")).unwrap();
    app.message = None;
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert_eq!(
        app.message.as_deref(),
        Some("Arcade list was not rebuilt.\nArtwork Pack unavailable: reconnect its storage and try again.\nThe previous complete cache remains in use.")
    );
    assert_eq!(pack_files(&app.cache_dir, "Arcade"), complete);
    assert_enriched(&app, &healthy, "Pack Healthy", "healthy");
    std::fs::rename(root.join("docs-away"), &docs).unwrap();

    // A cache folder that cannot be written is the same: nothing replaced.
    let store = app.cache_dir.join("artwork-pack");
    std::fs::rename(&store, app.cache_dir.join("artwork-pack.held")).unwrap();
    std::fs::write(&store, b"not a directory").unwrap();
    app.message = None;
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    let message = app.message.clone().unwrap();
    assert!(
        message
            .starts_with("Arcade list was not rebuilt.\nCheck the selected storage and try again."),
        "{message}"
    );
    assert!(!store.is_dir());
    std::fs::remove_file(&store).unwrap();
    std::fs::rename(app.cache_dir.join("artwork-pack.held"), &store).unwrap();
    assert_eq!(pack_files(&app.cache_dir, "Arcade"), complete);
    app.message = None;
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Arcade list rebuilt."));
    assert_enriched(&app, &set, "Pack Battletoads", "Battletoads");
    std::fs::set_permissions(&mame, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.ui.hide().unwrap();
}

fn run_auto_source_choice_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    use crate::artwork_source::Mode;

    let root = root.join("automatic-source-choice");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(games.join(name), b"fixture game").unwrap();
    }
    let mut app = fixture_app(&root, window.clone(), Settings::default());
    assert!(!games.join("gamelist.xml").exists());
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        Mode::Automatic
    );
    assert!(!app.effective_artwork_pack_roots.contains_key("NES"));
    assert_eq!(
        app.source_label("NES"),
        format!("Automatic (Using: {SOURCE_GAMELIST})")
    );
    let initial_cache = cache_snapshot(&app.cache_dir);
    let initial_rows: Vec<_> = app.here.iter().map(row_key).collect();
    app.open_game_data_source();
    assert_eq!(app.screen, Screen::GameDataSource);
    assert_eq!(app.menu.len(), 3);
    assert_eq!(app.menu_list.selected(), 0);
    for (selected, name) in ["source-automatic", "source-gamelist", "source-artwork-pack"]
        .into_iter()
        .enumerate()
    {
        app.menu_list.select(selected);
        assert_eq!(app.menu_list.selected(), selected);
        assert_eq!(
            crate::artwork_source::mode(&app.settings, "NES"),
            Mode::Automatic
        );
        if let Some(directory) = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR") {
            let directory = PathBuf::from(directory);
            assert!(directory.is_absolute() && directory.is_dir());
            capture_frame(&mut app, &directory, name, 352, 240);
        }
    }
    app.menu_list.select(0);

    app.begin_source_switch(crate::source_cache::Target::Gamelist);
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        Mode::Gamelist
    );
    assert!(app.settings.gamelist_sources.contains("NES"));
    assert!(
        app.source_job.is_none(),
        "saving an unchanged effective source must not rebuild caches"
    );
    assert!(app.source_resolution.is_none());
    assert_eq!(cache_snapshot(&app.cache_dir), initial_cache);
    assert_eq!(
        app.here.iter().map(row_key).collect::<Vec<_>>(),
        initial_rows
    );
    assert!(
        app.message.is_none(),
        "Gamelist choice failed: {:?}",
        app.message
    );
    let saved = Settings::load(&app.settings_path).unwrap();
    assert!(saved.gamelist_sources.contains("NES"));
    app.ui.hide().unwrap();
    drop(app);

    let mut app = fixture_app(&root, window, saved);
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        Mode::Gamelist
    );
    assert!(!app.effective_artwork_pack_roots.contains_key("NES"));
    assert!(!games.join("gamelist.xml").exists());
    let restarted_cache = cache_snapshot(&app.cache_dir);
    app.open_game_data_source();
    assert_eq!(app.menu_list.selected(), 1);
    if let Some(directory) = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR") {
        capture_frame(
            &mut app,
            &PathBuf::from(directory),
            "source-explicit-gamelist",
            352,
            240,
        );
    }
    app.menu_list.select(0);
    app.handle(Action::Accept);
    // Choosing Automatic is a saved choice and nothing else: no worker,
    // no Pack looked for, no cache touched. Whether a Pack is used is
    // decided when the system is entered.
    assert!(
        app.source_resolution.is_none() && app.source_job.is_none() && app.provider_job.is_none(),
        "Automatic starts no work of its own"
    );
    assert_eq!(app.screen, Screen::GameDataSource);
    assert_eq!(app.menu[0], format!("Automatic (Using: {SOURCE_GAMELIST})"));
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        Mode::Automatic
    );
    assert!(!app.settings.gamelist_sources.contains("NES"));
    assert!(!Settings::load(&app.settings_path)
        .unwrap()
        .gamelist_sources
        .contains("NES"));
    assert!(app.source_job.is_none());
    assert!(app.source_resolution.is_none());
    assert_eq!(cache_snapshot(&app.cache_dir), restarted_cache);
    assert!(
        app.message.is_none(),
        "Automatic choice failed: {:?}",
        app.message
    );

    app.begin_source_switch(crate::source_cache::Target::Gamelist);
    let before_failure = app.settings.clone();
    let effective_before_failure = app.effective_artwork_pack_roots.clone();
    let settings_path = app.settings_path.clone();
    let blocked_parent = root.join("blocked-settings-parent");
    std::fs::write(&blocked_parent, b"not a directory").unwrap();
    app.settings_path = blocked_parent.join("settings.toml");
    app.source_system_id = Some("NES".into());
    app.choose_automatic_source();
    app.finish_background_work_for_headless();
    assert_eq!(
        app.settings, before_failure,
        "a failed save must preserve the explicit mode"
    );
    assert_eq!(app.effective_artwork_pack_roots, effective_before_failure);
    assert_eq!(cache_snapshot(&app.cache_dir), restarted_cache);
    assert!(app.message.is_some(), "save failure must remain visible");
    assert!(Settings::load(&settings_path)
        .unwrap()
        .gamelist_sources
        .contains("NES"));
    app.settings_path = settings_path;

    app.artwork_source_errors
        .insert("NES".into(), "fixture source metadata failure".into());
    assert!(
        app.source_problem("SNES").is_none(),
        "source failures are isolated by group"
    );
    let rows_before_error: Vec<_> = app.here.iter().map(row_key).collect();
    app.open_system_now();
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.contains("fixture source metadata failure")));
    assert_eq!(
        app.here.iter().map(row_key).collect::<Vec<_>>(),
        rows_before_error
    );
    assert_eq!(cache_snapshot(&app.cache_dir), restarted_cache);
    assert!(app.source_job.is_none());
    assert!(app.provider_job.is_none());

    // Changing the game data source scans the system again, and an archive
    // that scan leaves out has to be named in the completion message with
    // its reason, as every other completion is: a pointer to the log would
    // leave the person at the screen without the cause.
    let docs = root.join("docs");
    let artwork = docs.join("NES/Artwork");
    std::fs::create_dir_all(&artwork).unwrap();
    std::fs::write(
        artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nFirst Game\tbox-2D\t3\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nFirst Game\t\t\tFirst Game\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\n",
    )
    .unwrap();
    std::fs::write(artwork.join("First Game.jpg"), crate::covers::JPEG_16).unwrap();
    let broken = games.join("Broken.zip");
    std::fs::write(&broken, b"not an archive").unwrap();
    app.open_game_data_source();
    app.begin_source_switch(crate::source_cache::Target::ArtworkPack {
        docs_root: docs.clone(),
    });
    assert!(app.source_job.is_some(), "a new source must be prepared");
    app.finish_background_work_for_headless();
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.starts_with("Now using Artwork Pack.\nFinished with problems:\n"),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "NES: {}: skipped: zip archive is malformed: no valid end-of-directory record",
            broken.display()
        )),
        "{message}"
    );
    capture_live_if_requested(&mut app, "source-switch-finished-with-problems");
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        Mode::ArtworkPack
    );
    assert_eq!(
        crate::cache::load_artwork_pack_data(&app.cache_dir, "NES")
            .unwrap()
            .cache
            .summary(&Place::Dir(games.clone()))
            .games,
        2,
        "the healthy games are prepared beside the skipped archive"
    );
    std::fs::remove_file(&broken).unwrap();
    app.ui.hide().unwrap();
}

fn run_degraded_pack_acknowledgement_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    use crate::artwork_pack::ProviderHealth;
    use crate::pack_health::Acknowledgements;

    let root = root.join("degraded-pack-acknowledgement");
    let games = root.join("games/NES");
    let docs = root.join("docs");
    let artwork = docs.join("NES/Artwork");
    for directory in [&games, &artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(games.join("Known.nes"), b"first rom").unwrap();
    std::fs::write(games.join("Second.nes"), b"second rom").unwrap();
    let write_manifest = |keys: &[&str]| {
        let rows: String = keys
            .iter()
            .map(|key| format!("{key}\tbox-2D\t3\n"))
            .collect();
        std::fs::write(
            artwork.join("manifest.tsv"),
            format!("#key\tstyle\tss_system_id\n{rows}"),
        )
        .unwrap();
    };
    write_manifest(&["Known", "Second"]);
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\nSecond\t\t\tSecond\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nKnown\tPack First\t1990\tAction\tStudio\t1\nSecond\tPack Second\t1991\tPuzzle\tStudio\t2\n",
    )
    .unwrap();
    for key in ["Known", "Second"] {
        std::fs::write(artwork.join(format!("{key}.jpg")), crate::covers::JPEG_16).unwrap();
    }
    let settings_path = root.join("settings.toml");
    let warnings = crate::pack_health::path_beside(&settings_path);
    assert_eq!(warnings.parent(), settings_path.parent());
    let mut settings = Settings::default();
    settings
        .artwork_pack_roots
        .insert("NES".into(), docs.to_string_lossy().into_owned());
    settings.save(&settings_path).unwrap();
    // Every start reads the card afresh, as a return from a game does.
    // A Pack whose tables changed since it was prepared is asked about
    // on entry; an update is the answer every step of this flow wants.
    let answer_update = |app: &mut App| {
        assert!(
            matches!(app.pending, Some(Pending::UpdateArtworkPack(_))),
            "the changed Pack is asked about: {:?}",
            app.message
        );
        app.handle(Action::Accept);
        app.finish_background_work_for_headless();
    };
    let start = |window: Rc<MinimalSoftwareWindow>| {
        let mut app = unopened_fixture_app(&root, window, Settings::load(&settings_path).unwrap());
        app.open_system_by_index(0);
        // The wordmark takes the first press; the question needs the next.
        app.leave_splash();
        if matches!(app.pending, Some(Pending::UpdateArtworkPack(_))) {
            answer_update(&mut app);
        }
        assert!(
            app.source_resolution.is_none() && app.source_job.is_none(),
            "a headless open must also finish the resolution that a cache recovery restarts"
        );
        assert!(app.build.is_none());
        assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
        app.leave_splash();
        app
    };
    let message_contains = |app: &App, text: &str| {
        app.message
            .as_deref()
            .is_some_and(|message| message.contains(text))
    };
    // The overlay is small and names no file: the cause of a file that
    // could not be read or written stays in the log. The cause is the one
    // the read itself gives, whatever words it uses.
    let names_the_file = |app: &App, cause: &str| {
        message_contains(app, &warnings.display().to_string()) || message_contains(app, cause)
    };
    let malformed_cause = || {
        Acknowledgements::load(&warnings)
            .unwrap()
            .malformed()
            .expect("the file written as broken must read as malformed")
            .to_string()
    };
    let reopen = |app: &mut App| {
        app.handle(Action::Quit);
        assert!(
            app.open_system.is_none(),
            "B on the top folder leaves the system"
        );
        app.open_system_by_index(0);
        assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    };
    let acknowledged = |group: &str, digest: &str| {
        Acknowledgements::load(&warnings)
            .unwrap()
            .acknowledged(group, digest)
    };
    // The log is one file for every process on the host, appended to by
    // whatever else runs; counted lines carry this fixture's path, so only
    // this flow adds to them. Read as bytes: a stray byte from elsewhere is
    // no reason to fail, and a log that cannot be read at all is named. A
    // log nobody has started yet holds no lines.
    let logged = |line: &str| {
        let bytes = match std::fs::read(crate::LOG_PATH) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("{} could not be read: {error}", crate::LOG_PATH),
        };
        String::from_utf8_lossy(&bytes).matches(line).count()
    };
    // Writing the log is best effort and never stops the program, so a
    // host where the file cannot be written would fail the counts below
    // as if nothing had been logged. One probe line first: a missing probe
    // is the environment, not the acknowledgement.
    let probe = format!(
        "degraded pack acknowledgement flow probe at {}",
        docs.display()
    );
    let probed_before = logged(&probe);
    crate::note(&probe);
    assert_eq!(
        logged(&probe),
        probed_before + 1,
        "{} must take what note() appends on this host; the log counts below depend on it",
        crate::LOG_PATH
    );

    // 6. A complete pack: no warning, and nothing written down.
    let app = start(window.clone());
    assert_eq!(
        app.artwork_provider.as_ref().unwrap().health,
        ProviderHealth::Ready,
        "{:?}",
        app.artwork_provider.as_ref().unwrap().diagnostics
    );
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(
        !warnings.exists(),
        "a Ready pack must not create the acknowledgement file"
    );
    app.ui.hide().unwrap();
    drop(app);

    // 1. The first look at an incomplete pack warns. An image gone is
    // not a change to the tables: the entry stands on what it prepared,
    // and the Pack is read again when its list is rebuilt. Putting the
    // warning up is not seeing it: a headless render opens a system the
    // same way and nobody dismisses what it draws, so nothing is written
    // down yet.
    std::fs::remove_file(artwork.join("Second.jpg")).unwrap();
    let mut app = start(window.clone());
    assert_eq!(
        app.artwork_provider.as_ref().unwrap().health,
        ProviderHealth::Ready,
        "an image removed is read on demand, not as a new pack state"
    );
    assert!(app.message.is_none(), "{:?}", app.message);
    app.rebuild_open_system();
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("NES list rebuilt."));
    app.handle(Action::Accept);
    assert!(app.message.is_none());
    reopen(&mut app);
    assert_eq!(
        app.artwork_provider.as_ref().unwrap().health,
        ProviderHealth::Degraded
    );
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    let first_digest = app.artwork_provider.as_ref().unwrap().health_digest();
    assert!(
        !warnings.exists(),
        "a warning nobody dismissed must not be written down"
    );
    app.ui.hide().unwrap();
    drop(app);

    // A press that takes another message off the screen, put up over the
    // warning meanwhile, has not seen the warning: nothing is written down.
    let mut app = start(window.clone());
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    app.message = Some("Put up over the warning before any press".into());
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(
        !warnings.exists(),
        "dismissing another message is not seeing the warning under it"
    );
    app.ui.hide().unwrap();
    drop(app);

    // The same pack in the next process: still not seen, so warned again;
    // dismissing it is what writes it down.
    let mut app = start(window.clone());
    assert!(
        message_contains(&app, "is incomplete"),
        "an undismissed warning must come back: {:?}",
        app.message
    );
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(
        acknowledged("NES", &first_digest),
        "the dismissed warning is written down beside settings.toml"
    );
    let first_file = std::fs::read_to_string(&warnings).unwrap();

    // 2. Dismissed, left and re-entered in the same process: not again.
    reopen(&mut app);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert_eq!(std::fs::read_to_string(&warnings).unwrap(), first_file);

    // 4. A new diagnostic on the same pack, noticed on re-entry, is a new
    // warning even while the process that saw the old one is still running.
    // A table gone is a change: asked about, and updated.
    std::fs::remove_file(artwork.join("index.tsv")).unwrap();
    app.handle(Action::Quit);
    assert!(app.open_system.is_none());
    app.open_system_by_index(0);
    answer_update(&mut app);
    assert_eq!(app.open_system.as_deref(), Some("NES"), "{:?}", app.message);
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    assert!(
        message_contains(&app, "Artwork Pack prepared."),
        "the warning stays up with the report under it: {:?}",
        app.message
    );
    let no_index_digest = app.artwork_provider.as_ref().unwrap().health_digest();
    assert_ne!(no_index_digest, first_digest);
    assert!(app
        .artwork_provider
        .as_ref()
        .unwrap()
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("index.tsv is missing")));
    assert_eq!(
        std::fs::read_to_string(&warnings).unwrap(),
        first_file,
        "nothing is written until the new warning is dismissed"
    );
    app.handle(Action::Accept);
    assert!(acknowledged("NES", &no_index_digest));
    assert!(
        !acknowledged("NES", &first_digest),
        "one acknowledgement per source group: the old state is gone"
    );
    let no_index_file = std::fs::read_to_string(&warnings).unwrap();
    let no_index_diagnostics = app.artwork_provider.as_ref().unwrap().diagnostics.clone();
    assert_eq!(
        no_index_diagnostics.len(),
        2,
        "a missing image and a missing table are two diagnostics: {no_index_diagnostics:?}"
    );
    app.ui.hide().unwrap();
    drop(app);

    // 3. Back from a game: a new process, the same pack, no warning. The
    // log still gets the whole diagnostic, every part of it, acknowledged
    // or not: the screen is quiet, the record is not. The warning just
    // dismissed wrote the same line, so only one more line proves it.
    let logged_line = format!(
        "artwork pack Degraded at {}: {}",
        docs.display(),
        no_index_diagnostics.join("; ")
    );
    let logged_before = logged(&logged_line);
    let app = start(window.clone());
    assert!(
        app.message.is_none(),
        "an unchanged degraded pack must not warn again after a restart: {:?}",
        app.message
    );
    let provider = app.artwork_provider.as_ref().unwrap();
    assert_eq!(provider.health, ProviderHealth::Degraded);
    assert_eq!(provider.diagnostics, no_index_diagnostics);
    assert_eq!(
        logged(&logged_line),
        logged_before + 1,
        "an acknowledged warning must still be written to {} in full: {logged_line:?}",
        crate::LOG_PATH
    );
    assert_eq!(std::fs::read_to_string(&warnings).unwrap(), no_index_file);
    app.ui.hide().unwrap();
    drop(app);

    // 4. Pack content updated without a new diagnostic: a well-formed row
    // added to gameinfo.tsv changes only the source fingerprint, and that
    // alone brings the warning back in a new process. Meanwhile the file
    // on the card breaks after the warning went up, so the warning could
    // not say so: the press that writes over it says it instead, on screen
    // and in the log.
    std::fs::write(
        artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nKnown\tPack First\t1990\tAction\tStudio\t1\nSecond\tPack Second\t1991\tPuzzle\tStudio\t2\nExtra\tPack Extra\t1992\tAction\tStudio\t1\n",
    )
    .unwrap();
    let malformed_line = |cause: &str| {
        format!(
            "artwork pack warnings: {} is malformed: {cause}",
            warnings.display()
        )
    };
    let mut app = start(window.clone());
    assert!(
        message_contains(&app, "is incomplete"),
        "changed content with an unchanged diagnostic is a warning not yet seen: {:?}",
        app.message
    );
    assert!(
        !message_contains(&app, "could not be read"),
        "the file was whole when the warning went up: {:?}",
        app.message
    );
    let provider = app.artwork_provider.as_ref().unwrap();
    assert_eq!(
        provider.diagnostics, no_index_diagnostics,
        "the diagnostic is unchanged; only the content is"
    );
    let extra_digest = provider.health_digest();
    assert_ne!(extra_digest, no_index_digest);
    std::fs::write(&warnings, "degraded = \"not a table").unwrap();
    let cause = malformed_cause();
    let malformed_before = logged(&malformed_line(&cause));
    app.handle(Action::Accept);
    assert!(
        message_contains(&app, "remembered")
            && message_contains(&app, "could not be read and was replaced"),
        "a file that broke between the warning and the press is said at the press: {:?}",
        app.message
    );
    assert!(
        !names_the_file(&app, &cause),
        "the press says the file was replaced, not the parse error or the path: {:?}",
        app.message
    );
    assert_eq!(
        logged(&malformed_line(&cause)),
        malformed_before + 1,
        "the replacement of a file that broke meanwhile must reach {} with its cause",
        crate::LOG_PATH
    );
    assert!(
        acknowledged("NES", &extra_digest),
        "the broken file is replaced by a readable one"
    );
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    app.ui.hide().unwrap();
    drop(app);

    // 4. An updated manifest is another pack state: warned about once more.
    // Meanwhile the file on the card was replaced by hand while this
    // process runs (another group's acknowledgement, this one's gone): the
    // save must start from what is on the card, not from what was read
    // earlier, or the deletion is undone and the other group's entry lost.
    write_manifest(&["Known", "Second", "Third"]);
    let mut app = start(window.clone());
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    let updated_digest = app.artwork_provider.as_ref().unwrap().health_digest();
    assert_ne!(updated_digest, extra_digest);
    let mut by_hand = Acknowledgements::default();
    by_hand.acknowledge("SNES", "kept");
    by_hand.save(&warnings).unwrap();
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(acknowledged("NES", &updated_digest));
    assert!(
        acknowledged("SNES", "kept"),
        "an entry written by hand while the program runs must survive the next save"
    );
    assert!(!acknowledged("NES", &extra_digest));
    app.ui.hide().unwrap();
    drop(app);

    // 7. A broken acknowledgement file costs one more warning, nothing
    // else; that warning says the file was set aside, and why, because the
    // press that dismisses it writes over what was there.
    std::fs::write(&warnings, "degraded = \"not a table").unwrap();
    let cause = malformed_cause();
    let malformed_before = logged(&malformed_line(&cause));
    let mut app = start(window.clone());
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    assert!(
        message_contains(&app, "could not be read and will be replaced"),
        "a file set aside as malformed must be said on screen: {:?}",
        app.message
    );
    assert!(
        !names_the_file(&app, &cause),
        "the parse error and the path belong in the log, not on the overlay: {:?}",
        app.message
    );
    app.handle(Action::Accept);
    assert!(
        acknowledged("NES", &updated_digest),
        "the broken file is replaced by a readable one"
    );
    assert_eq!(
        logged(&malformed_line(&cause)),
        malformed_before + 1,
        "one broken file is one line in {}, with its cause, not one per read of it",
        crate::LOG_PATH
    );
    let acknowledged_file = std::fs::read_to_string(&warnings).unwrap();
    app.ui.hide().unwrap();
    drop(app);

    // A file that is there but cannot be read is another matter: it may
    // hold acknowledgements, so the warning is shown with the reason and
    // nothing is written over it.
    std::fs::remove_file(&warnings).unwrap();
    std::fs::create_dir(&warnings).unwrap();
    let cause = Acknowledgements::load(&warnings)
        .expect_err("a directory in the file's place cannot be read")
        .to_string();
    let mut app = start(window.clone());
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    assert!(
        message_contains(&app, "cannot be remembered"),
        "an unreadable file must be said on screen: {:?}",
        app.message
    );
    assert!(
        !names_the_file(&app, &cause),
        "the read error and the path belong in the log, not on the overlay: {:?}",
        app.message
    );
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(
        warnings.is_dir(),
        "an unreadable file must not be replaced by a fresh list"
    );
    app.ui.hide().unwrap();
    drop(app);

    // A dismissal whose acknowledgement cannot be written down is said on
    // screen with its cause, and the warning comes back on the next start.
    // The file is read again before the save, so a file turned into a
    // directory after the warning went up fails at that read.
    std::fs::remove_dir(&warnings).unwrap();
    let mut app = start(window.clone());
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    std::fs::create_dir(&warnings).unwrap();
    app.handle(Action::Accept);
    assert!(
        message_contains(&app, "not remembered")
            && message_contains(&app, "reading artwork pack warnings failed for"),
        "a failed acknowledgement must say which step failed: {:?}",
        app.message
    );
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    assert!(warnings.is_dir());
    app.ui.hide().unwrap();
    drop(app);
    std::fs::remove_dir(&warnings).unwrap();
    std::fs::write(&warnings, &acknowledged_file).unwrap();
    let app = start(window.clone());
    assert!(
        app.message.is_none(),
        "the acknowledgement put back reads as before: {:?}",
        app.message
    );
    app.ui.hide().unwrap();
    drop(app);

    // 5. A pack that turned invalid is a change that cannot be prepared:
    // said at every start with the previous complete result kept in use,
    // and never written down. An unavailable pack is said at every start
    // as well.
    write_manifest(&["../escape"]);
    let complete = cache_snapshot(&crate::cache::dir_for(&settings_path).join("artwork-pack"));
    for _ in 0..2 {
        let mut app = start(window.clone());
        assert_eq!(
            app.artwork_provider.as_ref().unwrap().health,
            ProviderHealth::Degraded,
            "the previous complete result stays in use"
        );
        let message = app.message.clone().expect("the failure is said");
        assert!(
            message.starts_with("NES Artwork Pack was not prepared.")
                && message.ends_with("The previous complete cache remains in use."),
            "{message}"
        );
        assert_eq!(
            cache_snapshot(&app.cache_dir.join("artwork-pack")),
            complete,
            "a failed update replaces nothing"
        );
        app.handle(Action::Accept);
        app.ui.hide().unwrap();
    }
    std::fs::remove_dir_all(&artwork).unwrap();
    for _ in 0..2 {
        let mut app = start(window.clone());
        assert_eq!(
            app.artwork_provider.as_ref().unwrap().health,
            ProviderHealth::Unavailable
        );
        assert!(
            message_contains(&app, "is unavailable"),
            "{:?}",
            app.message
        );
        app.handle(Action::Accept);
        app.ui.hide().unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&warnings).unwrap(),
        acknowledged_file,
        "actionable failures leave the incomplete-pack acknowledgement alone"
    );

    // Two systems, one source: NeoGeo and NeoGeoMVS read the same NEOGEO
    // pack, so its warning is one warning. Dismissed from one member, it
    // must stay dismissed for the other in the next process.
    let shared = root.join("shared-group");
    let shared_games = shared.join("games/NEOGEO");
    let shared_docs = shared.join("docs");
    let shared_artwork = shared_docs.join("NEOGEO/Artwork");
    for directory in [&shared_games, &shared_artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(shared_games.join("One.neo"), b"first rom").unwrap();
    std::fs::write(shared_games.join("Two.neo"), b"second rom").unwrap();
    std::fs::write(
        shared_artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nOne\tbox-2D\t142\nTwo\tbox-2D\t142\n",
    )
    .unwrap();
    std::fs::write(
        shared_artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nOne\t\t\tOne\nTwo\t\t\tTwo\n",
    )
    .unwrap();
    std::fs::write(
        shared_artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nOne\tPack One\t1990\tAction\tStudio\t1\nTwo\tPack Two\t1991\tPuzzle\tStudio\t2\n",
    )
    .unwrap();
    std::fs::write(shared_artwork.join("One.jpg"), crate::covers::JPEG_16).unwrap();
    let shared_settings_path = shared.join("settings.toml");
    let shared_warnings = crate::pack_health::path_beside(&shared_settings_path);
    let mut shared_settings = Settings::default();
    shared_settings
        .artwork_pack_roots
        .insert("NeoGeo".into(), shared_docs.to_string_lossy().into_owned());
    shared_settings.save(&shared_settings_path).unwrap();
    let start_member = |window: Rc<MinimalSoftwareWindow>, index: usize, id: &str| {
        let mut app = unopened_fixture_app_with_systems(
            &shared,
            window,
            Settings::load(&shared_settings_path).unwrap(),
            &["NeoGeoMVS", "NeoGeo"],
            "games/NEOGEO",
        );
        app.open_system_by_index(index);
        assert!(app.source_resolution.is_none() && app.source_job.is_none());
        assert!(app.build.is_none());
        assert_eq!(app.open_system.as_deref(), Some(id), "{:?}", app.message);
        assert_eq!(
            app.artwork_provider.as_ref().unwrap().health,
            ProviderHealth::Degraded,
            "{:?}",
            app.artwork_provider.as_ref().unwrap().diagnostics
        );
        app.leave_splash();
        app
    };
    let mut app = start_member(window.clone(), 0, "NeoGeoMVS");
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
    let group_digest = app.artwork_provider.as_ref().unwrap().health_digest();
    app.handle(Action::Accept);
    assert!(app.message.is_none(), "{:?}", app.message);
    let group_seen = Acknowledgements::load(&shared_warnings).unwrap();
    assert!(
        group_seen.acknowledged("NeoGeo", &group_digest),
        "the acknowledgement is written under the source group, not the system"
    );
    assert!(!group_seen.acknowledged("NeoGeoMVS", &group_digest));
    app.ui.hide().unwrap();
    drop(app);
    let app = start_member(window.clone(), 1, "NeoGeo");
    assert_eq!(
        app.artwork_provider.as_ref().unwrap().health_digest(),
        group_digest,
        "both members of the group read the same pack as the same snapshot"
    );
    assert!(
        app.message.is_none(),
        "a warning dismissed from the other member of the group must not come back: {:?}",
        app.message
    );
    app.ui.hide().unwrap();
    drop(app);
}

/// An arcade descriptor with its ROM set spelled out as bare files, kept
/// under `_Arcade` while the files live under `games/<setname>`, as the
/// public packs install it. The application has to match it to its Pack by
/// its own name, hold a favourite for it, and give that favourite the same
/// artwork, without ever looking beside the descriptor.
fn run_arcade_core_descriptor_favourite_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("arcade-core-descriptor");
    let arcade = root.join("_Arcade");
    let set = root.join("games/Battletoads");
    let docs = root.join("docs");
    let artwork = docs.join("Arcade/Artwork");
    for directory in [&arcade, &set, &artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    for name in ["btc0-p0.bin", "btc0-p1.bin", "btc0-s.bin"] {
        std::fs::write(set.join(name), b"payload").unwrap();
    }
    std::fs::write(arcade.join("btc0-s.bin"), b"decoy beside the descriptor").unwrap();
    let descriptor = arcade.join("Battletoads.mgl");
    std::fs::write(
        &descriptor,
        "<mistergamedescription>\n\t<rbf>_Arcade/cores/Battletoads</rbf>\n\t<setname>Battletoads</setname>\n\t\
         <file delay=\"1\" type=\"f\" index=\"0\" path=\"btc0-p0.bin\"/>\n\t\
         <file delay=\"1\" type=\"f\" index=\"1\" path=\"btc0-p1.bin\"/>\n\t\
         <file delay=\"1\" type=\"f\" index=\"2\" path=\"btc0-s.bin\"/>\n\
         </mistergamedescription>\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nBattletoads\tbox-2D\t75\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nBattletoads\t\t\tBattletoads\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("gameinfo.tsv"),
        "#key\tname\tyear\tgenre\tdeveloper\tplayers\nBattletoads\tPack Battletoads\t1994\tBeat 'em up\tStudio\t2\n",
    )
    .unwrap();
    std::fs::write(artwork.join("Battletoads.jpg"), crate::covers::JPEG_16).unwrap();
    let mut settings = Settings::default();
    settings
        .artwork_pack_roots
        .insert("Arcade".into(), docs.to_string_lossy().into_owned());
    let mut app =
        unopened_fixture_app_with_systems(&root, window, settings, &["Arcade"], "_Arcade");
    app.open_system_by_index(0);
    assert_eq!(
        app.open_system.as_deref(),
        Some("Arcade"),
        "{:?}",
        app.message
    );
    assert!(app.message.is_none(), "{:?}", app.message);
    let provider = app.artwork_provider.as_ref().expect("the Pack is selected");
    assert_eq!(
        provider.health,
        crate::artwork_pack::ProviderHealth::Ready,
        "{:?}",
        provider.diagnostics
    );
    app.leave_splash();
    assert_eq!(app.here.len(), 1, "{:?}", app.here);
    let row = app.here[0].clone();
    assert_eq!(row_target(&row).as_deref(), Some(descriptor.as_path()));
    assert_eq!(
        row.name, "Pack Battletoads",
        "the descriptor is matched by its own name, not by a component"
    );
    assert_eq!(
        row.cover.as_deref(),
        Some(artwork.join("Battletoads.jpg").as_path())
    );
    let logged = std::fs::read_to_string(crate::LOG_PATH).unwrap();
    assert!(
        !logged.contains(&arcade.join("btc0-s.bin").display().to_string()),
        "nothing is looked for beside the descriptor"
    );
    // The worker's own preparation of the descriptor, as a result rather
    // than a log line: the interface holds only the prepared rows, the
    // catalogue that matched them lives in the worker.
    let presentation = crate::artwork_pack::Provider::load("Arcade", &docs, None)
        .presentation_for_launch_with_fingerprints(
            &browse::Launch::File(descriptor.clone()),
            &crate::cache::ContentFingerprints::new(),
            &app.homes(),
        )
        .expect("the set's preparation does not fail on a bare component")
        .expect("the descriptor is matched");
    assert_eq!(presentation.name.as_deref(), Some("Pack Battletoads"));
    let matched = presentation
        .diagnostic
        .expect("a Pack match records how it was made");
    assert_eq!(
        (matched.key.as_str(), matched.method),
        ("Battletoads", crate::artwork_pack::MatchMethod::ExactKey),
        "the set is matched under the descriptor's own key, not a component's"
    );

    // Favourited the way the stock script favourites a core file: a link.
    let favorite_root = root.join("_@Favorites");
    std::fs::create_dir_all(&favorite_root).unwrap();
    let favorite =
        crate::favorites::add_core(&favorite_root, "Battletoads.mgl", &descriptor).unwrap();
    let mut favorites = app.all_systems[0].clone();
    favorites.def.id = "Favorites".into();
    favorites.def.name = "Favorites".into();
    favorites.def.category = Some("Favorites".into());
    favorites.paths = vec![favorite_root];
    app.all_systems.push(favorites);
    app.reread_favorites();
    assert!(
        app.favorites.holds(&descriptor),
        "the link is held under the descriptor"
    );
    assert_eq!(
        app.favorites.file_for(&descriptor),
        Some(favorite.as_path())
    );
    app.open_system = Some("Favorites".into());
    let mut shelf = vec![browse::Row {
        name: "Battletoads".into(),
        kind: browse::Kind::Play(browse::Launch::File(favorite.clone())),
        cover: None,
        ..row.clone()
    }];
    app.enrich_favorites(&mut shelf);
    assert_eq!(
        shelf[0].name, "Pack Battletoads",
        "the favourite is given the descriptor's own Pack match"
    );
    assert_eq!(
        shelf[0].cover.as_deref(),
        Some(artwork.join("Battletoads.jpg").as_path())
    );
    app.here = shelf;
    app.game_list = ListState::new(1, app.geometry.visible);
    assert!(app.in_favorites());
    let request = app.information_request(&app.here[0]).unwrap();
    assert_eq!(request.launch, browse::Launch::File(descriptor.clone()));
    assert!(
        matches!(
            request.source,
            crate::information_job::Source::ArtworkPack(_)
        ),
        "the owner is Arcade, whose source is the Pack"
    );

    // A console favourite whose bare name is in two of its system's
    // folders: pressing A must not start whichever MiSTer's folder order
    // finds first, but say which two files were found.
    let nes = root.join("games/NES");
    let famicom = root.join("games/Famicom");
    for folder in [&nes, &famicom] {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("Twice.nes"), b"a build").unwrap();
    }
    let mut console = app.all_systems[0].clone();
    console.def.id = "NES".into();
    console.def.name = "NES".into();
    console.def.category = None;
    console.def.rbf = "_Console/NES".into();
    console.def.extensions = vec!["nes".into(), "mgl".into()];
    console.paths = vec![nes.clone(), famicom.clone()];
    app.all_systems.push(console);
    let ambiguous = app.all_systems[1].paths[0].join("Twice.mgl");
    std::fs::write(
        &ambiguous,
        "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"Twice.nes\"/></mistergamedescription>",
    )
    .unwrap();
    app.reread_favorites();
    assert!(!app.favorites.holds(&nes.join("Twice.nes")));
    assert!(!app.favorites.holds(&famicom.join("Twice.nes")));
    app.here = vec![browse::Row {
        name: "Twice".into(),
        kind: browse::Kind::Play(browse::Launch::File(ambiguous.clone())),
        cover: None,
        ..row.clone()
    }];
    app.game_list = ListState::new(1, app.geometry.visible);
    assert!(
        app.confirm_launch().is_none(),
        "an ambiguous favourite stays in the interface"
    );
    let message = app.message.clone().expect("the launch says why");
    assert!(
        message.contains(&nes.join("Twice.nes").display().to_string())
            && message.contains(&famicom.join("Twice.nes").display().to_string()),
        "{message}"
    );
    app.ui.hide().unwrap();
}

/// A folder holding one game shows that game's picture before it is
/// opened, in the views that draw pictures, and stays a folder in every
/// other respect. Everything here goes through the same listing the
/// screen is drawn from, so a picture that reached the row but not the
/// view, or a folder that stopped opening, fails here.
fn run_folder_artwork_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("folder-artwork");
    let games = root.join("games/NES");
    let logos = root.join("logos");
    std::fs::create_dir_all(games.join("Example Game")).unwrap();
    std::fs::create_dir_all(games.join("media")).unwrap();
    std::fs::create_dir_all(&logos).unwrap();
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos/NES.png");
    let image = games.join("media/example.png");
    let second_image = games.join("media/second.png");
    let other_image = games.join("media/other.png");
    let logo = logos.join("NES.png");
    let custom = logos.join("custom.png");
    for target in [&image, &second_image, &other_image, &logo, &custom] {
        std::fs::copy(&shipped, target).unwrap();
    }
    std::fs::write(games.join("Example Game/Example Game.nes"), b"fixture").unwrap();
    std::fs::write(games.join("Second Game.nes"), b"fixture").unwrap();
    let write_gamelist = |extra: &str| {
        std::fs::write(
            games.join("gamelist.xml"),
            format!(
                "<gameList><game><path>./Example Game/Example Game.nes</path><name>Example Game</name><image>./media/example.png</image><genre>Platform</genre><publisher>Example Publisher</publisher></game><game><path>./Second Game.nes</path><name>Second Game</name><image>./media/second.png</image></game>{extra}</gameList>"
            ),
        )
        .unwrap();
    };
    write_gamelist("");
    let mut app = fixture_app(&root, window, Settings::default());
    app.logo_dir = Some(logos.clone());
    app.all_systems[0].logo_dir = Some(logos.clone());
    app.artwork_scale = ArtworkScale::FourThree;
    let folder_place = Place::Dir(games.join("Example Game"));
    let folder_at = |app: &App| {
        app.here
            .iter()
            .position(|row| row.kind == browse::Kind::Enter(folder_place.clone()))
            .expect("the folder is listed")
    };

    // Only the picture is taken: the row is still the folder, under its
    // own name, with its own count, and nothing about it is a favourite.
    let at = folder_at(&app);
    let folder = app.here[at].clone();
    assert!(folder.is_folder());
    assert_eq!(folder.cover.as_deref(), Some(image.as_path()));
    assert_eq!(folder.name, "Example Game");
    assert!(!folder.favorite);
    assert_eq!(folder.below, Some(1));
    assert_eq!(folder.details, browse::Details::default());
    assert_eq!(folder.genre, None);
    let game = app
        .system_cache
        .as_ref()
        .and_then(|cache| cache.get(&folder_place))
        .map(|folder| folder.rows[0].clone())
        .expect("the game inside is written down");
    assert_eq!(game.genre.as_deref(), Some("Platform"));
    assert_eq!(game.details.publisher, "Example Publisher");
    app.game_list.select(at);

    // Details draws it as game artwork; the grid and strip views draw it
    // on the row, corrected like a game's picture; the text views stay
    // text. The logo is taken away while the picture views are checked:
    // a folder without a picture is drawn with the system's logo, so
    // with one in reach a drawn row would not say which of the two it
    // carries.
    app.set_layout(Layout::Details);
    assert_eq!(
        app.current_art(),
        (Some(image.clone()), "Example Game".to_string(), false, true)
    );
    app.all_systems[0].logo_dir = None;
    for layout in [Layout::Tiled, Layout::Carousel, Layout::Gallery] {
        app.screen = Screen::Browse;
        app.set_layout(layout);
        app.load_art();
        app.refresh();
        let (range, _) = app.game_list.window();
        let row = app.rows.row_data(at - range.start).unwrap();
        assert!(
            row.has_cover,
            "{}: the folder row carries the picture, with no logo to stand in",
            layout.label()
        );
        assert_eq!(
            row.art_scale_x,
            artwork_horizontal(ArtworkScale::FourThree, app.width, app.height, true),
            "{}: the picture is corrected as game artwork",
            layout.label()
        );
        assert_eq!(row.title.as_str(), "[ Example Game ]");
    }
    app.all_systems[0].logo_dir = Some(logos.clone());
    for layout in [Layout::List, Layout::MultiList] {
        app.set_layout(layout);
        app.load_art();
        app.refresh();
        let (range, _) = app.game_list.window();
        let row = app.rows.row_data(at - range.start).unwrap();
        assert!(!row.has_cover, "{}: text only", layout.label());
        assert_eq!(row.art_scale_x, 1.0);
    }
    app.set_layout(Layout::Details);

    // Selecting it opens it, and coming back lands on it with its picture.
    app.handle(Action::Accept);
    assert_eq!(app.trail.len(), 2);
    assert_eq!(app.here.len(), 1);
    assert_eq!(app.here[0].cover.as_deref(), Some(image.as_path()));
    assert!(matches!(app.here[0].kind, browse::Kind::Play(_)));
    app.handle(Action::Quit);
    assert_eq!(app.trail.len(), 1);
    assert_eq!(app.game_list.selected(), folder_at(&app));
    assert_eq!(
        app.here[app.game_list.selected()].cover.as_deref(),
        Some(image.as_path())
    );

    // Hiding the only game takes its picture off the folder, which then
    // falls back to the logo; showing hidden rows does not put it back,
    // because they are still hidden; unhiding does.
    app.handle(Action::Accept);
    app.game_list.select(0);
    app.toggle_hidden();
    app.handle(Action::Quit);
    let at = folder_at(&app);
    app.game_list.select(at);
    assert_eq!(app.here[at].cover, None);
    assert_eq!(
        app.current_art(),
        (Some(logo.clone()), "Example Game".to_string(), false, false)
    );
    app.show_hidden = true;
    app.relist_here();
    let at = folder_at(&app);
    app.game_list.select(at);
    assert_eq!(app.here[at].cover, None, "shown, but still hidden");
    app.handle(Action::Accept);
    assert_eq!(app.here.len(), 1);
    app.game_list.select(0);
    app.toggle_hidden();
    assert!(app.settings.hidden_paths.is_empty());
    app.handle(Action::Quit);
    app.show_hidden = false;
    app.relist_here();
    let at = folder_at(&app);
    app.game_list.select(at);
    assert_eq!(app.here[at].cover.as_deref(), Some(image.as_path()));

    // A custom system image changes the system row and remains the fallback
    // for a folder without one clear game picture. It must not replace the
    // game artwork derived for this one-game folder.
    crate::category_images::install_system(&logos, "NES", &custom).unwrap();
    app.relist_here();
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover.as_deref(),
        Some(image.as_path()),
        "the custom system image does not replace derived folder artwork"
    );
    app.handle(Action::Quit);
    assert_eq!(app.browsing, Browsing::Systems);
    app.open_system_by_index(0);
    let at = folder_at(&app);
    app.game_list.select(at);
    assert_eq!(app.here[at].cover.as_deref(), Some(image.as_path()));
    assert_eq!(
        app.current_art(),
        (Some(image.clone()), "Example Game".to_string(), false, true)
    );
    assert!(crate::category_images::clear_system(&logos, "NES").unwrap());
    app.handle(Action::Quit);
    app.open_system_by_index(0);
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover.as_deref(),
        Some(image.as_path()),
        "clearing the custom image leaves the same derived folder artwork"
    );

    // Reading the system again is answered from the new cache, listed by
    // the reading itself: a second, different game in the folder takes
    // the picture away once Index All has run, and its removal gives it
    // back once the library refresh that follows a scrape has.
    std::fs::write(games.join("Example Game/Other Game.nes"), b"fixture").unwrap();
    write_gamelist("<game><path>./Example Game/Other Game.nes</path><name>Other Game</name><image>./media/other.png</image></game>");
    app.start_build(true);
    app.finish_background_work_for_headless();
    assert_eq!(app.index_terminal.as_ref().unwrap().state, "Complete");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    let at = folder_at(&app);
    assert_eq!(app.here[at].below, Some(2));
    assert_eq!(app.here[at].cover, None, "two games: the logo again");
    std::fs::remove_file(games.join("Example Game/Other Game.nes")).unwrap();
    write_gamelist("");
    app.screen = Screen::ScraperProgress;
    app.scraper_return = Screen::Browse;
    app.scraper_details = false;
    app.scraper_terminal = None;
    app.scraper_progress = crate::scraper::Progress {
        phase: crate::scraper::Phase::Finishing,
        scope: "Fixture System".into(),
        total: 1,
        completed: 1,
        updated: 1,
        updated_systems: vec!["NES".into()],
        ..Default::default()
    };
    app.begin_scraper_finish(ScraperTerminal::Finished);
    let refreshed = app
        .refreshing
        .take()
        .expect("a scrape that updated the system refreshes it");
    app.start_scraper_cache_refresh(refreshed);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.scraper_refresh_job.is_some() {
        assert!(
            Instant::now() < deadline,
            "the library refresh after the scrape did not finish"
        );
        app.poll_scraper_cache_refresh();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(app.scraper_terminal, Some(ScraperTerminal::Finished));
    assert_eq!(app.scraper_progress.system_errors, 0);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    let at = folder_at(&app);
    assert_eq!(app.here[at].below, Some(1));
    assert_eq!(
        app.here[at].cover.as_deref(),
        Some(image.as_path()),
        "one game again: its picture, listed by the refresh"
    );

    // With the Pack selected the folder's picture is the Pack's, asked
    // through the same listing the screen is drawn from. While the Pack
    // is not yet prepared, or cannot be used, the folder shows nothing:
    // the gamelist picture still on the card is not borrowed.
    let docs = root.join("docs");
    let art = docs.join("NES/Artwork");
    std::fs::create_dir_all(&art).unwrap();
    let pack_cover = art.join("Example Game.jpg");
    std::fs::write(&pack_cover, crate::covers::JPEG_16).unwrap();
    // The second game's picture keeps the Pack usable while the folder's
    // game has none, below; a Pack with no picture at all is refused.
    std::fs::write(art.join("Second Game.jpg"), crate::covers::JPEG_16).unwrap();
    std::fs::write(
        art.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nExample Game\tbox-2D\t3\nSecond Game\tbox-2D\t3\n",
    )
    .unwrap();
    std::fs::write(
        art.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nExample Game\t\t\tExample Game\nSecond Game\t\t\tSecond Game\n",
    )
    .unwrap();
    let pack_library = Library::open_source_neutral(
        &app.all_systems[0].to_config(),
        browse::DisplayNames::default(),
    )
    .unwrap();
    let pack_cache = crate::cache::build_system(&pack_library);
    let homes = crate::mgl::Homes::default();
    let prepare_provider = |provider: &mut crate::artwork_pack::Provider| {
        let mut skipped = crate::artwork_pack::SkippedEntries::new();
        provider.prepare_for_cache(
            &pack_cache,
            &crate::cache::ContentFingerprints::new(),
            &homes,
            &std::sync::atomic::AtomicBool::new(false),
            &mut skipped,
        )
    };
    assert!(image.is_file(), "the gamelist picture stays on the card");
    let mut provider = crate::artwork_pack::Provider::load("NES", &docs, Some("en"));
    assert!(provider.health.usable());
    app.artwork_provider = Some(provider.clone());
    app.relist_here();
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover, None,
        "a Pack not yet prepared: nothing, whichever rows are listed"
    );
    let gamelist_cache = app
        .system_cache
        .replace(pack_cache.clone())
        .expect("the gamelist cache was open");
    app.relist_here();
    let at = folder_at(&app);
    assert_eq!(app.here[at].cover, None, "nor over the Pack's own rows");
    prepare_provider(&mut provider)
        .unwrap()
        .expect("the Pack is prepared");
    app.artwork_provider = Some(provider);
    app.relist_here();
    let at = folder_at(&app);
    assert!(app.here[at].is_folder());
    assert_eq!(app.here[at].name, "Example Game");
    assert_eq!(app.here[at].cover.as_deref(), Some(pack_cover.as_path()));
    app.game_list.select(at);
    assert_eq!(
        app.current_art(),
        (
            Some(pack_cover.clone()),
            "Example Game".to_string(),
            false,
            true
        )
    );
    let mut unusable = crate::artwork_pack::Provider::load("NoSuchSystem", &docs, Some("en"));
    assert!(!unusable.health.usable());
    assert_eq!(
        prepare_provider(&mut unusable).unwrap(),
        Some(0),
        "an unusable Pack prepares no row"
    );
    app.artwork_provider = Some(unusable);
    app.relist_here();
    let at = folder_at(&app);
    assert_eq!(app.here[at].cover, None, "so it has nothing to answer");
    // A prepared Pack with no picture for the game, listed over the
    // gamelist rows, which do carry one: the walk asks only the Pack, so
    // the folder shows nothing rather than the rows' picture.
    std::fs::remove_file(&pack_cover).unwrap();
    let mut without = crate::artwork_pack::Provider::load("NES", &docs, Some("en"));
    assert!(without.health.usable());
    prepare_provider(&mut without)
        .unwrap()
        .expect("the Pack is prepared");
    assert!(without.covers_prepared());
    app.artwork_provider = Some(without);
    app.system_cache = Some(gamelist_cache);
    app.relist_here();
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover, None,
        "a Pack without the picture does not borrow the gamelist's"
    );
    app.artwork_provider = None;
    app.relist_here();
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover.as_deref(),
        Some(image.as_path()),
        "back on the gamelist, its picture again"
    );

    // The same change of source made the way it is made from the menu:
    // choosing the Pack lists the folder with the Pack's picture once the
    // switch has completed, reading the system again under the Pack
    // follows the Pack as it is now, and choosing the gamelist again
    // brings its picture back. Nothing here lists by hand.
    std::fs::write(&pack_cover, crate::covers::JPEG_16).unwrap();
    app.open_game_data_source();
    assert_eq!(app.screen, Screen::GameDataSource);
    app.begin_source_switch(crate::source_cache::Target::ArtworkPack {
        docs_root: docs.clone(),
    });
    assert!(app.source_job.is_some(), "the switch runs on its worker");
    app.finish_background_work_for_headless();
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(app.message.as_deref(), Some("Now using Artwork Pack."));
    assert!(app.artwork_provider.is_some());
    let at = folder_at(&app);
    assert!(app.here[at].is_folder());
    assert_eq!(
        app.here[at].cover.as_deref(),
        Some(pack_cover.as_path()),
        "the switch itself lists the folder with the Pack's picture"
    );
    std::fs::remove_file(&pack_cover).unwrap();
    app.rebuild_open_system_resolved();
    assert!(
        app.source_job.is_some(),
        "the Pack is read again on its worker"
    );
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("NES list rebuilt."));
    assert!(image.is_file(), "the gamelist picture is still on the card");
    let reread = app.artwork_provider.as_ref().expect("the Pack is attached");
    assert!(reread.health.usable() && reread.covers_prepared());
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover, None,
        "the Pack lost the picture, and the gamelist's is not borrowed"
    );
    app.open_game_data_source();
    app.begin_source_switch(crate::source_cache::Target::Gamelist);
    assert!(app.source_job.is_some());
    app.finish_background_work_for_headless();
    assert_eq!(app.message.as_deref(), Some("Now using Gamelist."));
    assert!(app.artwork_provider.is_none());
    let at = folder_at(&app);
    assert_eq!(
        app.here[at].cover.as_deref(),
        Some(image.as_path()),
        "the switch back lists the folder with the gamelist picture"
    );
    app.message = None;

    // A shelf inside favourites is left alone: no picture, the heart. The
    // favourite on it, which names the game inside the folder, is listed
    // the way the screen lists it and answered from the game's own
    // written-down row: its picture, name, genre and details, not the
    // folder's derived presentation.
    let shelf = root.join("_@Favorites/Shelf");
    std::fs::create_dir_all(&shelf).unwrap();
    let favorite = shelf.join("Example Game.mgl");
    let mgl = crate::launch::favorite_mgl(
        &app.all_systems[0].to_config(),
        &games.join("Example Game/Example Game.nes"),
    )
    .unwrap()
    .unwrap();
    std::fs::write(&favorite, mgl).unwrap();
    let mut favorites = app.all_systems[0].clone();
    favorites.def.id = "Favorites".into();
    favorites.def.name = "Favorites".into();
    favorites.def.category = Some("Favorites".into());
    favorites.paths = vec![root.join("_@Favorites")];
    let favorites_library =
        Library::open_with_names(&favorites.to_config(), browse::DisplayNames::default()).unwrap();
    let favorites_cache = crate::cache::build_system(&favorites_library);
    let written = &favorites_cache
        .get(&Place::Dir(shelf.clone()))
        .expect("the shelf is written down")
        .rows;
    assert_eq!(written.len(), 1);
    assert_eq!(
        written[0].kind,
        browse::Kind::Play(browse::Launch::File(favorite))
    );
    assert_eq!(written[0].cover, None, "a favourite carries nothing itself");
    crate::cache::save_system(&app.cache_dir, "Favorites", &favorites_cache).unwrap();
    let game = crate::cache::load_system(&app.cache_dir, "NES")
        .and_then(|cache| {
            cache
                .get(&folder_place)
                .map(|folder| folder.rows[0].clone())
        })
        .expect("the game is written down");
    assert_eq!(game.cover.as_deref(), Some(image.as_path()));
    assert_eq!(game.name, "Example Game");
    assert!(game.genre.is_some());
    assert_ne!(game.details, browse::Details::default());
    app.all_systems.push(favorites);
    app.open_system_by_index(1);
    assert!(app.in_favorites());
    assert_eq!(app.here.len(), 1);
    assert!(app.here[0].is_folder());
    assert_eq!(
        app.here[0].cover, None,
        "a shelf has no artwork and never will"
    );
    assert_eq!(app.current_art(), (None, "Shelf".to_string(), true, false));
    app.handle(Action::Accept);
    assert_eq!(app.trail.len(), 2);
    assert_eq!(app.here.len(), 1);
    let listed = &app.here[0];
    assert_eq!(listed.cover, game.cover);
    assert_eq!(listed.name, game.name);
    assert_eq!(listed.genre, game.genre);
    assert_eq!(listed.details, game.details);
    assert!(listed.favorite);
    assert_eq!(
        app.current_art(),
        (Some(image.clone()), "Example Game".to_string(), false, true)
    );
    app.ui.hide().unwrap();
}

/// A Neo Geo library of ROM sets, browsed, started and favourited through
/// the interface: the rows a card owner sees are games, choosing one hands
/// Main the complete set path, and a favourite made here is the ordinary
/// MGL the stock menu reads, back on its row after a restart.
fn run_neogeo_romset_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("neogeo");
    let games = root.join("games/NEOGEO");
    std::fs::create_dir_all(games.join("kof98")).unwrap();
    std::fs::create_dir_all(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NeoGeo.rbf"), b"core").unwrap();
    std::fs::write(
        games.join("romsets.xml"),
        "<romsets><romset name=\"mslug\" altname=\"Metal Slug\"/>\
         <romset name=\"kof98,kof98n\" altname=\"The King of Fighters '98\"/></romsets>",
    )
    .unwrap();
    // Not an archive: a row for it proves the ZIP was taken whole.
    std::fs::write(games.join("mslug.zip"), b"not an archive").unwrap();
    std::fs::write(games.join("kof98/prom"), b"p").unwrap();
    std::fs::write(games.join("Blazing Star.neo"), b"neo").unwrap();

    let open = |settings: Settings| -> App {
        let mut config = Config::parse("[app]", &root.join("degauss.toml")).unwrap();
        config.menu_root = root.to_string_lossy().into_owned();
        config.game_roots = vec![root.join("games").to_string_lossy().into_owned()];
        let table = crate::systems::parse_table(
            include_str!("../assets/systems.toml"),
            Path::new("systems.toml"),
        )
        .unwrap();
        let def = table
            .into_iter()
            .find(|system| system.id == "NeoGeo")
            .unwrap();
        let loaded = Loaded {
            config,
            settings,
            settings_path: root.join("settings.toml"),
            core_catalogue: Default::default(),
            systems: vec![FoundSystem {
                def: def.clone(),
                paths: vec![games.clone()],
                logo_dir: None,
                menu_folder: None,
            }],
            table: vec![def],
            names: Default::default(),
            logo_dir: None,
            themes_dir: root.join("themes"),
            themes: Default::default(),
        };
        let mut app = App::new(
            loaded,
            window.clone(),
            DegaussWindow::new().unwrap(),
            StartupTimings::default(),
            352,
            240,
        );
        app.open_system_by_index(0);
        assert!(app.build.is_none(), "fixture indexing must actually finish");
        assert!(
            app.message.is_none(),
            "fixture startup failed: {:?}",
            app.message
        );
        app.leave_splash();
        assert_eq!(app.screen, Screen::Browse);
        app
    };

    let mut app = open(Settings::default());
    let names: Vec<&str> = app.here.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        ["Blazing Star.neo", "Metal Slug", "The King of Fighters '98"]
    );
    assert!(
        app.here.iter().all(|row| !row.is_folder()),
        "a set is a game, not a folder to enter"
    );
    let sets = [
        ("mslug.zip", "Metal Slug"),
        ("kof98", "The King of Fighters '98"),
    ];
    let position = |app: &App, title: &str| {
        app.here
            .iter()
            .position(|row| row.name == title)
            .unwrap_or_else(|| panic!("{title} is listed"))
    };
    for (set, title) in sets {
        app.game_list.select(position(&app, title));
        let Some(Outcome::Launch { plan, name, .. }) = app.confirm_launch() else {
            panic!("{set} must launch: {:?}", app.message);
        };
        assert_eq!(name, title);
        assert!(
            plan.mgl.contains(&format!(
                "type=\"f\" index=\"1\" path=\"../../../../..{}\"",
                games.join(set).display()
            )),
            "the complete set path goes to Main: {}",
            plan.mgl
        );
        assert!(app.message.is_none());
    }

    for (set, title) in sets {
        app.game_list.select(position(&app, title));
        let favourite_folder = root.join("_@Favorites/Neo Geo");
        app.add_favorite_in(&favourite_folder);
        let favourite = favourite_folder.join(format!("{title}.mgl"));
        let text = std::fs::read_to_string(&favourite).expect("the favourite is written");
        assert!(
            text.contains(&format!("path=\"{}\"", games.join(set).display())),
            "an ordinary MGL with the absolute set path: {text}"
        );
        // Favourites lead the list once marked, so the row is found again
        // by its title rather than by where it was.
        assert!(
            app.here[position(&app, title)].favorite,
            "the heart shows at once"
        );
    }
    assert!(!app.here[position(&app, "Blazing Star.neo")].favorite);

    app.ui.hide().unwrap();
    drop(app);
    let mut app = open(Settings::default());
    for (_, title) in sets {
        assert!(
            app.here[position(&app, title)].favorite,
            "{title} keeps its heart after a restart"
        );
    }
    assert!(!app.here[position(&app, "Blazing Star.neo")].favorite);
    app.game_list.select(position(&app, "Metal Slug"));
    app.remove_favorite();
    assert!(!root.join("_@Favorites/Neo Geo/Metal Slug.mgl").exists());
    assert!(!app.here[position(&app, "Metal Slug")].favorite);
    assert!(app.here[position(&app, "The King of Fighters '98")].favorite);
    assert!(app.message.is_none(), "{:?}", app.message);

    // A catalogue broken after the index was written is met by a listing
    // read from the card, which has no build report to carry it. The
    // zipped set then shows as an archive and the set folder, holding
    // nothing the system opens, is hidden as empty; the screen says why
    // with that listing, once, or a card owner would see two games gone
    // and nothing but the log to explain it.
    std::fs::write(games.join("romsets.xml"), "<romsets><romset name=\"mslug\"").unwrap();
    app.system_cache = None;
    app.library = None;
    app.relist_here();
    let names: Vec<&str> = app.here.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["mslug", "Blazing Star.neo"]);
    let expected = format!("Neo Geo: {}", games.join("romsets.xml").display());
    let message = app
        .message
        .clone()
        .expect("the broken catalogue is said on screen");
    assert!(
        message.starts_with(&expected) && message.contains("malformed"),
        "the system, the file and the reason: {message}"
    );
    app.relist_here();
    assert!(
        app.message.is_none(),
        "said once, not at every folder: {:?}",
        app.message
    );
    app.ui.hide().unwrap();
}

/// The picture's half of the safe width in Details, before the browse-games
/// split is applied to it.
fn details_half_width(app: &App) -> f32 {
    Geometry::compute(
        Layout::Details,
        app.plain_screen(),
        app.chrome_here(),
        app.bar_here(),
        app.width,
        app.height,
        &app.config,
    )
    .art_width
}

fn assert_details_split(app: &App, style: DetailsStyle, over_game: bool, context: &str) {
    assert_eq!(app.details_style, style, "{context}");
    assert_eq!(app.layout, Layout::Details, "{context}");
    // The picture's share of the safe width while browsing games, 42% for
    // Information and 62% for Large Artwork, measured from the safe width
    // itself rather than from the Details half, so a drift in either the
    // base split or the factor fails here. The half is rounded to a pixel
    // before the factor, hence the tolerance.
    let share = match style {
        DetailsStyle::Information => 0.42,
        DetailsStyle::LargeArtwork => 0.62,
    };
    let inset = (app.width as f32 * app.config.app.overscan_x as f32 / 100.0).round();
    let safe = (app.width as f32 - inset * 2.0).max(64.0);
    let expected = safe * share;
    assert!(
        (app.ui.get_art_width() - expected).abs() <= 1.0,
        "{context}: {style:?} must give the picture {expected} of the width, not {}",
        app.ui.get_art_width()
    );
    let panel = app.ui.get_detail_height();
    match style {
        DetailsStyle::Information if over_game => assert!(
            panel > 0.0,
            "{context}: Information keeps the compact lines under the picture"
        ),
        DetailsStyle::Information => {
            assert_eq!(panel, 0.0, "{context}: a folder has no compact lines")
        }
        DetailsStyle::LargeArtwork => assert_eq!(
            panel, 0.0,
            "{context}: Large Artwork gives the whole column height to the picture"
        ),
    }
}

fn leave_options_to_browse(app: &mut App) {
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::OptionsRoot, "leaving the page saves");
    app.handle(Action::Quit);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
}

/// Name shortening is a projection of the final effective row name. The
/// canonical row, target path and cache stay complete while every browse view,
/// search and jump use the same projected text.
fn run_game_name_display_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("game-name-display");
    let games = root.join("games/NES");
    std::fs::create_dir_all(games.join("[Proto] Folder (Japan)")).unwrap();
    std::fs::create_dir_all(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NES.rbf"), b"fixture core").unwrap();
    for name in [
        "(USA) Zebra [Rev 1].nes",
        "Metadata File.nes",
        "Twin A.nes",
        "Twin B.nes",
    ] {
        std::fs::write(games.join(name), b"fixture game").unwrap();
    }
    std::fs::write(
        games.join("[Proto] Folder (Japan)/Inside.nes"),
        b"fixture game",
    )
    .unwrap();
    let gamelist = r#"<gameList>
        <game><path>(USA) Zebra [Rev 1].nes</path><name>(USA) Zebra [Rev 1]</name></game>
        <game><path>Metadata File.nes</path><name>Metadata Name (Europe) [T+ENG]</name></game>
        <game><path>Twin A.nes</path><name>Twin (USA)</name></game>
        <game><path>Twin B.nes</path><name>Twin (Europe)</name></game>
    </gameList>"#;
    std::fs::write(games.join("gamelist.xml"), gamelist).unwrap();

    let mut app = unopened_fixture_app(&root, window.clone(), Settings::default());
    app.open_system_by_index(0);
    assert!(app.build.is_none(), "fixture indexing must finish");
    assert!(
        app.message.is_none(),
        "fixture startup failed: {:?}",
        app.message
    );
    app.leave_splash();
    assert_eq!(app.screen, Screen::Browse);
    app.refresh();

    let canonical = [
        "[Proto] Folder (Japan)",
        "(USA) Zebra [Rev 1]",
        "Metadata Name (Europe) [T+ENG]",
        "Twin (Europe)",
        "Twin (USA)",
    ];
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        canonical,
        "legacy settings retain complete effective names"
    );
    assert_eq!(
        app.rows.row_data(0).unwrap().title,
        "[ [Proto] Folder (Japan) ]"
    );
    let cache_before = cache_snapshot(&app.cache_dir);
    let gamelist_before = std::fs::read(games.join("gamelist.xml")).unwrap();

    select_option(&mut app, OptionsPage::Appearance, OptionId::GameNameDisplay);
    for _ in 0..3 {
        app.handle(Action::Faster);
    }
    assert_eq!(
        app.game_name_display,
        GameNameDisplay::RemoveParenthesesAndBrackets
    );
    assert!(
        app.build.is_none(),
        "a display change must not start indexing"
    );
    leave_options_to_browse(&mut app);
    app.refresh();

    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        [
            "[Proto] Folder (Japan)",
            "Metadata Name (Europe) [T+ENG]",
            "Twin (Europe)",
            "Twin (USA)",
            "(USA) Zebra [Rev 1]",
        ],
        "only visible ordering changes; complete names remain on the rows"
    );
    assert_eq!(cache_snapshot(&app.cache_dir), cache_before);
    assert_eq!(
        std::fs::read(games.join("gamelist.xml")).unwrap(),
        gamelist_before
    );
    let expected = ["[ Folder ]", "Metadata Name", "Twin", "Twin", "Zebra"];
    for layout in Layout::ALL {
        app.layout = layout;
        app.apply_geometry();
        for (index, expected) in expected.iter().enumerate() {
            app.game_list.select(index);
            app.refresh();
            assert_eq!(
                app.rows
                    .row_data(app.ui.get_selected() as usize)
                    .unwrap()
                    .title,
                *expected,
                "{layout:?} row {index} uses the same display projection"
            );
        }
    }
    let metadata = app
        .here
        .iter()
        .position(|row| row.name == "Metadata Name (Europe) [T+ENG]")
        .unwrap();
    app.game_list.select(metadata);
    assert_eq!(app.current_art().1, "Metadata Name");
    assert!(game_information_named(&app.here[metadata], "Metadata Name")
        .starts_with("Metadata Name\n\n"));

    app.filter = "ZEBRA".into();
    app.apply_filter();
    assert_eq!(app.here.len(), 1);
    assert_eq!(app.here[0].name, "(USA) Zebra [Rev 1]");
    app.clear_filter();
    app.jump_to('z');
    assert_eq!(
        app.here[app.game_list.selected()].name,
        "(USA) Zebra [Rev 1]",
        "jump follows the first visible letter, not the removed prefix"
    );

    let favourite_folder = root.join("_@Favorites/Name Display");
    for (name, file) in [
        ("Twin (USA)", "Twin A.nes"),
        ("Twin (Europe)", "Twin B.nes"),
    ] {
        let at = app.here.iter().position(|row| row.name == name).unwrap();
        app.game_list.select(at);
        let Some(Outcome::Launch {
            plan,
            name: launched,
            ..
        }) = app.confirm_launch()
        else {
            panic!("{name} must produce a launch plan: {:?}", app.message);
        };
        assert_eq!(launched, name, "launch identity remains canonical");
        assert!(
            plan.mgl.contains(file),
            "{name} launches {file}: {}",
            plan.mgl
        );
        app.add_favorite_in(&favourite_folder);
    }
    for (name, file) in [
        ("Twin (USA)", "Twin A.nes"),
        ("Twin (Europe)", "Twin B.nes"),
    ] {
        let saved = std::fs::read_to_string(favourite_folder.join(format!("{name}.mgl")))
            .expect("each visually identical title keeps its own favourite");
        assert!(saved.contains(file), "{name} keeps its own target: {saved}");
    }

    select_option(&mut app, OptionsPage::Appearance, OptionId::FolderBrackets);
    app.handle(Action::Faster);
    assert!(!app.folder_brackets);
    leave_options_to_browse(&mut app);
    app.refresh();
    assert_eq!(app.rows.row_data(0).unwrap().title, "Folder");

    select_option(&mut app, OptionsPage::Appearance, OptionId::GameNameDisplay);
    for _ in 0..3 {
        app.handle(Action::Slower);
    }
    assert_eq!(app.game_name_display, GameNameDisplay::Full);
    leave_options_to_browse(&mut app);
    app.refresh();
    let folder = app
        .here
        .iter()
        .position(|row| row.name == "[Proto] Folder (Japan)")
        .unwrap();
    assert_eq!(
        app.rows.row_data(folder).unwrap().title,
        "[Proto] Folder (Japan)",
        "Folder Brackets Off removes only Degauss's outer decoration"
    );
    assert_eq!(cache_snapshot(&app.cache_dir), cache_before);

    let saved = Settings::load(&app.settings_path).unwrap();
    assert_eq!(saved.game_name_display, Some(GameNameDisplay::Full));
    assert_eq!(saved.folder_brackets, Some(false));
    app.ui.hide().unwrap();
    drop(app);

    let mut restarted = unopened_fixture_app(&root, window, saved);
    restarted.open_system_by_index(0);
    assert!(restarted.build.is_none());
    restarted.leave_splash();
    restarted.refresh();
    assert_eq!(restarted.game_name_display, GameNameDisplay::Full);
    assert!(!restarted.folder_brackets);
    let folder = restarted
        .here
        .iter()
        .position(|row| row.name == "[Proto] Folder (Japan)")
        .unwrap();
    assert_eq!(
        restarted.rows.row_data(folder).unwrap().title,
        "[Proto] Folder (Japan)"
    );
    restarted.ui.hide().unwrap();
}

fn run_details_style_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let artwork = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos/NES.png");
    assert!(artwork.is_file());

    // An older settings file has no key: it must draw exactly the layout
    // it was drawn with, and starting must not write a choice the user
    // never made. The legacy "preview" name still means Details.
    let games_place = {
        let app = fixture_app(root, window.clone(), Settings::default());
        let place = app.current_view_place().unwrap();
        app.ui.hide().unwrap();
        place
    };
    for layout in [None, Some("preview")] {
        let mut app = fixture_app(
            root,
            window.clone(),
            Settings {
                layout: layout.map(str::to_string),
                ..Default::default()
            },
        );
        app.refresh();
        assert_details_split(&app, DetailsStyle::Information, true, "no saved key");
        assert!(
            app.settings.details_style.is_none(),
            "startup must not rewrite a choice"
        );
        assert_eq!(app.option_value(OptionId::DetailsStyle), "Information");
        app.ui.hide().unwrap();
    }

    // A token that is neither name is not quietly drawn as Information:
    // the first screen says so, and the text stays in the setting for the
    // user to correct rather than being replaced by a choice never made.
    // The startup source check finishes before anyone has read the line,
    // so its result must go under the report rather than replace it. The
    // log gets the same line, because a first-start library read takes
    // the screen before it and would leave the substitution without a
    // trace.
    {
        let log_before = std::fs::metadata(crate::LOG_PATH)
            .map(|meta| meta.len() as usize)
            .unwrap_or(0);
        let mut app = unopened_fixture_app(
            root,
            window.clone(),
            Settings {
                details_style: Some("large_artwork".into()),
                ..Default::default()
            },
        );
        assert_eq!(app.details_style, DetailsStyle::Information);
        assert_eq!(
            app.settings.details_style.as_deref(),
            Some("large_artwork"),
            "startup must not rewrite the text it could not read"
        );
        // The log is the one path outside the fixture; a log that cannot
        // be read fails the same assertion, with the reason in its place.
        let logged = match std::fs::read(crate::LOG_PATH) {
            Ok(log) => String::from_utf8_lossy(&log[log_before.min(log.len())..]).into_owned(),
            Err(error) => format!(
                "(the log at {} could not be read: {error})",
                crate::LOG_PATH
            ),
        };
        assert!(
            logged.contains("Details Style large_artwork is not information or large-artwork"),
            "the unreadable token is logged: {logged}"
        );
        app.finish_background_work_for_headless();
        assert!(
            app.source_resolution.is_none(),
            "the startup source check must have finished"
        );
        let message = app
            .message
            .clone()
            .expect("an unreadable Details Style is still reported once the startup check is in");
        assert!(message.contains("large_artwork"), "{message}");
        assert!(message.contains("using Information"), "{message}");
        app.leave_splash();
        app.handle(Action::Accept);
        assert!(
            app.message.is_none(),
            "a press takes the report down: {:?}",
            app.message
        );
        app.ui.hide().unwrap();
    }

    // Cancelling the startup check with B is its third way to finish and
    // follows the same rule as the other two: the problem lines stay and
    // the check's word goes under them, and the lines are given up so
    // nothing later can put them back over another message.
    {
        let mut app = unopened_fixture_app(
            root,
            window.clone(),
            Settings {
                details_style: Some("large_artwork".into()),
                ..Default::default()
            },
        );
        assert!(
            app.source_resolution.is_some() && app.build.is_none(),
            "the startup check is pending and B can cancel it"
        );
        app.handle(Action::Quit);
        app.finish_background_work_for_headless();
        let message = app
            .message
            .clone()
            .expect("the cancelled check leaves the problem lines up");
        assert!(
            message.starts_with("Details Style large_artwork"),
            "{message}"
        );
        assert!(
            message.ends_with("\nGame data source check cancelled"),
            "{message}"
        );
        assert!(
            app.startup_problems.is_none(),
            "every completion of the startup check takes the lines"
        );
        app.ui.hide().unwrap();
    }

    // A first start reads the card, and a press while the startup check is
    // pending opens the build report in place of the problem lines. A
    // check that then fails must say only its own word: the report it did
    // not write, ending in "No problems reported", must not stand above
    // the failure.
    {
        let first = root.join("details-style-first-start");
        std::fs::create_dir_all(first.join("games/NES")).unwrap();
        std::fs::write(first.join("games/NES/First Game.nes"), b"fixture").unwrap();
        let mut app = unopened_fixture_app(
            &first,
            window.clone(),
            Settings {
                details_style: Some("large_artwork".into()),
                ..Default::default()
            },
        );
        assert!(app.build.is_some(), "a first start reads the card");
        assert!(
            app.source_resolution.is_some(),
            "the startup check is still pending"
        );
        assert!(
            app.message
                .as_deref()
                .is_some_and(|message| message.contains("large_artwork")),
            "{:?}",
            app.message
        );
        app.leave_splash();
        app.handle(Action::Accept);
        assert!(app.index_details, "a press opens the build report");
        let report = app.message.clone().expect("the build report is on screen");
        assert!(report.contains("No problems reported"), "{report}");
        app.report_source_check(
            SourceResolutionAction::Startup,
            Some("Game data source check failed".into()),
        );
        assert_eq!(
            app.message.as_deref(),
            Some("Game data source check failed"),
            "a failed check replaces the report it did not write"
        );
        app.ui.hide().unwrap();
    }

    // Both styles persist: the choice is written when the page is left and
    // survives a fresh App from the reloaded file, in both directions.
    let mut app = fixture_app(root, window.clone(), Settings::default());
    let cache_before = cache_snapshot(&app.cache_dir);
    for (style, saved) in [
        (DetailsStyle::LargeArtwork, "large-artwork"),
        (DetailsStyle::Information, "information"),
    ] {
        select_option(&mut app, OptionsPage::Appearance, OptionId::DetailsStyle);
        // On the Options screen load_art wants no picture and only clears
        // the request, so a handler that asked for one again would leave
        // the flag raised.
        app.load_art();
        assert!(!app.art_pending);
        app.handle(Action::Faster);
        assert_eq!(app.details_style, style);
        assert_eq!(app.option_value(OptionId::DetailsStyle), style.shown());
        assert!(
            !app.art_pending,
            "a style change must not ask for the picture again"
        );
        assert!(
            app.build.is_none(),
            "a style change must not rebuild the library"
        );
        leave_options_to_browse(&mut app);
        app.refresh();
        assert_details_split(&app, style, true, "after leaving Options");
        assert_eq!(app.settings.details_style.as_deref(), Some(saved));
        let reloaded = Settings::load(&app.settings_path).unwrap();
        assert_eq!(reloaded.details_style.as_deref(), Some(saved));
        assert_eq!(
            cache_snapshot(&app.cache_dir),
            cache_before,
            "a display option must not touch the index or the artwork cache"
        );
        app.ui.hide().unwrap();
        drop(app);
        app = fixture_app(root, window.clone(), reloaded);
        app.refresh();
        assert_details_split(&app, style, true, "the saved style survives restart");
        assert_eq!(app.settings.details_style.as_deref(), Some(saved));
    }
    // Left steps back through the same two values, so neither is a dead end.
    select_option(&mut app, OptionsPage::Appearance, OptionId::DetailsStyle);
    app.handle(Action::Slower);
    assert_eq!(app.details_style, DetailsStyle::LargeArtwork);
    app.handle(Action::Slower);
    assert_eq!(app.details_style, DetailsStyle::Information);
    leave_options_to_browse(&mut app);

    // A folder, a game with a picture, a game without one and a title far
    // longer than the list column: every one has to remain usable in both
    // styles. The long title is left to the row to elide or scroll; the
    // list must hand it over whole.
    let long_title: String = "Fixture Game With A Very Long Name "
        .chars()
        .cycle()
        .take(120)
        .collect();
    let game = |name: &str, cover: Option<PathBuf>| {
        let mut row = information_row();
        row.name = name.to_string();
        row.sort_key = row.name.to_ascii_uppercase();
        row.kind = browse::Kind::Play(browse::Launch::File(root.join("games/NES/First Game.nes")));
        row.cover = cover;
        row
    };
    let rows = vec![
        browse::Row {
            name: "Fixture Folder".into(),
            sort_key: "FIXTURE FOLDER".into(),
            kind: browse::Kind::Enter(Place::Dir(root.join("games/NES"))),
            cover: None,
            genre: None,
            favorite: false,
            below: Some(2),
            details: browse::Details::default(),
        },
        game("Fixture Game With Picture", Some(artwork.clone())),
        game("Fixture Game Without Picture", None),
        game(&long_title, Some(artwork.clone())),
    ];
    app.here = rows.clone();
    app.all_here = rows;
    app.game_list = ListState::new(app.here.len(), app.geometry.visible);
    for style in DetailsStyle::ALL {
        app.details_style = style;
        app.apply_geometry();
        for (index, has_art, over_game) in [
            (0, false, false),
            (1, true, true),
            (2, false, true),
            (3, true, true),
        ] {
            app.game_list.select(index);
            app.load_art();
            app.refresh();
            let context = format!("{style:?} over row {index}");
            assert_details_split(&app, style, over_game, &context);
            assert_eq!(app.ui.get_has_art(), has_art, "{context}");
            if !has_art {
                assert_eq!(
                    app.ui.get_art_caption().as_str(),
                    app.here[index].name,
                    "{context}: the name stands in for a missing picture"
                );
            }
            // A folder's title is bracketed; a game's is the name itself.
            let title = app
                .rows
                .row_data(app.ui.get_selected() as usize)
                .unwrap()
                .title;
            assert!(
                title.contains(app.here[index].name.as_str()),
                "{context}: the selected row must carry its whole title, not {title:?}"
            );
            app.open_context();
            assert_eq!(app.screen, Screen::Context);
            assert_eq!(
                app.context_actions
                    .iter()
                    .any(|entry| entry == GAME_INFORMATION),
                over_game,
                "{context}: Game Information stays in Actions in both styles"
            );
            app.handle(Action::Quit);
            assert_eq!(app.screen, Screen::Browse);
        }
        // With artwork off the column shows the name instead of the
        // picture; the split and the compact lines are the style's own.
        app.game_list.select(1);
        app.show_art = false;
        app.load_art();
        app.refresh();
        assert!(!app.ui.get_has_art());
        assert_details_split(&app, style, true, "artwork off");
        app.show_art = true;
        app.load_art();
        app.refresh();
        assert!(app.ui.get_has_art());
    }

    // The display correction belongs to the picture, not to the style:
    // 4:3 artwork on a 4:3 tube needs the same horizontal stretch whether
    // its column is the narrow one or the wide one.
    app.game_list.select(1);
    app.artwork_scale = ArtworkScale::FourThree;
    let corrected = artwork_horizontal(ArtworkScale::FourThree, app.width, app.height, true);
    assert!(
        (corrected - 1.0).abs() > 0.01,
        "the fixture screen must need correction for this to prove anything"
    );
    for style in DetailsStyle::ALL {
        app.details_style = style;
        app.apply_geometry();
        app.load_art();
        app.refresh();
        assert!(app.ui.get_has_art());
        assert_eq!(
            app.ui.get_art_scale_x(),
            corrected,
            "{style:?} must keep the artwork aspect correction"
        );
    }
    app.artwork_scale = ArtworkScale::Framebuffer;
    app.load_art();
    assert_eq!(app.ui.get_art_scale_x(), 1.0);

    // The tube first, then the higher resolutions: every one of them
    // draws through the same split and hands the picture the whole column
    // in Large Artwork.
    for style in DetailsStyle::ALL {
        app.details_style = style;
        app.game_list.select(1);
        for (width, height) in [(352, 240), (640, 480), (1280, 720)] {
            app.width = width;
            app.height = height;
            app.window.set_size(slint::PhysicalSize::new(width, height));
            app.apply_geometry();
            if let Some(directory) = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR") {
                let name = format!("details-{}", style.setting());
                capture_frame(&mut app, &PathBuf::from(directory), &name, width, height);
            }
            app.load_art();
            app.refresh();
            assert_details_split(&app, style, true, &format!("{width}x{height}"));
        }
    }
    app.width = 352;
    app.height = 240;
    app.window.set_size(slint::PhysicalSize::new(352, 240));
    app.apply_geometry();

    // Only the games level of Details changes shape. The category and
    // system screens keep the half split their logos are placed in, and
    // every other view keeps the whole width for its rows.
    for style in DetailsStyle::ALL {
        app.details_style = style;
        for browsing in [Browsing::Categories, Browsing::Systems] {
            app.browsing = browsing;
            app.apply_geometry();
            assert!(
                (app.ui.get_art_width() - details_half_width(&app)).abs() < 0.01,
                "{style:?} must leave {browsing:?} at half the width"
            );
            assert_eq!(app.ui.get_detail_height(), 0.0);
        }
        app.browsing = Browsing::Games;
        for layout in Layout::ALL {
            if layout == Layout::Details {
                continue;
            }
            app.layout = layout;
            app.apply_geometry();
            assert_eq!(
                app.ui.get_art_width(),
                0.0,
                "{style:?} must leave {layout:?} without a picture column"
            );
            assert_eq!(app.ui.get_detail_height(), 0.0);
        }
        app.layout = Layout::Details;
        app.apply_geometry();
    }
    app.ui.hide().unwrap();
    drop(app);

    // A custom view naming Details, and the legacy "preview" spelling,
    // resolve to Details under either style: this is a style of one view,
    // not a seventh view.
    for (style, saved) in [
        (DetailsStyle::Information, "information"),
        (DetailsStyle::LargeArtwork, "large-artwork"),
    ] {
        let mut custom_views = CustomViews::default();
        games_place.set(&mut custom_views, "details".into());
        let mut app = fixture_app(
            root,
            window.clone(),
            Settings {
                layout: Some("tiled".into()),
                custom_views,
                details_style: Some(saved.into()),
                ..Default::default()
            },
        );
        app.refresh();
        assert_eq!(app.global_layout, Layout::Tiled);
        assert_details_split(&app, style, true, "custom Details view");
        app.ui.hide().unwrap();
        drop(app);

        let mut app = fixture_app(
            root,
            window.clone(),
            Settings {
                layout: Some("preview".into()),
                details_style: Some(saved.into()),
                ..Default::default()
            },
        );
        app.refresh();
        assert_eq!(app.global_layout, Layout::Details);
        assert_details_split(&app, style, true, "legacy preview view");
        app.ui.hide().unwrap();
    }
}

fn run_paged_theme_name_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("theme-paged-name");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Fixture.nes", "Second Fixture.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let mut app = fixture_app(&root, window.clone(), Settings::default());
    app.open_theme_editor();
    let save_as = app
        .theme_editor
        .as_ref()
        .unwrap()
        .rows()
        .iter()
        .position(|(label, _)| label == "Save as")
        .unwrap();
    app.select(save_as);
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Name);
    assert_eq!(
        app.theme_editor.as_ref().unwrap().name_page,
        NamePage::Lower
    );
    app.apply_geometry();
    assert_eq!(
        app.theme_editor_list.visible(),
        app.theme_editor.as_ref().unwrap().name_cell_count(),
        "a geometry refresh must keep the complete name page visible"
    );
    assert_eq!(app.ui.get_columns(), NAME_COLUMNS as i32);

    // Clear is an explicit cell rather than consuming another controller
    // button. Build the final name through all three pages, including a digit,
    // an internal space and safe punctuation.
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().name, "a");
    let clear = app
        .theme_editor
        .as_ref()
        .unwrap()
        .rows()
        .iter()
        .position(|(label, _)| label == "Clear")
        .unwrap();
    app.theme_editor.as_mut().unwrap().sub_selected = clear;
    app.sync_theme_editor();
    app.handle(Action::Accept);
    assert!(app.theme_editor.as_ref().unwrap().name.is_empty());

    app.theme_editor.as_mut().unwrap().sub_selected = 0;
    app.sync_theme_editor();
    app.handle(Action::Accept);
    app.handle(Action::Menu);
    assert_eq!(
        app.theme_editor.as_ref().unwrap().name_page,
        NamePage::Upper
    );
    app.handle(Action::Accept);
    let one = app
        .theme_editor
        .as_ref()
        .unwrap()
        .rows()
        .iter()
        .position(|(label, _)| label == "1")
        .unwrap();
    app.theme_editor.as_mut().unwrap().sub_selected = one;
    app.sync_theme_editor();
    app.handle(Action::Accept);
    let space = app
        .theme_editor
        .as_ref()
        .unwrap()
        .rows()
        .iter()
        .position(|(label, _)| label == "SP")
        .unwrap();
    app.theme_editor.as_mut().unwrap().sub_selected = space;
    app.sync_theme_editor();
    app.handle(Action::Accept);
    app.handle(Action::Menu);
    assert_eq!(
        app.theme_editor.as_ref().unwrap().name_page,
        NamePage::Symbols
    );
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().name, "aA1 !");
    app.handle(Action::Context);
    assert_eq!(app.theme_editor.as_ref().unwrap().name, "aA1 ");
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().name, "aA1 !");
    app.refresh();
    assert_eq!(app.ui.get_theme_name_page(), "symbols");

    let save = app.theme_editor.as_ref().unwrap().name_save_index();
    app.theme_editor.as_mut().unwrap().sub_selected = save;
    app.sync_theme_editor();
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Options);
    assert!(root.join("themes/aA1 !.toml").is_file());
    let saved = Settings::load(&app.settings_path).unwrap();
    assert_eq!(saved.theme.as_deref(), Some("aA1 !"));
    app.ui.hide().unwrap();
    drop(app);

    let restarted = fixture_app(&root, window, saved);
    assert_eq!(
        restarted
            .active_theme
            .map(|at| restarted.themes[at].name.as_str()),
        Some("aA1 !"),
        "a mixed-page theme name reloads after restart"
    );
    restarted.ui.hide().unwrap();
}

fn run_theme_editor_save_changes_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("theme-save-changes");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Fixture.nes", "Second Fixture.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let palette = Colors::default();
    let themes_dir = root.join("themes");
    let path = crate::theme::save_new(
        &themes_dir,
        "Editable",
        &crate::theme::ThemeFile::complete(&palette, None, 100),
        &palette,
    )
    .unwrap();
    let settings = Settings {
        theme: Some("Editable".into()),
        theme_font_override: Some(false),
        ..Settings::default()
    };
    let mut app = fixture_app(&root, window.clone(), settings);
    assert_eq!(
        app.active_theme.map(|at| app.themes[at].name.as_str()),
        Some("Editable")
    );
    app.open_theme_editor();
    assert_eq!(app.theme_editor.as_ref().unwrap().rows().len(), 18);
    assert!(app.theme_editor.as_ref().unwrap().source_is_custom());
    assert!(!app.horizontal_scrolls());

    app.select(1);
    app.handle(Action::Accept);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Picker);
    assert!(app.horizontal_scrolls());
    let before = app.theme_editor.as_ref().unwrap().draft.palette.background;
    app.handle(Action::Faster);
    let saved_colour = app.theme_editor.as_ref().unwrap().draft.palette.background;
    assert_ne!(saved_colour, before);
    app.handle(Action::Context);
    assert_eq!(app.theme_editor.as_ref().unwrap().mode, EditorMode::Hex);
    assert!(
        !app.horizontal_scrolls(),
        "hex digit selection and every non-picker editor mode remain one step per press"
    );
    app.handle(Action::Context);
    assert!(app.horizontal_scrolls());
    app.handle(Action::Accept);
    assert!(!app.horizontal_scrolls());
    app.select(14);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Options);
    assert!(app.theme_editor.is_none());
    assert_eq!(app.message, None);
    assert_eq!(
        crate::theme::load(&themes_dir).themes[0]
            .file
            .apply(&palette)
            .background,
        saved_colour
    );
    let saved_settings = Settings::load(&app.settings_path).unwrap();
    assert_eq!(saved_settings.theme.as_deref(), Some("Editable"));
    assert_eq!(saved_settings.theme_font_override, Some(false));
    app.ui.hide().unwrap();
    drop(app);

    let mut app = fixture_app(&root, window, saved_settings);
    assert_eq!(
        app.active_theme.map(|at| app.themes[at].name.as_str()),
        Some("Editable"),
        "the updated selected theme must survive restart"
    );
    app.open_theme_editor();
    app.select(1);
    app.handle(Action::Accept);
    app.handle(Action::Slower);
    app.handle(Action::Accept);
    let previous_file = std::fs::read(&path).unwrap();
    let settings_path = app.settings_path.clone();
    let blocked = root.join("settings-parent-is-a-file");
    std::fs::write(&blocked, b"not a directory").unwrap();
    app.settings_path = blocked.join("settings.toml");
    app.select(14);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::ThemeEditor);
    assert!(
        app.theme_editor.is_some(),
        "the unsaved draft remains editable"
    );
    assert!(app.message.as_deref().is_some_and(|message| {
        message.contains("settings") && message.contains("previous theme was restored")
    }));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        previous_file,
        "a failed settings write restores the exact previous theme file"
    );
    assert!(crate::theme::load(&themes_dir)
        .themes
        .iter()
        .any(|theme| theme.name == "Editable"));
    app.settings_path = settings_path;
    app.ui.hide().unwrap();
}

fn metadata_filter_game(
    name: &str,
    genre: Option<&str>,
    released: &str,
    players: &str,
    language: &str,
    developer: &str,
    favorite: bool,
) -> browse::Row {
    browse::Row {
        name: name.to_string(),
        sort_key: name.to_lowercase(),
        kind: browse::Kind::Play(browse::Launch::File(PathBuf::from(format!("{name}.nes")))),
        cover: None,
        genre: genre.map(str::to_string),
        favorite,
        below: None,
        details: browse::Details {
            released: released.to_string(),
            players: players.to_string(),
            lang: language.to_string(),
            developer: developer.to_string(),
            ..Default::default()
        },
    }
}

fn choose_metadata_filter(app: &mut App, field: GameFilterField, label: &str) {
    app.show_game_filters(field.index());
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::GameFilterValues);
    let selected = app.game_filter_options[field.index()]
        .as_ref()
        .unwrap()
        .iter()
        .position(|choice| choice.label() == label)
        .unwrap();
    app.menu_list.select(selected);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::GameFilters);
}

fn run_metadata_filter_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("metadata-filters");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let mut app = fixture_app(&root, window, Settings::default());
    app.here = vec![
        browse::Row {
            name: "Nested".into(),
            sort_key: "nested".into(),
            kind: browse::Kind::Enter(Place::Dir(root.join("games/NES/Nested"))),
            cover: None,
            genre: None,
            favorite: false,
            below: Some(1),
            details: Default::default(),
        },
        metadata_filter_game(
            "Alpha",
            Some(" Action "),
            "1994-12-03",
            "1",
            "English",
            "Studio A",
            true,
        ),
        metadata_filter_game(
            "Beta",
            Some("RPG"),
            "1995",
            "2",
            "French",
            "Studio B",
            false,
        ),
        metadata_filter_game("Gamma", Some("action"), "", "", "", "", true),
    ];
    app.game_list = ListState::new(app.here.len(), app.geometry.visible);
    app.game_list.select(1);
    let settings_before = std::fs::read(&app.settings_path).ok();
    let cache_before = if app.cache_dir.exists() {
        cache_snapshot(&app.cache_dir)
    } else {
        Vec::new()
    };

    // The feature is reached through Actions / Find and opening it performs
    // no settings or cache write.
    app.open_context();
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == ContextPage::Find.label())
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == FILTER_GAMES)
            .unwrap_or_else(|| panic!("Filter Games missing from Find page: {:?}", app.menu)),
    );
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::GameFilters);
    assert_eq!(std::fs::read(&app.settings_path).ok(), settings_before);

    // Both supported framebuffers show every field, and a wholly empty
    // metadata field is visible but inert rather than offering Unknown alone.
    for (width, height) in [(352, 240), (640, 480)] {
        app.width = width;
        app.height = height;
        app.window.set_size(slint::PhysicalSize::new(width, height));
        app.apply_geometry();
        app.refresh();
        assert_eq!(app.ui.get_heading(), "Filter Games");
        assert_eq!(app.rows.row_count(), GameFilterField::ALL.len());
        assert_eq!(app.rows.row_data(5).unwrap().title, "Publisher");
        assert_eq!(app.rows.row_data(5).unwrap().value, "Unavailable");
        if let Some(directory) = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR") {
            let directory = PathBuf::from(directory);
            assert!(
                directory.is_absolute() && directory.is_dir(),
                "DEGAUSS_UI_CAPTURE_DIR must name an existing absolute directory"
            );
            capture_frame(&mut app, &directory, "metadata-filters", width, height);
        }
    }
    app.width = 352;
    app.height = 240;
    app.window.set_size(slint::PhysicalSize::new(352, 240));
    app.apply_geometry();
    app.menu_list.select(GameFilterField::Publisher.index());
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::GameFilters);

    choose_metadata_filter(&mut app, GameFilterField::Genre, "Action");
    assert_eq!(app.game_filters.label(GameFilterField::Genre), "Action");
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Nested", "Alpha", "Gamma"]
    );

    // Choosers always come from the complete place, and B discards only the
    // highlighted alternative rather than the applied selection.
    app.menu_list.select(GameFilterField::Genre.index());
    app.handle(Action::Accept);
    let rpg = app.game_filter_options[GameFilterField::Genre.index()]
        .as_ref()
        .unwrap()
        .iter()
        .position(|choice| choice.label() == "RPG")
        .unwrap();
    app.menu_list.select(rpg);
    app.handle(Action::Quit);
    assert_eq!(app.game_filters.label(GameFilterField::Genre), "Action");
    choose_metadata_filter(&mut app, GameFilterField::Year, "1994");
    assert!(app.game_filter_options[GameFilterField::Year.index()]
        .as_ref()
        .unwrap()
        .iter()
        .any(|choice| choice.label() == "1995"));
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Nested", "Alpha"]
    );

    // Title search and metadata use AND. Clearing either one leaves the
    // other projection active.
    app.filter = "GAMMA".into();
    app.apply_filter();
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Nested"]
    );
    app.clear_filter();
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Nested", "Alpha"]
    );
    app.filter = "ALPHA".into();
    app.apply_filter();
    app.clear_game_filters();
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha"]
    );

    app.filter.clear();
    app.apply_filter();
    app.open_game_filters();
    choose_metadata_filter(&mut app, GameFilterField::Language, "Unknown");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(app.game_filters.label(GameFilterField::Language), "Unknown");
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Nested", "Gamma"]
    );

    // Active metadata makes both random actions local to the visible games.
    app.random_launches = false;
    app.random_here(false);
    assert_eq!(app.here[app.game_list.selected()].name, "Gamma");
    app.random_here(true);
    assert_eq!(app.here[app.game_list.selected()].name, "Gamma");

    // Clear Filters is independent of Clear Search and is also storage-free.
    app.filter = "GAMMA".into();
    app.apply_filter();
    app.open_context();
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == ContextPage::Find.label())
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == CLEAR_FILTERS)
            .unwrap(),
    );
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(!app.game_filters.is_active());
    assert_eq!(app.filter, "GAMMA");
    assert_eq!(
        app.here
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Gamma"]
    );
    assert_eq!(std::fs::read(&app.settings_path).ok(), settings_before);
    let cache_after = if app.cache_dir.exists() {
        cache_snapshot(&app.cache_dir)
    } else {
        Vec::new()
    };
    assert_eq!(cache_after, cache_before);

    // Same-place changes re-read the rows but retain both temporary filter
    // kinds. The real fixture rows have no language metadata, so both remain
    // visible through Unknown after the synthetic rows above are replaced.
    app.filter = "GAME".into();
    app.game_filters
        .choose(GameFilterField::Language, &GameFilterChoice::Unknown);
    app.apply_filter();
    app.relist_here_preserving_game_filters();
    assert!(
        app.filter.is_empty(),
        "same-place relists retain the established title-search reset"
    );
    assert_eq!(app.game_filters.label(GameFilterField::Language), "Unknown");
    assert_eq!(
        app.here
            .iter()
            .filter(|row| !row.is_folder())
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["First Game.nes", "Second Game.nes"]
    );

    app.game_filters
        .choose(GameFilterField::Genre, &GameFilterChoice::Unknown);
    app.clear_place_filters();
    assert!(!app.game_filters.is_active());
    assert!(app.filter.is_empty());
    let restarted = unopened_fixture_app(&root, app.window.clone(), Settings::default());
    assert!(!restarted.game_filters.is_active());
    drop(restarted);
    app.ui.hide().unwrap();
}

pub(super) fn run_ui_acceptance_flow(window: Rc<MinimalSoftwareWindow>) {
    let root = fixture_directory();
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture").unwrap();
    }
    let complete_description = information_row().details.desc;
    let gamelist_path = root.join("games/NES/gamelist.xml");
    let gamelist_xml = format!("<gameList><game><path>First Game.nes</path><desc><![CDATA[{complete_description}]]></desc></game><game><path>Second Game.nes</path><desc><![CDATA[{complete_description}]]></desc></game></gameList>");
    std::fs::write(&gamelist_path, &gamelist_xml).unwrap();
    run_cores_browser_flow(&root, window.clone());
    run_misterzine_browser_flow(&root, window.clone());
    run_browse_bar_settings_flow(&root, window.clone());
    run_game_name_display_flow(&root, window.clone());
    run_details_style_flow(&root, window.clone());
    run_handheld_category_flow(&root, window.clone());
    run_scripts_flow(&root, window.clone());
    run_neogeo_romset_flow(&root, window.clone());
    run_artwork_matte_flow(&root, window.clone());
    run_automatic_pack_consent_flow(&root, window.clone());
    run_legacy_pack_cache_adoption_flow(&root, window.clone());
    #[cfg(unix)]
    run_arcade_descriptor_failures_flow(&root, window.clone());
    run_auto_source_choice_flow(&root, window.clone());
    run_degraded_pack_acknowledgement_flow(&root, window.clone());
    run_arcade_core_descriptor_favourite_flow(&root, window.clone());
    run_folder_artwork_flow(&root, window.clone());
    run_paged_theme_name_flow(&root, window.clone());
    run_theme_editor_save_changes_flow(&root, window.clone());
    run_metadata_filter_flow(&root, window.clone());
    run_last_played_flow(&root, window.clone());
    let mut app = fixture_app(&root, window.clone(), Settings::default());
    run_selected_controls_flow(&mut app);
    run_artwork_visibility_flow(&mut app);
    run_scraper_image_choice_flow(&mut app);
    app.filter = "GAME".into();
    app.apply_filter();
    app.game_list.select(1);
    let selected = row_key(&app.here[app.game_list.selected()]);
    let view = app.layout;
    app.handle(Action::Menu);
    assert_eq!(app.screen, Screen::Menu);
    app.menu_list.select(
        app.menu
            .iter()
            .position(|entry| entry == "Options")
            .unwrap(),
    );
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::OptionsRoot);
    let root_selection = app.options_root_list.selected();
    let speed = app.speed;
    app.handle(Action::Slower);
    app.handle(Action::Faster);
    assert_eq!(app.options_root_list.selected(), root_selection);
    assert_eq!(
        app.speed, speed,
        "Options categories must not change browsing speed"
    );

    for page in OptionsPage::ALL {
        app.options_root_list.select(page.index());
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::Options);
        assert_eq!(app.options_page, page);
        assert_eq!(app.option_ids(), page.ids());
        let last = page.ids().len() - 1;
        app.active_list_mut().select(last);
        app.handle(Action::Quit);
        assert_eq!(app.screen, Screen::OptionsRoot);
        assert_eq!(app.options_root_list.selected(), page.index());
        app.handle(Action::Accept);
        assert_eq!(
            app.active_list().selected(),
            last,
            "{page:?} must remember its row"
        );
        app.handle(Action::Quit);
    }

    select_option(&mut app, OptionsPage::Appearance, OptionId::Theme);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::ThemeEditor);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Options);
    assert_eq!(app.options_page, OptionsPage::Appearance);
    assert_eq!(
        app.option_ids()[app.active_list().selected()],
        OptionId::Theme
    );

    select_option(&mut app, OptionsPage::Library, OptionId::ScrapeAll);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scraper);
    assert!(matches!(app.scraper_scope, crate::scraper::Scope::All));
    assert!(
        app.scraper_job.is_none(),
        "opening settings must not start network work"
    );
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Options);
    assert_eq!(app.options_page, OptionsPage::Library);
    assert_eq!(
        app.option_ids()[app.active_list().selected()],
        OptionId::ScrapeAll
    );
    app.handle(Action::Accept);
    app.screen = Screen::ScraperProgress;
    app.scraper_terminal = Some(ScraperTerminal::Finished);
    app.close_scraper_progress();
    assert_eq!(
        app.screen,
        Screen::Options,
        "global scrape completion returns to Options"
    );
    assert_eq!(app.options_page, OptionsPage::Library);
    assert_eq!(
        app.option_ids()[app.active_list().selected()],
        OptionId::ScrapeAll
    );

    select_option(&mut app, OptionsPage::Appearance, OptionId::ShowArt);
    app.handle(Action::Faster);
    assert!(!app.show_art);
    let settings_path = app.settings_path.clone();
    std::fs::write(root.join("blocked"), b"not a directory").unwrap();
    app.settings_path = root.join("blocked/settings.toml");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Options, "failed saves keep their page");
    assert!(!app.show_art, "failed saves retain the current live choice");
    assert!(app
        .message
        .as_deref()
        .is_some_and(|message| message.starts_with("Settings not saved:")));
    app.handle(Action::Quit);
    assert!(
        app.message.is_none(),
        "the first Back only dismisses the error"
    );
    assert_eq!(app.screen, Screen::Options);
    app.settings_path = settings_path;
    app.handle(Action::Quit);
    assert_eq!(
        app.screen,
        Screen::OptionsRoot,
        "Back retries after storage recovery"
    );
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().show_art,
        Some(false)
    );

    select_option(
        &mut app,
        OptionsPage::Appearance,
        OptionId::ResetCustomViews,
    );
    app.settings.custom_views.categories = Some("gallery".into());
    for direction in [Action::Slower, Action::Faster] {
        app.handle(direction);
        assert!(app.pending.is_none());
        assert_eq!(
            app.settings.custom_views.categories.as_deref(),
            Some("gallery")
        );
    }
    app.handle(Action::Accept);
    assert_eq!(app.pending, Some(Pending::ResetCustomViews));
    app.handle(Action::Quit);
    assert_eq!(
        app.settings.custom_views.categories.as_deref(),
        Some("gallery")
    );
    app.handle(Action::Accept);
    app.handle(Action::Accept);
    assert!(app.settings.custom_views.is_empty());
    assert!(Settings::load(&app.settings_path)
        .unwrap()
        .custom_views
        .is_empty());
    app.handle(Action::Quit);

    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::OptionsRoot);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Menu);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected);
    assert_eq!(app.filter, "GAME");
    assert_eq!(app.layout, view);

    let info = information_row();
    let row = &mut app.here[app.game_list.selected()];
    row.details = info.details.clone();
    row.genre = info.genre;
    let expected_information = game_information(row);
    row.details.desc = "First paragraph.".to_string();
    let before_information = cache_snapshot(&app.cache_dir);
    app.handle(Action::Context);
    assert_eq!(
        app.menu.first().map(String::as_str),
        Some(ContextPage::Game.label())
    );
    app.handle(Action::Accept);
    assert_eq!(app.menu.first().map(String::as_str), Some(GAME_INFORMATION));
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Information);
    assert!(app
        .ui
        .get_information_text()
        .contains("Reading full description"));
    app.finish_background_work_for_headless();
    assert_eq!(app.ui.get_information_text().as_str(), expected_information);
    assert_eq!(
        cache_snapshot(&app.cache_dir),
        before_information,
        "opening full information must not rebuild or rewrite compact caches"
    );
    assert_eq!(
        app.here[app.game_list.selected()].details.desc,
        "First paragraph."
    );
    assert!(
        app.handle(Action::Accept).is_none(),
        "Information must not launch the game"
    );
    assert_eq!(app.screen, Screen::Information);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Context);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Context);
    assert!(app.context_is_root());
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected);
    assert_eq!(app.filter, "GAME");
    assert_eq!(app.layout, view);

    std::fs::write(&gamelist_path, "<gameList><game></broken>").unwrap();
    app.open_context();
    app.open_information();
    app.finish_background_work_for_headless();
    assert!(app
        .ui
        .get_information_text()
        .contains("Could not read description"));
    assert!(app.ui.get_information_text().contains("gamelist"));
    assert!(
        !app.ui.get_information_text().contains("First paragraph."),
        "a read error must not silently masquerade as the old short description"
    );
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Context);
    std::fs::write(&gamelist_path, &gamelist_xml).unwrap();
    app.open_information();
    app.handle(Action::Quit);
    app.finish_background_work_for_headless();
    assert_eq!(
        app.screen,
        Screen::Context,
        "cancelled reads return to Actions"
    );
    assert!(app.information.is_none());
    app.handle(Action::Quit);
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected);
    assert_eq!(app.filter, "GAME");

    let previous_screen = app.screen;
    let previous_progress = app.scraper_progress.clone();
    app.screen = Screen::ScraperProgress;
    app.apply_geometry();
    app.scraper_progress = crate::scraper::Progress {
        phase: crate::scraper::Phase::Scraping,
        total: 10,
        completed: 4,
        ..Default::default()
    };
    paint_index_frame(&mut app);
    assert!(app.ui.get_operation_footer_visible());
    assert!(!app.ui.get_bottom_bar_visible());
    app.handle(Action::Accept);
    paint_index_frame(&mut app);
    assert!(app.scraper_details);
    assert!(app.ui.get_bottom_bar_visible());
    assert!(!app.ui.get_operation_footer_visible());
    app.handle(Action::Quit);
    paint_index_frame(&mut app);
    assert!(!app.scraper_details);
    assert!(!app.ui.get_bottom_bar_visible());
    assert!(app.ui.get_operation_footer_visible());
    let running_controls = app.ui.get_operation_controls();
    let running_progress = app.ui.get_operation_progress();
    app.pending = Some(Pending::CancelScrape);
    app.message = Some("Cancel scrape for Fixture System?\n\nA yes, B no".into());
    paint_index_frame(&mut app);
    assert!(!app.ui.get_operation_footer_visible());
    assert!(!app.ui.get_bottom_bar_visible());
    capture_live_if_requested(&mut app, "scraper-cancel-confirmation");
    app.handle(Action::Quit);
    paint_index_frame(&mut app);
    assert!(app.pending.is_none());
    assert!(app.message.is_none());
    assert_eq!(app.screen, Screen::ScraperProgress);
    assert!(!app.scraper_cancelling);
    assert!(app.ui.get_operation_footer_visible());
    assert_eq!(app.ui.get_operation_controls(), running_controls);
    assert_eq!(app.ui.get_operation_progress(), running_progress);
    capture_live_if_requested(&mut app, "scraper-cancel-declined");
    app.screen = previous_screen;
    app.scraper_progress = previous_progress;
    app.apply_geometry();

    let long_error = format!(
        "Indexing failed\n{}Final actionable cause",
        "A detailed error line.\n".repeat(40)
    );
    app.message = Some(long_error.clone());
    paint_index_frame(&mut app);
    assert!(app.ui.get_overlay_max_scroll() > 0.0);
    assert!(!app.ui.get_bottom_bar_visible());
    app.handle(Action::Down);
    assert!(app.ui.get_overlay_offset() > 0.0);
    assert_eq!(app.message.as_deref(), Some(long_error.as_str()));
    app.handle(Action::End);
    assert_eq!(app.ui.get_overlay_offset(), app.ui.get_overlay_max_scroll());
    app.handle(Action::Quit);
    assert!(app.message.is_none());
    paint_index_frame(&mut app);
    assert_eq!(app.ui.get_bottom_bar_visible(), app.ui.get_show_bar());
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected);
    app.message = Some("A short notice".into());
    paint_index_frame(&mut app);
    assert_eq!(app.ui.get_overlay_max_scroll(), 0.0);
    app.handle(Action::Down);
    assert!(
        app.message.is_none(),
        "short notices retain their existing dismissal behavior"
    );

    select_option(&mut app, OptionsPage::Library, OptionId::ResetHidden);
    app.settings.hidden_paths.push("fixture hidden row".into());
    for direction in [Action::Slower, Action::Faster] {
        app.handle(direction);
        assert!(app.pending.is_none());
        assert_eq!(app.settings.hidden_paths.len(), 1);
    }
    app.handle(Action::Accept);
    assert_eq!(app.pending, Some(Pending::ResetHidden));
    app.handle(Action::Quit);
    assert_eq!(app.settings.hidden_paths.len(), 1);
    assert_eq!(app.filter, "GAME", "cancelling a reset must not relist");
    app.handle(Action::Accept);
    app.handle(Action::Accept);
    assert!(app.settings.hidden_paths.is_empty());
    assert!(Settings::load(&app.settings_path)
        .unwrap()
        .hidden_paths
        .is_empty());
    assert!(
        app.filter.is_empty(),
        "confirmed Unhide Everything retains v0.5.0's relist/search-reset behaviour"
    );
    assert_eq!(row_key(&app.here[app.game_list.selected()]), selected);
    app.handle(Action::Quit);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::OptionsRoot);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Menu);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);

    app.reopen_context_for(SCRAPE_FOLDER);
    assert!(app.menu.iter().any(|entry| entry == SCRAPE_FOLDER));
    assert!(app.menu.iter().any(|entry| entry == SCRAPE_GAME));
    let scrape_row = app
        .menu
        .iter()
        .position(|entry| entry == SCRAPE_FOLDER)
        .unwrap();
    app.menu_list.select(scrape_row);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scraper);
    assert!(app.scraper_job.is_none());
    app.screen = Screen::ScraperProgress;
    app.scraper_terminal = Some(ScraperTerminal::Finished);
    app.close_scraper_progress();
    assert_eq!(
        app.screen,
        Screen::Context,
        "context scrape completion returns to Actions"
    );
    assert_eq!(app.menu_list.selected(), scrape_row);
    run_manual_search_ui_flow(&mut app, &root);
    app.set_screen(Screen::Browse);
    app.search_for("GAME");
    app.refresh();
    assert!(app.ui.get_find_filtering());
    assert_eq!(app.ui.get_find_query(), "GAME");
    assert_eq!(app.ui.get_heading(), "Search This Folder");
    assert_eq!(app.ui.get_grid_help(), "A Type B Back X Del Y Clear");
    assert_eq!(app.rows.row_count(), FIND_CELLS.chars().count());
    app.handle(Action::Context);
    app.refresh();
    assert_eq!(app.ui.get_find_query(), "GAM", "X deletes the last letter");
    app.handle(Action::Menu);
    app.refresh();
    assert_eq!(app.ui.get_find_query(), "", "Y clears the live filter");
    app.handle(Action::Accept);
    app.refresh();
    assert_eq!(app.ui.get_find_query(), "A", "A adds the selected letter");
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(
        app.filter, "A",
        "B keeps the search when returning to Browse"
    );
    assert!(!app.ui.get_find_filtering());
    app.open_find(FindMode::Jump);
    assert!(!app.ui.get_find_filtering());
    assert_eq!(app.ui.get_grid_help(), "A Pick   B Back");
    app.handle(Action::Quit);
    app.filter.clear();
    app.apply_filter();
    app.settings.artwork_pack_roots.insert(
        "NES".into(),
        root.join("docs").to_string_lossy().into_owned(),
    );
    app.effective_artwork_pack_roots = crate::artwork_source::resolve(
        &app.all_systems,
        &app.settings,
        &app.cache_dir,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .unwrap()
    .roots;
    app.open_context();
    for scrape in [SCRAPE_SYSTEM, SCRAPE_FOLDER, SCRAPE_GAME] {
        assert!(
            !app.context_actions.iter().any(|entry| entry == scrape),
            "pack selection hides scrape actions"
        );
    }
    assert!(app
        .context_actions
        .iter()
        .any(|entry| entry == GAME_DATA_SOURCE));
    app.settings.artwork_pack_roots.clear();
    let saved = Settings::load(&app.settings_path).unwrap();
    drop(app);
    let restarted = fixture_app(&root, window.clone(), saved);
    assert!(
        !restarted.show_art,
        "saved Options changes survive a new App instance"
    );
    assert!(restarted.settings.custom_views.is_empty());
    assert!(restarted.settings.hidden_paths.is_empty());
    drop(restarted);
    run_favorite_information_flow(&root, window.clone());
    run_hold_shortcut_flow(&root, window.clone());
    run_main_favourites_flow(&root, window.clone());
    run_scraper_cache_refresh_flow(&root, window.clone());
    run_scraper_unresolved_report_flow(&root, window.clone());
    run_indexing_ui_flow(&root, window.clone());
    capture_ui_if_requested(&root, window);
    std::fs::remove_dir_all(root).unwrap();
}
