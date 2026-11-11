use crate::web::api::{
    Command as WebCommand, Error as WebError, HomeEdit as WebHomeEdit, Reply as WebReply,
};
use serde_json::{json, Value as JsonValue};

#[derive(Default)]
struct WebAccess {
    configured: Option<crate::web::Config>,
    server: Option<crate::web::Server>,
    bridge: Option<crate::web::Bridge>,
    addresses: Vec<std::net::IpAddr>,
    error: Option<String>,
    qr_message: Option<String>,
    qr: Option<(usize, slint::Image)>,
    password_job: Option<std::sync::mpsc::Receiver<std::result::Result<(), String>>>,
    targets: std::collections::BTreeMap<String, WebTarget>,
    media: std::collections::BTreeMap<String, PathBuf>,
    outcome: Option<Outcome>,
    last_event: Option<Instant>,
    library: Option<crate::web::library::Worker>,
    reading: Option<u64>,
    next_job: u64,
    jobs: std::collections::BTreeMap<u64, JsonValue>,
    script_folders: std::collections::BTreeMap<String, PathBuf>,
    core_job: Option<(u64, crate::misterzine::Job)>,
    core_snapshot: Option<crate::misterzine::Snapshot>,
    core_visit: Option<crate::core_changes::Visit>,
    core_details_job: Option<(u64, crate::misterzine::Item, crate::misterzine::GameJob)>,
    ini_job: Option<(u64, crate::ini_profile::Job)>,
    ini_snapshot: Option<crate::ini_profile::Snapshot>,
    controller_job: Option<u64>,
    scrape_job: Option<u64>,
    search_job: Option<(u64, String, crate::scraper::SearchJob)>,
    preview_job: Option<(u64, crate::scraper::PreviewJob)>,
    matches: std::collections::BTreeMap<String, (String, crate::scraper::Match)>,
    scraper_account: Option<crate::scraper::Account>,
    search_settings: Option<String>,
    source_job: Option<(u64, String, String, bool, crate::source_cache::Job)>,
    pack_job: Option<(u64, String, String, crate::provider_job::Job)>,
    pack_choices: std::collections::BTreeMap<String, (String, crate::artwork_pack::ManualMatch)>,
    pack_directories: std::collections::BTreeMap<String, PathBuf>,
    favorite_folders: std::collections::BTreeMap<String, PathBuf>,
    handle_order: std::collections::VecDeque<(bool, String)>,
    rebuild_job: Option<u64>,
    upload_job: Option<(u64, String, String, crate::web::upload::Job)>,
    option_job: Option<u64>,
    automatic_job: Option<(u64, String, String, Settings, crate::artwork_source::Job)>,
    pin_after: Option<Instant>,
}

#[derive(Clone)]
struct WebTarget {
    target: crate::home::Target,
    row: Option<browse::Row>,
    home_entry: Option<String>,
}

fn web_id<T: serde::Serialize>(kind: &str, value: &T) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).expect("serializable web identity");
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{kind}-{digest}")
}

fn web_error(error: impl std::fmt::Display) -> WebError {
    WebError::new("operation", error.to_string())
}

fn web_name(name: &str) -> std::result::Result<String, WebError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 255 || name.chars().any(char::is_control) {
        return Err(WebError::new(
            "name",
            "Use a name of 1 to 255 visible characters.",
        ));
    }
    Ok(name.into())
}

impl App {
    pub fn stop_web(&mut self) {
        self.web.server = None;
        self.web.bridge = None;
        self.web.library = None;
        self.web.core_job = None;
        self.web.core_details_job = None;
        self.web.source_job = None;
        self.web.pack_job = None;
        self.web.search_job = None;
        self.web.preview_job = None;
        self.web.upload_job = None;
        self.web.ini_job = None;
        self.web.automatic_job = None;
        self.web.reading = None;
        for job in self
            .web
            .jobs
            .values_mut()
            .filter(|job| job["state"] == "running")
        {
            job["state"] = json!("cancelled");
            job["error"] = json!({"code":"cancelled","message":"Web Access stopped."});
        }
    }

    fn web_directory(&self) -> PathBuf {
        self.settings_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("web-access")
    }

    fn web_revision(&self) -> String {
        let themes: Vec<_> = self
            .themes
            .iter()
            .map(|theme| (&theme.name, theme.file.to_toml(&self.config.colors)))
            .collect();
        let scraper_stamp = std::fs::metadata(crate::scraper::ScraperSettings::path_beside(
            &self.settings_path,
        ))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
        web_id("settings", &(&self.settings, themes, scraper_stamp))
    }

    fn web_check_revision(&self, revision: &str) -> std::result::Result<(), WebError> {
        if revision != self.web_revision()
            || self.home.edit.is_some()
            || self.theme_editor.is_some()
            || self.custom_view.editor.is_some()
            || self.controllers.menu == Some(ControllerMenu::Editor)
        {
            Err(WebError::new("conflict", "Settings changed, or an editor is open on MiSTer. Refresh before saving; the draft has not been changed."))
        } else {
            Ok(())
        }
    }

    fn web_configure(
        &mut self,
        revision: &str,
        config: crate::web::Config,
        password: Option<String>,
    ) -> crate::web::api::Result {
        if self.kiosk_locked {
            return Err(WebError::new(
                "locked",
                "Unlock kiosk mode before changing Web Access.",
            ));
        }
        self.web_check_revision(revision)?;
        self.web_storage_idle()?;
        config.validate().map_err(web_error)?;
        let mut settings = self.settings.clone();
        settings.web_access = Some(config.clone());
        if let Some(prepared) = password {
            // A directory flush may fail after the new password was installed.
            // Reload authentication on either outcome, never retain old sessions.
            self.web.configured = None;
            crate::web::install_password(&self.web_directory(), &prepared).map_err(web_error)?;
        }
        if config.mode != crate::web::Mode::Off
            && !crate::web::password_is_set(&self.web_directory())
        {
            return Err(web_error("Set a password before enabling Web Access."));
        }
        self.web_save(settings)?;
        Ok(WebReply::Json(
            json!({"accepted":true,"reconnect":true,"addresses":self.web.addresses.iter().map(|ip|config.address(*ip)).collect::<Vec<_>>()}),
        ))
    }

    fn web_save(&mut self, settings: Settings) -> std::result::Result<(), WebError> {
        let saved = settings.save(&self.settings_path).map_err(web_error)?;
        self.settings = settings;
        self.rebuild_system_list();
        self.rebuild_home_rows();
        self.global_layout = self
            .settings
            .layout
            .as_deref()
            .and_then(Layout::parse)
            .unwrap_or(Layout::Details);
        self.resolve_view();
        self.apply_geometry();
        self.dirty = true;
        if let SaveOutcome::InstalledWithWarning(error) = saved {
            return Err(WebError::new(
                "durability",
                format!("Changes are installed, but flushing settings failed: {error}"),
            ));
        }
        Ok(())
    }

    fn open_web_option(&mut self, option: OptionId) {
        if option != OptionId::WebAddress {
            if let Err(error) = self.web_storage_idle() {
                self.message = Some(error.message);
                return;
            }
        }
        match option {
            OptionId::WebPassword => {
                self.open_name_keyboard(NamePurpose::WebPassword, String::new())
            }
            OptionId::WebPort => self.open_name_keyboard(
                NamePurpose::WebPort,
                self.settings
                    .web_access
                    .clone()
                    .unwrap_or_default()
                    .port
                    .to_string(),
            ),
            OptionId::WebAddress => {
                let config = self.settings.web_access.clone().unwrap_or_default();
                match crate::web::local_addresses() {
                    Ok(addresses) => {
                        let addresses: Vec<_> =
                            addresses.into_iter().filter(|ip| ip.is_ipv4()).collect();
                        if self.web.addresses != addresses {
                            self.web.addresses = addresses;
                            self.web.configured = None;
                        }
                    }
                    Err(error) => self.web.error = Some(error),
                }
                let mut message = if config.mode == crate::web::Mode::Off {
                    "Web Access is Off. Set a password, then choose HTTPS or HTTP.".into()
                } else if let Some(error) = &self.web.error {
                    error.clone()
                } else if self.web.addresses.is_empty() {
                    "No network address. Connect Ethernet or Wi-Fi, then open Web Address again."
                        .into()
                } else {
                    self.web
                        .addresses
                        .iter()
                        .map(|ip| config.address(*ip))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                self.web.qr = None;
                if let Some(ip) =
                    self.web.addresses.first().filter(|_| {
                        config.mode != crate::web::Mode::Off && self.web.error.is_none()
                    })
                {
                    let safe_width = (self.width as f32
                        - 2.0 * (self.geometry.inset_x + self.geometry.pad))
                        .max(0.0) as usize;
                    let max = (self.height as usize)
                        .saturating_sub(100)
                        .min(safe_width)
                        .min(240);
                    match crate::web::qr_pixels(&config.address(*ip), max) {
                        Ok((side, pixels)) => {
                            let buffer =
                                slint::SharedPixelBuffer::<slint::Rgb8Pixel>::clone_from_slice(
                                    &pixels,
                                    side as u32,
                                    side as u32,
                                );
                            self.web.qr = Some((side, slint::Image::from_rgb8(buffer)));
                        }
                        Err(error) => {
                            message.push('\n');
                            message.push_str(&error);
                        }
                    }
                }
                self.web.qr_message = Some(message.clone());
                self.message = Some(message);
            }
            OptionId::WebCertificate => {
                self.web.server = None;
                let path = self.web_directory().join("identity.json");
                let result = if path.exists() {
                    std::fs::remove_file(&path)
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    self.message = Some(format!(
                        "Removing the old HTTPS certificate failed: {error}"
                    ));
                } else {
                    self.web.configured = None;
                    self.message = Some("HTTPS certificate renewed on the next server start. Trust it again in your browser.".into());
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn update_web_qr(&self) {
        if self.message.is_some() && self.message == self.web.qr_message {
            if let Some((side, image)) = &self.web.qr {
                self.ui.set_web_qr(image.clone());
                self.ui.set_web_qr_side(*side as i32);
                return;
            }
        }
        self.ui.set_web_qr_side(0);
    }

    fn step_web_mode(&mut self, delta: isize) {
        if let Err(error) = self.web_storage_idle() {
            self.message = Some(error.message);
            return;
        }
        let mut config = self.settings.web_access.clone().unwrap_or_default();
        let choices = [
            crate::web::Mode::Off,
            crate::web::Mode::Https,
            crate::web::Mode::Http,
        ];
        let index = choices
            .iter()
            .position(|mode| *mode == config.mode)
            .unwrap_or(0);
        config.mode = choices[step(index, delta, choices.len())];
        if config.mode != crate::web::Mode::Off
            && !crate::web::password_is_set(&self.web_directory())
        {
            self.message = Some("Set a Web Password before enabling Web Access.".into());
            return;
        }
        self.settings.web_access = Some(config);
        self.save_settings();
    }

    fn finish_web_port(&mut self, value: &str) {
        if let Err(error) = self.web_storage_idle() {
            self.message = Some(error.message);
            return;
        }
        match value.parse::<u16>().ok().filter(|port| *port >= 1024) {
            Some(port) => {
                self.settings
                    .web_access
                    .get_or_insert_with(Default::default)
                    .port = port;
                self.save_settings();
                self.open_options_page(OptionsPage::WebAccess);
            }
            None => self.message = Some("Choose a port from 1024 to 65535.".into()),
        }
    }

    fn finish_web_password(&mut self, value: &str) {
        if let Err(error) = self.web_storage_idle() {
            self.message = Some(error.message);
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let directory = self.web_directory();
        let password = value.to_string();
        self.name_keyboard_draft.clear();
        match std::thread::Builder::new()
            .name("web-password".into())
            .spawn(move || {
                let _ = sender.send(crate::web::set_password(&directory, &password));
            }) {
            Ok(_) => {
                self.web.password_job = Some(receiver);
                self.open_options_page(OptionsPage::WebAccess);
                self.message = Some("Saving web password…".into());
            }
            Err(error) => self.message = Some(format!("Starting password save failed: {error}")),
        }
    }

    fn poll_web(&mut self) -> Option<Outcome> {
        if let Some(result) = self
            .web
            .password_job
            .as_ref()
            .and_then(|job| match job.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                    "The password worker disconnected before returning its result.".into(),
                )),
            })
        {
            self.web.password_job = None;
            self.web.configured = None;
            match result {
                Ok(()) => {
                    self.web.server = None;
                    self.web.configured = None;
                    self.message = Some(
                        "Web password saved. Previous browser sessions are signed out.".into(),
                    );
                }
                Err(error) => self.message = Some(error),
            }
            self.dirty = true;
        }
        let config = self.settings.web_access.clone().unwrap_or_default();
        if self.web.configured.as_ref() != Some(&config) {
            self.web.server = None;
            self.web.bridge = None;
            self.web.error = None;
            self.web.configured = Some(config.clone());
            if config.mode != crate::web::Mode::Off {
                match crate::web::local_addresses() {
                    Ok(addresses) => {
                        self.web.addresses =
                            addresses.into_iter().filter(|ip| ip.is_ipv4()).collect();
                        let bridge = crate::web::Bridge::default();
                        match crate::web::Server::start(
                            &config,
                            self.web_directory(),
                            &self.web.addresses,
                            &bridge,
                        ) {
                            Ok(server) => {
                                self.web.server = Some(server);
                                self.web.bridge = Some(bridge);
                            }
                            Err(error) => self.web.error = Some(error),
                        }
                    }
                    Err(error) => self.web.error = Some(error),
                }
            } else {
                self.stop_web();
            }
            if let Some(error) = &self.web.error {
                self.message = Some(error.clone());
                self.dirty = true;
            }
        }
        self.poll_web_jobs();
        if let Some(server) = &self.web.server {
            let status = server.status.lock().expect("web status").clone();
            if let crate::web::Status::Failed(error) = status {
                if self.web.error.as_ref() != Some(&error) {
                    self.message = Some(error.clone());
                    self.web.error = Some(error);
                    self.dirty = true;
                }
            } else if matches!(status, crate::web::Status::Listening) {
                self.web.error = None;
            }
        }
        // A request whose browser timed out must never perform a delayed mutation.
        if let Some(request) = self
            .web
            .bridge
            .as_ref()
            .and_then(|bridge| bridge.requests.try_recv().ok())
        {
            if !request.reply.is_closed() {
                let result = match request.request {
                    crate::web::Request::Command(command) => self.web_command(*command),
                    crate::web::Request::Configure {
                        revision,
                        config,
                        password,
                    } => self.web_configure(&revision, config, password),
                    crate::web::Request::Upload {
                        revision,
                        target,
                        body,
                    } => self
                        .web_upload(&revision, &target, body)
                        .map(WebReply::Json),
                    crate::web::Request::Media(id) => self.web_media(&id),
                };
                let _ = request.reply.send(result);
            }
        }
        if self
            .web
            .bridge
            .as_ref()
            .is_some_and(|bridge| bridge.has_listeners())
            && self
                .web
                .last_event
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(1))
        {
            self.web.last_event = Some(Instant::now());
            if let Ok(WebReply::Json(mut value)) = self.web_command(WebCommand::State) {
                value["type"] = json!("state");
                if let Some(bridge) = &self.web.bridge {
                    bridge.event(&value);
                }
            }
        }
        self.web.outcome.take()
    }

    fn web_register(&mut self, target: crate::home::Target, row: Option<browse::Row>) -> String {
        let id = web_id("target", &target);
        let row = row.or_else(|| {
            self.web
                .targets
                .get(&id)
                .and_then(|entry| entry.row.clone())
        });
        if !self.web.targets.contains_key(&id) {
            self.web_remember_handle(false, &id);
        }
        self.web.targets.insert(
            id.clone(),
            WebTarget {
                target,
                row,
                home_entry: None,
            },
        );
        id
    }

    fn web_remember_handle(&mut self, media: bool, id: &str) {
        const MAX_HANDLES: usize = 8192;
        while self.web.handle_order.len() >= MAX_HANDLES {
            if let Some((media, key)) = self.web.handle_order.pop_front() {
                if media {
                    self.web.media.remove(&key);
                } else {
                    self.web.targets.remove(&key);
                }
            }
        }
        self.web.handle_order.push_back((media, id.to_string()));
    }

    fn web_art(&mut self, path: Option<PathBuf>) -> Option<String> {
        path.map(|path| {
            let id = web_id("media", &path);
            if !self.web.media.contains_key(&id) {
                self.web_remember_handle(true, &id);
            }
            self.web.media.insert(id.clone(), path);
            format!("/api/v1/media/{id}")
        })
    }

    fn web_media(&self, id: &str) -> crate::web::api::Result {
        let path = self.web.media.get(id).ok_or_else(|| {
            WebError::new(
                "missing",
                "This artwork is no longer in the current browser. Refresh its page.",
            )
        })?;
        Ok(WebReply::Media { path: path.clone() })
    }

    fn web_command(&mut self, command: WebCommand) -> crate::web::api::Result {
        if self.kiosk_locked
            && command.changes()
            && !matches!(
                command,
                WebCommand::Unlock { .. }
                    | WebCommand::Launch { .. }
                    | WebCommand::Remote { .. }
                    | WebCommand::Random { .. }
                    | WebCommand::Attract
            )
        {
            return Err(WebError::new(
                "locked",
                "Unlock kiosk mode with its PIN before changing Degauss.",
            ));
        }
        let value = match command {
            WebCommand::WebSettings => {
                let config = self.settings.web_access.clone().unwrap_or_default();
                json!({"addresses":self.web.addresses.iter().map(|ip|config.address(*ip)).collect::<Vec<_>>(),"config":config,"revision":self.web_revision(),"error":self.web.error})
            }
            WebCommand::WebSettingsSave { .. } => {
                return Err(web_error(
                    "Password preparation must use the authenticated HTTP endpoint.",
                ))
            }
            WebCommand::WebCertificateRenew => {
                self.web_storage_idle()?;
                let path = self.web_directory().join("identity.json");
                if path.exists() {
                    std::fs::remove_file(path).map_err(web_error)?;
                }
                self.web.configured = None;
                json!({"accepted":true,"reconnect":true})
            }
            WebCommand::IniProfiles => {
                if let Some((id, _)) = &self.web.ini_job {
                    json!({"job":id})
                } else {
                    let job = crate::ini_profile::Job::start(Path::new(crate::launch::CMD_FIFO))
                        .map_err(web_error)?;
                    let id = self.web_new_job("Reading MiSTer INI profiles");
                    self.web.ini_job = Some((id, job));
                    json!({"job":id})
                }
            }
            WebCommand::IniSwitch { slot } => {
                self.web_storage_idle()?;
                let snapshot =
                    self.web.ini_snapshot.as_ref().ok_or_else(|| {
                        web_error("Read the current INI profiles before switching.")
                    })?;
                if let Some(problem) =
                    ini_slot_problem(Some(snapshot.current), &snapshot.profiles, slot)
                {
                    return Err(web_error(problem));
                }
                if !self.save_settings() {
                    return Err(web_error(
                        self.message.as_deref().unwrap_or("Saving settings failed."),
                    ));
                }
                self.web.outcome = Some(Outcome::SwitchIni(slot));
                json!({"accepted":true,"state":"launching"})
            }
            WebCommand::State => {
                let palette = self.effective_palette();
                let color = |c: ConfigColor| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b);
                json!({"version":env!("CARGO_PKG_VERSION"),"revision":self.web_revision(),"locked":self.kiosk_locked,"font":self.font.label(),"text_size":self.text_size.label(),"crt_images":self.settings.web_crt_images.unwrap_or(false),"native":{"screen":format!("{:?}",self.screen),"system":self.open_system,"selected":self.browse_row(self.game_list.selected()).map(|r|&r.name)},"theme":{"background":color(palette.background),"panel":color(palette.panel),"surface":color(palette.surface),"bar":color(palette.bar),"text":color(palette.text),"text_dim":color(palette.text_dim),"accent":color(palette.accent),"accent_text":color(palette.accent_text),"state":color(palette.state),"favorite":color(palette.favorite)}})
            }
            WebCommand::Systems => {
                let systems: Vec<_> = self
                    .all_systems
                    .iter()
                    .filter(|system| {
                        (!self.kiosk_locked || self.kiosk_category_allowed(system.category()))
                            && (self.show_hidden || !self.settings.hidden.contains(&system.def.id))
                    })
                    .cloned()
                    .collect();
                let rows: Vec<_> = systems.iter().map(|system| {
                    let id = self.web_register(crate::home::Target::System { system: system.def.id.clone() }, None);
                    let image = self.web_art(self.system_logo(system));
                    json!({"id":id,"system":system.def.id,"name":system.name(),"category":display_category(system,self.settings.separate_handheld_category.unwrap_or(false)),"image":image,"games":self.index.as_ref().and_then(|index| index.systems.get(&system.def.id)).map(|s|s.games)})
                }).collect();
                json!({"items":rows})
            }
            WebCommand::Home { folder } => self.web_home(folder.as_deref())?,
            WebCommand::HomeArtwork { folder } => {
                let data = self.web_home(folder.as_deref())?;
                let mut targets =
                    std::collections::BTreeMap::<String, Vec<(String, browse::Launch)>>::new();
                for item in data["items"].as_array().expect("Home items") {
                    let key = item["key"].as_str().expect("Home key");
                    if item["hidden"] == true || item["available"] == false {
                        continue;
                    }
                    let Some(entry) = key
                        .strip_prefix("entry:")
                        .and_then(|id| self.settings.home.entries.get(id))
                    else {
                        continue;
                    };
                    if entry.image.is_some() {
                        continue;
                    }
                    if let crate::home::Target::Game { system, launch } = &entry.target {
                        targets
                            .entry(system.clone())
                            .or_default()
                            .push((key.into(), launch.clone()));
                    }
                }
                if targets.is_empty() {
                    json!({"images":{},"notices":[]})
                } else {
                    self.web_start_read(
                        crate::web::library::Operation::HomeArtwork { targets },
                        false,
                    )?
                }
            }
            WebCommand::HomeEdit { revision, edit } => {
                self.web_check_revision(&revision)?;
                self.web_edit_home(edit)?;
                self.web_home(None)?
            }
            WebCommand::Collections => {
                let collections = self.settings.collections.clone();
                let targets: std::collections::BTreeMap<_, _> = collections
                    .keys()
                    .map(|id| {
                        (
                            id.clone(),
                            self.web_register(
                                crate::home::Target::Collection {
                                    collection: id.clone(),
                                },
                                None,
                            ),
                        )
                    })
                    .collect();
                json!({"items":collections,"targets":targets,"revision":self.web_revision()})
            }
            WebCommand::CollectionSave {
                revision,
                id,
                name,
                query,
            } => {
                self.web_check_revision(&revision)?;
                let mut settings = self.settings.clone();
                let id = if let Some(id) = id {
                    if !settings.collections.contains_key(&id) {
                        return Err(web_error("The collection no longer exists."));
                    }
                    id
                } else {
                    let (id, next) = crate::explore::next_collection_id(
                        &settings.collections,
                        settings.next_collection,
                    );
                    settings.next_collection = next;
                    id
                };
                settings.collections.insert(
                    id.clone(),
                    crate::explore::Collection {
                        name: web_name(&name)?,
                        query,
                    },
                );
                self.web_save(settings)?;
                json!({"id":id,"revision":self.web_revision()})
            }
            WebCommand::CollectionDelete { revision, id } => {
                self.web_check_revision(&revision)?;
                let mut settings = self.settings.clone();
                settings
                    .collections
                    .remove(&id)
                    .ok_or_else(|| web_error("The collection no longer exists."))?;
                self.web_save(settings)?;
                json!({"revision":self.web_revision()})
            }
            WebCommand::Views { target } => {
                let place = self.web_view_place(target.as_deref())?;
                let current = place
                    .as_ref()
                    .and_then(|place| place.get(&self.settings.custom_views));
                json!({"items":self.settings.view_definitions,"choices":self.view_choices(),"current":current,"global":self.settings.layout,"revision":self.web_revision()})
            }
            WebCommand::ViewSave {
                revision,
                id,
                definition,
            } => {
                self.web_check_revision(&revision)?;
                definition.validate().map_err(web_error)?;
                let mut settings = self.settings.clone();
                let id = match id {
                    Some(id) if settings.view_definitions.contains_key(&id) => id,
                    Some(_) => return Err(web_error("The layout no longer exists.")),
                    None => crate::custom_view::next_id(&settings.view_definitions)
                        .ok_or_else(|| web_error("No layout identities available."))?,
                };
                settings.view_definitions.insert(id.clone(), definition);
                self.web_save(settings)?;
                json!({"id":id,"revision":self.web_revision()})
            }
            WebCommand::ViewDelete { revision, id } => {
                self.web_check_revision(&revision)?;
                if !self.settings.view_definitions.contains_key(&id) {
                    return Err(web_error("The layout no longer exists."));
                }
                let mut settings = self.settings.clone();
                crate::custom_view::remove(&mut settings, &id);
                self.web_save(settings)?;
                json!({"revision":self.web_revision()})
            }
            WebCommand::ViewAssign {
                revision,
                target,
                view,
            } => {
                self.web_check_revision(&revision)?;
                if !(self.view_choices().contains(&view) || view.is_empty() && target.is_some()) {
                    return Err(web_error("Choose an existing layout."));
                }
                let mut settings = self.settings.clone();
                let place = self.web_view_place(target.as_deref())?;
                if let Some(place) = place {
                    if view.is_empty() {
                        place.remove(&mut settings.custom_views);
                    } else {
                        place.set(&mut settings.custom_views, view);
                    }
                } else {
                    settings.layout = Some(view);
                }
                self.web_save(settings)?;
                json!({"revision":self.web_revision()})
            }
            WebCommand::Options => self.web_options(),
            WebCommand::Job { id } => {
                let mut job = self.web.jobs.get(&id).cloned().ok_or_else(|| {
                    WebError::new(
                        "missing",
                        "This operation is no longer retained. Refresh the page.",
                    )
                })?;
                job["cancellable"] =
                    json!(job["state"] == "running" && self.web_job_cancellable(id));
                job
            }
            WebCommand::OptionAdjust {
                revision,
                option,
                delta,
            } => {
                self.web_check_revision(&revision)?;
                self.web_storage_idle()?;
                if self.message.is_some() || self.pending.is_some() {
                    return Err(WebError::new("busy", "Close the message on MiSTer first."));
                }
                if ![-1, 1].contains(&delta) {
                    return Err(web_error("The option step must be -1 or 1."));
                }
                let id = OptionsPage::ALL
                    .iter()
                    .flat_map(|page| page.ids())
                    .find(|id| format!("{id:?}") == option)
                    .copied()
                    .ok_or_else(|| web_error("Unknown option."))?;
                if !matches!(
                    option_operation(id, OptionInput::Next),
                    OptionOperation::Adjust(_)
                ) || matches!(id, OptionId::WebMode)
                {
                    return Err(web_error("This option needs its dedicated editor."));
                }
                self.message = None;
                self.adjust_option_value(id, isize::from(delta));
                let adjustment_error = self.message.clone();
                if !self.save_settings() {
                    return Err(web_error(
                        self.message.as_deref().unwrap_or("Saving settings failed."),
                    ));
                }
                if let Some(error) = adjustment_error {
                    return Err(web_error(error));
                }
                if std::mem::take(&mut self.artwork_default_changed) {
                    self.resolve_artwork_sources(SourceResolutionAction::ArtworkDefaultChanged);
                    let id = self.web_new_job("Applying Artwork Pack default");
                    self.web.option_job = Some(id);
                    json!({"job":id})
                } else {
                    self.web_options()
                }
            }
            WebCommand::Remote { button } => {
                use crate::web::api::RemoteButton::*;
                let action = match button {
                    Up => Action::Up,
                    Down => Action::Down,
                    Left => Action::Slower,
                    Right => Action::Faster,
                    Select => Action::Accept,
                    Back => Action::Quit,
                    Actions => Action::Context,
                    Menu => Action::Menu,
                };
                if let Some(outcome) = self.handle(action) {
                    self.web.outcome = Some(outcome);
                }
                json!({"accepted":true,"leaving":self.web.outcome.is_some()})
            }
            WebCommand::Unlock { pin } => {
                if self
                    .web
                    .pin_after
                    .is_some_and(|after| after > Instant::now())
                {
                    return Err(WebError::new(
                        "rate-limit",
                        "Wait before trying the kiosk PIN again.",
                    ));
                }
                self.web.pin_after = Some(Instant::now() + Duration::from_secs(2));
                if !self
                    .config
                    .kiosk_pin
                    .as_ref()
                    .is_some_and(|expected| expected.matches(&pin))
                {
                    return Err(WebError::new("locked", "Incorrect kiosk PIN."));
                }
                self.kiosk_locked = false;
                self.rebuild_system_list();
                self.dirty = true;
                json!({"locked":false})
            }
            WebCommand::Launch { id } => self.web_launch(&id)?,
            // Remaining operations are implemented below through their shared jobs.
            other => self.web_edit_command(other)?,
        };
        Ok(WebReply::Json(value))
    }

    fn web_options(&self) -> JsonValue {
        let pages: Vec<_> = OptionsPage::ALL.iter().map(|page| {
            let items: Vec<_> = page.ids().iter().map(|id| {
                let help = if *id == OptionId::Theme {
                    "Choose a theme here. Open Themes to edit colours, logo and default text."
                } else { id.help() };
                json!({"id":format!("{id:?}"),"name":id.label(),"help":help,"value":self.option_value(*id),"adjustable":*id != OptionId::WebMode && matches!(option_operation(*id,OptionInput::Next),OptionOperation::Adjust(_))})
            }).collect();
            json!({"name":page.label(),"key":page.key(),"items":items})
        }).collect();
        json!({"pages":pages,"revision":self.web_revision()})
    }

    fn web_home(&mut self, folder: Option<&str>) -> std::result::Result<JsonValue, WebError> {
        if let Some(id) = folder {
            if !matches!(
                self.settings.home.entries.get(id).map(|e| &e.target),
                Some(crate::home::Target::Folder { .. })
            ) {
                return Err(web_error("That personal folder no longer exists."));
            }
        }
        let categories = self.home_editor_categories();
        let keys = self.settings.home.rows(folder, &categories, true);
        let mut items = Vec::new();
        for key in keys {
            let (name, target, image) = if let Some(id) = key.strip_prefix("entry:") {
                let entry = self
                    .settings
                    .home
                    .entries
                    .get(id)
                    .ok_or_else(|| web_error("Home contains a missing entry."))?;
                (
                    entry.name.clone(),
                    entry.target.clone(),
                    entry.image.clone(),
                )
            } else if let Some(category) = key.strip_prefix("category:") {
                (
                    category.to_string(),
                    crate::home::Target::Category {
                        category: category.into(),
                    },
                    self.category_logo(category),
                )
            } else {
                return Err(web_error("Home contains an unknown entry."));
            };
            if self.kiosk_locked && !self.kiosk_target_allowed(&target) {
                continue;
            }
            let folder_id = key
                .strip_prefix("entry:")
                .filter(|_| matches!(target, crate::home::Target::Folder { .. }))
                .map(str::to_owned);
            let target_id = if let Some(id) = folder_id.as_ref() {
                let handle = web_id("home", id);
                if !self.web.targets.contains_key(&handle) {
                    self.web_remember_handle(false, &handle);
                }
                self.web.targets.insert(
                    handle.clone(),
                    WebTarget {
                        target: target.clone(),
                        row: None,
                        home_entry: Some(id.clone()),
                    },
                );
                handle
            } else {
                self.web_register(target.clone(), None)
            };
            let image = image.or_else(|| match &target {
                crate::home::Target::System { system }
                | crate::home::Target::LibraryFolder { system, .. }
                | crate::home::Target::Game { system, .. } => self
                    .all_systems
                    .iter()
                    .find(|found| &found.def.id == system)
                    .and_then(|found| self.system_logo(found)),
                crate::home::Target::Category { category } => self.category_logo(category),
                _ => None,
            });
            let image = self.web_art(image);
            let destination = match &target {
                crate::home::Target::Folder { .. } => json!({"page":"home","folder":folder_id}),
                crate::home::Target::Category { category } => {
                    json!({"page":"category","category":category})
                }
                crate::home::Target::System { system } => json!({"page":"browse","system":system}),
                crate::home::Target::LibraryFolder { system, .. } => {
                    json!({"page":"browse","system":system,"folder":target_id})
                }
                crate::home::Target::Collection { collection } => {
                    json!({"page":"explore","collection":collection})
                }
                crate::home::Target::Game { system, .. } => {
                    json!({"page":"game","system":system,"id":target_id})
                }
                crate::home::Target::Core { .. } | crate::home::Target::Script { .. } => {
                    json!({"page":"launch","id":target_id})
                }
            };
            let hidden = self.settings.home.hidden.contains(&key)
                || key == crate::home::category_key(crate::explore::NAME)
                    && !self.settings.show_explore.unwrap_or(false);
            let available = key
                .strip_prefix("category:")
                .is_none_or(|category| self.categories.iter().any(|(name, _)| name == category));
            let view_editable = self.web_view_place(Some(&target_id)).is_ok();
            items.push(json!({"key":key,"name":name,"target":target_id,"destination":destination,"kind":match target {crate::home::Target::Folder{..}=>"folder",crate::home::Target::Category{..}=>"category",crate::home::Target::Collection{..}=>"collection",_=>"item"},"folder":folder_id,"hidden":hidden,"available":available,"image":image,"view_editable":view_editable}));
        }
        let mut breadcrumbs = Vec::new();
        let mut ancestor = folder.map(str::to_owned);
        while let Some(id) = ancestor {
            let entry = &self.settings.home.entries[&id];
            breadcrumbs.push(json!({"name":entry.name,"folder":id}));
            ancestor = self.settings.home.parent(&id);
        }
        breadcrumbs.reverse();
        Ok(
            json!({"items":items,"revision":self.web_revision(),"parent":folder.and_then(|id| self.settings.home.parent(id)),"breadcrumbs":breadcrumbs}),
        )
    }

    fn web_edit_home(&mut self, edit: WebHomeEdit) -> std::result::Result<(), WebError> {
        let mut settings = self.settings.clone();
        match edit {
            WebHomeEdit::Folder { name, parent } => {
                settings
                    .home
                    .add(
                        crate::home::Entry {
                            name: web_name(&name)?,
                            target: crate::home::Target::Folder { children: vec![] },
                            image: None,
                        },
                        parent.as_deref(),
                    )
                    .map_err(web_error)?;
            }
            WebHomeEdit::Add {
                name,
                target,
                parent,
            } => {
                let target = self
                    .web
                    .targets
                    .get(&target)
                    .ok_or_else(|| web_error("Refresh the target's page first."))?
                    .target
                    .clone();
                if matches!(target, crate::home::Target::Folder { .. }) {
                    return Err(web_error(
                        "Move an existing personal folder instead of copying it.",
                    ));
                }
                settings
                    .home
                    .add(
                        crate::home::Entry {
                            name: web_name(&name)?,
                            target,
                            image: None,
                        },
                        parent.as_deref(),
                    )
                    .map_err(web_error)?;
            }
            WebHomeEdit::Rename { id, name } => {
                settings
                    .home
                    .entries
                    .get_mut(&id)
                    .ok_or_else(|| web_error("The entry no longer exists."))?
                    .name = web_name(&name)?;
            }
            WebHomeEdit::Remove { id } => {
                if !settings.home.entries.contains_key(&id) {
                    return Err(web_error("The entry no longer exists."));
                }
                settings.home.remove(&id);
            }
            WebHomeEdit::Move { id, parent } => settings
                .home
                .move_to(&id, parent.as_deref())
                .map_err(web_error)?,
            WebHomeEdit::Reorder { folder, keys } => {
                let categories = self.home_editor_categories();
                let mut current = settings.home.rows(folder.as_deref(), &categories, true);
                current.sort();
                let mut requested = keys.clone();
                requested.sort();
                if current != requested {
                    return Err(WebError::new(
                        "conflict",
                        "The reorder must contain every current entry exactly once.",
                    ));
                }
                settings.home.reorder(folder.as_deref(), &keys);
            }
            WebHomeEdit::Visibility { key, visible } => {
                if key
                    .strip_prefix("entry:")
                    .is_none_or(|id| !settings.home.entries.contains_key(id))
                    && key.strip_prefix("category:").is_none_or(|name| {
                        !self
                            .home_editor_categories()
                            .iter()
                            .any(|category| category == name)
                    })
                {
                    return Err(web_error("The entry no longer exists."));
                }
                if key == crate::home::category_key(crate::explore::NAME) {
                    settings.show_explore = Some(visible);
                }
                if visible {
                    settings.home.hidden.remove(&key);
                } else {
                    settings.home.hidden.insert(key);
                }
            }
        }
        settings.home.validate().map_err(web_error)?;
        self.web_save(settings)
    }

    fn web_launch(&mut self, id: &str) -> std::result::Result<JsonValue, WebError> {
        self.web_storage_idle()?;
        let target = self
            .web
            .targets
            .get(id)
            .cloned()
            .ok_or_else(|| web_error("Refresh the game page before launching."))?;
        if self.kiosk_locked && !self.kiosk_target_allowed(&target.target) {
            return Err(WebError::new(
                "locked",
                "This target is not available in kiosk mode.",
            ));
        }
        let outcome = match target.target {
            crate::home::Target::Game { system, launch } => {
                let found = self
                    .all_systems
                    .iter()
                    .find(|found| found.def.id == system)
                    .ok_or_else(|| web_error("This system is no longer installed."))?;
                let config = found.to_config();
                let (config, history_system, history_launch, override_launch) =
                    if is_favorites(found.category()) {
                        self.favorite_launch_context(&launch, config)
                            .map_err(web_error)?
                    } else {
                        (config, Some(system.clone()), launch.clone(), launch.clone())
                    };
                let name = target.row.map(|r|r.name).or_else(||self.settings.home.entries.values().find(|entry|matches!(&entry.target,crate::home::Target::Game{system:owner,launch:other} if owner==&system&&other==&launch)).map(|entry|entry.name.clone())).ok_or_else(||web_error("Open the game details before launching this entry."))?;
                self.launch_game(
                    name,
                    launch,
                    config,
                    history_system,
                    history_launch,
                    override_launch,
                )
                .ok_or_else(|| web_error(self.message.as_deref().unwrap_or("Launch failed.")))?
            }
            crate::home::Target::Core { path } => {
                let plan = if self.web.core_snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .items
                        .iter()
                        .any(|item| item.launch_path.as_ref() == Some(&path))
                }) {
                    crate::launch::plan_misterzine(&path, Path::new(&self.config.menu_root))
                } else {
                    crate::launch::plan_core(&path, Path::new(&self.config.menu_root))
                };
                Outcome::Launch {
                    plan: Box::new(plan.map_err(web_error)?),
                    name: path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    history: None,
                    active_game: None,
                }
            }
            crate::home::Target::Script { path } => {
                let browser = crate::scripts::Browser::open(Path::new(&self.config.menu_root))
                    .map_err(web_error)?;
                let executable = std::env::current_exe().map_err(web_error)?;
                let launch = crate::scripts::Launch::prepare(
                    browser.root(),
                    &path,
                    &executable,
                    std::env::args_os().skip(1).collect(),
                )
                .map_err(web_error)?;
                if !self.save_settings() {
                    return Err(web_error(
                        self.message.as_deref().unwrap_or("Saving settings failed."),
                    ));
                }
                Outcome::Script(Box::new(launch))
            }
            _ => {
                return Err(web_error(
                    "This entry opens a folder; it is not a launch target.",
                ))
            }
        };
        self.web.outcome = Some(outcome);
        Ok(json!({"accepted":true,"state":"launching","target":id}))
    }

    fn web_new_job(&mut self, message: &str) -> u64 {
        self.web.next_job += 1;
        let id = self.web.next_job;
        while self.web.jobs.len() >= 16 {
            let oldest = self
                .web
                .jobs
                .iter()
                .find(|(_, job)| job["state"] != "running")
                .map(|(id, _)| *id);
            if let Some(oldest) = oldest {
                self.web.jobs.remove(&oldest);
            } else {
                break;
            }
        }
        self.web.jobs.insert(
            id,
            json!({"id":id,"state":"running","message":message,"cancellable":true}),
        );
        id
    }

    fn web_job_cancellable(&self, id: u64) -> bool {
        !self.web.ini_job.as_ref().is_some_and(|(job, _)| *job == id)
            && self.web.controller_job != Some(id)
            && !self
                .web
                .upload_job
                .as_ref()
                .is_some_and(|(job, _, _, _)| *job == id)
    }

    fn web_start_read(
        &mut self,
        operation: crate::web::library::Operation,
        explore: bool,
    ) -> std::result::Result<JsonValue, WebError> {
        if self.web.reading.is_some() {
            return Err(WebError::new(
                "busy",
                "A browser library read is already running. Wait or cancel it.",
            ));
        }
        if self.web.library.is_none() {
            self.web.library = Some(crate::web::library::Worker::start()?);
        }
        let snapshot = crate::web::library::Snapshot {
            library: self.indexed_library_request(!explore),
            settings: self.settings.clone(),
            menu_root: PathBuf::from(&self.config.menu_root),
            history_path: crate::history::path_beside(&self.settings_path),
            names: self.names.clone(),
            systems: self.all_systems.clone(),
            homes: self.homes(),
            favorites: self.favorites.clone(),
            pack_roots: self.effective_artwork_pack_roots.clone(),
        };
        let id = self.web_new_job("Reading library");
        self.web
            .library
            .as_ref()
            .expect("library worker")
            .submit(id, snapshot, operation)?;
        self.web.reading = Some(id);
        Ok(json!({"job":id}))
    }

    fn poll_web_jobs(&mut self) {
        self.poll_web_upload();
        self.poll_web_data_jobs();
        if let Some(event) = self
            .web
            .core_details_job
            .as_mut()
            .and_then(|(_, _, job)| job.try_recv())
        {
            let (id, item, _) = self.web.core_details_job.take().expect("core detail job");
            let result = match event {
                crate::misterzine::GameEvent::Ready { matches, .. } => {
                    Ok(self.web_core_detail(&item, Some(&matches)))
                }
                crate::misterzine::GameEvent::Failed { error, .. } => Err(web_error(error)),
                crate::misterzine::GameEvent::Cancelled => {
                    Err(WebError::new("cancelled", "Core details cancelled."))
                }
            };
            self.web_finish_job(id, result);
        }
        if let Some(id) = self.web.option_job {
            if self.source_resolution.is_none() {
                self.web.option_job = None;
                let mut result = self.web_options();
                result["source_errors"] = json!(self.artwork_source_errors);
                self.web_finish_job(id, Ok(result));
            }
        }
        if let Some(id) = self.web.rebuild_job {
            if self.build.is_some() || self.source_job.is_some() || self.source_resolution.is_some()
            {
                let view = self.index_overview();
                self.web.jobs.insert(id,json!({"id":id,"state":"running","message":view.as_ref().map(|view|view.subject.as_str()).unwrap_or("Preparing Artwork Pack"),"done":view.as_ref().map(|view|view.done),"total":view.as_ref().map(|view|view.total),"cancellable":true}));
            } else {
                self.web.rebuild_job = None;
                let result = match self.index_overview() {
                    Some(view)
                        if view.state == "Complete" || view.state == "Finished With Problems" =>
                    {
                        Ok(
                            json!({"report":view.report,"warnings":view.problem,"games":view.games,"folders":view.folders}),
                        )
                    }
                    Some(view) => Err(WebError::new(
                        if view.state == "Cancelled" {
                            "cancelled"
                        } else {
                            "index"
                        },
                        view.report,
                    )),
                    None => {
                        Err(web_error(self.message.as_deref().unwrap_or(
                            "The system rebuild ended without a completion report.",
                        )))
                    }
                };
                self.web_finish_job(id, result);
            }
        }
        if let Some(result) = self
            .web
            .ini_job
            .as_ref()
            .and_then(|(_, job)| job.try_recv())
        {
            let (id, _) = self.web.ini_job.take().expect("INI job");
            let result = result.map_err(web_error).map(|snapshot| {
                let value = json!({"current":snapshot.current,"profiles":snapshot.profiles.iter().map(|profile|json!({"slot":profile.slot,"name":profile.name})).collect::<Vec<_>>()});
                self.web.ini_snapshot = Some(snapshot);
                value
            });
            self.web_finish_job(id, result);
        }
        while let Some(event) = self
            .web
            .library
            .as_ref()
            .and_then(|worker| worker.events.try_recv().ok())
        {
            match event {
                crate::web::library::Event::Progress(id, message) => {
                    self.web
                        .jobs
                        .insert(id, json!({"id":id,"state":"running","message":message}));
                }
                crate::web::library::Event::Finished(id, result) => {
                    self.web.reading = None;
                    let result = result.and_then(|page| self.web_page(page));
                    self.web_finish_job(id, result);
                }
            }
        }
        while let Some(event) = self
            .web
            .core_job
            .as_mut()
            .and_then(|(_, job)| job.try_recv())
        {
            let id = self.web.core_job.as_ref().expect("core job").0;
            match event {
                crate::misterzine::Event::Progress(progress) => {
                    self.web.jobs.insert(id,json!({"id":id,"state":"running","message":progress.phase.label(),"done":progress.completed,"total":progress.total}));
                }
                crate::misterzine::Event::Snapshot(snapshot) => {
                    if let Some(visit) = &mut self.web.core_visit {
                        visit.observe(snapshot.catalogue.clone(), snapshot.complete_refresh);
                    }
                    self.web.core_snapshot = Some(snapshot);
                }
                crate::misterzine::Event::Ready { snapshot, notice } => {
                    if let Some(visit) = &mut self.web.core_visit {
                        visit.observe(snapshot.catalogue.clone(), snapshot.complete_refresh);
                    }
                    self.web.core_snapshot = Some(snapshot);
                    self.web.core_job = None;
                    let mut value = self.web_core_updates();
                    value["notice"] = json!(notice);
                    self.web_finish_job(id, Ok(value));
                    break;
                }
                crate::misterzine::Event::Cancelled => {
                    self.web.core_job = None;
                    self.web_finish_job(
                        id,
                        Err(WebError::new("cancelled", "Core refresh cancelled.")),
                    );
                    break;
                }
                crate::misterzine::Event::Failed { message } => {
                    self.web.core_job = None;
                    self.web_finish_job(id, Err(WebError::new("core-updates", message)));
                    break;
                }
            }
        }
    }

    fn web_finish_job(&mut self, id: u64, result: std::result::Result<JsonValue, WebError>) {
        let value = match result {
            Ok(value) => json!({"id":id,"state":"complete","result":value}),
            Err(error) => {
                json!({"id":id,"state":if error.code=="cancelled"{"cancelled"}else{"failed"},"error":error})
            }
        };
        if let Some(bridge) = &self.web.bridge {
            bridge.event(&json!({"type":"job","job":id,"state":value["state"]}));
        }
        self.web.jobs.insert(id, value);
    }

    fn web_page(
        &mut self,
        page: crate::web::library::Page,
    ) -> std::result::Result<JsonValue, WebError> {
        let mut value = page.data;
        if let Some(paths) = value.as_object_mut().expect("page").remove("home_artwork") {
            let paths: std::collections::BTreeMap<String, PathBuf> =
                serde_json::from_value(paths).map_err(web_error)?;
            value["images"] = json!(paths
                .into_iter()
                .map(|(key, path)| (key, self.web_art(Some(path))))
                .collect::<std::collections::BTreeMap<_, _>>());
            return Ok(value);
        }
        if let Some(trail) = value.get("trail") {
            let trail: Vec<browse::Place> =
                serde_json::from_value(trail.clone()).map_err(web_error)?;
            let system = value["system"]
                .as_str()
                .ok_or_else(|| web_error("Missing folder system."))?
                .to_string();
            let breadcrumbs: Vec<_> = trail
                .iter()
                .enumerate()
                .map(|(index, place)| {
                    let name = if index == 0 || matches!(place, browse::Place::Roots) {
                        value["name"].as_str().unwrap_or_default().to_string()
                    } else if let browse::Place::ArchiveDirectory { prefix, .. } = place {
                        prefix
                            .trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        place
                            .path()
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    };
                    let folder = self.web_register(
                        crate::home::Target::LibraryFolder {
                            system: system.clone(),
                            trail: trail[..=index].to_vec(),
                        },
                        None,
                    );
                    json!({"name":name,"folder":folder})
                })
                .collect();
            value["folder"] = breadcrumbs
                .last()
                .map(|item| item["folder"].clone())
                .unwrap_or(JsonValue::Null);
            value["parent"] = breadcrumbs
                .iter()
                .rev()
                .nth(1)
                .map(|item| item["folder"].clone())
                .unwrap_or(JsonValue::Null);
            value["breadcrumbs"] = json!(breadcrumbs);
            value.as_object_mut().expect("page").remove("trail");
        }
        if let Some(paths) = value.as_object_mut().expect("page").remove("script_trail") {
            let paths: Vec<PathBuf> = serde_json::from_value(paths).map_err(web_error)?;
            value["breadcrumbs"] = json!(paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    if index == 0 {
                        return json!({"name":"Scripts","folder":null});
                    }
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let id = web_id("script-folder", &path);
                    self.web.script_folders.insert(id.clone(), path);
                    json!({"name":name,"folder":id})
                })
                .collect::<Vec<_>>());
        }
        let rows:Vec<_> = page.entries.into_iter().map(|entry| {
            let target = entry.target.map(|target| self.web_register(target,entry.row.clone()));
            let folder = entry.folder.map(|folder| { let id=web_id("script-folder",&folder); self.web.script_folders.insert(id.clone(),folder); id });
            let artwork = entry.row.as_ref().and_then(|row|row.cover.clone()).or_else(||entry.system.as_ref().and_then(|id|self.all_systems.iter().find(|system|&system.def.id==id)).and_then(|system|self.system_logo(system)));
            let image = self.web_art(artwork);
            json!({"id":target,"name":entry.name,"system":entry.system,"system_name":entry.system_name,"image":image,"genre":entry.row.as_ref().and_then(|row|row.genre.clone()),"details":entry.row.as_ref().map(|row|&row.details),"favorite":entry.row.as_ref().is_some_and(|row|row.favorite),"kind":if folder.is_some()||entry.row.as_ref().is_some_and(|row|row.is_folder()){"folder"}else{"game"},"script_folder":folder,"games":entry.row.as_ref().and_then(|row|row.below)})
        }).collect();
        value["items"] = json!(rows);
        value["revision"] = json!(self.web_revision());
        if value["single_game"] == true {
            let mut game = value["items"][0].take();
            game["revision"] = value["revision"].take();
            game["notices"] = value["notices"].take();
            return Ok(game);
        }
        Ok(value)
    }

    fn web_core_updates(&mut self) -> JsonValue {
        let Some(snapshot) = self.web.core_snapshot.clone() else {
            return json!({"items":[],"loaded":false});
        };
        let items:Vec<_> = snapshot.items.iter().map(|item| {
            let target=item.launch_path.as_ref().map(|path|self.web_register(crate::home::Target::Core{path:path.clone()},None));
            let image=self.web_art(item.cover().map(Path::to_path_buf).or_else(||item.logo_id().and_then(|id|self.all_systems.iter().find(|found|found.def.id==id)).and_then(|found|self.system_logo(found))));
            json!({"id":item.key(),"name":item.display_title(),"source":item.source_label(),"state":item.state_label(),"installed":item.installed(),"update":item.update_available(),"build":item.build(),"details":item.information_with_games(None),"target":target,"image":image,"change":self.web.core_visit.as_ref().and_then(|visit|visit.label(item.key())),"new":self.web.core_visit.as_ref().is_some_and(|visit|visit.matches(crate::core_changes::Scope::WhatsNew,item))})
        }).collect();
        json!({"items":items,"updated":snapshot.updated,"loaded":true,"snapshot":web_id("catalogue",&snapshot.catalogue),"history":self.web.core_visit.as_ref().map(|visit|visit.message())})
    }

    fn web_core_detail(
        &mut self,
        item: &crate::misterzine::Item,
        matches: Option<&crate::misterzine::GameMatches>,
    ) -> JsonValue {
        let mut covers: Vec<PathBuf> = matches
            .into_iter()
            .flat_map(|matches| matches.games.iter().filter_map(|game| game.cover.clone()))
            .collect();
        if let Some(cover) = item.cover() {
            covers.push(cover.to_path_buf());
        }
        covers.sort();
        covers.dedup();
        let covers: Vec<_> = covers
            .into_iter()
            .filter_map(|path| self.web_art(Some(path)))
            .collect();
        json!({"name":item.display_title(),"installed":item.installed(),"details":item.information_with_games(matches),"images":covers,"target":item.launch_path.as_ref().map(|path|self.web_register(crate::home::Target::Core{path:path.clone()},None))})
    }

    fn web_library_command(
        &mut self,
        command: WebCommand,
    ) -> std::result::Result<JsonValue, WebError> {
        use crate::web::library::Operation;
        match command {
            WebCommand::Browse {
                system,
                folder,
                offset,
                limit,
                search,
                filters,
            } => {
                let found = self
                    .all_systems
                    .iter()
                    .find(|found| found.def.id == system)
                    .ok_or_else(|| web_error("Unknown system."))?;
                if self.kiosk_locked && !self.kiosk_category_allowed(found.category()) {
                    return Err(WebError::new(
                        "locked",
                        "This system is not available in kiosk mode.",
                    ));
                }
                let trail = match folder {
                    None => vec![],
                    Some(id) => match &self
                        .web
                        .targets
                        .get(&id)
                        .ok_or_else(|| web_error("Refresh this folder's parent first."))?
                        .target
                    {
                        crate::home::Target::LibraryFolder {
                            system: owner,
                            trail,
                        } if owner == &system => trail.clone(),
                        _ => return Err(web_error("The folder does not belong to this system.")),
                    },
                };
                self.web_start_read(
                    Operation::Browse {
                        system,
                        trail,
                        offset,
                        limit,
                        search,
                        filters,
                        pick: None,
                    },
                    false,
                )
            }
            WebCommand::Explore {
                query,
                offset,
                limit,
            } => self.web_start_read(
                Operation::Explore {
                    query,
                    offset,
                    limit,
                    pick: None,
                },
                true,
            ),
            WebCommand::History => self.web_start_read(Operation::History, false),
            WebCommand::Scripts { folder } => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Scripts are unavailable in kiosk mode.",
                    ));
                }
                let folder = folder
                    .map(|id| {
                        self.web
                            .script_folders
                            .get(&id)
                            .cloned()
                            .ok_or_else(|| web_error("Refresh the parent script folder first."))
                    })
                    .transpose()?;
                self.web_start_read(Operation::Scripts { folder }, false)
            }
            WebCommand::Game { id } => {
                let target = self
                    .web
                    .targets
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| web_error("Refresh the game list first."))?;
                if self.kiosk_locked && !self.kiosk_target_allowed(&target.target) {
                    return Err(WebError::new(
                        "locked",
                        "This game is unavailable in kiosk mode.",
                    ));
                }
                let crate::home::Target::Game { system, launch } = target.target else {
                    return Err(web_error("Choose a game."));
                };
                let found = self
                    .all_systems
                    .iter()
                    .find(|found| found.def.id == system)
                    .ok_or_else(|| web_error("System is unavailable."))?;
                let (details_system, details_launch) = if is_favorites(found.category()) {
                    let (_, owner, _, target) = self
                        .favorite_launch_context(&launch, found.to_config())
                        .map_err(web_error)?;
                    (
                        owner.ok_or_else(|| {
                            web_error("This favourite has no identifiable owning system.")
                        })?,
                        target,
                    )
                } else {
                    (system.clone(), launch.clone())
                };
                self.web_start_read(
                    Operation::Game {
                        system,
                        launch,
                        details_system,
                        details_launch,
                    },
                    false,
                )
            }
            WebCommand::CoreUpdate { key } => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Core Updates is unavailable in kiosk mode.",
                    ));
                }
                if self.web.core_details_job.is_some() {
                    return Err(WebError::new(
                        "busy",
                        "Core details are still loading. Wait or cancel first.",
                    ));
                }
                let item = self
                    .web
                    .core_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.items.iter().find(|item| item.key() == key))
                    .cloned()
                    .ok_or_else(|| web_error("Refresh the Core Updates list first."))?;
                if let Some(request) = self.misterzine_game_request(&item) {
                    let job = crate::misterzine::start_game_job(request).map_err(web_error)?;
                    let id = self.web_new_job("Reading games and artwork");
                    self.web.core_details_job = Some((id, item, job));
                    Ok(json!({"job":id}))
                } else {
                    Ok(self.web_core_detail(&item, None))
                }
            }
            WebCommand::CoreUpdates { refresh, begin } => {
                if self.kiosk_locked {
                    return Err(WebError::new(
                        "locked",
                        "Core Updates is unavailable in kiosk mode.",
                    ));
                }
                if self.web.core_visit.is_none() || begin && self.web.core_job.is_none() {
                    let mut visit = crate::core_changes::Visit::new(
                        crate::core_changes::load(&self.cache_dir).map_err(web_error)?,
                    );
                    if let Some(snapshot) = &self.web.core_snapshot {
                        visit.observe(snapshot.catalogue.clone(), snapshot.complete_refresh);
                    }
                    self.web.core_visit = Some(visit);
                }
                if !refresh && self.web.core_snapshot.is_some() {
                    return Ok(self.web_core_updates());
                }
                if let Some((id, _)) = &self.web.core_job {
                    return Ok(json!({"job":id}));
                }
                let job = crate::misterzine::start(self.misterzine_request(refresh))
                    .map_err(web_error)?;
                if let Some(visit) = &mut self.web.core_visit {
                    visit.begin_refresh();
                }
                let id = self.web_new_job("Reading Core Updates");
                self.web.core_job = Some((id, job));
                Ok(json!({"job":id}))
            }
            WebCommand::CoreUpdatesSeen { snapshot } => {
                let current = self
                    .web
                    .core_snapshot
                    .as_ref()
                    .ok_or_else(|| web_error("No completed catalogue is available."))?;
                if web_id("catalogue", &current.catalogue) != snapshot {
                    return Err(WebError::new(
                        "conflict",
                        "The catalogue changed before it was displayed.",
                    ));
                }
                if let Some(visit) = &mut self.web.core_visit {
                    visit.presented();
                    visit.acknowledge(&self.cache_dir).map_err(web_error)?;
                }
                Ok(json!({"acknowledged":true}))
            }
            WebCommand::Cancel { id } => {
                let status = self
                    .web
                    .jobs
                    .get(&id)
                    .ok_or_else(|| web_error("This operation is no longer retained."))?;
                if status["state"] != "running" {
                    return Ok(json!({"cancelled":false,"state":status["state"]}));
                }
                if !self.web_job_cancellable(id) {
                    return Err(web_error(
                        "This operation is committing its result and cannot be cancelled now.",
                    ));
                }
                if self.web.reading == Some(id) {
                    if let Some(worker) = &self.web.library {
                        worker.cancel();
                    }
                }
                if let Some((_, job)) = &self
                    .web
                    .core_job
                    .as_ref()
                    .filter(|(job_id, _)| *job_id == id)
                {
                    job.cancel();
                }
                if let Some((_, _, job)) = &self
                    .web
                    .core_details_job
                    .as_ref()
                    .filter(|(job_id, _, _)| *job_id == id)
                {
                    job.cancel();
                }
                self.web_cancel_data_job(id);
                if self.web.rebuild_job == Some(id) {
                    self.cancel_index();
                }
                if self.web.option_job == Some(id) {
                    if let Some(job) = &self.source_resolution {
                        job.cancel();
                    }
                }
                Ok(json!({"cancelling":true}))
            }
            WebCommand::Attract => {
                self.start_manual_attract_mode();
                Ok(json!({"accepted":true}))
            }
            WebCommand::Rebuild { system } => {
                self.web_storage_idle()?;
                if self.message.is_some() || self.pending.is_some() {
                    return Err(WebError::new("busy", "Close the message on MiSTer first."));
                }
                self.message = None;
                self.index_terminal = None;
                match system {
                    None => self.rebuild_all_systems(),
                    Some(id) => {
                        if !self.all_systems.iter().any(|found| found.def.id == id) {
                            return Err(web_error("Unknown system."));
                        }
                        if self.pack_selected(&id) {
                            let kind =
                                (!crate::artwork_pack::inherits_picture_type(&self.settings, &id))
                                    .then(|| {
                                        crate::artwork_pack::selected_picture_type(
                                            &self.settings,
                                            &id,
                                        )
                                    });
                            return self.web_source_save(
                                &self.web_revision(),
                                &id,
                                crate::web::api::GameDataSource::ArtworkPack,
                                None,
                                kind,
                            );
                        } else {
                            self.rebuild_ordinary_system(&id);
                        }
                    }
                }
                if self.build.is_none() && self.source_job.is_none() {
                    return Err(web_error(
                        self.message
                            .as_deref()
                            .unwrap_or("The rebuild did not start."),
                    ));
                }
                let id = self.web_new_job("Rebuilding library");
                self.web.rebuild_job = Some(id);
                Ok(json!({"job":id}))
            }
            WebCommand::Random {
                query,
                folder,
                favorites_only,
            } => {
                let pick = Some((next_random(&mut self.seed), favorites_only));
                if let Some(id) = folder {
                    let target =
                        self.web.targets.get(&id).ok_or_else(|| {
                            web_error("Refresh this folder before picking a game.")
                        })?;
                    if self.kiosk_locked && !self.kiosk_target_allowed(&target.target) {
                        return Err(WebError::new(
                            "locked",
                            "This folder is not available in kiosk mode.",
                        ));
                    }
                    let crate::home::Target::LibraryFolder { system, trail } = &target.target
                    else {
                        return Err(web_error("Choose a library folder."));
                    };
                    return self.web_start_read(
                        Operation::Browse {
                            system: system.clone(),
                            trail: trail.clone(),
                            offset: 0,
                            limit: 1,
                            search: query.title.clone(),
                            filters: query,
                            pick,
                        },
                        false,
                    );
                }
                self.web_start_read(
                    Operation::Explore {
                        query,
                        offset: 0,
                        limit: 1,
                        pick,
                    },
                    true,
                )
            }
            _ => Err(web_error("This command is not a library operation.")),
        }
    }
}
