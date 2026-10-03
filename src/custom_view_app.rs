// Custom templates use the ordinary browser and its selected-game workers.
use crate::custom_view::{
    Content as ViewContent, Definition as ViewDefinition, Template as ViewTemplate,
};

const MANAGE_VIEWS: &str = "Custom Views";
const FOCUS_INFORMATION: &str = "Focus Information";

#[derive(Default)]
struct ViewBrowser {
    active: Option<ViewDefinition>,
    active_id: Option<String>,
    editor: Option<ViewEditor>,
    menu: Option<ViewMenu>,
    ids: Vec<String>,
    description: Option<ReadingInformation>,
    description_key: Option<browse::Launch>,
    information_focus: bool,
    return_to_options: Option<ListState>,
}
struct ViewEditor {
    original: Option<String>,
    draft: ViewDefinition,
    control: usize,
}
#[derive(Clone)]
enum ViewMenu {
    List,
    Templates,
    Definition(String),
}

impl App {
    fn effective_view_key(&self) -> String {
        self.custom_view
            .active_id
            .as_ref()
            .map(|id| crate::custom_view::key(id))
            .unwrap_or_else(|| self.layout.label().into())
    }
    fn view_label(&self) -> String {
        self.custom_view
            .active
            .as_ref()
            .map(|view| view.name.clone())
            .unwrap_or_else(|| self.layout.shown().into())
    }
    fn global_view_label(&self) -> String {
        self.settings
            .layout
            .as_deref()
            .and_then(|key| crate::custom_view::from_key(&self.settings.view_definitions, key))
            .map(|view| view.name.clone())
            .unwrap_or_else(|| self.global_layout.shown().into())
    }
    fn view_choices(&self) -> Vec<String> {
        let mut choices = vec![
            "details",
            "tiled",
            "carousel",
            "list",
            "multi-list",
            "gallery",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let mut views = self.settings.view_definitions.iter().collect::<Vec<_>>();
        views.sort_by_cached_key(|(_, view)| view.name.to_lowercase());
        choices.extend(views.into_iter().map(|(id, _)| crate::custom_view::key(id)));
        choices
    }
    fn step_view_choice(&mut self, delta: isize, global: bool) {
        let choices = self.view_choices();
        let key = if global {
            self.settings
                .layout
                .clone()
                .unwrap_or_else(|| self.global_layout.label().into())
        } else {
            self.effective_view_key()
        };
        let at = choices.iter().position(|value| *value == key).unwrap_or(0);
        let next = choices[step(at, delta, choices.len())].clone();
        if global {
            self.global_layout = Layout::parse(&next).unwrap_or(Layout::Details);
            self.settings.layout = Some(next);
        } else if let Some(place) = self.current_view_place() {
            place.set(&mut self.settings.custom_views, next);
        }
        self.resolve_view();
    }
    fn custom_definition(&self) -> Option<&ViewDefinition> {
        if self.in_misterzine_browser() || self.layout_override.is_some() {
            return None;
        }
        self.custom_view
            .editor
            .as_ref()
            .map(|editor| &editor.draft)
            .or(self.custom_view.active.as_ref())
    }
    fn custom_rectangles(&self, geometry: Geometry) -> Vec<crate::custom_view::Rect> {
        let Some(view) = self.custom_definition() else {
            return Vec::new();
        };
        let width = self.width as f32 - geometry.inset_x * 2.0;
        let height = self.height as f32
            - geometry.inset_y * 2.0
            - if self.chrome_here() {
                geometry.chrome
            } else {
                0.0
            }
            - if self.bar_here() { geometry.bar } else { 0.0 };
        view.rectangles(width.max(1.0), height.max(1.0), geometry.pad / 2.0)
    }
    fn apply_custom_geometry(&mut self, geometry: &mut Geometry) {
        let custom = self.screen == Screen::Browse && self.custom_definition().is_some();
        self.ui.set_custom_view(custom);
        if !custom {
            return;
        }
        let view = self.custom_definition().expect("custom view");
        let rectangles = self.custom_rectangles(*geometry);
        let list = view
            .panels
            .iter()
            .position(|content| *content == ViewContent::List)
            .expect("single browser list");
        geometry.visible = (rectangles[list].height / geometry.row_height)
            .floor()
            .max(1.0) as usize;
        geometry.stride = 1;
        let mut art_width: f32 = 0.0;
        let mut art_height: f32 = 0.0;
        for (rect, content) in rectangles.iter().zip(&view.panels) {
            if *content == ViewContent::Artwork {
                art_width = art_width.max(rect.width);
                art_height = art_height.max(rect.height);
            }
        }
        geometry.art_width = art_width;
        geometry.art_height = art_height;
        self.ui.set_custom_panels(ModelRc::new(VecModel::from(
            rectangles
                .iter()
                .zip(&view.panels)
                .map(|(rect, content)| crate::CustomPanel {
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                    content: content.index(),
                })
                .collect::<Vec<_>>(),
        )));
        self.ui.set_custom_edit_panel(
            self.custom_view
                .editor
                .as_ref()
                .and_then(|editor| editor.control.checked_sub(1))
                .filter(|panel| *panel < view.panels.len())
                .map_or(-1, |panel| panel as i32),
        );
        self.ui
            .set_custom_editor_label(self.custom_editor_label().into());
        self.ui
            .set_custom_information_focus(self.custom_view.information_focus);
    }
    fn view_controls(&self) -> Vec<String> {
        let Some(editor) = &self.custom_view.editor else {
            return Vec::new();
        };
        let mut controls = vec![format!("Template: {}", editor.draft.template.label())];
        controls.extend(
            editor
                .draft
                .panels
                .iter()
                .enumerate()
                .map(|(at, content)| format!("Panel {}: {}", at + 1, content.label())),
        );
        if editor.draft.template != ViewTemplate::Stacked {
            controls.push(format!("Column divider: {}%", editor.draft.columns));
        }
        if editor.draft.template != ViewTemplate::Columns {
            controls.push(format!("Row divider: {}%", editor.draft.rows));
        }
        controls
    }
    fn custom_editor_label(&self) -> String {
        self.custom_view
            .editor
            .as_ref()
            .and_then(|editor| self.view_controls().get(editor.control).cloned())
            .unwrap_or_default()
    }
    fn custom_view_actions(&self) -> Vec<String> {
        let mut actions = vec![MANAGE_VIEWS.into()];
        if self
            .custom_definition()
            .is_some_and(|view| view.has(ViewContent::FullInformation))
            && self.custom_selected_game().is_some()
        {
            actions.push(FOCUS_INFORMATION.into());
        }
        actions
    }
    fn show_view_menu(&mut self, menu: ViewMenu) {
        self.home.menu = None;
        self.custom_view.information_focus = false;
        self.menu = match &menu {
            ViewMenu::List => {
                let mut views = self.settings.view_definitions.iter().collect::<Vec<_>>();
                views.sort_by_cached_key(|(_, view)| view.name.to_lowercase());
                self.custom_view.ids = views.iter().map(|(id, _)| (*id).clone()).collect();
                let mut rows = vec!["Create View".into()];
                rows.extend(views.into_iter().map(|(_, view)| view.name.clone()));
                rows
            }
            ViewMenu::Templates => ViewTemplate::ALL
                .map(|template| template.label().into())
                .to_vec(),
            ViewMenu::Definition(_) => [
                "Use Here",
                "Use Globally",
                "Edit",
                "Save As",
                "Rename",
                "Delete",
            ]
            .map(str::to_string)
            .to_vec(),
        };
        self.custom_view.menu = Some(menu);
        self.screen = Screen::Context;
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.apply_geometry();
        self.dirty = true;
    }
    fn leave_view_menu(&mut self) {
        self.custom_view.menu = None;
        self.screen = if let Some(list) = self.custom_view.return_to_options.take() {
            self.menu_list = list;
            Screen::Options
        } else {
            Screen::Browse
        };
        self.resolve_view();
        self.apply_geometry();
        self.touch_selection();
    }
    fn open_view_editor(&mut self, original: Option<String>, draft: ViewDefinition) {
        self.custom_view.menu = None;
        self.custom_view.information_focus = false;
        self.custom_view.editor = Some(ViewEditor {
            original,
            draft,
            control: 0,
        });
        self.screen = Screen::Browse;
        self.layout = Layout::Details;
        self.apply_geometry();
        self.touch_selection();
    }
    fn finish_custom_view(&mut self, name: &str) {
        let Some(editor) = &self.custom_view.editor else {
            return;
        };
        let mut view = editor.draft.clone();
        view.name = name.trim().into();
        if let Err(error) = view.validate() {
            self.message = Some(error);
            self.dirty = true;
            return;
        }
        let Some(id) = editor
            .original
            .clone()
            .or_else(|| crate::custom_view::next_id(&self.settings.view_definitions))
        else {
            self.message = Some("Custom view identity space is exhausted".into());
            self.dirty = true;
            return;
        };
        let mut settings = self.settings.clone();
        settings.view_definitions.insert(id.clone(), view);
        if let Some(place) = self
            .current_view_place()
            .filter(|_| editor.original.is_none())
        {
            place.set(&mut settings.custom_views, crate::custom_view::key(&id));
        }
        if !self.save_view_settings(settings) {
            return;
        }
        self.custom_view.editor = None;
        self.leave_view_menu();
    }
    fn rename_custom_view(&mut self, id: &str, name: &str) {
        let mut candidate = self.settings.view_definitions.clone();
        let Some(view) = candidate.get_mut(id) else {
            return;
        };
        view.name = name.trim().into();
        if let Err(error) = view.validate() {
            self.message = Some(error);
            self.dirty = true;
            return;
        }
        let mut settings = self.settings.clone();
        settings.view_definitions = candidate;
        if self.save_view_settings(settings) {
            self.show_view_menu(ViewMenu::Definition(id.into()));
        }
    }
    fn delete_custom_view(&mut self, id: &str) {
        let mut settings = self.settings.clone();
        let key = crate::custom_view::key(id);
        settings.view_definitions.remove(id);
        if settings.layout.as_deref() == Some(&key) {
            settings.layout = Some("details".into());
        }
        if settings.custom_views.categories.as_deref() == Some(&key) {
            settings.custom_views.categories = None;
        }
        settings
            .custom_views
            .systems
            .retain(|_, value| *value != key);
        for places in settings.custom_views.games.values_mut() {
            places.retain(|_, value| *value != key);
        }
        settings.folder_views.retain(|_, value| *value != key);
        if self.save_view_settings(settings) {
            self.global_layout = self
                .settings
                .layout
                .as_deref()
                .and_then(Layout::parse)
                .unwrap_or(Layout::Details);
            self.resolve_view();
            self.show_view_menu(ViewMenu::List);
        }
    }
    fn save_view_settings(&mut self, settings: Settings) -> bool {
        match settings.save(&self.settings_path) {
            Ok(outcome) => {
                self.settings = settings;
                self.explore_save_warning(outcome);
                true
            }
            Err(error) => {
                self.message = Some(format!("Custom view not saved: {error}"));
                self.dirty = true;
                false
            }
        }
    }
    fn adjust_custom_editor(&mut self, delta: isize) {
        let Some(editor) = &mut self.custom_view.editor else {
            return;
        };
        if editor.control == 0 {
            let at = ViewTemplate::ALL
                .iter()
                .position(|template| *template == editor.draft.template)
                .expect("template");
            let template = ViewTemplate::ALL[step(at, delta, ViewTemplate::ALL.len())];
            editor.draft.template = template;
            editor
                .draft
                .panels
                .resize(template.count(), ViewContent::ShortInformation);
            if !editor.draft.has(ViewContent::List) {
                editor.draft.panels[0] = ViewContent::List;
            }
        } else if editor.control <= editor.draft.panels.len() {
            let panel = editor.control - 1;
            let at = editor.draft.panels[panel].index() as usize;
            editor.draft.choose(
                panel,
                ViewContent::ALL[step(at, delta, ViewContent::ALL.len())],
            );
        } else {
            let first_divider = editor.draft.panels.len() + 1;
            let fraction = if editor.draft.template != ViewTemplate::Stacked
                && editor.control == first_divider
            {
                &mut editor.draft.columns
            } else {
                &mut editor.draft.rows
            };
            *fraction = (isize::from(*fraction) + delta.signum() * 5).clamp(20, 80) as u8;
        }
        self.apply_geometry();
        self.touch_selection();
    }
    fn handle_custom_view_input(&mut self, action: Action) -> bool {
        if self.message.is_some() || self.pending.is_some() {
            return false;
        }
        if self.screen == Screen::NameKeyboard
            && matches!(
                self.name_keyboard_purpose,
                NamePurpose::SaveView | NamePurpose::RenameView(_)
            )
        {
            let key = name_keyboard::keys(self.name_keyboard_page, true)
                .get(self.name_keyboard_list.selected())
                .copied();
            if action == Action::Quit || (action == Action::Accept && key == Some(NameKey::Cancel))
            {
                if self.custom_view.editor.is_some() {
                    self.screen = Screen::Browse;
                    self.apply_geometry();
                    self.touch_selection();
                } else {
                    self.show_view_menu(ViewMenu::List);
                }
                return true;
            }
            if action == Action::Accept && key == Some(NameKey::Save) {
                self.finish_name_keyboard();
                return true;
            }
            return false;
        }
        if self.screen == Screen::Browse && self.custom_view.editor.is_some() {
            let count = self.view_controls().len();
            match action {
                Action::Up | Action::Down => {
                    let editor = self.custom_view.editor.as_mut().expect("editor");
                    editor.control = step(
                        editor.control,
                        if action == Action::Up { -1 } else { 1 },
                        count,
                    );
                    self.apply_geometry();
                }
                Action::Slower => self.adjust_custom_editor(-1),
                Action::Faster | Action::Accept => self.adjust_custom_editor(1),
                Action::Context => {
                    let editor = self.custom_view.editor.as_ref().expect("editor");
                    if editor.original.is_some() {
                        let name = editor.draft.name.clone();
                        self.finish_custom_view(&name);
                    } else {
                        self.open_name_keyboard(NamePurpose::SaveView, editor.draft.name.clone());
                    }
                }
                Action::Quit => {
                    self.custom_view.editor = None;
                    self.leave_view_menu();
                }
                _ => {}
            }
            self.last_input = Instant::now();
            self.dirty = true;
            return true;
        }
        if self.screen == Screen::Browse && self.custom_view.information_focus {
            let limit = self.ui.get_custom_information_max_scroll();
            let offset = self.ui.get_custom_information_offset();
            let line = self.geometry.small_font * 2.0;
            let next = match action {
                Action::Up => offset - line,
                Action::Down => offset + line,
                Action::Slower | Action::PageUp => {
                    offset - self.geometry.art_height.max(line * 4.0)
                }
                Action::Faster | Action::PageDown => {
                    offset + self.geometry.art_height.max(line * 4.0)
                }
                Action::Home => 0.0,
                Action::End => limit,
                Action::Quit | Action::Context => {
                    self.custom_view.information_focus = false;
                    self.ui.set_custom_information_focus(false);
                    offset
                }
                _ => offset,
            };
            self.ui
                .set_custom_information_offset(next.clamp(0.0, limit.max(0.0)));
            self.last_input = Instant::now();
            self.dirty = true;
            return true;
        }
        if self.screen != Screen::Context {
            return false;
        }
        if let Some(menu) = self.custom_view.menu.clone() {
            match action {
                Action::Up => {
                    self.menu_list.move_items(-1);
                }
                Action::Down => {
                    self.menu_list.move_items(1);
                }
                Action::Home => self.menu_list.select(0),
                Action::End => self.menu_list.select(self.menu.len().saturating_sub(1)),
                Action::Quit => match menu {
                    ViewMenu::List => self.leave_view_menu(),
                    _ => self.show_view_menu(ViewMenu::List),
                },
                Action::Accept => {
                    let at = self.menu_list.selected();
                    match menu {
                        ViewMenu::List if at == 0 => self.show_view_menu(ViewMenu::Templates),
                        ViewMenu::List => {
                            if let Some(id) = self.custom_view.ids.get(at - 1).cloned() {
                                self.show_view_menu(ViewMenu::Definition(id));
                            }
                        }
                        ViewMenu::Templates => {
                            if let Some(template) = ViewTemplate::ALL.get(at) {
                                self.open_view_editor(None, ViewDefinition::new(*template));
                            }
                        }
                        ViewMenu::Definition(id) => match at {
                            0 | 1 => {
                                let key = crate::custom_view::key(&id);
                                let mut settings = self.settings.clone();
                                if at == 1 {
                                    settings.layout = Some(key);
                                } else if let Some(place) = self.current_view_place() {
                                    place.set(&mut settings.custom_views, key);
                                }
                                if self.save_view_settings(settings) {
                                    if at == 1 {
                                        self.global_layout = Layout::Details;
                                    }
                                    self.leave_view_menu();
                                }
                            }
                            2 | 3 => {
                                if let Some(view) = self.settings.view_definitions.get(&id).cloned()
                                {
                                    self.open_view_editor((at == 2).then_some(id), view);
                                }
                            }
                            4 => {
                                if let Some(view) = self.settings.view_definitions.get(&id) {
                                    self.open_name_keyboard(
                                        NamePurpose::RenameView(id),
                                        view.name.clone(),
                                    );
                                }
                            }
                            5 => self.delete_custom_view(&id),
                            _ => {}
                        },
                    }
                }
                _ => {}
            }
            self.last_input = Instant::now();
            self.dirty = true;
            return true;
        }
        if action == Action::Accept {
            let choice = self.menu.get(self.menu_list.selected()).map(String::as_str);
            if choice == Some(MANAGE_VIEWS) {
                self.show_view_menu(ViewMenu::List);
                return true;
            }
            if choice == Some(FOCUS_INFORMATION) {
                self.screen = Screen::Browse;
                self.custom_view.information_focus = true;
                self.apply_geometry();
                self.touch_selection();
                self.dirty = true;
                return true;
            }
        }
        false
    }
    fn custom_selected_game(&self) -> Option<&browse::Row> {
        self.home_selected_game().map(|(_, row)| row).or_else(|| {
            (self.browsing == Browsing::Games
                && !self.in_cores_browser()
                && !self.in_misterzine_browser())
            .then(|| self.browse_row(self.game_list.selected()))
            .flatten()
            .filter(|row| matches!(row.kind, browse::Kind::Play(_)))
        })
    }
    fn maintain_custom_information(&mut self) {
        let full = self.screen == Screen::Browse
            && self
                .custom_definition()
                .is_some_and(|view| view.has(ViewContent::FullInformation));
        if !full {
            self.custom_view.description = None;
            self.custom_view.description_key = None;
            self.ui.set_custom_full_text(SharedString::default());
            return;
        }
        let row = self.custom_selected_game().cloned();
        let key = row.as_ref().and_then(|row| match &row.kind {
            browse::Kind::Play(launch) => Some(launch.clone()),
            _ => None,
        });
        if key != self.custom_view.description_key {
            self.custom_view.description = None;
            self.custom_view.description_key = None;
            self.ui.set_custom_full_text(SharedString::default());
            self.ui.set_custom_information_offset(0.0);
        }
        let Some(mut row) = row else {
            return;
        };
        if self.custom_view.description_key.is_none()
            && self
                .settled_since
                .is_some_and(|since| since.elapsed() >= Duration::from_millis(250))
        {
            self.custom_view.description_key = key;
            let shown_name = self.game_name_display.apply(&row.name).into_owned();
            match self
                .information_request(&row)
                .and_then(crate::information_job::start)
            {
                Ok(job) => {
                    row.details.desc = "Reading full description...".into();
                    self.custom_view.description = Some(ReadingInformation {
                        row: row.clone(),
                        shown_name: shown_name.clone(),
                        job,
                        cancelling: false,
                    });
                }
                Err(error) => row.details.desc = format!("Could not read description: {error}"),
            }
            self.ui
                .set_custom_full_text(game_information_named(&row, &shown_name).into());
            self.dirty = true;
        }
        let event = self
            .custom_view
            .description
            .as_mut()
            .and_then(|reading| reading.job.try_recv());
        if let Some(event) = event {
            let mut reading = self
                .custom_view
                .description
                .take()
                .expect("description job");
            reading.row.details.desc = match event {
                crate::information_job::Event::Ready(text) => text.unwrap_or_default(),
                crate::information_job::Event::Failed(error) => {
                    format!("Could not read description: {error}")
                }
                crate::information_job::Event::Cancelled => return,
            };
            self.ui.set_custom_full_text(
                game_information_named(&reading.row, &reading.shown_name).into(),
            );
            self.dirty = true;
        }
    }
    fn update_custom_view_chrome(&self) {
        if self.screen != Screen::Browse || self.custom_definition().is_none() {
            self.ui.set_custom_short_text(SharedString::default());
            return;
        }
        let text = self
            .custom_selected_game()
            .map(|row| {
                let mut text = self.game_name_display.apply(&row.name).into_owned();
                for (label, value) in [
                    ("Genre", row.genre.as_deref().unwrap_or("")),
                    ("Publisher", row.details.publisher.as_str()),
                    ("Developer", row.details.developer.as_str()),
                    ("Released", row.details.released.as_str()),
                    ("Players", row.details.players.as_str()),
                    ("Language", row.details.lang.as_str()),
                ] {
                    if !value.is_empty() {
                        text.push_str(&format!("\n{label}: {value}"));
                    }
                }
                text
            })
            .unwrap_or_default();
        self.ui.set_custom_short_text(text.into());
    }
}
