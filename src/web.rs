//! Optional, frontend-owned web access. No worker survives the frontend.
pub mod api;
mod auth;
pub mod library;
mod server;
pub mod upload;

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use server::{Bridge, Request, Server, Status};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Https,
    Http,
}

impl Mode {
    pub fn scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https | Self::Off => "https",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Https => "HTTPS",
            Self::Http => "HTTP (unencrypted)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    pub port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            port: 8443,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.port < 1024 {
            return Err("Choose a web port from 1024 to 65535.".into());
        }
        Ok(())
    }

    pub fn address(&self, ip: IpAddr) -> String {
        let host = match ip {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        format!("{}://{host}:{}/", self.mode.scheme(), self.port)
    }
}

/// Exact interface addresses, not a guessed hostname or an external lookup.
pub fn local_addresses() -> Result<Vec<IpAddr>, String> {
    #[cfg(unix)]
    {
        let mut head = std::ptr::null_mut();
        // SAFETY: getifaddrs owns the list until freeifaddrs below. Each sockaddr
        // is read according to its address family, and never retained.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return Err(format!(
                "Reading network addresses failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut result = Vec::new();
        let mut cursor = head;
        while !cursor.is_null() {
            let entry = unsafe { &*cursor };
            if !entry.ifa_addr.is_null() && entry.ifa_flags & libc::IFF_UP as u32 != 0 {
                let family = unsafe { (*entry.ifa_addr).sa_family as i32 };
                let ip = match family {
                    libc::AF_INET => {
                        let addr = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
                        Some(IpAddr::V4(std::net::Ipv4Addr::from(
                            addr.sin_addr.s_addr.to_ne_bytes(),
                        )))
                    }
                    libc::AF_INET6 => {
                        let addr = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in6>() };
                        let ip = std::net::Ipv6Addr::from(addr.sin6_addr.s6_addr);
                        // Link-local addresses need a client-specific interface scope.
                        (!ip.is_unicast_link_local()).then_some(IpAddr::V6(ip))
                    }
                    _ => None,
                };
                if let Some(ip) = ip.filter(|ip| !ip.is_loopback() && !ip.is_unspecified()) {
                    result.push(ip);
                }
            }
            cursor = entry.ifa_next;
        }
        unsafe { libc::freeifaddrs(head) };
        result.sort();
        result.dedup();
        Ok(result)
    }
    #[cfg(not(unix))]
    Err("Network address discovery requires a Unix host.".into())
}

/// Integer-sized black/white modules with a four-module quiet border.
pub fn qr_pixels(address: &str, max_side: usize) -> Result<(usize, Vec<u8>), String> {
    let code = qrcode::QrCode::with_error_correction_level(address, qrcode::EcLevel::M)
        .map_err(|e| format!("Creating address QR code failed: {e}"))?;
    let modules = code.width() + 8;
    let scale = max_side / modules;
    if scale < 2 {
        return Err("Not enough screen space for a readable address QR code.".into());
    }
    let side = modules * scale;
    let mut pixels = vec![255; side * side * 3];
    for y in 0..code.width() {
        for x in 0..code.width() {
            if code[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let at = (((y + 4) * scale + dy) * side + (x + 4) * scale + dx) * 3;
                        pixels[at..at + 3].fill(0);
                    }
                }
            }
        }
    }
    Ok((side, pixels))
}

pub fn password_is_set(directory: &Path) -> bool {
    directory.join("password.json").is_file()
}

pub fn set_password(directory: &Path, password: &str) -> Result<(), String> {
    auth::set_password(directory, password)
}

pub fn install_password(directory: &Path, prepared: &str) -> Result<(), String> {
    auth::install_password(directory, prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_default_to_no_listener_and_urls_match_protocol_and_port() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(config.mode, Mode::Off);
        let config = Config {
            mode: Mode::Http,
            port: 8080,
        };
        assert_eq!(
            config.address("192.0.2.4".parse().unwrap()),
            "http://192.0.2.4:8080/"
        );
        assert_eq!(
            config.address("2001:db8::1".parse().unwrap()),
            "http://[2001:db8::1]:8080/"
        );
        assert!(Config { port: 0, ..config }.validate().is_err());
    }

    #[test]
    fn qr_uses_integer_pixels_and_preserves_white_quiet_zone_on_crt() {
        let (side, pixels) = qr_pixels("https://192.0.2.4:8443/", 150).unwrap();
        assert!(side <= 150);
        assert!(pixels[..side * 3 * 8].iter().all(|p| *p == 255));
        assert!(pixels.iter().all(|p| *p == 0 || *p == 255));
        assert!(pixels.contains(&0));
        assert!(qr_pixels("https://192.0.2.4:8443/", 40).is_err());
    }
}
