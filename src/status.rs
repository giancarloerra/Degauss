//! The machine's own state, for the corner of the bar: the time, and
//! whether wired and wireless connections are up.
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

/// A physical Ethernet interface needs an operational link and a usable
/// address, just as Wi-Fi does. Kernel metadata distinguishes physical
/// Ethernet from radios, loopback and virtual interfaces without naming it.
pub fn ethernet_up() -> bool {
    ethernet_up_from(std::path::Path::new("/sys/class/net"), has_address)
}

fn ethernet_up_from(
    interfaces: &std::path::Path,
    addressed: impl Fn(&std::ffi::OsStr) -> bool,
) -> bool {
    let Ok(interfaces) = std::fs::read_dir(interfaces) else {
        return false;
    };
    interfaces.flatten().any(|entry| {
        let path = entry.path();
        path.join("device").exists()
            && !path.join("wireless").exists()
            && std::fs::read_to_string(path.join("type")).is_ok_and(|value| value.trim() == "1")
            && std::fs::read_to_string(path.join("operstate"))
                .is_ok_and(|value| matches!(value.trim(), "up" | "unknown"))
            && std::fs::read_to_string(path.join("carrier")).is_ok_and(|value| value.trim() == "1")
            && addressed(&entry.file_name())
    })
}

/// IPv4 aliases use the base interface name followed by a colon.
#[cfg(any(target_os = "linux", test))]
fn interface_name_matches(address_name: &[u8], interface_name: &[u8]) -> bool {
    address_name == interface_name
        || address_name
            .strip_prefix(interface_name)
            .is_some_and(|suffix| suffix.starts_with(b":"))
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
        if !interface_name_matches(name.to_bytes(), want.as_bytes()) {
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

/// The clock and connections, re-read no more often than they can change.
///
/// The clock and connection state are read on a timer rather than every
/// frame: at the fastest scroll step a frame is seven milliseconds, and
/// querying the kernel on each of them would be needless work.
pub struct Status {
    pub clock: String,
    pub wifi: bool,
    pub ethernet: bool,
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
            ethernet: ethernet_up(),
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
            || fresh.ethernet != self.ethernet
            || fresh.bluetooth != self.bluetooth;
        self.clock = fresh.clock;
        self.wifi = fresh.wifi;
        self.ethernet = fresh.ethernet;
        self.bluetooth = fresh.bluetooth;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wired_indicator_requires_a_physical_ethernet_link_and_address() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("degauss-ethernet-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let wired = root.join("enp1s0");
        std::fs::create_dir_all(wired.join("device")).unwrap();
        for (name, value) in [("type", "1\n"), ("operstate", "up\n"), ("carrier", "1\n")] {
            std::fs::write(wired.join(name), value).unwrap();
        }
        assert!(ethernet_up_from(&root, |name| name == "enp1s0"));
        assert!(
            ethernet_up_from(&root, |name| {
                interface_name_matches(b"enp1s0:1", name.to_str().unwrap().as_bytes())
            }),
            "the sole IPv4 address may belong to a physical interface alias"
        );
        assert!(
            !ethernet_up_from(&root, |_| false),
            "an address is required"
        );

        std::fs::write(wired.join("operstate"), "unknown\n").unwrap();
        assert!(
            ethernet_up_from(&root, |_| true),
            "drivers without operational-state reporting can still have a usable link"
        );
        assert!(!ethernet_up_from(&root, |_| false));
        std::fs::write(wired.join("carrier"), "0\n").unwrap();
        assert!(!ethernet_up_from(&root, |_| true));
        std::fs::write(wired.join("carrier"), "1\n").unwrap();
        std::fs::write(wired.join("operstate"), "up\n").unwrap();

        for (name, disabled, enabled) in [
            ("operstate", "down\n", "up\n"),
            ("carrier", "0\n", "1\n"),
            ("type", "772\n", "1\n"),
        ] {
            std::fs::write(wired.join(name), disabled).unwrap();
            assert!(
                !ethernet_up_from(&root, |_| true),
                "reject {name}={disabled}"
            );
            std::fs::write(wired.join(name), enabled).unwrap();
            assert!(ethernet_up_from(&root, |_| true), "reconnection is reread");
        }
        std::fs::create_dir(wired.join("wireless")).unwrap();
        assert!(
            !ethernet_up_from(&root, |_| true),
            "radios keep the Wi-Fi indicator"
        );
        std::fs::remove_dir(wired.join("wireless")).unwrap();
        std::fs::remove_dir(wired.join("device")).unwrap();
        assert!(
            !ethernet_up_from(&root, |_| true),
            "virtual interfaces are not Ethernet"
        );
        std::fs::create_dir(wired.join("device")).unwrap();
        std::fs::remove_file(wired.join("carrier")).unwrap();
        assert!(
            !ethernet_up_from(&root, |_| true),
            "unreadable link state is not connected"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ipv4_aliases_match_only_their_base_interface() {
        assert!(interface_name_matches(b"eth0", b"eth0"));
        assert!(interface_name_matches(b"eth0:1", b"eth0"));
        assert!(!interface_name_matches(b"eth01", b"eth0"));
        assert!(!interface_name_matches(b"eth0-other", b"eth0"));
        assert!(!interface_name_matches(b"eth1:1", b"eth0"));
    }

    #[test]
    fn ethernet_changes_use_the_existing_five_second_status_refresh() {
        let mut status = Status::read();
        let checked = status.checked;
        status.ethernet = !ethernet_up();
        let unchanged = status.ethernet;
        assert!(!status.refresh(checked + Duration::from_secs(4)));
        assert_eq!(
            status.ethernet, unchanged,
            "no extra network polling between refreshes"
        );
        assert!(status.refresh(checked + Duration::from_secs(5)));
        assert_eq!(status.ethernet, ethernet_up());
    }
}
