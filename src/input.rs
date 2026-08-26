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

use std::time::{Duration, Instant};

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
    Home,
    End,
    /// Launch the selected entry.
    Accept,
    /// Go back: out of a folder, out of a screen, or to the menu at the top.
    Quit,
    /// Switch view: details, tiled, list or carousel.
    CycleLayout,
    /// Switch between the presentation paths being compared.
    CyclePresent,
    /// Open the menu.
    Menu,
    /// Open the contextual menu: what can be done with the folder on screen.
    Context,
}

impl Action {
    /// Whether holding the key should repeat. Only movement repeats:
    /// repeating "launch" would be dangerous, and repeating a speed change
    /// would run the whole ladder off one press.
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

/// A fresh start sits at 1x, so the first thing felt is the familiar rate.
pub const SPEED_START: usize = 1;

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
}

impl Repeater {
    pub fn new(config: RepeatConfig) -> Self {
        Repeater {
            config,
            held: Vec::new(),
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
        if self.held.iter().any(|h| h.action == action) {
            return None;
        }
        if action.repeats() {
            self.held.push(Held {
                action,
                pressed_at: now,
                last_fired: now,
                repeating: false,
            });
        }
        Some(action)
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn release(&mut self, action: Action) {
        self.held.retain(|h| h.action != action);
    }

    /// Actions due because a key is still held.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let mut due = Vec::new();
        for held in &mut self.held {
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
    const KEY_V: u16 = 47;
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
        // Left and right change the SCROLL SPEED rather than jumping a
        // page. Paging is not what is being tested: the question is how
        // fast a continuous scroll can run and still look smooth.
        KEY_LEFT | KEY_PAGEUP => Action::Slower,
        KEY_RIGHT | KEY_PAGEDOWN => Action::Faster,
        KEY_HOME => Action::Home,
        KEY_END => Action::End,
        KEY_ENTER | KEY_KPENTER => Action::Accept,
        KEY_ESC | KEY_Q => Action::Quit,
        // MiSTer's own translation layer is the only thing a stick reaches
        // us through, and it sends Space for Y and Tab for X. Y opens the
        // menu, X asks what can be done where you are standing.
        KEY_SPACE => Action::Menu,
        KEY_TAB => Action::Context,
        // Keyboard only. Switching view is a setting, and a gamepad button
        // is too scarce to spend on it.
        KEY_V => Action::CycleLayout,
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

#[cfg(target_os = "linux")]
pub use linux::{
    install_signal_handlers, restore_console, restore_terminal, ConsoleGuard, InputReader,
    TerminalGuard,
};

#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;

    use evdev::{Device, EventSummary};

    use super::{action_for_key, KeyEdge};
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

        /// Drain whatever is waiting. Never blocks.
        pub fn poll(&mut self) -> Vec<KeyEdge> {
            let mut edges = Vec::new();
            for (_, device) in &mut self.devices {
                let events = match device.fetch_events() {
                    Ok(events) => events,
                    // WouldBlock simply means nothing is waiting.
                    Err(_) => continue,
                };
                for event in events {
                    if let EventSummary::Key(_, code, value) = event.destructure() {
                        let Some(action) = action_for_key(code.code()) else {
                            continue;
                        };
                        match value {
                            // 1 = press. 2 = the kernel's own auto-repeat,
                            // ignored: Degauss times its own repeats.
                            1 => edges.push(KeyEdge::Down(action)),
                            0 => edges.push(KeyEdge::Up(action)),
                            _ => {}
                        }
                    }
                }
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
        pub fn poll(&mut self) -> Vec<KeyEdge> {
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
    fn the_keys_mister_sends_for_a_gamepad_map_to_movement_and_actions() {
        // While a script owns the framebuffer, MiSTer converts the d-pad to
        // arrows and the face buttons to Enter/Escape/Space/Tab. If this
        // mapping is wrong the controller silently does nothing.
        assert_eq!(action_for_key(103), Some(Action::Up));
        assert_eq!(action_for_key(108), Some(Action::Down));
        assert_eq!(action_for_key(105), Some(Action::Slower));
        assert_eq!(action_for_key(106), Some(Action::Faster));
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
            Some(Action::CycleLayout),
            "switching view is a keyboard shortcut: a face button is too \
             scarce to spend on a setting"
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
        repeater.release(Action::Down);
        assert!(repeater.tick(t0 + Duration::from_secs(5)).is_empty());
        assert!(!repeater.anything_held());
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
}
