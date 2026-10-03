use super::*;

pub(super) fn run(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = root.join("explore-games");
    for system in ["NES", "SNES"] {
        std::fs::create_dir_all(root.join("games").join(system)).unwrap();
        for name in ["Alpha", "Beta"] {
            let extension = if system == "NES" { "nes" } else { "sfc" };
            std::fs::write(
                root.join(format!("games/{system}/{name}.{extension}")),
                b"fixture",
            )
            .unwrap();
        }
        let extension = if system == "NES" { "nes" } else { "sfc" };
        std::fs::write(root.join(format!("games/{system}/gamelist.xml")), format!("<gameList><game><path>Alpha.{extension}</path><name>Alpha</name><genre>Action</genre><developer>Studio</developer><releasedate>19930101</releasedate><desc>Complete description</desc></game><game><path>Beta.{extension}</path><name>Beta</name><genre>Puzzle</genre></game></gameList>")).unwrap();
    }
    let mut app = unopened_fixture_app_with_systems(
        &root,
        window.clone(),
        Settings::default(),
        &["NES", "SNES"],
        "games/NES",
    );
    app.all_systems[1].paths = vec![root.join("games/SNES")];
    app.rebuild_system_list();
    for system in &app.all_systems {
        let library = Library::open(&system.to_config()).unwrap();
        let cache = crate::cache::build_system(&library);
        crate::cache::save_system(&app.cache_dir, &system.def.id, &cache).unwrap();
    }
    app.leave_splash();
    app.finish_background_work_for_headless();
    assert!(
        app.build.is_none(),
        "fixture startup must finish before measuring Explore"
    );
    let source_snapshot = cache_snapshot(&app.cache_dir);
    let origin = app.position();
    app.open_explore(None);
    paint_index_frame(&mut app);
    assert!(
        !app.ui.get_overlay().contains("B Cancel"),
        "the body must not duplicate the control hint"
    );
    assert_eq!(app.ui.get_overlay_close_controls(), "B Cancel");
    assert!(
        app.ui.get_bottom_controls().is_empty(),
        "indexing has one Back hint, not another browser Back row"
    );
    app.finish_background_work_for_headless();
    assert!(app.explore.active);
    assert!(app.explore.job.is_none());
    assert_eq!(
        app.browse_count(),
        4,
        "identical titles keep their system owners"
    );
    assert!(app.explore.catalogue.as_ref().unwrap().omitted.is_empty());
    assert_eq!(
        cache_snapshot(&app.cache_dir),
        source_snapshot,
        "Explore must not scan or rewrite indexes"
    );
    assert!(
        app.explore
            .catalogue
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .all(|entry| entry.row.details.desc.is_empty()),
        "full descriptions are not duplicated in the projection"
    );
    for layout in Layout::ALL {
        app.layout = layout;
        app.apply_geometry();
        app.refresh();
        assert_eq!(app.browse_count(), 4);
        assert!(app.browse_row(0).is_some());
        let first = app.ui.get_rows().row_data(0).unwrap();
        if layout.rows_have_art() {
            assert!(
                first.title.contains('('),
                "image views show the owning system in each caption"
            );
        } else {
            assert!(
                !first.value.is_empty(),
                "list views show the owning system beside each title"
            );
        }
    }
    app.explore_pivot(MORE_DEVELOPER);
    app.finish_background_work_for_headless();
    assert_eq!(app.browse_count(), 2);
    assert_eq!(app.explore.pivots.len(), 1);
    app.explore_back();
    assert_eq!(app.browse_count(), 4);
    app.explore.query.title = "beta".into();
    app.filter = "beta".into();
    app.filter_explore(false);
    assert_eq!(app.browse_count(), 2);
    app.game_list.select(1);
    let selected = app.explore_selected().unwrap().key();
    assert_eq!(app.context_system_id(), Some("SNES"));
    app.settings.collections.insert(
        "back-check".into(),
        crate::explore::Collection {
            name: "Earlier query".into(),
            query: crate::explore::Query {
                title: "alpha".into(),
                ..Default::default()
            },
        },
    );
    app.explore_menu(
        ExploreMenu::Collection("back-check".into()),
        vec!["Open".into(), "Rename".into(), "Delete".into()],
    );
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert_eq!(app.explore.query.title, "alpha");
    app.explore_back();
    app.finish_background_work_for_headless();
    assert!(
        app.explore.active,
        "Back returns to the query before opening a collection"
    );
    assert_eq!(app.explore.query.title, "beta");
    assert_eq!(app.explore_selected().unwrap().key(), selected);
    app.settings.collections.remove("back-check");
    app.explore_fields();
    assert_eq!(app.explore_menu_heading(), "Explore / Filters");
    app.menu_list.select(1);
    app.accept_explore_menu();
    assert_eq!(app.explore_menu_heading(), "Explore / System");
    app.explore_actions();
    assert!(app.menu.iter().any(|entry| entry == CLEAR_SEARCH));
    app.filter = "no such title".into();
    app.apply_filter();
    app.explore_actions();
    assert!(!app.menu.iter().any(|entry| entry == GAME_INFORMATION));
    assert!(!app.menu.iter().any(|entry| entry == GAME_LAUNCH_CORE));
    app.filter = "beta".into();
    app.apply_filter();
    app.game_list.select(1);
    app.open_name_keyboard(NamePurpose::SaveCollection, "Do not save".into());
    app.handle(Action::Quit);
    assert!(
        app.settings.collections.is_empty(),
        "Back cancels a new collection without persisting it"
    );
    app.open_name_keyboard(NamePurpose::SaveCollection, "Puzzle picks".into());
    let save = name_keyboard::keys(app.name_keyboard_page, true)
        .iter()
        .position(|key| matches!(key, NameKey::Save))
        .unwrap();
    app.name_keyboard_list.select(save);
    app.handle(Action::Accept);
    assert_eq!(app.explore_menu_heading(), "Saved Collections");
    let (id, collection) = app.settings.collections.iter().next().unwrap();
    let id = id.clone();
    assert_eq!(collection.query.title, "beta");
    app.save_explore_collection(Some(&id), "Renamed picks");
    assert_eq!(app.settings.collections[&id].name, "Renamed picks");
    assert_eq!(app.settings.collections[&id].query.title, "beta");
    app.screen = Screen::Browse;
    let saved = app.position();
    assert_eq!(saved.selected_row.as_deref(), Some(selected.as_str()));
    let serialized = toml::to_string(&saved).unwrap();
    let saved: crate::state::State = toml::from_str(&serialized).unwrap();
    app.leave_explore();
    assert!(!app.explore.active);
    assert_eq!(app.browsing, Browsing::Categories);
    assert_eq!(app.position().system, origin.system);
    app.restore_position(&saved);
    app.finish_background_work_for_headless();
    assert!(app.explore.active);
    assert_eq!(app.browse_count(), 2);
    assert_eq!(app.explore_selected().unwrap().key(), selected);
    app.leave_explore();
    app.open_explore(None);
    assert!(
        app.explore.job.is_some(),
        "source probes run on the worker, not before its loading screen"
    );
    app.explore.match_warning = Some("Saved match warning".into());
    assert!(app.handle_explore_input(Action::Quit));
    assert!(
        app.explore.job.is_none(),
        "Back must not wait for a blocked filesystem probe"
    );
    assert!(!app.explore.active);
    assert_eq!(app.browsing, Browsing::Categories);
    assert!(app.message.as_ref().unwrap().contains("cancelled"));
    assert!(app
        .message
        .as_ref()
        .unwrap()
        .contains("Saved match warning"));
    assert!(app.explore.match_warning.is_none());
    app.open_explore(Some(saved.explore.as_ref().unwrap().query.clone()));
    app.finish_background_work_for_headless();
    app.explore.query = Default::default();
    app.filter_explore(false);
    app.leave_explore();
    app.settings.hidden.push("SNES".into());
    app.rebuild_system_list();
    app.open_explore(None);
    app.finish_background_work_for_headless();
    assert_eq!(
        app.browse_count(),
        2,
        "hidden systems cannot leak into Explore"
    );
    app.leave_explore();
    app.settings.hidden.clear();
    app.rebuild_system_list();
    std::fs::remove_file(crate::cache::system_path(&app.cache_dir, "SNES")).unwrap();
    app.open_explore(None);
    app.finish_background_work_for_headless();
    assert_eq!(app.browse_count(), 2);
    assert_eq!(app.explore.catalogue.as_ref().unwrap().omitted.len(), 1);
    assert!(app.message.as_ref().unwrap().contains("omitted"));
    assert!(!crate::cache::system_path(&app.cache_dir, "SNES").exists());
    app.message = None;
    app.leave_explore();
    app.settings.show_explore = Some(false);
    app.rebuild_system_list();
    assert!(!app
        .categories
        .iter()
        .any(|(name, _)| name == crate::explore::NAME));
    app.ui.hide().unwrap();
}
