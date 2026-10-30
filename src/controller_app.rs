use crate::controller::{self, Control, Device as ControllerDevice, Profile as ControllerProfile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControllerMenu {
    List,
    Device,
    Editor,
    Test,
}

enum ControllerJob {
    Discover,
    Apply(std::collections::BTreeMap<String, ControllerProfile>),
    Capture(Control),
}

#[derive(Default)]
struct ControllerBrowser {
    menu: Option<ControllerMenu>,
    devices: Vec<ControllerDevice>,
    inventory_generation: u16,
    selected: usize,
    draft: ControllerProfile,
    job: Option<(ControllerJob, controller::Job)>,
    tester: Option<controller::Tester>,
    test_action: String,
    test_back_since: Option<Instant>,
    active: Option<ControllerDevice>,
    raw_nodes: std::collections::BTreeSet<u16>,
    raw_keys: std::collections::BTreeSet<(u16, u16)>,
    report_revision: Option<(std::time::SystemTime, u64)>,
    report_generation: Option<u16>,
    report_next: Option<Instant>,
    report_error: Option<String>,
    initialized: bool,
    reset_input: bool,
}

impl App {
    fn controller_request(&mut self, purpose: ControllerJob, request: controller::Request) {
        match controller::Job::start(Path::new("/dev/MiSTer_cmd"), request) {
            Ok(job) => self.controllers.job = Some((purpose, job)),
            Err(error) => self.controller_error(error.to_string()),
        }
        self.dirty = true;
    }

    fn controller_error(&mut self, detail: String) {
        crate::note(&format!("controller   {detail}"));
        self.add_to_message(format!("Controller settings failed: {detail}"));
    }

    fn invalidate_controller_report(&mut self) {
        self.controllers.report_revision = None;
        self.controllers.raw_nodes.clear();
        self.controllers.raw_keys.clear();
    }

    fn controller_consumes_raw_key(&self, node: u16, code: u16) -> bool {
        self.controllers.raw_nodes.contains(&node)
            || self.controllers.raw_keys.contains(&(node, code))
            || (self.controllers.report_revision.is_some()
                && (self.controllers.menu == Some(ControllerMenu::Test)
                    || matches!(self.controllers.job, Some((ControllerJob::Capture(_), _))))
                && self.controllers.devices.iter().any(|device| {
                    device.keyboard_joystick
                        && device.paths.iter().any(|path| {
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .and_then(|name| name.strip_prefix("event"))
                                .and_then(|number| number.parse::<u16>().ok())
                                == Some(node)
                        })
                }))
    }

    fn open_controllers(&mut self) {
        self.controllers.menu = Some(ControllerMenu::List);
        self.controllers.devices.clear();
        self.controllers.selected = 0;
        self.controllers.reset_input = true;
        self.screen = Screen::Context;
        self.rebuild_controller_menu();
        self.controller_request(ControllerJob::Discover, controller::Request::Discover);
    }

    fn controller_device(&self) -> Option<&ControllerDevice> {
        self.controllers.devices.get(self.controllers.selected)
    }

    fn controller_role_source(&self, role: Control) -> Control {
        role.source(
            self.settings.swap_a_b.unwrap_or(false),
            self.settings.swap_x_y.unwrap_or(false),
        )
    }

    fn controller_rows(&self) -> Vec<(String, String)> {
        let Some(menu) = self.controllers.menu else {
            return Vec::new();
        };
        match menu {
            ControllerMenu::List => {
                let mut rows = self
                    .controllers
                    .devices
                    .iter()
                    .map(|device| {
                        let mode = if device.custom { "Degauss" } else { "MiSTer" };
                        (
                            format!("{} (input {})", device.name, device.instance + 1),
                            mode.into(),
                        )
                    })
                    .collect::<Vec<_>>();
                rows.push(("Refresh Controllers".into(), String::new()));
                rows
            }
            ControllerMenu::Device => {
                let custom = self.controller_device().is_some_and(|device| device.custom);
                vec![
                    ("Test Controller".into(), String::new()),
                    ("Remap Degauss Controls".into(), String::new()),
                    (
                        "Use MiSTer Mapping".into(),
                        if custom {
                            String::new()
                        } else {
                            "Selected".into()
                        },
                    ),
                    (
                        "Use Saved Degauss Mapping".into(),
                        if custom {
                            "Selected".into()
                        } else {
                            String::new()
                        },
                    ),
                ]
            }
            ControllerMenu::Editor => {
                let mut rows = Vec::new();
                if self.controller_device().is_some() {
                    for role in Control::ALL {
                        let source = self.controller_role_source(role);
                        let bindings = self.controllers.draft.bindings.get(&source);
                        let label = bindings
                            .filter(|bindings| !bindings.is_empty())
                            .map(|bindings| {
                                bindings
                                    .iter()
                                    .map(|binding| binding.label.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" / ")
                            })
                            .unwrap_or_else(|| "Unassigned".into());
                        rows.push((role.label().into(), label));
                    }
                }
                rows.push(("Save Degauss Mapping".into(), String::new()));
                rows
            }
            ControllerMenu::Test => {
                let mut rows = vec![(
                    "Latest Action (Any Pad)".into(),
                    self.controllers.test_action.clone(),
                )];
                if let Some(tester) = &self.controllers.tester {
                    rows.extend(tester.rows());
                }
                rows
            }
        }
    }

    fn rebuild_controller_menu(&mut self) {
        self.menu = self
            .controller_rows()
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        let selected = self.menu_list.selected();
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.menu_list.select(selected);
        self.apply_geometry();
        self.dirty = true;
    }

    fn controller_menu_to(&mut self, menu: ControllerMenu) {
        self.controllers.tester = None;
        self.controllers.test_back_since = None;
        self.controllers.menu = Some(menu);
        self.controllers.reset_input = true;
        self.menu_list.select(0);
        self.rebuild_controller_menu();
    }

    fn close_controllers(&mut self) {
        self.controllers.menu = None;
        self.controllers.tester = None;
        self.controllers.reset_input = true;
        self.screen = Screen::Options;
        self.apply_geometry();
        self.dirty = true;
    }

    fn apply_controller_profiles(
        &mut self,
        profiles: std::collections::BTreeMap<String, ControllerProfile>,
    ) {
        match controller::runtime_profiles(&profiles) {
            Ok(bytes) => {
                self.controllers.reset_input = true;
                self.controller_request(
                    ControllerJob::Apply(profiles),
                    controller::Request::Apply(bytes),
                );
            }
            Err(error) => self.controller_error(error.to_string()),
        }
    }

    fn handle_controller_menu(&mut self, action: Action) -> bool {
        let Some(menu) = self.controllers.menu else {
            return false;
        };
        if self.message.is_some() {
            return false;
        }
        if self.controllers.job.is_some() {
            return true;
        }
        match action {
            Action::Up => {
                self.menu_list.move_items(-1);
            }
            Action::Down => {
                self.menu_list.move_items(1);
            }
            Action::PageUp => {
                self.menu_list
                    .move_rows(-(self.menu_list.visible() as isize));
            }
            Action::PageDown => {
                self.menu_list.move_rows(self.menu_list.visible() as isize);
            }
            Action::Home => self.menu_list.select(0),
            Action::End => self.menu_list.select(self.menu.len().saturating_sub(1)),
            Action::Quit => match menu {
                ControllerMenu::List => self.close_controllers(),
                ControllerMenu::Device => {
                    self.controller_menu_to(ControllerMenu::List);
                    self.menu_list.select(self.controllers.selected);
                }
                ControllerMenu::Editor | ControllerMenu::Test => {
                    self.controller_menu_to(ControllerMenu::Device)
                }
            },
            Action::Accept => match menu {
                ControllerMenu::List => {
                    let at = self.menu_list.selected();
                    if at == self.controllers.devices.len() {
                        self.controller_request(
                            ControllerJob::Discover,
                            controller::Request::Discover,
                        );
                    } else {
                        self.controllers.selected = at;
                        self.controller_menu_to(ControllerMenu::Device);
                    }
                }
                ControllerMenu::Device => {
                    let Some(device) = self.controller_device().cloned() else {
                        return true;
                    };
                    match self.menu_list.selected() {
                        0 => match controller::Tester::open(&device.paths) {
                            Ok(tester) => {
                                self.controller_menu_to(ControllerMenu::Test);
                                self.controllers.tester = Some(tester);
                                self.controllers.test_action = "Press a control".into();
                                self.rebuild_controller_menu();
                            }
                            Err(error) => self.controller_error(error.to_string()),
                        },
                        1 => {
                            let saved = self.settings.controller_profiles.get(&device.id).cloned();
                            if saved
                                .as_ref()
                                .is_none_or(|profile| profile.bindings.is_empty())
                                && !device.mapped
                            {
                                self.controller_error("Press a control on this controller first, so MiSTer loads its native mapping. Then choose Remap Degauss Controls again.".into());
                                return true;
                            }
                            self.controllers.draft = saved
                                .filter(|profile| !profile.bindings.is_empty())
                                .unwrap_or_else(|| ControllerProfile::from_device(&device));
                            self.controllers.draft.custom = true;
                            self.controller_menu_to(ControllerMenu::Editor);
                        }
                        at => {
                            let mut profiles = self.settings.controller_profiles.clone();
                            let profile = profiles.entry(device.id).or_default();
                            if at == 3 && profile.bindings.is_empty() {
                                self.controller_error(
                                    "No saved mapping. Choose Remap Degauss Controls first.".into(),
                                );
                            } else {
                                profile.custom = at == 3;
                                if let Err(error) = profile.validate() {
                                    self.controller_error(error);
                                } else {
                                    self.apply_controller_profiles(profiles);
                                }
                            }
                        }
                    }
                }
                ControllerMenu::Editor => {
                    let Some(device) = self.controller_device().cloned() else {
                        return true;
                    };
                    if let Some(role) = Control::ALL.get(self.menu_list.selected()).copied() {
                        self.controllers.reset_input = true;
                        self.controller_request(
                            ControllerJob::Capture(self.controller_role_source(role)),
                            controller::Request::Capture {
                                instance: device.instance,
                                generation: self.controllers.inventory_generation,
                            },
                        );
                    } else if let Err(error) = self.controllers.draft.validate() {
                        self.controller_error(error);
                    } else {
                        let mut profiles = self.settings.controller_profiles.clone();
                        profiles.insert(device.id, self.controllers.draft.clone());
                        self.apply_controller_profiles(profiles);
                    }
                }
                ControllerMenu::Test => {}
            },
            Action::Context if menu == ControllerMenu::Editor => {
                if let Some(role) = Control::ALL.get(self.menu_list.selected()).copied() {
                    let source = self.controller_role_source(role);
                    if role.required() {
                        self.controller_error(format!("{} is required", role.label()));
                    } else {
                        self.controllers.draft.bindings.insert(source, Vec::new());
                    }
                    self.rebuild_controller_menu();
                }
            }
            _ => {}
        }
        self.last_input = Instant::now();
        self.dirty = true;
        true
    }

    /// Test and capture input never reaches navigation, hold shortcuts or
    /// the Back+direction INI chord. Direct keyboard Escape remains usable.
    fn controller_intercept_edge(&mut self, edge: KeyEdge, controller: bool, now: Instant) -> bool {
        if self.controllers.job.is_some() {
            if matches!(edge, KeyEdge::Down(Action::Quit))
                && !controller
                && matches!(self.controllers.job, Some((ControllerJob::Capture(_), _)))
            {
                if let Some((_, job)) = &self.controllers.job {
                    job.cancel();
                }
            }
            return self.controllers.menu.is_some() || controller;
        }
        if self.controllers.menu != Some(ControllerMenu::Test) {
            return false;
        }
        if !controller && matches!(edge, KeyEdge::Down(Action::Quit)) {
            self.controller_menu_to(ControllerMenu::Device);
        } else if controller {
            let (action, state) = match edge {
                KeyEdge::Down(action) => (action, "pressed"),
                KeyEdge::Up(action) => (action, "released"),
            };
            let label = match action {
                Action::Up => "Up",
                Action::Down => "Down",
                Action::Slower => "Left",
                Action::Faster => "Right",
                Action::Accept => "Select",
                Action::Quit => "Back",
                Action::Context => "Actions",
                Action::Menu => "Menu",
                Action::PageUp => "Previous Page",
                Action::PageDown => "Next Page",
                Action::Home => "First",
                Action::End => "Last",
                _ => "Other Input",
            };
            self.controllers.test_action = format!("{label} {state}");
            match edge {
                KeyEdge::Down(Action::Quit) => {
                    self.controllers.test_back_since.get_or_insert(now);
                }
                KeyEdge::Up(Action::Quit) => self.controllers.test_back_since = None,
                _ => {}
            }
            self.dirty = true;
        } else if !controller {
            match edge {
                KeyEdge::Down(Action::Up) => {
                    self.menu_list.move_items(-1);
                }
                KeyEdge::Down(Action::Down) => {
                    self.menu_list.move_items(1);
                }
                _ => {}
            }
            self.dirty = true;
        }
        self.last_input = now;
        true
    }

    fn poll_controller_report(&mut self, now: Instant, force: bool) {
        self.poll_controller_report_at(
            now,
            force,
            Path::new("/tmp/degauss-controller-active.status"),
        );
    }

    fn poll_controller_report_at(&mut self, now: Instant, force: bool, path: &Path) {
        if !force && self.controllers.report_next.is_some_and(|at| now < at) {
            return;
        }
        self.controllers.report_next = Some(now + Duration::from_millis(300));
        let result = (|| -> std::io::Result<Option<ControllerDevice>> {
            let metadata = match std::fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.invalidate_controller_report();
                    if self.controllers.report_generation.take().is_some() {
                        self.controllers.reset_input = true;
                    }
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                metadata.ino()
            };
            #[cfg(not(unix))]
            let identity = metadata.len();
            let revision = (metadata.modified()?, identity);
            if self.controllers.report_revision == Some(revision) {
                return Ok(self.controllers.active.clone());
            }
            let controller::Reply::Active { instance, snapshot } =
                controller::parse_reply(&std::fs::read(path)?)?
            else {
                return Err(std::io::Error::other("Unexpected active-controller report"));
            };
            let generation = snapshot.generation;
            if self.controllers.report_generation != Some(generation) {
                let changed = self.controllers.report_generation.is_some();
                self.controllers.report_generation = Some(generation);
                self.controllers.reset_input = true;
                if changed && self.controllers.menu.is_some() {
                    self.controllers.devices = snapshot.devices.clone();
                    self.controllers.inventory_generation = generation;
                    self.controllers.selected = 0;
                    self.controller_menu_to(ControllerMenu::List);
                    self.controller_error(
                        "Input devices changed. Select the controller again.".into(),
                    );
                }
            }
            if matches!(
                self.controllers.menu,
                Some(ControllerMenu::List | ControllerMenu::Device | ControllerMenu::Test)
            ) {
                self.controllers.devices = snapshot.devices.clone();
                self.controllers.inventory_generation = generation;
            }
            self.controllers.raw_nodes.clear();
            self.controllers.raw_keys.clear();
            for device in snapshot
                .devices
                .iter()
                .filter(|device| device.keyboard_joystick)
            {
                for path in &device.paths {
                    let Some(node) = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.strip_prefix("event"))
                        .and_then(|number| number.parse().ok())
                    else {
                        continue;
                    };
                    if device.custom {
                        self.controllers.raw_nodes.insert(node);
                    } else {
                        self.controllers.raw_keys.extend(
                            device
                                .native
                                .iter()
                                .flatten()
                                .map(|binding| (node, binding.code)),
                        );
                    }
                }
            }
            self.controllers.report_revision = Some(revision);
            Ok(instance.and_then(|id| {
                snapshot
                    .devices
                    .into_iter()
                    .find(|device| device.instance == id)
            }))
        })();
        match result {
            Ok(device) => {
                self.controllers.report_error = None;
                if self.controllers.active != device {
                    self.controllers.active = device;
                    self.dirty = true;
                }
            }
            Err(error) => {
                self.invalidate_controller_report();
                let detail = format!("Reading active controller: {error}");
                if self.controllers.report_error.as_deref() != Some(&detail) {
                    self.controller_error(detail.clone());
                    self.controllers.report_error = Some(detail);
                }
            }
        }
    }

    fn poll_controllers(&mut self, now: Instant) {
        if !self.controllers.initialized {
            self.controllers.initialized = true;
            if self
                .settings
                .controller_profiles
                .values()
                .any(|profile| profile.custom)
            {
                self.apply_controller_profiles(self.settings.controller_profiles.clone());
            }
        }
        self.poll_controller_report(now, false);
        if let Some(result) = self
            .controllers
            .job
            .as_ref()
            .and_then(|(_, job)| job.try_recv())
        {
            let (purpose, _) = self.controllers.job.take().expect("completed request");
            self.controllers.reset_input = true;
            match (purpose, result) {
                (ControllerJob::Discover, Ok(controller::Reply::Snapshot(snapshot))) => {
                    self.poll_controller_report(now, true);
                    self.receive_controller_discovery(snapshot);
                }
                (ControllerJob::Apply(profiles), Ok(controller::Reply::Applied)) => {
                    self.settings.controller_profiles = profiles;
                    self.save_settings();
                    if self.controllers.menu == Some(ControllerMenu::Editor) {
                        self.controller_menu_to(ControllerMenu::Device);
                    }
                    if self.controllers.menu.is_some() {
                        self.controller_request(
                            ControllerJob::Discover,
                            controller::Request::Discover,
                        );
                    }
                }
                (
                    ControllerJob::Capture(role),
                    Ok(controller::Reply::Captured {
                        instance,
                        code,
                        label,
                    }),
                ) if self.controllers.menu == Some(ControllerMenu::Editor)
                    && self
                        .controller_device()
                        .is_some_and(|device| device.instance == instance) =>
                {
                    self.controllers
                        .draft
                        .bindings
                        .insert(role, vec![controller::Binding { code, label }]);
                }
                (ControllerJob::Capture(_), Ok(controller::Reply::Cancelled)) => {}
                (ControllerJob::Capture(_), Ok(controller::Reply::Captured { .. })) => {
                    self.controller_error("The captured controller no longer matches the selection. Select the controller again.".into());
                }
                (_, Err(error)) => self.controller_error(error),
                _ => self.controller_error("Unexpected response from Degauss Main".into()),
            }
            if self.controllers.menu.is_some() {
                self.rebuild_controller_menu();
            }
            self.dirty = true;
        }
        if let Some(tester) = &mut self.controllers.tester {
            match tester.poll() {
                Ok(true) => self.dirty = true,
                Ok(false) => {}
                Err(error) => {
                    self.controller_menu_to(ControllerMenu::Device);
                    self.controller_error(format!("Controller test stopped: {error}"));
                }
            }
        }
        if self
            .controllers
            .test_back_since
            .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(2))
        {
            self.controller_menu_to(ControllerMenu::Device);
        }
    }

    fn receive_controller_discovery(&mut self, snapshot: controller::Snapshot) {
        if let Some(error) = self.controllers.report_error.clone() {
            if self.message.is_none() {
                self.controller_error(error);
            }
            return;
        }
        if self.controllers.report_generation != Some(snapshot.generation) {
            self.controller_error(
                "Controllers changed during discovery. Choose Refresh Controllers.".into(),
            );
        } else {
            self.controllers.inventory_generation = snapshot.generation;
            self.controllers.devices = snapshot.devices;
            if self.controllers.devices.is_empty() {
                self.message = Some("No controllers are connected. Connect a controller, then choose Refresh Controllers.".into());
            }
        }
    }

    fn controller_heading_help(&self) -> (String, String, String) {
        let name = self
            .controller_device()
            .map(|device| device.name.as_str())
            .unwrap_or("Controllers");
        if let Some((ControllerJob::Capture(role), _)) = &self.controllers.job {
            return (format!("Set {}", self.controller_role_source(*role).label()),
                "Press and release a button or move an axis. Hold that control for two seconds to cancel. Ordinary keyboard Esc also cancels.".into(), String::new());
        }
        if self.controllers.job.is_some() {
            return (
                "Controllers".into(),
                "Waiting for MiSTer Main…".into(),
                String::new(),
            );
        }
        match self.controllers.menu {
            Some(ControllerMenu::List) => ("Controllers".into(), "Choose a connected controller. Separate input numbers identify connected instances.".into(), self.control_hint("A Select   B Back")),
            Some(ControllerMenu::Device) => (name.into(), "Changes apply only to Degauss. MiSTer, games, scripts and return shortcuts are unchanged.".into(), self.control_hint("A Select   B Back")),
            Some(ControllerMenu::Editor) => (format!("{name} / Degauss Controls"), "Set replaces this action's controls. Save keeps a complete Degauss-only mapping. Existing swaps and holds remain in effect.".into(), self.control_hint("A Set   X Clear   B Cancel")),
            Some(ControllerMenu::Test) => (format!("{name} / Test"), "Raw input is for this controller; Latest Action includes any controller. Ordinary keyboard arrows scroll; Esc exits.".into(), format!("Hold {} 2s to exit", self.controllers.active.as_ref().or_else(|| self.controller_device())
                .filter(|device| device.custom || device.mapped).map(|device| device.hint("B", self.settings.swap_a_b.unwrap_or(false), self.settings.swap_x_y.unwrap_or(false)))
                .unwrap_or_else(|| "Back".into()))),
            None => (String::new(), String::new(), String::new()),
        }
    }

    fn controller_option_label(&self, option: OptionId) -> String {
        let role = match option {
            OptionId::HoldA => Control::Select,
            OptionId::HoldB => Control::Back,
            OptionId::HoldX => Control::Actions,
            OptionId::HoldY => Control::Menu,
            _ => return option.label().into(),
        };
        self.controllers
            .active
            .as_ref()
            .map(|device| format!("Hold {}", device.labels[role.index()]))
            .unwrap_or_else(|| option.label().into())
    }
}

#[cfg(test)]
fn test_controller_menu_flow(app: &mut App) {
    let screen = app.screen;
    let menu = app.menu.clone();
    let menu_list = app.menu_list.clone();
    let saved = app.settings.clone();
    let message = app.message.take();
    app.settings.controller_profiles.insert(
        "invalid".into(),
        ControllerProfile {
            custom: true,
            bindings: Default::default(),
        },
    );
    app.poll_controllers(Instant::now());
    assert!(app.controllers.initialized && app.controllers.job.is_none());
    assert!(app
        .message
        .take()
        .is_some_and(|text| text.contains("missing")));
    assert!(app.settings.controller_profiles["invalid"].custom);
    app.poll_controllers(Instant::now());
    assert!(
        app.message.is_none(),
        "an invalid saved profile is not retried every frame"
    );
    app.settings = saved.clone();
    app.receive_controller_discovery(controller::Snapshot::default());
    assert!(
        app.message
            .take()
            .is_some_and(|text| text.contains("Controllers changed")),
        "a failed discovery cannot be presented as an empty controller list"
    );
    app.controllers.report_generation = Some(0);
    app.controllers.report_error = Some("Reading active controller: Permission denied".into());
    app.message = app.controllers.report_error.clone();
    app.receive_controller_discovery(controller::Snapshot::default());
    assert_eq!(
        app.message.take(),
        app.controllers.report_error,
        "a failed report remains visible even when the last generation matches discovery"
    );
    app.receive_controller_discovery(controller::Snapshot::default());
    assert!(app.message.as_ref().is_some_and(|message| message.contains("Reading active controller: Permission denied")),
        "explicit discovery repeats a dismissed underlying report error");
    let retry_message = app.message.clone();
    app.receive_controller_discovery(controller::Snapshot::default());
    assert_eq!(
        app.message, retry_message,
        "an error already on screen is not duplicated"
    );
    app.message = None;
    app.controllers.report_error = None;
    app.receive_controller_discovery(controller::Snapshot::default());
    assert!(app
        .message
        .take()
        .is_some_and(|text| text.contains("No controllers are connected")));
    app.controllers.report_generation = None;
    app.pending = Some(Pending::Exit);
    app.message = Some("Exit Degauss?".into());
    app.controller_error("Controller disconnected".into());
    assert_eq!(app.pending, Some(Pending::Exit));
    assert_eq!(
        app.message.as_deref(),
        Some("Exit Degauss?\n\nController settings failed: Controller disconnected"),
        "a controller error cannot hide the question whose action is pending"
    );
    assert!(app.handle(Action::Quit).is_none());
    assert!(app.pending.is_none() && app.message.is_none());
    app.controller_error("Controller disconnected".into());
    assert!(
        app.handle(Action::Accept).is_none(),
        "dismissing a plain controller error cannot execute an old confirmation"
    );
    let device = controller::tests::device();
    let report = app
        .settings_path
        .parent()
        .unwrap()
        .join("controller-report.status");
    std::fs::write(&report, controller::tests::snapshot_bytes(&device, true)).unwrap();
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(
        app.controllers.reset_input,
        "first report clears pending input"
    );
    assert!(
        app.controllers.raw_nodes.is_empty(),
        "ordinary controller nodes cannot suppress keyboard input"
    );
    app.controllers.reset_input = false;
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(
        !app.controllers.reset_input,
        "unchanged reports do not interrupt holds"
    );
    let mut keyboard_pad = device.clone();
    keyboard_pad.keyboard_joystick = true;
    let mut bytes = controller::tests::snapshot_bytes(&keyboard_pad, true);
    bytes[8..10].copy_from_slice(&43_u16.to_le_bytes());
    let next = report.with_extension("part");
    std::fs::write(&next, bytes).unwrap();
    std::fs::rename(&next, &report).unwrap();
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(
        app.controllers.reset_input,
        "Main device rescans cancel pending taps and holds before generated releases"
    );
    assert!(
        app.controllers.raw_nodes.contains(&3),
        "custom keyboard-style controls cannot bypass the saved controller mapping"
    );
    keyboard_pad.custom = false;
    keyboard_pad.effective = std::array::from_fn(|role| keyboard_pad.native[role][0].code);
    keyboard_pad.labels = std::array::from_fn(|role| keyboard_pad.native[role][0].label.clone());
    keyboard_pad.native[Control::Up.index()].push(controller::Binding {
        code: 100,
        label: "Mouse Up".into(),
    });
    keyboard_pad.native[Control::Up.index()].push(controller::Binding {
        code: 770,
        label: "Stick Up".into(),
    });
    let mut bytes = controller::tests::snapshot_bytes(&keyboard_pad, true);
    bytes[8..10].copy_from_slice(&43_u16.to_le_bytes());
    std::fs::write(&next, bytes).unwrap();
    std::fs::rename(&next, &report).unwrap();
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(
        !app.controllers.raw_nodes.contains(&3),
        "inherited mode preserves existing raw keyboard actions, including unassigned Escape"
    );
    assert!(app.controllers.raw_keys.contains(&(3, 10)));
    assert!(app.controllers.raw_keys.contains(&(3, 100)));
    assert!(app.controllers.raw_keys.contains(&(3, 770)));
    assert!(
        !app.controllers.raw_keys.contains(&(3, 1)),
        "unassigned raw Escape is retained"
    );
    assert!(
        !app.controllers.raw_keys.contains(&(3, 96)),
        "unassigned keypad Enter is retained"
    );
    assert!(
        !app.controllers.raw_keys.contains(&(4, 10)),
        "an ordinary keyboard node is not filtered"
    );
    app.controllers.devices = vec![keyboard_pad.clone()];
    assert!(
        !app.controller_consumes_raw_key(3, 1),
        "native browsing retains unassigned Escape"
    );
    assert!(
        app.controller_consumes_raw_key(3, 10),
        "Main-translated keys are never duplicated"
    );
    app.controllers.menu = Some(ControllerMenu::Test);
    assert!(
        app.controller_consumes_raw_key(3, 1),
        "a controller's raw Escape cannot exit its own tester"
    );
    assert!(
        app.controller_consumes_raw_key(3, 96),
        "unmapped controller keys remain diagnostic input"
    );
    assert!(
        !app.controller_consumes_raw_key(4, 1),
        "ordinary keyboard Escape remains available"
    );
    app.invalidate_controller_report();
    assert!(
        !app.controller_consumes_raw_key(3, 1),
        "stale device paths cannot suppress a reused keyboard node"
    );
    app.controllers.menu = None;
    app.controllers.devices.clear();
    std::fs::write(&next, b"invalid report").unwrap();
    std::fs::rename(&next, &report).unwrap();
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(app.controllers.raw_nodes.is_empty() && app.controllers.raw_keys.is_empty());
    assert!(app.controllers.report_revision.is_none());
    assert!(app
        .message
        .take()
        .is_some_and(|text| text.contains("Reading active controller")));
    std::fs::write(
        &next,
        controller::tests::snapshot_bytes(&keyboard_pad, true),
    )
    .unwrap();
    std::fs::rename(&next, &report).unwrap();
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(
        app.controllers.raw_keys.contains(&(3, 10)),
        "a corrected report restores current key routing"
    );
    app.invalidate_controller_report();
    assert!(app.controllers.raw_nodes.is_empty() && app.controllers.raw_keys.is_empty());
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(
        app.controllers.raw_keys.contains(&(3, 10)),
        "device loss forces the report to be reread"
    );
    std::fs::remove_file(&report).unwrap();
    app.poll_controller_report_at(Instant::now(), true, &report);
    assert!(app.controllers.raw_nodes.is_empty() && app.controllers.raw_keys.is_empty());
    app.controllers.devices = vec![device.clone()];
    app.controllers.inventory_generation = 43;
    app.controllers.active = Some(device);
    app.controllers.selected = 0;
    app.controller_menu_to(ControllerMenu::Device);
    app.screen = Screen::Context;
    app.refresh();
    assert_eq!(app.ui.get_heading().as_str(), "Controller");
    assert!(app.handle_controller_menu(Action::Down));
    app.controllers.devices[0].mapped = false;
    assert!(app.handle_controller_menu(Action::Accept));
    assert_eq!(app.controllers.menu, Some(ControllerMenu::Device));
    assert!(app
        .message
        .as_ref()
        .is_some_and(|text| text.contains("Press a control")));
    app.handle(Action::Quit);
    assert!(app.message.is_none());
    app.controllers.devices[0].mapped = true;
    assert!(app.handle_controller_menu(Action::Accept));
    assert_eq!(app.controllers.menu, Some(ControllerMenu::Editor));
    app.refresh();
    assert_eq!(
        app.ui.get_heading().as_str(),
        "Controller / Degauss Controls"
    );
    assert_eq!(
        app.available_hold_shortcuts(),
        [None; 4],
        "editing controls cannot run browsing hold actions"
    );
    assert_eq!(app.controller_rows().len(), controller::ROLE_COUNT + 1);
    assert!(app.controllers.draft.custom);
    assert!(
        app.settings.controller_profiles.is_empty(),
        "editing does not change saved settings before Save"
    );
    let draft = app.controllers.draft.clone();
    app.controllers
        .draft
        .bindings
        .insert(Control::Select, draft.bindings[&Control::Menu].clone());
    app.menu_list.select(controller::ROLE_COUNT);
    assert!(app.handle_controller_menu(Action::Accept));
    assert_eq!(
        app.message.as_deref(),
        Some("Controller settings failed: The same control is assigned more than once: Y"),
        "conflicts identify the duplicated physical control after both button swaps"
    );
    app.handle(Action::Quit);
    app.controllers.draft = draft;
    app.menu_list.select(Control::Actions.index());
    assert!(app.handle_controller_menu(Action::Context));
    assert_eq!(
        app.controllers.draft.bindings.get(&Control::Menu),
        Some(&Vec::new()),
        "the editor inverts Swap X/Y once"
    );
    assert!(app.handle_controller_menu(Action::Menu));
    assert!(
        app.controllers.draft.bindings[&Control::Menu].is_empty(),
        "custom editing has no live per-row MiSTer inheritance"
    );
    assert_eq!(
        app.controller_option_label(OptionId::HoldA),
        "Hold L",
        "hold settings name the pre-swap physical control"
    );
    assert_eq!(app.control_hint("A Select B Back"), "R Select L Back");
    assert!(app.handle_controller_menu(Action::Quit));
    assert_eq!(app.controllers.menu, Some(ControllerMenu::Device));
    app.controller_menu_to(ControllerMenu::Test);
    let now = Instant::now();
    assert!(app.controller_intercept_edge(KeyEdge::Down(Action::Accept), true, now));
    assert_eq!(app.controllers.test_action, "Select pressed");
    assert!(app.controller_intercept_edge(KeyEdge::Down(Action::Quit), true, now));
    assert_eq!(app.controllers.test_back_since, Some(now));
    assert!(app.controller_intercept_edge(KeyEdge::Up(Action::Quit), true, now));
    assert!(app.controllers.test_back_since.is_none());
    assert!(app.controller_intercept_edge(KeyEdge::Down(Action::Quit), false, now));
    assert_eq!(app.controllers.menu, Some(ControllerMenu::Device));
    app.controllers = ControllerBrowser::default();
    app.screen = screen;
    app.menu = menu;
    app.menu_list = menu_list;
    app.settings = saved;
    app.message = message;
    app.apply_geometry();
}
