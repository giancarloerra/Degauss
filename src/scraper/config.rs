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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub system_media_types: BTreeMap<String, String>,
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
            system_media_types: BTreeMap::new(),
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
        validate_media_type(&self.media_type)?;
        let mut media_override_names = BTreeSet::new();
        for (system, media_type) in &self.system_media_types {
            if !valid_system_override_name(system) {
                return Err(Error::new(
                    ErrorKind::Configuration,
                    format!("invalid ScreenScraper artwork override for {system:?}"),
                ));
            }
            validate_media_type(media_type).map_err(|error| {
                Error::new(
                    ErrorKind::Configuration,
                    format!(
                        "invalid ScreenScraper artwork override for {system:?}: {}",
                        error.detail
                    ),
                )
            })?;
            if !media_override_names.insert(system.to_ascii_lowercase()) {
                return Err(Error::new(
                    ErrorKind::Configuration,
                    format!("more than one ScreenScraper artwork override matches {system:?}"),
                ));
            }
        }
        let mut override_names = BTreeSet::new();
        for (system, id) in &self.system_ids {
            if !valid_system_override_name(system) || *id == 0 {
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

    /// Resolve by Degauss's stable system id, not the displayed name or the
    /// numeric ScreenScraper platform shared by some distinct systems.
    pub fn media_type_for(&self, system_id: &str) -> &str {
        self.media_type_override(system_id)
            .unwrap_or(&self.media_type)
    }

    pub fn media_type_override(&self, system_id: &str) -> Option<&str> {
        self.system_media_types
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(system_id))
            .map(|(_, media_type)| media_type.as_str())
    }

    /// None removes the override so later global changes apply again.
    pub fn set_media_type_override(
        &mut self,
        system_id: &str,
        media_type: Option<&str>,
    ) -> Result<()> {
        if !valid_system_override_name(system_id) {
            return Err(Error::new(
                ErrorKind::Configuration,
                format!("invalid ScreenScraper artwork override for {system_id:?}"),
            ));
        }
        if let Some(media_type) = media_type {
            validate_media_type(media_type)?;
        }
        self.system_media_types
            .retain(|key, _| !key.eq_ignore_ascii_case(system_id));
        if let Some(media_type) = media_type {
            self.system_media_types
                .insert(system_id.to_string(), media_type.to_string());
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

fn valid_system_override_name(system: &str) -> bool {
    !system.is_empty() && system.len() <= 80 && !system.chars().any(char::is_control)
}

pub(super) fn validate_media_type(media_type: &str) -> Result<()> {
    if media_type.is_empty()
        || media_type.len() > 24
        || !media_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(Error::new(
            ErrorKind::Configuration,
            "media_type must contain only letters, digits or '-'",
        ));
    }
    Ok(())
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
            .field("system_media_types", &self.system_media_types)
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
        assert_eq!(settings.media_type, "ss");
        assert!(settings.system_media_types.is_empty());
        assert_eq!(settings.media_type_for("NES"), "ss");
        assert!(!settings.ready());
    }

    #[test]
    fn user_settings_round_trip_without_developer_credentials() {
        let path = temp("roundtrip");
        let settings = ScraperSettings {
            username: "player".into(),
            password: "not-a-real-password".into(),
            accepted_plaintext_warning: true,
            media_type: "wheel-hd".into(),
            system_media_types: [
                ("NES".into(), "box-2D".into()),
                ("Arcade".into(), "ss".into()),
            ]
            .into(),
            system_ids: [("FutureSystem".into(), 999)].into(),
            ..Default::default()
        };
        settings.save(&path).unwrap();
        let read = ScraperSettings::load(&path).unwrap();
        assert_eq!(read, settings);
        assert_eq!(read.media_type_for("nes"), "box-2D");
        assert_eq!(read.media_type_for("ARCADE"), "ss");
        assert_eq!(read.media_type_for("FutureSystem"), "wheel-hd");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("developer_id"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_media_type_is_preserved_without_creating_system_overrides() {
        let path = temp("legacy-media");
        std::fs::write(
            &path,
            "username = 'player'\nmedia_type = 'sstitle'\n[system_ids]\nNES = 3\n",
        )
        .unwrap();
        let settings = ScraperSettings::load(&path).unwrap();
        assert_eq!(settings.media_type_for("NES"), "sstitle");
        assert_eq!(settings.media_type_override("NES"), None);
        assert_eq!(settings.system_ids.get("NES"), Some(&3));
        settings.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("system_media_types"));
        assert_eq!(ScraperSettings::load(&path).unwrap(), settings);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn artwork_overrides_use_stable_case_insensitive_ids_and_can_inherit_again() {
        let mut settings = ScraperSettings {
            system_ids: [("NES".into(), 3), ("NESMusic".into(), 3)].into(),
            ..Default::default()
        };
        settings
            .set_media_type_override("NES", Some("box-2D"))
            .unwrap();
        assert_eq!(settings.media_type_for("nes"), "box-2D");
        assert_eq!(settings.media_type_for("NESMusic"), "ss");
        settings
            .set_media_type_override("nes", Some("box-3D"))
            .unwrap();
        assert_eq!(settings.system_media_types.len(), 1);
        assert_eq!(settings.media_type_for("NES"), "box-3D");
        settings.media_type = "wheel-hd".into();
        assert_eq!(settings.media_type_for("NES"), "box-3D");
        assert_eq!(settings.media_type_for("NESMusic"), "wheel-hd");
        settings.set_media_type_override("NeS", None).unwrap();
        assert!(settings.system_media_types.is_empty());
        assert_eq!(settings.media_type_for("NES"), "wheel-hd");
    }

    #[test]
    fn invalid_artwork_overrides_are_rejected_without_replacing_saved_settings() {
        let path = temp("invalid-media");
        let settings = ScraperSettings {
            system_media_types: [("NES".into(), "box-2D".into())].into(),
            ..Default::default()
        };
        settings.save(&path).unwrap();
        let previous = std::fs::read(&path).unwrap();
        for (system, media_type) in [
            (String::new(), "ss".to_string()),
            ("N".repeat(81), "ss".to_string()),
            ("NES\n".to_string(), "ss".to_string()),
            ("NES".to_string(), String::new()),
            ("NES".to_string(), "s".repeat(25)),
            ("NES".to_string(), "box_2D".to_string()),
            ("NES".to_string(), "box 2D".to_string()),
            ("NES".to_string(), "ss\n".to_string()),
        ] {
            let mut invalid = settings.clone();
            assert_eq!(
                invalid
                    .set_media_type_override(&system, Some(&media_type))
                    .unwrap_err()
                    .kind,
                ErrorKind::Configuration
            );
            assert_eq!(
                invalid, settings,
                "failed edits must leave the old choice intact"
            );
            invalid.system_media_types = [(system, media_type)].into();
            assert_eq!(
                invalid.save(&path).unwrap_err().kind,
                ErrorKind::Configuration
            );
            assert_eq!(std::fs::read(&path).unwrap(), previous);
        }
        let mut duplicate = settings;
        duplicate
            .system_media_types
            .insert("nes".into(), "ss".into());
        assert_eq!(
            duplicate.save(&path).unwrap_err().kind,
            ErrorKind::Configuration
        );
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        std::fs::write(&path, "[system_media_types]\nNES = 'box-2D'\nnes = 'ss'\n").unwrap();
        assert_eq!(
            ScraperSettings::load(&path).unwrap_err().kind,
            ErrorKind::Configuration
        );
        std::fs::remove_file(path).unwrap();
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
