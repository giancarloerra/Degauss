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
        core_version: true,
        core_version_override: true,
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
        CORE_VERSION,
        USE_DEFAULT_CORE_VERSION,
        GAME_DATA_SOURCE,
        JUMP,
        SEARCH,
        CLEAR_SEARCH,
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
        themes: Default::default(),
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
    assert!(!app.random_shortcut_enabled());
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

fn run_hold_y_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("hold-y");
    std::fs::create_dir_all(root.join("games/NES")).unwrap();
    std::fs::create_dir(root.join("_Console")).unwrap();
    std::fs::write(root.join("_Console/NES.rbf"), b"fixture core").unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(root.join("games/NES").join(name), b"fixture game").unwrap();
    }
    let mut app = fixture_app(&root, window, Settings::default());
    assert!(!app.hold_y_random, "existing settings retain immediate Y");
    assert!(!app.random_shortcut_enabled());
    app.handle(Action::Menu);
    assert_eq!(app.screen, Screen::Menu);
    app.handle(Action::Quit);
    select_option(&mut app, OptionsPage::Navigation, OptionId::HoldYRandom);
    app.handle(Action::Accept);
    assert!(app.hold_y_random);
    app.handle(Action::Quit);
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().hold_y_random,
        Some(true)
    );
    app.screen = Screen::Browse;
    assert!(app.random_shortcut_enabled());
    let held = Instant::now();
    let mut repeater = Repeater::new(RepeatConfig::default());
    repeater.set_random_hold(app.random_shortcut_enabled());
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
        repeater.set_random_hold(app.random_shortcut_enabled());
        let pressed = Instant::now();
        assert_eq!(repeater.press(Action::Menu, pressed), None);
        let due = repeater.tick(pressed + Duration::from_secs(1));
        assert_eq!(due, vec![Action::RandomShortcut]);
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
            !app.random_shortcut_enabled(),
            "{screen:?} must never delay Y or launch a random game"
        );
    }
    app.screen = Screen::Browse;
    for browsing in [Browsing::Systems, Browsing::Categories] {
        app.browsing = browsing;
        assert!(!app.random_shortcut_enabled());
    }
    app.browsing = Browsing::Games;
    app.message = Some("Fixture message".into());
    assert!(!app.random_shortcut_enabled());
    app.message = None;
    app.pending = Some(Pending::Exit);
    assert!(!app.random_shortcut_enabled());
    app.pending = None;
    app.index_terminal = Some(IndexOverview::default());
    assert!(!app.random_shortcut_enabled());
    app.index_terminal = None;
    app.scraper_pending_terminal = Some(ScraperTerminal::Finished);
    assert!(!app.random_shortcut_enabled());
    app.scraper_pending_terminal = None;

    app.saver_return = Screen::Browse;
    app.screen = Screen::Screensaver;
    repeater.set_random_hold(app.random_shortcut_enabled());
    let wake = Instant::now();
    assert_eq!(repeater.press(Action::Menu, wake), Some(Action::Menu));
    let selected = app.game_list.selected();
    assert!(app.handle(Action::Menu).is_none());
    assert_eq!(app.screen, Screen::Browse);
    repeater.set_random_hold(app.random_shortcut_enabled());
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
    app.hold_y_random = true;
    assert!(!app.random_shortcut_enabled());
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
        core_version: true,
        core_version_override: true,
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
        problem: "Fixture Archive.zip could not be read: incomplete central directory. Healthy folders remain available.".into(),
        report: "Indexing finished with problems. Fixture Archive.zip: incomplete central directory. Healthy folders remain available.".into(),
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
    app.open_find(FindMode::NewFolder);
    capture_frame(&mut app, &directory, "new-favorite-folder", 352, 240);
    app.filter.clear();
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
        app.select(3);
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

fn run_fresh_auto_pack_index_flow(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let assert_operation_controls = |app: &mut App, details: bool, controls: &str| {
        app.refresh();
        assert!(
            !app.show_bar,
            "operation controls must not change the browse-bar preference"
        );
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
    let root = root.join("automatic-pack-indexing");
    let games = root.join("games/NES");
    let extra = root.join("games/NES-extra");
    let docs = root.join("docs");
    let artwork = docs.join("NES/Artwork");
    for directory in [&games, &extra, &artwork] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(games.join("Known.nes"), b"first rom").unwrap();
    std::fs::write(extra.join("Second.nes"), b"second rom").unwrap();
    std::fs::write(
        artwork.join("manifest.tsv"),
        "#key\tstyle\tss_system_id\nKnown\tbox-2D\t3\nSecond\tbox-2D\t3\n",
    )
    .unwrap();
    std::fs::write(
        artwork.join("index.tsv"),
        "#name\tcrc\tsize\tkey\nKnown\t\t\tKnown\nSecond\t\t\tSecond\n",
    )
    .unwrap();
    std::fs::write(artwork.join("gameinfo.tsv"), "#key\tname\tyear\tgenre\tdeveloper\tplayers\nKnown\tPack First\t1990\tAction\tStudio\t1\nSecond\tPack Second\t1991\tPuzzle\tStudio\t2\n").unwrap();
    for key in ["Known", "Second"] {
        std::fs::write(artwork.join(format!("{key}.jpg")), crate::covers::JPEG_16).unwrap();
    }
    let prepare = |app: &mut App| {
        app.all_systems[0].paths = vec![games.clone(), extra.clone()];
        app.systems = app.all_systems.clone();
        app.source_resolution = Some(crate::artwork_source::resolved_job_at_bases(
            &app.all_systems,
            &app.settings,
            std::slice::from_ref(&root),
        ));
    };
    let assert_complete = |app: &App, games: usize| {
        assert!(app.build.is_none());
        assert!(app.source_job.is_none());
        assert!(app.source_recovery_queue.is_empty());
        assert_eq!(app.index.as_ref().unwrap().systems["NES"].games, games);
        assert_eq!(
            crate::cache::load_index(&app.cache_dir).unwrap().systems["NES"].games,
            games
        );
        assert_eq!(
            app.total_games, games,
            "About and total counts must include prepared Pack caches"
        );
        assert!(app.ui.get_about_line().contains(&format!("{games} games")));
        assert!(!app.empty_systems.as_ref().unwrap().contains("NES"));
        assert_eq!(
            crate::artwork_source::mode(&app.settings, "NES"),
            crate::artwork_source::Mode::Automatic
        );
        assert!(app.settings.artwork_pack_roots.is_empty());
        let data = crate::cache::load_artwork_pack_data(&app.cache_dir, "NES").unwrap();
        assert!(data.fingerprints_complete);
        assert_eq!(data.cache.summary(&Place::Roots).games, games);
        assert!(
            !crate::cache::system_path(&app.cache_dir, "NES").exists(),
            "Pack indexing must not publish a Gamelist cache"
        );
    };
    let browse_bar_off = Settings {
        show_bar: Some(false),
        ..Default::default()
    };
    let mut app = unopened_fixture_app(&root, window.clone(), browse_bar_off.clone());
    prepare(&mut app);
    app.leave_splash();
    app.build_one_system();
    assert_eq!(app.source_label("NES"), "Automatic: Artwork Pack");
    assert!(
        app.open_system.is_none(),
        "cold index is tested before first browse"
    );
    assert!(app.build.as_ref().unwrap().awaiting_frame);
    paint_index_frame(&mut app);
    app.build_one_system();
    assert!(app.source_job.is_some());
    assert_eq!(app.build.as_ref().unwrap().done, 0);
    assert!(
        app.index_terminal.is_none(),
        "Pack preparation must not follow a completed Index All"
    );
    assert!(
        !crate::cache::index_path(&app.cache_dir).exists(),
        "an empty index must not be published ahead of the Pack cache"
    );
    app.handle(Action::Accept);
    assert!(app.index_details);
    assert_operation_controls(&mut app, true, "Up/Down Scroll   B Overview");
    capture_live_if_requested(&mut app, "source-auto-pack-cold-index-details");
    app.handle(Action::Quit);
    assert!(!app.index_details);
    assert!(!app.build.as_ref().unwrap().cancelling);
    assert_operation_controls(&mut app, false, "A Details   B Cancel");
    capture_live_if_requested(&mut app, "source-auto-pack-cold-index-overview");
    app.finish_background_work_for_headless();
    assert_complete(&app, 2);
    assert!(app.open_system.is_none());
    app.refill_saver();
    assert_eq!(
        app.saver_pool.len(),
        2,
        "Pack artwork must be available without first browsing the system"
    );
    assert_eq!(
        app.saver_pool
            .iter()
            .map(|picture| picture.path.clone())
            .collect::<HashSet<_>>(),
        [artwork.join("Known.jpg"), artwork.join("Second.jpg")]
            .into_iter()
            .collect()
    );
    let favorite = root.join("Known.mgl");
    std::fs::write(
        &favorite,
        crate::launch::favorite_mgl(&app.all_systems[0].to_config(), &games.join("Known.nes"))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut favorite_rows = [browse::Row {
        kind: browse::Kind::Play(browse::Launch::File(favorite)),
        ..information_row()
    }];
    enrich_favorite_rows(
        &mut favorite_rows,
        &app.all_systems,
        &app.homes(),
        &app.effective_artwork_pack_roots,
        &app.cache_dir,
        |id, _, _| app.artwork_provider_cache.get(id).cloned(),
    );
    assert_eq!(favorite_rows[0].name, "Pack First");
    assert_eq!(favorite_rows[0].cover, Some(artwork.join("Known.jpg")));
    let initial = cache_snapshot(&app.cache_dir);
    app.start_build(false);
    assert!(
        app.source_recovery_queue.is_empty(),
        "unchanged complete Pack caches are reused"
    );
    app.finish_background_work_for_headless();
    assert_complete(&app, 2);
    assert_eq!(cache_snapshot(&app.cache_dir), initial);

    app.start_build(true);
    app.build_one_system();
    paint_index_frame(&mut app);
    app.build_one_system();
    assert!(app.source_job.is_some());
    app.handle(Action::Quit);
    assert!(app.build.as_ref().unwrap().cancelling);
    assert!(app.source_cancelling);
    app.finish_background_work_for_headless();
    assert_eq!(app.index_terminal.as_ref().unwrap().state, "Cancelled");
    assert_operation_controls(&mut app, false, "A Details   B Back");
    assert_eq!(
        cache_snapshot(&app.cache_dir),
        initial,
        "cancellation before publication preserves the complete index and Pack cache"
    );
    capture_live_if_requested(&mut app, "source-auto-pack-cancelled");
    app.handle(Action::Quit);
    app.start_build(true);
    app.finish_background_work_for_headless();
    assert_complete(&app, 2);
    assert_eq!(app.index_terminal.as_ref().unwrap().games, 2);
    assert_eq!(app.index_terminal.as_ref().unwrap().done, 1);
    assert_operation_controls(&mut app, false, "A Details   B Back");
    capture_live_if_requested(&mut app, "source-auto-pack-rebuilt");
    app.handle(Action::Quit);

    let pack_cache = crate::cache::artwork_pack_system_path(&app.cache_dir, "NES");
    std::fs::remove_file(&pack_cache).unwrap();
    app.ui.hide().unwrap();
    drop(app);
    let mut app = unopened_fixture_app(&root, window, browse_bar_off);
    assert!(
        app.build.is_none(),
        "the existing global index is loaded at startup"
    );
    prepare(&mut app);
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert_complete(&app, 2);
    assert!(
        app.open_system.is_none(),
        "missing source cache recovery must not require browsing"
    );

    let provider = app.artwork_provider_cache.get("NES").unwrap().clone();
    app.open_system_now_with_provider(Some(provider));
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.enter(Place::Dir(games.clone()));
    assert_eq!(app.here[0].name, "Pack First");
    assert_eq!(app.here[0].cover, Some(artwork.join("Known.jpg")));
    app.start_build(true);
    app.finish_background_work_for_headless();
    assert_complete(&app, 2);
    assert!(
        app.artwork_provider.is_some(),
        "Index All must reattach the prepared provider to the open Pack system"
    );
    assert_eq!(app.here[0].name, "Pack First");
    assert_eq!(app.here[0].cover, Some(artwork.join("Known.jpg")));
    assert_eq!(app.here[0].genre.as_deref(), Some("Action"));
    app.handle(Action::Quit);
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
    std::fs::write(games.join("Third.nes"), b"third rom").unwrap();
    app.rebuild_open_system_resolved();
    app.finish_background_work_for_headless();
    assert_complete(&app, 3);
    app.message = None;
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
    assert!(app
        .index_terminal
        .as_ref()
        .unwrap()
        .problem
        .contains("Not a directory"));
    assert_eq!(cache_snapshot(&app.cache_dir), before_failure);
    assert_eq!(app.total_games, 3);
    assert_operation_controls(&mut app, false, "A Details   B Back");
    capture_live_if_requested(&mut app, "source-auto-pack-index-failed");
    std::fs::remove_file(&games).unwrap();
    std::fs::rename(&held, &games).unwrap();
    let complete = crate::cache::load_artwork_pack_data(&app.cache_dir, "NES").unwrap();
    crate::cache::stage_transactional(
        &app.cache_dir,
        crate::cache::CacheKind::ArtworkPack,
        vec![crate::cache::StagedSystemCache {
            id: "NeoGeo".into(),
            cache: complete.cache,
            fingerprints: complete.fingerprints,
            fingerprints_complete: true,
        }],
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .unwrap()
    .install()
    .unwrap();
    let mut first = app.all_systems[0].clone();
    first.def.id = "NeoGeo".into();
    let mut second = first.clone();
    second.def.id = "NeoGeoMVS".into();
    app.all_systems = vec![first, second];
    app.effective_artwork_pack_roots.clear();
    app.effective_artwork_pack_roots
        .insert("NeoGeo".into(), docs.to_string_lossy().into_owned());
    app.start_build(false);
    assert_eq!(app.source_recovery_queue.iter().map(String::as_str).collect::<Vec<_>>(), ["NeoGeo"],
        "a missing later member must queue its shared source group even when the first member is complete");
    app.build = None;
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
        format!("Automatic: {SOURCE_GAMELIST}")
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

    app.source_switch_automatic = false;
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
    app.handle(Action::Quit);
    app.finish_background_work_for_headless();
    assert!(
        app.source_problem("NES").is_none(),
        "cancelled uncommitted Automatic selection must keep Gamelist usable"
    );
    assert_eq!(
        crate::artwork_source::mode(&app.settings, "NES"),
        Mode::Gamelist
    );
    assert!(Settings::load(&app.settings_path)
        .unwrap()
        .gamelist_sources
        .contains("NES"));
    assert_eq!(cache_snapshot(&app.cache_dir), restarted_cache);
    app.open_game_data_source();
    app.menu_list.select(0);
    app.handle(Action::Accept);
    assert!(
        app.source_resolution.is_some(),
        "Automatic must resolve on its worker"
    );
    assert!(
        app.settings.gamelist_sources.contains("NES"),
        "the saved choice remains until resolution and save succeed"
    );
    app.finish_background_work_for_headless();
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
    app.resolve_artwork_sources(Some("NES"), SourceResolutionAction::Automatic);
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
    app.open_system_now_with_provider(None);
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
    let start = |window: Rc<MinimalSoftwareWindow>| {
        let mut app = unopened_fixture_app(&root, window, Settings::load(&settings_path).unwrap());
        app.open_system_by_index(0);
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

    // 1. The first look at an incomplete pack warns. Putting the warning up
    // is not seeing it: a headless render opens a system the same way and
    // nobody dismisses what it draws, so nothing is written down yet.
    std::fs::remove_file(artwork.join("Second.jpg")).unwrap();
    let app = start(window.clone());
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
    std::fs::remove_file(artwork.join("index.tsv")).unwrap();
    reopen(&mut app);
    assert!(message_contains(&app, "is incomplete"), "{:?}", app.message);
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

    // 5. Invalid and unavailable packs need acting on: shown at every start,
    // and never written down.
    write_manifest(&["../escape"]);
    for _ in 0..2 {
        let mut app = start(window.clone());
        assert_eq!(
            app.artwork_provider.as_ref().unwrap().health,
            ProviderHealth::Invalid
        );
        assert!(message_contains(&app, "is invalid"), "{:?}", app.message);
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
    run_browse_bar_settings_flow(&root, window.clone());
    run_scripts_flow(&root, window.clone());
    run_artwork_matte_flow(&root, window.clone());
    run_fresh_auto_pack_index_flow(&root, window.clone());
    run_auto_source_choice_flow(&root, window.clone());
    run_degraded_pack_acknowledgement_flow(&root, window.clone());
    run_arcade_core_descriptor_favourite_flow(&root, window.clone());
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
    run_hold_y_flow(&root, window.clone());
    run_scraper_cache_refresh_flow(&root, window.clone());
    run_indexing_ui_flow(&root, window.clone());
    capture_ui_if_requested(&root, window);
    std::fs::remove_dir_all(root).unwrap();
}
