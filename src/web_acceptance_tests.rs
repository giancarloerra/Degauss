//! App integration tests on the renderer's one Slint platform. Network and real
//! MiSTer acceptance are separate: these exercise shared commands and files.
use super::*;
use crate::cache;

fn command(app: &mut App, value: JsonValue) -> JsonValue {
    let command = serde_json::from_value(value).unwrap();
    match app.web_command(command) {
        Ok(WebReply::Json(value)) => value,
        Ok(_) => panic!("Expected JSON"),
        Err(error) => panic!("{}: {}", error.code, error.message),
    }
}
fn edit(app: &mut App, mut value: JsonValue) -> JsonValue {
    value["revision"] = json!(app.web_revision());
    command(app, value)
}
fn completed(app: &mut App, value: JsonValue) -> JsonValue {
    let Some(id) = value["job"].as_u64() else {
        return value;
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.poll_web_jobs();
        let job = app.web.jobs.get(&id).unwrap();
        match job["state"].as_str().unwrap() {
            "complete" => return job["result"].clone(),
            "failed" | "cancelled" => panic!("{job}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "Web job did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn run(parent: &Path, window: Rc<MinimalSoftwareWindow>) {
    let root = parent.join("web-access");
    let games = root.join("games/NES");
    std::fs::create_dir_all(&games).unwrap();
    std::fs::write(games.join("Example.nes"), b"fixture").unwrap();
    let description = "Complete game description. ".repeat(300);
    let xml=format!("<gameList><game><path>Example.nes</path><name>Example Game</name><desc>{description}</desc><image>old.png</image></game></gameList>");
    std::fs::write(games.join("gamelist.xml"), xml).unwrap();
    std::fs::write(games.join("old.png"), b"artwork-path-fixture").unwrap();
    std::fs::write(games.join("new.png"), b"artwork-path-fixture").unwrap();
    let mut app = unopened_fixture_app(&root, window, Settings::default());
    app.leave_splash();
    app.finish_background_work_for_headless();
    let config = app.all_systems[0].to_config();
    let library = browse::Library::open(&config).unwrap();
    let cache = cache::build_system_checked(&library, &mut vec![]).unwrap();
    cache::save_system(&app.cache_dir, "NES", &cache).unwrap();
    assert!(app.web.server.is_none());
    assert!(app.web.library.is_none());
    assert_eq!(
        command(&mut app, json!({"action":"state"}))["crt_images"],
        false
    );
    let listener_config = app.settings.web_access.clone();
    edit(
        &mut app,
        json!({"action":"option-adjust", "option":"WebCrtImages", "delta":1}),
    );
    assert_eq!(
        command(&mut app, json!({"action":"state"}))["crt_images"],
        true
    );
    assert_eq!(
        app.settings.web_access, listener_config,
        "Image treatment must not change or restart Web Access"
    );
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().web_crt_images,
        Some(true)
    );
    edit(
        &mut app,
        json!({"action":"option-adjust", "option":"WebCrtImages", "delta":-1}),
    );
    assert_eq!(
        command(&mut app, json!({"action":"state"}))["crt_images"],
        false
    );
    assert_eq!(
        command(&mut app, json!({"action":"web-settings"}))["config"]["mode"],
        "off"
    );
    let options = command(&mut app, json!({"action":"options"}));
    let mut support_core = crate::misterzine::Item::fixture(
        "Installed support core",
        crate::misterzine::LocalState::Current,
        Some(root.join("_Arcade/cores/Support.rbf")),
    );
    support_core.launch_path = None;
    let detail = app.web_core_detail(&support_core, None);
    assert_eq!(detail["installed"], true);
    assert!(
        detail["target"].is_null(),
        "Installed support cores must not be offered as standalone launches"
    );
    for page in options["pages"].as_array().unwrap() {
        for option in page["items"].as_array().unwrap() {
            if option["adjustable"] == false {
                let id = option["id"].as_str().unwrap();
                assert!(
                    include_str!("../web/app.js").contains(&format!("{id}:")),
                    "Every native software setting must have a web editor: {id}"
                );
            }
        }
    }
    let native = (
        app.screen,
        app.open_system.clone(),
        app.game_list.selected(),
    );
    let initial = command(&mut app, json!({"action":"browse","system":"NES"}));
    let page = completed(&mut app, initial);
    assert_eq!(page["breadcrumbs"][0]["name"], "NES");
    assert_eq!(page["breadcrumbs"][0]["folder"], page["folder"]);
    assert!(page["parent"].is_null());
    assert_eq!(page["letters"], json!([{"letter":"E","offset":0}]));
    let game = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "Example Game")
        .unwrap()
        .clone();
    let id = game["id"].as_str().unwrap().to_string();
    assert_eq!(
        (
            app.screen,
            app.open_system.clone(),
            app.game_list.selected()
        ),
        native,
        "Browser reads never move native navigation"
    );
    let initial = command(&mut app, json!({"action":"game","id":id}));
    let game = completed(&mut app, initial);
    assert_eq!(
        game["details"]["desc"],
        description.trim(),
        "The web game page reads the full description, not the cached excerpt"
    );
    let initial = command(&mut app, json!({"action":"game","id":id}));
    assert_eq!(
        completed(&mut app, initial),
        game,
        "Repeated information keeps the complete description and artwork"
    );
    edit(
        &mut app,
        json!({"action":"home-edit","edit":{"operation":"folder","name":"Personal"}}),
    );
    let home = command(&mut app, json!({"action":"home"}));
    let folder = home["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "Personal")
        .unwrap();
    let folder_id = folder["folder"].as_str().unwrap().to_string();
    let target = folder["target"].as_str().unwrap().to_string();
    edit(
        &mut app,
        json!({"action":"view-assign","target":target,"view":"list"}),
    );
    assert_eq!(
        command(&mut app, json!({"action":"views","target":target}))["current"],
        "list"
    );
    edit(
        &mut app,
        json!({"action":"view-assign","target":target,"view":""}),
    );
    assert!(command(&mut app, json!({"action":"views","target":target}))["current"].is_null());
    for (category, system, place) in [
        (crate::explore::NAME, crate::explore::STATE_ID, "explore"),
        (LAST_PLAYED_CATEGORY, LAST_PLAYED_SYSTEM, LAST_PLAYED_PLACE),
    ] {
        let target = app.web_register(
            crate::home::Target::Category {
                category: category.into(),
            },
            None,
        );
        edit(
            &mut app,
            json!({"action":"view-assign","target":target,"view":"list"}),
        );
        assert_eq!(
            app.settings.custom_views.games[system][place], "list",
            "Special Home entries must write the exact key read by their native screen"
        );
    }
    for category in [MISTERZINE_CATEGORY, SCRIPTS_CATEGORY, ATTRACT_MODE_CATEGORY] {
        let target = app.web_register(
            crate::home::Target::Category {
                category: category.into(),
            },
            None,
        );
        assert!(app.web_view_place(Some(&target)).is_err());
    }
    edit(
        &mut app,
        json!({"action":"home-edit","edit":{"operation":"add","name":"Pinned game","target":id,"parent":folder_id}}),
    );
    let folder = command(&mut app, json!({"action":"home","folder":folder_id}));
    assert_eq!(
        folder["breadcrumbs"],
        json!([{"name":"Personal","folder":folder_id}])
    );
    assert_eq!(
        folder["items"][0]["view_editable"], false,
        "A pinned game has no layout; the browser must not offer a non-working editor"
    );
    assert_eq!(
        folder["items"][0]["name"], "Pinned game",
        "Adding is immediate and needs no Edit Home confirmation"
    );
    let initial = command(
        &mut app,
        json!({"action":"home-artwork","folder":folder_id}),
    );
    let images = completed(&mut app, initial);
    assert!(images["images"]
        .as_object()
        .unwrap()
        .values()
        .any(|url| url.as_str() == game["image"].as_str()));
    let stale = app.web_revision();
    edit(
        &mut app,
        json!({"action":"home-edit","edit":{"operation":"rename","id":folder_id,"name":"Renamed"}}),
    );
    let conflict=app.web_command(serde_json::from_value(json!({"action":"home-edit","revision":stale,"edit":{"operation":"remove","id":folder_id}})).unwrap());
    assert!(matches!(
        conflict,
        Err(WebError {
            code: "conflict",
            ..
        })
    ));
    assert!(app.settings.home.entries.contains_key(&folder_id));
    let snapshot = Settings::load(&app.settings_path).unwrap();
    assert_eq!(snapshot.home.entries[&folder_id].name, "Renamed");
    let renamed = command(&mut app, json!({"action":"home","folder":folder_id}));
    assert_eq!(renamed["breadcrumbs"][0]["name"], "Renamed");
    // Breadcrumbs are generated from the real persisted hierarchy, not browser
    // history, so opening a nested Home link directly still has a parent.
    edit(
        &mut app,
        json!({"action":"home-edit","edit":{"operation":"folder","name":"Nested","parent":folder_id}}),
    );
    let parent_folder = command(&mut app, json!({"action":"home","folder":folder_id}));
    let nested = parent_folder["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "Nested")
        .unwrap()["folder"]
        .clone();
    let nested_page = command(&mut app, json!({"action":"home","folder":nested}));
    assert_eq!(nested_page["parent"], folder_id);
    assert_eq!(
        nested_page["breadcrumbs"],
        json!([{"name":"Renamed","folder":folder_id},{"name":"Nested","folder":nested}])
    );

    let trail = vec![
        browse::Place::Dir(games.clone()),
        browse::Place::Archive(games.join("Collection.zip")),
        browse::Place::ArchiveDirectory {
            archive: games.join("Collection.zip"),
            prefix: "Action/".into(),
        },
    ];
    let nested_library = app
        .web_page(crate::web::library::Page {
            entries: vec![],
            data: json!({"name":"NES","system":"NES","trail":trail}),
        })
        .unwrap();
    assert_eq!(
        nested_library["breadcrumbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["NES", "Collection.zip", "Action"]
    );
    assert_eq!(
        nested_library["parent"],
        nested_library["breadcrumbs"][1]["folder"]
    );
    assert_eq!(
        nested_library["folder"],
        nested_library["breadcrumbs"][2]["folder"]
    );
    assert!(
        nested_library.get("trail").is_none(),
        "Raw paths stay inside the application"
    );
    let scripts = root.join("Scripts");
    std::fs::create_dir_all(scripts.join("Tools/Network")).unwrap();
    std::fs::write(scripts.join("Tools/Network/Check.sh"), b"#!/bin/sh\n").unwrap();
    let pending = command(&mut app, json!({"action":"scripts"}));
    let script_root = completed(&mut app, pending);
    let pending = command(
        &mut app,
        json!({"action":"scripts","folder":script_root["items"][0]["script_folder"]}),
    );
    let script_tools = completed(&mut app, pending);
    let pending = command(
        &mut app,
        json!({"action":"scripts","folder":script_tools["items"][0]["script_folder"]}),
    );
    let script_network = completed(&mut app, pending);
    assert_eq!(
        script_network["breadcrumbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Scripts", "Tools", "Network"]
    );
    assert!(script_network["breadcrumbs"][0]["folder"].is_null());
    let pending = command(
        &mut app,
        json!({"action":"scripts","folder":script_network["breadcrumbs"][1]["folder"]}),
    );
    assert_eq!(completed(&mut app, pending)["items"][0]["name"], "Network");
    let initial = command(
        &mut app,
        json!({"action":"explore","query":{"system":"NES"},"limit":1}),
    );
    let before_update = completed(&mut app, initial);
    assert_eq!(before_update["total"], 1);
    // A new source revision must not return the old artwork held by a browser ID.
    let xml=format!("<gameList><game><path>Example.nes</path><name>Example Game</name><desc>{description}</desc><image>new.png</image></game></gameList>");
    std::fs::write(games.join("gamelist.xml"), xml).unwrap();
    std::fs::write(games.join("Second.nes"), b"fixture").unwrap();
    let cache =
        cache::build_system_checked(&browse::Library::open(&config).unwrap(), &mut vec![]).unwrap();
    cache::save_system(&app.cache_dir, "NES", &cache).unwrap();
    let initial = command(&mut app, json!({"action":"game","id":id}));
    let changed = completed(&mut app, initial);
    assert_ne!(changed["image"], game["image"]);
    for (offset, title, total, expected) in [
        (0, "", 2, "Example Game"),
        (1, "", 2, "Second.nes"),
        (0, "Example", 1, "Example Game"),
        (0, "", 2, "Example Game"),
    ] {
        let pending = command(
            &mut app,
            json!({"action":"explore","query":{"system":"NES","title":title},"offset":offset,"limit":1}),
        );
        let result = completed(&mut app, pending);
        assert_eq!(
            result["total"], total,
            "Cached projection must track source and query changes"
        );
        assert_eq!(result["items"][0]["name"], expected);
        assert_eq!(
            result["letters"],
            if title.is_empty() {
                json!([{"letter":"E","offset":0},{"letter":"S","offset":1}])
            } else {
                json!([{"letter":"E","offset":0}])
            }
        );
    }
    let mut history = crate::history::History::default();
    for (system, name) in [
        ("NES", "Second"),
        ("NES", "Missing"),
        ("Unavailable", "Gone"),
        ("NES", "Example"),
    ] {
        history.entries.push(crate::history::Entry {
            system: system.into(),
            name: name.into(),
            launch: browse::Launch::File(games.join(format!("{name}.nes"))),
        });
    }
    history
        .save(&crate::history::path_beside(&app.settings_path))
        .unwrap();
    for _ in 0..2 {
        let pending = command(&mut app, json!({"action":"history"}));
        let recent = completed(&mut app, pending);
        assert_eq!(
            recent["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["Second.nes", "Example Game"],
            "Grouping reads by system preserves history order"
        );
        assert_eq!(
            recent["items"][1]["image"], changed["image"],
            "History uses the current artwork source on each visit"
        );
        assert_eq!(recent["notices"].as_array().unwrap().len(), 2);
    }
    let original = std::fs::read(games.join("gamelist.xml")).unwrap();
    let launch = browse::Launch::File(games.join("Example.nes"));
    assert!(crate::scraper::install_uploaded_image(
        &app.all_systems,
        "NES",
        &launch,
        "Example Game",
        b"not an image",
        &Default::default()
    )
    .is_err());
    assert_eq!(
        std::fs::read(games.join("gamelist.xml")).unwrap(),
        original,
        "Rejected uploads must not change the current gamelist or artwork"
    );
    let image = include_bytes!("../assets/degauss-logo.png");
    crate::scraper::install_uploaded_image(
        &app.all_systems,
        "NES",
        &launch,
        "Example Game",
        image,
        &Default::default(),
    )
    .unwrap();
    let saved = std::fs::read_to_string(games.join("gamelist.xml")).unwrap();
    assert!(saved.contains("./media/degauss/"));
    assert!(saved.contains(&description));
    assert!(saved.contains("<name>Example Game</name>"));
    let image_start = saved.find("<image>").unwrap() + "<image>".len();
    let image_end = saved[image_start..].find("</image>").unwrap() + image_start;
    assert_eq!(
        std::fs::read(games.join(&saved[image_start..image_end])).unwrap(),
        image,
        "An accepted upload writes the actual image and preserves all existing metadata"
    );
    let parsed:WebCommand=serde_json::from_value(json!({"action":"home-edit","revision":app.web_revision(),"edit":{"operation":"remove","id":folder_id}})).unwrap();
    app.kiosk_locked = true;
    assert!(matches!(
        app.web_command(parsed),
        Err(WebError { code: "locked", .. })
    ));
    app.kiosk_locked = false;
    app.web.configured = Some(crate::web::Config {
        mode: crate::web::Mode::Http,
        port: 8443,
    });
    app.settings.web_access = None;
    app.poll_web();
    assert!(app.web.server.is_none());
    assert!(
        app.web.library.is_none(),
        "Switching Web Access Off drops its idle read worker too"
    );
    for purpose in [NamePurpose::WebPort, NamePurpose::WebPassword] {
        app.open_name_keyboard(purpose.clone(), "cancelled draft".into());
        app.handle(Action::Quit);
        assert_eq!(app.screen, Screen::Options);
        assert!(app.name_keyboard_draft.is_empty());
        assert!(app.settings.web_access.is_none());
        assert!(!crate::web::password_is_set(&app.web_directory()));
        app.open_name_keyboard(purpose, "cancelled draft".into());
        let cancel = name_keyboard::keys(NamePage::Lower, true)
            .iter()
            .position(|key| *key == NameKey::Cancel)
            .unwrap();
        app.name_keyboard_list.select(cancel);
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::Options);
        assert!(!crate::web::password_is_set(&app.web_directory()));
    }
    for (purpose, value) in [
        (NamePurpose::WebPort, "8080"),
        (NamePurpose::WebPassword, "native-keyboard-test-password"),
    ] {
        app.open_name_keyboard(purpose, value.into());
        let save = name_keyboard::keys(NamePage::Lower, true)
            .iter()
            .position(|key| *key == NameKey::Save)
            .unwrap();
        app.name_keyboard_list.select(save);
        app.handle(Action::Accept);
        assert_eq!(app.screen, Screen::Options);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.web.password_job.is_some() {
        app.poll_web();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(crate::web::password_is_set(&app.web_directory()));
    assert_eq!(app.settings.web_access.as_ref().unwrap().port, 8080);
    assert!(
        app.web.server.is_none(),
        "Saving credentials must not enable the listener"
    );
    random_selection(&mut app, &root, &games, &config);
    native_web_compatibility(&mut app, &root);
    if let Some(output) = std::env::var_os("DEGAUSS_WEB_VISUAL_DIR") {
        render_documentation_examples(&mut app, Path::new(&output));
    }
    app.ui.hide().unwrap();
}

fn native_web_compatibility(app: &mut App, root: &Path) {
    let mut failures = Vec::new();
    let original_settings = app.settings.clone();
    app.message = None;
    app.settings.font = Some("smooth 2".into());
    app.settings.theme_font_override = Some(true);
    app.save_settings();
    edit(
        app,
        json!({"action":"theme-save","name":"Web Review","text":"font = \"pixel\"","replace":false}),
    );
    if app.settings.theme_font_override != Some(true) {
        failures.push("Saving an unselected theme changed the active font override");
    }
    app.web_select_theme(Some("Web Review")).unwrap();
    app.settings.theme = Some("web review".into());
    app.settings.theme_font_override = Some(true);
    app.save_settings();
    let request = serde_json::from_value(json!({"action":"theme-save","revision":app.web_revision(),"name":"Other Theme","text":"font = \"pixel 2\"","replace":false})).unwrap();
    if app.web_command(request).is_err() {
        failures.push("Saving another theme cannot refresh a case-insensitive saved selection");
    }
    if app.settings.theme.as_deref() != Some("web review")
        || app.settings.theme_font_override != Some(true)
    {
        failures.push("Theme save changed existing persisted theme preferences");
    }
    edit(app, json!({"action":"theme-delete","name":"Other Theme"}));
    if app
        .active_theme
        .and_then(|index| app.themes.get(index))
        .map(|theme| theme.name.as_str())
        != Some("Web Review")
    {
        failures.push("Deleting an unselected theme changed the active theme index");
    }
    if app.web_select_theme(Some("WEB REVIEW")).is_err() {
        failures.push("Explicit theme selection stopped accepting native case-insensitive names");
    }
    edit(app, json!({"action":"theme-delete","name":"web review"}));
    if app.settings.theme.is_some() || app.active_theme.is_some() {
        failures.push("Deleting the current theme left a stale case-insensitive selection");
    }

    let question = "Reset view selections?";
    app.message = Some(question.into());
    app.pending = Some(Pending::SwitchIni(1));
    let request = serde_json::from_value(json!({"action":"option-adjust","revision":app.web_revision(),"option":"WebCrtImages","delta":1})).unwrap();
    if app.web_command(request).is_ok() || app.message.as_deref() != Some(question) {
        failures.push("Browser option adjustment hid a native confirmation");
    }
    app.message = Some(question.into());
    let request =
        serde_json::from_value(json!({"action":"rebuild","system":"missing-test-system"})).unwrap();
    let _ = app.web_command(request);
    if app.message.as_deref() != Some(question) {
        failures.push("Browser rebuild hid a native confirmation before validation");
    }

    let upload = crate::web::upload::Job::start(
        crate::web::upload::Destination::Logo(root.join("review-logos")),
        include_bytes!("../assets/degauss-logo.png").to_vec(),
    )
    .unwrap();
    app.web.upload_job = Some((u64::MAX, "".into(), "".into(), upload));
    for action in [Action::Quit, Action::Accept] {
        app.message = Some(question.into());
        app.handle(action);
        if app.message.as_deref() != Some(question) || app.pending.is_none() {
            failures.push("A blocked button hid or replaced a pending native question");
        }
    }
    app.pending = None;
    app.message = None;
    app.screen = Screen::Browse;
    app.open_system = Some("NES".into());
    app.home.pending_open = Some(crate::home::Target::System {
        system: "NES".into(),
    });
    app.poll_home_open();
    if app.home.pending_open.is_none() {
        failures.push("A pending Home launch was consumed during a browser write");
    }
    app.boot_launch_waiting = true;
    app.boot_launch_deadline = Some(Instant::now());
    let _ = app.poll_boot_launch(Instant::now());
    if !app.boot_launch_waiting || app.boot_launch_deadline.is_none() {
        failures.push("Boot launch was consumed while a browser write was committing");
    }
    app.boot_launch_waiting = false;
    app.boot_launch_deadline = None;
    app.web.upload_job.take();
    app.poll_home_open();
    assert!(
        app.home.pending_open.is_none(),
        "Deferred Home opening must resume after the write completes"
    );

    let job_id = app.web_new_job("Applying controller preferences");
    app.web.controller_job = Some(job_id);
    app.controllers.job = Some((
        ControllerJob::Apply(Default::default()),
        controller::Job::start(
            &root.join("absent-controller-fifo"),
            controller::Request::Apply(vec![]),
        )
        .unwrap(),
    ));
    app.controller_error("Reading active controller: report was replaced".into());
    assert_eq!(app.web.controller_job, Some(job_id));
    assert_eq!(app.web.jobs[&job_id]["state"], "running");
    app.controllers.initialized = true;
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.controllers.job.is_some() {
        app.poll_controllers(Instant::now());
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        app.web.jobs[&job_id]["state"], "failed",
        "The actual apply failure, not the unrelated report error, must finish the browser job"
    );
    assert!(app.web.controller_job.is_none());
    app.message = None;

    let script = root.join("Scripts/Review.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    let script = app.web_register(crate::home::Target::Script { path: script }, None);
    app.settings.speed_step = Some(3);
    app.web_launch(&script).unwrap();
    assert!(matches!(app.web.outcome.take(), Some(Outcome::Script(_))));
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().speed_step,
        Some(3)
    );
    let settings_path = app.settings_path.clone();
    app.settings_path = root.join("not-a-settings-file");
    std::fs::create_dir_all(&app.settings_path).unwrap();
    assert!(
        app.web_launch(&script).is_err(),
        "A failed settings save must stop script handover"
    );
    assert!(app.web.outcome.is_none());
    app.settings_path = settings_path;
    app.message = None;

    let shelf_path = root.join("review-favourites");
    std::fs::create_dir_all(&shelf_path).unwrap();
    let mut shelf = app.all_systems[0].clone();
    shelf.def.id = "Favorites".into();
    shelf.paths = vec![shelf_path.clone()];
    if crate::scraper::install_uploaded_image(
        &[shelf],
        "Favorites",
        &browse::Launch::File(shelf_path.join("Game.mgl")),
        "Game",
        include_bytes!("../assets/degauss-logo.png"),
        &Default::default(),
    )
    .is_ok()
    {
        failures.push("Legacy Favourites definitions accepted artwork at the shelf root");
    }
    app.settings = original_settings;
    app.message = None;
    app.active_theme = None;
    app.save_settings();
    app.web_refresh_theme().unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn random_selection(
    app: &mut App,
    root: &Path,
    games: &Path,
    config: &crate::config::SystemConfig,
) {
    let native = (
        app.screen,
        app.open_system.clone(),
        app.game_list.selected(),
    );
    let settings = std::fs::read(&app.settings_path).unwrap();
    let nested = games.join("Nested/Child");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("Nested.nes"), b"game path fixture").unwrap();
    let xml = std::fs::read_to_string(games.join("gamelist.xml")).unwrap();
    std::fs::write(
        games.join("gamelist.xml"),
        xml.replace(
            "</gameList>",
            "<game><path>Second.nes</path><genre>Action</genre></game></gameList>",
        ),
    )
    .unwrap();
    let library = browse::Library::open(config).unwrap();
    let cache = cache::build_system_checked(&library, &mut vec![]).unwrap();
    cache::save_system(&app.cache_dir, "NES", &cache).unwrap();
    let favorites = root.join("_@Favorites");
    std::fs::create_dir_all(&favorites).unwrap();
    let mgl = crate::launch::favorite_mgl(config, &games.join("Second.nes"))
        .unwrap()
        .unwrap();
    crate::favorites::add_game(&favorites, "Second", &mgl).unwrap();
    app.favorites = crate::favorites::Favorites::read_with(&favorites, &app.homes());
    assert!(app.favorites.holds(&games.join("Second.nes")));
    let pending = command(app, json!({"action":"browse","system":"NES"}));
    let page = completed(app, pending);
    let folder = page["folder"].clone();
    for scope in [json!({"folder":folder}), json!({"query":{"system":"NES"}})] {
        let mut request = scope.clone();
        request["action"] = json!("random");
        request["favorites_only"] = json!(true);
        let pending = command(app, request);
        let picked = completed(app, pending);
        assert_eq!(picked["items"][0]["name"], "Second.nes");
        assert_eq!(picked["items"][0]["favorite"], true);
        assert_eq!(picked["items"].as_array().unwrap().len(), 1);
        let pending = command(app, json!({"action":"game","id":picked["items"][0]["id"]}));
        assert_eq!(
            completed(app, pending)["favorite"],
            true,
            "Opening the random favourite's details must retain its real favourite state"
        );
    }
    let pending = command(app, json!({"action":"explore","query":{"system":"NES"}}));
    let explored = completed(app, pending);
    assert_eq!(
        explored["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "Second.nes")
            .unwrap()["favorite"],
        true
    );
    // Legacy query-only API calls still choose from the complete filtered list,
    // not the browser's first page. The folder equivalent applies its search.
    for folder in [JsonValue::Null, folder.clone()] {
        let pending = command(
            app,
            json!({"action":"random","folder":folder,"query":{"system":"NES","fields":{"Genre":{"Known":{"key":"action","label":"Action"}}}}}),
        );
        assert_eq!(
            completed(app, pending)["items"][0]["name"],
            "Second.nes",
            "Random picks must preserve the active metadata filters"
        );
        let pending = command(
            app,
            json!({"action":"random","folder":folder,"query":{"system":"NES","title":"Second"}}),
        );
        assert_eq!(completed(app, pending)["items"][0]["name"], "Second.nes");
        let pending = command(
            app,
            json!({"action":"random","folder":folder,"favorites_only":true,"query":{"system":"NES","title":"Example"}}),
        );
        let id = pending["job"].as_u64().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.web.jobs[&id]["state"] == "running" {
            app.poll_web_jobs();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(app.web.jobs[&id]["state"], "failed");
        assert_eq!(app.web.jobs[&id]["error"]["code"], "empty");
    }
    let child = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "Nested")
        .unwrap();
    let pending = command(app, json!({"action":"random","folder":child["id"]}));
    assert_eq!(
        completed(app, pending)["items"][0]["name"],
        "Nested.nes",
        "A folder of folders follows native descending picks without loading Explore"
    );
    let game = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "Second.nes")
        .unwrap();
    let invalid = serde_json::from_value(json!({"action":"random","folder":game["id"]})).unwrap();
    assert!(
        app.web_command(invalid).is_err(),
        "A game ID is not a folder scope"
    );
    let category = app.all_systems[0].menu_folder.clone();
    app.all_systems[0].menu_folder = Some("Utility".into());
    app.kiosk_locked = true;
    let forbidden = serde_json::from_value(json!({"action":"random","folder":folder})).unwrap();
    assert!(
        matches!(
            app.web_command(forbidden),
            Err(WebError { code: "locked", .. })
        ),
        "Previously issued folder IDs must respect current kiosk restrictions"
    );
    app.kiosk_locked = false;
    app.all_systems[0].menu_folder = category;
    assert!(
        app.web.outcome.is_none(),
        "Random selection does not launch a game"
    );
    assert_eq!(
        (
            app.screen,
            app.open_system.clone(),
            app.game_list.selected()
        ),
        native
    );
    assert_eq!(std::fs::read(&app.settings_path).unwrap(), settings);
    app.web.library.take();
}

// Documentation uses a reserved example address, never the test device's LAN
// address. These are renderer examples; device acceptance uses real captures.
fn render_documentation_examples(app: &mut App, output: &Path) {
    use crate::surface::{MemorySurface, PixelFormat};
    std::fs::create_dir_all(output).unwrap();
    let config = crate::web::Config {
        mode: crate::web::Mode::Https,
        port: 8443,
    };
    let ip = "192.0.2.1".parse().unwrap();
    let address = config.address(ip);
    app.settings.web_access = Some(config);
    app.settings.web_crt_images = Some(true);
    app.web.addresses = vec![ip];
    app.web.error = None;
    app.message = None;
    select_option(app, OptionsPage::WebAccess, OptionId::WebCrtImages);
    let mut surface = MemorySurface::new(app.width, app.height, PixelFormat::Xrgb8888);
    let mut presenter = Presenter::with_rotation(
        surface.geometry(),
        PresentMode::Direct,
        app.screen_rotation(),
    );
    app.dirty = true;
    app.render_once(&mut surface, &mut presenter).unwrap();
    surface
        .write_bmp(&output.join("native-web-access-example.bmp"))
        .unwrap();

    select_option(app, OptionsPage::WebAccess, OptionId::WebAddress);
    let (side, pixels) = crate::web::qr_pixels(&address, 140).unwrap();
    let buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
        &pixels,
        side as u32,
        side as u32,
    );
    app.web.qr = Some((side, slint::Image::from_rgb8(buffer)));
    app.web.qr_message = Some(address.clone());
    app.message = Some(address);
    app.dirty = true;
    app.render_once(&mut surface, &mut presenter).unwrap();
    surface
        .write_bmp(&output.join("native-web-qr-example.bmp"))
        .unwrap();
    assert!(
        app.web.server.is_none(),
        "Documentation rendering must not start a listener"
    );
}
