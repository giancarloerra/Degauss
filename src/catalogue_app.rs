#[derive(Default)]
struct CatalogueBrowser {
    location: Option<crate::catalogue_update::Location>,
    open: bool,
    job: Option<crate::catalogue_update::Job>,
    release: Option<crate::catalogue_update::Release>,
    status: String,
}
impl CatalogueBrowser {
    fn record_rebuild(&mut self, id: &str) {
        if let Some(location) = &mut self.location {
            if let Err(e) = location.rebuilt(id) {
                crate::note(&format!("catalogue: recording completed rescan: {e}"));
            }
        }
    }
    fn start(
        &mut self,
        loc: crate::catalogue_update::Location,
        install: bool,
    ) -> std::result::Result<(), String> {
        let release = if install {
            Some(
                self.release
                    .clone()
                    .ok_or("Check for a revision before installing.")?,
            )
        } else {
            None
        };
        let job = crate::catalogue_update::Job::start(loc, release)?;
        self.release = None;
        self.status = if install {
            "Installing…"
        } else {
            "Checking GitHub…"
        }
        .into();
        self.job = Some(job);
        Ok(())
    }
}
impl App {
    fn catalogue_rebuilds(&self) -> Vec<&String> {
        self.catalogue_update
            .location
            .iter()
            .flat_map(|l| &l.pending)
            .filter(|id| {
                self.all_systems
                    .iter()
                    .any(|s| s.def.id == id.as_str() && !s.paths.is_empty())
            })
            .collect()
    }
    fn catalogue_web_status(&self) -> JsonValue {
        json!({"busy":self.catalogue_update.job.is_some(),"status":self.catalogue_update.status,"catalogue_revision":self.catalogue_update.release.as_ref().map(|r|r.meta.revision),"changed":self.catalogue_rebuilds()})
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
            let install = match command {
                WebCommand::CatalogueInstall { revision } => {
                    if self
                        .catalogue_update
                        .release
                        .as_ref()
                        .is_none_or(|r| r.meta.revision != revision)
                    {
                        return Err(web_error("Check again before installing this revision."));
                    }
                    true
                }
                _ => false,
            };
            self.catalogue_update
                .start(loc, install)
                .map_err(web_error)?;
        }
        Ok(self.catalogue_web_status())
    }
    fn catalogue_rows(&mut self) {
        self.screen = Screen::Context;
        self.menu = if self.catalogue_update.job.is_some() {
            vec!["Cancel".into()]
        } else {
            std::iter::once("Back".to_string())
                .chain(
                    self.catalogue_update
                        .release
                        .as_ref()
                        .map(|_| "Install".to_string()),
                )
                .chain(
                    self.catalogue_rebuilds()
                        .into_iter()
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
        if let Err(e) = self.catalogue_update.start(loc, false) {
            self.catalogue_error(&e);
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
            }
            Ok(O::Checked(C::AwaitingRestart)) => {
                self.catalogue_update.status =
                    "Catalogue updated. Applies next time Degauss opens.".into();
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
                if let Some(loc) = &mut self.catalogue_update.location {
                    loc.pending.clear(); // The new revision applies at the next start.
                }
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
                if let Err(e) = self.catalogue_update.start(loc, true) {
                    self.catalogue_error(&e);
                }
                self.catalogue_rows();
            }
            Action::Accept if at > 0 => {
                let offset = 1 + usize::from(self.catalogue_update.release.is_some());
                if let Some(id) = self
                    .catalogue_rebuilds()
                    .get(at - offset)
                    .map(|id| (*id).clone())
                {
                    self.catalogue_update.open = false;
                    self.open_options_page(OptionsPage::Library);
                    if self.pack_selected(&id) {
                        if !self.begin_source_recovery(&id, SourceRecoveryPurpose::RefreshSystem)
                            && self.message.is_none()
                        {
                            self.message = Some("The Artwork Pack rebuild could not start. Finish any current library operation and retry.".into());
                        }
                    } else {
                        self.rebuild_ordinary_system(&id);
                    }
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
