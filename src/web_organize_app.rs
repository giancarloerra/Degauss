impl App {
    fn web_upload(
        &mut self,
        revision: &str,
        target: &str,
        body: Vec<u8>,
    ) -> std::result::Result<JsonValue, WebError> {
        if self.kiosk_locked {
            return Err(WebError::new(
                "locked",
                "Unlock kiosk mode before importing artwork.",
            ));
        }
        self.web_check_revision(revision)?;
        self.web_storage_idle()?;
        if body.len() > crate::scraper::MAX_UPLOAD_BYTES {
            return Err(web_error("Artwork must be no larger than 16 MiB."));
        }
        let destination = if let Some(entry) = target.strip_prefix("entry:") {
            if !self.settings.home.entries.contains_key(entry) {
                return Err(web_error("The Home entry no longer exists."));
            }
            crate::web::upload::Destination::Logo(
                self.logo_dir
                    .clone()
                    .ok_or_else(|| web_error("The logos folder is unavailable."))?,
            )
        } else {
            let selected = self
                .web
                .targets
                .get(target)
                .ok_or_else(|| web_error("Refresh the selected page first."))?;
            match &selected.target {
                crate::home::Target::Game { .. } => {
                    let (system, launch, title) = self.web_game_target(target)?;
                    if self.pack_selected(&system) {
                        return Err(web_error("This game uses Artwork Pack. Choose Gamelist explicitly before uploading replacement artwork."));
                    }
                    crate::web::upload::Destination::Game {
                        systems: self.all_systems.clone(),
                        system,
                        launch,
                        title,
                        excluded: self.artwork_pack_scraper_exclusions(),
                    }
                }
                crate::home::Target::Category { .. } | crate::home::Target::System { .. } => {
                    crate::web::upload::Destination::Logo(
                        self.logo_dir
                            .clone()
                            .ok_or_else(|| web_error("The logos folder is unavailable."))?,
                    )
                }
                _ => {
                    return Err(web_error(
                        "Choose a game, system, category or personal Home entry.",
                    ))
                }
            }
        };
        let job = crate::web::upload::Job::start(destination, body).map_err(web_error)?;
        let id = self.web_new_job("Importing artwork");
        self.web.upload_job = Some((id, target.into(), revision.into(), job));
        Ok(json!({"job":id}))
    }

    fn poll_web_upload(&mut self) {
        let Some(result) = self
            .web
            .upload_job
            .as_ref()
            .and_then(|(_, _, _, job)| job.poll())
        else {
            return;
        };
        let (id, target, revision, _) = self.web.upload_job.take().expect("artwork upload");
        match result {
            Err(error) => self.web_finish_job(id, Err(web_error(error))),
            Ok(crate::web::upload::Saved::Logo(path)) => {
                let image = web_id("media", &path);
                self.web_art(Some(path));
                let result = self.web_organize_command(WebCommand::ImageChoose {
                    revision,
                    target,
                    image: Some(image),
                });
                self.web_finish_job(id, result);
            }
            Ok(crate::web::upload::Saved::Game(systems)) => {
                self.scraper_progress = crate::scraper::Progress {
                    updated: 1,
                    completed: 1,
                    total: 1,
                    updated_systems: systems,
                    scope: "Uploaded artwork".into(),
                    ..Default::default()
                };
                self.scraper_terminal = None;
                self.scraper_open_matches_after_refresh = false;
                self.web.scrape_job = Some(id);
                self.begin_scraper_finish(ScraperTerminal::Finished);
            }
        }
    }

    fn web_favorite_folders(&mut self) -> std::result::Result<JsonValue, WebError> {
        let root = self.favorites_root();
        let mut items = vec![json!({"id":JsonValue::Null,"name":"Main Favourites"})];
        for name in crate::favorites::folders(&root).map_err(web_error)? {
            let path = root.join(&name);
            let id = web_id("favorite-folder", &path);
            self.web.favorite_folders.insert(id.clone(), path);
            items.push(json!({"id":id,"name":name}));
        }
        Ok(json!({"items":items}))
    }

    fn web_favorite_folder(&self, id: Option<&str>) -> std::result::Result<PathBuf, WebError> {
        let root = self.favorites_root();
        let Some(id) = id else { return Ok(root) };
        let path = self
            .web
            .favorite_folders
            .get(id)
            .ok_or_else(|| web_error("Refresh the favourite folder list first."))?;
        if !crate::favorites::is_user_folder(&root, path) {
            return Err(web_error("This favourite folder is no longer available."));
        }
        Ok(path.clone())
    }

    fn web_refresh_favorites(&mut self) -> std::result::Result<(), WebError> {
        self.reread_favorites();
        let error = self.refresh_favorites_system();
        self.relist_here_preserving_game_filters();
        if let Some(error) = error {
            return Err(WebError::new(
                "refresh",
                format!("Favourites were changed, but refreshing their list failed: {error}"),
            ));
        }
        Ok(())
    }

    fn web_organize_command(
        &mut self,
        command: WebCommand,
    ) -> std::result::Result<JsonValue, WebError> {
        match command {
            WebCommand::FavoriteFolders => self.web_favorite_folders(),
            WebCommand::FavoriteFolderCreate { name } => {
                self.web_storage_idle()?;
                crate::favorites::make_folder(&self.favorites_root(), &name).map_err(web_error)?;
                self.web_refresh_favorites()?;
                self.web_favorite_folders()
            }
            WebCommand::FavoriteFolderRename { folder, name } => {
                self.web_storage_idle()?;
                let path = self.web_favorite_folder(Some(&folder))?;
                crate::favorites::rename_folder(&self.favorites_root(), &path, &name)
                    .map_err(web_error)?;
                self.web.favorite_folders.remove(&folder);
                self.web_refresh_favorites()?;
                self.web_favorite_folders()
            }
            WebCommand::FavoriteFolderDelete { folder } => {
                self.web_storage_idle()?;
                let path = self.web_favorite_folder(Some(&folder))?;
                crate::favorites::remove_empty_folder(&self.favorites_root(), &path)
                    .map_err(web_error)?;
                self.web.favorite_folders.remove(&folder);
                self.web_refresh_favorites()?;
                self.web_favorite_folders()
            }
            WebCommand::FavoriteAdd { game, folder, name } => {
                self.web_storage_idle()?;
                let folder = self.web_favorite_folder(folder.as_deref())?;
                let (system, launch, title) = self.web_game_target(&game)?;
                let found = self
                    .all_systems
                    .iter()
                    .find(|found| found.def.id == system)
                    .ok_or_else(|| web_error("The game's system is unavailable."))?;
                let config = found.to_config();
                let family = crate::game_launch_cores::effective(&self.settings, &system, &launch);
                let version = self.settings.core_choices.get(&system).map(String::as_str);
                let ra = self.settings.core_preference.unwrap_or_default()
                    == crate::settings::CorePreference::RetroAchievementsFirst;
                let root = Path::new(&self.config.menu_root);
                let (mgl, original, extension, core) = match &launch {
                    browse::Launch::AmigaVision { install, title } => {
                        let mgl = crate::launch::favorite_mgl_amiga_with_selections(
                            &config, install, title, root, ra, version, family,
                        )
                        .map_err(web_error)?;
                        (Some(mgl), sanitise(title), ".mgl".to_string(), None)
                    }
                    browse::Launch::File(path) => {
                        let mgl = crate::launch::favorite_mgl_with_selections(
                            &config, path, root, ra, version, family,
                        )
                        .map_err(web_error)?;
                        if mgl.is_some() {
                            (
                                mgl,
                                crate::favorites::favorite_name(&title, path),
                                ".mgl".to_string(),
                                None,
                            )
                        } else {
                            (
                                None,
                                path.file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                                path.extension()
                                    .map(|extension| format!(".{}", extension.to_string_lossy()))
                                    .unwrap_or_default(),
                                Some(path.clone()),
                            )
                        }
                    }
                };
                let stem = name.as_deref().map(str::trim).unwrap_or(&original);
                if !crate::favorites::name_is_usable(stem)
                    || stem.chars().any(char::is_control)
                    || stem.chars().count() + extension.chars().count() > 255
                {
                    return Err(web_error("Choose a valid favourite filename of at most 255 characters including its extension."));
                }
                if std::fs::symlink_metadata(folder.join(format!("{stem}{extension}"))).is_ok() {
                    return Err(WebError::new(
                        "name-conflict",
                        format!("{stem}{extension} already exists. Choose another name."),
                    ));
                }
                if let Some(mgl) = mgl {
                    crate::favorites::add_game(&folder, stem, &mgl).map_err(web_error)?;
                } else if let Some(core) = core {
                    if needs_native_arcade_core_link(Some(found), &core) {
                        crate::favorites::ensure_arcade_cores_link(root).map_err(web_error)?;
                    }
                    crate::favorites::add_core(&folder, &format!("{stem}{extension}"), &core)
                        .map_err(web_error)?;
                }
                self.web_refresh_favorites()?;
                Ok(json!({"saved":true}))
            }
            WebCommand::FavoriteRemove { game } => {
                self.web_storage_idle()?;
                let target = self
                    .web
                    .targets
                    .get(&game)
                    .ok_or_else(|| web_error("Refresh the game list first."))?;
                let crate::home::Target::Game { system, launch } = &target.target else {
                    return Err(web_error("Choose a favourite game."));
                };
                let selected = match launch {
                    browse::Launch::File(path) => path.clone(),
                    browse::Launch::AmigaVision { install, title } => {
                        crate::favorites::amiga_key(install, title)
                    }
                };
                let in_favorites = self
                    .all_systems
                    .iter()
                    .any(|found| found.def.id == *system && is_favorites(found.category()));
                let file = if in_favorites {
                    selected
                } else {
                    self.favorites
                        .file_for(&selected)
                        .ok_or_else(|| web_error("This game is not a favourite."))?
                        .to_path_buf()
                };
                let root = self.favorites_root();
                if !file.starts_with(&root)
                    || file.parent().is_none_or(|parent| {
                        parent != root && !crate::favorites::is_user_folder(&root, parent)
                    })
                {
                    return Err(web_error(
                        "The selected file is not a manageable favourite.",
                    ));
                }
                crate::favorites::remove(&file).map_err(web_error)?;
                self.web_refresh_favorites()?;
                Ok(json!({"removed":true}))
            }
            WebCommand::Images => {
                let directory = self
                    .logo_dir
                    .as_deref()
                    .ok_or_else(|| web_error("The logos folder is unavailable."))?;
                let images = crate::category_images::choices(directory).map_err(web_error)?;
                let items: Vec<_> = images
                    .into_iter()
                    .map(|choice| {
                        let id = web_id("media", &choice.path);
                        let image = self.web_art(Some(choice.path));
                        json!({"id":id,"name":choice.label,"image":image})
                    })
                    .collect();
                Ok(json!({"items":items,"revision":self.web_revision()}))
            }
            WebCommand::ImageChoose {
                revision,
                target,
                image,
            } => {
                self.web_check_revision(&revision)?;
                let source = image
                    .map(|id| {
                        self.web
                            .media
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| web_error("Refresh the image list first."))
                    })
                    .transpose()?;
                let directory = self
                    .logo_dir
                    .as_deref()
                    .ok_or_else(|| web_error("The logos folder is unavailable."))?;
                if let Some(source) = &source {
                    if source.parent() != Some(directory) {
                        return Err(web_error("Choose an image from the logos folder."));
                    }
                }
                if let Some(id) = target.strip_prefix("entry:") {
                    let mut settings = self.settings.clone();
                    let entry = settings
                        .home
                        .entries
                        .get_mut(id)
                        .ok_or_else(|| web_error("This Home entry no longer exists."))?;
                    if let Some(source) = &source {
                        crate::covers::load_scaled(
                            source,
                            1,
                            [0, 0, 0],
                            &mut crate::covers::CoverStats::default(),
                        )
                        .map_err(web_error)?;
                    }
                    entry.image = source;
                    self.web_save(settings)?;
                } else {
                    let target = &self
                        .web
                        .targets
                        .get(&target)
                        .ok_or_else(|| web_error("Refresh the target page first."))?
                        .target;
                    let target = match target {
                        crate::home::Target::System { system } => ImageTarget::System {
                            id: system.clone(),
                            name: system.clone(),
                        },
                        crate::home::Target::Category { category } => {
                            ImageTarget::Category(category.clone())
                        }
                        _ => return Err(web_error("Choose a Home entry, category or system.")),
                    };
                    if let Some(source) = source {
                        target.install(directory, &source).map_err(web_error)?;
                    } else {
                        target.clear(directory).map_err(web_error)?;
                    }
                }
                self.covers = self.fresh_cover_cache();
                self.group_covers = self.fresh_group_cover_cache();
                self.reroll_category_art();
                self.dirty = true;
                Ok(json!({"saved":true,"revision":self.web_revision()}))
            }
            other => self.web_library_command(other),
        }
    }
}
