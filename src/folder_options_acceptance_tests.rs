use super::*;

fn choose(app: &mut App, action: &str, delta: isize) {
    app.screen = Screen::Browse;
    app.open_context();
    let group = app
        .menu
        .iter()
        .position(|entry| entry == "Folder Options")
        .unwrap();
    app.menu_list.select(group);
    app.handle(Action::Accept);
    assert_eq!(app.context_page, Some(ContextPage::Appearance));
    let mut expected = vec![CHANGE_VIEW];
    if app.has_custom_view() {
        expected.push(USE_GLOBAL_VIEW);
    }
    expected.extend([MANAGE_VIEWS, FOLDER_ROTATION, FOLDER_POSITION, FOLDER_NAMES]);
    if app.home_pin_target().is_some() {
        expected.push(ADD_HOME);
    }
    assert_eq!(
        app.menu,
        expected.into_iter().map(str::to_string).collect::<Vec<_>>()
    );
    let at = app.menu.iter().position(|entry| entry == action).unwrap();
    app.menu_list.select(at);
    assert_eq!(app.context_scope(), app.here_label());
    app.handle(if delta < 0 {
        Action::Slower
    } else {
        Action::Faster
    });
}

fn enter(app: &mut App, name: &str) {
    app.screen = Screen::Browse;
    select_row_named(app, name);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.message.is_none(), "{:?}", app.message);
}

pub(super) fn run(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    for (width, height) in [(352, 240), (960, 540)] {
        let root = root.join(format!("folder-options-{width}"));
        let games = root.join("games/NES");
        for child in ["Child", "Sibling"] {
            std::fs::create_dir_all(games.join(child)).unwrap();
        }
        for name in ["First Game (USA).nes", "Second Game [Rev 1].nes"] {
            std::fs::write(games.join(name), b"fixture").unwrap();
        }
        std::fs::write(games.join("gamelist.xml"), "<gameList><game><path>First Game (USA).nes</path><name>First Game (USA)</name><genre>Puzzle</genre></game><game><path>Second Game [Rev 1].nes</path><name>Second Game [Rev 1]</name><genre>Platform</genre></game></gameList>").unwrap();
        let mut app = unopened_fixture_app(
            &root,
            window.clone(),
            Settings {
                show_empty: Some(true),
                ..Default::default()
            },
        );
        app.open_system_by_index(0);
        app.leave_splash();
        app.physical_width = width;
        app.physical_height = height;
        app.width = width;
        app.height = height;
        app.window.set_size(slint::PhysicalSize::new(width, height));
        app.apply_geometry();
        let root_key = app.current_folder_place().unwrap();
        let original_cache = cache_snapshot(&app.cache_dir);
        assert!(app.settings.folder_preferences.is_empty());
        assert_eq!(app.screen_rotation, ScreenRotation::Off);
        assert!(!app.folders_last);
        assert_eq!(app.game_name_display, GameNameDisplay::Full);
        app.layout = Layout::List;
        app.remember_view();
        let existing_views = app.settings.custom_views.clone();
        app.resolve_view();

        // A selected child must not acquire the preferences of the open root.
        select_row_named(&mut app, "Child");
        choose(&mut app, FOLDER_POSITION, 1);
        assert!(!app.folders_last);
        choose(&mut app, FOLDER_POSITION, 1);
        assert!(app.folders_last);
        assert_eq!(app.here.last().unwrap().name, "Sibling");
        assert_eq!(app.here[app.game_list.selected()].name, "Child");
        assert_eq!(app.settings.folder_preferences[&root_key.0].len(), 1);
        assert_eq!(app.settings.folders_last, None);

        // Name choices keep the real launch target and the active search.
        app.screen = Screen::Browse;
        select_row_named(&mut app, "First Game (USA)");
        let target = row_target(&app.here[app.game_list.selected()]);
        for mode in GameNameDisplay::ALL {
            choose(&mut app, FOLDER_NAMES, 1);
            assert_eq!(app.game_name_display, mode);
            assert_eq!(row_target(&app.here[app.game_list.selected()]), target);
            assert_eq!(app.settings.game_name_display, None);
        }
        choose(&mut app, FOLDER_NAMES, 1);
        assert_eq!(app.folder_preference_value(FOLDER_NAMES), "Use Global");
        app.filter = "First".into();
        app.apply_filter();
        choose(&mut app, FOLDER_POSITION, -1);
        assert_eq!(app.filter, "First");
        assert_eq!(app.here.len(), 1);
        assert_eq!(row_target(&app.here[0]), target);
        app.clear_filter();
        app.game_filters.choose(
            GameFilterField::Genre,
            &GameFilterChoice::Known {
                label: "Puzzle".into(),
                key: "puzzle".into(),
            },
        );
        app.apply_filter();
        select_row_named(&mut app, "First Game (USA)");
        let filters = app.game_filters.clone();
        choose(&mut app, FOLDER_POSITION, 1);
        assert_eq!(app.game_filters, filters);
        assert_eq!(app.here.iter().filter(|row| !row.is_folder()).count(), 1);
        assert_eq!(row_target(&app.here[app.game_list.selected()]), target);
        app.clear_game_filters();
        choose(&mut app, FOLDER_POSITION, -1);
        let capture_directory = std::env::var_os("DEGAUSS_UI_CAPTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        capture_frame(
            &mut app,
            &capture_directory,
            "folder-options",
            width,
            height,
        );

        // Off is an explicit choice even when it matches Global.
        choose(&mut app, FOLDER_ROTATION, 1);
        assert!(app.pending.is_none());
        assert_eq!(app.folder_preference_value(FOLDER_ROTATION), "Off");
        choose(&mut app, FOLDER_ROTATION, 1);
        assert_eq!(app.screen_rotation, ScreenRotation::Clockwise);
        assert_eq!((app.width, app.height), (height, width));
        assert!(matches!(app.pending, Some(Pending::KeepRotation { .. })));
        app.handle(Action::Quit);
        assert_eq!(app.screen_rotation, ScreenRotation::Off);
        assert_eq!(app.folder_preference_value(FOLDER_ROTATION), "Off");
        choose(&mut app, FOLDER_ROTATION, 1);
        app.rotation_preview_deadline = Some(Instant::now() - Duration::from_millis(1));
        app.expire_rotation_preview(Instant::now());
        assert_eq!(app.screen_rotation, ScreenRotation::Off);
        choose(&mut app, FOLDER_ROTATION, 1);
        app.handle(Action::Accept);
        assert_eq!(app.screen_rotation, ScreenRotation::Clockwise);
        assert_eq!(app.settings.screen_rotation, None);
        assert_eq!(app.settings.custom_views, existing_views);
        assert_eq!(app.layout, Layout::List);
        capture_frame(
            &mut app,
            &capture_directory,
            "folder-options-clockwise",
            width,
            height,
        );
        assert_eq!(
            cache_snapshot(&app.cache_dir),
            original_cache,
            "presentation must not rebuild indexes"
        );

        // Empty children do not inherit. Back restores the exact root.
        enter(&mut app, "Child");
        assert!(app.here.is_empty());
        assert_eq!(app.screen_rotation, ScreenRotation::Off);
        assert!(!app.folders_last);
        choose(&mut app, FOLDER_POSITION, -1);
        assert!(app.folders_last);
        app.screen = Screen::Browse;
        app.handle(Action::Quit);
        assert_eq!(app.screen_rotation, ScreenRotation::Clockwise);
        enter(&mut app, "Sibling");
        assert_eq!(app.screen_rotation, ScreenRotation::Off);
        assert!(!app.folders_last);
        app.handle(Action::Quit);

        // Global changes do not rewrite an explicit local rotation/order.
        let local = app.folder_preferences().cloned().unwrap();
        select_option(&mut app, OptionsPage::Library, OptionId::FoldersLast);
        app.handle(Action::Accept);
        assert_eq!(app.settings.folders_last, Some(true));
        assert!(
            !app.folders_last,
            "the open folder keeps its explicit Before Games choice"
        );
        assert_eq!(app.folder_preferences(), Some(&local));
        select_option(&mut app, OptionsPage::Display, OptionId::ScreenRotation);
        app.handle(Action::Accept); // Global CW equals the currently local CW.
        assert!(app.pending.is_none());
        assert_eq!(app.global_screen_rotation, ScreenRotation::Clockwise);
        assert_eq!(app.folder_preferences(), Some(&local));
        app.begin_rotation_preview(ScreenRotation::CounterClockwise);
        app.handle(Action::Accept);
        assert_eq!(app.global_screen_rotation, ScreenRotation::CounterClockwise);
        assert_eq!(app.screen_rotation, ScreenRotation::Clockwise);
        assert_eq!(app.folder_preferences(), Some(&local));
        app.screen = Screen::Browse;
        enter(&mut app, "Sibling");
        assert_eq!(app.screen_rotation, ScreenRotation::CounterClockwise);
        app.handle(Action::Quit);
        assert_eq!(app.screen_rotation, ScreenRotation::Clockwise);

        // Each field can independently return to Global, with normal preview.
        choose(&mut app, FOLDER_ROTATION, 1); // A different image still needs confirmation.
        assert!(matches!(app.pending, Some(Pending::KeepRotation { .. })));
        app.handle(Action::Accept);
        assert_eq!(
            app.folder_preference_value(FOLDER_ROTATION),
            "90° Counterclockwise"
        );
        choose(&mut app, FOLDER_ROTATION, 1); // Use Global, same image.
        assert_eq!(app.folder_preference_value(FOLDER_ROTATION), "Use Global");
        assert_eq!(app.folder_preferences().unwrap().folders_last, Some(false));
        choose(&mut app, FOLDER_NAMES, 1);
        assert_eq!(
            app.folder_preferences().unwrap().game_name_display,
            Some(GameNameDisplay::Full)
        );
        choose(&mut app, FOLDER_POSITION, -1);
        assert_eq!(app.folder_preference_value(FOLDER_POSITION), "Use Global");
        assert!(app.folders_last);
        assert_eq!(
            app.folder_preferences().unwrap().game_name_display,
            Some(GameNameDisplay::Full)
        );
        choose(&mut app, FOLDER_POSITION, 1);
        assert!(!app.folders_last);

        let preferences = app.settings.folder_preferences.clone();
        let view = crate::custom_view::Definition::new(crate::custom_view::Template::Columns);
        app.open_view_editor(None, view);
        app.finish_custom_view("Folder View");
        assert!(app.message.is_none(), "{:?}", app.message);
        assert_eq!(app.custom_view.active.as_ref().unwrap().name, "Folder View");
        app.use_global_view();
        assert!(app.custom_view.active.is_none());
        assert_eq!(
            app.settings.folder_preferences, preferences,
            "Use Global View changes only the layout"
        );

        // A failed save keeps both the previous image and saved preferences.
        let saved = Settings::load(&app.settings_path).unwrap();
        let in_memory = app.settings.clone();
        let saved_bytes = std::fs::read(&app.settings_path).unwrap();
        let settings_path = app.settings_path.clone();
        app.settings_path = root.join("absent/settings.toml");
        choose(&mut app, FOLDER_NAMES, 1);
        assert!(app.message.as_deref().unwrap().contains("not saved"));
        assert_eq!(app.settings, in_memory);
        assert_eq!(std::fs::read(&settings_path).unwrap(), saved_bytes);
        app.handle(Action::Accept);
        choose(&mut app, FOLDER_ROTATION, 1); // Off.
        app.handle(Action::Accept);
        assert_eq!(app.screen_rotation, ScreenRotation::CounterClockwise);
        assert_eq!(app.settings, in_memory);
        assert_eq!(std::fs::read(&settings_path).unwrap(), saved_bytes);
        assert!(app.message.as_deref().unwrap().contains("reverted"));
        app.settings_path = settings_path;
        app.message = None;
        app.ui.hide().unwrap();
        drop(app);

        let mut restarted = unopened_fixture_app(&root, window.clone(), saved.clone());
        restarted.open_system_by_index(0);
        restarted.leave_splash();
        assert_eq!(restarted.settings, saved);
        assert_eq!(restarted.current_folder_place(), Some(root_key));
        assert_eq!(restarted.screen_rotation, ScreenRotation::CounterClockwise);
        assert_eq!(restarted.folder_preference_value(FOLDER_NAMES), "Full");
        enter(&mut restarted, "Child");
        assert!(restarted.folders_last);
        restarted.screen = Screen::Browse;
        restarted.handle(Action::Quit);
        // Unavailable storage is not a deletion and does not erase choices.
        std::fs::rename(games.join("Child"), games.join("Unavailable")).unwrap();
        restarted.relist_here();
        assert_eq!(
            restarted.settings.folder_preferences,
            saved.folder_preferences
        );
        restarted.ui.hide().unwrap();
    }
    run_cli_rotation(root, window.clone());
    run_favourites(root, window);
}

fn run_cli_rotation(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("folder-options-cli");
    let games = root.join("games/NES");
    std::fs::create_dir_all(games.join("Child")).unwrap();
    std::fs::write(games.join("Game.nes"), b"fixture").unwrap();
    let mut settings = Settings {
        show_empty: Some(true),
        ..Default::default()
    };
    set_folder_preferences(
        &mut settings,
        "NES",
        &Place::Dir(games).key(),
        FolderPreferences {
            screen_rotation: Some(ScreenRotation::Clockwise),
            ..Default::default()
        },
    );
    let mut app = unopened_fixture_app_with_rotation(
        &root,
        window,
        settings,
        &["NES"],
        "games/NES",
        Some(ScreenRotation::Off),
    );
    app.open_system_by_index(0);
    app.leave_splash();
    assert_eq!(
        app.screen_rotation,
        ScreenRotation::Off,
        "CLI rotation overrides automatic saved-folder rotation"
    );
    enter(&mut app, "Child");
    app.handle(Action::Quit);
    assert_eq!(app.screen_rotation, ScreenRotation::Off);
    choose(&mut app, FOLDER_ROTATION, 1);
    app.handle(Action::Quit);
    app.resolve_view();
    assert_eq!(
        app.screen_rotation,
        ScreenRotation::Off,
        "cancellation preserves the temporary CLI override"
    );
    choose(&mut app, FOLDER_ROTATION, 1);
    app.handle(Action::Accept);
    assert_eq!(
        app.screen_rotation,
        ScreenRotation::CounterClockwise,
        "an explicit kept UI choice can replace the CLI override"
    );
    assert_eq!(
        app.settings.screen_rotation, None,
        "the temporary flag is never saved as a global choice"
    );
    app.ui.hide().unwrap();
}

fn run_favourites(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("folder-options-favourites");
    let favorites = root.join("_@Favorites");
    std::fs::create_dir_all(favorites.join("Original/Child")).unwrap();
    std::fs::create_dir_all(favorites.join("Original Extra")).unwrap();
    let mut settings = Settings {
        show_empty: Some(true),
        ..Default::default()
    };
    for name in ["Original", "Original/Child", "Original Extra"] {
        set_folder_preferences(
            &mut settings,
            "Favorites",
            &Place::Dir(favorites.join(name)).key(),
            FolderPreferences {
                folders_last: Some(true),
                ..Default::default()
            },
        );
    }
    let mut app =
        unopened_fixture_app_with_systems(&root, window, settings, &["Favorites"], "_@Favorites");
    // MiSTer's menu root is also a game root, where _@Favorites lives.
    app.config
        .game_roots
        .push(root.to_string_lossy().into_owned());
    app.open_system_by_index(0);
    app.leave_splash();
    assert!(app.in_favorites());
    enter(&mut app, "Original");
    assert!(app.folders_last);
    choose(&mut app, FOLDER_NAMES, 1);
    assert_eq!(app.settings.game_name_display, None);
    app.screen = Screen::Browse;
    app.handle(Action::Quit);
    app.open_name_keyboard(
        NamePurpose::RenameFavoriteFolder(favorites.join("Original")),
        "Renamed".into(),
    );
    app.finish_name_keyboard();
    assert!(
        app.message.is_none(),
        "Favourites refresh failed: {:?}",
        app.message
    );
    assert!(favorites.join("Renamed/Child").is_dir());
    let places = &app.settings.folder_preferences["Favorites"];
    assert!(!places.contains_key(&Place::Dir(favorites.join("Original")).key()));
    assert!(places.contains_key(&Place::Dir(favorites.join("Renamed/Child")).key()));
    assert!(places.contains_key(&Place::Dir(favorites.join("Original Extra")).key()));
    assert_eq!(
        Settings::load(&app.settings_path)
            .unwrap()
            .folder_preferences,
        app.settings.folder_preferences
    );
    let before_failed = app.settings.folder_preferences.clone();
    app.open_name_keyboard(
        NamePurpose::RenameFavoriteFolder(favorites.join("Renamed")),
        "Original Extra".into(),
    );
    app.finish_name_keyboard();
    assert!(
        app.message.is_some(),
        "an occupied destination prevents renaming"
    );
    assert_eq!(app.settings.folder_preferences, before_failed);
    app.message = None;
    app.screen = Screen::Browse;
    app.delete_favorite_folder(&favorites.join("Renamed"));
    assert!(app.message.is_some(), "a nonempty folder is not deleted");
    assert_eq!(app.settings.folder_preferences, before_failed);
    app.message = None;
    enter(&mut app, "Renamed");
    assert!(app.folders_last);
    app.delete_favorite_folder(&favorites.join("Renamed/Child"));
    assert!(!favorites.join("Renamed/Child").exists());
    assert!(!app.settings.folder_preferences["Favorites"]
        .contains_key(&Place::Dir(favorites.join("Renamed/Child")).key()));
    app.screen = Screen::Browse;
    app.handle(Action::Quit);
    app.delete_favorite_folder(&favorites.join("Renamed"));
    assert_eq!(app.settings.folder_preferences["Favorites"].len(), 1);
    assert!(favorites.join("Original Extra").exists());
    let settings_path = app.settings_path.clone();
    let saved_bytes = std::fs::read(&settings_path).unwrap();
    app.settings_path = root.join("absent/settings.toml");
    app.open_name_keyboard(
        NamePurpose::RenameFavoriteFolder(favorites.join("Original Extra")),
        "Unsaved".into(),
    );
    app.finish_name_keyboard();
    assert!(favorites.join("Unsaved").is_dir());
    let message = app.message.as_deref().unwrap();
    assert!(
        message.contains("folder changed")
            && message.contains("not saved")
            && message.contains("creating temporary settings"),
        "{message}"
    );
    assert!(app.settings.folder_preferences["Favorites"]
        .contains_key(&Place::Dir(favorites.join("Unsaved")).key()));
    assert_eq!(std::fs::read(&settings_path).unwrap(), saved_bytes);
    app.settings_path = settings_path;
    app.message = None;
    assert!(app.save_settings());
    assert_eq!(
        Settings::load(&app.settings_path)
            .unwrap()
            .folder_preferences,
        app.settings.folder_preferences
    );
    app.ui.hide().unwrap();
}
