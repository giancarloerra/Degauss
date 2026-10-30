//! Degauss-only controller profiles and the acknowledged Main interface.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const ROLE_COUNT: usize = 10;
const VERSION: u16 = 2;
const MAX_DEVICES: usize = 30;
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Control {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Actions,
    Menu,
    PreviousPage,
    NextPage,
}

impl Control {
    pub const ALL: [Self; ROLE_COUNT] = [
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Select,
        Self::Back,
        Self::Actions,
        Self::Menu,
        Self::PreviousPage,
        Self::NextPage,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Select => "Select",
            Self::Back => "Back",
            Self::Actions => "Actions",
            Self::Menu => "Menu",
            Self::PreviousPage => "Previous Page",
            Self::NextPage => "Next Page",
        }
    }

    pub const fn required(self) -> bool {
        matches!(
            self,
            Self::Up | Self::Down | Self::Left | Self::Right | Self::Select | Self::Back
        )
    }

    /// Existing swaps are resolved exactly once by the normal input path.
    /// The editor names the final action, not its pre-swap control role.
    pub fn source(self, swap_ab: bool, swap_xy: bool) -> Self {
        match self {
            Self::Select if swap_ab => Self::Back,
            Self::Back if swap_ab => Self::Select,
            Self::Actions if swap_xy => Self::Menu,
            Self::Menu if swap_xy => Self::Actions,
            _ => self,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub code: u16,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub custom: bool,
    /// Complete saved controls. Empty optional rows are explicitly unassigned.
    /// Directions may retain the native D-pad and stick binding copied at setup.
    #[serde(default)]
    pub bindings: BTreeMap<Control, Vec<Binding>>,
}

impl Profile {
    pub fn from_device(device: &Device) -> Self {
        let bindings = Control::ALL
            .into_iter()
            .map(|role| {
                let at = role.index();
                let code = device.native[at];
                let mut bindings = Vec::new();
                if code != 0 {
                    bindings.push(Binding {
                        code,
                        label: device.native_labels[at].clone(),
                    });
                }
                if at < 4 {
                    let code = device.native_alternates[at];
                    if code != 0 && !bindings.iter().any(|binding| binding.code == code) {
                        bindings.push(Binding {
                            code,
                            label: device.native_alternate_labels[at].clone(),
                        });
                    }
                }
                (role, bindings)
            })
            .collect();
        Self {
            custom: true,
            bindings,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.custom {
            return Ok(());
        }
        let mut seen = BTreeMap::new();
        for role in Control::ALL {
            let bindings = self
                .bindings
                .get(&role)
                .ok_or_else(|| format!("{} is missing from the saved mapping", role.label()))?;
            if role.required() && bindings.is_empty() {
                return Err(format!("{} must have a control", role.label()));
            }
            if bindings.len() > if role.index() < 4 { 2 } else { 1 } {
                return Err(format!("Too many bindings for {}", role.label()));
            }
            for binding in bindings {
                if binding.code == 0
                    || binding.label.is_empty()
                    || binding.label.len() > 48
                    || binding.label.contains('\0')
                {
                    return Err(format!("Invalid binding for {}", role.label()));
                }
                if seen.insert(binding.code, role).is_some() {
                    return Err(format!(
                        "The same control is assigned more than once: {}",
                        binding.label
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub instance: u16,
    pub keyboard_joystick: bool,
    pub custom: bool,
    pub mapped: bool,
    pub id: String,
    pub name: String,
    pub paths: Vec<PathBuf>,
    pub native: [u16; ROLE_COUNT],
    pub native_alternates: [u16; 4],
    pub effective: [u16; ROLE_COUNT],
    pub native_labels: [String; ROLE_COUNT],
    pub native_alternate_labels: [String; 4],
    pub labels: [String; ROLE_COUNT],
}

impl Device {
    pub fn hint(&self, text: &str, swap_ab: bool, swap_xy: bool) -> String {
        let chars: Vec<_> = text.chars().collect();
        let mut result = String::new();
        for (at, ch) in chars.iter().enumerate() {
            let standalone = (at == 0 || !chars[at - 1].is_alphanumeric())
                && (at + 1 == chars.len() || !chars[at + 1].is_alphanumeric());
            let role = match ch {
                'A' => Some(Control::Select),
                'B' => Some(Control::Back),
                'X' => Some(Control::Actions),
                'Y' => Some(Control::Menu),
                'L' => Some(Control::PreviousPage),
                'R' => Some(Control::NextPage),
                _ => None,
            };
            if let Some(role) = role.filter(|_| standalone) {
                let label = &self.labels[role.source(swap_ab, swap_xy).index()];
                if let Some(number) = label.strip_prefix("Button ") {
                    result.push('#');
                    result.push_str(number);
                } else {
                    result.push_str(label);
                }
            } else {
                result.push(*ch);
            }
        }
        result
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub generation: u16,
    pub devices: Vec<Device>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Snapshot(Snapshot),
    Active {
        instance: Option<u16>,
        snapshot: Snapshot,
    },
    Applied,
    Captured {
        instance: u16,
        code: u16,
        label: String,
    },
    Cancelled,
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("MiSTer controller reply: {detail}"),
    )
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn word(&mut self) -> io::Result<u16> {
        if self.0.len() < 2 {
            return Err(invalid("truncated field"));
        }
        let value = u16::from_le_bytes([self.0[0], self.0[1]]);
        self.0 = &self.0[2..];
        Ok(value)
    }
    fn text(&mut self) -> io::Result<String> {
        let length = usize::from(self.word()?);
        if length > 1024 || self.0.len() < length {
            return Err(invalid("invalid string length"));
        }
        let text = std::str::from_utf8(&self.0[..length])
            .map_err(|_| invalid("invalid UTF-8"))?
            .to_owned();
        self.0 = &self.0[length..];
        Ok(text)
    }
    fn controls(&mut self) -> io::Result<[u16; ROLE_COUNT]> {
        let mut result = [0; ROLE_COUNT];
        for code in &mut result {
            *code = self.word()?;
        }
        Ok(result)
    }
}

pub fn parse_reply(bytes: &[u8]) -> io::Result<Reply> {
    if let Some(detail) = bytes.strip_prefix(b"error ") {
        return Err(io::Error::other(
            String::from_utf8_lossy(detail).trim().to_owned(),
        ));
    }
    if bytes.len() < 6 {
        return Err(invalid("missing header"));
    }
    let mut reader = Reader(&bytes[4..]);
    if reader.word()? != VERSION {
        return Err(invalid("unsupported protocol version"));
    }
    let result = match &bytes[..4] {
        b"DGCA" => Reply::Applied,
        b"DGCX" => Reply::Cancelled,
        b"DGCC" => Reply::Captured {
            instance: reader.word()?,
            code: reader.word()?,
            label: reader.text()?,
        },
        b"DGCS" | b"DGCR" => {
            let active = if &bytes[..4] == b"DGCR" {
                Some(reader.word()?)
            } else {
                None
            };
            let generation = reader.word()?;
            let count = usize::from(reader.word()?);
            if count > MAX_DEVICES {
                return Err(invalid("too many devices"));
            }
            let mut devices = Vec::with_capacity(count);
            for _ in 0..count {
                let instance = reader.word()?;
                let flags = reader.word()?;
                if flags > 7 {
                    return Err(invalid("unknown controller flags"));
                }
                let id = reader.text()?;
                let name = reader.text()?;
                let count = usize::from(reader.word()?);
                if count > MAX_DEVICES || id.is_empty() {
                    return Err(invalid("invalid device identity"));
                }
                let mut paths = Vec::with_capacity(count);
                for _ in 0..count {
                    let path = PathBuf::from(reader.text()?);
                    if !path.starts_with("/dev/input") {
                        return Err(invalid("invalid input path"));
                    }
                    paths.push(path);
                }
                let native = reader.controls()?;
                let mut native_alternates = [0; 4];
                for code in &mut native_alternates {
                    *code = reader.word()?;
                }
                let effective = reader.controls()?;
                let mut native_labels: [String; ROLE_COUNT] =
                    std::array::from_fn(|_| String::new());
                for label in &mut native_labels {
                    *label = reader.text()?;
                }
                let mut native_alternate_labels = std::array::from_fn(|_| String::new());
                for label in &mut native_alternate_labels {
                    *label = reader.text()?;
                }
                let mut labels: [String; ROLE_COUNT] = std::array::from_fn(|_| String::new());
                for label in &mut labels {
                    *label = reader.text()?;
                }
                if devices
                    .iter()
                    .any(|device: &Device| device.instance == instance)
                {
                    return Err(invalid("duplicate device instance"));
                }
                devices.push(Device {
                    instance,
                    keyboard_joystick: flags & 1 != 0,
                    custom: flags & 2 != 0,
                    mapped: flags & 4 != 0,
                    id,
                    name,
                    paths,
                    native,
                    native_alternates,
                    effective,
                    native_labels,
                    native_alternate_labels,
                    labels,
                });
            }
            let snapshot = Snapshot {
                generation,
                devices,
            };
            if let Some(instance) = active {
                let instance = (instance != u16::MAX).then_some(instance);
                if instance
                    .is_some_and(|id| !snapshot.devices.iter().any(|device| device.instance == id))
                {
                    return Err(invalid("active controller is unavailable"));
                }
                Reply::Active { instance, snapshot }
            } else {
                Reply::Snapshot(snapshot)
            }
        }
        _ => return Err(invalid("unknown response")),
    };
    if !reader.0.is_empty() {
        return Err(invalid("trailing data"));
    }
    Ok(result)
}

pub fn runtime_profiles(profiles: &BTreeMap<String, Profile>) -> io::Result<Vec<u8>> {
    let selected = profiles
        .iter()
        .filter(|(_, profile)| profile.custom)
        .collect::<Vec<_>>();
    if selected.len() > MAX_DEVICES {
        return Err(invalid("too many custom profiles"));
    }
    let mut bytes = b"DGCP".to_vec();
    bytes.extend(VERSION.to_le_bytes());
    bytes.extend((selected.len() as u16).to_le_bytes());
    for (id, profile) in selected {
        profile.validate().map_err(|error| invalid(&error))?;
        if id.is_empty() || id.len() > 255 {
            return Err(invalid("invalid profile identity"));
        }
        bytes.extend((id.len() as u16).to_le_bytes());
        bytes.extend(id.as_bytes());
        for role in Control::ALL {
            let bindings = &profile.bindings[&role];
            bytes.extend((bindings.len() as u16).to_le_bytes());
            for binding in bindings {
                bytes.extend(binding.code.to_le_bytes());
                bytes.extend((binding.label.len() as u16).to_le_bytes());
                bytes.extend(binding.label.as_bytes());
            }
        }
    }
    if bytes.len() > 32768 {
        return Err(invalid("controller profile payload is too large"));
    }
    Ok(bytes)
}

pub enum Request {
    Discover,
    Apply(Vec<u8>),
    Capture { instance: u16, generation: u16 },
}

pub struct Job {
    result: mpsc::Receiver<Result<Reply, String>>,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn start(fifo: &Path, request: Request) -> io::Result<Self> {
        let fifo = fifo.to_owned();
        let (sender, result) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancellation = cancel.clone();
        std::thread::Builder::new()
            .name("degauss-controller".into())
            .spawn(move || {
                let reply = exchange_cancellable(&fifo, request, &cancellation)
                    .map_err(|error| error.to_string());
                let _ = sender.send(reply);
            })?;
        Ok(Self { result, cancel })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn try_recv(&self) -> Option<Result<Reply, String>> {
        match self.result.try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Controller request disconnected".into()))
            }
        }
    }
}

#[cfg(test)]
fn exchange(fifo: &Path, request: Request) -> io::Result<Reply> {
    exchange_cancellable(fifo, request, &AtomicBool::new(false))
}

fn exchange_cancellable(fifo: &Path, request: Request, cancel: &AtomicBool) -> io::Result<Reply> {
    let token = 100_000_000
        + std::process::id()
            .wrapping_mul(1_000_003)
            .wrapping_add(SEQUENCE.fetch_add(1, Ordering::Relaxed))
            % 900_000_000;
    let status = PathBuf::from(format!("/tmp/degauss-controller-{token}.status"));
    let data = PathBuf::from(format!("/tmp/degauss-controller-{token}.data"));
    let operation = match request {
        Request::Discover => "list".to_owned(),
        Request::Capture {
            instance,
            generation,
        } => format!("capture {instance} {generation}"),
        Request::Apply(bytes) => {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&data)?;
            if let Err(error) = file.write_all(&bytes) {
                let _ = std::fs::remove_file(&data);
                return Err(error);
            }
            "apply".to_owned()
        }
    };
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        options
            .open(fifo)?
            .write_all(format!("degauss_controller {token} {operation}\n").as_bytes())?;
        let timeout = if operation.starts_with("capture ") {
            30
        } else {
            3
        };
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let mut capture_started = false;
        let mut cancel_sent = false;
        while Instant::now() < deadline {
            // Main's FIFO reads a command at a time. Wait for the capture's
            // acknowledgement before sending its cancellation, so the two
            // writes cannot be consumed as a single malformed command.
            if capture_started && cancel.load(Ordering::Relaxed) && !cancel_sent {
                options
                    .open(fifo)?
                    .write_all(format!("degauss_controller {token} cancel\n").as_bytes())?;
                cancel_sent = true;
            }
            match std::fs::read(&status) {
                Ok(bytes) if bytes == b"DGCB\x02\0" && operation.starts_with("capture ") => {
                    capture_started = true
                }
                Ok(bytes) => {
                    let reply = parse_reply(&bytes)?;
                    if matches!(reply, Reply::Captured { .. }) && cancel.load(Ordering::Relaxed) {
                        if !cancel_sent {
                            return Ok(Reply::Cancelled);
                        }
                    } else {
                        return Ok(reply);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(io::Error::new(io::ErrorKind::TimedOut,
            "MiSTer Main did not answer the controller request. Update the Degauss Main installation."))
    })();
    for path in [&status, &data] {
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(error);
            }
        }
    }
    result
}

/// Raw evdev readers exist only while the tester is open. They never take
/// exclusive ownership; Main continues to perform controller translation.
#[cfg(target_os = "linux")]
pub struct Tester {
    devices: Vec<(PathBuf, evdev::Device)>,
    buttons: BTreeMap<(usize, u16), bool>,
    axes: BTreeMap<(usize, u16), (i32, i32, i32)>,
    last: String,
}

#[cfg(target_os = "linux")]
impl Tester {
    pub fn open(paths: &[PathBuf]) -> io::Result<Self> {
        let mut result = Self {
            devices: Vec::new(),
            buttons: BTreeMap::new(),
            axes: BTreeMap::new(),
            last: "Press a button or move an axis".into(),
        };
        for path in paths {
            let device = evdev::Device::open(path)?;
            device.set_nonblocking(true)?;
            let index = result.devices.len();
            if device.supported_keys().is_some() {
                for key in device.get_key_state()?.iter() {
                    result.buttons.insert((index, key.code()), true);
                }
            }
            if device.supported_absolute_axes().is_some() {
                for (code, info) in device.get_absinfo()? {
                    result.axes.insert(
                        (index, code.0),
                        (info.value(), info.minimum(), info.maximum()),
                    );
                }
            }
            result.devices.push((path.clone(), device));
        }
        if result.devices.is_empty() {
            return Err(io::Error::other(
                "No input nodes for the selected controller",
            ));
        }
        Ok(result)
    }

    pub fn poll(&mut self) -> io::Result<bool> {
        let mut changed = false;
        for (index, (path, device)) in self.devices.iter_mut().enumerate() {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", path.display()),
                    ))
                }
            };
            for event in events {
                match event.destructure() {
                    evdev::EventSummary::Key(_, code, value) => {
                        self.buttons.insert((index, code.code()), value != 0);
                        self.last = format!(
                            "Button {}:{} {}",
                            index + 1,
                            code.code(),
                            if value == 0 { "released" } else { "pressed" }
                        );
                        changed = true;
                    }
                    evdev::EventSummary::AbsoluteAxis(_, code, value) => {
                        if let Some(axis) = self.axes.get_mut(&(index, code.0)) {
                            changed |= axis.0 != value;
                            axis.0 = value;
                            self.last = format!(
                                "Axis {}:{} {value} ({}..{})",
                                index + 1,
                                code.0,
                                axis.1,
                                axis.2
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(changed)
    }

    pub fn rows(&self) -> Vec<(String, String)> {
        let pressed = self
            .buttons
            .iter()
            .filter(|(_, down)| **down)
            .map(|((node, code), _)| format!("{}:{code}", node + 1))
            .collect::<Vec<_>>();
        let mut rows = vec![
            ("Last Input".into(), self.last.clone()),
            (
                "Buttons (node:code)".into(),
                if pressed.is_empty() {
                    "None".into()
                } else {
                    pressed.join(" ")
                },
            ),
        ];
        rows.extend(self.axes.iter().map(|((node, code), (value, min, max))| {
            (
                format!("Axis {}:{code}", node + 1),
                format!("{value} ({min}..{max})"),
            )
        }));
        rows
    }
}

#[cfg(not(target_os = "linux"))]
pub struct Tester;
#[cfg(not(target_os = "linux"))]
impl Tester {
    pub fn open(_: &[PathBuf]) -> io::Result<Self> {
        Err(io::Error::other(
            "Controller testing requires MiSTer Linux input devices",
        ))
    }
    pub fn poll(&mut self) -> io::Result<bool> {
        Ok(false)
    }
    pub fn rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn device() -> Device {
        Device {
            instance: 1,
            keyboard_joystick: false,
            custom: true,
            mapped: true,
            id: "pad".into(),
            name: "Controller".into(),
            paths: vec!["/dev/input/event3".into()],
            native: [10, 11, 12, 13, 14, 15, 16, 17, 18, 19],
            native_alternates: [0; 4],
            effective: [10, 11, 12, 13, 18, 19, 16, 17, 300, 301],
            native_labels: ["Up", "Down", "Left", "Right", "A", "B", "X", "Y", "L", "R"]
                .map(str::to_owned),
            native_alternate_labels: std::array::from_fn(|_| "Unassigned".into()),
            labels: [
                "Up",
                "Down",
                "Left",
                "Right",
                "L",
                "R",
                "X",
                "Y",
                "Button 9",
                "Button 10",
            ]
            .map(str::to_owned),
        }
    }

    #[test]
    fn prompts_follow_actual_bindings_and_swaps_without_rewriting_words() {
        let device = device();
        assert_eq!(
            device.hint("A Select B Back X Actions Y Menu L/R Page", false, false),
            "L Select R Back X Actions Y Menu #9/#10 Page"
        );
        assert_eq!(
            device.hint("A Select B Back X Actions Y Menu", true, true),
            "R Select L Back Y Actions X Menu"
        );
        assert_eq!(
            device.hint("AB XY ROM ARCADE", true, true),
            "AB XY ROM ARCADE"
        );
    }

    pub(crate) fn snapshot_bytes(device: &Device, active: bool) -> Vec<u8> {
        fn text(out: &mut Vec<u8>, text: &str) {
            out.extend((text.len() as u16).to_le_bytes());
            out.extend(text.as_bytes());
        }
        let mut bytes = if active {
            b"DGCR\x02\0".to_vec()
        } else {
            b"DGCS\x02\0".to_vec()
        };
        if active {
            bytes.extend(device.instance.to_le_bytes());
        }
        bytes.extend(42_u16.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend(device.instance.to_le_bytes());
        bytes.extend(
            (u16::from(device.keyboard_joystick)
                | (u16::from(device.custom) << 1)
                | (u16::from(device.mapped) << 2))
                .to_le_bytes(),
        );
        text(&mut bytes, &device.id);
        text(&mut bytes, &device.name);
        bytes.extend((device.paths.len() as u16).to_le_bytes());
        for path in &device.paths {
            text(&mut bytes, path.to_str().unwrap());
        }
        for code in device
            .native
            .into_iter()
            .chain(device.native_alternates)
            .chain(device.effective)
        {
            bytes.extend(code.to_le_bytes());
        }
        for label in device
            .native_labels
            .iter()
            .chain(&device.native_alternate_labels)
            .chain(&device.labels)
        {
            text(&mut bytes, label);
        }
        bytes
    }

    #[test]
    fn snapshots_preserve_instance_paths_native_controls_and_effective_labels() {
        let device = device();
        assert_eq!(
            parse_reply(&snapshot_bytes(&device, false)).unwrap(),
            Reply::Snapshot(Snapshot {
                generation: 42,
                devices: vec![device.clone()]
            })
        );
        assert_eq!(
            parse_reply(&snapshot_bytes(&device, true)).unwrap(),
            Reply::Active {
                instance: Some(1),
                snapshot: Snapshot {
                    generation: 42,
                    devices: vec![device]
                }
            }
        );
        assert_eq!(
            parse_reply(b"DGCR\x02\0\xff\xff\0\0\0\0").unwrap(),
            Reply::Active {
                instance: None,
                snapshot: Snapshot::default()
            }
        );
        assert!(parse_reply(b"DGCR\x02\0\x01\0\0\0\0\0").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn requests_exercise_real_fifo_status_and_profile_file_boundaries() {
        use std::io::BufRead;
        fn status(token: u32, bytes: &[u8]) {
            // Match Main's atomic reply replacement. Truncating the live
            // status file would expose a partial header the real producer
            // never publishes, especially between capture start and cancel.
            let path = PathBuf::from(format!("/tmp/degauss-controller-{token}.status"));
            let part = path.with_extension("part");
            std::fs::write(&part, bytes).unwrap();
            std::fs::rename(part, path).unwrap();
        }
        let unique = format!(
            "degauss-controls-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir(&root).unwrap();
        let fifo = root.join("MiSTer_cmd");
        let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        let worker = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(file);
            for expected in ["list", "apply", "capture", "capture-cancel"] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let words = line.split_whitespace().collect::<Vec<_>>();
                assert_eq!(words[0], "degauss_controller");
                if expected.starts_with("capture") {
                    assert_eq!(&words[3..], &["1", "42"]);
                }
                assert_eq!(
                    words[2],
                    if expected == "capture-cancel" {
                        "capture"
                    } else {
                        expected
                    }
                );
                let token = words[1].parse::<u32>().unwrap();
                let response = match expected {
                    "list" => snapshot_bytes(&device(), false),
                    "apply" => {
                        let bytes =
                            std::fs::read(format!("/tmp/degauss-controller-{token}.data")).unwrap();
                        assert_eq!(bytes, b"DGCP\x02\0\0\0");
                        b"DGCA\x02\0".to_vec()
                    }
                    "capture-cancel" => {
                        status(token, b"DGCB\x02\0");
                        let mut cancel = String::new();
                        reader.read_line(&mut cancel).unwrap();
                        assert_eq!(cancel, format!("degauss_controller {token} cancel\n"));
                        b"DGCX\x02\0".to_vec()
                    }
                    _ => b"error The selected controller disconnected\n".to_vec(),
                };
                status(token, &response);
            }
        });
        assert!(matches!(
            exchange(&fifo, Request::Discover).unwrap(),
            Reply::Snapshot(_)
        ));
        assert_eq!(
            exchange(
                &fifo,
                Request::Apply(runtime_profiles(&BTreeMap::new()).unwrap())
            )
            .unwrap(),
            Reply::Applied
        );
        assert!(exchange(
            &fifo,
            Request::Capture {
                instance: 1,
                generation: 42
            }
        )
        .unwrap_err()
        .to_string()
        .contains("disconnected"));
        let job = Job::start(
            &fifo,
            Request::Capture {
                instance: 1,
                generation: 42,
            },
        )
        .unwrap();
        job.cancel(); // Cancellation before Main acknowledges capture must not coalesce FIFO commands.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = job.try_recv() {
                assert_eq!(result.unwrap(), Reply::Cancelled);
                break;
            }
            assert!(Instant::now() < deadline, "capture cancellation timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        worker.join().unwrap();
        std::fs::remove_file(fifo).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn disabled_profiles_leave_main_in_native_mode_without_deleting_the_saved_map() {
        let mut profile = Profile::from_device(&device());
        profile.custom = false;
        assert_eq!(profile.bindings[&Control::Select][0].code, 14);
        assert_eq!(
            runtime_profiles(&[("pad".into(), profile)].into()).unwrap(),
            b"DGCP\x02\0\0\0"
        );
    }

    #[test]
    fn saved_controls_and_labels_do_not_follow_later_native_mapping_changes() {
        let mut device = device();
        let mut profile = Profile::from_device(&device);
        profile.bindings.insert(
            Control::Select,
            vec![Binding {
                code: 100,
                label: "L".into(),
            }],
        );
        assert_eq!(profile.bindings[&Control::Back][0].code, 15);
        assert_eq!(profile.validate(), Ok(()));
        let before = runtime_profiles(&[(device.id.clone(), profile.clone())].into()).unwrap();
        device.native[Control::Back.index()] = 100;
        device.native_labels[Control::Select.index()] = "R".into();
        assert_eq!(
            runtime_profiles(&[(device.id.clone(), profile.clone())].into()).unwrap(),
            before
        );
        assert_eq!(profile.bindings[&Control::Select][0].label, "L");
        profile.bindings.insert(
            Control::Back,
            vec![Binding {
                code: 100,
                label: "R".into(),
            }],
        );
        assert!(profile.validate().unwrap_err().contains("same control"));
    }

    #[test]
    fn action_editor_inverts_existing_swaps_without_applying_them_twice() {
        for role in Control::ALL {
            for ab in [false, true] {
                for xy in [false, true] {
                    assert_eq!(role.source(ab, xy).source(ab, xy), role);
                }
            }
        }
        assert_eq!(Control::Select.source(true, false), Control::Back);
        assert_eq!(Control::Actions.source(false, true), Control::Menu);
    }

    #[test]
    fn complete_profiles_preserve_stick_alternatives_and_explicit_unassignment() {
        let mut device = device();
        device.native_alternates = [770, 771, 768, 769];
        device.native_alternate_labels =
            ["Axis 1 -", "Axis 1 +", "Axis 0 -", "Axis 0 +"].map(str::to_owned);
        let mut profile = Profile::from_device(&device);
        assert_eq!(profile.bindings[&Control::Up].len(), 2);
        assert_eq!(profile.bindings[&Control::Up][1].code, 770);
        profile.bindings.insert(Control::Actions, Vec::new());
        assert_eq!(profile.validate(), Ok(()));
        let text = toml::to_string(&profile).unwrap();
        assert_eq!(toml::from_str::<Profile>(&text).unwrap(), profile);
        assert!(profile.bindings[&Control::Actions].is_empty());
        profile.bindings.remove(&Control::Menu);
        assert!(profile.validate().unwrap_err().contains("missing"));
    }

    #[test]
    fn malformed_or_incompatible_main_replies_are_errors_not_empty_successes() {
        for bytes in [
            &b""[..],
            &b"DGCS\x01\0\0\0"[..],
            &b"DGCS\x02\0\0\0\x1f\0"[..],
            &b"DGCA\x02\0x"[..],
        ] {
            assert!(parse_reply(bytes).is_err());
        }
        assert_eq!(
            parse_reply(b"DGCS\x02\0\0\0\0\0").unwrap(),
            Reply::Snapshot(Snapshot::default())
        );
        assert!(parse_reply(b"error Degauss controls are not active\n")
            .unwrap_err()
            .to_string()
            .contains("not active"));
    }
}
