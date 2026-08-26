//! The machine's own state, for the corner of the bar: the time, and
//! whether the radios are up.
//!
//! Read from the kernel rather than from any daemon, so there is nothing to
//! install and nothing to keep running. Everything here answers "no" rather
//! than failing: a bar that cannot draw itself because a sysfs file moved
//! would be worse than a bar with one icon missing.

use std::time::{Duration, Instant};

/// The local time as "HH:MM".
///
/// Through libc rather than by arithmetic on the epoch: the card's clock is
/// UTC and the zone comes from /etc/localtime, which only the C library
/// knows how to read. Doing it by hand would show the wrong hour for most
/// of the year in most of the world.
#[cfg(target_os = "linux")]
pub fn clock() -> String {
    // SAFETY: time(NULL) is always valid; localtime_r fills a struct we own.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut parts: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut parts).is_null() {
            return String::new();
        }
        format!("{:02}:{:02}", parts.tm_hour, parts.tm_min)
    }
}

#[cfg(not(target_os = "linux"))]
pub fn clock() -> String {
    String::new()
}

/// True when a wireless interface has actually been given an address.
///
/// An interface is wireless when it has a `wireless` directory, which is how
/// the kernel marks one; the name is not to be trusted, since it is `wlan0`
/// on this card and something else on the next.
///
/// Three things have to hold, and the last is the one that matters. The
/// radio being up says only that it was switched on. Being associated says
/// it joined a network. Neither means the machine can be reached: a router
/// that hands out no lease leaves the card associated and addressless, which
/// is not a working connection and must not be drawn as one.
///
/// A link-local 169.254 address is what a failed DHCP leaves behind, so it
/// counts as no address at all.
pub fn wifi_up() -> bool {
    let Ok(interfaces) = std::fs::read_dir("/sys/class/net") else {
        return false;
    };
    interfaces.flatten().any(|entry| {
        let path = entry.path();
        if !path.join("wireless").exists() {
            return false;
        }
        let up = std::fs::read_to_string(path.join("operstate"))
            .map(|state| state.trim() == "up")
            .unwrap_or(false);
        // Unreadable while the interface is down, which is not associated.
        let linked = std::fs::read_to_string(path.join("carrier"))
            .map(|state| state.trim() == "1")
            .unwrap_or(false);
        up && linked && has_address(&entry.file_name())
    })
}

/// Whether the named interface holds a usable IPv4 address.
#[cfg(target_os = "linux")]
fn has_address(want: &std::ffi::OsStr) -> bool {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;

    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: the pointer is written by the call and freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return false;
    }
    let mut found = false;
    let mut item = list;
    while !item.is_null() {
        // SAFETY: walking the list the call built, one node at a time.
        let node = unsafe { &*item };
        item = node.ifa_next;

        if node.ifa_name.is_null() || node.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: the name is a NUL-terminated string owned by the list.
        let name = unsafe { CStr::from_ptr(node.ifa_name) };
        if name.to_bytes() != want.as_bytes() {
            continue;
        }
        // SAFETY: reading the family tag, which every address carries.
        if unsafe { (*node.ifa_addr).sa_family } != libc::AF_INET as libc::sa_family_t {
            continue;
        }
        // SAFETY: a family of AF_INET means this is a sockaddr_in.
        let addr = unsafe { &*(node.ifa_addr as *const libc::sockaddr_in) };
        let octets = u32::from_be(addr.sin_addr.s_addr).to_be_bytes();
        // 169.254.0.0/16 is what a failed DHCP leaves behind.
        if octets[0] == 169 && octets[1] == 254 {
            continue;
        }
        found = true;
        break;
    }
    // SAFETY: freeing exactly the list the call allocated.
    unsafe { libc::freeifaddrs(list) };
    found
}

/// Off the device there is no interface to ask, and the two checks above
/// have already answered false.
#[cfg(not(target_os = "linux"))]
fn has_address(_want: &std::ffi::OsStr) -> bool {
    false
}

/// True when a bluetooth adapter is present.
pub fn bluetooth_present() -> bool {
    std::fs::read_dir("/sys/class/bluetooth")
        .map(|mut entries| entries.any(|entry| entry.is_ok()))
        .unwrap_or(false)
}

/// The three of them, re-read no more often than they can change.
///
/// The clock only moves once a minute and the radios almost never, so this
/// is read on a timer rather than every frame: at the fastest scroll step a
/// frame is seven milliseconds, and three file reads in each of them would
/// be the most expensive thing on screen.
pub struct Status {
    pub clock: String,
    pub wifi: bool,
    pub bluetooth: bool,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    checked: Instant,
}

impl Status {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    const EVERY: Duration = Duration::from_secs(5);

    pub fn read() -> Self {
        Status {
            clock: clock(),
            wifi: wifi_up(),
            bluetooth: bluetooth_present(),
            checked: Instant::now(),
        }
    }

    /// True when anything changed, so the caller knows to redraw.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn refresh(&mut self, now: Instant) -> bool {
        if now.duration_since(self.checked) < Self::EVERY {
            return false;
        }
        self.checked = now;
        let fresh = Status::read();
        let changed = fresh.clock != self.clock
            || fresh.wifi != self.wifi
            || fresh.bluetooth != self.bluetooth;
        self.clock = fresh.clock;
        self.wifi = fresh.wifi;
        self.bluetooth = fresh.bluetooth;
        changed
    }
}
