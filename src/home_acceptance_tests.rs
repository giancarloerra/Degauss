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
    let mut store = app.settings.home.clone();
    let folder = store
        .add(
            Entry {
                name: "Friday".into(),
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
    assert_eq!(app.settings.home.entries[&folder].name, "Friday");
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
    app.name_keyboard_list.select(42);
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
