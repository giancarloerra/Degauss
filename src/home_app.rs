// Home organisation shares the ordinary renderer, source consent and handover.
#[derive(Default)]
struct HomeBrowser {
    folder: Option<String>,
    rows: Vec<String>,
    edit: Option<crate::home::Store>,
    explicit_editor: bool,
    edit_origin: Option<crate::home::Resume>,
    edit_return: Option<(Screen, ListState)>,
    menu: Option<HomeMenu>,
    menu_keys: Vec<String>,
    pending_entry: Option<crate::home::Entry>,
    image: Option<String>,
    origin: Option<crate::home::Resume>,
    pending_open: Option<crate::home::Target>,
    direct_game: bool,
    previews: std::collections::BTreeMap<String, browse::Row>,
    preview_done: HashSet<String>,
    preview_requested: Vec<String>,
    preview_job: Option<crate::home::PreviewJob>,
    preview_waiting: HashSet<String>,
}

#[derive(Clone)]
enum HomeMenu {
    Actions,
    Editor,
    Entry(String),
    Destination(Option<String>),
}
const EDIT_HOME: &str = "Edit Home";
const EDIT_HOME_FOLDER: &str = "Edit Home Folder";
const ADD_HOME: &str = "Add to Home";
const NEW_HOME_FOLDER: &str = "New Personal Folder";
const SAVE_HOME: &str = "Save Changes";
const CANCEL_HOME: &str = "Cancel Changes";

impl App {
    fn home_location_label(&self) -> &'static str {
        if self.home.folder.is_some() {
            "Home Folder"
        } else {
            "Home"
        }
    }

    fn home_edit_label(&self) -> &'static str {
        if self.home.folder.is_some() {
            EDIT_HOME_FOLDER
        } else {
            EDIT_HOME
        }
    }

    fn home_store(&self) -> &crate::home::Store {
        self.home.edit.as_ref().unwrap_or(&self.settings.home)
    }

    fn rebuild_home_rows(&mut self) {
        if self
            .home
            .folder
            .as_ref()
            .is_some_and(|id| !self.settings.home.entries.contains_key(id))
        {
            self.home.folder = None;
        }
        let categories = self
            .categories
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        self.home.rows = self
            .settings
            .home
            .rows(self.home.folder.as_deref(), &categories, false);
        self.home.previews.clear();
        self.home.preview_done.clear();
        self.home.preview_job = None;
        self.home.preview_waiting.clear();
    }

    fn selected_home_key(&self) -> Option<&str> {
        self.home
            .rows
            .get(self.category_list.selected())
            .map(String::as_str)
    }

    fn home_label(&self, key: &str) -> String {
        if let Some(category) = key.strip_prefix("category:") {
            return category.to_string();
        }
        key.strip_prefix("entry:")
            .and_then(|id| self.home_store().entries.get(id))
            .map(|entry| match entry.target {
                crate::home::Target::Folder { .. } => format!("[ {} ]", entry.name),
                _ => entry.name.clone(),
            })
            .unwrap_or_else(|| "Unavailable Home entry".into())
    }

    fn home_problem(&self, key: &str) -> Option<String> {
        if let Some(category) = key.strip_prefix("category:") {
            return (!self.categories.iter().any(|(name, _)| name == category))
                .then(|| "Disabled or unavailable in Library options".into());
        }
        let entry = self.home_store().entries.get(key.strip_prefix("entry:")?)?;
        use crate::home::Target;
        let owner = match &entry.target {
            Target::System { system }
            | Target::Game { system, .. }
            | Target::LibraryFolder { system, .. } => Some(system),
            _ => None,
        };
        if let Some(id) = owner {
            if !self.all_systems.iter().any(|system| &system.def.id == id) {
                return Some("Owning system is not installed".into());
            }
            if !self.show_hidden && self.settings.hidden.contains(id) {
                return Some("Owning system is hidden".into());
            }
            if !self.show_empty
                && self
                    .empty_systems
                    .as_ref()
                    .is_some_and(|empty| empty.contains(id))
            {
                return Some("Owning system is hidden by the empty-system setting".into());
            }
        }
        match &entry.target {
            Target::Category { category } => {
                (!self.categories.iter().any(|(name, _)| name == category))
                    .then(|| "Category is disabled or unavailable".into())
            }
            Target::Collection { collection } => {
                (!self.settings.collections.contains_key(collection))
                    .then(|| "Saved collection is unavailable".into())
            }
            Target::Game { system, launch } => {
                let path = match launch {
                    browse::Launch::File(path) => crate::zip::split_member_path(path)
                        .map(|(archive, _)| archive)
                        .unwrap_or_else(|| path.clone()),
                    browse::Launch::AmigaVision { install, .. } => install.clone(),
                };
                if !path.exists() {
                    Some("Game storage or file is unavailable".into())
                } else if self.home_hidden_path(system, &path) {
                    Some("Game folder is hidden".into())
                } else {
                    None
                }
            }
            Target::LibraryFolder { system, trail } => trail.last().and_then(|place| {
                let path = place.path();
                (!path.exists() || self.home_hidden_path(system, path))
                    .then(|| "Library folder is unavailable or hidden".into())
            }),
            Target::Core { path } => {
                (!path.is_file()).then(|| "Installed core is unavailable".into())
            }
            Target::Script { path } => {
                (!path.is_file()).then(|| "Installed script is unavailable".into())
            }
            _ => None,
        }
    }

    fn home_art(&self, key: &str) -> (Option<PathBuf>, bool) {
        if let Some(category) = key.strip_prefix("category:") {
            return (
                self.category_logo(category),
                self.category_art_is_game_art(category),
            );
        }
        let Some(entry) = key
            .strip_prefix("entry:")
            .and_then(|id| self.settings.home.entries.get(id))
        else {
            return (None, false);
        };
        if let Some(path) = &entry.image {
            return (Some(path.clone()), false);
        }
        use crate::home::Target;
        match &entry.target {
            Target::Category { category } => (
                self.category_logo(category),
                self.category_art_is_game_art(category),
            ),
            Target::System { system } | Target::LibraryFolder { system, .. } => {
                (self.load_explore_logo(system), false)
            }
            Target::Game { system, .. } => {
                let cover = self
                    .home
                    .previews
                    .get(key)
                    .and_then(|row| row.cover.clone());
                let game_art = cover.is_some();
                (cover.or_else(|| self.load_explore_logo(system)), game_art)
            }
            _ => (None, false),
        }
    }

    fn home_hidden_path(&self, _system: &str, path: &Path) -> bool {
        !self.show_hidden
            && self
                .settings
                .hidden_paths
                .iter()
                .filter_map(|key| key_path(key))
                .any(|hidden| path.starts_with(hidden))
    }

    fn resolve_home_preview(&mut self) {
        if self.screen != Screen::Browse
            || self.browsing != Browsing::Categories
            || self.home.preview_job.is_some()
        {
            return;
        }
        let mut sources = std::collections::BTreeMap::<String, crate::home::PreviewSource>::new();
        let mut requested = Vec::new();
        let (range, _) = self.category_list.window();
        let keys = self.home.rows[range].to_vec();
        for key in keys {
            if self.home.preview_done.contains(&key) {
                continue;
            }
            let Some(entry) = key
                .strip_prefix("entry:")
                .and_then(|id| self.settings.home.entries.get(id))
                .cloned()
            else {
                self.home.preview_done.insert(key);
                continue;
            };
            let crate::home::Target::Game { system, launch } = entry.target else {
                self.home.preview_done.insert(key);
                continue;
            };
            if self.home_problem(&key).is_some() || self.source_problem(&system).is_some() {
                continue;
            }
            if !sources.contains_key(&system) {
                let root =
                    crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, &system)
                        .map(Path::to_path_buf);
                let provider = if let Some(root) = &root {
                    match self.provider_from_state(&system, root) {
                        Ok(Some(provider)) => Some(provider),
                        Ok(None) => {
                            if self.home.preview_waiting.insert(system.clone()) {
                                self.queue_provider_read(&system, false);
                                self.start_provider_job_if_ready();
                            }
                            continue;
                        }
                        Err(error) => {
                            self.message = Some(error.to_string());
                            self.home.preview_done.insert(key);
                            continue;
                        }
                    }
                } else {
                    None
                };
                sources.insert(
                    system.clone(),
                    crate::home::PreviewSource {
                        system: system.clone(),
                        pack: root.is_some(),
                        provider,
                        targets: Vec::new(),
                    },
                );
            }
            sources
                .get_mut(&system)
                .expect("Home source")
                .targets
                .push((key.clone(), launch));
            requested.push(key);
        }
        if sources.is_empty() {
            return;
        }
        self.home.preview_requested = requested;
        match crate::home::PreviewJob::start(
            self.cache_dir.clone(),
            sources.into_values().collect(),
        ) {
            Ok(job) => self.home.preview_job = Some(job),
            Err(error) => self.message = Some(error),
        }
    }

    fn poll_home_preview(&mut self) {
        if self.home.preview_job.is_none() {
            self.resolve_home_preview();
        }
        let Some(result) = self
            .home
            .preview_job
            .as_ref()
            .and_then(crate::home::PreviewJob::poll)
        else {
            return;
        };
        self.home.preview_job = None;
        self.home
            .preview_done
            .extend(std::mem::take(&mut self.home.preview_requested));
        match result {
            Ok(preview) => {
                self.home.preview_done.extend(preview.missing);
                self.home.previews.extend(preview.rows);
            }
            Err(error) => self.message = Some(error),
        }
        self.dirty = true;
        self.art_pending = true;
        self.resolve_home_preview();
    }

    fn home_resume(&self) -> crate::home::Resume {
        crate::home::Resume {
            folder: self.home.folder.clone(),
            key: self.selected_home_key().unwrap_or_default().into(),
            anchor: None,
        }
    }

    fn home_selected_game(&self) -> Option<(&str, &browse::Row)> {
        if self.browsing != Browsing::Categories {
            return None;
        }
        let key = self.selected_home_key()?;
        let entry = self
            .settings
            .home
            .entries
            .get(key.strip_prefix("entry:")?)?;
        let crate::home::Target::Game { system, .. } = &entry.target else {
            return None;
        };
        let row = self.home.previews.get(key)?;
        Some((system, row))
    }

    fn restore_home(&mut self, resume: crate::home::Resume) {
        if self.in_misterzine_browser() {
            if let Err(error) = self.acknowledge_core_changes() {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
            self.misterzine_job = None;
            self.clear_misterzine_games();
        }
        self.home.origin = None;
        self.home.edit = None;
        self.home.explicit_editor = false;
        self.home.edit_origin = None;
        self.home.edit_return = None;
        self.home.pending_open = None;
        self.home.direct_game = false;
        self.home.folder = resume.folder;
        self.explore.active = false;
        self.open_category = None;
        self.open_system = None;
        self.last_played_open = false;
        self.filter.clear();
        self.game_filters = GameFilters::default();
        self.browsing = Browsing::Categories;
        self.screen = Screen::Browse;
        self.home.menu = None;
        self.rebuild_system_list();
        if let Some(index) = self.home.rows.iter().position(|key| key == &resume.key) {
            self.category_list.select(index);
        }
        self.resolve_view();
        self.apply_geometry();
        self.touch_selection();
    }

    fn open_home_entry(&mut self) -> Option<Outcome> {
        let key = self.selected_home_key()?.to_string();
        if key.starts_with("category:") {
            self.home.origin = None;
            self.open_selected_category();
            return None;
        }
        if let Some(problem) = self.home_problem(&key) {
            self.message = Some(format!("{}: {problem}", self.home_label(&key)));
            self.dirty = true;
            return None;
        }
        let entry = self
            .settings
            .home
            .entries
            .get(key.strip_prefix("entry:")?)?
            .clone();
        use crate::home::Target;
        if let Target::Folder { .. } = entry.target {
            self.home.folder = key.strip_prefix("entry:").map(str::to_string);
            self.rebuild_home_rows();
            self.category_list = ListState::new(self.home.rows.len(), self.geometry.visible);
            self.resolve_view();
            self.apply_geometry();
            self.touch_selection();
            if entry.image.as_ref().is_some_and(|path| !path.is_file()) {
                self.message = Some("This personal folder's custom image is unavailable. Its saved image choice has been retained.".into());
            }
            return None;
        }
        self.home.origin = Some(self.home_resume());
        match entry.target {
            Target::Category { category } => {
                self.home.pending_open = Some(Target::Category {
                    category: category.clone(),
                });
                self.open_category_named(category);
            }
            Target::Collection { collection } => {
                if let Some(saved) = self.settings.collections.get(&collection) {
                    self.open_explore(Some(saved.query.clone()));
                }
            }
            Target::System { ref system }
            | Target::LibraryFolder { ref system, .. }
            | Target::Game { ref system, .. } => {
                let id = system.clone();
                let system = self.all_systems.iter().find(|system| system.def.id == id)?;
                self.open_category = Some(
                    display_category(
                        system,
                        self.settings.separate_handheld_category.unwrap_or(false),
                    )
                    .to_string(),
                );
                self.rebuild_system_list();
                if let Some(index) = self.systems.iter().position(|system| system.def.id == id) {
                    self.system_list.select(index);
                    self.browsing = Browsing::Systems;
                    self.home.pending_open = Some(entry.target);
                    self.open_selected_system();
                } else {
                    self.message = Some(
                        "The pinned system is unavailable under the current Library options."
                            .into(),
                    );
                }
            }
            Target::Core { path } => {
                match crate::launch::plan_core(&path, Path::new(&self.config.menu_root)) {
                    Ok(plan) => {
                        return Some(Outcome::Launch {
                            plan: Box::new(plan),
                            name: entry.name,
                            history: None,
                            active_game: None,
                        })
                    }
                    Err(error) => self.message = Some(error.to_string()),
                }
            }
            Target::Script { path } => {
                self.open_scripts();
                if let Some(parent) = path.parent() {
                    self.show_scripts_directory(parent.to_path_buf(), Some(&path));
                    if self
                        .scripts_entries
                        .get(self.menu_list.selected())
                        .is_some_and(|entry| entry.path == path)
                    {
                        self.activate_script();
                    } else if self.message.is_none() {
                        self.message = Some("The pinned script could not be selected.".into());
                    }
                }
            }
            Target::Folder { .. } => unreachable!(),
        }
        if self.home.pending_open.is_none()
            && !matches!(self.screen, Screen::Scripts | Screen::Screensaver)
        {
            let anchor = self.position_without_home();
            if let Some(origin) = &mut self.home.origin {
                origin.anchor = Some(Box::new(anchor));
            }
        }
        None
    }

    fn position_without_home(&self) -> crate::state::State {
        let mut saved = self.position_browse();
        saved.home = None;
        saved
    }

    fn poll_home_open(&mut self) -> Option<Outcome> {
        if self.screen != Screen::Browse
            || self.opening.is_some()
            || self.source_resolution.is_some()
            || self.source_job.is_some()
            || self.provider_job.is_some()
            || self.message.is_some()
        {
            return None;
        }
        let target = self.home.pending_open.as_ref()?;
        if matches!(target, crate::home::Target::Category { .. }) {
            if self.explore.job.is_some() {
                return None;
            }
            self.home.pending_open = None;
            let anchor = self.position_without_home();
            if let Some(origin) = &mut self.home.origin {
                origin.anchor = Some(Box::new(anchor));
            }
            return None;
        }
        let system = match target {
            crate::home::Target::System { system }
            | crate::home::Target::LibraryFolder { system, .. }
            | crate::home::Target::Game { system, .. } => system,
            _ => return None,
        };
        if self.open_system.as_deref() != Some(system) {
            return None;
        }
        let target = self.home.pending_open.take()?;
        match target {
            crate::home::Target::LibraryFolder { system, trail } => {
                let places = trail
                    .iter()
                    .cloned()
                    .map(|place| (place, 0))
                    .collect::<Vec<_>>();
                let saved = crate::state::State::record(
                    &system,
                    self.open_category.as_deref().unwrap_or_default(),
                    &places,
                    0,
                    &self.left_at,
                    &self.category_system,
                );
                self.restore_saved_trail(&saved);
                if trail.last().map(Place::key) != self.trail.last().map(|crumb| crumb.place.key())
                {
                    self.message =
                        Some("The exact pinned library folder could not be opened.".into());
                }
            }
            crate::home::Target::Game { launch, .. } => {
                let key = self.home.origin.as_ref()?.key.clone();
                let entry = self
                    .settings
                    .home
                    .entries
                    .get(key.strip_prefix("entry:")?)?;
                let row = self
                    .system_cache
                    .as_ref()
                    .and_then(|cache| {
                        cache
                            .folders
                            .values()
                            .flat_map(|folder| &folder.rows)
                            .find(|row| row.kind == browse::Kind::Play(launch.clone()))
                            .cloned()
                    })
                    .unwrap_or_else(|| browse::Row {
                        name: entry.name.clone(),
                        sort_key: entry.name.to_lowercase(),
                        kind: browse::Kind::Play(launch),
                        cover: None,
                        genre: None,
                        favorite: false,
                        below: None,
                        details: Default::default(),
                    });
                self.here = vec![row];
                self.game_list = ListState::new(1, self.geometry.visible);
                self.browsing = Browsing::Games;
                self.home.direct_game = true;
                return self.confirm_launch();
            }
            _ => {}
        }
        let anchor = self.position_without_home();
        if let Some(origin) = &mut self.home.origin {
            origin.anchor = Some(Box::new(anchor));
        }
        None
    }

    fn home_pin_target(&self) -> Option<crate::home::Entry> {
        use crate::home::{Entry, Target};
        let (name, target) = if self.screen == Screen::Scripts {
            let script = self.scripts_entries.get(self.menu_list.selected())?;
            if script.is_directory {
                return None;
            }
            (
                script.name.clone(),
                Target::Script {
                    path: script.path.clone(),
                },
            )
        } else if self.explore.active {
            if let ExploreMenu::Collection(id) = &self.explore.menu {
                let saved = self.settings.collections.get(id)?;
                (
                    saved.name.clone(),
                    Target::Collection {
                        collection: id.clone(),
                    },
                )
            } else {
                let game = self.explore_selected()?;
                let browse::Kind::Play(launch) = &game.row.kind else {
                    return None;
                };
                (
                    game.row.name.clone(),
                    Target::Game {
                        system: game.system.clone(),
                        launch: launch.clone(),
                    },
                )
            }
        } else {
            match self.browsing {
                Browsing::Categories => {
                    let category = self.selected_category_name()?;
                    (
                        category.into(),
                        Target::Category {
                            category: category.into(),
                        },
                    )
                }
                Browsing::Systems if !self.in_cores_browser() => {
                    let system = self.systems.get(self.system_list.selected())?;
                    (
                        system.name().into(),
                        Target::System {
                            system: system.def.id.clone(),
                        },
                    )
                }
                Browsing::Games if !self.in_misterzine_browser() => {
                    let row = self.browse_row(self.game_list.selected())?;
                    if self.in_cores_browser() {
                        let browse::Kind::Play(browse::Launch::File(path)) = &row.kind else {
                            return None;
                        };
                        (row.name.clone(), Target::Core { path: path.clone() })
                    } else {
                        let system = self
                            .explore_selected()
                            .map(|entry| entry.system.clone())
                            .or_else(|| {
                                self.selected_last_played()
                                    .map(|entry| entry.system.clone())
                            })
                            .or_else(|| self.open_system.clone())?;
                        let target = match &row.kind {
                            browse::Kind::Play(launch) => Target::Game {
                                system,
                                launch: launch.clone(),
                            },
                            browse::Kind::Enter(place) => {
                                let mut trail = self
                                    .trail
                                    .iter()
                                    .map(|crumb| crumb.place.clone())
                                    .collect::<Vec<_>>();
                                trail.push(place.clone());
                                Target::LibraryFolder { system, trail }
                            }
                        };
                        (row.name.clone(), target)
                    }
                }
                _ => return None,
            }
        };
        Some(Entry {
            name,
            target,
            image: None,
        })
    }

    fn home_set_menu(&mut self, kind: HomeMenu, rows: Vec<String>, keys: Vec<String>) {
        self.home.menu = Some(kind);
        self.home.menu_keys = keys;
        self.menu = rows;
        self.context_page = None;
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::Context;
        self.apply_geometry();
        self.dirty = true;
    }

    fn home_context(&mut self) -> bool {
        if self.browsing != Browsing::Categories || self.explore.active {
            self.home.menu = None;
            return false;
        }
        let mut rows = vec![self.home_edit_label().into(), NEW_HOME_FOLDER.into()];
        if self
            .selected_home_key()
            .is_some_and(|key| key.starts_with("entry:"))
        {
            rows.push("Manage Entry".into());
        }
        if self.home_selected_game().is_some() {
            rows.push(GAME_INFORMATION.into());
        }
        if self.home_pin_target().is_some() {
            rows.push(ADD_HOME.into());
        }
        rows.push(CHANGE_VIEW.into());
        rows.extend(self.custom_view_actions());
        if self.has_custom_view() {
            rows.push(USE_GLOBAL_VIEW.into());
        }
        if let Some(target) = self.selected_image_target() {
            rows.push(CHANGE_CATEGORY_IMAGE.into());
            if self
                .logo_dir
                .as_deref()
                .is_some_and(|dir| target.has_override(dir))
            {
                rows.push(CLEAR_CATEGORY_IMAGE.into());
            }
        }
        self.home_set_menu(HomeMenu::Actions, rows, Vec::new());
        true
    }

    fn begin_home_edit(&mut self) {
        if self.home.edit.is_none() {
            self.home.edit_return = Some((
                if self.screen == Screen::Scripts {
                    Screen::Scripts
                } else {
                    Screen::Browse
                },
                self.menu_list.clone(),
            ));
            self.home.edit_origin =
                (self.browsing == Browsing::Categories).then(|| self.home_resume());
            self.home.edit = Some(self.settings.home.clone());
        }
    }

    fn home_editor(&mut self) {
        self.begin_home_edit();
        self.home.explicit_editor = true;
        let categories = self.home_editor_categories();
        let keys = self
            .home_store()
            .rows(self.home.folder.as_deref(), &categories, true);
        let mut rows = keys
            .iter()
            .map(|key| {
                let state = if self.home_store().hidden.contains(key) {
                    "Hidden"
                } else if self.home_problem(key).is_some() {
                    "Unavailable"
                } else {
                    "Shown"
                };
                format!("{} · {state}", self.home_label(key))
            })
            .collect::<Vec<_>>();
        rows.extend([NEW_HOME_FOLDER.into(), SAVE_HOME.into(), CANCEL_HOME.into()]);
        self.home_set_menu(HomeMenu::Editor, rows, keys);
    }

    fn home_editor_select(&mut self, key: &str) {
        self.home_editor();
        if let Some(at) = self.home.menu_keys.iter().position(|held| held == key) {
            self.menu_list.select(at);
        }
    }

    fn home_editor_categories(&self) -> Vec<String> {
        let mut categories = self
            .categories
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for name in [
            crate::explore::NAME,
            "Arcade",
            "Console",
            HANDHELD_CATEGORY,
            "Computer",
            "Utility",
            "Other",
            "Unstable",
            LAST_PLAYED_CATEGORY,
            FAVORITES_ID,
            CORES_CATEGORY,
            MISTERZINE_CATEGORY,
            SCRIPTS_CATEGORY,
            ATTRACT_MODE_CATEGORY,
        ] {
            if !categories.iter().any(|category| category == name) {
                categories.push(name.into());
            }
        }
        categories
    }

    fn home_entry_actions(&mut self, key: String) {
        let mut rows = vec![
            "Move Up".into(),
            "Move Down".into(),
            if self.home_store().hidden.contains(&key) {
                format!("Show on {}", self.home_location_label())
            } else {
                format!("Hide on {}", self.home_location_label())
            },
        ];
        if let Some(id) = key.strip_prefix("entry:") {
            rows.extend(["Rename".into(), "Move to Folder".into(), "Remove".into()]);
            if self
                .home_store()
                .entries
                .get(id)
                .is_some_and(|entry| matches!(entry.target, crate::home::Target::Folder { .. }))
            {
                rows.extend([
                    "Edit Folder".into(),
                    "Change Folder Image".into(),
                    "Remove Folder Image".into(),
                ]);
            }
        }
        rows.extend([SAVE_HOME.into(), CANCEL_HOME.into()]);
        self.home_set_menu(HomeMenu::Entry(key), rows, Vec::new());
    }

    fn home_destinations(&mut self, moving: Option<String>) {
        self.begin_home_edit();
        let mut rows = vec!["Home".into()];
        let mut keys = vec![String::new()];
        for (id, entry) in &self.home_store().entries {
            if matches!(entry.target, crate::home::Target::Folder { .. }) {
                let mut path = vec![entry.name.clone()];
                let mut parent = self.home_store().parent(id);
                while let Some(id) = parent {
                    let Some(entry) = self.home_store().entries.get(&id) else {
                        break;
                    };
                    path.push(entry.name.clone());
                    parent = self.home_store().parent(&id);
                }
                path.reverse();
                rows.push(path.join(" / "));
                keys.push(id.clone());
            }
        }
        rows.extend([NEW_HOME_FOLDER.into(), CANCEL_HOME.into()]);
        self.home_set_menu(HomeMenu::Destination(moving), rows, keys);
    }

    fn finish_home_folder(&mut self, name: &str) {
        let parent = self.home.folder.clone();
        let entry = crate::home::Entry {
            name: name.trim().into(),
            image: None,
            target: crate::home::Target::Folder {
                children: Vec::new(),
            },
        };
        let mut store = self.home.edit.clone().expect("Home editor");
        let result = store.add(entry, parent.as_deref());
        match result {
            Ok(id) => {
                let placed = if let Some(entry) = self.home.pending_entry.as_ref() {
                    store.add(entry.clone(), Some(&id)).map(|_| ())
                } else if let Some(HomeMenu::Destination(Some(moving))) = &self.home.menu {
                    store.move_to(moving, Some(&id))
                } else {
                    Ok(())
                };
                if let Err(error) = placed {
                    self.message = Some(error);
                    return;
                }
                self.home.edit = Some(store);
                self.home.pending_entry = None;
                if self.home.explicit_editor {
                    self.home_editor_select(&crate::home::entry_key(&id));
                } else {
                    self.finish_home_edit(true);
                }
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn finish_home_rename(&mut self, id: &str, name: &str) {
        if name.trim().is_empty() {
            self.message = Some("A Home entry needs a name.".into());
            return;
        }
        if let Some(entry) = self
            .home
            .edit
            .as_mut()
            .and_then(|store| store.entries.get_mut(id))
        {
            entry.name = name.trim().into();
        }
        self.home_entry_actions(crate::home::entry_key(id));
    }

    fn save_home_store(&mut self, store: crate::home::Store) -> bool {
        if let Err(error) = store.validate() {
            self.message = Some(error);
            return false;
        }
        let mut settings = self.settings.clone();
        settings.home = store;
        match settings.save(&self.settings_path) {
            Ok(outcome) => {
                self.settings = settings;
                self.explore_save_warning(outcome);
                true
            }
            Err(error) => {
                self.message = Some(error.to_string());
                false
            }
        }
    }

    fn finish_home_edit(&mut self, save: bool) {
        if save {
            let Some(store) = self.home.edit.clone() else {
                return;
            };
            if !self.save_home_store(store) {
                return;
            }
        }
        self.home.edit = None;
        self.home.explicit_editor = false;
        let origin = self.home.edit_origin.take();
        if let Some(origin) = &origin {
            self.home.folder = origin.folder.clone();
        }
        self.home.menu = None;
        self.home.pending_entry = None;
        self.screen = Screen::Browse;
        if let Some((Screen::Scripts, list)) = self.home.edit_return.take() {
            self.screen = Screen::Scripts;
            self.show_scripts_directory(self.scripts_directory.clone(), None);
            self.menu_list = list;
        }
        let selected = origin
            .as_ref()
            .map(|origin| origin.key.clone())
            .or_else(|| self.selected_home_key().map(str::to_string));
        self.rebuild_home_rows();
        self.category_list = ListState::new(self.home.rows.len(), self.geometry.visible);
        if let Some(index) = selected
            .as_ref()
            .and_then(|selected| self.home.rows.iter().position(|key| key == selected))
        {
            self.category_list.select(index);
        }
        self.resolve_view();
        self.apply_geometry();
        self.touch_selection();
    }

    fn accept_home_menu(&mut self) -> bool {
        let Some(mode) = self.home.menu.clone() else {
            return false;
        };
        let at = self.menu_list.selected();
        let choice = self.menu.get(at).cloned().unwrap_or_default();
        let destination = matches!(mode, HomeMenu::Destination(_)) && at < self.home.menu_keys.len();
        if !destination && choice == SAVE_HOME {
            self.finish_home_edit(true);
            return true;
        }
        if !destination && choice == CANCEL_HOME {
            self.finish_home_edit(false);
            return true;
        }
        if !destination && choice == NEW_HOME_FOLDER {
            self.begin_home_edit();
            self.open_name_keyboard(NamePurpose::HomeFolder, String::new());
            return true;
        }
        match mode {
            HomeMenu::Actions => match choice.as_str() {
                EDIT_HOME | EDIT_HOME_FOLDER => self.home_editor(),
                "Manage Entry" => {
                    self.begin_home_edit();
                    if let Some(key) = self.selected_home_key().map(str::to_string) {
                        self.home_entry_actions(key);
                    }
                }
                ADD_HOME => {
                    self.home.pending_entry = self.home_pin_target();
                    self.home_destinations(None);
                }
                _ => return false,
            },
            HomeMenu::Editor => {
                if let Some(key) = self.home.menu_keys.get(at).cloned() {
                    self.home_entry_actions(key);
                }
            }
            HomeMenu::Destination(moving) => {
                if let Some(parent) = self.home.menu_keys.get(at).cloned() {
                    let parent = (!parent.is_empty()).then_some(parent);
                    let immediate = moving.is_none() || !self.home.explicit_editor;
                    let mut store = self.home.edit.clone().expect("Home editor");
                    let result = if let Some(id) = moving {
                        store.move_to(&id, parent.as_deref())
                    } else if let Some(entry) = self.home.pending_entry.clone() {
                        store.add(entry, parent.as_deref()).map(|_| ())
                    } else {
                        Err("No shortcut was selected.".into())
                    };
                    match result {
                        Ok(()) => {
                            if immediate {
                                if self.save_home_store(store) {
                                    self.finish_home_edit(false);
                                }
                            } else {
                                self.home.edit = Some(store);
                                self.home.pending_entry = None;
                                self.home_editor();
                            }
                        }
                        Err(error) => self.message = Some(error),
                    }
                }
            }
            HomeMenu::Entry(key) => {
                let id = key.strip_prefix("entry:").map(str::to_string);
                match choice.as_str() {
                    "Move Up" | "Move Down" => {
                        let mut keys = self.home_store().rows(
                            self.home.folder.as_deref(),
                            &self.home_editor_categories(),
                            true,
                        );
                        if let Some(at) = keys.iter().position(|held| held == &key) {
                            let to = if choice == "Move Up" {
                                at.saturating_sub(1)
                            } else {
                                (at + 1).min(keys.len().saturating_sub(1))
                            };
                            keys.swap(at, to);
                            self.home
                                .edit
                                .as_mut()
                                .expect("Home editor")
                                .reorder(self.home.folder.as_deref(), &keys);
                        }
                        self.home_editor_select(&key);
                    }
                    "Hide on Home"
                    | "Show on Home"
                    | "Hide on Home Folder"
                    | "Show on Home Folder" => {
                        let store = self.home.edit.as_mut().expect("Home editor");
                        if !store.hidden.remove(&key) {
                            store.hidden.insert(key.clone());
                        }
                        self.home_editor_select(&key);
                    }
                    "Rename" => {
                        if let Some(id) = id {
                            let name = self.home_store().entries[&id].name.clone();
                            self.open_name_keyboard(NamePurpose::HomeRename(id), name);
                        }
                    }
                    "Move to Folder" => self.home_destinations(id),
                    "Remove" => {
                        if let Some(id) = id {
                            let mut saved = self.settings.home.clone();
                            saved.remove(&id);
                            if !self.save_home_store(saved) {
                                return true;
                            }
                            self.home.edit.as_mut().expect("Home editor").remove(&id);
                        }
                        if self.home.explicit_editor {
                            self.home_editor();
                        } else {
                            self.finish_home_edit(false);
                        }
                    }
                    "Edit Folder" => {
                        self.home.folder = id;
                        self.home_editor();
                    }
                    "Change Folder Image" => {
                        self.home.image = id;
                        self.open_home_image_picker();
                    }
                    "Remove Folder Image" => {
                        if let Some(id) = id {
                            if let Some(entry) = self
                                .home
                                .edit
                                .as_mut()
                                .and_then(|store| store.entries.get_mut(&id))
                            {
                                entry.image = None;
                            }
                        }
                        self.home_entry_actions(key);
                    }
                    _ => {}
                }
            }
        }
        true
    }

    fn open_home_image_picker(&mut self) {
        let Some(dir) = self.logo_dir.as_deref() else {
            self.message = Some("The logos folder is unavailable.".into());
            return;
        };
        match crate::category_images::choices(dir) {
            Ok(choices) if !choices.is_empty() => {
                self.menu = choices.iter().map(|choice| choice.label.clone()).collect();
                self.category_image_choices = choices;
                self.category_image_target = None;
                self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
                self.screen = Screen::CategoryImage;
                self.apply_geometry();
                self.touch_selection();
            }
            Ok(_) => {
                self.message = Some("No PNG or JPG images were found in the logos folder.".into())
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn handle_home_input(&mut self, action: Action) -> Option<Option<Outcome>> {
        if action == Action::Quit
            && self.home.pending_open.is_some()
            && self.screen == Screen::Browse
        {
            self.home.pending_open = None;
            self.opening = None;
            if let Some(origin) = self.home.origin.clone() {
                self.restore_home(origin);
                return Some(None);
            }
        }
        if self.screen == Screen::CategoryImage
            && self.home.image.is_some()
            && matches!(action, Action::Accept | Action::Quit)
        {
            if action == Action::Accept {
                let choice = self
                    .category_image_choices
                    .get(self.menu_list.selected())?
                    .clone();
                if let Err(error) =
                    crate::covers::load_scaled(&choice.path, 1, [0, 0, 0], &mut Default::default())
                {
                    self.message = Some(error.to_string());
                    return Some(None);
                }
                if let Some(entry) = self
                    .home
                    .edit
                    .as_mut()?
                    .entries
                    .get_mut(self.home.image.as_ref()?)
                {
                    entry.image = Some(choice.path);
                }
            }
            let id = self.home.image.take()?;
            self.category_image_choices.clear();
            self.home_entry_actions(crate::home::entry_key(&id));
            return Some(None);
        }
        if self.screen == Screen::NameKeyboard
            && matches!(
                self.name_keyboard_purpose,
                NamePurpose::HomeFolder | NamePurpose::HomeRename(_)
            )
        {
            let key = name_keyboard::keys(self.name_keyboard_page, true)
                .get(self.name_keyboard_list.selected())
                .cloned();
            if action == Action::Quit
                || (action == Action::Accept && matches!(key, Some(NameKey::Cancel)))
            {
                match self.home.menu.clone() {
                    Some(HomeMenu::Destination(moving)) => self.home_destinations(moving),
                    Some(HomeMenu::Entry(key)) => self.home_entry_actions(key),
                    Some(HomeMenu::Editor) => self.home_editor(),
                    _ => self.finish_home_edit(false),
                }
                return Some(None);
            }
            if action == Action::Accept && matches!(key, Some(NameKey::Save)) {
                self.finish_name_keyboard();
                return Some(None);
            }
        }
        if self.screen == Screen::Scripts && action == Action::Context {
            if let Some(entry) = self.home_pin_target() {
                self.home.pending_entry = Some(entry);
                self.home_destinations(None);
            }
            return Some(None);
        }
        if self.screen == Screen::Context && action == Action::Accept {
            if self.home.menu.is_some() {
                if self.accept_home_menu() {
                    return Some(None);
                }
            } else if self
                .menu
                .get(self.menu_list.selected())
                .is_some_and(|choice| choice == ADD_HOME)
            {
                self.home.pending_entry = self.home_pin_target();
                self.home_destinations(None);
                return Some(None);
            }
        }
        if self.screen == Screen::Context && self.home.menu.is_some() && action == Action::Quit {
            match self.home.menu.clone()? {
                HomeMenu::Entry(_) | HomeMenu::Destination(_) => {
                    if self.home.explicit_editor {
                        self.home_editor();
                    } else {
                        self.finish_home_edit(false);
                    }
                }
                HomeMenu::Editor => {
                    if let Some(folder) = self.home.folder.clone() {
                        self.home.folder = self.home_store().parent(&folder);
                        self.home_editor();
                    } else {
                        self.finish_home_edit(false);
                    }
                }
                HomeMenu::Actions => {
                    if !self.save_context_view_if_changed() {
                        return Some(None);
                    }
                    self.home.menu = None;
                    self.screen = Screen::Browse;
                    self.apply_geometry();
                    self.touch_selection();
                }
            }
            return Some(None);
        }
        if self.screen == Screen::Browse && self.browsing == Browsing::Categories {
            if action == Action::Accept {
                return Some(self.open_home_entry());
            }
            if action == Action::Quit {
                if let Some(folder) = self.home.folder.clone() {
                    let parent = self.settings.home.parent(&folder);
                    self.restore_home(crate::home::Resume {
                        folder: parent,
                        key: crate::home::entry_key(&folder),
                        anchor: None,
                    });
                    return Some(None);
                }
            }
        }
        if action == Action::Quit && self.screen == Screen::Browse {
            if let Some(origin) = self.home.origin.clone() {
                let at_entry = self.home.direct_game
                    || self.in_misterzine_browser()
                    || origin.anchor.as_ref().is_some_and(|anchor| {
                        let current = self.position_without_home();
                        current.system == anchor.system
                            && current.trail.len() == anchor.trail.len()
                            && current.places() == anchor.places()
                            && self.browsing
                                == if anchor.system.is_empty() {
                                    Browsing::Systems
                                } else {
                                    Browsing::Games
                                }
                    });
                if at_entry {
                    self.restore_home(origin);
                    return Some(None);
                }
            }
        }
        if action == Action::Quit && self.screen == Screen::Scripts {
            if let Some(origin) = self.home.origin.clone() {
                let target = origin
                    .key
                    .strip_prefix("entry:")
                    .and_then(|id| self.settings.home.entries.get(id));
                let at_entry = target.is_some_and(|entry| match &entry.target {
                    crate::home::Target::Script { path } => path
                        .parent()
                        .is_some_and(|parent| parent == self.scripts_directory),
                    crate::home::Target::Category { category } if category == SCRIPTS_CATEGORY => {
                        self.scripts_browser
                            .as_ref()
                            .is_some_and(|browser| browser.root() == self.scripts_directory)
                    }
                    _ => false,
                });
                if at_entry {
                    self.restore_home(origin);
                    return Some(None);
                }
            }
        }
        None
    }
}
