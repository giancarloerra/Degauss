use super::*;

pub(super) fn run(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    use crate::custom_view::{Content, Definition, Template};
    let root = root.join("custom-views");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    for name in ["First Game.nes", "Second Game.nes"] {
        std::fs::write(games.join(name), b"fixture").unwrap();
    }
    std::fs::write(
        games.join("art.png"),
        include_bytes!("../assets/logos/NES.png"),
    )
    .unwrap();
    let description = "Complete selected-game description. ".repeat(120);
    let gamelist = games.join("gamelist.xml");
    std::fs::write(&gamelist, format!("<gameList><game><path>First Game.nes</path><name>First Game</name><image>art.png</image><publisher>First Publisher</publisher><desc>{description}</desc></game><game><path>Second Game.nes</path><name>Second Game</name><publisher>Second Publisher</publisher><desc>Second description.</desc></game></gameList>")).unwrap();
    let mut app = fixture_app(&root, window.clone(), Settings::default());
    app.leave_splash();
    app.finish_background_work_for_headless();
    app.open_system_by_index(0);
    app.finish_background_work_for_headless();
    if app.index_terminal.is_some() {
        app.handle(Action::Quit);
    }
    if app.message.is_some() {
        app.handle(Action::Quit);
    }
    app.screen = Screen::Browse;
    app.resolve_view();
    app.apply_geometry();
    assert!(
        !app.ui.get_custom_view(),
        "old settings must not enable a new layout"
    );
    let initial_assignment = app.settings.custom_views.clone();
    for template in Template::ALL {
        app.open_view_editor(None, Definition::new(template));
        paint_index_frame(&mut app);
        assert!(app.ui.get_custom_view());
        assert_eq!(app.ui.get_custom_panels().row_count(), template.count());
        let selected = app.game_list.selected();
        app.handle(Action::Down);
        assert_eq!(
            app.game_list.selected(),
            selected,
            "editor controls are not game navigation"
        );
        app.handle(Action::Faster);
        assert_eq!(
            app.custom_view
                .editor
                .as_ref()
                .unwrap()
                .draft
                .panels
                .iter()
                .filter(|content| **content == Content::List)
                .count(),
            1
        );
        app.handle(Action::Quit);
        assert!(app.custom_view.editor.is_none());
        assert_eq!(
            app.settings.custom_views, initial_assignment,
            "Cancel must not save assignments"
        );
    }
    let mut definition = Definition::new(Template::SplitRight);
    definition.panels[2] = Content::FullInformation;
    app.open_view_editor(None, definition);
    app.finish_custom_view("Readable games");
    assert!(app.message.is_none());
    let id = app.settings.view_definitions.keys().next().unwrap().clone();
    let key = crate::custom_view::key(&id);
    let persisted = Settings::load(&app.settings_path).unwrap();
    assert_eq!(persisted.view_definitions[&id].name, "Readable games");
    assert_eq!(
        app.current_view_place()
            .unwrap()
            .get(&persisted.custom_views),
        Some(key.as_str())
    );
    assert_eq!(app.view_label(), "Readable games");
    assert!(app.view_choices().contains(&key));
    app.game_list.select(0);
    app.touch_selection();
    app.settled_since = Some(Instant::now() - Duration::from_millis(300));
    app.finish_background_work_for_headless();
    paint_index_frame(&mut app);
    assert!(
        app.ui.get_custom_full_text().contains(description.trim()),
        "the embedded panel must contain the complete, source-trimmed description: {}",
        app.ui.get_custom_full_text()
    );
    assert!(app.ui.get_custom_short_text().contains("First Publisher"));
    assert!(app.ui.get_custom_information_max_scroll() > 0.0);
    let now = Instant::now();
    app.information_scroll_next_at
        .set(now + INFORMATION_SCROLL_WAIT);
    app.maintain_information_scroll(now);
    assert_eq!(
        app.ui.get_custom_information_offset(),
        0.0,
        "reading starts with a pause"
    );
    app.maintain_information_scroll(now + INFORMATION_SCROLL_WAIT);
    assert_eq!(
        app.ui.get_custom_information_offset(),
        1.0,
        "full information scrolls without requiring focus"
    );
    app.maintain_information_scroll(now + INFORMATION_SCROLL_WAIT + INFORMATION_SCROLL_STEP);
    assert_eq!(
        app.ui.get_custom_information_offset(),
        2.0,
        "long descriptions advance slowly at the same speed"
    );
    app.load_art();
    assert!(app.ui.get_has_art());
    app.open_context();
    app.show_context_page(Some(ContextPage::Game));
    app.load_art();
    assert!(!app.ui.get_has_art(), "Actions does not draw artwork");
    let focus = app
        .menu
        .iter()
        .position(|row| row == FOCUS_INFORMATION)
        .unwrap();
    app.menu_list.select(focus);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.custom_view.information_focus);
    assert!(
        app.art_pending,
        "returning to the browser must reload its artwork"
    );
    app.load_art();
    assert!(
        app.ui.get_has_art(),
        "information focus must retain the selected game's picture"
    );
    let selected = app.game_list.selected();
    app.information_scroll_next_at.set(now);
    let offset = app.ui.get_custom_information_offset();
    app.maintain_information_scroll(now + INFORMATION_SCROLL_WAIT * 2);
    assert_eq!(
        app.ui.get_custom_information_offset(),
        offset,
        "manual information focus owns scrolling"
    );
    app.handle(Action::Down);
    assert_eq!(app.game_list.selected(), selected);
    assert!(app.ui.get_custom_information_offset() > 0.0);
    app.handle(Action::Quit);
    assert!(!app.custom_view.information_focus);
    app.handle(Action::Down);
    assert_ne!(app.game_list.selected(), selected);
    app.settled_since = Some(Instant::now() - Duration::from_millis(300));
    app.finish_background_work_for_headless();
    assert!(app
        .ui
        .get_custom_full_text()
        .contains("Second description."));
    assert!(
        !app.ui.get_custom_full_text().contains("First Publisher"),
        "late data cannot attach to a new selection"
    );

    let original_bytes = std::fs::read(&app.settings_path).unwrap();
    let old = app.settings.view_definitions[&id].clone();
    app.open_view_editor(Some(id.clone()), old.clone());
    app.custom_view.editor.as_mut().unwrap().control = 4;
    app.handle(Action::Faster);
    assert_eq!(app.custom_view.editor.as_ref().unwrap().draft.columns, 55);
    app.handle(Action::Quit);
    assert_eq!(std::fs::read(&app.settings_path).unwrap(), original_bytes);
    assert_eq!(app.settings.view_definitions[&id], old);

    let settings_path = app.settings_path.clone();
    let blocked = root.join("blocked-parent");
    std::fs::write(&blocked, b"not a directory").unwrap();
    app.settings_path = blocked.join("settings.toml");
    app.open_view_editor(None, old.clone());
    app.finish_custom_view("Unsaved copy");
    assert!(app
        .message
        .as_deref()
        .unwrap()
        .contains("Custom view not saved:"));
    assert!(
        app.custom_view.editor.is_some(),
        "failed Save retains the draft"
    );
    assert_eq!(app.settings.view_definitions.len(), 1);
    assert_eq!(std::fs::read(&settings_path).unwrap(), original_bytes);
    app.handle(Action::Quit);
    app.handle(Action::Quit);
    app.settings_path = settings_path;

    app.open_view_editor(None, old.clone());
    app.finish_custom_view("Copy");
    let copy = app
        .settings
        .view_definitions
        .keys()
        .find(|other| **other != id)
        .unwrap()
        .clone();
    app.rename_custom_view(&copy, "Renamed copy");
    assert_eq!(app.settings.view_definitions[&copy].name, "Renamed copy");
    assert_eq!(app.settings.view_definitions[&id].name, old.name);
    app.settings
        .custom_views
        .systems
        .insert("Old category".into(), "tiled".into());
    app.settings.layout = Some(crate::custom_view::key(&copy));
    app.delete_custom_view(&copy);
    assert!(!app.settings.view_definitions.contains_key(&copy));
    assert_eq!(app.settings.layout.as_deref(), Some("details"));
    assert_eq!(app.settings.custom_views.systems["Old category"], "tiled");
    assert!(Settings::load(&app.settings_path)
        .unwrap()
        .view_definitions
        .contains_key(&id));
    app.leave_view_menu();
    select_option(&mut app, OptionsPage::Appearance, OptionId::CustomViews);
    let selected = app.menu_list.selected();
    app.handle(Action::Accept);
    assert!(matches!(app.custom_view.menu, Some(ViewMenu::List)));
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Options);
    assert_eq!(app.menu_list.selected(), selected);
    let appearance = OptionsPage::Appearance.ids();
    let view = appearance
        .iter()
        .position(|id| *id == OptionId::Layout)
        .unwrap();
    assert_eq!(
        &appearance[view..view + 4],
        &[
            OptionId::Layout,
            OptionId::CustomViews,
            OptionId::ResetCustomViews,
            OptionId::StartFolder
        ]
    );
    app.screen = Screen::Browse;
    app.show_view_menu(ViewMenu::Definition(id.clone()));
    app.menu_list.select(1);
    app.handle(Action::Accept);
    assert_eq!(
        Settings::load(&app.settings_path)
            .unwrap()
            .layout
            .as_deref(),
        Some(key.as_str())
    );
    let place = app.current_view_place().unwrap();
    place.set(&mut app.settings.custom_views, key.clone());
    app.resolve_view();
    app.apply_geometry();

    // Repeated settling, source failure and recovery use the real information worker.
    std::fs::rename(&gamelist, games.join("gamelist-good.xml")).unwrap();
    std::fs::create_dir(&gamelist).unwrap();
    app.custom_view.description_key = None;
    app.touch_selection();
    app.settled_since = Some(Instant::now() - Duration::from_millis(300));
    app.finish_background_work_for_headless();
    assert!(app
        .ui
        .get_custom_full_text()
        .contains("Could not read description:"));
    std::fs::remove_dir(&gamelist).unwrap();
    std::fs::rename(games.join("gamelist-good.xml"), &gamelist).unwrap();
    app.custom_view.description_key = None;
    app.finish_background_work_for_headless();
    assert!(!app
        .ui
        .get_custom_full_text()
        .contains("Could not read description:"));

    // Preview limits and navigation share the actual renderer geometry.
    for template in Template::ALL {
        for (width, height) in [(352, 240), (240, 352), (640, 480)] {
            app.width = width;
            app.height = height;
            app.window.set_size(slint::PhysicalSize::new(width, height));
            for fraction in [20, 80] {
                let mut view = Definition::new(template);
                view.name = "Limits".into();
                view.columns = fraction;
                view.rows = fraction;
                view.choose(template.count() - 1, Content::List);
                app.custom_view.active = Some(view.clone());
                app.apply_geometry();
                app.refresh();
                app.ui.show().unwrap();
                let mut surface = crate::surface::MemorySurface::new(
                    width,
                    height,
                    crate::surface::PixelFormat::Rgb565,
                );
                let mut presenter = Presenter::new(surface.geometry(), PresentMode::Direct);
                presenter.force_repaint(&app.window);
                assert!(presenter.draw(&app.window, &mut surface).unwrap().is_some());
                let list = view
                    .panels
                    .iter()
                    .position(|content| *content == Content::List)
                    .unwrap();
                let rect = app.custom_rectangles(app.geometry)[list];
                assert_eq!(
                    app.geometry.visible,
                    (rect.height / app.geometry.row_height).floor().max(1.0) as usize
                );
                app.game_list
                    .select(app.game_list.count().saturating_sub(1));
                app.refresh();
                assert!(
                    app.ui.get_selected() >= 0
                        && (app.ui.get_selected() as usize) < app.geometry.visible
                );
            }
        }
    }
    app.width = 352;
    app.height = 240;
    app.window.set_size(slint::PhysicalSize::new(352, 240));
    for font in Font::ALL {
        for size in crate::font::TextSize::ALL {
            app.font = font;
            app.text_size = size;
            app.apply_geometry();
            paint_index_frame(&mut app);
            assert!(app.geometry.visible > 0);
            assert!(app.ui.get_custom_view());
        }
    }
    app.settings.layout = Some(key.clone());
    app.open_system = Some(MISTERZINE_SYSTEM_ID.into());
    app.open_category = Some(MISTERZINE_CATEGORY.into());
    app.resolve_view();
    app.apply_geometry();
    assert_eq!(app.layout, Layout::Details);
    assert!(
        !app.ui.get_custom_view(),
        "Core Updates keeps its dedicated layout"
    );
    app.open_system = Some("NES".into());
    app.open_category = None;
    app.resolve_view();
    app.apply_geometry();
    assert!(app.ui.get_custom_view());

    app.browsing = Browsing::Categories;
    app.open_system = None;
    app.resolve_view();
    app.apply_geometry();
    app.update_chrome();
    app.maintain_custom_information();
    assert!(
        app.ui.get_custom_short_text().is_empty(),
        "non-game Home rows have no invented metadata"
    );
    assert!(app.ui.get_custom_full_text().is_empty());
    app.ui.hide().unwrap();
    drop(app);
    let mut restarted = unopened_fixture_app(
        &root,
        window,
        Settings::load(&root.join("settings.toml")).unwrap(),
    );
    restarted.leave_splash();
    restarted.finish_background_work_for_headless();
    assert!(restarted.settings.view_definitions.contains_key(&id));
    assert_eq!(restarted.settings.layout.as_deref(), Some(key.as_str()));
    assert!(
        restarted.ui.get_custom_view(),
        "the global named view survives restart"
    );
    // Only the explicit global choice survives. The old built-in map is untouched.
    assert_eq!(
        restarted.settings.custom_views.systems["Old category"],
        "tiled"
    );
    restarted.ui.hide().unwrap();
}
