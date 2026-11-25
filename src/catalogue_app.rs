#[derive(Default)]
struct CatalogueBrowser {
    location: Option<crate::catalogue_update::Location>,
    open: bool,
    job: Option<crate::catalogue_update::Job>,
    release: Option<crate::catalogue_update::Release>,
    status: String,
    rebuild: Vec<String>,
}
impl App {
    fn catalogue_web_status(&self) -> JsonValue {
        json!({"busy":self.catalogue_update.job.is_some(),"status":self.catalogue_update.status,"revision":self.catalogue_update.release.as_ref().map(|r|r.meta.revision),"changed":self.catalogue_update.rebuild})
    }
    fn catalogue_web_command(
        &mut self,
        command: WebCommand,
    ) -> std::result::Result<JsonValue, WebError> {
        if self.catalogue_update.open {
            return Err(web_error("Finish the catalogue check on MiSTer."));
        }
        let loc = self
            .catalogue_update
            .location
            .clone()
            .ok_or_else(|| web_error("Catalogue location unavailable."))?;
        if matches!(command, WebCommand::CatalogueCancel) {
            if let Some(job) = &self.catalogue_update.job {
                job.cancel();
            } else {
                self.catalogue_update.release = None;
                self.catalogue_update.status = "Cancelled.".into();
            }
        } else {
            if self.catalogue_update.job.is_some() {
                return Err(web_error("A catalogue operation is already running."));
            }
            let release = match command {
                WebCommand::CatalogueInstall { revision } => {
                    if self
                        .catalogue_update
                        .release
                        .as_ref()
                        .is_none_or(|r| r.meta.revision != revision)
                    {
                        return Err(web_error("Check again before installing this revision."));
                    }
                    self.catalogue_update.release.take()
                }
                _ => {
                    self.catalogue_update.release = None;
                    None
                }
            };
            self.catalogue_update.status = if release.is_some() {
                "Installing…"
            } else {
                "Checking GitHub…"
            }
            .into();
            self.catalogue_update.rebuild.clear();
            self.catalogue_update.job =
                Some(crate::catalogue_update::Job::start(loc, release).map_err(web_error)?);
        }
        Ok(self.catalogue_web_status())
    }
    fn catalogue_rows(&mut self) {
        self.screen = Screen::Context;
        self.menu = if self.catalogue_update.job.is_some() {
            vec!["Cancel".into()]
        } else if self.catalogue_update.release.is_some() {
            vec!["Back".into(), "Install".into()]
        } else {
            std::iter::once("Back".to_string())
                .chain(
                    self.catalogue_update
                        .rebuild
                        .iter()
                        .map(|id| format!("Rebuild {id}")),
                )
                .collect()
        };
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.apply_geometry();
        self.dirty = true;
    }
    fn catalogue_open(&mut self) {
        if self.kiosk_locked {
            return;
        }
        if self.catalogue_update.job.is_some() {
            self.catalogue_update.open = true;
            self.catalogue_rows();
            return;
        }
        let Some(loc) = self.catalogue_update.location.clone() else {
            self.message = Some("Catalogue location unavailable.".into());
            self.dirty = true;
            return;
        };
        self.catalogue_update.open = true;
        self.catalogue_update.release = None;
        self.catalogue_update.rebuild.clear();
        self.catalogue_update.status = "Checking GitHub…".into();
        match crate::catalogue_update::Job::start(loc, None) {
            Ok(j) => self.catalogue_update.job = Some(j),
            Err(e) => self.catalogue_error(&e),
        }
        self.catalogue_rows();
    }
    fn catalogue_error(&mut self, e: &str) {
        crate::note(&format!("catalogue: {e}"));
        self.catalogue_update.status = if e.contains("cancelled") {
            "Cancelled. Catalogue unchanged.".into()
        } else {
            "Couldn’t update; current catalogue unchanged.".into()
        };
    }
    fn catalogue_poll(&mut self) {
        use crate::catalogue_update::{Check as C, Outcome as O};
        let Some(job) = &self.catalogue_update.job else {
            return;
        };
        let result = match job.rx.try_recv() {
            Ok(v) => v,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Err("catalogue worker disconnected".into()),
        };
        self.catalogue_update.job = None;
        match result {
            Err(e) => self.catalogue_error(&e),
            Ok(O::Checked(C::Current)) => {
                self.catalogue_update.status = "Catalogue is up to date.".into();
                if let Some(loc) = &self.catalogue_update.location {
                    match loc.changed() {
                        Ok(ids) => {
                            self.catalogue_update.rebuild = ids
                                .into_iter()
                                .filter(|id| self.all_systems.iter().any(|s| s.def.id == *id))
                                .collect()
                        }
                        Err(e) => self.catalogue_error(&e),
                    }
                }
            }
            Ok(O::Checked(C::Custom)) => {
                self.catalogue_update.status = "Custom catalogue in use; no changes made.".into()
            }
            Ok(O::Checked(C::Incompatible(ver))) => {
                self.catalogue_update.status =
                    format!("Newer catalogue requires Degauss {ver} or a newer catalogue format.")
            }
            Ok(O::Checked(C::Update(r))) => {
                self.catalogue_update.status = format!("{}: {}", r.meta.revision, r.meta.summary);
                self.catalogue_update.release = Some(r);
            }
            Ok(O::Installed(ids)) => {
                self.catalogue_update.status =
                    "Catalogue updated. Applies next time Degauss opens.".into();
                if !ids.is_empty() {
                    self.catalogue_update
                        .status
                        .push_str(" Reopen Check to rescan changed libraries.");
                }
            }
        }
        if self.catalogue_update.open {
            self.catalogue_rows();
        }
    }
    fn catalogue_input(&mut self, action: Action) -> bool {
        if !self.catalogue_update.open || self.screen != Screen::Context {
            return false;
        }
        let at = self.menu_list.selected();
        match action {
            Action::Up => {
                self.menu_list.move_items(-1);
            }
            Action::Down => {
                self.menu_list.move_items(1);
            }
            Action::Quit | Action::Menu | Action::Accept if self.catalogue_update.job.is_some() => {
                if let Some(j) = &self.catalogue_update.job {
                    j.cancel();
                }
                self.catalogue_update.status = "Cancelling…".into();
            }
            Action::Accept if at == 1 && self.catalogue_update.release.is_some() => {
                let loc = self
                    .catalogue_update
                    .location
                    .clone()
                    .expect("checked location");
                let release = self.catalogue_update.release.take();
                match crate::catalogue_update::Job::start(loc, release) {
                    Ok(job) => {
                        self.catalogue_update.job = Some(job);
                        self.catalogue_update.status = "Installing…".into();
                    }
                    Err(e) => self.catalogue_error(&e),
                }
                self.catalogue_rows();
            }
            Action::Accept if at > 0 && !self.catalogue_update.rebuild.is_empty() => {
                if let Some(id) = self.catalogue_update.rebuild.get(at - 1).cloned() {
                    self.catalogue_update.open = false;
                    self.open_options_page(OptionsPage::Library);
                    self.rebuild_ordinary_system(&id);
                }
            }
            Action::Quit | Action::Accept | Action::Menu => {
                self.catalogue_update.open = false;
                self.catalogue_update.release = None;
                self.open_options_page(OptionsPage::Library);
            }
            _ => {}
        }
        self.dirty = true;
        true
    }
}
