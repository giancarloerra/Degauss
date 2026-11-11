impl App {
    fn web_writing(&self) -> bool {
        self.web.source_job.is_some()
            || self.web.pack_job.is_some()
            || self.web.upload_job.is_some()
            || self.web.automatic_job.is_some()
    }

    fn defer_for_web_write(&mut self) -> bool {
        if !self.web_writing() {
            return false;
        }
        if self.pending.is_some() {
            return true;
        }
        self.message=Some("A Web Access library operation is running. Wait for it to finish, or cancel it in the browser.".into());
        self.dirty = true;
        true
    }

    fn web_storage_idle(&self) -> std::result::Result<(), WebError> {
        if self.build.is_some()
            || self.refreshing.is_some()
            || self.source_job.is_some()
            || self.source_resolution.is_some()
            || self.provider_job.is_some()
            || self.scraper_job.is_some()
            || self.scraper_cache_refresh_active
            || self.web_writing()
            || self.web.password_job.is_some()
            || self.controllers.job.is_some()
        {
            Err(WebError::new(
                "busy",
                "Wait for the current library operation to finish.",
            ))
        } else {
            Ok(())
        }
    }

    fn web_game_target(
        &self,
        id: &str,
    ) -> std::result::Result<(String, browse::Launch, String), WebError> {
        let target = self
            .web
            .targets
            .get(id)
            .ok_or_else(|| web_error("Refresh the game list first."))?;
        if self.kiosk_locked && !self.kiosk_target_allowed(&target.target) {
            return Err(WebError::new(
                "locked",
                "This game is unavailable in kiosk mode.",
            ));
        }
        let crate::home::Target::Game { system, launch } = &target.target else {
            return Err(web_error("Choose a game."));
        };
        let name = target
            .row
            .as_ref()
            .map(|row| row.name.clone())
            .ok_or_else(|| web_error("Open the game's library entry first."))?;
        let found = self
            .all_systems
            .iter()
            .find(|found| found.def.id == *system)
            .ok_or_else(|| web_error("System is unavailable."))?;
        if is_favorites(found.category()) {
            let (_, system, _, launch) = self
                .favorite_launch_context(launch, found.to_config())
                .map_err(web_error)?;
            Ok((
                system.ok_or_else(|| {
                    web_error("This favourite has no identifiable owning system.")
                })?,
                launch,
                name,
            ))
        } else {
            Ok((system.clone(), launch.clone(), name))
        }
    }

    fn web_scraper_settings(&self) -> JsonValue {
        let mut values = serde_json::to_value(&self.scraper_settings).expect("scraper settings");
        let fields = values.as_object_mut().expect("settings object");
        fields.remove("password");
        fields.remove("accepted_plaintext_warning");
        fields.insert("system_ids".into(), json!(self.scraper_settings.system_ids));
        fields.insert(
            "system_media_types".into(),
            json!(self.scraper_settings.system_media_types),
        );
        json!({"values":values,"password_set":!self.scraper_settings.password.is_empty(),"revision":self.web_revision()})
    }

    fn web_scraper_developer(
        &self,
    ) -> std::result::Result<Option<crate::scraper::DeveloperCredentials>, WebError> {
        if self.scraper_settings.source == crate::scraper::ScraperSource::Libretro {
            Ok(None)
        } else {
            crate::scraper::DeveloperCredentials::embedded()
                .map(Some)
                .map_err(web_error)
        }
    }

    fn web_scraper_scope(
        &self,
        target: Option<&str>,
    ) -> std::result::Result<crate::scraper::Scope, WebError> {
        let Some(id) = target else {
            return Ok(crate::scraper::Scope::All);
        };
        let target = self
            .web
            .targets
            .get(id)
            .ok_or_else(|| web_error("Refresh the selected page first."))?;
        match &target.target {
            crate::home::Target::Game { .. } => {
                let (system_id, launch, title) = self.web_game_target(id)?;
                Ok(crate::scraper::Scope::Game {
                    system_id,
                    launch,
                    title,
                })
            }
            crate::home::Target::System { system } => {
                let found = self
                    .all_systems
                    .iter()
                    .find(|found| found.def.id == *system)
                    .ok_or_else(|| web_error("System is unavailable."))?;
                Ok(crate::scraper::Scope::System {
                    system_id: system.clone(),
                    place: browse::start_for(&found.to_config()),
                    display_name: found.name().to_string(),
                })
            }
            crate::home::Target::LibraryFolder { system, trail } => {
                let place = trail
                    .last()
                    .cloned()
                    .ok_or_else(|| web_error("Missing folder identity."))?;
                Ok(crate::scraper::Scope::Folder {
                    system_id: system.clone(),
                    display_name: target
                        .row
                        .as_ref()
                        .map(|row| row.name.clone())
                        .unwrap_or_else(|| system.clone()),
                    place,
                })
            }
            _ => Err(web_error(
                "Choose a game, library folder or system to scrape.",
            )),
        }
    }

    fn web_data_command(
        &mut self,
        command: WebCommand,
    ) -> std::result::Result<JsonValue, WebError> {
        match command {
            WebCommand::ScraperSettings => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Unlock kiosk mode to view scraper settings.",
                    ));
                }
                Ok(self.web_scraper_settings())
            }
            WebCommand::ScraperSettingsSave {
                revision,
                values,
                password,
                accept_plaintext_storage,
            } => {
                self.web_check_revision(&revision)?;
                self.web_storage_idle()?;
                let current = self.web_scraper_settings();
                let submitted = values
                    .as_object()
                    .ok_or_else(|| web_error("Scraper settings must be an object."))?;
                if submitted
                    .keys()
                    .any(|key| current["values"].get(key).is_none())
                {
                    return Err(web_error("Unknown scraper setting."));
                }
                let mut merged = serde_json::to_value(&self.scraper_settings).map_err(web_error)?;
                for (key, value) in submitted {
                    merged[key] = value.clone();
                }
                if let Some(password) = password {
                    if !password.is_empty() && !accept_plaintext_storage {
                        return Err(web_error("Confirm that the ScreenScraper password will be stored as plain text on the card."));
                    }
                    merged["password"] = json!(password);
                    merged["accepted_plaintext_warning"] = json!(accept_plaintext_storage);
                }
                let settings: crate::scraper::ScraperSettings =
                    serde_json::from_value(merged).map_err(web_error)?;
                settings.validate().map_err(web_error)?;
                if !self.replace_scraper_settings(settings) {
                    return Err(web_error(
                        self.message
                            .as_deref()
                            .unwrap_or("Scraper settings were not saved."),
                    ));
                }
                self.web.matches.clear();
                Ok(self.web_scraper_settings())
            }
            WebCommand::ScraperSearch { game, term } => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Unlock kiosk mode to scrape artwork.",
                    ));
                }
                if self.web.search_job.is_some() {
                    return Err(WebError::new(
                        "busy",
                        "An artwork search is already running.",
                    ));
                }
                let (system, _, _) = self.web_game_target(&game)?;
                if self.pack_selected(&system) {
                    return Err(web_error("This system uses Artwork Pack. Use its manual pack matcher, or explicitly choose Gamelist first."));
                }
                let system_id = crate::scraper::platform_id_for_source(
                    &system,
                    &self.scraper_settings.system_ids,
                    self.scraper_settings.source,
                )
                .ok_or_else(|| {
                    web_error("The selected scraper has no platform mapping for this system.")
                })?;
                let term = web_name(&term)?;
                let job = crate::scraper::start_search(crate::scraper::SearchRequest {
                    system_id,
                    term,
                    settings: self.scraper_settings.clone(),
                    developer: self.web_scraper_developer()?,
                    cache_dir: self.cache_dir.clone(),
                })
                .map_err(web_error)?;
                self.web.search_settings = Some(web_id("scraper-settings", &self.scraper_settings));
                let id = self.web_new_job("Searching artwork");
                self.web.search_job = Some((id, game, job));
                Ok(json!({"job":id}))
            }
            WebCommand::ScraperPreview { match_id } => {
                if self.web.search_settings.as_deref()
                    != Some(web_id("scraper-settings", &self.scraper_settings).as_str())
                {
                    return Err(web_error("Scraper settings changed. Search again before previewing or saving a match."));
                }
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Unlock kiosk mode to scrape artwork.",
                    ));
                }
                if self.web.preview_job.is_some() {
                    return Err(WebError::new(
                        "busy",
                        "An artwork preview is already loading.",
                    ));
                }
                let (_, matched) = self
                    .web
                    .matches
                    .get(&match_id)
                    .ok_or_else(|| web_error("Search again before choosing this match."))?;
                let media = matched
                    .media
                    .clone()
                    .ok_or_else(|| web_error("This match has no image."))?;
                let max_download_speed = if self.scraper_settings.source
                    == crate::scraper::ScraperSource::Libretro
                {
                    u64::MAX
                } else {
                    self.web.scraper_account.as_ref().and_then(|account|account.max_download_speed).ok_or_else(||web_error("ScreenScraper did not report a media-download allowance. Search again."))?
                };
                let job = crate::scraper::start_preview(crate::scraper::PreviewRequest {
                    match_id,
                    media,
                    libretro_urls: crate::scraper::libretro_thumbnail_candidates(matched),
                    settings: self.scraper_settings.clone(),
                    developer: self.web_scraper_developer()?,
                    max_download_speed,
                    max_edge: 320,
                    ground: [73, 73, 73],
                })
                .map_err(web_error)?;
                let id = self.web_new_job("Loading artwork preview");
                self.web.preview_job = Some((id, job));
                Ok(json!({"job":id}))
            }
            WebCommand::Scrape { target, match_id } => {
                self.web_storage_idle()?;
                let scope = self.web_scraper_scope(target.as_deref())?;
                let exclusions = self.artwork_pack_scraper_exclusions();
                if let crate::scraper::Scope::Game { system_id, .. } = &scope {
                    if exclusions.contains(system_id) {
                        return Err(web_error("This game uses Artwork Pack. Use its manual pack matcher, or explicitly choose Gamelist first."));
                    }
                }
                if self.scraper_settings.source == crate::scraper::ScraperSource::ScreenScraper
                    && !self.scraper_settings.accepted_plaintext_warning
                {
                    return Err(web_error(
                        "Save and confirm ScreenScraper credentials in scraper settings first.",
                    ));
                }
                let request = crate::scraper::Request {
                    scope: scope.clone(),
                    scope_label: scope.label().into(),
                    systems: self.all_systems.clone(),
                    names: self.names.clone(),
                    settings: self.scraper_settings.clone(),
                    developer: self.web_scraper_developer()?,
                    cache_dir: self.cache_dir.clone(),
                    artwork_pack_system_ids: exclusions,
                };
                let job = if let Some(match_id) = match_id {
                    if self.web.search_settings.as_deref()
                        != Some(web_id("scraper-settings", &self.scraper_settings).as_str())
                    {
                        return Err(web_error(
                            "Scraper settings changed. Search again before saving a match.",
                        ));
                    }
                    let (game, matched) = self
                        .web
                        .matches
                        .get(&match_id)
                        .ok_or_else(|| web_error("Search again before saving this match."))?;
                    if target.as_deref() != Some(game) {
                        return Err(web_error("The selected match belongs to another game."));
                    }
                    crate::scraper::start_selected(request, matched.clone())
                } else {
                    crate::scraper::start(request)
                }
                .map_err(web_error)?;
                self.scraper_scope = scope;
                self.begin_scraper_job(job, false);
                let id = self.web_new_job("Scraping");
                self.web.scrape_job = Some(id);
                Ok(json!({"job":id}))
            }
            WebCommand::GameData { system } => self.web_game_data(&system),
            WebCommand::GameDataLocations {
                system,
                picture_type,
                directory,
            } => self.web_pack_locations(&system, picture_type, directory.as_deref()),
            WebCommand::GameDataSave {
                revision,
                system,
                source,
                location,
                picture_type,
            } => self.web_source_save(
                &revision,
                &system,
                source,
                location.as_deref(),
                picture_type,
            ),
            WebCommand::PackMatches { game, search } => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Unlock kiosk mode to match artwork.",
                    ));
                }
                self.web_start_pack_job(&game, crate::provider_job::Matching::Search(search))
            }
            WebCommand::PackMatchSave {
                revision,
                game,
                choice,
            } => {
                self.web_check_revision(&revision)?;
                let (_, launch, _) = self.web_game_target(&game)?;
                let selected = choice
                    .map(|id| {
                        let (owner, choice) = self
                            .web
                            .pack_choices
                            .get(&id)
                            .ok_or_else(|| web_error("Search the Artwork Pack again first."))?;
                        if owner != &game {
                            return Err(web_error("This match belongs to another game."));
                        }
                        Ok(choice.clone())
                    })
                    .transpose()?;
                self.web_start_pack_job(
                    &game,
                    crate::provider_job::Matching::Save(Box::new(crate::provider_job::MatchSave {
                        launch,
                        selected,
                        settings: self.settings.clone(),
                        settings_path: self.settings_path.clone(),
                    })),
                )
            }
            other => self.web_organize_command(other),
        }
    }

    fn web_game_data(&self, system: &str) -> std::result::Result<JsonValue, WebError> {
        if !self.all_systems.iter().any(|found| found.def.id == system) {
            return Err(web_error("System is unavailable."));
        }
        let group = crate::artwork_pack::source_group(system);
        Ok(
            json!({"system":system,"supports_pack":group.is_some(),"source":if group.is_some_and(|group|self.settings.gamelist_sources.contains(group)){"gamelist"}else if group.is_some_and(|group|self.settings.artwork_pack_roots.contains_key(group)){"artwork-pack"}else{"automatic"},"effective":if self.pack_selected(system){"artwork-pack"}else{"gamelist"},"picture_type":crate::artwork_pack::selected_picture_type(&self.settings,system),"inherit_type":crate::artwork_pack::inherits_picture_type(&self.settings,system),"location":self.effective_artwork_pack_roots.get(system),"error":group.and_then(|group|self.artwork_source_errors.get(group)),"revision":self.web_revision()}),
        )
    }

    fn web_pack_locations(
        &mut self,
        system: &str,
        kind: crate::artwork_pack::PictureType,
        directory: Option<&str>,
    ) -> std::result::Result<JsonValue, WebError> {
        if self.kiosk_locked {
            return Err(WebError::new(
                "locked",
                "Unlock kiosk mode to choose an artwork location.",
            ));
        }
        self.web_game_data(system)?;
        let locations = if let Some(directory) = directory {
            let root = self
                .web
                .pack_directories
                .get(directory)
                .ok_or_else(|| web_error("Refresh the parent directory first."))?;
            artwork_directory_children(root, &self.config.game_roots, &[])
                .map_err(web_error)?
                .into_iter()
                .map(|(_, path)| path)
                .collect()
        } else {
            let mut roots = crate::artwork_pack::discover_candidate_roots_for_type(
                system,
                &self.config.game_roots,
                kind,
            );
            roots.extend(
                self.config
                    .game_roots
                    .iter()
                    .filter_map(|root| Path::new(root).parent().map(Path::to_path_buf))
                    .filter(|root| root.is_dir()),
            );
            roots.sort();
            roots.dedup();
            roots
        };
        let mut items = Vec::new();
        for path in locations {
            let root = chosen_pack_root_for_type(system, &path, &self.config.game_roots, kind)
                .map_err(|path| {
                    web_error(format!(
                        "Artwork directory is outside configured storage: {}",
                        path.display()
                    ))
                })?;
            let id = web_id("pack-directory", &path);
            self.web.pack_directories.insert(id.clone(), path.clone());
            items.push(json!({"id":id,"name":path.file_name().unwrap_or_default().to_string_lossy(),"location":path,"selectable":root.is_some()}));
        }
        Ok(json!({"items":items,"directory":directory,"picture_type":kind}))
    }

    fn web_source_save(
        &mut self,
        revision: &str,
        system: &str,
        source: crate::web::api::GameDataSource,
        location: Option<&str>,
        kind: Option<crate::artwork_pack::PictureType>,
    ) -> std::result::Result<JsonValue, WebError> {
        use crate::web::api::GameDataSource;
        self.web_check_revision(revision)?;
        self.web_storage_idle()?;
        self.web_game_data(system)?;
        let group = crate::artwork_pack::source_group(system)
            .ok_or_else(|| web_error("This system uses Gamelist only."))?;
        if matches!(source, GameDataSource::Automatic) {
            let mut settings = self.settings.clone();
            settings.artwork_pack_roots.remove(group);
            settings.artwork_pack_picture_types.remove(group);
            settings.artwork_pack_inherit.remove(group);
            settings.gamelist_sources.remove(group);
            let members: Vec<_> = self
                .all_systems
                .iter()
                .filter(|found| crate::artwork_pack::source_group(&found.def.id) == Some(group))
                .cloned()
                .collect();
            let job = crate::artwork_source::Job::start(
                members,
                settings.clone(),
                self.cache_dir.clone(),
            )
            .map_err(web_error)?;
            let id = self.web_new_job("Resolving automatic artwork source");
            self.web.automatic_job = Some((id, system.into(), revision.into(), settings, job));
            return Ok(json!({"job":id}));
        }
        let inherit_type = kind.is_none();
        let kind = kind.unwrap_or_else(|| self.settings.default_artwork_pack.unwrap_or_default());
        let target = match source {
            GameDataSource::Gamelist => crate::source_cache::Target::Gamelist,
            GameDataSource::ArtworkPack => {
                let path = if let Some(id) = location {
                    self.web
                        .pack_directories
                        .get(id)
                        .cloned()
                        .ok_or_else(|| web_error("Choose an installed Artwork Pack location."))?
                } else {
                    self.effective_artwork_pack_roots
                        .get(system)
                        .map(PathBuf::from)
                        .ok_or_else(|| web_error("Choose an installed Artwork Pack location."))?
                };
                let root = chosen_pack_root_for_type(system, &path, &self.config.game_roots, kind)
                    .map_err(|path| {
                        web_error(format!(
                            "Artwork directory is outside configured storage: {}",
                            path.display()
                        ))
                    })?
                    .ok_or_else(|| web_error("This location has no matching Artwork Pack."))?;
                crate::source_cache::Target::ArtworkPack {
                    docs_root: root,
                    picture_type: kind,
                }
            }
            GameDataSource::Automatic => unreachable!(),
        };
        let systems = self
            .all_systems
            .iter()
            .filter(|found| crate::artwork_pack::source_group(&found.def.id) == Some(group))
            .map(|found| crate::source_cache::System {
                id: found.def.id.clone(),
                name: found.name().into(),
                config: found.to_config(),
            })
            .collect();
        let job = crate::source_cache::start(crate::source_cache::Request {
            target,
            systems,
            names: self.names.clone(),
            synopsis_language: self.scraper_settings.language.clone(),
            cache_dir: self.cache_dir.clone(),
            require_usable_provider: true,
            homes: self.homes(),
            manual_matches: self.settings.artwork_pack_matches.clone(),
        })
        .map_err(web_error)?;
        let id = self.web_new_job("Preparing game data source");
        self.web.source_job = Some((id, group.into(), self.web_revision(), inherit_type, job));
        Ok(json!({"job":id}))
    }

    fn web_start_pack_job(
        &mut self,
        game: &str,
        matching: crate::provider_job::Matching,
    ) -> std::result::Result<JsonValue, WebError> {
        self.web_storage_idle()?;
        let (system_id, _, system_name) = self.web_game_target(game)?;
        let docs_root = self
            .effective_artwork_pack_roots
            .get(&system_id)
            .map(PathBuf::from)
            .ok_or_else(|| web_error("Select Artwork Pack for this system first."))?;
        let job = crate::provider_job::start(vec![crate::provider_job::Request {
            picture_type: crate::artwork_pack::selected_picture_type(&self.settings, &system_id),
            system_id,
            system_name,
            docs_root,
            synopsis_language: self.scraper_settings.language.clone(),
            cache_dir: self.cache_dir.clone(),
            validate_location_only: false,
            cached_provider: None,
            homes: self.homes(),
            write_state: false,
            manual_matches: Default::default(),
            matching: Some(matching),
        }])
        .map_err(web_error)?;
        let id = self.web_new_job("Matching Artwork Pack");
        self.web.pack_job = Some((id, game.into(), self.web_revision(), job));
        Ok(json!({"job":id}))
    }

    fn web_invalidate_source(&mut self, group: &str) {
        self.invalidate_artwork_provider_group(group);
        self.covers = self.fresh_cover_cache();
        self.group_covers = self.fresh_group_cover_cache();
        self.gallery_covers = self.fresh_gallery_cover_cache();
        self.saver_covers = self.fresh_saver_cover_cache();
        self.invalidate_saver_candidates();
        self.saver_pool.clear();
        self.saver_queue.clear();
        if let Some(catalogue) = &mut self.explore.catalogue {
            catalogue.signatures.clear();
        }
        if let Some(open) = self
            .open_system
            .clone()
            .filter(|id| crate::artwork_pack::source_group(id) == Some(group))
        {
            self.reload_open_system_source(&open);
        }
        self.correct_system_counts();
        self.rebuild_system_list();
        self.dirty = true;
    }

    fn web_cancel_data_job(&mut self, id: u64) {
        if let Some((_, _, _, _, job)) = self
            .web
            .automatic_job
            .as_ref()
            .filter(|(job_id, _, _, _, _)| *job_id == id)
        {
            job.cancel();
        }
        if let Some((_, _, job)) = self
            .web
            .search_job
            .as_ref()
            .filter(|(job_id, _, _)| *job_id == id)
        {
            job.cancel();
        }
        if let Some((_, job)) = self
            .web
            .preview_job
            .as_ref()
            .filter(|(job_id, _)| *job_id == id)
        {
            job.cancel();
        }
        if let Some((_, _, _, _, job)) = self
            .web
            .source_job
            .as_ref()
            .filter(|(job_id, _, _, _, _)| *job_id == id)
        {
            job.cancel();
        }
        if let Some((_, _, _, job)) = self
            .web
            .pack_job
            .as_ref()
            .filter(|(job_id, _, _, _)| *job_id == id)
        {
            job.cancel();
        }
        if self.web.scrape_job == Some(id) {
            if let Some(job) = &self.scraper_job {
                job.cancel();
                self.scraper_cancelling = true;
            }
        }
    }

    fn poll_web_data_jobs(&mut self) {
        if let Some(result) = self
            .web
            .automatic_job
            .as_mut()
            .and_then(|(_, _, _, _, job)| job.try_recv())
        {
            let (id, system, revision, settings, _) =
                self.web.automatic_job.take().expect("automatic source job");
            let result = (|| {
                let resolution = result.map_err(web_error)?.ok_or_else(|| {
                    WebError::new(
                        "cancelled",
                        "Automatic source selection cancelled. The previous choice is unchanged.",
                    )
                })?;
                self.web_check_revision(&revision)?;
                let group = crate::artwork_pack::source_group(&system)
                    .ok_or_else(|| web_error("The system's source group changed."))?;
                let saved = self.web_save(settings);
                // The durable write can report a flush error after installation.
                if saved.is_ok()
                    || saved
                        .as_ref()
                        .is_err_and(|error| error.code == "durability")
                {
                    self.apply_resolution(&HashSet::from([group.into()]), &resolution);
                    self.web_invalidate_source(group);
                }
                saved?;
                let mut data = self.web_game_data(&system)?;
                data["warnings"] = json!(resolution.errors.into_values().collect::<Vec<_>>());
                Ok(data)
            })();
            self.web_finish_job(id, result);
        }
        while let Some(event) = self
            .web
            .search_job
            .as_mut()
            .and_then(|(_, _, job)| job.try_recv())
        {
            let (id, game, _) = self.web.search_job.as_ref().expect("search job");
            let id = *id;
            let game = game.clone();
            match event {
                crate::scraper::SearchEvent::Activity(message) => {
                    self.web
                        .jobs
                        .insert(id, json!({"id":id,"state":"running","message":message}));
                }
                crate::scraper::SearchEvent::Finished {
                    matches,
                    account,
                    requests_started,
                    failed_searches,
                } => {
                    self.web.scraper_account = account;
                    self.web.search_job = None;
                    self.web.matches.clear();
                    let items:Vec<_>=matches.into_iter().map(|matched|{
                        let key=web_id("match",&(id,&matched.id));let metadata=&matched.metadata;
                        let value=json!({"id":key,"name":matched.name,"image_available":matched.media.is_some(),"metadata":{"name":metadata.name,"description":metadata.desc,"publisher":metadata.publisher,"developer":metadata.developer,"releasedate":metadata.releasedate,"players":metadata.players,"genre":metadata.genre,"language":metadata.lang}});
                        self.web.matches.insert(key,(game.clone(),matched));value
                    }).collect();
                    self.web_finish_job(id,Ok(json!({"items":items,"requests":requests_started,"failed_searches":failed_searches})));
                }
                crate::scraper::SearchEvent::Cancelled => {
                    self.web.search_job = None;
                    self.web_finish_job(
                        id,
                        Err(WebError::new("cancelled", "Artwork search cancelled.")),
                    );
                }
                crate::scraper::SearchEvent::Failed(error) => {
                    self.web.search_job = None;
                    self.web_finish_job(id, Err(web_error(error)));
                }
            }
        }
        while let Some(event) = self
            .web
            .preview_job
            .as_mut()
            .and_then(|(_, job)| job.try_recv())
        {
            let id = self.web.preview_job.as_ref().expect("preview job").0;
            let result = match event {
                crate::scraper::PreviewEvent::Finished { match_id, image } => Ok(
                    json!({"match_id":match_id,"width":image.width,"height":image.height,"rgb":image.rgb}),
                ),
                crate::scraper::PreviewEvent::Missing { .. } => {
                    Err(web_error("The selected match has no downloadable image."))
                }
                crate::scraper::PreviewEvent::Cancelled => {
                    Err(WebError::new("cancelled", "Artwork preview cancelled."))
                }
                crate::scraper::PreviewEvent::Failed { error, .. } => Err(web_error(error)),
            };
            self.web.preview_job = None;
            self.web_finish_job(id, result);
        }
        if let Some(id) = self.web.scrape_job {
            let progress = &self.scraper_progress;
            let report = json!({"done":progress.completed,"total":progress.total,"updated":progress.updated,"unchanged":progress.unchanged,"not_found":progress.not_found,"ambiguous":progress.ambiguous,"no_media":progress.no_media,"failed":progress.failed,"skipped_artwork_pack":progress.skipped_artwork_pack,"system_errors":progress.system_errors,"problem":progress.last_problem,"unresolved":progress.unresolved_games.iter().map(|game|json!({"name":game.label,"reason":game.reason})).collect::<Vec<_>>()});
            if let Some(terminal) = &self.scraper_terminal {
                let result = match terminal {
                    ScraperTerminal::Finished => Ok(report),
                    ScraperTerminal::Cancelled => Err(WebError::new(
                        "cancelled",
                        format!(
                            "Scrape cancelled after {} of {} games.",
                            progress.completed, progress.total
                        ),
                    )),
                    ScraperTerminal::Failed(error) => Err(web_error(error)),
                };
                self.web.scrape_job = None;
                self.web_finish_job(id, result);
            } else {
                self.web.jobs.insert(
                    id,
                    json!({"id":id,"state":"running","message":progress.current,"progress":report}),
                );
            }
        }
        while let Some(event) = self
            .web
            .source_job
            .as_mut()
            .and_then(|(_, _, _, _, job)| job.try_recv())
        {
            let (id, group, revision, inherit, _) =
                self.web.source_job.as_ref().expect("source job");
            let (id, group, revision, inherit) = (*id, group.clone(), revision.clone(), *inherit);
            match event {
                crate::source_cache::Event::Progress(progress) => {
                    self.web.jobs.insert(id,json!({"id":id,"state":"running","message":progress.current,"done":progress.system,"total":progress.systems,"games":progress.games}));
                }
                crate::source_cache::Event::Cancelled(_) => {
                    self.web.source_job = None;
                    self.web_finish_job(
                        id,
                        Err(WebError::new(
                            "cancelled",
                            "Source change cancelled. The previous source is unchanged.",
                        )),
                    );
                }
                crate::source_cache::Event::Failed { error, .. } => {
                    self.web.source_job = None;
                    self.web_finish_job(id, Err(web_error(error)));
                }
                crate::source_cache::Event::Staged {
                    target,
                    prepared,
                    providers,
                    warnings,
                    ..
                } => {
                    self.web.source_job = None;
                    let result = (|| {
                        self.web_check_revision(&revision)?;
                        let (settings, label) =
                            source_choice(&self.settings, &group, &target, inherit);
                        let (caches, storage_warnings) = prepared
                            .with_settings(&self.settings_path, &settings)
                            .and_then(crate::cache::PreparedCacheGroup::install)
                            .map_err(web_error)?;
                        self.settings = settings;
                        match &target {
                            crate::source_cache::Target::Gamelist => self.clear_group_roots(&group),
                            crate::source_cache::Target::ArtworkPack { docs_root, .. } => {
                                self.set_group_root(&group, &docs_root.to_string_lossy())
                            }
                        }
                        let index_result = if let Some(index) = self.index.as_mut() {
                            update_index_summaries(index, &self.all_systems, &caches);
                            crate::cache::save_index(&self.cache_dir, index).map_err(web_error)
                        } else {
                            Ok(())
                        };
                        self.web_invalidate_source(&group);
                        self.store_source_providers(providers);
                        self.artwork_source_errors.remove(&group);
                        self.source_recovery_suppressed.remove(&group);
                        index_result?;
                        Ok(
                            json!({"source":label,"warnings":warnings.into_iter().chain(storage_warnings).collect::<Vec<_>>(),"revision":self.web_revision()}),
                        )
                    })();
                    self.web_finish_job(id, result);
                }
            }
        }
        while let Some(event) = self
            .web
            .pack_job
            .as_mut()
            .and_then(|(_, _, _, job)| job.try_recv())
        {
            let (id, game, revision, _) = self.web.pack_job.as_ref().expect("pack job");
            let (id, game, revision) = (*id, game.clone(), revision.clone());
            match event {
                crate::provider_job::Event::Progress(progress) => {
                    self.web.jobs.insert(
                        id,
                        json!({"id":id,"state":"running","message":progress.current}),
                    );
                }
                crate::provider_job::Event::MatchChoices { choices, .. } => {
                    self.web.pack_job = None;
                    self.web.pack_choices.clear();
                    let items:Vec<_>=choices.into_iter().map(|choice|{
                        let key=web_id("pack-match",&(id,&choice.reference));let image=self.web_art(choice.presentation.cover);
                        let value=json!({"id":key,"name":choice.label,"image":image,"details":choice.presentation.details,"genre":choice.presentation.genre});
                        self.web.pack_choices.insert(key,(game.clone(),choice.reference));value
                    }).collect();
                    self.web_finish_job(
                        id,
                        Ok(json!({"items":items,"revision":self.web_revision()})),
                    );
                }
                crate::provider_job::Event::MatchStaged {
                    provider,
                    prepared,
                    settings,
                    warnings,
                } => {
                    self.web.pack_job = None;
                    let result = (|| {
                        self.web_check_revision(&revision)?;
                        let (_, storage_warnings) = prepared.install().map_err(web_error)?;
                        self.settings = *settings;
                        if let Some(group) = crate::artwork_pack::source_group(&provider.system_id)
                        {
                            self.web_invalidate_source(group);
                        }
                        self.artwork_provider_cache
                            .insert(provider.system_id.clone(), *provider);
                        Ok(
                            json!({"saved":true,"warnings":warnings.into_iter().chain(storage_warnings).collect::<Vec<_>>(),"revision":self.web_revision()}),
                        )
                    })();
                    self.web_finish_job(id, result);
                }
                crate::provider_job::Event::Cancelled(_) => {
                    self.web.pack_job = None;
                    self.web_finish_job(
                        id,
                        Err(WebError::new("cancelled", "Artwork matching cancelled.")),
                    );
                }
                crate::provider_job::Event::Failed { error, .. } => {
                    self.web.pack_job = None;
                    self.web_finish_job(id, Err(web_error(error)));
                }
                crate::provider_job::Event::Loaded { .. } => {
                    self.web.pack_job = None;
                    self.web_finish_job(
                        id,
                        Err(web_error(
                            "Artwork matching returned an unexpected provider result.",
                        )),
                    );
                }
            }
        }
    }
}
