use crate::ai_guides as guides;
const AI_RESUME: &str = "__degauss_ai_guide__";

#[derive(Clone, Copy, PartialEq, Eq)]
enum AiPage {
    Setup,
    Prompts,
    Manage,
    Edit,
    Delete,
    SaveKey,
    ClearKey,
    Working,
    Confirm,
    Guide,
    Story,
}
#[derive(Default)]
struct AiBrowser {
    page: Option<AiPage>,
    custom: Vec<(String, String)>,
    selected: usize,
    full: bool,
    guide: Option<guides::LocalGuide>,
    ready: Option<Box<guides::Ready>>,
    job: Option<guides::Job>,
    progress: String,
    error: Option<String>,
    draft_name: String,
    old_text: Option<String>,
    pending_key: String,
    from_settings: bool,
    web_only: bool,
}
impl AiBrowser {
    fn story_game(&self) -> Option<&guides::LocalGame> {
        self.guide.as_ref()?.games.get(self.selected)
    }
}
impl App {
    fn ai_related_question(&self) -> Option<String> {
        if self.kiosk_locked
            || !self.settings.ai_enabled.unwrap_or(false)
            || self.in_misterzine_browser()
        {
            return None;
        }
        let title = if let Some(e) = self.explore_selected() {
            e.guide_title()
        } else {
            let row = self
                .home_selected_game()
                .map(|(_, r)| r)
                .or_else(|| self.browse_row(self.game_list.selected()))?;
            if !matches!(row.kind, browse::Kind::Play(_)) {
                return None;
            }
            &row.name
        };
        Some(format!(
            "Explore games like {} in this library, and explain the connections.",
            guides::public_title(title)
        ))
    }
    fn web_ai_guides(&mut self) -> std::result::Result<JsonValue, WebError> {
        if self.kiosk_locked {
            return Err(web_error("Explore with AI is locked."));
        }
        let provider = self.ai_provider();
        let enabled = self.settings.ai_enabled.unwrap_or(false);
        let configured = !guides::key(self.ai_dir(), provider)
            .map_err(web_error)?
            .is_empty();
        let history_error = if enabled && self.ai.guide.is_none() && self.ai.job.is_none() {
            self.ai_read_last().err()
        } else {
            None
        };
        let saved = guides::prompts(&self.ai_user_file()).map_err(web_error)?;
        // While polling a job the browser only displays progress or confirmation.
        let guide = if enabled && self.ai.job.is_none() && self.ai.ready.is_none() {
            self.ai.guide.clone().map(|g|{
            let items:Vec<_>=g.games.into_iter().filter_map(|p|{
                let browse::Kind::Play(launch)=&p.row.kind else{return None};
                let id=self.web_register(crate::home::Target::Game{system:p.system.clone(),launch:launch.clone()},Some(p.row.clone()));
                Some(json!({"id":id,"name":p.row.name,"system":p.system,"note":p.note,"art":self.web_art(p.row.cover)}))
            }).collect();json!({"title":g.heading,"intro":g.intro,"games":items})
        })
        } else {
            None
        };
        Ok(
            json!({"enabled":enabled,"provider":provider,"configured":configured,"builtins":guides::PROMPTS,"prompts":saved,"busy":self.ai.job.is_some(),"confirm":self.ai.ready.as_ref().map(|_|self.ai_estimate()),"progress":self.ai.progress,"error":self.ai.error.as_ref().or(history_error.as_ref()),"guide":guide,"revision":self.web_revision()}),
        )
    }
    fn ai_provider(&self) -> guides::Provider {
        self.settings.ai_provider.unwrap_or_default()
    }
    fn ai_dir(&self) -> &Path {
        self.settings_path.parent().unwrap_or(Path::new("."))
    }
    fn ai_user_file(&self) -> PathBuf {
        self.settings_path.with_file_name("degauss-user.toml")
    }
    fn ai_guide_file(&self) -> PathBuf {
        self.cache_dir.join("last-ai-guide.json")
    }
    fn ai_read_last(&mut self) -> std::result::Result<(), String> {
        self.ai.guide = None;
        self.ai.guide = guides::last_guide(&self.ai_guide_file())?;
        Ok(())
    }
    fn ai_fail(&mut self, err: String) {
        self.message = Some(err);
        self.dirty = true;
    }
    fn ai_restore_story(&mut self, at: usize) {
        match guides::last_guide(&self.ai_guide_file()) {
            Ok(Some(guide)) if at < guide.games.len() => {
                self.ai.guide = Some(guide);
                self.ai_story(at);
            }
            Ok(_) => self.ai_fail("This guide is no longer available.".into()),
            Err(e) => self.ai_fail(e),
        }
    }
    fn ai_open(&mut self, page: AiPage) {
        if self.kiosk_locked {
            return;
        }
        self.ai.from_settings = page == AiPage::Setup;
        if self.ai.job.is_some() || self.ai.ready.is_some() {
            self.ai.web_only = false;
            self.ai_menu(if self.ai.job.is_some() {
                AiPage::Working
            } else {
                AiPage::Confirm
            });
            return;
        }
        self.ai_menu(page);
        if page == AiPage::Prompts {
            if let Err(e) = self.ai_read_last() {
                self.ai_fail(e);
            }
        }
    }
    fn ai_menu(&mut self, page: AiPage) {
        if matches!(page, AiPage::Prompts | AiPage::Manage) {
            match guides::prompts(&self.ai_user_file()) {
                Ok(p) => self.ai.custom = p,
                Err(e) => {
                    self.ai_fail(e);
                    return;
                }
            }
        }
        self.ai.page = Some(page);
        self.screen = Screen::Context;
        let s = |x: &str| x.to_owned();
        self.menu = match page {
            AiPage::Setup => vec![
                format!(
                    "Show on Home: {}",
                    on_off(self.settings.ai_enabled.unwrap_or(false))
                ),
                format!("Provider: {}", self.ai_provider().label()),
                "Set API key".into(),
                "Clear saved key".into(),
            ],
            AiPage::Prompts => [
                guides::LUCKY.to_string(),
                "Last guide".into(),
                format!(
                    "Scope: {}",
                    if self.ai.full {
                        "Read whole catalogue"
                    } else {
                        "Search library"
                    }
                ),
            ]
            .into_iter()
            .chain(guides::PROMPTS.iter().map(|p| s(p.0)))
            .chain(self.ai.custom.iter().map(|p| p.0.clone()))
            .chain([s("Manage custom prompts"), s("AI settings")])
            .collect(),
            AiPage::Manage => std::iter::once(s("New prompt"))
                .chain(self.ai.custom.iter().map(|p| p.0.clone()))
                .collect(),
            AiPage::Edit => vec![s("Edit prompt text"), s("Delete prompt")],
            AiPage::Delete => vec![s("Keep prompt"), s("Delete prompt")],
            AiPage::SaveKey => vec![s("Cancel"), s("Save key on this card")],
            AiPage::ClearKey => vec![s("Keep key"), s("Clear key")],
            AiPage::Working => vec![self.ai.progress.clone(), s("Cancel")],
            AiPage::Confirm => vec![s("Back"), s("Read the whole catalogue")],
            AiPage::Guide => self
                .ai
                .guide
                .as_ref()
                .map(|g| g.games.iter().map(|p| p.row.name.clone()).collect())
                .unwrap_or_default(),
            AiPage::Story => vec![],
        };
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.apply_geometry();
        self.dirty = true;
    }
    fn ai_chrome(&self) -> (String, String) {
        match self.ai.page {
            Some(AiPage::Setup) => ("Explore with AI / Setup".into(), "A Change   B Back".into()),
            Some(AiPage::SaveKey) => (
                "Save API key?".into(),
                "Anyone with card access can read it.".into(),
            ),
            Some(AiPage::ClearKey) => (
                "Clear this provider’s key?".into(),
                "A Select   B Back".into(),
            ),
            Some(AiPage::Confirm) => ("Whole catalogue?".into(), self.ai_estimate()),
            Some(AiPage::Working) => (self.ai_provider().label().into(), "B Cancel".into()),
            Some(AiPage::Guide | AiPage::Story) => (
                self.ai
                    .guide
                    .as_ref()
                    .map(|g| g.heading.clone())
                    .unwrap_or_default(),
                if self.ai.page == Some(AiPage::Story) {
                    "A Play   ←→ Next   B Back".into()
                } else {
                    "A Read   B Guides".into()
                },
            ),
            Some(AiPage::Manage | AiPage::Edit | AiPage::Delete) => {
                ("Custom prompts".into(), "A Select   B Back".into())
            }
            _ => (guides::NAME.into(), "A Select   B Back".into()),
        }
    }
    fn ai_estimate(&self) -> String {
        self.ai
            .ready
            .as_ref()
            .map(|r| {
                let requests = r.max_requests();
                let mut summary = format!(
                    "{} indexed games · {} KB of titles · up to {requests} paid request{}.",
                    r.catalogue.entries.len(),
                    r.bytes.div_ceil(1024),
                    if requests == 1 { "" } else { "s" }
                );
                if !r.catalogue.omitted.is_empty() {
                    summary.push_str(
                        " Some entries are unavailable. See Explore Games: Indexed Coverage.",
                    );
                }
                summary
            })
            .unwrap_or_default()
    }
    fn ai_story(&mut self, index: usize) {
        self.ai.selected = index;
        let Some(game) = self.ai.story_game() else {
            return;
        };
        let text = format!(
            "{} · {}\n\n{}\n\n{}",
            game.row.name,
            game.system,
            game.note,
            self.ai.guide.as_ref().unwrap().intro
        );
        self.ai.page = Some(AiPage::Story);
        self.screen = Screen::Information;
        self.ui.set_information_text(text.into());
        self.ui.set_information_offset(0.);
        self.information_scroll_manual = false;
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }
    fn ai_launch(&mut self, index: usize) -> Option<Outcome> {
        let game = self.ai.guide.as_ref()?.games.get(index)?.clone();
        let Some(system) = self.all_systems.iter().find(|s| s.def.id == game.system) else {
            self.ai_fail("This system is no longer installed.".into());
            return None;
        };
        let browse::Kind::Play(launch) = game.row.kind else {
            self.ai_fail("This entry is not playable.".into());
            return None;
        };
        self.launch_game(
            game.row.name,
            launch.clone(),
            system.to_config(),
            Some(game.system),
            launch.clone(),
            launch,
        )
    }
    fn ai_start(&mut self, question: String, web: bool) -> std::result::Result<(), String> {
        if self.kiosk_locked || !self.settings.ai_enabled.unwrap_or(false) {
            return Err("Explore with AI is off or locked.".into());
        }
        if self.ai.job.is_some() || self.ai.ready.is_some() {
            return Err("A guide is already in progress.".into());
        }
        if web && matches!(self.ai.page, Some(AiPage::Guide | AiPage::Story)) {
            return Err("Close the guide being read on MiSTer before starting another.".into());
        }
        let provider = self.ai_provider();
        let key = guides::key(self.ai_dir(), provider)?;
        if key.is_empty() {
            return Err(format!(
                "Set the {} API key in Explore with AI settings.",
                provider.label()
            ));
        }
        let history_error = if self.ai.guide.is_none() {
            self.ai_read_last().err()
        } else {
            None
        };
        let brief = guides::Brief {
            question,
            recent: self
                .ai
                .guide
                .as_ref()
                .map(|g| g.recent.clone())
                .unwrap_or_default(),
            seed: next_random(&mut self.seed),
            whole: self.ai.full,
        };
        self.ai.job = Some(guides::Job::start(
            self.indexed_library_request(false),
            brief,
            provider,
            key,
        )?);
        self.ai.web_only = web;
        self.ai.error = history_error;
        if let Some(e) = &self.ai.error {
            crate::note(&format!("ai guides: {e}"));
        }
        self.ai.progress = "Reading indexed games".into();
        if !web {
            self.ai_menu(AiPage::Working);
        }
        Ok(())
    }
    fn ai_confirm(&mut self) -> std::result::Result<(), String> {
        let provider = self.ai_provider();
        let key = guides::key(self.ai_dir(), provider)?;
        if key.is_empty() {
            return Err("The provider key is not set.".into());
        }
        let ready = self.ai.ready.take().ok_or("No catalogue read to confirm")?;
        match guides::Job::confirmed(*ready, provider, key) {
            Ok(job) => self.ai.job = Some(job),
            Err(e) => {
                if !self.ai.web_only {
                    self.ai_menu(AiPage::Prompts);
                }
                self.ai.error = Some(e.clone());
                return Err(e);
            }
        }
        self.ai.progress = "Reading whole catalogue".into();
        if !self.ai.web_only {
            self.ai_menu(AiPage::Working);
        }
        Ok(())
    }
    fn ai_cancel(&mut self) {
        if let Some(j) = self.ai.job.take() {
            j.cancel();
        }
        self.ai.ready = None;
        self.ai.error = None;
        self.ai.progress = "Cancelled.".into();
        if !self.ai.web_only {
            self.ai_menu(AiPage::Prompts);
        }
    }
    fn ai_poll(&mut self) {
        while let Some(job) = &self.ai.job {
            let ev = match job.rx.try_recv() {
                Ok(e) => e,
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(_) => guides::Event::Error("Guide worker disconnected.".into()),
            };
            match ev {
                guides::Event::Progress(s) => {
                    self.ai.progress = s;
                    if self.ai.page == Some(AiPage::Working) && !self.menu.is_empty() {
                        self.menu[0].clone_from(&self.ai.progress);
                        self.dirty = true;
                    }
                }
                guides::Event::Confirm(r) => {
                    self.ai.job = None;
                    self.ai.ready = Some(r);
                    if !self.ai.web_only {
                        self.ai_menu(AiPage::Confirm);
                    }
                    return;
                }
                guides::Event::Error(e) => {
                    self.ai.job = None;
                    self.ai.error = Some(e.clone());
                    if !self.ai.web_only {
                        self.ai_menu(AiPage::Prompts);
                        self.ai_fail(e);
                    }
                    return;
                }
                guides::Event::Done(g) => {
                    self.ai.job = None;
                    let result = serde_json::to_vec(&g)
                        .map_err(|_| "Could not encode the guide.".to_string())
                        .and_then(|b| {
                            crate::scraper::atomic_write(&self.ai_guide_file(), &b, "last guide")
                                .map_err(|e| e.detail)
                        });
                    if let Err(e) = result {
                        self.ai.error = Some(e.clone());
                        if !self.ai.web_only {
                            self.ai_menu(AiPage::Prompts);
                            self.ai_fail(e);
                        }
                        return;
                    }
                    self.ai.guide = Some(g);
                    if !self.ai.web_only {
                        self.ai_menu(AiPage::Guide);
                    }
                    return;
                }
            }
        }
    }
    fn ai_keyboard_action(&mut self, action: Action) -> bool {
        if !matches!(
            self.name_keyboard_purpose,
            NamePurpose::AiKey | NamePurpose::AiPromptName | NamePurpose::AiPromptText
        ) {
            return false;
        }
        let k = name_keyboard::keys(self.name_keyboard_page, true)
            .get(self.name_keyboard_list.selected())
            .copied();
        if action == Action::Quit || (action == Action::Accept && k == Some(NameKey::Cancel)) {
            self.name_keyboard_draft.clear();
            self.ai_menu(if self.name_keyboard_purpose == NamePurpose::AiKey {
                AiPage::Setup
            } else {
                AiPage::Manage
            });
            return true;
        }
        if action == Action::Accept && k == Some(NameKey::Save) {
            self.finish_name_keyboard();
            return true;
        }
        false
    }
    fn ai_finish_input(&mut self, draft: String) {
        self.name_keyboard_draft.clear();
        match self.name_keyboard_purpose {
            NamePurpose::AiKey => {
                self.ai.pending_key = draft;
                self.ai_menu(AiPage::SaveKey);
            }
            NamePurpose::AiPromptName => {
                if draft.trim().is_empty() {
                    self.ai_menu(AiPage::Manage);
                    return;
                }
                self.ai.draft_name = draft;
                self.ai.old_text = None;
                self.open_name_keyboard(NamePurpose::AiPromptText, String::new());
            }
            NamePurpose::AiPromptText => {
                let res = guides::edit_prompt(
                    &self.ai_user_file(),
                    &self.ai.draft_name,
                    self.ai.old_text.as_deref(),
                    Some(&draft),
                );
                self.ai_menu(AiPage::Manage);
                if let Err(e) = res {
                    self.ai_fail(e);
                }
            }
            _ => {}
        }
    }
    fn ai_input(&mut self, action: Action) -> Option<Option<Outcome>> {
        let page = self.ai.page?;
        if self.screen == Screen::NameKeyboard {
            return None;
        }
        if self.screen != Screen::Context
            && !(self.screen == Screen::Information && page == AiPage::Story)
        {
            return None;
        }
        let at = self.menu_list.selected();
        if page == AiPage::Story {
            match action {
                Action::Accept => return Some(self.ai_launch(self.ai.selected)),
                Action::Quit | Action::Context | Action::Menu => {
                    self.ai_menu(AiPage::Guide);
                    self.menu_list.select(self.ai.selected);
                }
                Action::Slower | Action::Faster => {
                    let count = self.ai.guide.as_ref()?.games.len();
                    let next = if action == Action::Slower {
                        (self.ai.selected + count - 1) % count
                    } else {
                        (self.ai.selected + 1) % count
                    };
                    self.ai_story(next);
                }
                _ => self.handle_information(action),
            }
            return Some(None);
        }
        if matches!(action, Action::Quit | Action::Menu) {
            match page {
                AiPage::Working | AiPage::Confirm => self.ai_cancel(),
                AiPage::Setup if self.ai.from_settings => {
                    self.ai.page = None;
                    self.open_options_page(OptionsPage::Library);
                }
                AiPage::Setup | AiPage::Manage | AiPage::Guide => self.ai_menu(AiPage::Prompts),
                AiPage::Edit | AiPage::Delete => self.ai_menu(AiPage::Manage),
                AiPage::SaveKey | AiPage::ClearKey => {
                    self.ai.pending_key.clear();
                    self.ai_menu(AiPage::Setup);
                }
                _ => {
                    self.ai.page = None;
                    self.screen = Screen::Browse;
                    self.apply_geometry();
                    self.touch_selection();
                    self.dirty = true;
                }
            }
            return Some(None);
        }
        match action {
            Action::Up => {
                self.menu_list.move_items(-1);
            }
            Action::Down => {
                self.menu_list.move_items(1);
            }
            Action::PageUp | Action::Slower => {
                self.menu_list
                    .move_rows(-(self.menu_list.visible() as isize));
            }
            Action::PageDown | Action::Faster => {
                self.menu_list.move_rows(self.menu_list.visible() as isize);
            }
            Action::Home => self.menu_list.select(0),
            Action::End => self.menu_list.select(self.menu.len().saturating_sub(1)),
            Action::Accept => {
                if let Err(e) = self.ai_choose(page, at) {
                    self.ai_fail(e);
                }
            }
            _ => {}
        }
        self.dirty = true;
        Some(None)
    }
    fn ai_choose(&mut self, page: AiPage, at: usize) -> std::result::Result<(), String> {
        match page {
            AiPage::Setup => match at {
                0 => {
                    let on = !self.settings.ai_enabled.unwrap_or(false);
                    let mut next = self.settings.clone();
                    next.ai_enabled = Some(on);
                    let key = crate::home::category_key(guides::NAME);
                    if on {
                        next.home.hidden.remove(&key);
                    } else {
                        next.home.hidden.insert(key);
                    }
                    next.save(&self.settings_path).map_err(|e| e.to_string())?;
                    self.settings = next;
                    self.rebuild_system_list();
                    self.ai_menu(page);
                }
                1 => {
                    let provider = if self.ai_provider() == guides::Provider::OpenAi {
                        guides::Provider::Claude
                    } else {
                        guides::Provider::OpenAi
                    };
                    let mut next = self.settings.clone();
                    next.ai_provider = Some(provider);
                    next.save(&self.settings_path).map_err(|e| e.to_string())?;
                    self.settings = next;
                    self.ai_menu(page);
                }
                2 => self.open_name_keyboard(NamePurpose::AiKey, String::new()),
                _ => self.ai_menu(AiPage::ClearKey),
            },
            AiPage::Prompts => {
                let base = 3 + guides::PROMPTS.len();
                if at == 0 {
                    self.ai_start(
                        "Surprise me with an original story connecting games on this machine."
                            .into(),
                        false,
                    )?;
                } else if at == 1 {
                    if self.ai.job.is_some() || self.ai.ready.is_some() {
                        return Err("A new guide is in progress.".into());
                    }
                    self.ai_read_last()?;
                    if self.ai.guide.is_some() {
                        self.ai_menu(AiPage::Guide);
                    } else {
                        return Err("No guide has been made yet.".into());
                    }
                } else if at == 2 {
                    self.ai.full = !self.ai.full;
                    self.ai_menu(page);
                    self.menu_list.select(2);
                } else if at < base {
                    self.ai_start(guides::PROMPTS[at - 3].1.into(), false)?;
                } else if at < base + self.ai.custom.len() {
                    self.ai_start(self.ai.custom[at - base].1.clone(), false)?;
                } else if at == base + self.ai.custom.len() {
                    self.ai_menu(AiPage::Manage);
                } else {
                    self.ai.from_settings = false;
                    self.ai_menu(AiPage::Setup);
                }
            }
            AiPage::Manage => {
                if at == 0 {
                    self.open_name_keyboard(NamePurpose::AiPromptName, String::new());
                } else if let Some((n, q)) = self.ai.custom.get(at - 1) {
                    self.ai.draft_name = n.clone();
                    self.ai.old_text = Some(q.clone());
                    self.ai_menu(AiPage::Edit);
                }
            }
            AiPage::Edit => {
                if at == 0 {
                    self.open_name_keyboard(
                        NamePurpose::AiPromptText,
                        self.ai.old_text.clone().unwrap_or_default(),
                    );
                } else {
                    self.ai_menu(AiPage::Delete);
                }
            }
            AiPage::Delete => {
                if at == 1 {
                    guides::edit_prompt(
                        &self.ai_user_file(),
                        &self.ai.draft_name,
                        self.ai.old_text.as_deref(),
                        None,
                    )?;
                }
                self.ai_menu(AiPage::Manage);
            }
            AiPage::SaveKey => {
                let key = std::mem::take(&mut self.ai.pending_key);
                self.ai_menu(AiPage::Setup);
                if at == 1 {
                    if key.is_empty() {
                        return Err("Enter an API key first.".into());
                    }
                    guides::save_key(self.ai_dir(), self.ai_provider(), &key)?;
                }
            }
            AiPage::ClearKey => {
                if at == 1 {
                    guides::save_key(self.ai_dir(), self.ai_provider(), "")?;
                }
                self.ai_menu(AiPage::Setup);
            }
            AiPage::Confirm => {
                if at == 1 {
                    self.ai_confirm()?;
                } else {
                    self.ai_cancel();
                }
            }
            AiPage::Working => {
                if at == 1 {
                    self.ai_cancel();
                }
            }
            AiPage::Guide => self.ai_story(at),
            AiPage::Story => {}
        }
        Ok(())
    }
}
