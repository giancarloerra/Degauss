use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::config::{validate_media_type, DeveloperCredentials, ScraperSettings};
use super::hashes::Hashes;
use super::{Account, Error, ErrorKind, Match, Media, Metadata, Result};

const API_BASE: &str = "https://api.screenscraper.fr/api2";
const MAX_XML_BYTES: u64 = 8 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_MEDIA_TIMEOUT: Duration = Duration::from_secs(300);
const MEDIA_TIMEOUT_MARGIN: Duration = Duration::from_secs(15);
const CURL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const CURL_ARGS: [&str; 3] = ["-q", "--config", "-"];
const CA_BUNDLE_CANDIDATES: [&str; 3] = [
    "/etc/ssl/certs/cacert.pem",
    "/etc/ssl/cert.pem",
    "/etc/ssl/certs/ca-certificates.crt",
];
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    fn get(&self, endpoint: &str, params: &[(String, String)], limit: u64) -> Result<HttpResponse>;
    fn get_media(
        &self,
        url: &str,
        limit: u64,
        max_kib_per_second: Option<u64>,
    ) -> Result<HttpResponse>;
}

/// HTTPS transport supplied by MiSTer's existing `curl` command.
///
/// ScreenScraper credentials are query parameters. They are written to
/// curl's stdin as an in-memory config, never placed in process arguments,
/// logs, temporary file names, or error messages.
#[derive(Clone)]
pub struct CurlTransport {
    cancelled: Arc<AtomicBool>,
    ca_bundle: Option<PathBuf>,
}

impl CurlTransport {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            ca_bundle: system_ca_bundle(),
        }
    }

    fn fetch(
        &self,
        url: &str,
        limit: u64,
        max_kib_per_second: Option<u64>,
    ) -> Result<HttpResponse> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
        }
        if max_kib_per_second == Some(0) {
            return Err(Error::new(
                ErrorKind::RateLimited,
                "ScreenScraper reported a zero media-download allowance",
            ));
        }
        let timeout = request_timeout(limit, max_kib_per_second);
        let response_file = ResponseFile::create()?;
        let config = curl_config(
            url,
            response_file.path(),
            limit,
            max_kib_per_second,
            self.ca_bundle.as_deref(),
        );
        // `-q` must be curl's first option. It prevents a user curlrc from
        // changing the request or copying credentials elsewhere.
        let mut child = Command::new("curl")
            .args(CURL_ARGS)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(curl_spawn_error)?;
        let write_result = child
            .stdin
            .take()
            .ok_or_else(|| logged_transport_error("curl stdin was unavailable"))
            .and_then(|mut stdin| {
                stdin.write_all(config.as_bytes()).map_err(|error| {
                    logged_transport_error(&format!("curl config write failed: {error}"))
                })
            });
        if let Err(error) = write_result {
            stop_child(&mut child);
            return Err(error);
        }

        let started = Instant::now();
        let status = loop {
            if self.cancelled.load(Ordering::Relaxed) {
                stop_child(&mut child);
                return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
            }
            if started.elapsed() > timeout + Duration::from_secs(2) {
                stop_child(&mut child);
                crate::note(&format!(
                    "scraper      curl failed: application watchdog expired after {}s",
                    started.elapsed().as_secs()
                ));
                return Err(timeout_error());
            }
            if response_file.len().is_some_and(|size| size > limit) {
                stop_child(&mut child);
                return Err(response_too_large(limit));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(CURL_POLL_INTERVAL),
                Err(error) => {
                    stop_child(&mut child);
                    return Err(logged_transport_error(&format!(
                        "curl process monitoring failed: {error}"
                    )));
                }
            }
        };
        let output = child.wait_with_output().map_err(|error| {
            logged_transport_error(&format!("curl process collection failed: {error}"))
        })?;
        if !status.success() {
            let code = status.code();
            let ca_bundle = self
                .ca_bundle
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "curl default".to_string());
            crate::note(&format!(
                "scraper      curl failed: exit={}, detail={}, ca={ca_bundle}",
                code.map_or_else(|| "signal".to_string(), |code| code.to_string()),
                curl_exit_diagnostic(code),
            ));
            return Err(curl_exit_error(code, response_file.len(), limit));
        }
        let summary = std::str::from_utf8(&output.stdout).map_err(|error| {
            logged_transport_error(&format!("curl summary was not UTF-8: {error}"))
        })?;
        let mut fields = summary.lines();
        let status = fields
            .next()
            .and_then(|value| value.trim().parse::<u16>().ok())
            .filter(|value| *value != 0)
            .ok_or_else(|| logged_transport_error("curl summary did not contain an HTTP status"))?;
        let content_type = fields
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let body = response_file.read(limit)?;
        Ok(HttpResponse {
            status,
            content_type,
            body,
        })
    }
}

impl Transport for CurlTransport {
    fn get(&self, endpoint: &str, params: &[(String, String)], limit: u64) -> Result<HttpResponse> {
        if !matches!(
            endpoint,
            "ssuserInfos.php" | "jeuInfos.php" | "jeuRecherche.php"
        ) {
            return Err(Error::new(
                ErrorKind::Configuration,
                "unknown ScreenScraper API operation",
            ));
        }
        self.fetch(&api_url(endpoint, params), limit, None)
    }

    fn get_media(
        &self,
        url: &str,
        limit: u64,
        max_kib_per_second: Option<u64>,
    ) -> Result<HttpResponse> {
        validate_media_url(url)?;
        // Redirects are deliberately not followed. The address has been
        // restricted to ScreenScraper before curl sees it, and a redirect
        // must not widen that network boundary behind the user's back.
        self.fetch(url, limit, max_kib_per_second)
    }
}

fn api_url(endpoint: &str, params: &[(String, String)]) -> String {
    let mut url = format!("{API_BASE}/{endpoint}");
    for (index, (key, value)) in params.iter().enumerate() {
        url.push(if index == 0 { '?' } else { '&' });
        percent_encode(&mut url, key.as_bytes());
        url.push('=');
        percent_encode(&mut url, value.as_bytes());
    }
    url
}

fn percent_encode(output: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(*byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[(byte >> 4) as usize]));
            output.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
}

fn curl_config(
    url: &str,
    output: &Path,
    limit: u64,
    max_kib_per_second: Option<u64>,
    ca_bundle: Option<&Path>,
) -> String {
    let connect_timeout = CONNECT_TIMEOUT.as_secs();
    let global_timeout = request_timeout(limit, max_kib_per_second).as_secs();
    let rate = max_kib_per_second
        .filter(|speed| *speed > 0)
        .map(|speed| format!("limit-rate = \"{speed}K\"\n"))
        .unwrap_or_default();
    let ca_bundle = ca_bundle
        .map(|path| format!("cacert = \"{}\"\n", curl_quote(&path.to_string_lossy())))
        .unwrap_or_default();
    format!(
        "silent\nshow-error\ncompressed\nproto = \"=https\"\n{ca_bundle}connect-timeout = \"{connect_timeout}\"\nmax-time = \"{global_timeout}\"\nmax-filesize = \"{limit}\"\n{rate}user-agent = \"Degauss/{}\"\noutput = \"{}\"\nwrite-out = \"%{{http_code}}\\n%{{content_type}}\"\nurl = \"{}\"\n",
        env!("CARGO_PKG_VERSION"),
        curl_quote(&output.to_string_lossy()),
        curl_quote(url),
    )
}

fn system_ca_bundle() -> Option<PathBuf> {
    first_readable_ca_bundle(CA_BUNDLE_CANDIDATES.iter().map(Path::new))
}

fn first_readable_ca_bundle<'a>(candidates: impl IntoIterator<Item = &'a Path>) -> Option<PathBuf> {
    candidates.into_iter().find_map(|path| {
        OpenOptions::new()
            .read(true)
            .open(path)
            .ok()
            .map(|_| path.to_path_buf())
    })
}

fn request_timeout(limit: u64, max_kib_per_second: Option<u64>) -> Duration {
    let Some(speed) = max_kib_per_second.filter(|speed| *speed > 0) else {
        return GLOBAL_TIMEOUT;
    };
    let bytes_per_second = speed.saturating_mul(1024).max(1);
    let transfer_seconds = limit.div_ceil(bytes_per_second);
    Duration::from_secs(transfer_seconds)
        .saturating_add(MEDIA_TIMEOUT_MARGIN)
        .clamp(GLOBAL_TIMEOUT, MAX_MEDIA_TIMEOUT)
}

fn curl_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted
}

fn curl_spawn_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        crate::note("scraper      curl failed: executable was not found");
        Error::new(
            ErrorKind::Configuration,
            "Degauss cannot contact ScreenScraper because MiSTer's curl command is missing.",
        )
    } else {
        logged_transport_error(&format!("curl could not start: {error}"))
    }
}

fn logged_transport_error(diagnostic: &str) -> Error {
    crate::note(&format!("scraper      curl failed: {diagnostic}"));
    Error::new(
        ErrorKind::Transport,
        "The ScreenScraper request failed. Try again.",
    )
}

fn timeout_error() -> Error {
    Error::new(
        ErrorKind::Timeout,
        "ScreenScraper did not respond in time. Try again.",
    )
}

fn curl_exit_error(code: Option<i32>, size: Option<u64>, limit: u64) -> Error {
    if code == Some(63) || size.is_some_and(|size| size > limit) {
        return response_too_large(limit);
    }
    match code {
        Some(5..=7) => Error::new(
            ErrorKind::Transport,
            "Could not reach ScreenScraper. Check the network connection.",
        ),
        Some(28) => timeout_error(),
        Some(35 | 51 | 60) => Error::new(
            ErrorKind::Transport,
            "Secure connection to ScreenScraper failed. Check MiSTer's date and network.",
        ),
        Some(52 | 55 | 56) => Error::new(
            ErrorKind::Transport,
            "The ScreenScraper connection was interrupted. Try again.",
        ),
        Some(58 | 77) => Error::new(
            ErrorKind::Configuration,
            "Degauss could not use MiSTer's security certificates.",
        ),
        Some(_) | None => Error::new(
            ErrorKind::Transport,
            "The ScreenScraper request failed. Try again.",
        ),
    }
}

fn curl_exit_diagnostic(code: Option<i32>) -> &'static str {
    match code {
        Some(5) => "proxy name could not be resolved",
        Some(6) => "ScreenScraper host name could not be resolved",
        Some(7) => "connection failed",
        Some(28) => "request timed out",
        Some(35) => "TLS handshake failed",
        Some(51) => "peer certificate or fingerprint was rejected",
        Some(52) => "server returned no data",
        Some(55) => "request send failed",
        Some(56) => "response receive failed",
        Some(58) => "local client certificate could not be used",
        Some(60) => "peer certificate could not be authenticated",
        Some(63) => "response exceeded the configured size limit",
        Some(77) => "CA certificate bundle could not be read",
        Some(_) => "unclassified curl failure",
        None => "curl was terminated by a signal",
    }
}

fn response_too_large(limit: u64) -> Error {
    Error::new(
        ErrorKind::MalformedResponse,
        format!("response exceeded the {limit}-byte limit"),
    )
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct ResponseFile {
    path: PathBuf,
}

impl ResponseFile {
    fn create() -> Result<Self> {
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                ".degauss-screenscraper-{}-{sequence}.part",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    drop(file);
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err(Error::local(
                        "could not create a temporary ScreenScraper response file",
                    ))
                }
            }
        }
        Err(Error::local(
            "could not allocate a temporary ScreenScraper response file",
        ))
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn len(&self) -> Option<u64> {
        std::fs::metadata(&self.path)
            .ok()
            .map(|metadata| metadata.len())
    }

    fn read(&self, limit: u64) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|error| {
                logged_transport_error(&format!("response file could not be opened: {error}"))
            })?
            .take(limit.saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|error| {
                logged_transport_error(&format!("response file could not be read: {error}"))
            })?;
        if body.len() as u64 > limit {
            return Err(response_too_large(limit));
        }
        Ok(body)
    }
}

impl Drop for ResponseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn validate_media_url(url: &str) -> Result<()> {
    if url.chars().any(char::is_control) {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned an invalid media address",
        ));
    }
    let rest = url.strip_prefix("https://").ok_or_else(|| {
        Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned a non-HTTPS media address",
        )
    })?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains(char::is_whitespace)
        || authority.contains('#')
    {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned an invalid media address",
        ));
    }
    let (host, port) = authority
        .rsplit_once(':')
        .map(|(host, port)| (host, Some(port)))
        .unwrap_or((authority, None));
    if port.is_some_and(|port| port != "443") {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned media on an unexpected port",
        ));
    }
    let host = host.to_ascii_lowercase();
    if host != "screenscraper.fr" && !host.ends_with(".screenscraper.fr") {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned media from an unexpected host",
        ));
    }
    Ok(())
}

pub struct Client {
    transport: Arc<dyn Transport>,
    developer: DeveloperCredentials,
    username: String,
    password: String,
    region: Option<String>,
    language: Option<String>,
    media_type: String,
}

impl Client {
    pub fn new(
        transport: Arc<dyn Transport>,
        developer: DeveloperCredentials,
        settings: &ScraperSettings,
    ) -> Result<Self> {
        settings.validate()?;
        if settings.username.is_empty() || settings.password.is_empty() {
            return Err(Error::new(
                ErrorKind::Authentication,
                "ScreenScraper username and password are required",
            ));
        }
        Ok(Self {
            transport,
            developer,
            username: settings.username.clone(),
            password: settings.password.clone(),
            region: settings
                .region
                .as_ref()
                .map(|value| value.to_ascii_lowercase()),
            language: settings
                .language
                .as_ref()
                .map(|value| value.to_ascii_lowercase()),
            media_type: settings.media_type.to_ascii_lowercase(),
        })
    }

    fn auth(&self) -> Vec<(String, String)> {
        vec![
            ("devid".into(), self.developer.developer_id.clone()),
            (
                "devpassword".into(),
                self.developer.developer_password.clone(),
            ),
            (
                "softname".into(),
                format!("Degauss-{}", env!("CARGO_PKG_VERSION")),
            ),
            ("output".into(), "xml".into()),
            ("ssid".into(), self.username.clone()),
            ("sspassword".into(), self.password.clone()),
        ]
    }

    pub fn account(&self) -> Result<Account> {
        let response = self
            .transport
            .get("ssuserInfos.php", &self.auth(), MAX_XML_BYTES)?;
        response_status(&response, Answer::Account)?;
        content_is_xml(&response, Answer::Account)?;
        parse(
            &response.body,
            self.region.as_deref(),
            self.language.as_deref(),
            &self.media_type,
        )
        .map(|parsed| parsed.account)
        .and_then(|account| {
            account.ok_or_else(|| {
                Error::new(
                    ErrorKind::MalformedResponse,
                    "account response did not contain account information",
                )
            })
        })
        .and_then(|account| match account.level {
            Some(0) => Err(Error::new(
                ErrorKind::Authentication,
                "ScreenScraper rejected the login. Check the username and password",
            )),
            None => Err(Error::new(
                ErrorKind::MalformedResponse,
                "account response omitted the account level",
            )),
            Some(_) => require_account_limits(account),
        })
    }

    #[cfg(test)]
    pub fn by_hash(
        &self,
        system_id: u32,
        file_name: &str,
        hashes: &Hashes,
    ) -> Result<LookupResponse> {
        self.by_hash_with_media_type(system_id, file_name, hashes, &self.media_type)
    }

    /// Select one target's artwork without rebuilding the shared client or
    /// changing the media choice seen by other concurrent workers.
    pub fn by_hash_with_media_type(
        &self,
        system_id: u32,
        file_name: &str,
        hashes: &Hashes,
        media_type: &str,
    ) -> Result<LookupResponse> {
        validate_media_type(media_type)?;
        let mut params = self.auth();
        params.extend([
            ("systemeid".into(), system_id.to_string()),
            ("romnom".into(), file_name.to_string()),
            ("romtype".into(), "rom".into()),
            ("romtaille".into(), hashes.size.to_string()),
            ("crc".into(), hashes.crc32.clone()),
            ("md5".into(), hashes.md5.clone()),
            ("sha1".into(), hashes.sha1.clone()),
        ]);
        let response = self.transport.get("jeuInfos.php", &params, MAX_XML_BYTES)?;
        match response_status(&response, Answer::Lookup) {
            Err(error) if error.kind == ErrorKind::NotFound => {
                return Ok(LookupResponse {
                    lookup: Lookup::NotFound,
                    alternatives: Vec::new(),
                    account: None,
                    server_miss: true,
                })
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        if let Err(error) = content_is_xml(&response, Answer::Lookup) {
            if error.kind == ErrorKind::NotFound {
                return Ok(LookupResponse {
                    lookup: Lookup::NotFound,
                    alternatives: Vec::new(),
                    account: None,
                    server_miss: true,
                });
            }
            return Err(error);
        }
        let parsed = parse(
            &response.body,
            self.region.as_deref(),
            self.language.as_deref(),
            media_type,
        )?;
        if !parsed.saw_game && !parsed.saw_games_container {
            return Err(Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper's game response did not contain a game list",
            ));
        }
        let server_miss = !parsed.saw_game;
        let account = parsed.account;
        let games = parsed.games;
        let response_has_hashes = games.iter().any(has_rom_hash);
        let mut candidates: Vec<Match> = games
            .iter()
            .filter(|candidate| matching_rom_hashes(candidate, hashes))
            .cloned()
            .collect();
        if candidates.is_empty() && !response_has_hashes {
            // `jeuInfos` is the exact hash endpoint. Some replies omit the
            // ROM node even though that endpoint found one game.
            candidates = games;
        }
        let alternatives = unique_candidates(candidates.clone());
        Ok(LookupResponse {
            lookup: unique(candidates),
            alternatives,
            account,
            server_miss,
        })
    }

    /// Return every game ScreenScraper offers for an editable manual query.
    /// Automatic matching uses [`Self::by_name_with_media_type`] below and still accepts only
    /// an exact normalised title; this broader result is for a person to
    /// inspect and choose from explicitly.
    pub fn search(&self, system_id: u32, title: &str) -> Result<LookupResponse> {
        self.search_with_media_type(system_id, title, &self.media_type)
    }

    pub fn search_with_media_type(
        &self,
        system_id: u32,
        title: &str,
        media_type: &str,
    ) -> Result<LookupResponse> {
        validate_media_type(media_type)?;
        if title.trim().is_empty() {
            return Err(Error::new(
                ErrorKind::Configuration,
                "enter a game title to search for",
            ));
        }
        let mut params = self.auth();
        params.extend([
            ("systemeid".into(), system_id.to_string()),
            ("recherche".into(), title.to_string()),
        ]);
        let response = self
            .transport
            .get("jeuRecherche.php", &params, MAX_XML_BYTES)?;
        match response_status(&response, Answer::Lookup) {
            Err(error) if error.kind == ErrorKind::NotFound => {
                return Ok(LookupResponse {
                    lookup: Lookup::NotFound,
                    alternatives: Vec::new(),
                    account: None,
                    server_miss: true,
                })
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        if let Err(error) = content_is_xml(&response, Answer::Lookup) {
            if error.kind == ErrorKind::NotFound {
                return Ok(LookupResponse {
                    lookup: Lookup::NotFound,
                    alternatives: Vec::new(),
                    account: None,
                    server_miss: true,
                });
            }
            return Err(error);
        }
        let parsed = parse(
            &response.body,
            self.region.as_deref(),
            self.language.as_deref(),
            media_type,
        )?;
        if !parsed.saw_game && !parsed.saw_games_container {
            return Err(Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper's search response did not contain a game list",
            ));
        }
        let account = parsed.account;
        let server_miss = !parsed.saw_game;
        let alternatives = unique_candidates(parsed.games);
        Ok(LookupResponse {
            lookup: unique(alternatives.clone()),
            alternatives,
            account,
            server_miss,
        })
    }

    #[cfg(test)]
    pub fn by_name(&self, system_id: u32, title: &str) -> Result<LookupResponse> {
        self.by_name_with_media_type(system_id, title, &self.media_type)
    }

    pub fn by_name_with_media_type(
        &self,
        system_id: u32,
        title: &str,
        media_type: &str,
    ) -> Result<LookupResponse> {
        let response = self.search_with_media_type(system_id, title, media_type)?;
        let candidates: Vec<Match> = response
            .alternatives
            .iter()
            .filter(|candidate| {
                candidate
                    .names
                    .iter()
                    .any(|name| title_matches(name, title))
            })
            .cloned()
            .collect();
        Ok(LookupResponse {
            lookup: unique(candidates),
            alternatives: response.alternatives,
            account: response.account,
            server_miss: response.server_miss,
        })
    }

    pub fn media(&self, media: &Media, limit: u64) -> Result<HttpResponse> {
        let response = self.transport.get_media(&media.url, limit, None)?;
        response_status(&response, Answer::Media)?;
        if std::str::from_utf8(&response.body)
            .ok()
            .is_some_and(|text| text.trim().eq_ignore_ascii_case("NOMEDIA"))
        {
            return Err(Error::new(
                ErrorKind::NotFound,
                "ScreenScraper has no selected image for this game",
            ));
        }
        Ok(response)
    }
}

fn require_account_limits(account: Account) -> Result<Account> {
    if account.max_threads.is_none()
        || account.max_download_speed.is_none()
        || account.requests_today.is_none()
        || account.failed_today.is_none()
        || account.max_requests_per_minute.is_none()
        || account.max_requests_per_day.is_none()
        || account.max_failed_per_day.is_none()
    {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "account response omitted one or more quota limits",
        ));
    }
    Ok(account)
}

fn has_rom_hash(candidate: &Match) -> bool {
    candidate.rom_crc32.is_some() || candidate.rom_md5.is_some() || candidate.rom_sha1.is_some()
}

fn matching_rom_hashes(candidate: &Match, hashes: &Hashes) -> bool {
    let values = [
        (candidate.rom_crc32.as_deref(), hashes.crc32.as_str()),
        (candidate.rom_md5.as_deref(), hashes.md5.as_str()),
        (candidate.rom_sha1.as_deref(), hashes.sha1.as_str()),
    ];
    let mut compared = false;
    for (remote, local) in values {
        if let Some(remote) = remote {
            compared = true;
            if !remote.eq_ignore_ascii_case(local) {
                return false;
            }
        }
    }
    compared
}

fn unique_candidates(mut candidates: Vec<Match>) -> Vec<Match> {
    // ScreenScraper returns search results in relevance order. Keep the
    // first occurrence of each id instead of sorting by an opaque numeric
    // id, otherwise the manual picker would discard that useful ordering.
    let mut seen = HashSet::new();
    candidates.retain(|candidate| {
        !is_non_game_name(&candidate.name) && seen.insert(candidate.id.clone())
    });
    candidates
}

fn unique(candidates: Vec<Match>) -> Lookup {
    let mut candidates = unique_candidates(candidates);
    match candidates.len() {
        0 => Lookup::NotFound,
        1 => Lookup::Found(Box::new(candidates.remove(0))),
        count => Lookup::Ambiguous(count),
    }
}

fn match_key(value: &str) -> String {
    let mut depth = 0usize;
    value
        .chars()
        .filter(|character| match character {
            '(' | '[' => {
                depth += 1;
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0 && character.is_alphanumeric(),
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn title_matches(candidate: &str, wanted: &str) -> bool {
    if match_key(candidate) == match_key(wanted) {
        return true;
    }
    let candidate_keys = crate::gamelist::slug_candidates(candidate);
    let wanted_keys = crate::gamelist::slug_candidates(wanted);
    candidate_keys
        .iter()
        .any(|candidate| wanted_keys.contains(candidate))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Found(Box<Match>),
    NotFound,
    Ambiguous(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupResponse {
    pub lookup: Lookup,
    /// All distinct game candidates returned for this request. Automatic
    /// matching ignores inexact alternatives; a one-game interactive scrape
    /// can present them without repeating the same API request.
    pub alternatives: Vec<Match>,
    pub account: Option<Account>,
    /// True only when ScreenScraper itself reported that the request found
    /// no game. Local safety rejection of a conflicting or inexact result
    /// must not consume Degauss's estimate of the server-side KO quota.
    pub server_miss: bool,
}

/// What a response answers, which decides whose problem a rejection is:
/// a lookup's concerns the one game, the account check runs before any
/// game, and a media transfer answers with an image rather than text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// `jeuInfos.php` or `jeuRecherche.php` for one game.
    Lookup,
    /// `ssuserInfos.php` before the run.
    Account,
    /// An image download for a matched game.
    Media,
}

/// The meaning of an HTTP status other than 400, which `rejected_request`
/// classifies from the body.
fn status(code: u16) -> Result<()> {
    match code {
        200..=299 => Ok(()),
        401 | 423 => Err(Error::new(
            ErrorKind::Unavailable,
            format!("ScreenScraper is temporarily closed (HTTP {code})"),
        )),
        403 => Err(Error::new(
            ErrorKind::Authentication,
            "this Degauss build could not authenticate with ScreenScraper (HTTP 403)",
        )),
        404 => Err(Error::new(
            ErrorKind::NotFound,
            "no matching game (HTTP 404)",
        )),
        426 => Err(Error::new(
            ErrorKind::Configuration,
            "this Degauss scraper client was refused by ScreenScraper (HTTP 426)",
        )),
        429 => Err(Error::new(
            ErrorKind::RateLimited,
            "ScreenScraper's concurrent or per-minute limit was reached (HTTP 429)",
        )),
        430 => Err(Error::new(
            ErrorKind::DailyQuota,
            "the daily ScreenScraper request allowance is exhausted (HTTP 430)",
        )),
        431 => Err(Error::new(
            ErrorKind::FailedQuota,
            "the daily failed-search allowance is exhausted (HTTP 431)",
        )),
        500..=599 => Err(Error::new(
            ErrorKind::Server,
            format!("ScreenScraper is temporarily unavailable (HTTP {code}). Try again later."),
        )),
        _ => Err(Error::new(
            ErrorKind::Transport,
            format!("ScreenScraper returned unexpected HTTP {code}. Try again."),
        )),
    }
}

/// Interpret a body-level ScreenScraper error before losing its more precise
/// login diagnosis to a generic HTTP status. In particular, the service can
/// use HTTP 403 for either developer credentials or an end-user login and
/// states which pair failed only in its plain-text response. A documented
/// error text keeps its classification under HTTP 400 as well; any other
/// body under that status is classified by `rejected_request`.
fn response_status(response: &HttpResponse, answer: Answer) -> Result<()> {
    let text = std::str::from_utf8(&response.body)
        .ok()
        .map(|text| text.trim_start_matches('\u{feff}').trim_start());
    if let Some(trimmed) = text {
        let lower = trimmed.to_lowercase();
        if lower.starts_with("erreur") {
            let documented = documented_error(&lower, trimmed);
            let documented_under_400 = response.status == 400 && documented.is_some();
            let body_error = documented.unwrap_or_else(|| unrecognised_error(trimmed, answer));
            if response.status < 300
                || body_error.kind == ErrorKind::Authentication
                || documented_under_400
            {
                crate::note(match authentication_source(trimmed) {
                    Some(AuthenticationSource::Application) => {
                        "scraper      ScreenScraper rejected application authentication"
                    }
                    Some(AuthenticationSource::Account) => {
                        "scraper      ScreenScraper rejected account authentication"
                    }
                    None => "scraper      ScreenScraper returned a body-level error",
                });
                return Err(body_error);
            }
        }
    }
    match response.status {
        400 => Err(rejected_request(response, text, answer)),
        code => status(code),
    }
}

/// HTTP 400 is one game's rejection on a lookup or a media transfer and a
/// setup fault at the account check. The XML endpoints answer it with text,
/// so markup, a page content type, an empty or non-text body stops the run.
fn rejected_request(response: &HttpResponse, text: Option<&str>, answer: Answer) -> Error {
    let text = text.map(str::trim).filter(|text| !text.is_empty());
    let kind = match answer {
        Answer::Account => ErrorKind::Configuration,
        Answer::Lookup | Answer::Media => ErrorKind::InvalidRequest,
    };
    if answer != Answer::Media {
        if let Some(content_type) = page_content_type(response) {
            return not_an_answer(&format!(
                "content type {} under HTTP 400",
                excerpt(content_type)
            ));
        }
        if text.is_none() {
            return not_an_answer("no error text under HTTP 400");
        }
        if text.is_some_and(|text| text.starts_with('<')) {
            return not_an_answer("markup instead of an error text under HTTP 400");
        }
    }
    let rejected = "ScreenScraper rejected the request parameters (HTTP 400)";
    Error::new(
        kind,
        match text {
            Some(text) => format!("{rejected}: {}", excerpt(text)),
            None => rejected.into(),
        },
    )
}

/// The content type of a body that is neither XML nor plain text: a
/// maintenance or intermediary page rather than a ScreenScraper answer.
fn page_content_type(response: &HttpResponse) -> Option<&str> {
    response.content_type.as_deref().filter(|value| {
        let value = value.to_ascii_lowercase();
        !value.contains("xml") && !value.starts_with("text/plain")
    })
}

fn content_is_xml(response: &HttpResponse, answer: Answer) -> Result<()> {
    // ScreenScraper answers with XML or a plain-text error. Any other
    // content type (a maintenance or intermediary page) is not an answer
    // to the one request, so it counts as the service being unavailable
    // rather than as one game's unreadable match. So does a body that is
    // not text at all: no `<Data>` answer can be read from it.
    if let Some(content_type) = page_content_type(response) {
        return Err(Error::new(
            ErrorKind::Unavailable,
            format!(
                "ScreenScraper returned a non-XML response (content type {})",
                excerpt(content_type)
            ),
        ));
    }
    let text = std::str::from_utf8(&response.body).map_err(|_| not_an_answer("non-UTF-8 body"))?;
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if !trimmed.starts_with('<') {
        return Err(text_error(trimmed, answer));
    }
    Ok(())
}

fn text_error(text: &str, answer: Answer) -> Error {
    if text.trim().is_empty() {
        return Error::new(
            ErrorKind::Unavailable,
            "ScreenScraper returned an empty response",
        );
    }
    let lower = text.to_lowercase();
    if lower.starts_with("erreur") {
        return documented_error(&lower, text).unwrap_or_else(|| unrecognised_error(text, answer));
    }
    // Neither XML nor an error text is not an answer to the one request
    // (a maintenance notice), so the run stops.
    Error::new(
        ErrorKind::Unavailable,
        format!(
            "ScreenScraper returned a response that was not XML: {}",
            excerpt(text)
        ),
    )
}

/// The classification of an error text ScreenScraper documents; `lower`
/// is `text` lowercased. None for a text the service has not documented.
fn documented_error(lower: &str, text: &str) -> Option<Error> {
    if lower.contains("développeur") || lower.contains("developpeur") {
        return Some(Error::new(
            ErrorKind::Authentication,
            "this Degauss build could not authenticate with ScreenScraper",
        ));
    }
    if lower.contains("utilisateur") {
        return Some(Error::new(
            ErrorKind::Authentication,
            "ScreenScraper rejected the login. Check the username and password",
        ));
    }
    if lower.contains("quota") {
        return Some(Error::new(
            ErrorKind::DailyQuota,
            "the ScreenScraper allowance is exhausted",
        ));
    }
    if lower.contains("introuv") || lower.contains("non trouv") {
        return Some(Error::new(ErrorKind::NotFound, "no matching game"));
    }
    // The documented closure texts ("API fermé pour les non membres",
    // "API totalement fermé") describe the service, not the request.
    if lower.contains("ferm") {
        return Some(Error::new(
            ErrorKind::Unavailable,
            "ScreenScraper reported that the API is closed",
        ));
    }
    // The documented HTTP 429 texts (every one names "threads") and the
    // HTTP 431 text describe the account's limits, not the request. Their
    // HTTP statuses carry the retryable and allowance kinds; the text on
    // its own stops the run like any other outage, wherever it arrives.
    if lower.contains("threads") {
        return Some(Error::new(
            ErrorKind::Unavailable,
            "ScreenScraper reported that its thread limit is reached",
        ));
    }
    if lower.contains("faite du tri") || lower.contains("repassez demain") {
        return Some(Error::new(
            ErrorKind::Unavailable,
            "the ScreenScraper failed-search allowance is exhausted",
        ));
    }
    // The blacklist text is the one HTTP 426 carries: the same kind, so a
    // refused client stops the run wherever the text arrives.
    if lower.contains("blacklist") {
        return Some(Error::new(
            ErrorKind::Configuration,
            "this Degauss scraper client was refused by ScreenScraper",
        ));
    }
    // Degauss sends the same request fields for every game, so the
    // documented texts for a call missing its fields concern the run.
    if lower.contains("l'url") {
        return Some(Error::new(
            ErrorKind::Configuration,
            format!(
                "ScreenScraper rejected the request address, which is the same for every game: {}",
                excerpt(text)
            ),
        ));
    }
    // The documented HTTP 400 texts for a rom name carrying a path or not
    // conforming and for a malformed hash field concern this one request:
    // the run continues with the next game.
    if lower.contains("fichier rom") || lower.contains("crc, md5 ou sha1") {
        return Some(Error::new(
            ErrorKind::InvalidRequest,
            format!("ScreenScraper rejected this request: {}", excerpt(text)),
        ));
    }
    None
}

/// An error text ScreenScraper has not documented: one game's rejection
/// on a lookup, an outage at the account check or on a media transfer.
fn unrecognised_error(text: &str, answer: Answer) -> Error {
    match answer {
        Answer::Lookup => Error::new(
            ErrorKind::InvalidRequest,
            format!(
                "ScreenScraper rejected this request (unrecognised error text): {}",
                excerpt(text)
            ),
        ),
        Answer::Account | Answer::Media => Error::new(
            ErrorKind::Unavailable,
            format!(
                "ScreenScraper returned an unrecognised error response: {}",
                excerpt(text)
            ),
        ),
    }
}

/// One bounded line of a server text for the log. The documented error
/// texts are short sentences; a longer body is cut so a page cannot
/// flood the log, and the cut line is redacted so a page that echoes the
/// request address cannot put the credentials from its query in the log.
fn excerpt(text: &str) -> String {
    const LIMIT: usize = 120;
    let mut chars = text
        .split_whitespace()
        .flat_map(|word| std::iter::once(' ').chain(word.chars()))
        .skip(1);
    let line = redact_credentials(&chars.by_ref().take(LIMIT).collect::<String>());
    if chars.next().is_some() {
        format!("{line}...")
    } else {
        line
    }
}

/// Replace the value of every credential query parameter Degauss sends
/// (`devid`, `devpassword`, `ssid`, `sspassword`) with "[redacted]". The
/// value runs to the next `&`, whitespace or markup delimiter, or to the
/// end of the line, so a value the cut left half in place is blanked too.
fn redact_credentials(line: &str) -> String {
    const KEYS: [&str; 4] = ["devid", "devpassword", "ssid", "sspassword"];
    let lower = line.to_ascii_lowercase();
    let mut redacted = String::with_capacity(line.len());
    let mut index = 0;
    while let Some(next) = line[index..].chars().next() {
        let key = KEYS.iter().copied().find(|key| {
            let at_boundary = index == 0 || !lower.as_bytes()[index - 1].is_ascii_alphanumeric();
            at_boundary
                && lower[index..].starts_with(key)
                && lower[index + key.len()..].starts_with('=')
        });
        let Some(key) = key else {
            redacted.push(next);
            index += next.len_utf8();
            continue;
        };
        let value_start = index + key.len() + 1;
        let value_end = line[value_start..]
            .find(|character: char| {
                character == '&' || character.is_whitespace() || "\"'<>".contains(character)
            })
            .map_or(line.len(), |offset| value_start + offset);
        redacted.push_str(&line[index..value_start]);
        redacted.push_str("[redacted]");
        index = value_end;
    }
    redacted
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthenticationSource {
    Application,
    Account,
}

fn authentication_source(text: &str) -> Option<AuthenticationSource> {
    let lower = text.to_lowercase();
    if lower.contains("développeur") || lower.contains("developpeur") {
        Some(AuthenticationSource::Application)
    } else if lower.contains("utilisateur") {
        Some(AuthenticationSource::Account)
    } else {
        None
    }
}

#[derive(Debug, Default)]
struct Parsed {
    account: Option<Account>,
    games: Vec<Match>,
    saw_game: bool,
    saw_games_container: bool,
}

#[derive(Default)]
struct RawGame {
    id: String,
    not_game: bool,
    names: Vec<(String, String)>,
    descriptions: Vec<(String, String)>,
    dates: Vec<(String, String)>,
    genres: Vec<(String, String)>,
    developer: Option<String>,
    publisher: Option<String>,
    players: Option<String>,
    rom_descriptions: Vec<(String, String)>,
    rom_dates: Vec<(String, String)>,
    rom_developer: Option<String>,
    rom_publisher: Option<String>,
    rom_players: Option<String>,
    languages: Option<String>,
    media: Vec<RawMedia>,
    rom_crc32: Option<String>,
    rom_md5: Option<String>,
    rom_sha1: Option<String>,
}

struct RawMedia {
    kind: String,
    region: String,
    format: Option<String>,
    url: String,
}

struct Capture {
    tag: String,
    attribute: Option<String>,
    text: String,
    rom_override: bool,
}

fn parse(
    bytes: &[u8],
    region: Option<&str>,
    language: Option<&str>,
    media_type: &str,
) -> Result<Parsed> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned non-UTF-8 XML",
        )
    })?;
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<String> = Vec::new();
    let mut parsed = Parsed::default();
    let mut account = Account::default();
    let mut saw_account = false;
    let mut game: Option<RawGame> = None;
    let mut capture: Option<Capture> = None;
    // Every ScreenScraper answer is a `<Data>` document. A body with any
    // other root (a maintenance, proxy or challenge page, whatever content
    // type it was served with) is not an answer to the one request, so it
    // is the service being unavailable; only a `<Data>` answer whose
    // content is unusable is a malformed response for that request.
    let mut saw_data_root = false;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Err(_) if !saw_data_root => return Err(not_an_answer("invalid XML")),
            Err(_) => {
                return Err(Error::new(
                    ErrorKind::MalformedResponse,
                    format!(
                        "ScreenScraper returned invalid XML at byte {}",
                        reader.buffer_position()
                    ),
                ))
            }
            Ok(Event::Start(element)) => {
                let tag = element.name().as_ref().to_ascii_lowercase();
                if !saw_data_root {
                    require_data_root(&tag)?;
                    saw_data_root = true;
                }
                let attributes = xml_attributes(&element)?;
                let parent = stack.last().map(String::as_str);
                let direct_game_rom = parent == Some("rom")
                    && stack.iter().rev().nth(1).map(String::as_str) == Some("jeu");
                let inside_direct_game_rom = stack
                    .windows(2)
                    .any(|pair| pair[0] == "jeu" && pair[1] == "rom");
                if tag == "jeux" {
                    parsed.saw_games_container = true;
                }
                if tag == "jeu" {
                    parsed.saw_game = true;
                    let mut raw = RawGame::default();
                    if let Some(id) = attribute_value(&attributes, "id") {
                        raw.id = id.to_string();
                    }
                    if let Some(not_game) = attribute_value(&attributes, "notgame") {
                        raw.not_game = parse_not_game(not_game)?;
                    }
                    game = Some(raw);
                }
                let wanted = if parent == Some("ssuser") {
                    matches!(
                        tag.as_str(),
                        "niveau"
                            | "maxthreads"
                            | "maxdownloadspeed"
                            | "requeststoday"
                            | "requestskotoday"
                            | "maxrequestspermin"
                            | "maxrequestsperday"
                            | "maxrequestskoperday"
                            | "favregion"
                    )
                } else if game.is_some() {
                    matches!(
                        (parent, tag.as_str()),
                        (Some("noms"), "nom")
                            | (Some("synopsis"), "synopsis")
                            | (Some("dates"), "date")
                            | (Some("genres"), "genre")
                            | (Some("medias"), "media")
                            | (Some("jeu"), "developpeur")
                            | (Some("jeu"), "editeur")
                            | (Some("jeu"), "joueurs")
                            | (Some("jeu"), "notgame")
                    ) || (direct_game_rom
                        && matches!(
                            tag.as_str(),
                            "romcrc"
                                | "rommd5"
                                | "romsha1"
                                | "romlangues"
                                | "developpeur"
                                | "editeur"
                                | "joueurs"
                        ))
                        || (inside_direct_game_rom
                            && matches!(
                                (parent, tag.as_str()),
                                (Some("synopsis"), "synopsis") | (Some("dates"), "date")
                            ))
                } else {
                    false
                };
                if wanted {
                    let attribute_name = match (parent, tag.as_str()) {
                        (Some("noms" | "dates" | "medias"), _) => "region",
                        (Some("synopsis" | "genres"), _) => "langue",
                        _ => "",
                    };
                    let attribute = (!attribute_name.is_empty()).then(|| {
                        attribute_value(&attributes, attribute_name)
                            .unwrap_or_default()
                            .to_ascii_lowercase()
                    });
                    let tag = if parent == Some("medias") {
                        let kind = attribute_value(&attributes, "type")
                            .map(str::to_ascii_lowercase)
                            .unwrap_or_default();
                        let format = attribute_value(&attributes, "format").map(str::to_string);
                        format!("media\u{0}{kind}\u{0}{}", format.unwrap_or_default())
                    } else {
                        tag
                    };
                    capture = Some(Capture {
                        tag,
                        attribute,
                        text: String::new(),
                        rom_override: inside_direct_game_rom,
                    });
                }
                stack.push(element.name().as_ref().to_ascii_lowercase());
            }
            Ok(Event::Empty(element)) => {
                if !saw_data_root {
                    require_data_root(&element.name().as_ref().to_ascii_lowercase())?;
                    saw_data_root = true;
                }
                xml_attributes(&element)?;
                if element.name().as_ref().eq_ignore_ascii_case("jeux") {
                    parsed.saw_games_container = true;
                }
                if element.name().as_ref().eq_ignore_ascii_case("notgame")
                    && stack.last().is_some_and(|parent| parent == "jeu")
                {
                    return Err(Error::new(
                        ErrorKind::MalformedResponse,
                        "ScreenScraper returned an empty <notgame> value",
                    ));
                }
            }
            Ok(Event::Text(value)) => {
                if !saw_data_root && !value.xml10_content().trim().is_empty() {
                    return Err(not_an_answer("text before the root element"));
                }
                if let Some(capture) = capture.as_mut() {
                    capture.text.push_str(&value.xml10_content());
                }
            }
            Ok(Event::CData(value)) => {
                if !saw_data_root {
                    return Err(not_an_answer("text before the root element"));
                }
                if let Some(capture) = capture.as_mut() {
                    capture.text.push_str(value.as_ref());
                }
            }
            Ok(Event::GeneralRef(value)) => {
                if let Some(capture) = capture.as_mut() {
                    let name = value.into_inner();
                    if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(&name) {
                        capture.text.push_str(resolved);
                    } else if let Some(resolved) = numeric_entity(&name) {
                        capture.text.push(resolved);
                    } else {
                        return Err(Error::new(
                            ErrorKind::MalformedResponse,
                            "ScreenScraper XML contained an unknown entity",
                        ));
                    }
                }
            }
            Ok(Event::End(element)) => {
                let tag = element.name().as_ref().to_ascii_lowercase();
                if capture.as_ref().is_some_and(|capture| {
                    capture.tag.split('\0').next().unwrap_or_default() == tag
                        || (capture.tag.starts_with("media\0") && tag == "media")
                }) {
                    if let Some(done) = capture.take() {
                        apply_capture(&mut account, &mut saw_account, game.as_mut(), done)?;
                    }
                }
                if tag == "jeu" {
                    if let Some(game) = game.take() {
                        if let Some(game) = finish_game(game, region, language, media_type)? {
                            parsed.games.push(game);
                        }
                    }
                }
                stack.pop();
            }
            _ => {}
        }
    }
    if !saw_data_root {
        return Err(not_an_answer("no root element"));
    }
    if !stack.is_empty() || game.is_some() || capture.is_some() {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned incomplete XML",
        ));
    }
    if saw_account {
        parsed.account = Some(account);
    }
    Ok(parsed)
}

/// Accepts ScreenScraper's `<Data>` root; any other root element is a
/// page the service did not answer with.
fn require_data_root(tag: &str) -> Result<()> {
    if tag == "data" {
        return Ok(());
    }
    Err(not_an_answer(&format!("root element <{}>", excerpt(tag))))
}

fn not_an_answer(what: &str) -> Error {
    Error::new(
        ErrorKind::Unavailable,
        format!("ScreenScraper returned a page instead of an answer ({what})"),
    )
}

fn xml_attributes(element: &BytesStart<'_>) -> Result<Vec<(String, String)>> {
    element
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|_| {
                Error::new(
                    ErrorKind::MalformedResponse,
                    "ScreenScraper XML has a malformed attribute",
                )
            })?;
            let key = attribute.key.as_ref().to_ascii_lowercase();
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|_| {
                    Error::new(
                        ErrorKind::MalformedResponse,
                        "ScreenScraper XML has a malformed attribute value",
                    )
                })?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn attribute_value<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let value = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(value)
}

fn apply_capture(
    account: &mut Account,
    saw_account: &mut bool,
    game: Option<&mut RawGame>,
    capture: Capture,
) -> Result<()> {
    let value = capture.text.trim().to_string();
    let (tag, media_kind, media_format) = if capture.tag.starts_with("media\0") {
        let mut fields = capture.tag.split('\0');
        (
            fields.next().unwrap_or_default(),
            fields.next().unwrap_or_default(),
            fields.next().filter(|value| !value.is_empty()),
        )
    } else {
        (capture.tag.as_str(), "", None)
    };
    if value.is_empty() {
        if tag == "notgame" {
            return Err(Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper returned an empty <notgame> value",
            ));
        }
        return Ok(());
    }
    let number = || {
        value.parse::<u64>().map_err(|_| {
            Error::new(
                ErrorKind::MalformedResponse,
                format!("ScreenScraper returned a non-numeric <{tag}> value"),
            )
        })
    };
    match tag {
        "niveau" => {
            *saw_account = true;
            account.level = Some(u32::try_from(number()?).map_err(|_| {
                Error::new(
                    ErrorKind::MalformedResponse,
                    "ScreenScraper returned an out-of-range <niveau> value",
                )
            })?);
        }
        "maxthreads" => {
            *saw_account = true;
            account.max_threads = Some(usize::try_from(number()?).map_err(|_| {
                Error::new(
                    ErrorKind::MalformedResponse,
                    "ScreenScraper returned an out-of-range <maxthreads> value",
                )
            })?);
        }
        "maxdownloadspeed" => {
            *saw_account = true;
            account.max_download_speed = Some(number()?);
        }
        "requeststoday" => {
            *saw_account = true;
            account.requests_today = Some(number()?);
        }
        "requestskotoday" => {
            *saw_account = true;
            account.failed_today = Some(number()?);
        }
        "maxrequestspermin" => {
            *saw_account = true;
            account.max_requests_per_minute = Some(number()?);
        }
        "maxrequestsperday" => {
            *saw_account = true;
            account.max_requests_per_day = Some(number()?);
        }
        "maxrequestskoperday" => {
            *saw_account = true;
            account.max_failed_per_day = Some(number()?);
        }
        "favregion" => {
            *saw_account = true;
            account.preferred_region = Some(normalise_account_region(&value)?);
        }
        _ => {
            let Some(game) = game else {
                return Ok(());
            };
            let attribute = capture.attribute.unwrap_or_default();
            match tag {
                "nom" => game.names.push((attribute, value)),
                "synopsis" if capture.rom_override => {
                    game.rom_descriptions.push((attribute, value))
                }
                "synopsis" => game.descriptions.push((attribute, value)),
                "date" if capture.rom_override => game.rom_dates.push((attribute, value)),
                "date" => game.dates.push((attribute, value)),
                "genre" => game.genres.push((attribute, value)),
                "developpeur" if capture.rom_override => game.rom_developer = Some(value),
                "developpeur" => game.developer = Some(value),
                "editeur" if capture.rom_override => game.rom_publisher = Some(value),
                "editeur" => game.publisher = Some(value),
                "joueurs" if capture.rom_override => game.rom_players = Some(value),
                "joueurs" => game.players = Some(value),
                "notgame" => game.not_game = parse_not_game(&value)?,
                "romlangues" => game.languages = Some(value),
                "romcrc" => game.rom_crc32 = Some(value),
                "rommd5" => game.rom_md5 = Some(value),
                "romsha1" => game.rom_sha1 = Some(value),
                "media" => game.media.push(RawMedia {
                    kind: media_kind.to_string(),
                    region: attribute,
                    format: media_format.map(str::to_string),
                    url: value.replace(' ', "%20"),
                }),
                _ => {}
            }
        }
    }
    Ok(())
}

fn finish_game(
    game: RawGame,
    region: Option<&str>,
    language: Option<&str>,
    media_type: &str,
) -> Result<Option<Match>> {
    if game.not_game || game.names.iter().any(|(_, name)| is_non_game_name(name)) {
        return Ok(None);
    }
    if game.id.trim().is_empty() {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned a game without an ID",
        ));
    }
    let name = pick(
        &game.names,
        fallbacks(region, &["wor", "us", "ss", "eu", "jp"]),
    )
    .ok_or_else(|| {
        Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned a game without a name",
        )
    })?;
    let description = pick(&game.rom_descriptions, fallbacks(language, &["en", "wor"]))
        .or_else(|| pick(&game.descriptions, fallbacks(language, &["en", "wor"])));
    let date = pick(
        &game.rom_dates,
        fallbacks(region, &["wor", "us", "ss", "eu", "jp"]),
    )
    .or_else(|| {
        pick(
            &game.dates,
            fallbacks(region, &["wor", "us", "ss", "eu", "jp"]),
        )
    })
    .and_then(date_for_gamelist);
    let genre = pick(&game.genres, fallbacks(language, &["en"]));
    let media = pick_media(&game.media, media_type, region);
    let names = game.names.into_iter().map(|(_, name)| name).collect();
    Ok(Some(Match {
        id: game.id,
        name: name.clone(),
        names,
        metadata: Metadata {
            name: Some(name),
            desc: description,
            publisher: game.rom_publisher.or(game.publisher),
            developer: game.rom_developer.or(game.developer),
            releasedate: date,
            players: game.rom_players.or(game.players),
            genre,
            lang: game.languages,
        },
        media,
        rom_crc32: game.rom_crc32,
        rom_md5: game.rom_md5,
        rom_sha1: game.rom_sha1,
    }))
}

fn is_non_game_name(value: &str) -> bool {
    let value = value.trim_start().to_ascii_lowercase();
    value == "zzz(notgame)" || value.starts_with("zzz(notgame):")
}

fn parse_not_game(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned an invalid notgame value",
        )),
    }
}

fn normalise_account_region(value: &str) -> Result<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "france" | "fr" => Ok("fr".into()),
        "europe" | "eu" => Ok("eu".into()),
        "usa" | "us" => Ok("us".into()),
        "japon" | "jp" => Ok("jp".into()),
        _ => Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned an unknown account region",
        )),
    }
}

fn fallbacks<'a>(chosen: Option<&'a str>, rest: &'a [&'a str]) -> Vec<&'a str> {
    chosen
        .into_iter()
        .chain(rest.iter().copied())
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        })
}

fn pick(values: &[(String, String)], priorities: Vec<&str>) -> Option<String> {
    for priority in priorities {
        if let Some((_, value)) = values
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(priority))
        {
            return Some(value.clone());
        }
    }
    values.first().map(|(_, value)| value.clone())
}

fn pick_media(media: &[RawMedia], kind: &str, region: Option<&str>) -> Option<Media> {
    let candidates: Vec<&RawMedia> = media
        .iter()
        .filter(|media| media.kind.eq_ignore_ascii_case(kind))
        .collect();
    let regions = fallbacks(region, &["wor", "us", "eu", "jp", "cus", "ss"]);
    let chosen = regions
        .into_iter()
        .find_map(|region| {
            candidates
                .iter()
                .find(|media| media.region.eq_ignore_ascii_case(region))
                .copied()
        })
        .or_else(|| candidates.first().copied())?;
    Some(Media {
        url: chosen.url.clone(),
        format: chosen.format.clone(),
    })
}

fn date_for_gamelist(value: String) -> Option<String> {
    let value = value.trim();
    if value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(format!("{value}0000T000000"));
    }
    let digits: String = value.chars().filter(char::is_ascii_digit).collect();
    if digits.len() >= 8 {
        Some(format!("{}T000000", &digits[..8]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FIXTURE: &str = r#"<?xml version="1.0"?>
<Data>
  <ssuser>
    <niveau>2</niveau><maxthreads>3</maxthreads><maxdownloadspeed>512</maxdownloadspeed><requeststoday>98</requeststoday>
    <requestskotoday>4</requestskotoday><maxrequestspermin>20</maxrequestspermin>
    <maxrequestsperday>100</maxrequestsperday><maxrequestskoperday>10</maxrequestskoperday>
    <favregion>eu</favregion>
  </ssuser>
  <jeux><jeu id="42">
    <noms><nom region="jp">Nihon</nom><nom region="wor">World Name</nom></noms>
    <synopsis><synopsis langue="fr">Francais</synopsis><synopsis langue="en"><![CDATA[English & clear]]></synopsis></synopsis>
    <dates><date region="wor">1991-02-03</date></dates>
    <developpeur>Studio</developpeur><editeur>Publisher</editeur><joueurs>1-2</joueurs>
    <genres><genre langue="en">Action</genre></genres>
    <rom><romcrc>352441C2</romcrc><rommd5>900150983CD24FB0D6963F7D28E17F72</rommd5><romsha1>A999</romsha1><romlangues>en,fr</romlangues></rom>
    <medias>
      <media type="ss" region="jp" format="jpg">https://media.screenscraper.fr/jp.jpg</media>
      <media type="ss" region="wor" format="png">https://media.screenscraper.fr/world image.png</media>
    </medias>
  </jeu></jeux>
</Data>"#;

    #[test]
    fn account_game_metadata_and_media_are_parsed_with_fallbacks() {
        let parsed = parse(FIXTURE.as_bytes(), Some("us"), Some("de"), "ss").unwrap();
        let account = parsed.account.unwrap();
        assert_eq!(account.max_threads, Some(3));
        assert_eq!(account.max_download_speed, Some(512));
        assert_eq!(account.requests_left(), Some(2));
        assert_eq!(account.failed_left(), Some(6));
        assert_eq!(account.preferred_region.as_deref(), Some("eu"));
        let game = &parsed.games[0];
        assert_eq!(game.id, "42");
        assert_eq!(game.name, "World Name");
        assert_eq!(game.metadata.desc.as_deref(), Some("English & clear"));
        assert_eq!(
            game.metadata.releasedate.as_deref(),
            Some("19910203T000000")
        );
        assert_eq!(game.metadata.lang.as_deref(), Some("en,fr"));
        assert_eq!(
            game.media.as_ref().map(|media| media.url.as_str()),
            Some("https://media.screenscraper.fr/world%20image.png")
        );
    }

    #[test]
    fn matched_rom_metadata_overrides_the_game_without_reading_the_rom_catalogue() {
        let xml = br#"<Data><jeux><jeu id="42">
            <noms><nom region="wor">Game name</nom></noms>
            <synopsis><synopsis langue="en">Game description</synopsis></synopsis>
            <dates><date region="us">1990-01-02</date></dates>
            <developpeur>Game developer</developpeur>
            <editeur>Game publisher</editeur>
            <joueurs>1</joueurs>
            <roms><rom><joueurs>99</joueurs><editeur>Unmatched publisher</editeur></rom></roms>
            <rom>
                <rommd5>900150983CD24FB0D6963F7D28E17F72</rommd5>
                <romlangues>en,fr</romlangues>
                <synopsis><synopsis langue="en">Matched ROM description</synopsis></synopsis>
                <dates><date region="us">1991-03-04</date></dates>
                <developpeur>Matched ROM developer</developpeur>
                <editeur>Matched ROM publisher</editeur>
                <joueurs>2</joueurs>
            </rom>
        </jeu></jeux></Data>"#;

        let parsed = parse(xml, Some("us"), Some("en"), "ss").unwrap();
        let metadata = &parsed.games[0].metadata;
        assert_eq!(metadata.desc.as_deref(), Some("Matched ROM description"));
        assert_eq!(metadata.releasedate.as_deref(), Some("19910304T000000"));
        assert_eq!(metadata.developer.as_deref(), Some("Matched ROM developer"));
        assert_eq!(metadata.publisher.as_deref(), Some("Matched ROM publisher"));
        assert_eq!(metadata.players.as_deref(), Some("2"));
        assert_eq!(metadata.lang.as_deref(), Some("en,fr"));
    }

    #[test]
    fn bad_xml_and_non_utf8_are_not_empty_results() {
        assert_eq!(
            parse(b"<Data><jeu>", None, None, "ss").unwrap_err().kind,
            ErrorKind::MalformedResponse
        );
        assert_eq!(
            parse(&[0xff], None, None, "ss").unwrap_err().kind,
            ErrorKind::MalformedResponse
        );
        assert_eq!(
            parse(
                b"<Data><ssuser><maxthreads>many</maxthreads></ssuser></Data>",
                None,
                None,
                "ss"
            )
            .unwrap_err()
            .kind,
            ErrorKind::MalformedResponse
        );
        for xml in [
            "<Data><jeux><jeu><noms><nom region='wor'>Game</nom></noms></jeu></jeux></Data>",
            "<Data><jeux><jeu id='42'></jeu></jeux></Data>",
        ] {
            assert_eq!(
                parse(xml.as_bytes(), None, None, "ss").unwrap_err().kind,
                ErrorKind::MalformedResponse
            );
        }
    }

    #[test]
    fn every_documented_status_has_a_specific_meaning() {
        // HTTP 400 is classified from its body, so every status goes
        // through `response_status` with a text that is no error text.
        for (code, kind) in [
            (400, ErrorKind::InvalidRequest),
            (401, ErrorKind::Unavailable),
            (403, ErrorKind::Authentication),
            (404, ErrorKind::NotFound),
            (423, ErrorKind::Unavailable),
            (426, ErrorKind::Configuration),
            (429, ErrorKind::RateLimited),
            (430, ErrorKind::DailyQuota),
            (431, ErrorKind::FailedQuota),
            (503, ErrorKind::Server),
        ] {
            let error = response_status(
                &HttpResponse {
                    status: code,
                    content_type: Some("text/plain".into()),
                    body: b"Rejected".to_vec(),
                },
                Answer::Lookup,
            )
            .unwrap_err();
            assert_eq!(error.kind, kind);
            assert!(
                error.detail.contains(&code.to_string()),
                "the local diagnostic must retain HTTP {code}: {}",
                error.detail
            );
        }
        assert!(!status(403).unwrap_err().detail.contains("developer"));
    }

    #[test]
    fn only_https_screenscraper_media_hosts_are_accepted() {
        assert!(validate_media_url("https://media.screenscraper.fr/a.png").is_ok());
        assert!(validate_media_url("https://media.screenscraper.fr:443/a.png").is_ok());
        assert!(validate_media_url("https://screenscraper.fr/a.png").is_ok());
        assert!(validate_media_url("http://media.screenscraper.fr/a.png").is_err());
        assert!(validate_media_url("https://media.screenscraper.fr:8443/a.png").is_err());
        assert!(validate_media_url("https://media.screenscraper.fr:/a.png").is_err());
        assert!(validate_media_url("https://screenscraper.fr.evil.test/a.png").is_err());
        assert!(validate_media_url("https://user@screenscraper.fr/a.png").is_err());
    }

    #[test]
    fn name_matching_removes_dump_tags_and_refuses_ambiguity() {
        assert_eq!(match_key("Game (USA) [!]"), match_key("game"));
        assert!(title_matches(
            "Formula One Grand Prix",
            "Formula 1 Grand Prix (Europe)"
        ));
        assert!(!title_matches(
            "Formula One Grand Prix",
            "Formula 2 Grand Prix"
        ));
        let game = |id: &str| Match {
            id: id.into(),
            name: "Game".into(),
            names: vec!["Game".into()],
            metadata: Metadata::default(),
            media: None,
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        };
        assert_eq!(unique(vec![game("1"), game("2")]), Lookup::Ambiguous(2));
    }

    #[test]
    fn not_game_is_never_accepted() {
        let game = Match {
            id: "1".into(),
            name: "ZZZ(notgame): Demo".into(),
            names: vec!["ZZZ(notgame): Demo".into()],
            metadata: Metadata::default(),
            media: None,
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        };
        assert_eq!(unique(vec![game]), Lookup::NotFound);
    }

    #[test]
    fn documented_non_game_flag_and_name_prefix_are_filtered_before_validation() {
        for xml in [
            "<Data><jeux><jeu notgame='true'></jeu></jeux></Data>",
            "<Data><jeux><jeu><notgame>true</notgame></jeu></jeux></Data>",
            "<Data><jeux><jeu><noms><nom region='wor'>ZZZ(notgame): Demo</nom></noms></jeu></jeux></Data>",
        ] {
            let parsed = parse(xml.as_bytes(), None, None, "ss").unwrap();
            assert!(parsed.games.is_empty());
        }
    }

    #[test]
    fn a_non_game_only_search_is_rejected_without_inventing_a_server_miss() {
        let response = client(HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><jeux><jeu><notgame>true</notgame></jeu></jeux></Data>".to_vec(),
        })
        .by_name(3, "Demo")
        .unwrap();
        assert_eq!(response.lookup, Lookup::NotFound);
        assert!(!response.server_miss);
    }

    #[test]
    fn malformed_non_game_flags_are_not_silently_accepted() {
        for xml in [
            "<Data><jeux><jeu id='1' notgame='maybe'><noms><nom region='wor'>Game</nom></noms></jeu></jeux></Data>",
            "<Data><jeux><jeu id='1'><notgame>maybe</notgame><noms><nom region='wor'>Game</nom></noms></jeu></jeux></Data>",
            "<Data><jeux><jeu id='1'><notgame></notgame><noms><nom region='wor'>Game</nom></noms></jeu></jeux></Data>",
            "<Data><jeux><jeu id='1'><notgame/><noms><nom region='wor'>Game</nom></noms></jeu></jeux></Data>",
        ] {
            assert_eq!(
                parse(xml.as_bytes(), None, None, "ss").unwrap_err().kind,
                ErrorKind::MalformedResponse
            );
        }

        let parsed = parse(
            b"<Data><jeux><jeu id='1' notgame='false'><noms><nom region='wor'>Game</nom></noms></jeu></jeux></Data>",
            None,
            None,
            "ss",
        )
        .unwrap();
        assert_eq!(parsed.games.len(), 1);
    }

    #[test]
    fn documented_account_regions_are_normalised_for_media_selection() {
        for (reported, expected) in [
            ("france", "fr"),
            ("europe", "eu"),
            ("usa", "us"),
            ("japon", "jp"),
            ("EU", "eu"),
        ] {
            assert_eq!(normalise_account_region(reported).unwrap(), expected);
        }
        assert_eq!(
            normalise_account_region("unknown").unwrap_err().kind,
            ErrorKind::MalformedResponse
        );
    }

    struct Mock {
        response: HttpResponse,
    }

    impl Transport for Mock {
        fn get(
            &self,
            _endpoint: &str,
            _params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            Ok(self.response.clone())
        }

        fn get_media(
            &self,
            _url: &str,
            _limit: u64,
            _max_kib_per_second: Option<u64>,
        ) -> Result<HttpResponse> {
            Ok(self.response.clone())
        }
    }

    fn client(response: HttpResponse) -> Client {
        Client::new(
            Arc::new(Mock { response }),
            DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
            &ScraperSettings {
                username: "user".into(),
                password: "password".into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn shared_client_selects_each_targets_requested_media_without_changing_the_default() {
        let response = HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: br#"<Data><jeux><jeu id="42">
                <noms><nom region="wor">Fixture Game</nom></noms>
                <medias>
                    <media type="ss" region="wor" format="png">https://media.screenscraper.fr/screenshot.png</media>
                    <media type="box-2D" region="jp" format="png">https://media.screenscraper.fr/box-jp.png</media>
                    <media type="box-2D" region="us" format="png">https://media.screenscraper.fr/box-us.png</media>
                    <media type="box-3D" region="wor" format="png">https://media.screenscraper.fr/box-3d.png</media>
                    <media type="wheel-hd" region="wor" format="png">https://media.screenscraper.fr/wheel.png</media>
                </medias>
                </jeu></jeux></Data>"#.to_vec(),
        };
        let client = client(response);
        let hashes = Hashes {
            size: 3,
            crc32: "352441C2".into(),
            md5: "900150983CD24FB0D6963F7D28E17F72".into(),
            sha1: "A9993E364706816ABA3E25717850C26C9CD0D89D".into(),
        };
        for (media_type, image) in [
            ("ss", Some("screenshot.png")),
            ("box-2d", Some("box-us.png")),
            ("BOX-3D", Some("box-3d.png")),
            ("wheel-hd", Some("wheel.png")),
            ("sstitle", None),
        ] {
            for response in [
                client
                    .search_with_media_type(3, "Fixture", media_type)
                    .unwrap(),
                client
                    .by_name_with_media_type(3, "Fixture Game", media_type)
                    .unwrap(),
                client
                    .by_hash_with_media_type(3, "fixture.nes", &hashes, media_type)
                    .unwrap(),
            ] {
                let Lookup::Found(found) = response.lookup else {
                    panic!("media selection must not change game matching");
                };
                let expected = image.map(|image| format!("https://media.screenscraper.fr/{image}"));
                assert_eq!(
                    found.media.as_ref().map(|media| media.url.as_str()),
                    expected.as_deref(),
                    "a missing requested kind must not substitute another kind"
                );
                assert_eq!(response.alternatives[0].media, found.media);
            }
            let default = client.search(3, "Fixture").unwrap();
            assert_eq!(
                default.alternatives[0].media.as_ref().unwrap().url,
                "https://media.screenscraper.fr/screenshot.png",
                "one target's choice must not change the shared client's global default"
            );
        }
    }

    #[test]
    fn custom_global_media_and_region_are_preserved_by_default_lookup_wrappers() {
        let client = Client::new(
            Arc::new(Mock {
                response: HttpResponse {
                    status: 200,
                    content_type: Some("application/xml".into()),
                    body: br#"<Data><jeux><jeu id="42"><noms><nom region="wor">Fixture Game</nom></noms>
                        <medias>
                            <media type="ss" region="wor" format="png">https://media.screenscraper.fr/screenshot.png</media>
                            <media type="wheel-hd" region="us" format="png">https://media.screenscraper.fr/wheel-us.png</media>
                            <media type="wheel-hd" region="jp" format="png">https://media.screenscraper.fr/wheel-jp.png</media>
                        </medias></jeu></jeux></Data>"#.to_vec(),
                },
            }),
            DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
            &ScraperSettings {
                username: "user".into(),
                password: "password".into(),
                media_type: "WHEEL-HD".into(),
                region: Some("jp".into()),
                system_media_types: [("NES".into(), "box-2D".into())].into(),
                ..Default::default()
            },
        )
        .unwrap();
        for response in [
            client.search(3, "Fixture").unwrap(),
            client.by_name(3, "Fixture Game").unwrap(),
        ] {
            assert_eq!(
                response.alternatives[0].media.as_ref().unwrap().url,
                "https://media.screenscraper.fr/wheel-jp.png",
                "a numeric platform id must not accidentally resolve a Degauss system override"
            );
        }
    }

    #[test]
    fn an_invalid_account_is_not_a_valid_empty_account() {
        let xml = b"<Data><ssuser><niveau>0</niveau></ssuser></Data>".to_vec();
        let error = client(HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: xml,
        })
        .account()
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Authentication);
    }

    #[test]
    fn missing_account_limits_are_not_treated_as_unlimited() {
        let xml = b"<Data><ssuser><niveau>1</niveau><maxthreads>1</maxthreads><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>100</maxrequestsperday><maxrequestskoperday>10</maxrequestskoperday></ssuser></Data>".to_vec();
        let error = client(HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: xml,
        })
        .account()
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::MalformedResponse);
    }

    #[test]
    fn content_type_cannot_turn_html_into_xml() {
        // An HTML page is never parsed as the account answer, and it is
        // reported as the service being unavailable rather than as a
        // malformed answer to one request.
        let error = client(HttpResponse {
            status: 200,
            content_type: Some("text/html".into()),
            body: b"<html>error</html>".to_vec(),
        })
        .account()
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unavailable);
    }

    #[test]
    fn a_page_without_the_data_root_is_an_outage_whatever_its_content_type() {
        // A maintenance, proxy or challenge page can arrive with no
        // Content-Type header or with an XML one; its body starts with '<'
        // like an answer, so the content type alone cannot tell it apart.
        // Without the root check it would count as one game's unreadable
        // match and a batch would spend one lookup per remaining game
        // against a service that is not answering. A body that is not
        // text at all carries no `<Data>` root either.
        let hashes = Hashes {
            size: 3,
            crc32: "352441C2".into(),
            md5: "900150983CD24FB0D6963F7D28E17F72".into(),
            sha1: "A9993E364706816ABA3E25717850C26C9CD0D89D".into(),
        };
        for (content_type, body) in [
            (None, &b"<html><body>Maintenance</body></html>"[..]),
            (
                Some("application/xml"),
                &b"<html><body>Maintenance</body></html>"[..],
            ),
            (
                None,
                &b"<!DOCTYPE html><html><body><p>Maintenance</html>"[..],
            ),
            (
                Some("text/xml"),
                &b"<?xml version='1.0'?><Response>busy</Response>"[..],
            ),
            (None, &b"<!-- nothing -->"[..]),
            (None, &b"<<<"[..]),
            (None, &b"<html>Maint\xe9nance</html>"[..]),
            (Some("application/xml"), &b"\xff\xfe<Data/>"[..]),
            // A notice placed between the XML declaration and the `<Data>`
            // root is text the service never emits; without the check it
            // would pass as a miss for every game.
            (
                None,
                &b"<?xml version=\"1.0\"?>Maintenance<Data><jeux/></Data>"[..],
            ),
            (
                Some("text/xml"),
                &b"<?xml version=\"1.0\"?>\n  Maintenance\n<Data><jeux/></Data>"[..],
            ),
            (
                None,
                &b"<?xml version=\"1.0\"?><![CDATA[Maintenance]]><Data><jeux/></Data>"[..],
            ),
        ] {
            let response = HttpResponse {
                status: 200,
                content_type: content_type.map(str::to_string),
                body: body.to_vec(),
            };
            let body = String::from_utf8_lossy(body);
            let searched = client(response.clone()).by_name(3, "Game").unwrap_err();
            assert_eq!(searched.kind, ErrorKind::Unavailable, "{body}");
            assert!(
                !searched.detail.contains("Maintenance"),
                "{}",
                searched.detail
            );
            assert_eq!(
                client(response.clone())
                    .by_hash(3, "Game.rom", &hashes)
                    .unwrap_err()
                    .kind,
                ErrorKind::Unavailable,
                "{body}"
            );
            assert_eq!(
                client(response).account().unwrap_err().kind,
                ErrorKind::Unavailable,
                "{body}"
            );
        }
        // A byte order mark and whitespace between the declaration and the
        // root are ordinary XML layout, and the empty game list behind them
        // is a server miss.
        let response = client(HttpResponse {
            status: 200,
            content_type: Some("text/xml".into()),
            body: b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n  \n<Data>\n  <jeux/>\n</Data>\n".to_vec(),
        })
        .by_name(3, "Game")
        .unwrap();
        assert_eq!(response.lookup, Lookup::NotFound);
        assert!(response.server_miss);
        // A `<Data>` answer that is cut short or carries no game list is
        // still that one request's unreadable answer.
        for body in [
            "<Data><jeux><jeu id='42'><noms>",
            "<Data><message>no game list here</message></Data>",
        ] {
            assert_eq!(
                client(HttpResponse {
                    status: 200,
                    content_type: None,
                    body: body.as_bytes().to_vec(),
                })
                .by_name(3, "Game")
                .unwrap_err()
                .kind,
                ErrorKind::MalformedResponse,
                "{body}"
            );
        }
    }

    #[test]
    fn a_reflected_request_address_is_redacted_from_the_diagnosis() {
        // `Client::auth` carries the developer and account credentials in
        // the query string, and the excerpt of a rejection, of an
        // unrecognised error text and of a plain page reaches the local
        // log. A page that reflects the request address must not put those
        // values there, including a value the cut leaves half in place.
        let echo = "?devid=dm1&devpassword=ds1&ssid=um1&sspassword=us1&romnom=G.rom";
        for text in [
            format!("Erreur : Problème dans le nom du fichier rom {echo}"),
            format!("Erreur : Problème dans la recherche {echo}"),
            format!("Bad request {echo}"),
        ] {
            let detail = text_error(&text, Answer::Lookup).detail;
            for marker in ["=dm1", "=ds1", "=um1", "=us1"] {
                assert!(!detail.contains(marker), "{detail}");
            }
            assert!(
                detail.ends_with(
                    "?devid=[redacted]&devpassword=[redacted]&ssid=[redacted]&sspassword=[redacted]&romnom=G.rom"
                ),
                "{detail}"
            );
        }
        let straddling = format!("{} sspassword=user-secret-marker", "x".repeat(105));
        let cut = excerpt(&straddling);
        assert!(cut.ends_with(" sspassword=[redacted]..."), "{cut}");
        assert_eq!(
            redact_credentials("<a href='x.php?ssid=user-marker'>ssid=user-marker</a> myssid=kept"),
            "<a href='x.php?ssid=[redacted]'>ssid=[redacted]</a> myssid=kept"
        );
    }

    #[test]
    fn unrelated_well_formed_xml_is_not_silently_treated_as_no_match() {
        let response = HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><message>temporarily unavailable</message></Data>".to_vec(),
        };
        assert_eq!(
            client(response.clone())
                .by_name(3, "Game")
                .unwrap_err()
                .kind,
            ErrorKind::MalformedResponse
        );
        let hashes = Hashes {
            size: 3,
            crc32: "352441C2".into(),
            md5: "900150983CD24FB0D6963F7D28E17F72".into(),
            sha1: "A9993E364706816ABA3E25717850C26C9CD0D89D".into(),
        };
        assert_eq!(
            client(response)
                .by_hash(3, "Game.rom", &hashes)
                .unwrap_err()
                .kind,
            ErrorKind::MalformedResponse
        );

        let empty = client(HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><jeux/></Data>".to_vec(),
        })
        .by_name(3, "Game")
        .unwrap();
        assert_eq!(empty.lookup, Lookup::NotFound);
        assert!(empty.server_miss);

        let empty = client(HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><jeux/></Data>".to_vec(),
        })
        .by_hash(3, "Game.rom", &hashes)
        .unwrap();
        assert_eq!(empty.lookup, Lookup::NotFound);
        assert!(empty.server_miss);
    }

    #[test]
    fn name_search_matches_every_region_but_writes_the_preferred_title() {
        let response = HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><jeux><jeu id='42' notgame='false'><noms><nom region='fr'>Les Schtroumpfs</nom><nom region='us'>The Smurfs</nom></noms></jeu></jeux></Data>".to_vec(),
        };
        let client = Client::new(
            Arc::new(Mock { response }),
            DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
            &ScraperSettings {
                username: "user".into(),
                password: "password".into(),
                region: Some("fr".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let result = client.by_name(3, "The Smurfs (USA)").unwrap();
        let Lookup::Found(found) = result.lookup else {
            panic!("the exact title in another returned region was not matched");
        };
        assert_eq!(found.name, "Les Schtroumpfs");
        assert_eq!(found.names, ["Les Schtroumpfs", "The Smurfs"]);
    }

    #[test]
    fn manual_search_keeps_broad_results_in_server_order_without_weakening_automatic_matching() {
        let response = HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><jeux>\
                <jeu id='20'><noms><nom region='wor'>Adventure Island</nom></noms></jeu>\
                <jeu id='10'><noms><nom region='wor'>Adventure Time</nom></noms></jeu>\
                <jeu id='20'><noms><nom region='wor'>Adventure Island duplicate</nom></noms></jeu>\
                </jeux></Data>"
                .to_vec(),
        };
        let client = client(response);

        let manual = client.search(3, "Adventure").unwrap();
        assert_eq!(
            manual
                .alternatives
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>(),
            ["20", "10"],
            "the picker keeps relevance order and removes duplicate ids"
        );
        assert_eq!(manual.lookup, Lookup::Ambiguous(2));

        let automatic = client.by_name(3, "Adventure").unwrap();
        assert_eq!(automatic.lookup, Lookup::NotFound);
        assert_eq!(automatic.alternatives.len(), 2);
    }

    #[test]
    fn manual_search_rejects_a_blank_term_before_contacting_the_service() {
        let error = client(HttpResponse {
            status: 200,
            content_type: Some("application/xml".into()),
            body: b"<Data><jeux/></Data>".to_vec(),
        })
        .search(3, "  \n ")
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Configuration);
    }

    #[test]
    fn verified_plain_text_login_errors_are_not_empty_results() {
        for (body, expected) in [
            (
                "Erreur de login : Vérifier vos identifiants développeur !",
                ErrorKind::Authentication,
            ),
            (
                "Erreur de login : Vérifier les identifiants utilisateurs !",
                ErrorKind::Authentication,
            ),
        ] {
            let error = client(HttpResponse {
                status: 200,
                content_type: Some("text/plain".into()),
                body: body.as_bytes().to_vec(),
            })
            .account()
            .unwrap_err();
            assert_eq!(error.kind, expected);
            assert!(!error.to_string().contains(body));
        }

        let echoed = "Erreur utilisateur: ssid=user-marker&sspassword=user-secret-marker";
        let error = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: echoed.as_bytes().to_vec(),
        })
        .account()
        .unwrap_err();
        assert!(!error.to_string().contains("user-marker"));
        assert!(!error.to_string().contains("user-secret-marker"));
    }

    #[test]
    fn a_forbidden_user_login_is_not_misreported_as_a_developer_failure() {
        let error = client(HttpResponse {
            status: 403,
            content_type: Some("text/plain".into()),
            body: "Erreur de login : Vérifier les identifiants utilisateurs !"
                .as_bytes()
                .to_vec(),
        })
        .account()
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Authentication);
        assert_eq!(
            error.detail,
            "ScreenScraper rejected the login. Check the username and password"
        );
        assert_eq!(
            authentication_source("Erreur: identifiants developpeur"),
            Some(AuthenticationSource::Application)
        );
        assert_eq!(
            authentication_source("Erreur: identifiants utilisateurs"),
            Some(AuthenticationSource::Account)
        );
    }

    #[test]
    fn a_plain_text_no_match_is_a_server_miss_not_a_transport_failure() {
        let result = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: b"Erreur : jeu non trouve".to_vec(),
        })
        .by_name(3, "Missing Game")
        .unwrap();
        assert_eq!(result.lookup, Lookup::NotFound);
        assert!(result.server_miss);
    }

    #[test]
    fn only_documented_per_request_error_texts_leave_the_run_going() {
        // The documented 400 texts about one rom name or hash field, and a
        // text ScreenScraper has not documented answering a game's lookup
        // (the invalid search the batch must continue past), concern one
        // request and must not stop the batch by masquerading as an
        // outage. The documented 400 texts about the request address concern
        // every game, because Degauss sends the same fields for each. A
        // closure, a refused client (the blacklist text is classified like
        // HTTP 426, which carries it), or a body that is no answer at all
        // concerns every remaining game and stops the run instead of
        // costing one lookup per game.
        for (body, kind) in [
            (
                "Erreur : API fermé pour les non membres ou les membres inactifs",
                ErrorKind::Unavailable,
            ),
            ("Erreur : API totalement fermé", ErrorKind::Unavailable),
            (
                "Erreur : Le logiciel de scrape utilisé a été blacklisté",
                ErrorKind::Configuration,
            ),
            (
                "Erreur : Problème dans le nom du fichier rom",
                ErrorKind::InvalidRequest,
            ),
            (
                "Erreur dans le nom du fichier rom : celui-ci contient un chemin d'accés",
                ErrorKind::InvalidRequest,
            ),
            (
                "Erreur : Champ crc, md5 ou sha1 erroné",
                ErrorKind::InvalidRequest,
            ),
            (
                "Erreur : Il manque des champs obligatoires dans l'url",
                ErrorKind::Configuration,
            ),
            ("Erreur : problème avec l'url", ErrorKind::Configuration),
            (
                "Erreur : Problème dans la recherche",
                ErrorKind::InvalidRequest,
            ),
            (
                "Erreur de login : Vérifier vos identifiants développeur !",
                ErrorKind::Authentication,
            ),
            (
                "Erreur de login : Vérifier les identifiants utilisateurs !",
                ErrorKind::Authentication,
            ),
            (
                "Erreur : Votre quota de scrape est dépassé pour aujourd'hui !",
                ErrorKind::DailyQuota,
            ),
            (
                "Erreur : Faite du tri dans vos fichiers roms et repassez demain !",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : Le nombre de threads autorisé pour le membre est atteint",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : Le nombre de threads par minute autorisé pour le membre est atteint",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : The maximum threads allowed to leecher users is already used",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : The maximum threads is already used",
                ErrorKind::Unavailable,
            ),
            ("Erreur : Jeu non trouvée !", ErrorKind::NotFound),
            ("Service paused for maintenance", ErrorKind::Unavailable),
            ("", ErrorKind::Unavailable),
        ] {
            assert_eq!(text_error(body, Answer::Lookup).kind, kind, "{body}");
        }
        let rejected = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: "Erreur : Problème dans le nom du fichier rom"
                .as_bytes()
                .to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(rejected.kind, ErrorKind::InvalidRequest);
        assert!(!rejected.retryable());
        let incomplete = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: "Erreur : Il manque des champs obligatoires dans l'url"
                .as_bytes()
                .to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(incomplete.kind, ErrorKind::Configuration);
        let undocumented = HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: "Erreur : Problème dans la recherche".as_bytes().to_vec(),
        };
        assert_eq!(
            client(undocumented.clone())
                .by_name(3, "Game")
                .unwrap_err()
                .kind,
            ErrorKind::InvalidRequest
        );
        // The same text before the run, at the account check, or on an
        // image transfer cannot be one game's invalid search: an outage.
        assert_eq!(
            client(undocumented.clone()).account().unwrap_err().kind,
            ErrorKind::Unavailable
        );
        assert_eq!(
            client(undocumented)
                .media(
                    &Media {
                        url: "https://media.screenscraper.fr/box.png".into(),
                        format: None,
                    },
                    1024,
                )
                .err()
                .expect("an undocumented error text on a media transfer was accepted")
                .kind,
            ErrorKind::Unavailable
        );
        let closed = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: "Erreur : API totalement fermé".as_bytes().to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(closed.kind, ErrorKind::Unavailable);
        // The 429 and 431 texts are classified by their HTTP status; the
        // same text in a 2xx body is an outage, as it always was: the batch
        // stops instead of repeating a lookup the server is refusing for
        // every game.
        let limited = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: "Erreur : The maximum threads is already used"
                .as_bytes()
                .to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(limited.kind, ErrorKind::Unavailable);
        let exhausted = client(HttpResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: "Erreur : Faite du tri dans vos fichiers roms et repassez demain !"
                .as_bytes()
                .to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(exhausted.kind, ErrorKind::Unavailable);
        // A page that is not XML at all is not a match response for one
        // game: it stops the run like any other outage.
        let page = client(HttpResponse {
            status: 200,
            content_type: Some("text/html; charset=utf-8".into()),
            body: b"<html>maintenance</html>".to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(page.kind, ErrorKind::Unavailable);
        assert!(page.detail.contains("text/html"), "{}", page.detail);
        // So does a plain-text body that is no error text, whatever its
        // content type says, and so does an empty answer.
        for (content_type, body) in [
            (Some("text/plain"), "Service paused for maintenance"),
            (Some("application/xml"), ""),
            (None, "Service paused for maintenance"),
        ] {
            let notice = client(HttpResponse {
                status: 200,
                content_type: content_type.map(str::to_string),
                body: body.as_bytes().to_vec(),
            })
            .by_name(3, "Game")
            .unwrap_err();
            assert_eq!(
                notice.kind,
                ErrorKind::Unavailable,
                "{content_type:?} {body:?}"
            );
        }
    }

    #[test]
    fn a_service_wide_text_under_http_400_still_stops_the_run() {
        // HTTP 400 is one game's rejection, so it no longer stops the run.
        // A documented closure, limit, allowance or refused-client text
        // delivered with that status would otherwise cost one lookup per
        // remaining game. A per-request text and one ScreenScraper has not
        // documented keep the status's per-game meaning, and the log detail
        // keeps the server's words either way, because the fixed HTTP 400
        // message alone would not say which rejection was sent.
        for (body, kind) in [
            ("Erreur : API totalement fermé", ErrorKind::Unavailable),
            (
                "Erreur : Le logiciel de scrape utilisé a été blacklisté",
                ErrorKind::Configuration,
            ),
            (
                "Erreur : Votre quota de scrape est dépassé pour aujourd'hui !",
                ErrorKind::DailyQuota,
            ),
            (
                "Erreur : Le nombre de threads par minute autorisé pour le membre est atteint",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : The maximum threads is already used",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : Faite du tri dans vos fichiers roms et repassez demain !",
                ErrorKind::Unavailable,
            ),
            (
                "Erreur : Il manque des champs obligatoires dans l'url",
                ErrorKind::Configuration,
            ),
            (
                "Erreur : Problème dans le nom du fichier rom",
                ErrorKind::InvalidRequest,
            ),
            (
                "Erreur : Problème dans la recherche",
                ErrorKind::InvalidRequest,
            ),
        ] {
            let error = client(HttpResponse {
                status: 400,
                content_type: Some("text/plain".into()),
                body: body.as_bytes().to_vec(),
            })
            .by_name(3, "Game")
            .unwrap_err();
            assert_eq!(error.kind, kind, "{body}");
            if kind == ErrorKind::InvalidRequest {
                assert!(
                    error.detail.ends_with(&format!(": {body}")),
                    "{body}: {}",
                    error.detail
                );
            }
        }
        // A documented not-found text keeps its own meaning under HTTP 400
        // as it does on a successful status: the game is a server miss,
        // not a rejection.
        let missed = client(HttpResponse {
            status: 400,
            content_type: Some("text/plain".into()),
            body: "Erreur : Jeu non trouvée !".as_bytes().to_vec(),
        })
        .by_name(3, "Game")
        .unwrap();
        assert_eq!(missed.lookup, Lookup::NotFound);
        assert!(missed.server_miss);
        // Other statuses keep their own meaning over a body text, so a
        // rate limit stays retryable whatever the text says.
        let limited = client(HttpResponse {
            status: 429,
            content_type: Some("text/plain".into()),
            body: "Erreur : API totalement fermé".as_bytes().to_vec(),
        })
        .by_name(3, "Game")
        .unwrap_err();
        assert_eq!(limited.kind, ErrorKind::RateLimited);
    }

    #[test]
    fn a_page_under_http_400_stops_the_run_and_a_text_names_the_rejection() {
        // ScreenScraper answers HTTP 400 with an error text. An HTML page
        // (whatever content type it is served with, or none), an empty body
        // or a body that is not text under that status comes from an
        // intermediary, as it would on a successful status, and must not
        // cost one lookup per remaining game while the log stays silent
        // about the cause. A text that is not one of the documented error
        // texts is still that game's rejection, with the server's words in
        // the log detail.
        let hashes = Hashes {
            size: 3,
            crc32: "352441C2".into(),
            md5: "900150983CD24FB0D6963F7D28E17F72".into(),
            sha1: "A9993E364706816ABA3E25717850C26C9CD0D89D".into(),
        };
        for (content_type, body, what) in [
            (
                Some("text/html"),
                &b"<html><body>Bad request</body></html>"[..],
                "content type text/html",
            ),
            (
                None,
                &b"<html><body>Bad request</body></html>"[..],
                "markup instead of an error text",
            ),
            (
                Some("application/xml"),
                &b"<!DOCTYPE html><html><body><p>Bad request</html>"[..],
                "markup instead of an error text",
            ),
            (Some("application/xml"), &b""[..], "no error text"),
            (Some("text/plain"), &b" \n"[..], "no error text"),
            (None, &b"Bad requ\xe9st"[..], "no error text"),
        ] {
            let response = HttpResponse {
                status: 400,
                content_type: content_type.map(str::to_string),
                body: body.to_vec(),
            };
            let body = String::from_utf8_lossy(body);
            for error in [
                client(response.clone()).by_name(3, "Game").unwrap_err(),
                client(response.clone())
                    .by_hash(3, "Game.rom", &hashes)
                    .unwrap_err(),
                client(response.clone()).account().unwrap_err(),
            ] {
                assert_eq!(
                    error.kind,
                    ErrorKind::Unavailable,
                    "{content_type:?} {body:?}"
                );
                assert!(
                    error.detail.contains(what) && error.detail.contains("HTTP 400"),
                    "{}",
                    error.detail
                );
            }
        }
        let response = HttpResponse {
            status: 400,
            content_type: Some("text/plain".into()),
            body: b"Bad request".to_vec(),
        };
        let rejected = client(response.clone()).by_name(3, "Game").unwrap_err();
        assert_eq!(rejected.kind, ErrorKind::InvalidRequest);
        assert!(
            rejected.detail.ends_with("(HTTP 400): Bad request"),
            "{}",
            rejected.detail
        );
        // The account check sends only the credential fields, so a
        // rejection there is a setup fault as it always was.
        let setup = client(response).account().unwrap_err();
        assert_eq!(setup.kind, ErrorKind::Configuration);
        assert!(
            setup.detail.ends_with("(HTTP 400): Bad request"),
            "{}",
            setup.detail
        );
        // An image transfer answers with bytes: a rejection there is that
        // game's picture failure whatever the body holds.
        for body in [&b"<html>Bad request</html>"[..], &b""[..], &b"\x89PNG"[..]] {
            let error = client(HttpResponse {
                status: 400,
                content_type: Some("text/html".into()),
                body: body.to_vec(),
            })
            .media(
                &Media {
                    url: "https://media.screenscraper.fr/box.png".into(),
                    format: None,
                },
                1024,
            )
            .err()
            .expect("HTTP 400 on a media transfer was accepted");
            assert_eq!(error.kind, ErrorKind::InvalidRequest, "{body:?}");
        }
    }

    #[test]
    fn an_undocumented_error_text_is_kept_in_the_diagnosis_but_bounded() {
        // The interface shows only the kind's fixed message, so the log
        // detail is the one place the server's actual words can be seen,
        // for a per-game refusal and for the unrecognised text that stopped
        // a run alike. A long body is cut so a page cannot flood it.
        let rejected = text_error(
            "Erreur : Problème   dans le nom\ndu fichier rom",
            Answer::Lookup,
        );
        assert_eq!(rejected.kind, ErrorKind::InvalidRequest);
        assert!(
            rejected
                .detail
                .ends_with(": Erreur : Problème dans le nom du fichier rom"),
            "{}",
            rejected.detail
        );
        for (answer, kind) in [
            (Answer::Lookup, ErrorKind::InvalidRequest),
            (Answer::Account, ErrorKind::Unavailable),
        ] {
            let error = text_error("Erreur : Problème   dans\nla recherche", answer);
            assert_eq!(error.kind, kind, "{answer:?}");
            assert!(
                error
                    .detail
                    .ends_with(": Erreur : Problème dans la recherche"),
                "{}",
                error.detail
            );
        }
        let long = format!("Erreur : {}", "é".repeat(500));
        let detail = text_error(&long, Answer::Lookup).detail;
        assert!(detail.ends_with("..."), "{detail}");
        assert!(detail.chars().count() < 200, "{detail}");
        let plain = text_error("Service paused for maintenance", Answer::Lookup);
        assert_eq!(plain.kind, ErrorKind::Unavailable);
        assert!(
            plain.detail.ends_with(": Service paused for maintenance"),
            "{}",
            plain.detail
        );
        // An empty answer has no words to keep: the detail names the
        // emptiness instead of ending in a bare colon.
        for empty in ["", " \n\t"] {
            let error = text_error(empty, Answer::Lookup);
            assert_eq!(error.kind, ErrorKind::Unavailable);
            assert_eq!(error.detail, "ScreenScraper returned an empty response");
        }
        // The cut falls exactly at the limit, with the whitespace between
        // words counted once, so a documented text is never shortened.
        let exact = format!("{} {}", "a".repeat(60), "b".repeat(59));
        assert_eq!(excerpt(&exact), exact);
        assert_eq!(excerpt(&format!("{exact} c")), format!("{exact}..."));
        assert_eq!(excerpt("  one \n\t two  "), "one two");
    }

    #[test]
    fn api_values_cannot_inject_curl_configuration() {
        let url = api_url(
            "jeuRecherche.php",
            &[
                ("recherche".into(), "Game & Watch".into()),
                ("sspassword".into(), "secret\noutput = \"stolen\"".into()),
            ],
        );
        assert!(url.contains("Game%20%26%20Watch"));
        assert!(url.contains("secret%0Aoutput%20%3D%20%22stolen%22"));
        assert!(!url.contains('\n'));
        assert!(!url.contains('"'));

        let config = curl_config(&url, std::path::Path::new("/tmp/result"), 1024, None, None);
        assert!(!config.contains("location"));
        assert!(!config.contains("limit-rate"));
        assert!(!config.contains("cacert"));
        assert_eq!(CURL_ARGS, ["-q", "--config", "-"]);
    }

    #[test]
    fn curl_uses_an_explicit_readable_ca_bundle_without_disabling_verification() {
        let config = curl_config(
            "https://api.screenscraper.fr/api2/ssuserInfos.php",
            Path::new("/tmp/result"),
            1024,
            None,
            Some(Path::new("/etc/ssl/certs/cacert.pem")),
        );
        assert!(config.contains("cacert = \"/etc/ssl/certs/cacert.pem\""));
        assert!(!config.contains("insecure"));
    }

    #[test]
    fn ca_bundle_selection_prefers_the_first_readable_candidate() {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "degauss-ca-selection-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let first = directory.join("first.pem");
        let second = directory.join("second.pem");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        assert_eq!(
            first_readable_ca_bundle([first.as_path(), second.as_path()]),
            Some(first.clone())
        );
        std::fs::remove_file(&first).unwrap();
        assert_eq!(
            first_readable_ca_bundle([first.as_path(), second.as_path()]),
            Some(second.clone())
        );
        std::fs::remove_file(&second).unwrap();
        assert_eq!(
            first_readable_ca_bundle([first.as_path(), second.as_path()]),
            None
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn curl_failures_are_concise_on_screen_and_detailed_in_the_log_mapping() {
        let unreachable = curl_exit_error(Some(7), None, 1024);
        assert_eq!(unreachable.kind, ErrorKind::Transport);
        assert_eq!(
            unreachable.detail,
            "Could not reach ScreenScraper. Check the network connection."
        );

        let timeout = curl_exit_error(Some(28), None, 1024);
        assert_eq!(timeout.kind, ErrorKind::Timeout);
        assert_eq!(
            timeout.detail,
            "ScreenScraper did not respond in time. Try again."
        );

        let certificate = curl_exit_error(Some(60), None, 1024);
        assert_eq!(certificate.kind, ErrorKind::Transport);
        assert_eq!(
            certificate.detail,
            "Secure connection to ScreenScraper failed. Check MiSTer's date and network."
        );
        assert!(!certificate.detail.contains("curl"));
        assert_eq!(
            curl_exit_diagnostic(Some(60)),
            "peer certificate could not be authenticated"
        );

        let ca_bundle = curl_exit_error(Some(77), None, 1024);
        assert_eq!(ca_bundle.kind, ErrorKind::Configuration);
        assert_eq!(
            ca_bundle.detail,
            "Degauss could not use MiSTer's security certificates."
        );
        assert_eq!(
            curl_exit_diagnostic(Some(77)),
            "CA certificate bundle could not be read"
        );

        let server = status(503).unwrap_err();
        assert_eq!(server.kind, ErrorKind::Server);
        assert_eq!(
            server.detail,
            "ScreenScraper is temporarily unavailable (HTTP 503). Try again later."
        );
    }

    #[test]
    fn media_download_speed_is_encoded_only_as_a_curl_limit() {
        let config = curl_config(
            "https://media.screenscraper.fr/image.png",
            std::path::Path::new("/tmp/result"),
            1024,
            Some(256),
            None,
        );
        assert!(config.contains("limit-rate = \"256K\""));
    }

    #[test]
    fn media_timeout_allows_for_the_reported_speed_but_remains_bounded() {
        assert_eq!(request_timeout(1024, None), GLOBAL_TIMEOUT);
        assert_eq!(
            request_timeout(8 * 1024 * 1024, Some(128)),
            Duration::from_secs(79)
        );
        assert_eq!(request_timeout(u64::MAX, Some(1)), MAX_MEDIA_TIMEOUT);
    }

    #[test]
    fn numeric_xml_entities_are_preserved_in_metadata() {
        let xml = FIXTURE.replace("World Name", "World &#x26; Name");
        let parsed = parse(xml.as_bytes(), Some("us"), Some("en"), "ss").unwrap();
        assert_eq!(parsed.games[0].name, "World & Name");
    }

    #[test]
    fn only_the_direct_scraped_rom_hashes_are_used() {
        let xml = r#"<Data><jeux><jeu id="42">
            <noms><nom region="wor">Game</nom></noms>
            <roms><rom><rommd5>WRONG-NESTED-HASH</rommd5></rom></roms>
            <rom><rommd5>RIGHT-DIRECT-HASH</rommd5></rom>
        </jeu></jeux></Data>"#;
        let parsed = parse(xml.as_bytes(), None, None, "ss").unwrap();
        assert_eq!(
            parsed.games[0].rom_md5.as_deref(),
            Some("RIGHT-DIRECT-HASH")
        );
    }

    #[test]
    fn every_present_remote_hash_must_agree() {
        let hashes = Hashes {
            size: 3,
            crc32: "352441C2".into(),
            md5: "900150983CD24FB0D6963F7D28E17F72".into(),
            sha1: "A9993E364706816ABA3E25717850C26C9CD0D89D".into(),
        };
        let mut candidate = Match {
            id: "42".into(),
            name: "Game".into(),
            names: vec!["Game".into()],
            metadata: Metadata::default(),
            media: None,
            rom_crc32: Some(hashes.crc32.clone()),
            rom_md5: Some(hashes.md5.clone()),
            rom_sha1: None,
        };
        assert!(matching_rom_hashes(&candidate, &hashes));
        candidate.rom_md5 = Some("00000000000000000000000000000000".into());
        assert!(!matching_rom_hashes(&candidate, &hashes));
    }

    struct RecordingMock {
        response: HttpResponse,
        params: Mutex<Vec<(String, String)>>,
    }

    impl Transport for RecordingMock {
        fn get(
            &self,
            _endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            *self.params.lock().unwrap() = params.to_vec();
            Ok(self.response.clone())
        }

        fn get_media(
            &self,
            _url: &str,
            _limit: u64,
            _max_kib_per_second: Option<u64>,
        ) -> Result<HttpResponse> {
            unreachable!()
        }
    }

    #[test]
    fn hash_lookup_sends_rom_type_and_rejects_conflicting_server_hashes() {
        let transport = Arc::new(RecordingMock {
            response: HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: br#"<Data><jeux><jeu id="42"><noms><nom region="wor">Game</nom></noms><rom><rommd5>00000000000000000000000000000000</rommd5></rom></jeu></jeux></Data>"#.to_vec(),
            },
            params: Mutex::new(Vec::new()),
        });
        let client = Client::new(
            transport.clone(),
            DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
            &ScraperSettings {
                username: "user".into(),
                password: "password".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let hashes = Hashes {
            size: 3,
            crc32: "352441C2".into(),
            md5: "900150983CD24FB0D6963F7D28E17F72".into(),
            sha1: "A9993E364706816ABA3E25717850C26C9CD0D89D".into(),
        };
        let result = client.by_hash(3, "Game.rom", &hashes).unwrap();
        assert_eq!(result.lookup, Lookup::NotFound);
        assert!(!result.server_miss);
        assert!(transport
            .params
            .lock()
            .unwrap()
            .iter()
            .any(|(key, value)| key == "romtype" && value == "rom"));
    }
}
