use super::*;

pub(super) fn run(root: &Path, window: Rc<MinimalSoftwareWindow>) {
    use crate::home::{entry_key, Entry, Target};
    let root = root.join("personal-home");
    std::fs::create_dir_all(root.join("games/NES/subfolder")).unwrap();
    let path = root.join("games/NES/subfolder/Example.nes");
    std::fs::write(&path, b"fixture").unwrap();
    let mut app = unopened_fixture_app(&root, window.clone(), Settings::default());
    app.leave_splash();
    app.finish_background_work_for_headless();
    if app.index_terminal.is_some() {
        app.handle(Action::Quit);
    }
    if app.message.is_some() {
        app.handle(Action::Quit);
    }
    assert!(app.settings.home.is_empty());
    assert_eq!(
        app.home.rows.len(),
        app.categories.len(),
        "old settings preserve ordinary Home rows"
    );
    let initial = app.home.rows.clone();
    // Non-game rows cannot produce a preview. In particular, an unavailable
    // script/core must not be stat-ed again on every main-loop poll. Game
    // availability failures must remain retryable when storage/source recovers.
    let missing = root.join("games/NES/Missing.nes");
    let mut preview_keys = Vec::new();
    for (name, target) in [
        (
            "Missing script",
            Target::Script {
                path: root.join("absent.sh"),
            },
        ),
        (
            "Missing core",
            Target::Core {
                path: root.join("absent.rbf"),
            },
        ),
        (
            "Missing game",
            Target::Game {
                system: "NES".into(),
                launch: browse::Launch::File(missing.clone()),
            },
        ),
        (
            "Source recovery",
            Target::Game {
                system: "NES".into(),
                launch: browse::Launch::File(path.clone()),
            },
        ),
    ] {
        let id = app
            .settings
            .home
            .add(
                Entry {
                    name: name.into(),
                    image: None,
                    target,
                },
                None,
            )
            .unwrap();
        preview_keys.push(entry_key(&id));
    }
    app.artwork_source_errors
        .insert("NES".into(), "Temporary source error".into());
    app.home.rows = preview_keys.clone();
    app.home.rows.push("malformed".into());
    app.category_list = ListState::new(app.home.rows.len(), app.geometry.visible);
    app.resolve_home_preview();
    for key in [&preview_keys[0], &preview_keys[1], &"malformed".to_string()] {
        assert!(
            app.home.preview_done.contains(key),
            "non-preview rows are checked only once"
        );
    }
    assert!(!app.home.preview_done.contains(&preview_keys[2]));
    assert!(!app.home.preview_done.contains(&preview_keys[3]));
    assert!(app.home.preview_job.is_none());
    app.artwork_source_errors.remove("NES");
    std::fs::write(&missing, b"fixture").unwrap();
    app.resolve_home_preview();
    assert!(
        app.home.preview_job.is_some(),
        "restored games/sources must retry previews"
    );
    app.finish_background_work_for_headless();
    app.message = None;
    app.settings.home = Default::default();
    app.rebuild_system_list();
    std::fs::remove_file(&missing).unwrap();
    assert_eq!(app.home.rows, initial);
    let mut store = app.settings.home.clone();
    let folder = store
        .add(
            Entry {
                name: "Play Later".into(),
                image: None,
                target: Target::Folder {
                    children: Vec::new(),
                },
            },
            None,
        )
        .unwrap();
    let game = store
        .add(
            Entry {
                name: "Example".into(),
                image: None,
                target: Target::Game {
                    system: "NES".into(),
                    launch: browse::Launch::File(path.clone()),
                },
            },
            Some(&folder),
        )
        .unwrap();
    let system = store
        .add(
            Entry {
                name: "Nintendo".into(),
                image: None,
                target: Target::System {
                    system: "NES".into(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    let nested = store
        .add(
            Entry {
                name: "Nested".into(),
                image: None,
                target: Target::LibraryFolder {
                    system: "NES".into(),
                    trail: vec![
                        Place::Dir(root.join("games/NES")),
                        Place::Dir(root.join("games/NES/subfolder")),
                    ],
                },
            },
            Some(&folder),
        )
        .unwrap();
    app.settings.home = store;
    app.rebuild_system_list();
    assert_eq!(app.home.rows[0], entry_key(&folder));
    assert_eq!(&app.home.rows[1..], initial.as_slice());
    app.category_list.select(0);
    assert!(app.handle(Action::Accept).is_none());
    assert_eq!(app.home.folder.as_deref(), Some(folder.as_str()));
    assert_eq!(app.home.rows.len(), 3);
    let saved = app.position();
    assert_eq!(
        saved.home.as_ref().unwrap().folder.as_deref(),
        Some(folder.as_str())
    );
    app.handle(Action::Quit);
    assert!(app.home.folder.is_none());
    app.restore_position(&saved);
    assert_eq!(app.selected_home_key(), Some(entry_key(&game).as_str()));

    app.category_list.select(1);
    app.home_editor();
    app.home
        .edit
        .as_mut()
        .unwrap()
        .entries
        .get_mut(&folder)
        .unwrap()
        .name = "Cancelled".into();
    app.finish_home_edit(false);
    assert_eq!(app.settings.home.entries[&folder].name, "Play Later");
    assert_eq!(
        app.selected_home_key(),
        Some(entry_key(&system).as_str()),
        "cancelling Home editing restores the exact entry"
    );
    app.home_editor();
    app.finish_home_rename(&folder, "Renamed");
    app.finish_home_edit(true);
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().home.entries[&folder].name,
        "Renamed"
    );
    app.message = None;

    // The editor returns to the changed row, rather than silently staying
    // in its action submenu. Cancel still discards ordering/visibility.
    app.home_context();
    paint_index_frame(&mut app);
    assert_eq!(app.ui.get_heading(), "Home Folder / Actions");
    assert!(app.menu.iter().any(|row| row == EDIT_HOME_FOLDER));
    app.home_editor();
    let before = app.home.edit.as_ref().unwrap().clone();
    let key = entry_key(&system);
    app.home_entry_actions(key.clone());
    app.menu_list.select(0);
    app.handle(Action::Accept);
    assert!(matches!(app.home.menu, Some(HomeMenu::Editor)));
    assert_eq!(app.home.menu_keys[app.menu_list.selected()], key);
    assert_eq!(app.menu_list.selected(), 0);
    paint_index_frame(&mut app);
    assert_eq!(app.ui.get_heading(), "Edit Home Folder");
    app.handle(Action::Accept);
    assert!(app.menu.iter().any(|row| row == "Hide on Home Folder"));
    app.menu_list.select(2);
    app.handle(Action::Accept);
    assert!(matches!(app.home.menu, Some(HomeMenu::Editor)));
    assert_eq!(app.home.menu_keys[app.menu_list.selected()], key);
    assert!(app.menu[app.menu_list.selected()].ends_with("Hidden"));
    app.finish_home_edit(false);
    assert_eq!(app.settings.home, before);

    // Pinning from a normal game browser must save and populate Home
    // before any editor is opened, including a second repeated addition.
    let pinned = Entry {
        name: "Immediate game".into(),
        image: None,
        target: Target::Game {
            system: "NES".into(),
            launch: browse::Launch::File(path.clone()),
        },
    };
    app.browsing = Browsing::Games;
    let saved_path = app.settings_path.clone();
    let before_pin = app.settings.home.clone();
    app.home.pending_entry = Some(pinned.clone());
    app.home_destinations(None);
    app.settings_path = root.clone();
    app.menu_list.select(0);
    app.handle(Action::Accept);
    assert!(app.message.is_some());
    assert_eq!(app.settings.home, before_pin);
    assert_eq!(app.home.edit.as_ref().unwrap(), &before_pin);
    assert!(
        app.home.pending_entry.is_some(),
        "a failed immediate add can be retried"
    );
    app.settings_path = saved_path;
    app.message = None;
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(
        app.settings
            .home
            .entries
            .values()
            .filter(|entry| entry.name == "Immediate game")
            .count(),
        1
    );
    let id = app
        .settings
        .home
        .entries
        .iter()
        .find(|(_, entry)| entry.name == "Immediate game")
        .unwrap()
        .0
        .clone();
    app.settings.home.remove(&id);
    app.settings.save(&app.settings_path).unwrap();
    // Cancelling a new-folder name keeps the immediate pin workflow,
    // rather than entering an unrelated explicit Home edit session.
    for cancel in [Action::Quit, Action::Accept] {
        app.home.pending_entry = Some(pinned.clone());
        app.home_destinations(None);
        app.menu_list.select(
            app.menu
                .iter()
                .position(|row| row == NEW_HOME_FOLDER)
                .unwrap(),
        );
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::NameKeyboard);
        app.name_keyboard_list.select(
            name_keyboard::keys(app.name_keyboard_page, true)
                .iter()
                .position(|key| matches!(key, NameKey::Cancel))
                .unwrap(),
        );
        app.handle(cancel);
        assert!(matches!(app.home.menu, Some(HomeMenu::Destination(None))));
        assert!(!app.home.explicit_editor);
        assert!(app.home.pending_entry.is_some());
        app.menu_list.select(0);
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::Browse);
        assert!(app.home.edit.is_none());
        let id = app
            .settings
            .home
            .entries
            .iter()
            .find(|(_, entry)| entry.name == "Immediate game")
            .unwrap()
            .0
            .clone();
        assert!(Settings::load(&app.settings_path)
            .unwrap()
            .home
            .entries
            .contains_key(&id));
        app.settings.home.remove(&id);
        app.settings.save(&app.settings_path).unwrap();
    }
    app.begin_home_edit();
    app.home_entry_actions(entry_key(&folder));
    app.menu_list
        .select(app.menu.iter().position(|row| row == "Rename").unwrap());
    app.handle(Action::Accept);
    app.handle(Action::Quit);
    assert!(matches!(&app.home.menu, Some(HomeMenu::Entry(key)) if key == &entry_key(&folder)));
    assert!(!app.home.explicit_editor);
    assert_eq!(app.home_store().entries[&folder].name, "Renamed");
    app.finish_home_edit(false);
    app.home_destinations(Some(folder.clone()));
    app.menu_list.select(
        app.menu
            .iter()
            .position(|row| row == NEW_HOME_FOLDER)
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.handle(Action::Quit);
    assert!(matches!(&app.home.menu, Some(HomeMenu::Destination(Some(id))) if id == &folder));
    assert!(!app.home.explicit_editor);
    app.finish_home_edit(false);
    app.restore_home(crate::home::Resume {
        folder: None,
        key: String::new(),
        anchor: None,
    });
    app.home_context();
    app.menu_list.select(
        app.menu
            .iter()
            .position(|row| row == NEW_HOME_FOLDER)
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert!(app.home.edit.is_none());
    assert!(!app.home.explicit_editor);
    app.home_editor();
    app.home
        .edit
        .as_mut()
        .unwrap()
        .entries
        .get_mut(&folder)
        .unwrap()
        .name = "Retained draft".into();
    app.menu_list.select(
        app.menu
            .iter()
            .position(|row| row == NEW_HOME_FOLDER)
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.handle(Action::Quit);
    assert!(matches!(app.home.menu, Some(HomeMenu::Editor)));
    assert!(app.home.explicit_editor);
    assert_eq!(app.home_store().entries[&folder].name, "Retained draft");
    app.finish_home_edit(false);
    // Creating a destination must move the chosen entry, not leave behind
    // an empty folder. Explicit editing keeps Save/Cancel semantics.
    let before_move = app.settings.home.clone();
    for explicit in [false, true] {
        if explicit {
            app.home_editor();
        }
        app.home_destinations(Some(system.clone()));
        app.menu_list.select(
            app.menu
                .iter()
                .position(|row| row == NEW_HOME_FOLDER)
                .unwrap(),
        );
        app.handle(Action::Accept);
        app.name_keyboard_draft = "Moved items".into();
        app.name_keyboard_list.select(
            name_keyboard::keys(app.name_keyboard_page, true)
                .iter()
                .position(|key| matches!(key, NameKey::Save))
                .unwrap(),
        );
        app.handle(Action::Accept);
        let destination = app
            .home_store()
            .entries
            .iter()
            .find(|(_, entry)| entry.name == "Moved items")
            .unwrap()
            .0
            .clone();
        assert_eq!(
            app.home_store().parent(&system),
            Some(destination.clone()),
            "Move to Folder must place the selected entry in its new folder"
        );
        if explicit {
            assert_eq!(app.settings.home, before_move);
            assert_eq!(
                Settings::load(&app.settings_path).unwrap().home,
                before_move
            );
            assert!(matches!(app.home.menu, Some(HomeMenu::Editor)));
            app.finish_home_edit(false);
            assert_eq!(app.settings.home, before_move);
        } else {
            assert_eq!(app.screen, Screen::Browse);
            assert_eq!(
                Settings::load(&app.settings_path).unwrap().home,
                app.settings.home
            );
            app.settings.home = before_move.clone();
            app.settings.save(&app.settings_path).unwrap();
            app.rebuild_home_rows();
        }
    }
    // A subtree at the depth limit cannot be moved one level deeper.
    // That rejected placement must not save an empty destination either.
    let mut deepest = before_move.clone();
    let level_two = deepest
        .add(
            Entry {
                name: "Level two".into(),
                image: None,
                target: Target::Folder {
                    children: Vec::new(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    deepest
        .add(
            Entry {
                name: "Level three".into(),
                image: None,
                target: Target::Folder {
                    children: Vec::new(),
                },
            },
            Some(&level_two),
        )
        .unwrap();
    app.settings.home = deepest.clone();
    app.settings.save(&app.settings_path).unwrap();
    app.home_destinations(Some(folder.clone()));
    app.finish_home_folder("Too deep");
    assert!(app.message.as_deref().unwrap().contains("three levels"));
    assert_eq!(app.home_store(), &deepest);
    assert_eq!(app.settings.home, deepest);
    assert_eq!(Settings::load(&app.settings_path).unwrap().home, deepest);
    app.message = None;
    app.finish_home_edit(false);
    app.settings.home = before_move;
    app.settings.save(&app.settings_path).unwrap();
    app.rebuild_home_rows();
    app.browsing = Browsing::Games;
    for (name, parent) in [
        ("Immediate game", None),
        ("Second immediate game", Some(folder.as_str())),
    ] {
        let mut entry = pinned.clone();
        entry.name = name.into();
        app.home.pending_entry = Some(entry);
        app.home_destinations(None);
        let at = app
            .home
            .menu_keys
            .iter()
            .position(|key| key == parent.unwrap_or(""))
            .unwrap();
        app.menu_list.select(at);
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::Browse);
        assert!(app.home.edit.is_none());
        assert!(app.home.menu.is_none());
        assert!(Settings::load(&app.settings_path)
            .unwrap()
            .home
            .entries
            .values()
            .any(|entry| entry.name == name));
    }
    app.restore_home(crate::home::Resume {
        folder: None,
        key: String::new(),
        anchor: None,
    });
    assert!(app
        .home
        .rows
        .iter()
        .any(|key| app.home_label(key) == "Immediate game"));
    app.home_context();
    paint_index_frame(&mut app);
    assert_eq!(app.ui.get_heading(), "Home / Actions");
    assert!(app.menu.iter().any(|row| row == EDIT_HOME));
    let added = app
        .settings
        .home
        .entries
        .iter()
        .find(|(_, entry)| entry.name == "Immediate game")
        .unwrap()
        .0
        .clone();
    app.begin_home_edit();
    app.home_entry_actions(entry_key(&added));
    app.menu_list
        .select(app.menu.iter().position(|row| row == "Remove").unwrap());
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Browse);
    assert!(!app.settings.home.entries.contains_key(&added));
    assert!(!app.home.rows.contains(&entry_key(&added)));
    assert!(!Settings::load(&app.settings_path)
        .unwrap()
        .home
        .entries
        .contains_key(&added));
    // Removing from an explicit editor remains immediate, but must not
    // save unrelated draft edits or resurrect the entry on Cancel.
    app.home.folder = Some(folder.clone());
    app.rebuild_home_rows();
    let added = app
        .settings
        .home
        .entries
        .iter()
        .find(|(_, entry)| entry.name == "Second immediate game")
        .unwrap()
        .0
        .clone();
    app.home_editor();
    app.home
        .edit
        .as_mut()
        .unwrap()
        .entries
        .get_mut(&folder)
        .unwrap()
        .name = "Do not save".into();
    app.home_entry_actions(entry_key(&added));
    app.menu_list
        .select(app.menu.iter().position(|row| row == "Remove").unwrap());
    app.handle(Action::Accept);
    assert!(matches!(app.home.menu, Some(HomeMenu::Editor)));
    app.finish_home_edit(false);
    assert!(!app.settings.home.entries.contains_key(&added));
    assert_eq!(app.settings.home.entries[&folder].name, "Renamed");

    let settings_before = app.settings.clone();
    let settings_path = app.settings_path.clone();
    app.home_editor();
    app.finish_home_rename(&folder, "Unsaved");
    app.settings_path = root.clone();
    app.finish_home_edit(true);
    assert!(
        app.message.is_some(),
        "a failed settings write remains visible"
    );
    assert_eq!(app.settings.home, settings_before.home);
    assert!(
        app.home.edit.is_some(),
        "failed edits remain available to cancel"
    );
    app.settings_path = settings_path;
    app.message = None;
    app.finish_home_edit(false);

    app.category_list.select(
        app.home
            .rows
            .iter()
            .position(|key| key == &entry_key(&nested))
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.opening = None;
    app.open_system_now();
    app.finish_background_work_for_headless();
    app.message = None;
    assert!(app.poll_home_open().is_none());
    assert_eq!(
        app.trail.last().unwrap().place,
        Place::Dir(root.join("games/NES/subfolder"))
    );
    app.handle(Action::Quit);
    assert_eq!(app.home.folder.as_deref(), Some(folder.as_str()));
    assert_eq!(app.selected_home_key(), Some(entry_key(&nested).as_str()));

    app.category_list.select(
        app.home
            .rows
            .iter()
            .position(|key| key == &entry_key(&system))
            .unwrap(),
    );
    app.handle(Action::Accept);
    app.opening = None;
    app.open_system_now();
    app.finish_background_work_for_headless();
    app.message = None;
    app.poll_home_open();
    assert_eq!(app.open_system.as_deref(), Some("NES"));
    app.handle(Action::Quit);
    assert_eq!(app.selected_home_key(), Some(entry_key(&system).as_str()));

    app.category_list.select(
        app.home
            .rows
            .iter()
            .position(|key| key == &entry_key(&game))
            .unwrap(),
    );
    app.touch_selection();
    app.finish_background_work_for_headless();
    paint_index_frame(&mut app);
    assert!(
        app.ui.get_browse_playable(),
        "a pinned game must say Play, not Open"
    );
    app.category_list.select(1);
    paint_index_frame(&mut app);
    assert!(
        !app.ui.get_browse_playable(),
        "a pinned system must still say Open"
    );
    app.category_list.select(0);
    std::fs::remove_file(&path).unwrap();
    assert!(app
        .home_problem(&entry_key(&game))
        .unwrap()
        .contains("unavailable"));
    app.handle(Action::Accept);
    assert!(app.message.as_deref().unwrap().contains("unavailable"));
    assert!(app.settings.home.entries.contains_key(&game));
    app.message = None;
    std::fs::write(&path, b"fixture").unwrap();
    assert!(app.home_problem(&entry_key(&game)).is_none());
    app.settings.hidden.push("NES".into());
    assert!(app
        .home_problem(&entry_key(&game))
        .unwrap()
        .contains("hidden"));
    app.show_hidden = true;
    assert!(app.home_problem(&entry_key(&game)).is_none());
    app.show_hidden = false;
    app.settings.hidden.clear();
    for layout in Layout::ALL {
        app.layout = layout;
        app.apply_geometry();
        app.refresh();
        assert!(!app.ui.get_rows().iter().collect::<Vec<_>>().is_empty());
    }

    let scripts = root.join("Scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    let script_path = scripts.join("Example.sh");
    std::fs::write(&script_path, "#!/bin/bash\nexit 0\n").unwrap();
    app.settings.show_scripts = Some(true);
    let script = app
        .settings
        .home
        .add(
            Entry {
                name: "Example script".into(),
                image: None,
                target: Target::Script {
                    path: script_path.clone(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    let scripts_category = app
        .settings
        .home
        .add(
            Entry {
                name: "Tools".into(),
                image: None,
                target: Target::Category {
                    category: SCRIPTS_CATEGORY.into(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    app.rebuild_system_list();
    let select = |app: &mut App, id: &str| {
        app.category_list.select(
            app.home
                .rows
                .iter()
                .position(|key| key == &entry_key(id))
                .unwrap(),
        );
    };
    select(&mut app, &script);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scripts);
    assert!(matches!(app.pending, Some(Pending::RunScript(_))));
    let returned = app.position();
    app.handle(Action::Quit);
    assert!(app.pending.is_none());
    app.handle(Action::Quit);
    assert_eq!(app.selected_home_key(), Some(entry_key(&script).as_str()));
    app.resume_scripts(&script_path, returned.home.as_ref());
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(app.home.folder.as_deref(), Some(folder.as_str()));
    assert_eq!(app.selected_home_key(), Some(entry_key(&script).as_str()));
    for name in ["Not a script.txt", ".Hidden.sh"] {
        let unavailable = scripts.join(name);
        std::fs::write(&unavailable, "not a selectable script").unwrap();
        app.settings.home.entries.get_mut(&script).unwrap().target =
            Target::Script { path: unavailable };
        select(&mut app, &script);
        app.handle(Action::Accept);
        assert!(
            app.pending.is_none(),
            "an unselectable Home script must not run another directory entry"
        );
        assert!(app.message.as_deref().unwrap().contains("pinned script"));
        app.handle(Action::Quit);
        app.handle(Action::Quit);
        assert_eq!(app.selected_home_key(), Some(entry_key(&script).as_str()));
    }
    app.settings.home.entries.get_mut(&script).unwrap().target = Target::Script {
        path: script_path.clone(),
    };
    select(&mut app, &scripts_category);
    app.handle(Action::Accept);
    assert_eq!(app.screen, Screen::Scripts);
    app.handle(Action::Quit);
    assert_eq!(app.screen, Screen::Browse);
    assert_eq!(
        app.selected_home_key(),
        Some(entry_key(&scripts_category).as_str())
    );
    assert!(app.home.origin.is_none());
    assert!(app.home.pending_open.is_none());

    let core_path = root.join("_Console/NES_20261001.rbf");
    std::fs::create_dir_all(core_path.parent().unwrap()).unwrap();
    std::fs::write(&core_path, b"fixture").unwrap();
    let core = app
        .settings
        .home
        .add(
            Entry {
                name: "NES core".into(),
                image: None,
                target: Target::Core {
                    path: core_path.clone(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    app.rebuild_system_list();
    select(&mut app, &core);
    let outcome = app
        .handle(Action::Accept)
        .expect("installed core uses the common launcher");
    assert!(
        matches!(outcome, Outcome::Launch { plan, .. } if plan.command.contains(core_path.canonicalize().unwrap().to_str().unwrap()))
    );
    let returned = app.position();
    app.restore_position(&returned);
    assert_eq!(app.selected_home_key(), Some(entry_key(&core).as_str()));

    app.settings.collections.insert(
        "example".into(),
        crate::explore::Collection {
            name: "Example picks".into(),
            query: crate::explore::Query {
                title: "Example".into(),
                ..Default::default()
            },
        },
    );
    let collection = app
        .settings
        .home
        .add(
            Entry {
                name: "Example picks".into(),
                image: None,
                target: Target::Collection {
                    collection: "example".into(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    app.rebuild_system_list();
    select(&mut app, &collection);
    assert!(
        app.explore.catalogue.is_none(),
        "Home has not eagerly built Explore"
    );
    app.handle(Action::Accept);
    app.finish_background_work_for_headless();
    assert!(app.explore.active);
    assert_eq!(app.explore.query.title, "Example");
    assert_eq!(app.browse_count(), 1);
    app.handle(Action::Quit);
    assert_eq!(
        app.selected_home_key(),
        Some(entry_key(&collection).as_str())
    );
    app.settings.collections.remove("example");
    assert!(app
        .home_problem(&entry_key(&collection))
        .unwrap()
        .contains("unavailable"));

    let logos = root.join("logos");
    std::fs::create_dir_all(&logos).unwrap();
    let logo = logos.join("NES.png");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/logos/NES.png"),
        &logo,
    )
    .unwrap();
    app.logo_dir = Some(logos);
    app.home_editor();
    app.home.image = Some(folder.clone());
    app.open_home_image_picker();
    app.handle(Action::Accept);
    assert_eq!(
        app.home.edit.as_ref().unwrap().entries[&folder]
            .image
            .as_ref(),
        Some(&logo)
    );
    app.finish_home_edit(true);
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().home.entries[&folder]
            .image
            .as_ref(),
        Some(&logo)
    );
    app.restore_home(crate::home::Resume {
        folder: None,
        key: entry_key(&folder),
        anchor: None,
    });
    assert_eq!(app.home_art(&entry_key(&folder)).0.as_ref(), Some(&logo));
    std::fs::remove_file(&logo).unwrap();
    app.handle(Action::Accept);
    assert!(app
        .message
        .as_deref()
        .unwrap()
        .contains("image is unavailable"));
    assert_eq!(
        app.settings.home.entries[&folder].image.as_ref(),
        Some(&logo)
    );
    app.message = None;
    let second = app
        .settings
        .home
        .add(
            Entry {
                name: "Second".into(),
                image: None,
                target: Target::Folder {
                    children: Vec::new(),
                },
            },
            Some(&folder),
        )
        .unwrap();
    let third = app
        .settings
        .home
        .add(
            Entry {
                name: "Third".into(),
                image: None,
                target: Target::Folder {
                    children: Vec::new(),
                },
            },
            Some(&second),
        )
        .unwrap();
    app.home.folder = Some(third);
    app.home_editor();
    let unchanged = app.home.edit.clone().unwrap();
    app.open_name_keyboard(NamePurpose::HomeFolder, "Fourth".into());
    app.name_keyboard_list.select(
        name_keyboard::keys(app.name_keyboard_page, true)
            .iter()
            .position(|key| matches!(key, NameKey::Save))
            .unwrap(),
    );
    app.dirty = false;
    app.handle(Action::Accept);
    assert!(
        app.dirty,
        "a rejected Home edit must render its actual error"
    );
    assert_eq!(
        app.home.edit.as_ref(),
        Some(&unchanged),
        "depth rejection is atomic"
    );
    app.refresh();
    assert!(app.ui.get_overlay().contains("only three levels"));
    app.message = None;
    app.finish_home_edit(false);
    let mut resumed =
        unopened_fixture_app(&root, window, Settings::load(&app.settings_path).unwrap());
    resumed.leave_splash();
    resumed.restore_position(&saved);
    assert_eq!(resumed.home.folder.as_deref(), Some(folder.as_str()));
    assert!(
        path.is_file(),
        "Home organisation never removes the referenced game"
    );
}
