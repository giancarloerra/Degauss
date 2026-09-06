use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Error, ErrorKind, Result};

pub const SETTINGS_FILE: &str = "screenscraper.toml";
pub const DEFAULT_HASH_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_MEDIA_BYTES: u64 = 32 * 1024 * 1024;

// Keep both compile-time application fields as addressable data. Besides
// supplying the runtime client, this lets the package script prove that an
// optimized MiSTer executable contains the exact authorization it was built
// with instead of trusting Cargo's environment alone.
#[used]
static EMBEDDED_DEVELOPER_ID: Option<&str> = option_env!("DEGAUSS_SCREENSCRAPER_DEVID");
#[used]
static EMBEDDED_DEVELOPER_PASSWORD: Option<&str> = option_env!("DEGAUSS_SCREENSCRAPER_DEVPASSWORD");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePolicy {
    Off,
    #[default]
    MissingOnly,
    ReplaceExisting,
}

impl ImagePolicy {
    pub const ALL: [Self; 3] = [Self::Off, Self::MissingOnly, Self::ReplaceExisting];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::MissingOnly => "Missing only",
            Self::ReplaceExisting => "Replace existing",
        }
    }

    pub fn step(self, delta: isize) -> Self {
        step(Self::ALL, self, delta)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataPolicy {
    Off,
    #[default]
    FillMissing,
    ReplaceExisting,
}

impl MetadataPolicy {
    pub const ALL: [Self; 3] = [Self::Off, Self::FillMissing, Self::ReplaceExisting];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::FillMissing => "Fill missing",
            Self::ReplaceExisting => "Replace existing",
        }
    }

    pub fn step(self, delta: isize) -> Self {
        step(Self::ALL, self, delta)
    }
}

fn step<T: Copy + PartialEq, const N: usize>(values: [T; N], current: T, delta: isize) -> T {
    let at = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    values[(at as isize + delta).rem_euclid(N as isize) as usize]
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScraperSettings {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub accepted_plaintext_warning: bool,
    #[serde(default)]
    pub image_policy: ImagePolicy,
    #[serde(default)]
    pub metadata_policy: MetadataPolicy,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_hash_limit_mib")]
    pub hash_limit_mib: u64,
    #[serde(default = "default_media_limit_mib")]
    pub max_media_mib: u64,
    #[serde(default = "default_media_type")]
    pub media_type: String,
    #[serde(default)]
    pub system_ids: BTreeMap<String, u32>,
}

impl Default for ScraperSettings {
    fn default() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
            accepted_plaintext_warning: false,
            image_policy: ImagePolicy::default(),
            metadata_policy: MetadataPolicy::default(),
            region: None,
            language: None,
            hash_limit_mib: default_hash_limit_mib(),
            max_media_mib: default_media_limit_mib(),
            media_type: default_media_type(),
            system_ids: BTreeMap::new(),
        }
    }
}

fn default_hash_limit_mib() -> u64 {
    DEFAULT_HASH_LIMIT_BYTES / 1024 / 1024
}

fn default_media_limit_mib() -> u64 {
    DEFAULT_MAX_MEDIA_BYTES / 1024 / 1024
}

fn default_media_type() -> String {
    "ss".to_string()
}

impl ScraperSettings {
    pub fn path_beside(settings_path: &Path) -> PathBuf {
        settings_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(SETTINGS_FILE)
    }

    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let parsed: Self = toml::from_str(&text).map_err(|_| {
                    Error::new(
                        ErrorKind::Configuration,
                        format!("{} is malformed", path.display()),
                    )
                })?;
                parsed.validate()?;
                Ok(parsed)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(Error::new(
                ErrorKind::Configuration,
                format!("could not read {}: {error}", path.display()),
            )),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let text = toml::to_string_pretty(self).map_err(|error| {
            Error::new(
                ErrorKind::Configuration,
                format!("could not encode scraper settings: {error}"),
            )
        })?;
        let body = format!(
            "# Written by Degauss. The password is plain text because MiSTer cards\n\
             # normally use FAT/exFAT, which has no private Unix file permissions.\n\n{text}"
        );
        atomic_write(path, body.as_bytes(), "scraper settings")
    }

    pub fn validate(&self) -> Result<()> {
        validate_text("username", &self.username, 80)?;
        validate_text("password", &self.password, 256)?;
        validate_code("region", self.region.as_deref(), 2, 3)?;
        validate_code("language", self.language.as_deref(), 2, 2)?;
        if self.hash_limit_mib == 0 || self.hash_limit_mib > 4096 {
            return Err(Error::new(
                ErrorKind::Configuration,
                "hash_limit_mib must be between 1 and 4096",
            ));
        }
        if self.max_media_mib == 0 || self.max_media_mib > 256 {
            return Err(Error::new(
                ErrorKind::Configuration,
                "max_media_mib must be between 1 and 256",
            ));
        }
        if self.media_type.is_empty()
            || self.media_type.len() > 24
            || !self
                .media_type
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Error::new(
                ErrorKind::Configuration,
                "media_type must contain only letters, digits or '-'",
            ));
        }
        let mut override_names = BTreeSet::new();
        for (system, id) in &self.system_ids {
            if system.is_empty()
                || system.len() > 80
                || system.chars().any(char::is_control)
                || *id == 0
            {
                return Err(Error::new(
                    ErrorKind::Configuration,
                    format!("invalid ScreenScraper system override for {system:?}"),
                ));
            }
            if !override_names.insert(system.to_ascii_lowercase()) {
                return Err(Error::new(
                    ErrorKind::Configuration,
                    format!("more than one ScreenScraper system override matches {system:?}"),
                ));
            }
        }
        Ok(())
    }

    pub fn ready(&self) -> bool {
        !self.username.is_empty()
            && !self.password.is_empty()
            && self.accepted_plaintext_warning
            && !(self.image_policy == ImagePolicy::Off
                && self.metadata_policy == MetadataPolicy::Off)
    }

    pub fn hash_limit_bytes(&self) -> u64 {
        self.hash_limit_mib.saturating_mul(1024 * 1024)
    }

    pub fn max_media_bytes(&self) -> u64 {
        self.max_media_mib.saturating_mul(1024 * 1024)
    }

    pub fn clear_login(&mut self) {
        self.username.clear();
        self.password.clear();
        self.accepted_plaintext_warning = false;
    }
}

fn validate_text(name: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(Error::new(
            ErrorKind::Configuration,
            format!("{name} is longer than {max} characters or contains a control character"),
        ));
    }
    Ok(())
}

fn validate_code(name: &str, value: Option<&str>, min: usize, max: usize) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() < min
        || value.len() > max
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(Error::new(
            ErrorKind::Configuration,
            format!("{name} must be {min} to {max} ASCII letters or digits"),
        ));
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeveloperCredentials {
    pub developer_id: String,
    pub developer_password: String,
}

impl std::fmt::Debug for ScraperSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScraperSettings")
            .field("username", &"[redacted]")
            .field("password", &"[redacted]")
            .field(
                "accepted_plaintext_warning",
                &self.accepted_plaintext_warning,
            )
            .field("image_policy", &self.image_policy)
            .field("metadata_policy", &self.metadata_policy)
            .field("region", &self.region)
            .field("language", &self.language)
            .field("hash_limit_mib", &self.hash_limit_mib)
            .field("max_media_mib", &self.max_media_mib)
            .field("media_type", &self.media_type)
            .field("system_ids", &self.system_ids)
            .finish()
    }
}

impl std::fmt::Debug for DeveloperCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeveloperCredentials")
            .field("developer_id", &"[redacted]")
            .field("developer_password", &"[redacted]")
            .finish()
    }
}

impl DeveloperCredentials {
    pub fn embedded() -> Result<Self> {
        Self::from_embedded(EMBEDDED_DEVELOPER_ID, EMBEDDED_DEVELOPER_PASSWORD)
    }

    fn from_embedded(developer_id: Option<&str>, developer_password: Option<&str>) -> Result<Self> {
        match (developer_id, developer_password) {
            (None, None) => Err(Error::new(
                ErrorKind::Configuration,
                "ScreenScraper support is unavailable in this build",
            )),
            (Some(developer_id), Some(developer_password)) => {
                let credentials = Self {
                    developer_id: developer_id.to_string(),
                    developer_password: developer_password.to_string(),
                };
                credentials.validate()?;
                Ok(credentials)
            }
            _ => Err(Error::new(
                ErrorKind::Configuration,
                "ScreenScraper support is incomplete in this build",
            )),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.developer_id.is_empty() || self.developer_password.is_empty() {
            return Err(Error::new(
                ErrorKind::Configuration,
                "developer ID and password must both be present",
            ));
        }
        validate_text("developer ID", &self.developer_id, 128)?;
        validate_text("developer password", &self.developer_password, 256)
    }
}

fn atomic_write(path: &Path, bytes: &[u8], what: &str) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::local(format!("could not create {}: {error}", parent.display())))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("screenscraper.toml");
    let temp = parent.join(format!(".{file_name}.{}.part", std::process::id()));
    if temp.exists() {
        std::fs::remove_file(&temp)
            .map_err(|error| Error::local(format!("could not clear old {what}: {error}")))?;
    }
    let outcome = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|error| Error::local(format!("could not create {what}: {error}")))?;
        file.write_all(bytes)
            .map_err(|error| Error::local(format!("could not write {what}: {error}")))?;
        file.sync_all()
            .map_err(|error| Error::local(format!("could not sync {what}: {error}")))?;
        std::fs::rename(&temp, path)
            .map_err(|error| Error::local(format!("could not install {what}: {error}")))?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "degauss-scraper-settings-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn missing_settings_are_safe_defaults_and_do_not_enable_network_access() {
        let settings = ScraperSettings::load(Path::new("/not/here/screenscraper.toml")).unwrap();
        assert_eq!(settings.image_policy, ImagePolicy::MissingOnly);
        assert_eq!(settings.metadata_policy, MetadataPolicy::FillMissing);
        assert!(!settings.ready());
    }

    #[test]
    fn user_settings_round_trip_without_developer_credentials() {
        let path = temp("roundtrip");
        let settings = ScraperSettings {
            username: "player".into(),
            password: "not-a-real-password".into(),
            accepted_plaintext_warning: true,
            system_ids: [("FutureSystem".into(), 999)].into(),
            ..Default::default()
        };
        settings.save(&path).unwrap();
        let read = ScraperSettings::load(&path).unwrap();
        assert_eq!(read, settings);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("developer_id"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unknown_future_user_setting_is_ignored_for_downgrade_compatibility() {
        let text = "username = 'a'\nfuture_setting = true\n";
        let parsed: ScraperSettings = toml::from_str(text).unwrap();
        assert_eq!(parsed.username, "a");
    }

    #[test]
    fn both_policies_off_cannot_be_ready() {
        let settings = ScraperSettings {
            username: "a".into(),
            password: "b".into(),
            accepted_plaintext_warning: true,
            image_policy: ImagePolicy::Off,
            metadata_policy: MetadataPolicy::Off,
            ..Default::default()
        };
        assert!(!settings.ready());
    }

    #[test]
    fn debug_output_redacts_all_account_and_developer_credentials() {
        let settings = ScraperSettings {
            username: "user-name-marker".into(),
            password: "user-secret-marker".into(),
            ..Default::default()
        };
        let developer = DeveloperCredentials {
            developer_id: "developer-id-marker".into(),
            developer_password: "developer-secret-marker".into(),
        };

        let settings_debug = format!("{settings:?}");
        let developer_debug = format!("{developer:?}");
        assert!(!settings_debug.contains("user-name-marker"));
        assert!(!settings_debug.contains("user-secret-marker"));
        assert!(!developer_debug.contains("developer-id-marker"));
        assert!(!developer_debug.contains("developer-secret-marker"));
        assert!(settings_debug.contains("[redacted]"));
        assert!(developer_debug.contains("[redacted]"));
    }

    #[test]
    fn malformed_credential_files_never_echo_their_source_lines() {
        let user_path = temp("malformed-user-secret");
        std::fs::write(
            &user_path,
            "username = 'player'\npassword = 'user-secret-marker\n",
        )
        .unwrap();
        let user_error = ScraperSettings::load(&user_path).unwrap_err().to_string();
        assert!(!user_error.contains("user-secret-marker"));
        let _ = std::fs::remove_file(&user_path);
    }

    #[test]
    fn embedded_application_credentials_are_all_or_nothing() {
        assert!(DeveloperCredentials::from_embedded(None, None).is_err());
        assert!(DeveloperCredentials::from_embedded(Some("app"), None).is_err());
        assert!(DeveloperCredentials::from_embedded(None, Some("secret")).is_err());
        let credentials = DeveloperCredentials::from_embedded(Some("app"), Some("secret"))
            .expect("both fields make a complete integration");
        assert_eq!(credentials.developer_id, "app");
        assert_eq!(credentials.developer_password, "secret");
    }

    #[test]
    fn bad_codes_limits_and_overrides_are_rejected() {
        for mut settings in [
            ScraperSettings {
                region: Some("u/s".into()),
                ..Default::default()
            },
            ScraperSettings {
                language: Some("eng".into()),
                ..Default::default()
            },
            ScraperSettings {
                hash_limit_mib: 0,
                ..Default::default()
            },
            ScraperSettings {
                max_media_mib: 0,
                ..Default::default()
            },
        ] {
            assert!(settings.validate().is_err());
            settings.clear_login();
        }
        let settings = ScraperSettings {
            system_ids: [("Bad".into(), 0)].into(),
            ..Default::default()
        };
        assert!(settings.validate().is_err());

        let settings = ScraperSettings {
            system_ids: [("Genesis".into(), 1), ("genesis".into(), 2)].into(),
            ..Default::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn clearing_login_clears_the_plaintext_consent_too() {
        let mut settings = ScraperSettings {
            username: "a".into(),
            password: "b".into(),
            accepted_plaintext_warning: true,
            ..Default::default()
        };
        settings.clear_login();
        assert!(settings.username.is_empty());
        assert!(settings.password.is_empty());
        assert!(!settings.accepted_plaintext_warning);
    }
}
