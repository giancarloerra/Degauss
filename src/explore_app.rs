// Explore shares the ordinary renderer and exact-target launch path.

#[derive(Default)]
struct ExploreBrowser {
    active: bool,
    catalogue: Option<crate::explore::Catalogue>,
    job: Option<crate::explore::Job>,
    query: crate::explore::Query,
    matches: Vec<usize>,
    origin: Option<ExploreOrigin>,
    menu: ExploreMenu,
    choices: Vec<crate::explore::Choice>,
    collection_ids: Vec<String>,
    logos: std::collections::BTreeMap<String, Option<PathBuf>>,
    restore_selection: Option<(Option<String>, usize)>,
    pending_resume: Option<crate::state::State>,
    pivots: Vec<(crate::explore::Query, usize)>,
    selection: usize,
    revision: u64,
}

struct ExploreOrigin {
    browsing: Browsing,
    list: ListState,
    filter: String,
    filters: GameFilters,
    last_played: bool,
    layout: Layout,
    position: crate::state::State,
}

#[derive(Default, Clone)]
enum ExploreMenu {
    #[default]
    Actions,
    Fields,
    Values(crate::explore::Facet),
    Collections,
    Collection(String),
}

const SAVE_COLLECTION: &str = "Save Collection";
const OPEN_COLLECTION: &str = "Saved Collections";
const EXPLORE_COVERAGE: &str = "Indexed Coverage";
const MORE_DEVELOPER: &str = "More by Developer";
const MORE_PUBLISHER: &str = "More by Publisher";
const SAME_GENRE: &str = "Same Genre";

impl App {
    fn name_keyboard_controls(&self) -> bool {
        matches!(
            self.name_keyboard_purpose,
            NamePurpose::SaveView
                | NamePurpose::RenameView(_)
                | NamePurpose::SaveCollection
                | NamePurpose::RenameCollection(_)
                | NamePurpose::HomeFolder
                | NamePurpose::HomeRename(_)
        )
    }
    fn browse_row(&self, index: usize) -> Option<&browse::Row> {
        if self.explore.active {
            let at = *self.explore.matches.get(index)?;
            return self
                .explore
                .catalogue
                .as_ref()?
                .entries
                .get(at)
                .map(|entry| &entry.row);
        }
        self.here.get(index)
    }

    fn browse_count(&self) -> usize {
        if self.explore.active {
            self.explore.matches.len()
        } else {
            self.here.len()
        }
    }

    fn browse_rows(&self) -> impl Iterator<Item = &browse::Row> {
        (0..self.browse_count()).filter_map(|index| self.browse_row(index))
    }

    fn explore_selected(&self) -> Option<&crate::explore::Entry> {
        if !self.explore.active {
            return None;
        }
        let at = *self.explore.matches.get(self.game_list.selected())?;
        self.explore.catalogue.as_ref()?.entries.get(at)
    }

    fn explore_logo(&self, id: &str) -> Option<PathBuf> {
        self.explore.logos.get(id).cloned().flatten()
    }

    fn load_explore_logo(&self, id: &str) -> Option<PathBuf> {
        let system = self.all_systems.iter().find(|system| system.def.id == id)?;
        self.logo_dir
            .as_deref()
            .and_then(|dir| crate::category_images::system_image(dir, id))
            .or_else(|| {
                let category = display_category(
                    system,
                    self.settings.separate_handheld_category.unwrap_or(false),
                );
                let dir = self.logo_dir.as_deref()?;
                (category == "Arcade" && crate::category_images::has_override(dir, category))
                    .then(|| self.category_logo(category))
                    .flatten()
            })
            .or_else(|| system.logo())
    }

    fn open_explore(&mut self, query: Option<crate::explore::Query>) {
        if !self.explore.active {
            self.explore.origin = Some(ExploreOrigin {
                browsing: self.browsing,
                list: self.game_list.clone(),
                filter: self.filter.clone(),
                filters: self.game_filters.clone(),
                last_played: self.last_played_open,
                layout: self.layout,
                position: self.position(),
            });
        }
        self.explore.active = true;
        self.explore.logos = self
            .all_systems
            .iter()
            .map(|system| {
                (
                    system.def.id.clone(),
                    self.load_explore_logo(&system.def.id),
                )
            })
            .collect();
        self.last_played_open = false;
        self.browsing = Browsing::Games;
        self.screen = Screen::Browse;
        self.explore.menu = ExploreMenu::Actions;
        if let Some(query) = query {
            self.explore.query = query;
            self.explore.selection = 0;
        }
        self.filter = self.explore.query.title.clone();
        self.game_filters.clear();
        self.message = None;
        self.resolve_view();
        self.filter_explore(true);
        self.start_explore();
    }

    fn start_explore(&mut self) {
        if self.explore.job.is_some() {
            return;
        }
        let sources = self
            .all_systems
            .iter()
            .filter(|system| {
                let category = display_category(
                    system,
                    self.settings.separate_handheld_category.unwrap_or(false),
                );
                !is_favorites(system.category())
                    && (self.show_hidden || !self.settings.hidden.contains(&system.def.id))
                    && self.categories.iter().any(|(name, _)| name == category)
            })
            .map(|system| crate::explore::Source {
                id: system.def.id.clone(),
                name: system.name().to_string(),
                category: display_category(
                    system,
                    self.settings.separate_handheld_category.unwrap_or(false),
                )
                .into(),
                config: system.to_config(),
                pack: crate::artwork_pack::selected_root(
                    &self.effective_artwork_pack_roots,
                    &system.def.id,
                )
                .map(PathBuf::from),
                error: self.source_problem(&system.def.id).map(str::to_string),
            })
            .collect();
        let mut request = crate::explore::Request {
            cache_dir: self.cache_dir.clone(),
            sources,
            hidden: if self.show_hidden {
                HashSet::new()
            } else {
                self.settings.hidden_paths.iter().cloned().collect()
            },
            language: crate::artwork_pack::normalized_language(
                self.scraper_settings.language.as_deref(),
            ),
            names: self.game_name_display,
            previous: None,
            signatures: Default::default(),
        };
        request.signatures = request.source_signatures();
        if self
            .explore
            .catalogue
            .as_ref()
            .is_some_and(|catalogue| catalogue.signatures == request.signatures)
        {
            return;
        }
        self.explore.restore_selection = Some((
            self.explore_selected().map(crate::explore::Entry::key),
            self.game_list.selected(),
        ));
        request.previous = self.explore.catalogue.take();
        self.explore.matches.clear();
        self.game_list = ListState::new(0, self.geometry.visible);
        self.apply_geometry();
        match crate::explore::Job::start(request) {
            Ok(job) => {
                self.explore.job = Some(job);
                self.message = Some("Reading indexed library...".into());
            }
            Err(error) => {
                self.message = Some(error.to_string());
            }
        }
        self.dirty = true;
    }

    fn poll_explore(&mut self) {
        if self.explore.pending_resume.is_some()
            && self.source_resolution.is_none()
            && self.source_job.is_none()
            && self.provider_job.is_none()
            && self.build.is_none()
            && self.pending_restore.is_none()
        {
            let saved = self
                .explore
                .pending_resume
                .take()
                .expect("pending Explore return");
            if let Some(resume) = &saved.explore {
                self.open_explore(Some(resume.query.clone()));
                self.explore.pivots = resume.pivots.clone();
                self.explore.restore_selection = Some((saved.selected_row.clone(), saved.selected));
                if self.explore.job.is_none() {
                    self.restore_explore_selection();
                    self.touch_selection();
                }
            }
        }
        loop {
            let Some(event) = self
                .explore
                .job
                .as_ref()
                .and_then(crate::explore::Job::try_recv)
            else {
                return;
            };
            match event {
                crate::explore::Event::Progress(name) => {
                    self.message = Some(format!("Reading indexed library\n{name}"))
                }
                crate::explore::Event::Ready(mut catalogue) => {
                    for entry in &mut catalogue.entries {
                        entry.row.favorite = row_target(&entry.row)
                            .is_some_and(|target| self.favorites.holds(&target));
                    }
                    let summary = format!(
                        "{} games indexed for Explore; {} system/folder notices",
                        catalogue.entries.len(),
                        catalogue.omitted.len()
                    );
                    crate::note(&summary);
                    self.explore.catalogue = Some(catalogue);
                    self.explore.job = None;
                    self.message = None;
                    self.filter_explore(true);
                    self.restore_explore_selection();
                    self.touch_selection();
                    if let Some(catalogue) = &self.explore.catalogue {
                        if !catalogue.omitted.is_empty() {
                            self.message = Some(format!("{} games available. {} indexed sources/folders were omitted.\n{}\nActions: Indexed Coverage for the complete report.", catalogue.entries.len(), catalogue.omitted.len(), catalogue.omitted[0]));
                        }
                    }
                }
                crate::explore::Event::Cancelled => {
                    self.explore.job = None;
                    self.message = None;
                    self.explore.pivots.clear();
                    self.leave_explore();
                    self.message = Some("Explore Games: reading cancelled.".into());
                }
                crate::explore::Event::Failed(error) => {
                    self.explore.job = None;
                    self.message = None;
                    self.explore.pivots.clear();
                    self.leave_explore();
                    self.message = Some(error);
                }
            }
            self.dirty = true;
        }
    }

    fn filter_explore(&mut self, preserve: bool) {
        if !preserve {
            self.explore.selection = 0;
        }
        self.explore.matches = self
            .explore
            .catalogue
            .as_ref()
            .map(|catalogue| self.explore.query.matching(catalogue))
            .unwrap_or_default();
        self.explore.revision = self.explore.revision.wrapping_add(1);
        let selected = if preserve { self.explore.selection } else { 0 };
        self.game_list = ListState::new(self.explore.matches.len(), self.geometry.visible);
        self.game_list.select(selected);
        self.apply_geometry();
        self.touch_selection();
    }

    fn restore_explore_selection(&mut self) {
        if let Some((key, fallback)) = self.explore.restore_selection.take() {
            let selected = key
                .and_then(|key| {
                    self.explore.matches.iter().position(|at| {
                        self.explore
                            .catalogue
                            .as_ref()
                            .is_some_and(|catalogue| catalogue.entries[*at].key() == key)
                    })
                })
                .unwrap_or(fallback);
            self.game_list.select(selected);
            self.explore.selection = self.game_list.selected();
        }
    }

    fn leave_explore(&mut self) {
        if let Some((query, selected)) = self.explore.pivots.pop() {
            self.explore.query = query;
            self.explore.selection = selected;
            self.filter = self.explore.query.title.clone();
            self.filter_explore(true);
            self.screen = Screen::Browse;
            self.resolve_view();
            self.apply_geometry();
            self.touch_selection();
            return;
        }
        self.explore.selection = self.game_list.selected();
        self.explore.active = false;
        if let Some(origin) = self.explore.origin.take() {
            self.browsing = origin.browsing;
            self.game_list = origin.list;
            self.filter = origin.filter;
            self.game_filters = origin.filters;
            self.last_played_open = origin.last_played;
            self.layout = origin.layout;
        }
        self.screen = Screen::Browse;
        self.message = None;
        self.resolve_view();
        self.apply_geometry();
        self.touch_selection();
    }

    fn explore_back(&mut self) -> bool {
        if self.screen == Screen::Browse {
            self.leave_explore();
            return true;
        }
        if self.screen != Screen::Context {
            return false;
        }
        if !self.save_context_view_if_changed() {
            return true;
        }
        match self.explore.menu.clone() {
            ExploreMenu::Values(_) => self.explore_fields(),
            ExploreMenu::Collection(_) => self.explore_collections(),
            ExploreMenu::Fields | ExploreMenu::Collections => self.explore_actions(),
            ExploreMenu::Actions => {
                self.screen = Screen::Browse;
                self.resolve_view();
                self.apply_geometry();
                self.touch_selection();
            }
        }
        true
    }

    fn explore_menu(&mut self, mode: ExploreMenu, rows: Vec<String>) {
        self.explore.menu = mode;
        self.context_page = None;
        self.menu = rows;
        if matches!(
            self.explore.menu,
            ExploreMenu::Actions | ExploreMenu::Collection(_)
        ) && self.home_pin_target().is_some()
        {
            self.menu.push(ADD_HOME.into());
        }
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::Context;
        self.apply_geometry();
        self.dirty = true;
    }

    fn explore_actions(&mut self) {
        let mut actions = vec![
            FILTER_GAMES.into(),
            SEARCH.into(),
            CLEAR_FILTERS.into(),
            SAVE_COLLECTION.into(),
            OPEN_COLLECTION.into(),
            EXPLORE_COVERAGE.into(),
        ];
        if !self.filter.is_empty() {
            actions.push(CLEAR_SEARCH.into());
        }
        if self.explore_selected().is_some() {
            actions.push(GAME_INFORMATION.into());
        }
        actions.push(CHANGE_VIEW.into());
        actions.extend(self.custom_view_actions());
        if self.selected_game_launch_core_target().is_some() {
            actions.push(GAME_LAUNCH_CORE.into());
        }
        if self.selected_game().is_some() {
            actions.push(
                if self
                    .selected_game()
                    .is_some_and(|target| self.favorites.holds(&target))
                {
                    REMOVE_FAVORITE.into()
                } else {
                    ADD_FAVORITE.into()
                },
            );
        }
        actions.extend(self.explore_pivots());
        self.context_actions = actions.clone();
        self.explore_menu(ExploreMenu::Actions, actions);
    }

    fn explore_menu_heading(&self) -> String {
        match &self.explore.menu {
            ExploreMenu::Actions => "Actions".into(),
            ExploreMenu::Fields => "Explore / Filters".into(),
            ExploreMenu::Values(facet) => format!("Explore / {}", facet.label()),
            ExploreMenu::Collections => "Saved Collections".into(),
            ExploreMenu::Collection(_) => "Collection Actions".into(),
        }
    }

    fn explore_fields(&mut self) {
        self.explore_menu(
            ExploreMenu::Fields,
            crate::explore::Facet::ALL
                .into_iter()
                .map(|facet| facet.label().into())
                .collect(),
        );
    }

    fn explore_collections(&mut self) {
        let mut collections: Vec<_> = self.settings.collections.iter().collect();
        collections.sort_by_cached_key(|(_, collection)| collection.name.to_lowercase());
        let rows = collections
            .iter()
            .map(|(_, collection)| collection.name.clone())
            .collect();
        self.explore.collection_ids = collections.iter().map(|(id, _)| (*id).clone()).collect();
        self.explore_menu(ExploreMenu::Collections, rows);
    }

    fn explore_menu_value(&self, at: usize) -> String {
        match &self.explore.menu {
            ExploreMenu::Fields => crate::explore::Facet::ALL
                .get(at)
                .map(|facet| self.explore.query.label(*facet))
                .unwrap_or_default(),
            ExploreMenu::Values(_) => self
                .explore
                .choices
                .get(at)
                .map(|choice| choice.count.to_string())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn accept_explore_menu(&mut self) -> bool {
        let at = self.menu_list.selected();
        let choice = self.menu.get(at).cloned().unwrap_or_default();
        match self.explore.menu.clone() {
            ExploreMenu::Fields => {
                let Some(facet) = crate::explore::Facet::ALL.get(at).copied() else {
                    return true;
                };
                self.explore.choices = self
                    .explore
                    .catalogue
                    .as_ref()
                    .map(|catalogue| self.explore.query.choices(catalogue, facet))
                    .unwrap_or_default();
                let rows = self
                    .explore
                    .choices
                    .iter()
                    .map(|choice| {
                        if facet == crate::explore::Facet::System {
                            choice
                                .criterion
                                .as_ref()
                                .and_then(|criterion| {
                                    self.all_systems
                                        .iter()
                                        .find(|system| system.def.id == criterion.label())
                                })
                                .map(|system| system.name().to_string())
                                .unwrap_or_else(|| choice.label().into())
                        } else {
                            choice.label().into()
                        }
                    })
                    .collect();
                self.explore_menu(ExploreMenu::Values(facet), rows);
            }
            ExploreMenu::Values(facet) => {
                if let Some(choice) = self.explore.choices.get(at) {
                    self.explore.query.choose(facet, choice.criterion.clone());
                    self.filter_explore(false);
                }
                self.explore_fields();
            }
            ExploreMenu::Collections => {
                if let Some(id) = self.explore.collection_ids.get(at).cloned() {
                    self.explore_menu(
                        ExploreMenu::Collection(id),
                        vec!["Open".into(), "Rename".into(), "Delete".into()],
                    );
                }
            }
            ExploreMenu::Collection(id) => match choice.as_str() {
                "Open" => {
                    if let Some(collection) = self.settings.collections.get(&id) {
                        self.open_explore(Some(collection.query.clone()));
                    }
                }
                "Rename" => {
                    if let Some(collection) = self.settings.collections.get(&id) {
                        self.open_name_keyboard(
                            NamePurpose::RenameCollection(id),
                            collection.name.clone(),
                        );
                    }
                }
                "Delete" => {
                    let mut settings = self.settings.clone();
                    settings.collections.remove(&id);
                    match settings.save(&self.settings_path) {
                        Ok(outcome) => {
                            self.settings = settings;
                            self.explore_collections();
                            self.explore_save_warning(outcome);
                        }
                        Err(error) => self.message = Some(error.to_string()),
                    }
                }
                _ => {}
            },
            ExploreMenu::Actions => match choice.as_str() {
                FILTER_GAMES => self.explore_fields(),
                SAVE_COLLECTION => {
                    self.open_name_keyboard(NamePurpose::SaveCollection, String::new())
                }
                OPEN_COLLECTION => self.explore_collections(),
                EXPLORE_COVERAGE => {
                    self.screen = Screen::Browse;
                    self.message = self.explore.catalogue.as_ref().map(|catalogue| {
                        let mut text = format!(
                            "{} indexed games\nOnly indexed, visible systems are included.\n",
                            catalogue.entries.len()
                        );
                        if catalogue.omitted.is_empty() {
                            text.push_str("No omitted indexed sources.");
                        } else {
                            text.push_str(&catalogue.omitted.join("\n"));
                        }
                        text
                    });
                    self.apply_geometry();
                    self.touch_selection();
                }
                MORE_DEVELOPER | MORE_PUBLISHER | SAME_GENRE => self.explore_pivot(&choice),
                _ => return false,
            },
        }
        self.dirty = true;
        true
    }

    fn explore_save_warning(&mut self, outcome: SaveOutcome) {
        if let SaveOutcome::InstalledWithWarning(warning) = outcome {
            self.message = Some(warning.to_string());
        }
    }

    fn save_explore_collection(&mut self, id: Option<&str>, draft: &str) {
        let name = match crate::explore::collection_name(draft, &self.settings.collections, id) {
            Ok(name) => name,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };
        let mut settings = self.settings.clone();
        if let Some(id) = id {
            let Some(collection) = settings.collections.get_mut(id) else {
                self.message = Some("Collection no longer exists.".into());
                return;
            };
            collection.name = name;
        } else {
            let (id, next) =
                crate::explore::next_collection_id(&settings.collections, settings.next_collection);
            settings.next_collection = next;
            settings.collections.insert(
                id,
                crate::explore::Collection {
                    name,
                    query: self.explore.query.clone(),
                },
            );
        }
        match settings.save(&self.settings_path) {
            Ok(outcome) => {
                self.settings = settings;
                self.explore_collections();
                self.explore_save_warning(outcome);
            }
            Err(error) => self.message = Some(error.to_string()),
        }
        self.dirty = true;
    }

    fn explore_pivots(&self) -> Vec<String> {
        let Some(row) = self.browse_row(self.game_list.selected()).filter(|row| {
            self.browsing == Browsing::Games && matches!(row.kind, browse::Kind::Play(_))
        }) else {
            return Vec::new();
        };
        [
            (GameFilterField::Developer, MORE_DEVELOPER),
            (GameFilterField::Publisher, MORE_PUBLISHER),
            (GameFilterField::Genre, SAME_GENRE),
        ]
        .into_iter()
        .filter(|(field, _)| field.value(row).is_some())
        .map(|(_, label)| label.to_string())
        .collect()
    }

    fn refresh_explore_favourites(&mut self) {
        if let Some(catalogue) = &mut self.explore.catalogue {
            for entry in &mut catalogue.entries {
                entry.row.favorite =
                    row_target(&entry.row).is_some_and(|target| self.favorites.holds(&target));
            }
        }
    }

    fn explore_pivot(&mut self, action: &str) {
        let field = match action {
            MORE_DEVELOPER => GameFilterField::Developer,
            MORE_PUBLISHER => GameFilterField::Publisher,
            _ => GameFilterField::Genre,
        };
        let Some(value) = self
            .browse_row(self.game_list.selected())
            .and_then(|row| field.value(row))
            .map(str::to_string)
        else {
            return;
        };
        let mut query = crate::explore::Query::default();
        query.fields[field.index()] = Some(crate::game_filter::Criterion::Known {
            key: value.to_lowercase(),
            label: value,
        });
        if self.explore.active {
            self.explore
                .pivots
                .push((self.explore.query.clone(), self.game_list.selected()));
        }
        self.open_explore(Some(query));
    }

    fn handle_explore_input(&mut self, action: Action) -> bool {
        if self.home.menu.is_some() {
            return false;
        }
        if self.explore.job.is_some() {
            if matches!(action, Action::Quit) {
                if let Some(job) = &self.explore.job {
                    job.cancel();
                }
            }
            self.last_input = Instant::now();
            return true;
        }
        if self.explore.active
            && self.screen == Screen::NameKeyboard
            && self.message.is_none()
            && matches!(
                self.name_keyboard_purpose,
                NamePurpose::SaveCollection | NamePurpose::RenameCollection(_)
            )
        {
            if action == Action::Quit {
                self.explore_actions();
                return true;
            }
            if action != Action::Accept {
                return false;
            }
            match name_keyboard::keys(self.name_keyboard_page, true)
                .get(self.name_keyboard_list.selected())
            {
                Some(NameKey::Save) => {
                    self.finish_name_keyboard();
                    return true;
                }
                Some(NameKey::Cancel) => {
                    self.explore_actions();
                    return true;
                }
                _ => {}
            }
        }
        false
    }
}
