impl App {
    fn web_view_place(
        &self,
        target: Option<&str>,
    ) -> std::result::Result<Option<ViewPlace>, WebError> {
        let place = match target {
            None => return Ok(None),
            Some("home") => ViewPlace::Categories,
            Some("explore") => ViewPlace::Games {
                system: crate::explore::STATE_ID.into(),
                place: "explore".into(),
            },
            Some("history") => ViewPlace::Games {
                system: LAST_PLAYED_SYSTEM.into(),
                place: LAST_PLAYED_PLACE.into(),
            },
            Some("cores") => ViewPlace::Systems(CORES_CATEGORY.into()),
            Some(id) if id.starts_with("cores:") => {
                let category = &id[6..];
                if !self
                    .core_catalogue
                    .entries
                    .iter()
                    .any(|entry| entry.category == category)
                {
                    return Err(web_error("That installed-core category is unavailable."));
                }
                ViewPlace::Games {
                    system: CORES_SYSTEM_ID.into(),
                    place: category.into(),
                }
            }
            Some(id) => {
                let target = self
                    .web
                    .targets
                    .get(id)
                    .ok_or_else(|| web_error("Refresh the target folder first."))?;
                match &target.target {
                    crate::home::Target::Category { category } => match category.as_str() {
                        crate::explore::NAME => ViewPlace::Games {
                            system: crate::explore::STATE_ID.into(),
                            place: "explore".into(),
                        },
                        LAST_PLAYED_CATEGORY => ViewPlace::Games {
                            system: LAST_PLAYED_SYSTEM.into(),
                            place: LAST_PLAYED_PLACE.into(),
                        },
                        MISTERZINE_CATEGORY | SCRIPTS_CATEGORY | ATTRACT_MODE_CATEGORY => {
                            return Err(web_error("This native screen has a fixed layout."));
                        }
                        _ => ViewPlace::Systems(category.clone()),
                    },
                    crate::home::Target::LibraryFolder { .. }
                    | crate::home::Target::System { .. } => {
                        let (system, place) = self.web_folder_target(id)?;
                        ViewPlace::Games { system, place }
                    }
                    crate::home::Target::Folder { .. } => ViewPlace::Games {
                        system: "@home".into(),
                        place: target
                            .home_entry
                            .clone()
                            .ok_or_else(|| web_error("Refresh the personal folder first."))?,
                    },
                    crate::home::Target::Collection { .. } => ViewPlace::Games {
                        system: crate::explore::STATE_ID.into(),
                        place: "explore".into(),
                    },
                    _ => return Err(web_error("Choose Home, a category or a library folder.")),
                }
            }
        };
        Ok(Some(place))
    }
    fn web_folder_target(&self, id: &str) -> std::result::Result<(String, String), WebError> {
        match &self
            .web
            .targets
            .get(id)
            .ok_or_else(|| web_error("Refresh the folder page first."))?
            .target
        {
            crate::home::Target::LibraryFolder { system, trail } => Ok((
                system.clone(),
                trail
                    .last()
                    .ok_or_else(|| web_error("Missing folder identity."))?
                    .key(),
            )),
            crate::home::Target::System { system } => {
                let found = self
                    .all_systems
                    .iter()
                    .find(|found| found.def.id == *system)
                    .ok_or_else(|| web_error("System is unavailable."))?;
                Ok((
                    system.clone(),
                    crate::browse::start_for(&found.to_config()).key(),
                ))
            }
            _ => Err(web_error("Choose a library or Favourites folder.")),
        }
    }

    fn web_core_choices(
        &self,
        system: &str,
        game: Option<&str>,
    ) -> std::result::Result<JsonValue, WebError> {
        let resolved = game.map(|id| self.web_game_target(id)).transpose()?;
        if let Some(id) = game {
            if !matches!(&self.web.targets[id].target,crate::home::Target::Game{system:owner,..} if owner==system)
            {
                return Err(web_error("The game does not belong to this system."));
            }
        }
        let system = resolved
            .as_ref()
            .map(|(system, _, _)| system.as_str())
            .unwrap_or(system);
        let found = self
            .all_systems
            .iter()
            .find(|found| found.def.id == system)
            .ok_or_else(|| web_error("System is unavailable."))?;
        let config = found.to_config();
        let game_launch = resolved.as_ref().map(|(_, launch, _)| launch);
        let family = game_launch
            .and_then(|launch| {
                self.settings
                    .game_launch_cores
                    .get(system)
                    .and_then(|choices| choices.get(&crate::game_launch_cores::key(launch)))
            })
            .or_else(|| {
                if game.is_none() {
                    self.settings.launch_cores.get(system)
                } else {
                    None
                }
            });
        let version = self.settings.core_choices.get(system);
        let root = Path::new(&self.config.menu_root);
        let ra = self.settings.core_preference.unwrap_or_default()
            == crate::settings::CorePreference::RetroAchievementsFirst;
        let choices = if game.is_some() {
            crate::launch_cores::game_choices_for_version(
                &config,
                root,
                self.settings.launch_cores.get(system).map(String::as_str),
                family.map(String::as_str),
                version.map(String::as_str),
                ra,
            )
        } else {
            crate::launch_cores::choices_for_version(
                &config,
                root,
                family.map(String::as_str),
                version.map(String::as_str),
                ra,
            )
        };
        let families: Vec<_> = choices
            .into_iter()
            .map(|choice| json!({"id":choice.id,"label":choice.label,"available":choice.available}))
            .collect();
        let versions: Vec<_> = crate::core_choices::available(&config, root)
            .map_err(web_error)?
            .into_iter()
            .map(|choice| json!({"id":choice.key,"label":choice.label}))
            .collect();
        Ok(
            json!({"families":families,"versions":versions,"family":family,"version":version,"revision":self.web_revision(),"game":game}),
        )
    }

    fn web_themes(&self) -> JsonValue {
        let items:Vec<_>=self.themes.iter().map(|theme|json!({"name":theme.name,"text":theme.file.to_toml(&self.config.colors),"editable":theme.file.created_by_editor})).collect();
        json!({"items":items,"current":self.settings.theme,"revision":self.web_revision(),"base":self.config.colors})
    }

    fn web_select_theme(&mut self, name: Option<&str>) -> std::result::Result<(), WebError> {
        let selected = name
            .map(|name| {
                self.themes
                    .iter()
                    .position(|theme| theme.name == name)
                    .ok_or_else(|| web_error("The theme no longer exists."))
            })
            .transpose()?;
        let mut settings = self.settings.clone();
        settings.theme = name.map(str::to_string);
        settings.theme_font_override = Some(false);
        let saved = self.web_save(settings);
        if saved
            .as_ref()
            .is_err_and(|error| error.code != "durability")
        {
            return saved;
        }
        self.active_theme = selected;
        self.font = self.effective_font();
        self.text_size = self.effective_text_size();
        self.apply_palette();
        self.apply_geometry();
        invalidate_geometry(&mut self.pending_complete_repaints, &mut self.dirty);
        saved
    }

    fn web_edit_command(
        &mut self,
        command: WebCommand,
    ) -> std::result::Result<JsonValue, WebError> {
        match command {
            WebCommand::Reset { revision, kind } => {
                self.web_check_revision(&revision)?;
                let mut settings = self.settings.clone();
                let count = match kind {
                    crate::web::api::Reset::ViewSelections => clear_custom_views(&mut settings),
                    crate::web::api::Reset::Hidden => clear_hidden(&mut settings),
                };
                self.web_save(settings)?;
                self.relist_here_preserving_game_filters();
                Ok(json!({"changed":count,"revision":self.web_revision()}))
            }
            WebCommand::FolderOptions { target } => {
                let (system, place) = self.web_folder_target(&target)?;
                Ok(
                    json!({"preferences":self.settings.folder_preferences.get(&system).and_then(|places|places.get(&place)).cloned().unwrap_or_default(),"revision":self.web_revision()}),
                )
            }
            WebCommand::FolderOptionsSave {
                revision,
                target,
                preferences,
            } => {
                self.web_check_revision(&revision)?;
                let (system, place) = self.web_folder_target(&target)?;
                let mut settings = self.settings.clone();
                set_folder_preferences(&mut settings, &system, &place, preferences);
                self.web_save(settings)?;
                self.resolve_folder_preferences();
                self.relist_here_preserving_game_filters();
                Ok(json!({"revision":self.web_revision()}))
            }
            WebCommand::Hide {
                revision,
                target,
                hidden,
            } => {
                self.web_check_revision(&revision)?;
                let target = self
                    .web
                    .targets
                    .get(&target)
                    .ok_or_else(|| web_error("Refresh the item's page first."))?;
                let mut settings = self.settings.clone();
                let (values, key) = match &target.target {
                    crate::home::Target::System { system } => {
                        (&mut settings.hidden, system.clone())
                    }
                    crate::home::Target::LibraryFolder { trail, .. } => (
                        &mut settings.hidden_paths,
                        trail
                            .last()
                            .ok_or_else(|| web_error("Missing folder identity."))?
                            .key(),
                    ),
                    crate::home::Target::Game { .. } => (
                        &mut settings.hidden_paths,
                        row_key(
                            target
                                .row
                                .as_ref()
                                .ok_or_else(|| web_error("Open the game information first."))?,
                        ),
                    ),
                    _ => return Err(web_error("Use Home Editor to hide Home entries.")),
                };
                if hidden {
                    if !values.contains(&key) {
                        values.push(key);
                    }
                } else {
                    values.retain(|value| value != &key);
                }
                self.web_save(settings)?;
                self.relist_here_preserving_game_filters();
                Ok(json!({"revision":self.web_revision()}))
            }
            WebCommand::Themes => Ok(self.web_themes()),
            WebCommand::ThemeSave {
                revision,
                name,
                text,
                replace,
            } => {
                self.web_check_revision(&revision)?;
                let file = crate::theme::ThemeFile::parse(&text).map_err(web_error)?;
                if replace {
                    let original = self
                        .themes
                        .iter()
                        .find(|theme| theme.name == name)
                        .ok_or_else(|| web_error("The theme no longer exists."))?;
                    crate::theme::replace_editor_theme(
                        &self.themes_dir,
                        original,
                        &file,
                        &self.config.colors,
                    )
                    .map_err(web_error)?
                    .commit()
                    .map_err(web_error)?;
                } else {
                    crate::theme::save_new(&self.themes_dir, &name, &file, &self.config.colors)
                        .map_err(web_error)?;
                }
                let loaded = crate::theme::load_available(&self.themes_dir);
                self.themes = loaded.themes;
                let current = self.settings.theme.clone();
                self.web_select_theme(current.as_deref())?;
                Ok(self.web_themes())
            }
            WebCommand::ThemeDelete { revision, name } => {
                self.web_check_revision(&revision)?;
                let theme = self
                    .themes
                    .iter()
                    .find(|theme| theme.name == name)
                    .ok_or_else(|| web_error("The theme no longer exists."))?;
                let staged = crate::theme::stage_editor_theme_deletion(&self.themes_dir, theme)
                    .map_err(web_error)?;
                if self.settings.theme.as_deref() == Some(&name) {
                    if let Err(error) = self.web_select_theme(None) {
                        staged.restore().map_err(web_error)?;
                        return Err(error);
                    }
                }
                staged.commit().map_err(web_error)?;
                self.themes = crate::theme::load_available(&self.themes_dir).themes;
                Ok(self.web_themes())
            }
            WebCommand::ThemeSelect { revision, name } => {
                self.web_check_revision(&revision)?;
                self.web_select_theme(name.as_deref())?;
                Ok(self.web_themes())
            }
            WebCommand::Controllers => Ok(
                json!({"profiles":self.settings.controller_profiles,"revision":self.web_revision(),"capture":"Capture new physical buttons and test the controller on MiSTer."}),
            ),
            WebCommand::ControllerSave {
                revision,
                id,
                profile,
            } => {
                self.web_check_revision(&revision)?;
                if !self.settings.controller_profiles.contains_key(&id) {
                    return Err(web_error("First capture this controller on MiSTer."));
                }
                if self.controllers.job.is_some() {
                    return Err(WebError::new(
                        "busy",
                        "A controller operation is already running.",
                    ));
                }
                profile.validate().map_err(web_error)?;
                let mut profiles = self.settings.controller_profiles.clone();
                profiles.insert(id, profile);
                self.apply_controller_profiles(profiles);
                if self.controllers.job.is_none() {
                    return Err(web_error(
                        self.message
                            .as_deref()
                            .unwrap_or("Controller apply did not start."),
                    ));
                }
                let id = self.web_new_job("Applying controller preferences");
                self.web.controller_job = Some(id);
                Ok(json!({"job":id}))
            }
            WebCommand::Cores { category } => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Cores are unavailable in kiosk mode.",
                    ));
                }
                let entries: Vec<_> = self
                    .core_catalogue
                    .entries
                    .iter()
                    .filter(|entry| {
                        category
                            .as_ref()
                            .is_none_or(|category| &entry.category == category)
                    })
                    .cloned()
                    .collect();
                let items:Vec<_>=entries.iter().map(|entry| {let id=self.web_register(crate::home::Target::Core{path:entry.path.clone()},None);let image=self.web_art(self.core_logo(entry));json!({"id":id,"name":entry.label(),"category":entry.category,"image":image})}).collect();
                Ok(
                    json!({"items":items,"categories":self.core_catalogue.categories().into_iter().map(|(name,_)|name).collect::<Vec<_>>(),"view_target":category.map(|name|format!("cores:{name}")).unwrap_or_else(||"cores".into())}),
                )
            }
            WebCommand::CoreChoices { system, game } => {
                self.web_core_choices(&system, game.as_deref())
            }
            WebCommand::CoreSelect {
                revision,
                system,
                game,
                family,
                version,
            } => {
                self.web_check_revision(&revision)?;
                let choices = self.web_core_choices(&system, game.as_deref())?;
                let family = family.filter(|id| !id.is_empty());
                if let Some(family) = &family {
                    if !choices["families"]
                        .as_array()
                        .expect("choices")
                        .iter()
                        .any(|choice| choice["id"] == *family && choice["available"] == true)
                    {
                        return Err(web_error("This core family is not available."));
                    }
                }
                let mut settings = self.settings.clone();
                if let Some(game) = &game {
                    if version.is_some() {
                        return Err(web_error("Core Version is a per-system setting. A game can choose its compatible core family."));
                    }
                    let (owner, launch, _) = self.web_game_target(game)?;
                    crate::game_launch_cores::set(
                        &mut settings,
                        &owner,
                        &crate::game_launch_cores::key(&launch),
                        family.as_deref(),
                    );
                } else {
                    if let Some(family) = family {
                        settings.launch_cores.insert(system.clone(), family);
                    } else {
                        settings.launch_cores.remove(&system);
                    }
                    if let Some(version) = version.filter(|version| !version.is_empty()) {
                        if !choices["versions"]
                            .as_array()
                            .expect("versions")
                            .iter()
                            .any(|choice| choice["id"] == version)
                        {
                            return Err(web_error("This core version is not available."));
                        }
                        settings.core_choices.insert(system.clone(), version);
                    } else {
                        settings.core_choices.remove(&system);
                    }
                }
                self.web_save(settings)?;
                self.web_core_choices(&system, game.as_deref())
            }
            other => self.web_data_command(other),
        }
    }
}
