//! Reading the controller and keyboard while MiSTer is still running.
//!
//! This module is shaped entirely by how MiSTer's Main process behaves, all
//! of it read from its source rather than assumed:
//!
//! * Main normally holds an EXCLUSIVE grab on every input device, so an
//!   ordinary process sees nothing at all. It releases that grab in exactly
//!   one situation: while a script runs on the framebuffer terminal, which
//!   is how Degauss is launched. Outside that situation Degauss will
//!   correctly report that it is receiving no input rather than appearing
//!   broken.
//! * Degauss must NEVER grab a device itself. Main re-grabs everything
//!   when the script ends and does not check whether that succeeded, so a
//!   grab held here would leave the user's controller dead until a reboot.
//!   There is no call to `grab()` anywhere in this file, and the tests
//!   assert the reader does not take one.
//! * While the framebuffer terminal is up, Main translates gamepad input
//!   into ordinary key events through its own virtual device: d-pad to the
//!   arrow keys, and the face buttons to Enter, Escape, Space and Tab. So
//!   reading the keyboard is enough to support a controller, and no
//!   per-pad mapping is needed.
//! * Keystrokes still reach the console as well, so the terminal is put
//!   into a quiet mode while Degauss draws and restored when it exits.
//!
//! Key repeat is generated here rather than taken from the kernel, because
//! the cadence of a held direction is a setting the user controls.
//!
//! Some controllers, or the input stack between them and Degauss, deliver
//! one physical press as two very fast press and release pairs. The second
//! pair is dropped by the [`DuplicateGuard`] before anything else sees it,
//! so one press moves once.

use std::time::{Duration, Instant, SystemTime};

/// What Degauss does, independent of which key or button produced it.
///
/// Only the device build turns real key codes into these; a development
/// machine constructs just the couple the benchmark drives, hence the
/// off-target allowance.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    /// Scroll slower: one step down the speed ladder.
    Slower,
    /// Scroll faster: one step up the speed ladder. This is the control that
    /// matters, because the question is how fast the list can move before it
    /// stops looking smooth.
    Faster,
    /// A screenful back. Keyboard only, and always a page whatever the
    /// Left and right setting makes of the stick.
    PageUp,
    /// A screenful on.
    PageDown,
    Home,
    End,
    /// Launch the selected entry.
    Accept,
    /// Go back: out of a folder, out of a screen, or to the menu at the top.
    Quit,
    /// Switch between the presentation paths being compared.
    CyclePresent,
    /// Open the menu.
    Menu,
    /// Open the contextual menu: what can be done with the folder on screen.
    Context,
    /// Add or remove the selected game as a favourite after X is held.
    FavoriteShortcut,
    RandomShortcut,
}

impl Action {
    /// Every variant in declaration order; sizes the [`DuplicateGuard`]
    /// table, see the check under `ACTION_SLOTS`.
    pub const ALL: [Action; 15] = [
        Action::Up,
        Action::Down,
        Action::Slower,
        Action::Faster,
        Action::PageUp,
        Action::PageDown,
        Action::Home,
        Action::End,
        Action::Accept,
        Action::Quit,
        Action::CyclePresent,
        Action::Menu,
        Action::Context,
        Action::FavoriteShortcut,
        Action::RandomShortcut,
    ];

    /// Whether holding the key always repeats. Only movement repeats:
    /// repeating "launch" would be dangerous, and repeating a speed change
    /// would run the whole ladder off one press. Left and right join in
    /// only through the [`Repeater`]'s own flag, while the Direction
    /// setting turns them into movement too.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn repeats(self) -> bool {
        matches!(self, Action::Up | Action::Down)
    }
}

/// Scroll speeds, expressed as a multiple of the baseline rate so the
/// interface can say "3x" instead of "30 ms per row". The baseline is one
/// row every 90 ms, which is what a conventional frontend gives for a held
/// direction; everything above it is the point of the exercise.
pub const SPEED_STEPS: [(f32, u64); 7] = [
    (0.5, 180),
    (1.0, 90),
    (2.0, 45),
    (3.0, 30),
    (6.0, 15),
    (8.0, 11),
    (12.0, 7),
];

/// A fresh start sits at 3x: quick enough to feel the point of the
/// exercise, slow enough to read on the way past.
pub const SPEED_START: usize = 3;

/// How long X is held before the optional favourite shortcut fires.
pub const FAVORITE_HOLD: Duration = Duration::from_secs(1);

/// Held-key repeat cadence.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct RepeatConfig {
    /// Wait after the first press before repeating.
    pub delay: Duration,
    /// Time between repeats once started.
    pub interval: Duration,
}

impl Default for RepeatConfig {
    fn default() -> Self {
        RepeatConfig {
            // Short, because Degauss is about the held scroll, not about
            // discrete taps.
            delay: Duration::from_millis(220),
            interval: Duration::from_millis(SPEED_STEPS[SPEED_START].1),
        }
    }
}

/// Tracks one held action and decides when it should fire again.
#[derive(Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Held {
    action: Action,
    pressed_at: Instant,
    last_fired: Instant,
    repeating: bool,
}

/// Turns key up/down transitions into a stream of actions, including
/// repeats while a key stays down.
#[derive(Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct Repeater {
    config: RepeatConfig,
    held: Vec<Held>,
    /// Whether holding left or right repeats. In the Direction setting
    /// they move the cursor, and a held stick should scroll the way a held
    /// up or down does. In every other setting they step a ladder or a
    /// choice, where one press must mean one step.
    horizontal_repeats: bool,
    /// Whether X is being treated as a short/long gesture. This is enabled
    /// only while the optional favourite shortcut can act on the selected
    /// row, leaving X immediate everywhere else.
    favorite_hold: bool,
    random_hold: bool,
}

impl Repeater {
    pub fn new(config: RepeatConfig) -> Self {
        Repeater {
            config,
            held: Vec::new(),
            horizontal_repeats: false,
            favorite_hold: false,
            random_hold: false,
        }
    }

    /// Delay X until release so a one-second hold can be distinguished from
    /// the ordinary contextual-menu press. Disabling the gesture also drops
    /// an X still held, so a shortcut that opened another screen cannot fire
    /// the short action when the button is released there.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_favorite_hold(&mut self, enabled: bool) {
        if self.favorite_hold == enabled {
            return;
        }
        self.favorite_hold = enabled;
        if !enabled {
            self.held.retain(|held| held.action != Action::Context);
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_random_hold(&mut self, enabled: bool) {
        if self.random_hold == enabled {
            return;
        }
        self.random_hold = enabled;
        if !enabled {
            self.held.retain(|held| held.action != Action::Menu);
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn hold_action(&self, action: Action) -> Option<Action> {
        match action {
            Action::Context if self.favorite_hold => Some(Action::FavoriteShortcut),
            Action::Menu if self.random_hold => Some(Action::RandomShortcut),
            _ => None,
        }
    }

    /// Turn held-key repeat for left and right on or off. Turning it off
    /// also drops either of them if it is held right now, so a key pressed
    /// while browsing cannot keep firing into a screen opened under it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_horizontal_repeats(&mut self, enabled: bool) {
        if self.horizontal_repeats == enabled {
            return;
        }
        self.horizontal_repeats = enabled;
        if !enabled {
            self.held.retain(|held| {
                held.action.repeats()
                    || (self.favorite_hold && held.action == Action::Context)
                    || (self.random_hold && held.action == Action::Menu)
            });
        }
    }

    /// Change the repeat interval while a key may already be held, so a
    /// speed change takes effect mid-scroll rather than at the next press.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_interval(&mut self, interval: Duration) {
        self.config.interval = interval;
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn interval(&self) -> Duration {
        self.config.interval
    }

    /// A key went down. Returns the action to perform immediately.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn press(&mut self, action: Action, now: Instant) -> Option<Action> {
        if self.random_hold && action != Action::Menu {
            self.held.retain(|held| held.action != Action::Menu);
        }
        if self.favorite_hold && action != Action::Context {
            // The shortcut acts on the selected row. Moving or pressing
            // anything else while X is down cancels it, so the eventual
            // hold cannot add or remove a different row.
            self.held.retain(|held| held.action != Action::Context);
        }
        if self.hold_action(action).is_some() && self.held.iter().any(|held| held.action != action)
        {
            // A direction already held can change the shortcut's scope.
            // Keep the ordinary menu action immediate in that case.
            return Some(action);
        }
        if self.held.iter().any(|h| h.action == action) {
            return None;
        }
        let retained = action.repeats()
            || (self.horizontal_repeats && matches!(action, Action::Slower | Action::Faster))
            || self.hold_action(action).is_some();
        if retained {
            self.held.push(Held {
                action,
                pressed_at: now,
                last_fired: now,
                repeating: false,
            });
        }
        if self.hold_action(action).is_some() {
            None
        } else {
            Some(action)
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn release(&mut self, action: Action, now: Instant) -> Option<Action> {
        let held = self
            .held
            .iter()
            .position(|held| held.action == action)
            .map(|at| self.held.remove(at));
        let held = held?;
        if let Some(shortcut) = self.hold_action(action).filter(|_| !held.repeating) {
            if now.duration_since(held.pressed_at) >= FAVORITE_HOLD {
                Some(shortcut)
            } else {
                Some(action)
            }
        } else {
            None
        }
    }

    /// Actions due because a key is still held.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let mut due = Vec::new();
        for held in &mut self.held {
            let shortcut = match held.action {
                Action::Context if self.favorite_hold => Some(Action::FavoriteShortcut),
                Action::Menu if self.random_hold => Some(Action::RandomShortcut),
                _ => None,
            };
            if let Some(shortcut) = shortcut {
                if !held.repeating && now.duration_since(held.pressed_at) >= FAVORITE_HOLD {
                    held.repeating = true;
                    due.push(shortcut);
                }
                continue;
            }
            let ready = if held.repeating {
                now.duration_since(held.last_fired) >= self.config.interval
            } else {
                now.duration_since(held.pressed_at) >= self.config.delay
            };
            if ready {
                held.repeating = true;
                held.last_fired = now;
                due.push(held.action);
            }
        }
        due
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn anything_held(&self) -> bool {
        !self.held.is_empty()
    }
}

/// Map a Linux key code to an action. Codes are the kernel's own, and the
/// gamepad reaches Degauss through the same codes courtesy of MiSTer. Only the
/// device build reads real key codes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn action_for_key(code: u16) -> Option<Action> {
    // From the kernel's input-event-codes.h.
    const KEY_ESC: u16 = 1;
    const KEY_ENTER: u16 = 28;
    const KEY_SPACE: u16 = 57;
    const KEY_TAB: u16 = 15;
    const KEY_Q: u16 = 16;
    const KEY_P: u16 = 25;
    const KEY_KPENTER: u16 = 96;
    const KEY_UP: u16 = 103;
    const KEY_PAGEUP: u16 = 104;
    const KEY_LEFT: u16 = 105;
    const KEY_RIGHT: u16 = 106;
    const KEY_END: u16 = 107;
    const KEY_DOWN: u16 = 108;
    const KEY_PAGEDOWN: u16 = 109;
    const KEY_HOME: u16 = 102;

    Some(match code {
        KEY_UP => Action::Up,
        KEY_DOWN => Action::Down,
        // Left and right change the scroll speed unless the Left and right
        // setting says otherwise. PageUp and PageDown are their own
        // actions rather than aliases of these, so a keyboard keeps two
        // keys that always page whatever the stick is set to do.
        KEY_LEFT => Action::Slower,
        KEY_RIGHT => Action::Faster,
        KEY_PAGEUP => Action::PageUp,
        KEY_PAGEDOWN => Action::PageDown,
        KEY_HOME => Action::Home,
        KEY_END => Action::End,
        KEY_ENTER | KEY_KPENTER => Action::Accept,
        KEY_ESC | KEY_Q => Action::Quit,
        // MiSTer's own translation layer is the only thing a stick reaches
        // us through, and it sends Space for Y and Tab for X. Y opens the
        // menu, X asks what can be done where you are standing.
        KEY_SPACE => Action::Menu,
        KEY_TAB => Action::Context,
        KEY_P => Action::CyclePresent,
        _ => return None,
    })
}

/// A key transition read from a device.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEdge {
    Down(Action),
    Up(Action),
}

/// The edge a Linux key event value means. 1 is a press and 0 a release;
/// 2 is the kernel's own auto-repeat, ignored because Degauss times its
/// own repeats.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn key_edge(action: Action, value: i32) -> Option<KeyEdge> {
    match value {
        1 => Some(KeyEdge::Down(action)),
        0 => Some(KeyEdge::Up(action)),
        _ => None,
    }
}

/// Merge the edges one device delivered, `edges[split..]`, into those the
/// devices before it delivered, `edges[..split]`, by stamp. Devices are
/// drained one after another, so without this a batch carries one
/// device's later edges before another's earlier ones, and the guard and
/// the repeater take the edges in turn: a press placed before the release
/// it followed reaches the repeater while that action is still held, and
/// the press is lost. Each device's own order is kept whatever its stamps
/// say, so a wall-clock step between a press and its release cannot swap
/// them and leave the key held. Nothing is allocated unless both sides
/// hold edges.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn merge_by_stamp(edges: &mut Vec<(KeyEdge, SystemTime)>, split: usize) {
    if split == 0 || split == edges.len() {
        return;
    }
    let later = edges.split_off(split);
    let earlier = std::mem::replace(edges, Vec::with_capacity(split + later.len()));
    let mut earlier = earlier.into_iter().peekable();
    let mut later = later.into_iter().peekable();
    while let (Some(&(_, before)), Some(&(_, after))) = (earlier.peek(), later.peek()) {
        if after < before {
            edges.extend(later.next());
        } else {
            edges.extend(earlier.next());
        }
    }
    edges.extend(earlier);
    edges.extend(later);
}

/// A second press of the same action closer than this to the accepted one
/// is the same physical press arriving twice, not a deliberate second tap.
pub const DUPLICATE_WINDOW: Duration = Duration::from_millis(40);

/// One slot per [`Action`], indexed by discriminant.
const ACTION_SLOTS: usize = Action::ALL.len();

// `Action::ALL` must hold every variant at its own discriminant, or the
// guard would index past its table on the first press of the missing one.
// The match is exhaustive: a variant added to the enum does not compile
// until it has an arm here, and the assertion refuses a list out of
// declaration order. What the check cannot see is a variant that has an
// arm but no entry in `ALL`, so append the entry with the arm.
const _: () = {
    let mut i = 0;
    while i < ACTION_SLOTS {
        let listed = match Action::ALL[i] {
            Action::Up
            | Action::Down
            | Action::Slower
            | Action::Faster
            | Action::PageUp
            | Action::PageDown
            | Action::Home
            | Action::End
            | Action::Accept
            | Action::Quit
            | Action::CyclePresent
            | Action::Menu
            | Action::Context
            | Action::FavoriteShortcut
            | Action::RandomShortcut => Action::ALL[i] as usize,
        };
        assert!(listed == i, "Action::ALL is not in declaration order");
        i += 1;
    }
};

#[derive(Debug, Clone, Copy)]
struct Slot {
    /// When the device delivered the last accepted press of this action.
    /// It anchors the window: a rejected duplicate never moves it, so a
    /// run of duplicates cannot keep the window open.
    accepted_at: Option<SystemTime>,
    /// A rejected duplicate is still down as far as its device is
    /// concerned, so a release is owed for each one. Those releases must
    /// be swallowed rather than end the genuine hold under them.
    duplicates_down: u8,
}

/// Drops the second delivery of one physical press before it reaches the
/// [`Repeater`]. Only real key edges pass through here; the repeats the
/// [`Repeater`] generates for a held key never do, which is what keeps the
/// 7 ms scroll interval intact.
///
/// The window is measured on the kernel's own timestamps, taken when each
/// device delivered the event, not on the run loop's clock: a frame that
/// stalls on a system read drains every press queued behind it in one
/// poll, and two deliberate taps in that queue must still count as two.
/// One array index, one subtraction and one comparison per edge, nothing
/// allocated.
#[derive(Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct DuplicateGuard {
    slots: [Slot; ACTION_SLOTS],
}

impl DuplicateGuard {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn new() -> Self {
        DuplicateGuard {
            slots: [Slot {
                accepted_at: None,
                duplicates_down: 0,
            }; ACTION_SLOTS],
        }
    }

    /// Filter one key edge read from a device, `at` being the time the
    /// kernel stamped on it. `None` means drop it.
    ///
    /// A press inside [`DUPLICATE_WINDOW`] of the last accepted press of the
    /// same action is a duplicate: rejected, and its eventual release is
    /// swallowed too, one release per rejected press, so a press delivered
    /// three times still ends its hold on the last release. A press exactly
    /// at the window's end is accepted. A different action is never
    /// affected, an opposite direction included. An accepted press clears
    /// any releases still owed, so a duplicate whose release never arrives
    /// cannot swallow a later genuine release.
    ///
    /// The window reaches both ways from the accepted press. A poll merges
    /// what it drained by stamp, but a copy injected on one device just
    /// after that device was read reaches the next poll behind the other
    /// device's copy stamped later; and the stamps come from the wall
    /// clock, which can step. A step larger than the window lets one press
    /// through and the next accepted press re-anchors.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn admit(&mut self, edge: KeyEdge, at: SystemTime) -> Option<KeyEdge> {
        match edge {
            KeyEdge::Down(action) => {
                let slot = &mut self.slots[action as usize];
                if slot.accepted_at.is_some_and(|accepted| {
                    let apart = match at.duration_since(accepted) {
                        Ok(later) => later,
                        Err(earlier) => earlier.duration(),
                    };
                    apart < DUPLICATE_WINDOW
                }) {
                    slot.duplicates_down = slot.duplicates_down.saturating_add(1);
                    return None;
                }
                slot.accepted_at = Some(at);
                slot.duplicates_down = 0;
                Some(edge)
            }
            KeyEdge::Up(action) => {
                let slot = &mut self.slots[action as usize];
                if slot.duplicates_down > 0 {
                    slot.duplicates_down -= 1;
                    return None;
                }
                Some(edge)
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{
    install_signal_handlers, restore_console, restore_terminal, ConsoleGuard, InputReader,
    TerminalGuard,
};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use evdev::{Device, EventSummary};

    use super::{action_for_key, key_edge, merge_by_stamp, KeyEdge};
    use crate::error::{DegaussError, Result};

    /// What was opened, so Degauss can show whether it is actually
    /// listening to anything.
    #[derive(Debug, Clone)]
    pub struct DeviceSummary {
        pub path: PathBuf,
        pub name: String,
        /// True for MiSTer's own virtual device, which is where gamepad
        /// presses arrive once Main has translated them.
        pub is_mister_virtual: bool,
    }

    pub struct InputReader {
        devices: Vec<(PathBuf, Device)>,
        summaries: Vec<DeviceSummary>,
    }

    impl InputReader {
        /// Open every keyboard-capable input device, without grabbing any.
        pub fn open() -> Result<Self> {
            let mut devices = Vec::new();
            let mut summaries = Vec::new();

            for (path, device) in evdev::enumerate() {
                // Only devices that can produce the keys we act on.
                let useful = device
                    .supported_keys()
                    .is_some_and(|keys| keys.iter().any(|k| action_for_key(k.code()).is_some()));
                if !useful {
                    continue;
                }

                // Non-blocking: Degauss polls between frames and must never
                // stall the render loop waiting for a keypress.
                device
                    .set_nonblocking(true)
                    .map_err(|e| DegaussError::io("setting input device non-blocking", &path, e))?;

                let name = device.name().unwrap_or("unnamed").to_string();
                summaries.push(DeviceSummary {
                    path: path.clone(),
                    // MiSTer names its translated-gamepad device this; unlike
                    // Main we deliberately keep it, because it is the only
                    // place controller input appears while a script runs.
                    is_mister_virtual: name.contains("MiSTer virtual input"),
                    name,
                });
                devices.push((path, device));
            }

            Ok(InputReader { devices, summaries })
        }

        pub fn devices(&self) -> &[DeviceSummary] {
            &self.summaries
        }

        pub fn has_mister_virtual(&self) -> bool {
            self.summaries.iter().any(|d| d.is_mister_virtual)
        }

        /// Drain whatever is waiting, each edge with the time the kernel
        /// stamped on its event, in stamp order. Never blocks.
        pub fn poll(&mut self) -> Vec<(KeyEdge, SystemTime)> {
            let mut edges = Vec::new();
            for (_, device) in &mut self.devices {
                let events = match device.fetch_events() {
                    Ok(events) => events,
                    // WouldBlock simply means nothing is waiting.
                    Err(_) => continue,
                };
                let drained = edges.len();
                for event in events {
                    let at = event.timestamp();
                    if let EventSummary::Key(_, code, value) = event.destructure() {
                        let Some(action) = action_for_key(code.code()) else {
                            continue;
                        };
                        if let Some(edge) = key_edge(action, value) {
                            edges.push((edge, at));
                        }
                    }
                }
                merge_by_stamp(&mut edges, drained);
            }
            edges
        }
    }

    /// The terminal settings as they were before Degauss touched them.
    /// Kept globally as well as in the guard because the release build
    /// aborts on panic, which skips destructors: the panic hook restores
    /// the console through this.
    static ORIGINAL: std::sync::Mutex<Option<(i32, libc::termios)>> = std::sync::Mutex::new(None);

    /// Put the console back. Safe to call more than once, and from a panic
    /// hook. Never fails loudly: it runs while something else is already
    /// going wrong.
    pub fn restore_terminal() {
        let Ok(mut slot) = ORIGINAL.lock() else {
            return;
        };
        if let Some((fd, original)) = slot.take() {
            // SAFETY: restoring settings captured from this same descriptor.
            unsafe {
                libc::tcsetattr(fd, libc::TCSANOW, &original);
                libc::tcflush(fd, libc::TCIFLUSH);
            }
        }
    }

    // Console ioctls, from linux/kd.h.
    const KDGETMODE: libc::Ioctl = 0x4B3B;
    const KDSETMODE: libc::Ioctl = 0x4B3A;
    const KD_TEXT: libc::c_int = 0x00;
    const KD_GRAPHICS: libc::c_int = 0x01;

    /// The console mode as found, for the same reason as [`ORIGINAL`].
    static ORIGINAL_KD: std::sync::Mutex<Option<(i32, libc::c_int)>> = std::sync::Mutex::new(None);

    /// Put the console back into text mode. Safe to call repeatedly and from
    /// a panic hook.
    pub fn restore_console() {
        let Ok(mut slot) = ORIGINAL_KD.lock() else {
            return;
        };
        if let Some((fd, mode)) = slot.take() {
            // The cursor comes back with the text mode it belongs to.
            hide_cursor(fd, false);
            // SAFETY: restoring a mode read from this same descriptor.
            unsafe {
                libc::ioctl(fd, KDSETMODE, mode);
                libc::close(fd);
            }
        }
    }

    /// Restore the console on the signals that end a process without
    /// unwinding.
    ///
    /// A destructor does not run for SIGTERM or SIGHUP, and the panic hook
    /// does not either, so without this a terminated Degauss leaves the
    /// terminal raw and the console in graphics mode: a black screen with no
    /// echo, which reads as a broken machine. The handler puts both back and
    /// then dies of the original signal, so the exit status still says what
    /// happened.
    pub fn install_signal_handlers() {
        extern "C" fn on_signal(sig: libc::c_int) {
            restore_terminal();
            restore_console();
            // SAFETY: restoring the default action and re-raising is the
            // documented way to die of the signal that arrived.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                libc::raise(sig);
            }
        }
        for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            // SAFETY: registering a handler for a signal this process owns.
            unsafe {
                libc::signal(
                    sig,
                    on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
                );
            }
        }
    }

    /// Stops the kernel's virtual terminal from drawing on the framebuffer
    /// while Degauss owns it.
    ///
    /// Without this, fbcon keeps painting a blinking block cursor on top of
    /// the rendered frame. Putting the terminal in graphics mode makes it
    /// stand back. On MiSTer's framebuffer driver this does not blank or
    /// clear anything: fbcon skips its blank path once the mode is no longer
    /// text.
    ///
    /// Leaving a console in graphics mode would give the user a black
    /// terminal, so it is restored on drop, on panic and on error.
    pub struct ConsoleGuard {
        active: bool,
    }

    /// True when this process is attached to a Linux virtual console rather
    /// than, say, an SSH session.
    ///
    /// This matters: taking the console into graphics mode blanks whatever
    /// the television is showing. Doing that from an SSH login would black
    /// out the screen of someone who is not even looking at a terminal, and
    /// they would have no way to interact with Degauss because MiSTer
    /// still holds the input devices in that situation.
    fn on_virtual_console() -> bool {
        // SAFETY: ttyname returns a pointer to a static buffer or null.
        let name = unsafe { libc::ttyname(libc::STDIN_FILENO) };
        if name.is_null() {
            return false;
        }
        // SAFETY: ttyname returned a NUL-terminated string.
        let name = unsafe { std::ffi::CStr::from_ptr(name) };
        let Ok(name) = name.to_str() else {
            return false;
        };
        name.strip_prefix("/dev/tty")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    }

    impl ConsoleGuard {
        /// Take the current terminal into graphics mode. A failure here is
        /// reported, not fatal: Degauss still renders, with a cursor
        /// blinking over it.
        pub fn acquire() -> std::result::Result<Self, String> {
            if !on_virtual_console() {
                return Err(
                    "not attached to a virtual console, so the screen was left alone \
                     (run this from the Scripts menu, not over SSH)"
                        .to_string(),
                );
            }

            // /dev/tty0 is whichever terminal is in front, which is the one
            // MiSTer switched to before running the script.
            let path = c"/dev/tty0";
            // SAFETY: opening a device by a NUL-terminated literal path.
            let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR) };
            if fd < 0 {
                return Err(format!(
                    "cannot open /dev/tty0: {}",
                    std::io::Error::last_os_error()
                ));
            }

            let mut previous: libc::c_int = KD_TEXT;
            // SAFETY: fd is open; the ioctl writes one int through this pointer.
            if unsafe { libc::ioctl(fd, KDGETMODE, &mut previous) } != 0 {
                let err = std::io::Error::last_os_error();
                // SAFETY: closing the descriptor we just opened.
                unsafe { libc::close(fd) };
                return Err(format!("cannot read console mode: {err}"));
            }

            // SAFETY: fd is an open terminal descriptor.
            if unsafe { libc::ioctl(fd, KDSETMODE, KD_GRAPHICS) } != 0 {
                let err = std::io::Error::last_os_error();
                // SAFETY: closing the descriptor we just opened.
                unsafe { libc::close(fd) };
                return Err(format!("cannot switch console to graphics mode: {err}"));
            }

            if let Ok(mut slot) = ORIGINAL_KD.lock() {
                *slot = Some((fd, previous));
            }

            // Belt and braces. Graphics mode is meant to stop the terminal
            // drawing, and on some kernels the block cursor keeps blinking
            // through it anyway: a black square in the corner that vanishes
            // whenever the screen is redrawn over it and comes back a moment
            // later. Ask for it to be hidden as well.
            hide_cursor(fd, true);

            Ok(ConsoleGuard { active: true })
        }

        pub fn restore(&mut self) {
            if !self.active {
                return;
            }
            self.active = false;
            restore_console();
        }
    }

    impl Drop for ConsoleGuard {
        fn drop(&mut self) {
            self.restore();
        }
    }

    /// Show or hide the terminal's own cursor.
    ///
    /// Ignores failure on purpose: this is a nicety on top of graphics mode,
    /// and a terminal that will not take the escape is not a reason to stop.
    fn hide_cursor(fd: libc::c_int, hide: bool) {
        let sequence: &[u8] = if hide { b"\x1b[?25l" } else { b"\x1b[?25h" };
        // SAFETY: fd is an open terminal descriptor and the slice is valid
        // for the length given.
        unsafe {
            libc::write(fd, sequence.as_ptr() as *const libc::c_void, sequence.len());
        }
    }

    /// Puts the console into a quiet mode for the duration of Degauss:
    /// no echo, no line buffering, so keystrokes do not print over the UI
    /// and do not queue up for the shell.
    ///
    /// The original settings are restored on drop and on panic, because
    /// leaving a terminal in raw mode would make the machine feel broken.
    /// A signal that terminates the process (SIGTERM, SIGHUP) is not
    /// handled: no destructor runs, so the terminal is left raw.
    pub struct TerminalGuard {
        active: bool,
    }

    impl TerminalGuard {
        pub fn acquire() -> Result<Self> {
            let fd = libc::STDIN_FILENO;
            // SAFETY: zeroed termios is a valid value to fill in via ioctl.
            let mut original: libc::termios = unsafe { std::mem::zeroed() };
            // SAFETY: fd is a valid descriptor, pointer is correctly typed.
            if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
                return Err(DegaussError::io(
                    "reading terminal settings",
                    "/dev/stdin",
                    std::io::Error::last_os_error(),
                ));
            }

            let mut raw = original;
            raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            // SAFETY: raw is a fully initialised termios for this fd.
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
                return Err(DegaussError::io(
                    "setting terminal to quiet mode",
                    "/dev/stdin",
                    std::io::Error::last_os_error(),
                ));
            }

            if let Ok(mut slot) = ORIGINAL.lock() {
                *slot = Some((fd, original));
            }
            Ok(TerminalGuard { active: true })
        }

        /// Put the terminal back exactly as it was found, discarding
        /// anything typed while Degauss was drawing so it is not replayed
        /// into the shell afterwards.
        pub fn restore(&mut self) {
            if !self.active {
                return;
            }
            self.active = false;
            restore_terminal();
        }
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            self.restore();
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(unused_imports)]
pub use elsewhere::{
    install_signal_handlers, restore_console, restore_terminal, ConsoleGuard, InputReader,
    TerminalGuard,
};

/// On a development machine there is no evdev and no console to protect.
/// These stubs exist so the rest of Degauss compiles and can be tested
/// without a MiSTer attached; nothing calls them there, which is the point.
#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
mod elsewhere {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::KeyEdge;
    use crate::error::Result;

    #[derive(Debug, Clone)]
    pub struct DeviceSummary {
        pub path: PathBuf,
        pub name: String,
        pub is_mister_virtual: bool,
    }

    pub struct InputReader;

    impl InputReader {
        pub fn open() -> Result<Self> {
            Ok(InputReader)
        }
        pub fn devices(&self) -> &[DeviceSummary] {
            &[]
        }
        pub fn has_mister_virtual(&self) -> bool {
            false
        }
        pub fn poll(&mut self) -> Vec<(KeyEdge, SystemTime)> {
            Vec::new()
        }
    }

    pub struct TerminalGuard;

    impl TerminalGuard {
        pub fn acquire() -> Result<Self> {
            Ok(TerminalGuard)
        }
        pub fn restore(&mut self) {}
    }

    pub fn restore_terminal() {}

    pub fn install_signal_handlers() {}

    pub struct ConsoleGuard;

    impl ConsoleGuard {
        pub fn acquire() -> std::result::Result<Self, String> {
            Ok(ConsoleGuard)
        }
        pub fn restore(&mut self) {}
    }

    pub fn restore_console() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn y_stays_immediate_when_random_hold_is_off() {
        let mut repeater = Repeater::new(RepeatConfig::default());
        let now = Instant::now();
        assert_eq!(repeater.press(Action::Menu, now), Some(Action::Menu));
        assert!(repeater.tick(now + FAVORITE_HOLD).is_empty());
        assert_eq!(repeater.release(Action::Menu, now + FAVORITE_HOLD), None);
    }

    #[test]
    fn short_y_release_opens_menu_and_a_one_second_hold_picks_once() {
        for released_at in [Duration::from_millis(999), FAVORITE_HOLD] {
            let mut repeater = Repeater::new(RepeatConfig::default());
            repeater.set_random_hold(true);
            let now = Instant::now();
            assert_eq!(repeater.press(Action::Menu, now), None);
            assert_eq!(
                repeater.release(Action::Menu, now + released_at),
                Some(if released_at < FAVORITE_HOLD {
                    Action::Menu
                } else {
                    Action::RandomShortcut
                })
            );
            assert!(repeater.tick(now + Duration::from_secs(3)).is_empty());
        }
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_random_hold(true);
        let now = Instant::now();
        assert_eq!(repeater.press(Action::Menu, now), None);
        assert!(repeater
            .tick(now + FAVORITE_HOLD - Duration::from_millis(1))
            .is_empty());
        assert_eq!(
            repeater.tick(now + FAVORITE_HOLD),
            vec![Action::RandomShortcut]
        );
        assert!(repeater.tick(now + Duration::from_secs(4)).is_empty());
        assert_eq!(
            repeater.release(Action::Menu, now + Duration::from_secs(4)),
            None
        );
    }

    #[test]
    fn disabling_y_hold_cancels_both_short_and_long_actions() {
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_random_hold(true);
        let now = Instant::now();
        repeater.press(Action::Menu, now);
        repeater.set_random_hold(false);
        assert!(repeater.tick(now + FAVORITE_HOLD).is_empty());
        assert_eq!(repeater.release(Action::Menu, now + FAVORITE_HOLD), None);
        assert_eq!(
            repeater.press(Action::Menu, now + FAVORITE_HOLD),
            Some(Action::Menu)
        );
    }

    #[test]
    fn moving_or_pressing_another_button_cancels_y_hold() {
        for action in [
            Action::Up,
            Action::Down,
            Action::Slower,
            Action::Faster,
            Action::Accept,
            Action::Quit,
            Action::Context,
        ] {
            let mut repeater = Repeater::new(RepeatConfig::default());
            repeater.set_random_hold(true);
            let now = Instant::now();
            repeater.press(Action::Menu, now);
            repeater.press(action, now + Duration::from_millis(10));
            assert!(!repeater
                .tick(now + FAVORITE_HOLD)
                .contains(&Action::RandomShortcut));
            assert_eq!(repeater.release(Action::Menu, now + FAVORITE_HOLD), None);
        }
    }

    #[test]
    fn y_pressed_during_a_held_direction_opens_menu_without_arming_random() {
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_random_hold(true);
        let now = Instant::now();
        repeater.press(Action::Down, now);
        assert_eq!(repeater.press(Action::Menu, now), Some(Action::Menu));
        assert!(!repeater
            .tick(now + FAVORITE_HOLD)
            .contains(&Action::RandomShortcut));
        assert_eq!(repeater.release(Action::Menu, now + FAVORITE_HOLD), None);
    }

    #[test]
    fn x_and_y_holds_cancel_each_other_without_losing_the_enabled_gesture() {
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        repeater.set_random_hold(true);
        let now = Instant::now();
        repeater.press(Action::Context, now);
        repeater.press(Action::Menu, now + Duration::from_millis(10));
        repeater.set_horizontal_repeats(true);
        repeater.set_horizontal_repeats(false);
        assert_eq!(
            repeater.tick(now + Duration::from_millis(1010)),
            vec![Action::RandomShortcut]
        );
        assert_eq!(
            repeater.release(Action::Context, now + Duration::from_secs(2)),
            None
        );
        assert_eq!(
            repeater.release(Action::Menu, now + Duration::from_secs(2)),
            None
        );
    }

    #[test]
    fn the_keys_mister_sends_for_a_gamepad_map_to_movement_and_actions() {
        // While a script owns the framebuffer, MiSTer converts the d-pad to
        // arrows and the face buttons to Enter/Escape/Space/Tab. If this
        // mapping is wrong the controller silently does nothing.
        assert_eq!(action_for_key(103), Some(Action::Up));
        assert_eq!(action_for_key(108), Some(Action::Down));
        assert_eq!(action_for_key(105), Some(Action::Slower));
        assert_eq!(action_for_key(106), Some(Action::Faster));
        assert_eq!(
            action_for_key(104),
            Some(Action::PageUp),
            "PageUp pages on its own, not as an alias of left"
        );
        assert_eq!(
            action_for_key(109),
            Some(Action::PageDown),
            "PageDown pages on its own, not as an alias of right"
        );
        assert_eq!(action_for_key(28), Some(Action::Accept));
        assert_eq!(action_for_key(1), Some(Action::Quit));
        assert_eq!(
            action_for_key(57),
            Some(Action::Menu),
            "Space is Y: the menu"
        );
        assert_eq!(
            action_for_key(15),
            Some(Action::Context),
            "Tab is X: what can be done with this folder"
        );
        assert_eq!(
            action_for_key(47),
            None,
            "V is intentionally unmapped: view scope is chosen explicitly"
        );
        assert_eq!(action_for_key(200), None, "unmapped keys must be ignored");
    }

    #[test]
    fn a_press_fires_once_then_repeats_only_after_the_delay() {
        let config = RepeatConfig {
            delay: Duration::from_millis(300),
            interval: Duration::from_millis(90),
        };
        let mut repeater = Repeater::new(config);
        let t0 = Instant::now();

        assert_eq!(repeater.press(Action::Down, t0), Some(Action::Down));
        assert!(
            repeater.tick(t0 + Duration::from_millis(299)).is_empty(),
            "repeat must not start before the delay"
        );
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(300)),
            vec![Action::Down]
        );
    }

    #[test]
    fn changing_speed_takes_effect_on_a_key_that_is_already_held() {
        // Dialling the speed up mid-scroll must change the scroll, not wait
        // for the next press.
        let mut repeater = Repeater::new(RepeatConfig {
            delay: Duration::from_millis(10),
            interval: Duration::from_millis(100),
        });
        let t0 = Instant::now();
        repeater.press(Action::Down, t0);
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(10)),
            vec![Action::Down]
        );

        repeater.set_interval(Duration::from_millis(20));
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(30)),
            vec![Action::Down],
            "the new, shorter interval applies to the key still held"
        );
    }

    #[test]
    fn a_speed_change_does_not_repeat_while_held() {
        // Otherwise one press of "faster" would run the whole ladder.
        let mut repeater = Repeater::new(RepeatConfig {
            delay: Duration::from_millis(1),
            interval: Duration::from_millis(1),
        });
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Faster, t0), Some(Action::Faster));
        assert!(repeater.tick(t0 + Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn left_and_right_repeat_while_held_only_when_they_move_the_cursor() {
        // In the Direction setting a held left or right is a scroll, and
        // must repeat the way a held up or down does.
        let mut repeater = Repeater::new(RepeatConfig {
            delay: Duration::from_millis(10),
            interval: Duration::from_millis(10),
        });
        let t0 = Instant::now();
        repeater.set_horizontal_repeats(true);
        assert_eq!(repeater.press(Action::Faster, t0), Some(Action::Faster));
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(10)),
            vec![Action::Faster],
            "a held right must keep scrolling"
        );
        assert_eq!(
            repeater.release(Action::Faster, t0 + Duration::from_millis(10)),
            None
        );
        assert!(repeater.tick(t0 + Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn turning_horizontal_repeat_off_drops_a_left_or_right_still_held() {
        // Leaving the browse screen with the stick held must not keep
        // firing speed changes into whatever screen opened under it.
        let mut repeater = Repeater::new(RepeatConfig {
            delay: Duration::from_millis(10),
            interval: Duration::from_millis(10),
        });
        let t0 = Instant::now();
        repeater.set_horizontal_repeats(true);
        repeater.press(Action::Slower, t0);
        repeater.press(Action::Down, t0);
        repeater.set_horizontal_repeats(false);
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(10)),
            vec![Action::Down],
            "only the key that always repeats may keep firing"
        );
    }

    #[test]
    fn a_held_key_repeats_at_the_configured_interval() {
        let config = RepeatConfig {
            delay: Duration::from_millis(100),
            interval: Duration::from_millis(50),
        };
        let mut repeater = Repeater::new(config);
        let t0 = Instant::now();
        repeater.press(Action::Down, t0);

        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(100)),
            vec![Action::Down]
        );
        assert!(repeater.tick(t0 + Duration::from_millis(140)).is_empty());
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(150)),
            vec![Action::Down]
        );
        assert_eq!(
            repeater.tick(t0 + Duration::from_millis(205)),
            vec![Action::Down]
        );
    }

    #[test]
    fn releasing_stops_the_repeat() {
        let mut repeater = Repeater::new(RepeatConfig::default());
        let t0 = Instant::now();
        repeater.press(Action::Down, t0);
        assert_eq!(
            repeater.release(Action::Down, t0 + Duration::from_millis(1)),
            None
        );
        assert!(repeater.tick(t0 + Duration::from_secs(5)).is_empty());
        assert!(!repeater.anything_held());
    }

    #[test]
    fn x_stays_immediate_when_the_favourite_shortcut_is_off() {
        // Off is the upgrade default. A user who never enables the setting
        // must keep the exact press-time contextual-menu behaviour.
        let mut repeater = Repeater::new(RepeatConfig::default());
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Context, t0), Some(Action::Context));
        assert!(repeater.tick(t0 + FAVORITE_HOLD).is_empty());
        assert_eq!(repeater.release(Action::Context, t0 + FAVORITE_HOLD), None);
    }

    #[test]
    fn a_short_x_release_keeps_the_contextual_menu() {
        // Enabling the shortcut adds a long gesture; it must not take away
        // the ordinary X action used throughout Degauss.
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Context, t0), None);
        assert!(repeater
            .tick(t0 + FAVORITE_HOLD - Duration::from_millis(1))
            .is_empty());
        assert_eq!(
            repeater.release(
                Action::Context,
                t0 + FAVORITE_HOLD - Duration::from_millis(1)
            ),
            Some(Action::Context)
        );
    }

    #[test]
    fn x_held_for_one_second_fires_the_favourite_shortcut_once() {
        // A destructive removal must not repeat while X remains down.
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Context, t0), None);
        assert_eq!(
            repeater.tick(t0 + FAVORITE_HOLD),
            vec![Action::FavoriteShortcut]
        );
        assert!(repeater
            .tick(t0 + FAVORITE_HOLD + Duration::from_secs(2))
            .is_empty());
        assert_eq!(
            repeater.release(Action::Context, t0 + FAVORITE_HOLD + Duration::from_secs(2)),
            None,
            "release after a long hold must not open the context menu"
        );
    }

    #[test]
    fn release_at_the_threshold_cannot_turn_a_long_hold_into_a_short_press() {
        // Input edges are drained before timed actions. If X comes up exactly
        // at one second, release itself has to emit the long action.
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Context, t0), None);
        assert_eq!(
            repeater.release(Action::Context, t0 + FAVORITE_HOLD),
            Some(Action::FavoriteShortcut)
        );
    }

    #[test]
    fn leaving_an_eligible_row_cancels_x_without_leaking_a_context_press() {
        // A long hold can open the folder chooser. Disabling the gesture on
        // that new screen must consume the eventual button release there.
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Context, t0), None);
        assert_eq!(
            repeater.tick(t0 + FAVORITE_HOLD),
            vec![Action::FavoriteShortcut]
        );
        repeater.set_favorite_hold(false);
        assert_eq!(repeater.release(Action::Context, t0 + FAVORITE_HOLD), None);
    }

    #[test]
    fn another_button_cancels_the_held_x_target() {
        // The favourite action belongs to the row selected when X went down.
        // A movement while it is held must not apply it to the new row.
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Context, t0), None);
        assert_eq!(
            repeater.press(Action::Down, t0 + Duration::from_millis(100)),
            Some(Action::Down)
        );
        assert!(
            !repeater
                .tick(t0 + FAVORITE_HOLD)
                .contains(&Action::FavoriteShortcut),
            "movement may repeat, but the cancelled favourite action must not fire"
        );
        assert_eq!(repeater.release(Action::Context, t0 + FAVORITE_HOLD), None);
    }

    #[test]
    fn x_does_not_arm_the_shortcut_while_movement_is_already_held() {
        // The reverse input order has the same target risk: a held direction
        // can move after X goes down. X remains available as the ordinary
        // context action, but no delayed favourite action may be retained.
        let mut repeater = Repeater::new(RepeatConfig::default());
        repeater.set_favorite_hold(true);
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Down, t0), Some(Action::Down));
        assert_eq!(
            repeater.press(Action::Context, t0 + Duration::from_millis(100)),
            Some(Action::Context)
        );
        assert!(
            !repeater
                .tick(t0 + FAVORITE_HOLD + Duration::from_millis(100))
                .contains(&Action::FavoriteShortcut),
            "X pressed during movement must not retain a changing target"
        );
        assert_eq!(
            repeater.release(
                Action::Context,
                t0 + FAVORITE_HOLD + Duration::from_millis(100)
            ),
            None
        );
    }

    #[test]
    fn launching_never_repeats_while_the_button_is_held() {
        // A repeating Accept would fire a second launch into a core that is
        // already loading.
        let mut repeater = Repeater::new(RepeatConfig {
            delay: Duration::from_millis(1),
            interval: Duration::from_millis(1),
        });
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Accept, t0), Some(Action::Accept));
        assert!(
            repeater.tick(t0 + Duration::from_secs(1)).is_empty(),
            "Accept must fire once per press"
        );
    }

    #[test]
    fn pressing_a_key_that_is_already_down_does_not_double_fire() {
        let mut repeater = Repeater::new(RepeatConfig::default());
        let t0 = Instant::now();
        assert_eq!(repeater.press(Action::Up, t0), Some(Action::Up));
        assert_eq!(
            repeater.press(Action::Up, t0 + Duration::from_millis(10)),
            None,
            "a duplicate press (autorepeat leaking through) must be ignored"
        );
    }

    #[test]
    fn two_directions_held_at_once_both_repeat() {
        let mut repeater = Repeater::new(RepeatConfig {
            delay: Duration::from_millis(10),
            interval: Duration::from_millis(10),
        });
        let t0 = Instant::now();
        repeater.press(Action::Down, t0);
        repeater.press(Action::Up, t0);
        let due = repeater.tick(t0 + Duration::from_millis(10));
        assert_eq!(due.len(), 2);
        assert!(due.contains(&Action::Down) && due.contains(&Action::Up));
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The dispatch shape of the run loop: every key edge read from a
    /// device passes the guard before the repeater, only what the repeater
    /// returns is dispatched, and the repeater's own ticks never see the
    /// guard. The guard judges each edge on the kernel's stamp for it; the
    /// repeater sees the loop's one instant per iteration. Built without
    /// the guard to show what the repeater alone does with the same edges.
    struct Pipeline {
        guard: Option<DuplicateGuard>,
        repeater: Repeater,
        t0: Instant,
    }

    impl Pipeline {
        fn guarded(repeater: Repeater) -> Self {
            Pipeline {
                guard: Some(DuplicateGuard::new()),
                repeater,
                t0: Instant::now(),
            }
        }

        fn unguarded(repeater: Repeater) -> Self {
            Pipeline {
                guard: None,
                repeater,
                t0: Instant::now(),
            }
        }

        /// One loop iteration at `now` dispatching the edges one poll
        /// drained from one device, each stamped `at` milliseconds by the
        /// kernel.
        fn batch(&mut self, now: u64, edges: &[(KeyEdge, u64)]) -> Vec<Action> {
            self.devices(now, &[edges])
        }

        /// One loop iteration at `now` dispatching the edges one poll
        /// drained from several devices, given in the order the devices
        /// were drained, merged as the poll merges them.
        fn devices(&mut self, now: u64, drained: &[&[(KeyEdge, u64)]]) -> Vec<Action> {
            let now = self.t0 + ms(now);
            let mut edges: Vec<(KeyEdge, SystemTime)> = Vec::new();
            for device in drained {
                let split = edges.len();
                edges.extend(
                    device
                        .iter()
                        .map(|&(edge, at)| (edge, SystemTime::UNIX_EPOCH + ms(at))),
                );
                merge_by_stamp(&mut edges, split);
            }
            let mut dispatched = Vec::new();
            for (edge, at) in edges {
                let edge = match &mut self.guard {
                    Some(guard) => match guard.admit(edge, at) {
                        Some(edge) => edge,
                        None => continue,
                    },
                    None => edge,
                };
                let action = match edge {
                    KeyEdge::Down(action) => self.repeater.press(action, now),
                    KeyEdge::Up(action) => self.repeater.release(action, now),
                };
                dispatched.extend(action);
            }
            dispatched
        }

        /// One edge in an iteration of its own, dispatched as soon as it
        /// was delivered.
        fn edge(&mut self, edge: KeyEdge, at: u64) -> Option<Action> {
            self.batch(at, &[(edge, at)]).pop()
        }

        /// Edges each dispatched as soon as delivered: the loop keeping up.
        fn feed(&mut self, edges: &[(KeyEdge, u64)]) -> Vec<Action> {
            edges
                .iter()
                .filter_map(|&(edge, at)| self.edge(edge, at))
                .collect()
        }

        fn tick(&mut self, at: u64) -> Vec<Action> {
            self.repeater.tick(self.t0 + ms(at))
        }
    }

    /// One tap delivered twice: the sequence the affected controllers send
    /// for a single press of `action`.
    fn double_delivery(action: Action) -> [(KeyEdge, u64); 4] {
        [
            (KeyEdge::Down(action), 0),
            (KeyEdge::Up(action), 5),
            (KeyEdge::Down(action), 10),
            (KeyEdge::Up(action), 15),
        ]
    }

    #[test]
    fn a_double_delivery_of_one_press_moves_once() {
        // The repeater alone cannot catch this: the release in between
        // clears its held state, so the second press fires again and one
        // tap moves two rows.
        let mut unguarded = Pipeline::unguarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            unguarded.feed(&double_delivery(Action::Down)),
            vec![Action::Down, Action::Down],
            "the repeater's own duplicate check does not cover a press, release, press"
        );

        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&double_delivery(Action::Down)),
            vec![Action::Down]
        );
        assert!(!guarded.repeater.anything_held());
    }

    #[test]
    fn overlapping_double_delivery_moves_once_and_leaves_nothing_held() {
        // Both presses before either release. The second release belongs to
        // the rejected press; if it reached the repeater it would find
        // nothing held, and if the first one were swallowed instead the
        // direction would stay down and scroll for ever.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Down(Action::Down), 5),
                (KeyEdge::Up(Action::Down), 10),
                (KeyEdge::Up(Action::Down), 15),
            ]),
            vec![Action::Down]
        );
        assert!(!guarded.repeater.anything_held());
        assert!(guarded.tick(1000).is_empty(), "nothing may keep scrolling");
        assert_eq!(
            guarded.edge(KeyEdge::Down(Action::Down), 100),
            Some(Action::Down),
            "the next deliberate press must work"
        );
    }

    #[test]
    fn deliberate_taps_outside_the_window_both_count() {
        // Two quick taps are two moves. The window is exactly 40 ms: a
        // press at 40 ms is a second tap, a press at 39 ms is the same tap.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Up(Action::Down), 10),
                (KeyEdge::Down(Action::Down), 40),
                (KeyEdge::Up(Action::Down), 50),
            ]),
            vec![Action::Down, Action::Down]
        );

        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Up(Action::Down), 10),
                (KeyEdge::Down(Action::Down), 39),
                (KeyEdge::Up(Action::Down), 50),
            ]),
            vec![Action::Down]
        );
    }

    #[test]
    fn taps_queued_behind_a_stalled_frame_are_judged_on_their_own_stamps() {
        // Opening a large system parses its list inside the loop, so the
        // next poll drains everything pressed meanwhile at once. Two taps
        // 100 ms apart in that queue are two moves; only their kernel
        // stamps can tell them from a double delivery, which shares the
        // frame with them just the same.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.batch(
                2000,
                &[
                    (KeyEdge::Down(Action::Down), 0),
                    (KeyEdge::Up(Action::Down), 10),
                    (KeyEdge::Down(Action::Down), 100),
                    (KeyEdge::Up(Action::Down), 110),
                ]
            ),
            vec![Action::Down, Action::Down],
            "two deliberate taps dispatched by one frame must both count"
        );
        assert!(!guarded.repeater.anything_held());

        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.batch(2000, &double_delivery(Action::Down)),
            vec![Action::Down],
            "a double delivery dispatched by one frame is still one press"
        );
        assert!(!guarded.repeater.anything_held());

        // The other way round: a double delivery split across two frames
        // by a stall between them is still one press.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.batch(
                0,
                &[
                    (KeyEdge::Down(Action::Down), 0),
                    (KeyEdge::Up(Action::Down), 5)
                ]
            ),
            vec![Action::Down]
        );
        assert_eq!(
            guarded.batch(
                500,
                &[
                    (KeyEdge::Down(Action::Down), 10),
                    (KeyEdge::Up(Action::Down), 15)
                ]
            ),
            vec![],
            "the frame clock must not decide what the stamps already have"
        );
        assert!(!guarded.repeater.anything_held());
    }

    #[test]
    fn a_second_device_stamped_earlier_is_still_the_same_press() {
        // Devices are drained one after another. A copy of a press
        // injected on one device just after that device was read waits
        // for the next poll, behind the other device's copy stamped a
        // little later, so the copy already accepted can carry the later
        // stamp. The window has to reach both ways or the earlier copy
        // would pass and move the selection again.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.batch(
                20,
                &[
                    (KeyEdge::Down(Action::Down), 12),
                    (KeyEdge::Up(Action::Down), 18),
                ]
            ),
            vec![Action::Down]
        );
        assert_eq!(
            guarded.batch(
                36,
                &[
                    (KeyEdge::Down(Action::Down), 10),
                    (KeyEdge::Up(Action::Down), 16),
                ]
            ),
            vec![]
        );
        assert!(!guarded.repeater.anything_held());
        assert_eq!(
            guarded.edge(KeyEdge::Down(Action::Down), 52),
            Some(Action::Down),
            "the window stays anchored to the accepted stamp, 12 ms"
        );

        // A stamp a whole window or more earlier is not the same press: a
        // wall clock stepped back that far lets one press through and the
        // next accepted press re-anchors the window.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 1000),
                (KeyEdge::Up(Action::Down), 1005),
                (KeyEdge::Down(Action::Down), 960),
                (KeyEdge::Up(Action::Down), 965),
                (KeyEdge::Down(Action::Down), 970),
                (KeyEdge::Up(Action::Down), 975),
            ]),
            vec![Action::Down, Action::Down],
            "the press at 970 ms is inside the window of the one at 960 ms"
        );
    }

    #[test]
    fn a_poll_batch_is_dispatched_in_stamp_order_across_devices() {
        // Two devices report one press; the second copy is rejected and
        // its release is owed. A frame then stalls across the genuine
        // release and the next deliberate tap. The poll drains the first
        // device, which holds that release and the new press, before the
        // second, which holds the duplicate's release. In drain order the
        // owed release swallows the genuine one, the new press reaches the
        // repeater while it still holds the action and dispatches nothing,
        // and the duplicate's release then ends the hold: a deliberate tap
        // lost. In stamp order the two releases come first and the tap
        // acts.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Down(Action::Down), 2),
            ]),
            vec![Action::Down]
        );
        assert_eq!(
            guarded.devices(
                400,
                &[
                    &[
                        (KeyEdge::Up(Action::Down), 300),
                        (KeyEdge::Down(Action::Down), 350),
                    ],
                    &[(KeyEdge::Up(Action::Down), 302)],
                ]
            ),
            vec![Action::Down],
            "the tap at 350 ms must act whichever device was drained first"
        );
        assert!(
            guarded.repeater.anything_held(),
            "the tap at 350 ms is still held when the frame ends"
        );
        assert_eq!(guarded.edge(KeyEdge::Up(Action::Down), 500), None);
        assert!(!guarded.repeater.anything_held());

        // A device's own order is kept whatever its stamps say: the wall
        // clock stepped back between a press and its release queued in
        // one frame. Sorted on the stamps alone, the release would come
        // first and the press would leave the key held.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.devices(
                1000,
                &[
                    &[
                        (KeyEdge::Down(Action::Up), 1000),
                        (KeyEdge::Up(Action::Up), 5)
                    ],
                    &[
                        (KeyEdge::Down(Action::Down), 8),
                        (KeyEdge::Up(Action::Down), 12)
                    ],
                ]
            ),
            vec![Action::Down, Action::Up],
            "the other device's edges are merged in by stamp"
        );
        assert!(
            !guarded.repeater.anything_held(),
            "the release still follows its press"
        );
    }

    #[test]
    fn a_different_action_inside_the_window_is_immediate() {
        // The guard is per action. A reversal of direction, a speed change
        // either way, or launching then backing out must never wait.
        for (first, second) in [
            (Action::Down, Action::Up),
            (Action::Slower, Action::Faster),
            (Action::Accept, Action::Quit),
        ] {
            let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
            assert_eq!(
                guarded.feed(&[(KeyEdge::Down(first), 0), (KeyEdge::Down(second), 2)]),
                vec![first, second],
                "{first:?} then {second:?} must both act, in order"
            );
            assert_eq!(
                guarded.feed(&[(KeyEdge::Up(first), 4), (KeyEdge::Up(second), 6)]),
                vec![]
            );
            assert!(!guarded.repeater.anything_held());
        }
    }

    #[test]
    fn a_rejected_duplicate_does_not_move_the_window() {
        // The window is anchored to the accepted press. If a rejected
        // duplicate restarted it, a controller that keeps repeating a press
        // could hold the window open and swallow a deliberate second tap.
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Up(Action::Down), 5),
                (KeyEdge::Down(Action::Down), 30),
                (KeyEdge::Up(Action::Down), 32),
            ]),
            vec![Action::Down]
        );
        assert_eq!(
            guarded.edge(KeyEdge::Down(Action::Down), 45),
            Some(Action::Down),
            "45 ms after the accepted press is outside the window even though \
             a duplicate arrived at 30 ms"
        );
    }

    #[test]
    fn a_second_device_delivering_the_same_press_does_not_end_the_hold() {
        // Two devices report the same physical press: one press each, a
        // few milliseconds apart. Only one may act, and the release of the
        // rejected one must not end the scroll the accepted one started.
        let config = RepeatConfig {
            delay: ms(10),
            interval: ms(10),
        };

        // The duplicate's release comes first, while the press is held.
        let mut unguarded = Pipeline::unguarded(Repeater::new(config));
        assert_eq!(
            unguarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Down(Action::Down), 2),
                (KeyEdge::Up(Action::Down), 5),
            ]),
            vec![Action::Down]
        );
        assert!(
            unguarded.tick(10).is_empty(),
            "the repeater alone lets the duplicate's release end the hold"
        );

        let mut guarded = Pipeline::guarded(Repeater::new(config));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Down(Action::Down), 2),
                (KeyEdge::Up(Action::Down), 5),
            ]),
            vec![Action::Down]
        );
        assert_eq!(guarded.tick(10), vec![Action::Down], "the hold goes on");
        assert_eq!(guarded.tick(20), vec![Action::Down]);
        assert_eq!(guarded.edge(KeyEdge::Up(Action::Down), 500), None);
        assert!(!guarded.repeater.anything_held());
        assert!(guarded.tick(1000).is_empty());

        // Both releases come after the repeats started. The guard cannot
        // tell which device sent which, so the first release is taken as
        // the duplicate's and the hold ends on the second; nothing may stay
        // held after both.
        let mut guarded = Pipeline::guarded(Repeater::new(config));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Down(Action::Down), 2),
            ]),
            vec![Action::Down]
        );
        assert_eq!(guarded.tick(10), vec![Action::Down]);
        assert_eq!(guarded.tick(20), vec![Action::Down]);
        assert_eq!(guarded.edge(KeyEdge::Up(Action::Down), 30), None);
        assert_eq!(
            guarded.tick(30),
            vec![Action::Down],
            "one release of two does not end the hold"
        );
        assert_eq!(guarded.edge(KeyEdge::Up(Action::Down), 32), None);
        assert!(!guarded.repeater.anything_held());
        assert!(guarded.tick(1000).is_empty(), "nothing may stay held");
    }

    #[test]
    fn a_press_delivered_three_times_ends_its_hold_on_the_last_release() {
        // Three copies of one press owe three releases. The guard counts
        // the rejected copies, so only their releases are swallowed and the
        // hold ends on the genuine one; a single flag would let the second
        // release through and stop the scroll while the key is still down.
        let config = RepeatConfig {
            delay: ms(10),
            interval: ms(10),
        };
        let mut guarded = Pipeline::guarded(Repeater::new(config));
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 0),
                (KeyEdge::Down(Action::Down), 2),
                (KeyEdge::Down(Action::Down), 4),
                (KeyEdge::Up(Action::Down), 5),
                (KeyEdge::Up(Action::Down), 6),
            ]),
            vec![Action::Down]
        );
        assert_eq!(
            guarded.tick(10),
            vec![Action::Down],
            "two releases of three do not end the hold"
        );
        assert_eq!(guarded.tick(20), vec![Action::Down]);
        assert_eq!(guarded.edge(KeyEdge::Up(Action::Down), 500), None);
        assert!(!guarded.repeater.anything_held());
        assert!(guarded.tick(1000).is_empty(), "nothing may stay held");

        // Copies whose releases never come leave releases owed while the
        // key stays down. The next accepted press clears them, so that
        // press's own release still ends the hold instead of being
        // swallowed as the old debt.
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 1000),
                (KeyEdge::Down(Action::Down), 1002),
                (KeyEdge::Down(Action::Down), 1004),
                (KeyEdge::Up(Action::Down), 1005),
            ]),
            vec![Action::Down]
        );
        assert!(guarded.repeater.anything_held(), "one release of three");
        assert_eq!(
            guarded.feed(&[
                (KeyEdge::Down(Action::Down), 1100),
                (KeyEdge::Up(Action::Down), 1105),
            ]),
            vec![]
        );
        assert!(
            !guarded.repeater.anything_held(),
            "the owed releases were cleared"
        );
    }

    #[test]
    fn a_hold_in_every_direction_at_every_speed_starts_and_ends_through_the_guard() {
        // The repeater's cadence is its own (`a_held_key_repeats_at_the_
        // configured_interval`) and its ticks never see the guard, so what
        // the guard can break about a hold is its two ends: the press must
        // be admitted or nothing scrolls, and the release must be admitted
        // or the scroll never stops. Every direction, at every speed
        // including the 7 ms interval, in the settings where left and
        // right repeat.
        for (_, interval) in SPEED_STEPS {
            let config = RepeatConfig {
                delay: ms(220),
                interval: ms(interval),
            };
            for action in [Action::Up, Action::Down, Action::Slower, Action::Faster] {
                let mut guarded = Pipeline::guarded(Repeater::new(config));
                guarded.repeater.set_horizontal_repeats(true);
                assert_eq!(
                    guarded.edge(KeyEdge::Down(action), 0),
                    Some(action),
                    "{action:?} at {interval} ms: the press starts the hold"
                );
                let mut fired = 0;
                for at in 0..=500 {
                    fired += guarded.tick(at).len();
                }
                let expected = 1 + (500 - 220) / interval;
                assert_eq!(
                    fired, expected as usize,
                    "{action:?} at {interval} ms: the delay and the interval are the repeater's"
                );
                assert_eq!(guarded.edge(KeyEdge::Up(action), 500), None);
                assert!(
                    !guarded.repeater.anything_held(),
                    "{action:?} at {interval} ms: the release ends the hold"
                );
                assert!(guarded.tick(1000).is_empty());
                assert_eq!(
                    guarded.edge(KeyEdge::Down(action), 505),
                    Some(action),
                    "{action:?}: a press right after the release is a new press, \
                     the window ran from the first press, not from the release"
                );
            }
        }
    }

    #[test]
    fn directions_are_guarded_in_every_left_right_setting() {
        // Left and right are retained by the repeater in every Left and
        // Right Behaviour setting other than Scroll Speed Change (Letter,
        // Page and Direction); up and down always are; page keys never are.
        // The guard sits before all of that, so a double delivery is one
        // move in every case and two taps are two.
        for horizontal in [true, false] {
            for action in [
                Action::Up,
                Action::Down,
                Action::Slower,
                Action::Faster,
                Action::PageUp,
                Action::PageDown,
            ] {
                let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
                guarded.repeater.set_horizontal_repeats(horizontal);
                assert_eq!(
                    guarded.feed(&double_delivery(action)),
                    vec![action],
                    "{action:?} with horizontal repeats {horizontal}"
                );
                assert_eq!(
                    guarded.feed(&[
                        (KeyEdge::Down(action), 100),
                        (KeyEdge::Up(action), 105),
                        (KeyEdge::Down(action), 200),
                        (KeyEdge::Up(action), 205),
                    ]),
                    vec![action, action],
                    "{action:?} with horizontal repeats {horizontal}: two taps"
                );
                assert!(!guarded.repeater.anything_held());
            }
        }
    }

    #[test]
    fn face_buttons_act_once_per_physical_press_with_and_without_hold_shortcuts() {
        // The repeater does not retain Accept, Back, Menu or Context, so
        // without the guard a double delivery launches twice or backs out
        // two levels. With a hold shortcut enabled the repeater retains the
        // button, so a second press while it is held is already ignored;
        // what the guard adds is that the duplicate's release cannot end
        // the hold as a short press before the shortcut fires, and no
        // release afterwards may leak into the screen the shortcut opened.
        for action in [Action::Accept, Action::Quit, Action::Menu, Action::Context] {
            let mut unguarded = Pipeline::unguarded(Repeater::new(RepeatConfig::default()));
            assert_eq!(
                unguarded.feed(&double_delivery(action)),
                vec![action, action],
                "{action:?} is not retained, so the repeater alone acts twice"
            );
            let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
            assert_eq!(guarded.feed(&double_delivery(action)), vec![action]);
            assert!(!guarded.repeater.anything_held());
        }

        for (button, enable, shortcut) in [
            (
                Action::Context,
                Repeater::set_favorite_hold as fn(&mut Repeater, bool),
                Action::FavoriteShortcut,
            ),
            (
                Action::Menu,
                Repeater::set_random_hold,
                Action::RandomShortcut,
            ),
        ] {
            // Two devices report the press, and the duplicate's release
            // arrives while the button is still held.
            let mut unguarded = Pipeline::unguarded(Repeater::new(RepeatConfig::default()));
            enable(&mut unguarded.repeater, true);
            assert_eq!(
                unguarded.feed(&[
                    (KeyEdge::Down(button), 0),
                    (KeyEdge::Down(button), 5),
                    (KeyEdge::Up(button), 8),
                ]),
                vec![button],
                "the repeater alone takes the duplicate's release as a short {button:?}"
            );
            assert!(
                unguarded.tick(1000).is_empty(),
                "and the hold it ended never fires the shortcut"
            );

            let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
            enable(&mut guarded.repeater, true);
            assert_eq!(
                guarded.feed(&[
                    (KeyEdge::Down(button), 0),
                    (KeyEdge::Down(button), 5),
                    (KeyEdge::Up(button), 8),
                ]),
                vec![]
            );
            assert_eq!(
                guarded.tick(1000),
                vec![shortcut],
                "{button:?} held for one second fires its shortcut once"
            );
            assert!(guarded.tick(1005).is_empty());
            // The shortcut opened another screen, which disables the gesture.
            enable(&mut guarded.repeater, false);
            assert_eq!(
                guarded.edge(KeyEdge::Up(button), 1010),
                None,
                "the release may not act on the new screen"
            );
            assert!(!guarded.repeater.anything_held());

            let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
            enable(&mut guarded.repeater, true);
            assert_eq!(
                guarded.feed(&double_delivery(button)),
                vec![button],
                "a short {button:?} delivered twice is one short press"
            );
            assert!(!guarded.repeater.anything_held());
        }
    }

    #[test]
    fn the_kernel_auto_repeat_value_stays_ignored() {
        // The kernel repeats a held key on its own schedule. Degauss times
        // its own repeats, so taking those events as presses would double
        // every scroll.
        assert_eq!(key_edge(Action::Down, 1), Some(KeyEdge::Down(Action::Down)));
        assert_eq!(key_edge(Action::Down, 0), Some(KeyEdge::Up(Action::Down)));
        assert_eq!(key_edge(Action::Down, 2), None, "value 2 is auto-repeat");
        assert_eq!(key_edge(Action::Accept, 3), None);
        assert_eq!(key_edge(Action::Accept, -1), None);
    }

    #[test]
    fn ordinary_taps_and_holds_are_unchanged_without_duplicate_delivery() {
        // A keyboard or controller that delivers each press once must feel
        // exactly as before: every tap acts, and a hold starts on the
        // press, repeats what the delay and interval give and ends on the
        // release. The two shortcuts are left out: only the repeater
        // produces them.
        let actions: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|action| !matches!(action, Action::FavoriteShortcut | Action::RandomShortcut))
            .collect();
        let mut guarded = Pipeline::guarded(Repeater::new(RepeatConfig::default()));
        let taps: Vec<(KeyEdge, u64)> = actions
            .iter()
            .enumerate()
            .flat_map(|(i, &action)| {
                let at = i as u64 * 150;
                [(KeyEdge::Down(action), at), (KeyEdge::Up(action), at + 20)]
            })
            .collect();
        assert_eq!(guarded.feed(&taps), actions);
        assert!(!guarded.repeater.anything_held());

        let config = RepeatConfig::default();
        let mut guarded = Pipeline::guarded(Repeater::new(config));
        assert_eq!(
            guarded.edge(KeyEdge::Down(Action::Down), 0),
            Some(Action::Down)
        );
        let mut fired = 0;
        for at in 0..=500 {
            fired += guarded.tick(at).len();
        }
        let expected = 1 + (500 - config.delay.as_millis()) / config.interval.as_millis();
        assert_eq!(
            fired, expected as usize,
            "a 500 ms hold repeats what the delay and the interval give"
        );
        assert_eq!(guarded.edge(KeyEdge::Up(Action::Down), 500), None);
        assert!(!guarded.repeater.anything_held());
        assert!(guarded.tick(1000).is_empty());
    }
}
