//! ScreenScraper integration.
//!
//! The scraper is deliberately isolated from the browser's read-only model:
//! network work happens on worker threads, and a gamelist is installed only
//! after the complete proposed file has parsed through Degauss's real reader.

mod api;
mod config;
mod gamelist_edit;
mod hashes;
mod platforms;
mod targets;
mod worker;

pub use config::{DeveloperCredentials, ImagePolicy, MetadataPolicy, ScraperSettings};
pub use targets::{Scope, Target};
pub use worker::{
    start, start_preview, start_search, start_selected, Event, Job, Phase, PreviewEvent,
    PreviewJob, PreviewRequest, Progress, Request, SearchEvent, SearchJob, SearchRequest,
};

/// Resolve the reviewed ScreenScraper platform id used by the production
/// target collector. Manual one-game searches must use the identical table
/// and user override rather than maintaining a second mapping in the UI.
pub fn platform_id(
    system_id: &str,
    overrides: &std::collections::BTreeMap<String, u32>,
) -> Option<u32> {
    platforms::id_for(system_id, overrides)
}

use std::fmt;

/// A scraper failure safe to put on screen or in Degauss's local log.
///
/// Request URLs are deliberately absent because ScreenScraper authentication
/// is carried in their query strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub detail: String,
}

impl Error {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Error {
            kind,
            detail: detail.into(),
        }
    }

    pub fn local(detail: impl Into<String>) -> Self {
        Self::new(ErrorKind::Local, detail)
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::Transport | ErrorKind::Timeout | ErrorKind::RateLimited | ErrorKind::Server
        )
    }

    /// A concise explanation for the interface. `detail` remains available
    /// to the local log, where a technical diagnosis is useful and there is
    /// enough room to show it.
    pub fn user_message(&self) -> &'static str {
        match self.kind {
            ErrorKind::Configuration => "Scraper setup needs attention",
            ErrorKind::Authentication => "ScreenScraper rejected the login",
            ErrorKind::Unavailable | ErrorKind::Server => "ScreenScraper is unavailable",
            ErrorKind::RateLimited => "ScreenScraper rate limit reached",
            ErrorKind::DailyQuota => "Daily request allowance exhausted",
            ErrorKind::FailedQuota => "Failed-search allowance exhausted",
            ErrorKind::NotFound => "No matching game was found",
            ErrorKind::MalformedResponse => "ScreenScraper response was unreadable",
            ErrorKind::Transport => "Network connection failed",
            ErrorKind::Timeout => "ScreenScraper timed out",
            ErrorKind::Local => "A local game file could not be read or updated",
            ErrorKind::Cancelled => "Scrape cancelled",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.label(), self.detail)
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Configuration,
    Authentication,
    Unavailable,
    RateLimited,
    DailyQuota,
    FailedQuota,
    NotFound,
    MalformedResponse,
    Transport,
    Timeout,
    Server,
    Local,
    Cancelled,
}

impl ErrorKind {
    fn label(self) -> &'static str {
        match self {
            ErrorKind::Configuration => "configuration",
            ErrorKind::Authentication => "authentication",
            ErrorKind::Unavailable => "service unavailable",
            ErrorKind::RateLimited => "rate limit",
            ErrorKind::DailyQuota => "daily quota",
            ErrorKind::FailedQuota => "failed-search quota",
            ErrorKind::NotFound => "not found",
            ErrorKind::MalformedResponse => "bad server response",
            ErrorKind::Transport => "network",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Server => "server",
            ErrorKind::Local => "local write",
            ErrorKind::Cancelled => "cancelled",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Metadata fields Degauss and EmulationStation share.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub publisher: Option<String>,
    pub developer: Option<String>,
    pub releasedate: Option<String>,
    pub players: Option<String>,
    pub genre: Option<String>,
    pub lang: Option<String>,
}

impl Metadata {
    fn fields(&self) -> [(&'static str, Option<&str>); 8] {
        [
            ("name", self.name.as_deref()),
            ("desc", self.desc.as_deref()),
            ("publisher", self.publisher.as_deref()),
            ("developer", self.developer.as_deref()),
            ("releasedate", self.releasedate.as_deref()),
            ("players", self.players.as_deref()),
            ("genre", self.genre.as_deref()),
            ("lang", self.lang.as_deref()),
        ]
    }

    fn has_value(&self) -> bool {
        self.fields()
            .into_iter()
            .any(|(_, value)| value.is_some_and(|value| !value.trim().is_empty()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub url: String,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    pub level: Option<u32>,
    pub max_threads: Option<usize>,
    pub max_download_speed: Option<u64>,
    pub requests_today: Option<u64>,
    pub failed_today: Option<u64>,
    pub max_requests_per_minute: Option<u64>,
    pub max_requests_per_day: Option<u64>,
    pub max_failed_per_day: Option<u64>,
    pub preferred_region: Option<String>,
}

impl Account {
    pub fn requests_left(&self) -> Option<u64> {
        Some(
            self.max_requests_per_day?
                .saturating_sub(self.requests_today?),
        )
    }

    pub fn failed_left(&self) -> Option<u64> {
        Some(self.max_failed_per_day?.saturating_sub(self.failed_today?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub id: String,
    pub name: String,
    /// Every regional title returned for this game. Name searches compare
    /// against all of them; `name` remains the preferred title written to
    /// the gamelist.
    pub names: Vec<String>,
    pub metadata: Metadata,
    pub media: Option<Media>,
    pub rom_crc32: Option<String>,
    pub rom_md5: Option<String>,
    pub rom_sha1: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_allowance_saturates_when_the_server_reports_an_overrun() {
        let account = Account {
            requests_today: Some(101),
            max_requests_per_day: Some(100),
            failed_today: Some(12),
            max_failed_per_day: Some(10),
            ..Default::default()
        };
        assert_eq!(account.requests_left(), Some(0));
        assert_eq!(account.failed_left(), Some(0));
    }
}
