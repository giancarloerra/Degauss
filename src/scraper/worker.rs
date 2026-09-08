use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::browse::DisplayNames;
use crate::systems::FoundSystem;
use sha1::{Digest, Sha1};

use super::api::{Client, CurlTransport, HttpResponse, Lookup, Transport};
use super::gamelist_edit::{self, Backups, Needs, Update};
use super::hashes;
use super::{
    Account, DeveloperCredentials, Error, ErrorKind, Match, Result, Scope, ScraperSettings, Target,
};

const EVENT_QUEUE: usize = 32;
const MAX_WORKERS: usize = 32;
// ScreenScraper reports one user-level download-speed allowance. Keep media
// serial so curl's per-transfer limit also bounds the aggregate transfer rate.
const MAX_MEDIA_WORKERS: usize = 1;
const MAX_MEDIA_DIMENSION: usize = 4096;
const RETRIES: usize = 3;
static MEDIA_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct Request {
    pub scope: Scope,
    pub scope_label: String,
    pub systems: Vec<FoundSystem>,
    pub names: DisplayNames,
    pub settings: ScraperSettings,
    /// Not needed for an all-Artwork-Pack run, which must finish as an
    /// expected skip without touching any ScreenScraper credential.
    pub developer: Option<DeveloperCredentials>,
    pub artwork_pack_system_ids: HashSet<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Account,
    Enumerating,
    Scraping,
    Finishing,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Account => "Checking account",
            Self::Enumerating => "Scanning the card",
            Self::Scraping => "Scraping",
            Self::Finishing => "Finishing",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub activity: String,
    pub scope: String,
    pub current: String,
    pub total: usize,
    pub completed: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub not_found: usize,
    pub ambiguous: usize,
    pub no_media: usize,
    pub failed: usize,
    pub unsupported_systems: usize,
    pub skipped_artwork_pack: usize,
    pub system_errors: usize,
    pub ambiguous_targets: usize,
    pub workers: usize,
    pub account: Option<Account>,
    pub requests_started: u64,
    pub failed_searches: u64,
    pub last_problem: Option<String>,
    pub updated_systems: Vec<String>,
    /// Candidate records retained only for an unresolved one-game scrape.
    /// Batch runs keep counting unresolved entries without carrying every
    /// server response or interrupting an unattended scrape.
    pub manual_matches: Vec<Match>,
}

impl Progress {
    pub fn requests_left(&self) -> Option<u64> {
        Some(
            self.account
                .as_ref()?
                .requests_left()?
                .saturating_sub(self.requests_started),
        )
    }

    pub fn failed_left(&self) -> Option<u64> {
        Some(
            self.account
                .as_ref()?
                .failed_left()?
                .saturating_sub(self.failed_searches),
        )
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Progress(Progress),
    Finished(Progress),
    Cancelled(Progress),
    Failed { error: Error, progress: Progress },
}

pub struct Job {
    events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn try_recv(&mut self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    Event::Finished(_) | Event::Cancelled(_) | Event::Failed { .. }
                ) {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct SearchRequest {
    pub system_id: u32,
    pub term: String,
    pub settings: ScraperSettings,
    pub developer: DeveloperCredentials,
}

#[derive(Debug)]
pub enum SearchEvent {
    Activity(String),
    Finished {
        matches: Vec<Match>,
        account: Account,
        requests_started: u64,
        failed_searches: u64,
    },
    Cancelled,
    Failed(Error),
}

pub struct SearchJob {
    events: Receiver<SearchEvent>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SearchJob {
    pub fn try_recv(&mut self) -> Option<SearchEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    SearchEvent::Finished { .. } | SearchEvent::Cancelled | SearchEvent::Failed(_)
                ) {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub struct PreviewRequest {
    pub match_id: String,
    pub media: super::Media,
    pub settings: ScraperSettings,
    pub developer: DeveloperCredentials,
    pub max_download_speed: u64,
    pub max_edge: u32,
    pub ground: [u8; 3],
}

#[derive(Debug)]
pub enum PreviewEvent {
    Finished {
        match_id: String,
        image: crate::covers::RgbImage,
    },
    Missing {
        match_id: String,
    },
    Cancelled,
    Failed {
        match_id: String,
        error: Error,
    },
}

pub struct PreviewJob {
    events: Receiver<PreviewEvent>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PreviewJob {
    pub fn try_recv(&mut self) -> Option<PreviewEvent> {
        match self.events.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    PreviewEvent::Finished { .. }
                        | PreviewEvent::Missing { .. }
                        | PreviewEvent::Cancelled
                        | PreviewEvent::Failed { .. }
                ) {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for PreviewJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start(request: Request) -> Result<Job> {
    start_with_selected(request, None)
}

pub fn start_search(request: SearchRequest) -> Result<SearchJob> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(CurlTransport::new(Arc::clone(&cancelled)));
    start_search_with_transport(request, transport, cancelled)
}

fn start_search_with_transport(
    request: SearchRequest,
    transport: Arc<dyn Transport>,
    cancelled: Arc<AtomicBool>,
) -> Result<SearchJob> {
    let (sender, events) = mpsc::sync_channel(4);
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-scraper-search".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_search(request, transport, &sender, &worker_cancelled)
            }));
            if outcome.is_err() {
                let _ = sender.send(SearchEvent::Failed(Error::local(
                    "the ScreenScraper search stopped unexpectedly",
                )));
            }
        })
        .map_err(|error| Error::local(format!("could not start ScreenScraper search: {error}")))?;
    Ok(SearchJob {
        events,
        cancelled,
        handle: Some(handle),
    })
}

pub fn start_preview(request: PreviewRequest) -> Result<PreviewJob> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(CurlTransport::new(Arc::clone(&cancelled)));
    start_preview_with_transport(request, transport, cancelled)
}

fn start_preview_with_transport(
    request: PreviewRequest,
    transport: Arc<dyn Transport>,
    cancelled: Arc<AtomicBool>,
) -> Result<PreviewJob> {
    let (sender, events) = mpsc::sync_channel(1);
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-scraper-preview".into())
        .spawn(move || {
            let match_id = request.match_id.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_preview(request, transport, &worker_cancelled)
            }));
            let event = match result {
                Ok(Ok(image)) => PreviewEvent::Finished { match_id, image },
                Ok(Err(error)) if error.kind == ErrorKind::NotFound => {
                    PreviewEvent::Missing { match_id }
                }
                Ok(Err(error)) if error.kind == ErrorKind::Cancelled => PreviewEvent::Cancelled,
                Ok(Err(error)) => PreviewEvent::Failed { match_id, error },
                Err(_) => PreviewEvent::Failed {
                    match_id,
                    error: Error::local("the ScreenScraper preview stopped unexpectedly"),
                },
            };
            let _ = sender.send(event);
        })
        .map_err(|error| Error::local(format!("could not start artwork preview: {error}")))?;
    Ok(PreviewJob {
        events,
        cancelled,
        handle: Some(handle),
    })
}

/// Apply a match the user selected explicitly to one game. Enumeration,
/// policy checks, media validation, backups and atomic gamelist writing all
/// stay on the same path as an automatic scrape; only lookup is bypassed.
pub fn start_selected(request: Request, matched: Match) -> Result<Job> {
    start_with_selected(request, Some(matched))
}

fn start_with_selected(request: Request, selected: Option<Match>) -> Result<Job> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let transport = Arc::new(CurlTransport::new(Arc::clone(&cancelled)));
    start_with_transport_and_cancel_selected(request, transport, cancelled, selected)
}

#[cfg(test)]
fn start_with_transport(request: Request, transport: Arc<dyn Transport>) -> Result<Job> {
    let cancelled = Arc::new(AtomicBool::new(false));
    start_with_transport_and_cancel(request, transport, cancelled)
}

#[cfg(test)]
fn start_with_transport_and_cancel(
    request: Request,
    transport: Arc<dyn Transport>,
    cancelled: Arc<AtomicBool>,
) -> Result<Job> {
    start_with_transport_and_cancel_selected(request, transport, cancelled, None)
}

fn start_with_transport_and_cancel_selected(
    request: Request,
    transport: Arc<dyn Transport>,
    cancelled: Arc<AtomicBool>,
    selected: Option<Match>,
) -> Result<Job> {
    let (sender, events) = mpsc::sync_channel(EVENT_QUEUE);
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-scraper".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(request, transport, &sender, &worker_cancelled, selected)
            }));
            if outcome.is_err() {
                let _ = sender.send(Event::Failed {
                    error: Error::local("the scraper worker stopped unexpectedly"),
                    progress: Progress::default(),
                });
            }
        })
        .map_err(|error| Error::local(format!("could not start the scraper worker: {error}")))?;
    Ok(Job {
        events,
        cancelled,
        handle: Some(handle),
    })
}

fn run_search(
    request: SearchRequest,
    transport: Arc<dyn Transport>,
    events: &SyncSender<SearchEvent>,
    cancelled: &Arc<AtomicBool>,
) {
    let result = (|| {
        if request.term.trim().is_empty() {
            return Err(Error::new(
                ErrorKind::Configuration,
                "enter a game title to search for",
            ));
        }
        request.settings.validate()?;
        if request.settings.username.is_empty() || request.settings.password.is_empty() {
            return Err(Error::new(
                ErrorKind::Authentication,
                "ScreenScraper username and password are required",
            ));
        }

        let _ = events.send(SearchEvent::Activity("Checking account".into()));
        let account_client = Client::new(
            Arc::clone(&transport),
            request.developer.clone(),
            &request.settings,
        )?;
        let account_requests = AtomicU64::new(0);
        let account = retry(
            cancelled,
            |error, delay| {
                let _ = events.send(SearchEvent::Activity(retry_status(error, delay)));
            },
            || {
                account_requests.fetch_add(1, Ordering::Relaxed);
                account_client.account()
            },
        )?;
        let max_threads = account.max_threads.unwrap_or(0);
        let max_download_speed = account.max_download_speed.ok_or_else(|| {
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's download-speed limit",
            )
        })?;
        let max_requests_per_minute = account.max_requests_per_minute.ok_or_else(|| {
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's per-minute limit",
            )
        })?;
        let max_requests_per_minute = usize::try_from(max_requests_per_minute).map_err(|_| {
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper reported an out-of-range per-minute limit",
            )
        })?;
        let requests_left = account.requests_left().ok_or_else(|| {
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's daily request limit",
            )
        })?;
        let failed_left = account.failed_left().ok_or_else(|| {
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's failed-search limit",
            )
        })?;
        if max_threads == 0 || max_requests_per_minute == 0 {
            return Err(Error::new(
                ErrorKind::RateLimited,
                "ScreenScraper reported that this account cannot start a request now",
            ));
        }
        if requests_left == 0 {
            return Err(Error::new(
                ErrorKind::DailyQuota,
                "the daily ScreenScraper request allowance is exhausted",
            ));
        }
        if failed_left == 0 {
            return Err(Error::new(
                ErrorKind::FailedQuota,
                "the daily ScreenScraper failed-search allowance is exhausted",
            ));
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "search cancelled"));
        }

        let mut settings = request.settings;
        if settings.region.is_none() {
            settings.region = account.preferred_region.clone();
        }
        let limits = Arc::new(LimitedTransport::new(
            transport,
            requests_left,
            failed_left,
            max_requests_per_minute,
            max_download_speed,
            usize::try_from(account_requests.load(Ordering::Relaxed)).unwrap_or(usize::MAX),
            Arc::clone(cancelled),
        ));
        let client = Client::new(limits.clone(), request.developer, &settings)?;
        let _ = events.send(SearchEvent::Activity("Searching ScreenScraper".into()));
        let response = retry(
            cancelled,
            |error, delay| {
                let _ = events.send(SearchEvent::Activity(retry_status(error, delay)));
            },
            || {
                let reservation = limits.reserve_possible_failure()?;
                let result = client.search(request.system_id, request.term.trim());
                if let Ok(response) = &result {
                    reservation.finish(response.server_miss);
                }
                result
            },
        )?;
        Ok((
            response.alternatives,
            account,
            account_requests
                .load(Ordering::Relaxed)
                .saturating_add(limits.used()),
            limits.failed(),
        ))
    })();

    match result {
        Ok((matches, account, requests_started, failed_searches)) => {
            let _ = events.send(SearchEvent::Finished {
                matches,
                account,
                requests_started,
                failed_searches,
            });
        }
        Err(error) if error.kind == ErrorKind::Cancelled => {
            let _ = events.send(SearchEvent::Cancelled);
        }
        Err(error) => {
            let _ = events.send(SearchEvent::Failed(error));
        }
    }
}

fn run_preview(
    request: PreviewRequest,
    transport: Arc<dyn Transport>,
    cancelled: &Arc<AtomicBool>,
) -> Result<crate::covers::RgbImage> {
    if request.max_download_speed == 0 {
        return Err(Error::new(
            ErrorKind::RateLimited,
            "ScreenScraper reported a zero media-download allowance",
        ));
    }
    let limited = Arc::new(LimitedTransport::new(
        transport,
        u64::MAX,
        u64::MAX,
        usize::MAX,
        request.max_download_speed,
        0,
        Arc::clone(cancelled),
    ));
    let client = Client::new(limited, request.developer, &request.settings)?;
    let response = retry(
        cancelled,
        |_, _| {},
        || client.media(&request.media, request.settings.max_media_bytes()),
    )?;
    decode_media_image(&response, request.ground, request.max_edge)
}

fn run(
    request: Request,
    transport: Arc<dyn Transport>,
    events: &SyncSender<Event>,
    cancelled: &Arc<AtomicBool>,
    selected_match: Option<Match>,
) {
    let mut progress = Progress {
        scope: request.scope_label.clone(),
        phase: Phase::Enumerating,
        ..Default::default()
    };
    emit(events, &progress);

    let mut settings = request.settings.clone();
    let batch = match super::targets::collect(
        &request.systems,
        &request.names,
        &request.scope,
        &settings.system_ids,
        &request.artwork_pack_system_ids,
        cancelled,
        &mut |system| {
            progress.current = system.to_string();
            progress.activity = "Enumerating game folders".to_string();
            emit(events, &progress);
        },
    ) {
        Ok(batch) => batch,
        Err(error) if error.kind == ErrorKind::Cancelled => {
            let _ = events.send(Event::Cancelled(progress));
            return;
        }
        Err(error) => {
            finish_error(events, error, progress);
            return;
        }
    };
    progress.activity.clear();
    progress.total = batch.targets.len();
    progress.skipped_artwork_pack = batch.skipped_artwork_pack.len();
    progress.unsupported_systems = batch.unsupported_systems.len();
    progress.system_errors = batch.issues.len();
    progress.ambiguous_targets = batch.ambiguous_targets;
    for system in &batch.skipped_artwork_pack {
        log_scraper_detail("Artwork Pack system skipped", system);
    }
    for system in &batch.unsupported_systems {
        log_scraper_detail("unsupported system", system);
    }
    for issue in &batch.issues {
        log_scraper_detail(
            &format!("{} could not be enumerated", issue.system),
            &issue.detail,
        );
    }
    if batch.ambiguous_targets > 0 {
        log_scraper_detail(
            "shared library targets skipped",
            &format!(
                "{} target paths mapped to more than one ScreenScraper platform",
                batch.ambiguous_targets
            ),
        );
    }
    progress.last_problem = match (
        batch.unsupported_systems.is_empty(),
        batch.issues.is_empty(),
    ) {
        (false, false) => Some("Some systems or folders were skipped".to_string()),
        (false, true) => Some("Some systems are not supported".to_string()),
        (true, false) => Some("Some game folders could not be read".to_string()),
        (true, true) => None,
    };
    if batch.targets.is_empty()
        && !batch.skipped_artwork_pack.is_empty()
        && scope_has_only_artwork_pack_systems(
            &request.scope,
            &request.systems,
            &request.artwork_pack_system_ids,
        )
    {
        progress.phase = Phase::Finishing;
        progress.activity = format!(
            "{} system{} skipped: Artwork Pack",
            progress.skipped_artwork_pack,
            if progress.skipped_artwork_pack == 1 {
                ""
            } else {
                "s"
            }
        );
        log_scraper_summary("finished", &progress);
        let _ = events.send(Event::Finished(progress));
        return;
    }

    if let Err(error) = settings.validate() {
        finish_error(events, error, progress);
        return;
    }
    if !settings.ready() {
        finish_error(
            events,
            Error::new(
                ErrorKind::Configuration,
                "complete the ScreenScraper login and enable images or metadata",
            ),
            progress,
        );
        return;
    }
    let keep_manual_matches =
        matches!(request.scope, Scope::Game { .. }) && selected_match.is_none();
    let mut work = match plan_work(batch.targets, &settings, &mut progress, cancelled) {
        Ok(work) => work,
        Err(error) if error.kind == ErrorKind::Cancelled => {
            let _ = events.send(Event::Cancelled(progress));
            return;
        }
        Err(error) => {
            finish_error(events, error, progress);
            return;
        }
    };
    if keep_manual_matches {
        for item in &mut work {
            item.retain_alternatives = true;
        }
    }
    if let Some(selected) = selected_match {
        if !matches!(request.scope, Scope::Game { .. }) || work.len() > 1 {
            finish_error(
                events,
                Error::new(
                    ErrorKind::Configuration,
                    "a selected ScreenScraper match can be applied only to one game",
                ),
                progress,
            );
            return;
        }
        if let Some(item) = work.first_mut() {
            item.selected = Some(selected);
        }
    }
    emit(events, &progress);
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled(progress));
        return;
    }
    if work.is_empty() {
        progress.phase = Phase::Finishing;
        log_scraper_summary("finished", &progress);
        let _ = events.send(Event::Finished(progress));
        return;
    }

    progress.phase = Phase::Account;
    emit(events, &progress);
    let Some(developer) = request.developer.clone() else {
        finish_error(
            events,
            Error::new(
                ErrorKind::Configuration,
                "ScreenScraper support is unavailable in this build",
            ),
            progress,
        );
        return;
    };
    let account_client = match Client::new(Arc::clone(&transport), developer.clone(), &settings) {
        Ok(client) => client,
        Err(error) => {
            finish_error(events, error, progress);
            return;
        }
    };
    let account_requests = AtomicU64::new(0);
    let account = match retry(
        cancelled,
        |error, delay| {
            progress.activity = retry_status(error, delay);
            emit(events, &progress);
        },
        || {
            account_requests.fetch_add(1, Ordering::Relaxed);
            account_client.account()
        },
    ) {
        Ok(account) => account,
        Err(error) => {
            finish_error(events, error, progress);
            return;
        }
    };
    if settings.region.is_none() {
        settings.region = account.preferred_region.clone();
    }
    progress.account = Some(account.clone());
    let Some(max_threads) = account.max_threads else {
        finish_error(
            events,
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's worker limit",
            ),
            progress,
        );
        return;
    };
    let Some(max_download_speed) = account.max_download_speed else {
        finish_error(
            events,
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's download-speed limit",
            ),
            progress,
        );
        return;
    };
    let Some(max_requests_per_minute) = account.max_requests_per_minute else {
        finish_error(
            events,
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's per-minute limit",
            ),
            progress,
        );
        return;
    };
    let max_requests_per_minute = match usize::try_from(max_requests_per_minute) {
        Ok(value) => value,
        Err(_) => {
            finish_error(
                events,
                Error::new(
                    ErrorKind::MalformedResponse,
                    "ScreenScraper reported an out-of-range per-minute limit",
                ),
                progress,
            );
            return;
        }
    };
    let Some(requests_left) = account.requests_left() else {
        finish_error(
            events,
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's daily request limit",
            ),
            progress,
        );
        return;
    };
    let Some(failed_left) = account.failed_left() else {
        finish_error(
            events,
            Error::new(
                ErrorKind::MalformedResponse,
                "ScreenScraper did not report the account's failed-search limit",
            ),
            progress,
        );
        return;
    };
    if max_threads == 0 || max_requests_per_minute == 0 {
        finish_error(
            events,
            Error::new(
                ErrorKind::RateLimited,
                "ScreenScraper reported that this account cannot start a request now",
            ),
            progress,
        );
        return;
    }
    if work.iter().any(|item| item.needs.image) && max_download_speed == 0 {
        finish_error(
            events,
            Error::new(
                ErrorKind::RateLimited,
                "ScreenScraper reported that this account cannot download media now",
            ),
            progress,
        );
        return;
    }
    if requests_left == 0 {
        finish_error(
            events,
            Error::new(
                ErrorKind::DailyQuota,
                "the daily ScreenScraper request allowance is exhausted",
            ),
            progress,
        );
        return;
    }
    if failed_left == 0 {
        finish_error(
            events,
            Error::new(
                ErrorKind::FailedQuota,
                "the daily ScreenScraper failed-search allowance is exhausted",
            ),
            progress,
        );
        return;
    }
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled(progress));
        return;
    }

    let requests_as_workers = usize::try_from(requests_left).unwrap_or(usize::MAX);
    let workers = max_threads
        .clamp(1, MAX_WORKERS)
        .min(work.len())
        .min(requests_as_workers);
    progress.workers = workers;
    progress.phase = Phase::Scraping;
    progress.activity.clear();
    emit(events, &progress);

    let limits = Arc::new(LimitedTransport::new(
        transport,
        requests_left,
        failed_left,
        max_requests_per_minute,
        max_download_speed,
        usize::try_from(account_requests.load(Ordering::Relaxed)).unwrap_or(usize::MAX),
        Arc::clone(cancelled),
    ));
    let client = match Client::new(limits.clone(), developer, &settings) {
        Ok(client) => Arc::new(client),
        Err(error) => {
            finish_error(events, error, progress);
            return;
        }
    };
    let mut remaining_by_gamelist: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for item in &work {
        *remaining_by_gamelist
            .entry(item.target.gamelist_path())
            .or_default() += 1;
    }
    let queue = Arc::new(Mutex::new(VecDeque::from(work)));
    let media_gate = Arc::new(MediaGate::new(workers.min(MAX_MEDIA_WORKERS)));
    let stop = Arc::new(AtomicBool::new(false));
    let (results_tx, results_rx) = mpsc::sync_channel(workers.max(1));
    let mut backups = Backups::new();
    let mut pending: BTreeMap<PathBuf, Vec<Pending>> = BTreeMap::new();
    let mut fatal = None;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let client = Arc::clone(&client);
            let limits = Arc::clone(&limits);
            let cancelled = Arc::clone(cancelled);
            let stop = Arc::clone(&stop);
            let media_gate = Arc::clone(&media_gate);
            let results = results_tx.clone();
            let worker_settings = settings.clone();
            scope.spawn(move || {
                loop {
                    if cancelled.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let work = queue
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .pop_front();
                    let Some(work) = work else {
                        break;
                    };
                    let gamelist_path = work.target.gamelist_path();
                    let target_label = current_label(&work.target);
                    activity(&results, &work.target, "Checking local game");
                    let result = prepare(
                        work,
                        &client,
                        &limits,
                        &worker_settings,
                        &media_gate,
                        &cancelled,
                        &results,
                    );
                    if let Err(error) = &result {
                        if error.kind != ErrorKind::Cancelled {
                            log_scraper_problem(&target_label, error);
                        }
                    }
                    if results
                        .send(WorkerMessage::Target {
                            gamelist_path,
                            result: Box::new(result),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = results.send(WorkerMessage::Done);
            });
        }
        drop(results_tx);

        let mut done = 0usize;
        while done < workers {
            let Ok(message) = results_rx.recv() else {
                break;
            };
            match message {
                WorkerMessage::Done => done += 1,
                WorkerMessage::Activity { current, status } => {
                    progress.current = current;
                    progress.activity = status;
                    emit(events, &progress);
                }
                WorkerMessage::Target {
                    gamelist_path,
                    result,
                } => {
                    progress.completed += 1;
                    progress.activity = "Recording result".to_string();
                    match *result {
                        Ok(prepared) => {
                            progress.current = current_label(&prepared.target);
                            progress.failed_searches = limits.failed();
                            let target_label = current_label(&prepared.target);
                            match stage(prepared, &settings, &mut progress) {
                                Ok(Some(item)) => {
                                    pending
                                        .entry(item.target.gamelist_path())
                                        .or_default()
                                        .push(item);
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    progress.failed += 1;
                                    log_scraper_problem(&target_label, &error);
                                    progress.last_problem = Some(error.user_message().to_string());
                                    if fatal_for_run(&error) {
                                        let cancel = cancel_in_flight(&error);
                                        fatal.get_or_insert(error);
                                        stop.store(true, Ordering::Relaxed);
                                        if cancel {
                                            cancelled.store(true, Ordering::Relaxed);
                                        }
                                    }
                                }
                            }
                        }
                        Err(error) if error.kind == ErrorKind::Cancelled => {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                        Err(error) => {
                            progress.failed += 1;
                            progress.last_problem = Some(error.user_message().to_string());
                            if fatal_for_run(&error) {
                                let cancel = cancel_in_flight(&error);
                                fatal.get_or_insert(error);
                                stop.store(true, Ordering::Relaxed);
                                if cancel {
                                    cancelled.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    progress.requests_started = limits.used();
                    progress.failed_searches = limits.failed();
                    let gamelist_complete = remaining_by_gamelist
                        .get_mut(&gamelist_path)
                        .is_some_and(|remaining| {
                            *remaining = remaining.saturating_sub(1);
                            *remaining == 0
                        });
                    if gamelist_complete {
                        remaining_by_gamelist.remove(&gamelist_path);
                        if let Some(items) = pending.remove(&gamelist_path) {
                            progress.activity = "Writing gamelist".to_string();
                            flush_one_gamelist(
                                gamelist_path,
                                items,
                                &settings,
                                &mut backups,
                                &mut progress,
                            );
                        }
                    }
                    emit(events, &progress);
                }
            }
        }
    });

    progress.phase = Phase::Finishing;
    progress.activity.clear();
    progress.requests_started = limits.used();
    progress.failed_searches = limits.failed();
    emit(events, &progress);
    flush_pending(pending, &settings, &mut backups, &mut progress, events);
    if let Some(error) = fatal {
        finish_error(events, error, progress);
    } else if cancelled.load(Ordering::Relaxed) {
        log_scraper_summary("cancelled", &progress);
        let _ = events.send(Event::Cancelled(progress));
    } else {
        log_scraper_summary("finished", &progress);
        let _ = events.send(Event::Finished(progress));
    }
}

fn scope_has_only_artwork_pack_systems(
    scope: &Scope,
    systems: &[FoundSystem],
    artwork_pack_system_ids: &HashSet<String>,
) -> bool {
    let selected = |system_id: &str| {
        artwork_pack_system_ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(system_id))
    };
    match scope {
        Scope::All => {
            let mut candidates = systems
                .iter()
                .filter(|system| !super::targets::is_favorites_target(system));
            let Some(first) = candidates.next() else {
                return false;
            };
            selected(&first.def.id) && candidates.all(|system| selected(&system.def.id))
        }
        Scope::System { system_id, .. }
        | Scope::Folder { system_id, .. }
        | Scope::Game { system_id, .. } => selected(system_id),
    }
}

#[derive(Debug)]
struct Work {
    target: Target,
    needs: Needs,
    selected: Option<Match>,
    retain_alternatives: bool,
}

fn plan_work(
    targets: Vec<Target>,
    settings: &ScraperSettings,
    progress: &mut Progress,
    cancelled: &AtomicBool,
) -> Result<Vec<Work>> {
    let mut grouped: BTreeMap<PathBuf, Vec<Target>> = BTreeMap::new();
    for target in targets {
        grouped
            .entry(target.gamelist_path())
            .or_default()
            .push(target);
    }
    let mut work = Vec::new();
    for (gamelist_path, targets) in grouped {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
        }
        let relative_paths: Vec<String> = targets
            .iter()
            .map(|target| target.relative_path.clone())
            .collect();
        let outcomes = gamelist_edit::needs_many_with_fallback(
            &gamelist_path,
            &targets[0].folder,
            &relative_paths,
            &targets
                .iter()
                .map(|target| target.metadata_fallback)
                .collect::<Vec<_>>(),
            settings.image_policy,
            settings.metadata_policy,
        );
        match outcomes {
            Ok(outcomes) => {
                for (target, outcome) in targets.into_iter().zip(outcomes) {
                    match outcome {
                        Ok(needs) if needs.any() => work.push(Work {
                            target,
                            needs,
                            selected: None,
                            retain_alternatives: false,
                        }),
                        Ok(_) => {
                            progress.completed += 1;
                            progress.unchanged += 1;
                            progress.current = target.title;
                        }
                        Err(error) => {
                            progress.completed += 1;
                            progress.failed += 1;
                            let target_label = current_label(&target);
                            progress.current = target.title;
                            log_scraper_problem(&target_label, &error);
                            progress.last_problem = Some(error.user_message().to_string());
                        }
                    }
                }
            }
            Err(error) => {
                progress.completed += targets.len();
                progress.failed += targets.len();
                progress.current = targets
                    .last()
                    .map(|target| target.title.clone())
                    .unwrap_or_default();
                log_scraper_problem(&gamelist_path.display().to_string(), &error);
                progress.last_problem = Some(error.user_message().to_string());
            }
        }
    }
    Ok(work)
}

fn emit(events: &SyncSender<Event>, progress: &Progress) {
    let _ = events.try_send(Event::Progress(progress.clone()));
}

fn finish_error(events: &SyncSender<Event>, error: Error, progress: Progress) {
    log_scraper_problem("run stopped", &error);
    log_scraper_summary("failed", &progress);
    let _ = events.send(Event::Failed { error, progress });
}

struct LimitedTransport {
    inner: Arc<dyn Transport>,
    requests_left: u64,
    failed_left: u64,
    used: AtomicU64,
    failed: AtomicU64,
    possible_failures: AtomicU64,
    rate: RateGate,
    max_download_speed: u64,
    cancelled: Arc<AtomicBool>,
}

impl LimitedTransport {
    fn new(
        inner: Arc<dyn Transport>,
        requests_left: u64,
        failed_left: u64,
        per_minute: usize,
        max_download_speed: u64,
        initial_requests: usize,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        Self {
            inner,
            requests_left,
            failed_left,
            used: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            possible_failures: AtomicU64::new(0),
            rate: RateGate::new(per_minute, initial_requests),
            max_download_speed,
            cancelled,
        }
    }

    fn claim(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
        }
        self.rate.wait(&self.cancelled)?;
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                if used < self.requests_left {
                    Some(used + 1)
                } else {
                    None
                }
            })
            .map(|_| ())
            .map_err(|_| {
                Error::new(
                    ErrorKind::DailyQuota,
                    "the daily request allowance is exhausted",
                )
            })
    }

    /// Hold one failed-search slot for every lookup whose result is not yet
    /// known. Without this reservation, several workers can all start while
    /// one KO allowance remains and predictably overrun it together.
    fn reserve_possible_failure(&self) -> Result<FailureReservation<'_>> {
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
            }
            let failed = self.failed.load(Ordering::Relaxed);
            if failed >= self.failed_left {
                return Err(Error::new(
                    ErrorKind::FailedQuota,
                    "the failed-search allowance is exhausted",
                ));
            }
            if self
                .possible_failures
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |pending| {
                    (failed.saturating_add(pending) < self.failed_left)
                        .then_some(pending.saturating_add(1))
                })
                .is_ok()
            {
                return Ok(FailureReservation {
                    limits: self,
                    active: true,
                });
            }
            // Every remaining KO slot is held by an in-flight lookup. One
            // successful result releases a slot; a miss makes the next loop
            // report the real exhaustion.
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

struct FailureReservation<'a> {
    limits: &'a LimitedTransport,
    active: bool,
}

impl FailureReservation<'_> {
    fn finish(mut self, failed: bool) {
        if failed {
            self.limits.failed.fetch_add(1, Ordering::Relaxed);
        }
        self.limits
            .possible_failures
            .fetch_sub(1, Ordering::Relaxed);
        self.active = false;
    }
}

impl Drop for FailureReservation<'_> {
    fn drop(&mut self) {
        if self.active {
            self.limits
                .possible_failures
                .fetch_sub(1, Ordering::Relaxed);
        }
    }
}

fn limited_lookup(
    limits: &LimitedTransport,
    operation: impl FnOnce() -> Result<super::api::LookupResponse>,
) -> Result<super::api::LookupResponse> {
    let reservation = limits.reserve_possible_failure()?;
    let result = operation();
    if let Ok(response) = &result {
        reservation.finish(response.server_miss);
    }
    result
}

impl Transport for LimitedTransport {
    fn get(&self, endpoint: &str, params: &[(String, String)], limit: u64) -> Result<HttpResponse> {
        self.claim()?;
        self.inner.get(endpoint, params, limit)
    }

    fn get_media(
        &self,
        url: &str,
        limit: u64,
        _max_kib_per_second: Option<u64>,
    ) -> Result<HttpResponse> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
        }
        self.inner
            .get_media(url, limit, Some(self.max_download_speed))
    }
}

struct RateGate {
    per_minute: usize,
    window: Duration,
    starts: Mutex<VecDeque<Instant>>,
}

impl RateGate {
    fn new(per_minute: usize, initial_requests: usize) -> Self {
        let now = Instant::now();
        Self {
            per_minute,
            window: Duration::from_secs(60),
            starts: Mutex::new(std::iter::repeat_n(now, initial_requests).collect()),
        }
    }

    #[cfg(test)]
    fn with_window(per_minute: usize, window: Duration) -> Self {
        Self {
            per_minute,
            window,
            starts: Mutex::new(VecDeque::new()),
        }
    }

    #[cfg(test)]
    fn with_window_and_initial(
        per_minute: usize,
        window: Duration,
        initial_requests: usize,
    ) -> Self {
        let now = Instant::now();
        Self {
            per_minute,
            window,
            starts: Mutex::new(std::iter::repeat_n(now, initial_requests).collect()),
        }
    }

    fn wait(&self, cancelled: &AtomicBool) -> Result<()> {
        let limit = self.per_minute;
        if limit == 0 {
            return Err(Error::new(
                ErrorKind::RateLimited,
                "ScreenScraper reported a zero per-minute allowance",
            ));
        }
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
            }
            let wait = {
                let now = Instant::now();
                let mut starts = self
                    .starts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while starts
                    .front()
                    .is_some_and(|started| now.duration_since(*started) >= self.window)
                {
                    starts.pop_front();
                }
                if starts.len() < limit {
                    starts.push_back(now);
                    return Ok(());
                }
                starts
                    .front()
                    .map(|started| {
                        self.window
                            .saturating_sub(now.duration_since(*started))
                            .min(Duration::from_millis(100))
                    })
                    .unwrap_or(Duration::from_millis(20))
            };
            std::thread::sleep(wait.max(Duration::from_millis(10)));
        }
    }
}

enum WorkerMessage {
    Activity {
        current: String,
        status: String,
    },
    Target {
        gamelist_path: PathBuf,
        result: Box<Result<Prepared>>,
    },
    Done,
}

struct Prepared {
    target: Target,
    matched: Option<Match>,
    media: Option<InstalledMedia>,
    media_error: Option<Error>,
    no_media: bool,
    ambiguous: Option<usize>,
    not_found: bool,
    alternatives: Vec<Match>,
}

struct MediaGate {
    available: Mutex<usize>,
}

impl MediaGate {
    fn new(permits: usize) -> Self {
        Self {
            available: Mutex::new(permits.max(1)),
        }
    }

    fn acquire(self: &Arc<Self>, cancelled: &AtomicBool) -> Result<MediaPermit> {
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
            }
            {
                let mut available = self
                    .available
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if *available > 0 {
                    *available -= 1;
                    return Ok(MediaPermit {
                        gate: Arc::clone(self),
                    });
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

struct MediaPermit {
    gate: Arc<MediaGate>,
}

impl Drop for MediaPermit {
    fn drop(&mut self) {
        let mut available = self
            .gate
            .available
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *available = available.saturating_add(1);
    }
}

fn prepare(
    work: Work,
    client: &Client,
    limits: &LimitedTransport,
    settings: &ScraperSettings,
    media_gate: &Arc<MediaGate>,
    cancelled: &AtomicBool,
    events: &SyncSender<WorkerMessage>,
) -> Result<Prepared> {
    let Work {
        target,
        needs,
        selected,
        retain_alternatives,
    } = work;
    let current = current_label(&target);
    let lookup = if let Some(matched) = selected {
        activity_text(events, &current, "Using selected match");
        super::api::LookupResponse {
            lookup: Lookup::Found(Box::new(matched)),
            alternatives: Vec::new(),
            account: None,
            server_miss: false,
        }
    } else if let Some(path) = target.match_path.as_deref() {
        activity_text(events, &current, "Hashing ROM");
        match hashes::file(path, settings.hash_limit_bytes(), cancelled)? {
            Some(hashes) => {
                activity_text(events, &current, "Matching by hash");
                let result = retry(
                    cancelled,
                    |error, delay| retry_activity(events, &current, error, delay),
                    || {
                        limited_lookup(limits, || {
                            client.by_hash(
                                target.screen_scraper_system_id,
                                target.file_name(),
                                &hashes,
                            )
                        })
                    },
                )?;
                match result.lookup {
                    Lookup::NotFound => {
                        activity_text(events, &current, "Matching by name");
                        retry(
                            cancelled,
                            |error, delay| retry_activity(events, &current, error, delay),
                            || {
                                limited_lookup(limits, || {
                                    client.by_name(target.screen_scraper_system_id, &target.query)
                                })
                            },
                        )?
                    }
                    _ => result,
                }
            }
            None => {
                activity_text(events, &current, "Matching by name");
                retry(
                    cancelled,
                    |error, delay| retry_activity(events, &current, error, delay),
                    || {
                        limited_lookup(limits, || {
                            client.by_name(target.screen_scraper_system_id, &target.query)
                        })
                    },
                )?
            }
        }
    } else {
        activity_text(events, &current, "Matching by name");
        retry(
            cancelled,
            |error, delay| retry_activity(events, &current, error, delay),
            || {
                limited_lookup(limits, || {
                    client.by_name(target.screen_scraper_system_id, &target.query)
                })
            },
        )?
    };

    let alternatives = if retain_alternatives {
        lookup.alternatives
    } else {
        Vec::new()
    };
    match lookup.lookup {
        Lookup::NotFound => Ok(Prepared {
            target,
            matched: None,
            media: None,
            media_error: None,
            no_media: false,
            ambiguous: None,
            not_found: true,
            alternatives,
        }),
        Lookup::Ambiguous(count) => Ok(Prepared {
            target,
            matched: None,
            media: None,
            media_error: None,
            no_media: false,
            ambiguous: Some(count),
            not_found: false,
            alternatives,
        }),
        Lookup::Found(matched) => {
            let (media, media_error, no_media) = if needs.image {
                match matched.media.as_ref() {
                    Some(media) => {
                        let _permit = media_gate.acquire(cancelled)?;
                        activity_text(events, &current, "Downloading image");
                        match retry(
                            cancelled,
                            |error, delay| retry_activity(events, &current, error, delay),
                            || client.media(media, settings.max_media_bytes()),
                        ) {
                            Ok(response) => match install_media(&target, &matched, response) {
                                Ok(path) => (Some(path), None, false),
                                Err(error) => (None, Some(error), false),
                            },
                            Err(error) if error.kind == ErrorKind::NotFound => (None, None, true),
                            Err(error) if error.kind == ErrorKind::MalformedResponse => {
                                (None, Some(error), false)
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    None => (None, None, true),
                }
            } else {
                (None, None, false)
            };
            Ok(Prepared {
                target,
                matched: Some(*matched),
                media,
                media_error,
                no_media,
                ambiguous: None,
                not_found: false,
                alternatives: Vec::new(),
            })
        }
    }
}

fn retry<T>(
    cancelled: &AtomicBool,
    mut on_retry: impl FnMut(&Error, Duration),
    mut operation: impl FnMut() -> Result<T>,
) -> Result<T> {
    for attempt in 0..RETRIES {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
        }
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable() && attempt + 1 < RETRIES => {
                let delay = retry_delay(error.kind, attempt);
                on_retry(&error, delay);
                let until = Instant::now() + delay;
                while Instant::now() < until {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(Error::new(ErrorKind::Cancelled, "scrape cancelled"));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the retry loop always returns")
}

fn retry_delay(kind: ErrorKind, attempt: usize) -> Duration {
    if kind == ErrorKind::RateLimited {
        [Duration::from_secs(5), Duration::from_secs(15)]
            .get(attempt)
            .copied()
            .unwrap_or(Duration::from_secs(15))
    } else {
        Duration::from_millis(250).saturating_mul(1_u32 << attempt.min(3))
    }
}

fn current_label(target: &Target) -> String {
    format!("{}: {}", target.system_name, target.title)
}

fn retry_status(error: &Error, delay: Duration) -> String {
    let seconds = delay.as_secs();
    if seconds == 0 {
        format!("Retrying after {}", error.kind.label())
    } else {
        format!("Retrying in {seconds}s after {}", error.kind.label())
    }
}

fn activity(events: &SyncSender<WorkerMessage>, target: &Target, status: &str) {
    activity_text(events, &current_label(target), status);
}

fn activity_text(events: &SyncSender<WorkerMessage>, current: &str, status: &str) {
    let _ = events.send(WorkerMessage::Activity {
        current: current.to_string(),
        status: status.to_string(),
    });
}

fn retry_activity(
    events: &SyncSender<WorkerMessage>,
    current: &str,
    error: &Error,
    delay: Duration,
) {
    activity_text(events, current, &retry_status(error, delay));
}

struct Pending {
    target: Target,
    metadata: super::Metadata,
    media: Option<InstalledMedia>,
}

fn stage(
    prepared: Prepared,
    settings: &ScraperSettings,
    progress: &mut Progress,
) -> Result<Option<Pending>> {
    let target_label = current_label(&prepared.target);
    if prepared.not_found {
        log_scraper_detail(&target_label, "no exact ScreenScraper match; skipped");
        if !prepared.alternatives.is_empty() {
            progress.manual_matches = prepared.alternatives;
        }
        progress.not_found += 1;
        return Ok(None);
    }
    if let Some(count) = prepared.ambiguous {
        log_scraper_detail(
            &target_label,
            &format!("{count} automatic ScreenScraper matches; skipped"),
        );
        if !prepared.alternatives.is_empty() {
            progress.manual_matches = prepared.alternatives;
        }
        progress.ambiguous += 1;
        return Ok(None);
    }
    let matched = prepared
        .matched
        .ok_or_else(|| Error::local("a completed scrape had no match result"))?;
    let media_failed = prepared.media_error.is_some();
    if let Some(error) = prepared.media_error {
        progress.failed += 1;
        log_scraper_problem(&target_label, &error);
        progress.last_problem = Some(error.user_message().to_string());
        if settings.metadata_policy == super::MetadataPolicy::Off {
            return Ok(None);
        }
    }
    if prepared.no_media {
        log_scraper_detail(
            &target_label,
            "the match has no selected ScreenScraper image",
        );
        progress.no_media += 1;
    }
    let metadata_selected =
        settings.metadata_policy != super::MetadataPolicy::Off && matched.metadata.has_value();
    if prepared.media.is_none() && !metadata_selected {
        if !media_failed {
            progress.unchanged += 1;
        }
        return Ok(None);
    }
    Ok(Some(Pending {
        target: prepared.target,
        metadata: matched.metadata,
        media: prepared.media,
    }))
}

fn flush_pending(
    grouped: BTreeMap<PathBuf, Vec<Pending>>,
    settings: &ScraperSettings,
    backups: &mut Backups,
    progress: &mut Progress,
    events: &SyncSender<Event>,
) {
    for (gamelist_path, pending) in grouped {
        flush_one_gamelist(gamelist_path, pending, settings, backups, progress);
        emit(events, progress);
    }
}

fn flush_one_gamelist(
    gamelist_path: PathBuf,
    pending: Vec<Pending>,
    settings: &ScraperSettings,
    backups: &mut Backups,
    progress: &mut Progress,
) {
    let updates: Vec<Update> = pending
        .iter()
        .map(|item| Update {
            relative_game_path: item.target.relative_path.clone(),
            metadata: item.metadata.clone(),
            image_path: item.media.as_ref().map(|media| media.relative_path.clone()),
            image_created: item.media.as_ref().is_some_and(|media| media.created),
            image_policy: settings.image_policy,
            metadata_policy: settings.metadata_policy,
        })
        .collect();
    match gamelist_edit::apply_many_with_fallback(
        &gamelist_path,
        &pending[0].target.folder,
        &updates,
        &pending
            .iter()
            .map(|item| item.target.metadata_fallback)
            .collect::<Vec<_>>(),
        backups,
    ) {
        Ok(outcomes) => {
            for (item, outcome) in pending.iter().zip(outcomes) {
                progress.current = item.target.title.clone();
                match outcome {
                    Ok(change) if change.changed() => {
                        progress.updated += 1;
                        for system_id in &item.target.affected_system_ids {
                            if !progress
                                .updated_systems
                                .iter()
                                .any(|system| system == system_id)
                            {
                                progress.updated_systems.push(system_id.clone());
                            }
                        }
                    }
                    Ok(_) => progress.unchanged += 1,
                    Err(error) => {
                        progress.failed += 1;
                        log_scraper_problem(&current_label(&item.target), &error);
                        progress.last_problem = Some(error.user_message().to_string());
                    }
                }
            }
        }
        Err(error) => {
            progress.failed += pending.len();
            progress.current = pending
                .last()
                .map(|item| item.target.title.clone())
                .unwrap_or_default();
            log_scraper_problem(&gamelist_path.display().to_string(), &error);
            progress.last_problem = Some(error.user_message().to_string());
        }
    }
    cleanup_unreferenced_media(&gamelist_path, &pending, progress);
}

#[derive(Debug)]
struct InstalledMedia {
    relative_path: String,
    absolute_path: PathBuf,
    created: bool,
}

fn cleanup_unreferenced_media(gamelist_path: &Path, pending: &[Pending], progress: &mut Progress) {
    let mut checked = HashSet::new();
    let created: Vec<_> = pending
        .iter()
        .filter_map(|item| item.media.as_ref())
        .filter(|media| media.created)
        .filter(|media| checked.insert(media.absolute_path.clone()))
        .collect();
    if created.is_empty() {
        return;
    }

    let references = match gamelist_edit::referenced_art_paths(gamelist_path) {
        Ok(references) => references,
        Err(error) => {
            progress.failed += 1;
            log_scraper_problem(&gamelist_path.display().to_string(), &error);
            progress.last_problem = Some("Downloaded artwork cleanup could not finish".to_string());
            return;
        }
    };
    for media in created {
        if !references.iter().any(|reference| {
            same_media_reference(reference, &normalise_media_reference(&media.relative_path))
        }) {
            match std::fs::remove_file(&media.absolute_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    progress.failed += 1;
                    let detail = format!("could not remove unused downloaded media: {error}");
                    log_scraper_detail(&media.absolute_path.display().to_string(), &detail);
                    progress.last_problem =
                        Some("Downloaded artwork cleanup could not finish".to_string());
                }
            }
        }
    }
}

fn normalise_media_reference(value: &str) -> String {
    let value = value.trim().replace('\\', "/");
    value
        .strip_prefix("./")
        .unwrap_or(&value)
        .trim_start_matches('/')
        .to_string()
}

fn same_media_reference(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn install_media(
    target: &Target,
    matched: &Match,
    response: HttpResponse,
) -> Result<InstalledMedia> {
    let format = media_format(&response)?;
    decode_media_image(&response, [0, 0, 0], u32::MAX)?;

    let id = safe_component(&matched.id);
    let checksum = Sha1::digest(&response.body);
    let checksum = checksum
        .iter()
        .flat_map(|byte| {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            [
                char::from(HEX[(byte >> 4) as usize]),
                char::from(HEX[(byte & 0x0f) as usize]),
            ]
        })
        .collect::<String>();
    let file_name = format!(
        "{}-{id}-{checksum}.{format}",
        target.screen_scraper_system_id,
    );
    let relative = format!("./media/screenscraper/{file_name}");
    let directory = target.folder.join("media/screenscraper");
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::local(format!(
            "could not create the ScreenScraper media folder: {error}"
        ))
    })?;
    let destination = directory.join(&file_name);
    if destination.is_file() {
        let matches = std::fs::metadata(&destination)
            .ok()
            .is_some_and(|metadata| metadata.len() == response.body.len() as u64)
            && std::fs::read(&destination)
                .ok()
                .is_some_and(|existing| existing == response.body);
        if !matches {
            return Err(Error::local(
                "an existing content-named ScreenScraper image has different bytes",
            ));
        }
        return Ok(InstalledMedia {
            relative_path: relative,
            absolute_path: destination,
            created: false,
        });
    }
    let sequence = MEDIA_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = directory.join(format!(
        ".{file_name}.{}-{sequence}.part",
        std::process::id()
    ));
    let outcome = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| Error::local(format!("could not create downloaded media: {error}")))?;
        file.write_all(&response.body)
            .map_err(|error| Error::local(format!("could not write downloaded media: {error}")))?;
        file.sync_all()
            .map_err(|error| Error::local(format!("could not sync downloaded media: {error}")))?;
        match std::fs::rename(&temp, &destination) {
            Ok(()) => Ok(true),
            Err(_) if destination.is_file() => {
                let _ = std::fs::remove_file(&temp);
                Ok(false)
            }
            Err(error) => Err(Error::local(format!(
                "could not install downloaded media: {error}"
            ))),
        }
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    outcome.map(|created| InstalledMedia {
        relative_path: relative,
        absolute_path: destination,
        created,
    })
}

fn media_format(response: &HttpResponse) -> Result<&'static str> {
    let format = if response.body.starts_with(&[0xff, 0xd8]) {
        "jpg"
    } else if response.body.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else {
        return Err(Error::new(
            ErrorKind::MalformedResponse,
            "ScreenScraper returned media that is not a PNG or JPEG",
        ));
    };
    Ok(format)
}

fn decode_media_image(
    response: &HttpResponse,
    ground: [u8; 3],
    max_edge: u32,
) -> Result<crate::covers::RgbImage> {
    let _ = media_format(response)?;
    let decoded = crate::covers::decode_bounded(
        &response.body,
        Path::new("ScreenScraper download"),
        ground,
        MAX_MEDIA_DIMENSION,
    )
    .map_err(|error| {
        Error::new(
            ErrorKind::MalformedResponse,
            format!("ScreenScraper returned an invalid image: {error}"),
        )
    })?;
    Ok(crate::covers::scale_to_fit(&decoded, max_edge))
}

fn safe_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "game".into()
    } else {
        cleaned
    }
}

fn fatal_for_run(error: &Error) -> bool {
    !matches!(error.kind, ErrorKind::Local | ErrorKind::NotFound)
}

fn cancel_in_flight(error: &Error) -> bool {
    !matches!(error.kind, ErrorKind::DailyQuota | ErrorKind::FailedQuota)
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn log_scraper_detail(context: &str, detail: &str) {
    crate::note(&format!(
        "scraper      {}: {}",
        one_line(context),
        one_line(detail)
    ));
}

fn log_scraper_problem(context: &str, error: &Error) {
    log_scraper_detail(context, &error.to_string());
}

fn log_scraper_summary(outcome: &str, progress: &Progress) {
    log_scraper_detail(
        &format!("run {outcome}"),
        &format!(
            "{}: {}/{} completed, {} written, {} unchanged, {} missing, {} ambiguous, {} without image, {} Artwork Pack, {} unsupported, {} shared, {} system, {} failed",
            one_line(&progress.scope),
            progress.completed,
            progress.total,
            progress.updated,
            progress.unchanged,
            progress.not_found,
            progress.ambiguous,
            progress.no_media,
            progress.skipped_artwork_pack,
            progress.unsupported_systems,
            progress.ambiguous_targets,
            progress.system_errors,
            progress.failed,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::{ImagePolicy, MetadataPolicy};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Barrier;

    const PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 8, 215, 99, 248, 207, 192, 0, 0,
        3, 1, 1, 0, 24, 221, 141, 24, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    struct Mock {
        api_calls: AtomicUsize,
        media_calls: AtomicUsize,
        quota_empty: bool,
        bad_media: bool,
        no_media: bool,
    }

    impl Transport for Mock {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            self.api_calls.fetch_add(1, Ordering::Relaxed);
            let body = match endpoint {
                "ssuserInfos.php" => format!(
                    "<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>{}</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>10</maxrequestsperday><maxrequestskoperday>10</maxrequestskoperday></ssuser></Data>",
                    if self.quota_empty { 10 } else { 0 }
                ),
                "jeuInfos.php" => {
                    let md5 = params
                        .iter()
                        .find(|(key, _)| key == "md5")
                        .map(|(_, value)| value.as_str())
                        .unwrap_or_default();
                    format!(
                        "<Data><jeux><jeu id=\"42\"><noms><nom region=\"wor\">Remote Game</nom></noms><synopsis><synopsis langue=\"en\">Remote description</synopsis></synopsis><rom><rommd5>{md5}</rommd5></rom><medias><media type=\"ss\" region=\"wor\" format=\"png\">https://media.screenscraper.fr/42.png</media></medias></jeu></jeux></Data>"
                    )
                }
                other => panic!("unexpected endpoint {other}"),
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: body.into_bytes(),
            })
        }

        fn get_media(
            &self,
            _url: &str,
            _limit: u64,
            max_kib_per_second: Option<u64>,
        ) -> Result<HttpResponse> {
            assert_eq!(max_kib_per_second, Some(256));
            self.media_calls.fetch_add(1, Ordering::Relaxed);
            Ok(HttpResponse {
                status: if self.no_media { 404 } else { 200 },
                content_type: Some(if self.no_media {
                    "text/plain".into()
                } else {
                    "image/png".into()
                }),
                body: if self.no_media {
                    b"NOMEDIA".to_vec()
                } else if self.bad_media {
                    b"not-an-image".to_vec()
                } else {
                    PNG.to_vec()
                },
            })
        }
    }

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-scraper-worker-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn system(root: &Path) -> FoundSystem {
        FoundSystem {
            def: toml::from_str(
                "name = 'Nintendo'\nid = 'NES'\nfolders = ['NES']\nrbf = '_Console/NES'\nextensions = ['rom']\n",
            )
            .unwrap(),
            paths: vec![root.to_path_buf()],
            logo_dir: None,
            menu_folder: Some("Console".into()),
        }
    }

    #[test]
    fn all_artwork_pack_scope_ignores_renamed_favourites_categories() {
        let ordinary = system(Path::new("/games/NES"));
        for category in ["Favorites", "favorites", "FAVORITES"] {
            let mut favorite = system(Path::new("/games/MyShelf"));
            favorite.def.id = "MyShelf".into();
            favorite.def.category = Some(category.into());
            favorite.menu_folder = None;
            assert!(scope_has_only_artwork_pack_systems(
                &Scope::All,
                &[ordinary.clone(), favorite.clone()],
                &HashSet::from(["NES".into()])
            ));
            assert!(!scope_has_only_artwork_pack_systems(
                &Scope::All,
                &[favorite],
                &HashSet::from(["MyShelf".into()])
            ));
            assert!(!scope_has_only_artwork_pack_systems(
                &Scope::All,
                std::slice::from_ref(&ordinary),
                &HashSet::new()
            ));
        }
    }

    #[test]
    fn rate_limit_retries_wait_longer_than_transient_network_retries() {
        assert_eq!(
            retry_delay(ErrorKind::Transport, 0),
            Duration::from_millis(250)
        );
        assert_eq!(
            retry_delay(ErrorKind::Transport, 1),
            Duration::from_millis(500)
        );
        assert_eq!(
            retry_delay(ErrorKind::RateLimited, 0),
            Duration::from_secs(5)
        );
        assert_eq!(
            retry_delay(ErrorKind::RateLimited, 1),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn request_start_gate_enforces_its_window_and_remains_cancellable() {
        let gate = RateGate::with_window(1, Duration::from_millis(40));
        let cancelled = AtomicBool::new(false);
        gate.wait(&cancelled).unwrap();
        let started = Instant::now();
        gate.wait(&cancelled).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(30));

        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(
            gate.wait(&cancelled).unwrap_err().kind,
            ErrorKind::Cancelled
        );
    }

    #[test]
    fn account_preflight_request_is_included_in_the_first_rate_window() {
        let gate = RateGate::with_window_and_initial(1, Duration::from_millis(40), 1);
        let cancelled = AtomicBool::new(false);
        let started = Instant::now();
        gate.wait(&cancelled).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn one_remaining_failed_search_slot_waits_then_is_reused_or_consumed() {
        let limits = LimitedTransport::new(
            Arc::new(Mock {
                api_calls: AtomicUsize::new(0),
                media_calls: AtomicUsize::new(0),
                quota_empty: false,
                bad_media: false,
                no_media: false,
            }),
            10,
            1,
            60,
            256,
            0,
            Arc::new(AtomicBool::new(false)),
        );

        let first = limits.reserve_possible_failure().unwrap();
        let released = AtomicBool::new(false);
        let started = Barrier::new(2);
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| {
                started.wait();
                let reservation = limits.reserve_possible_failure().unwrap();
                released.store(true, Ordering::Relaxed);
                reservation.finish(false);
            });
            started.wait();
            std::thread::sleep(Duration::from_millis(40));
            assert!(!released.load(Ordering::Relaxed));
            first.finish(false);
            waiter.join().unwrap();
        });
        assert!(released.load(Ordering::Relaxed));

        let last = limits.reserve_possible_failure().unwrap();
        last.finish(true);
        assert_eq!(limits.failed(), 1);
        assert_eq!(
            limits.reserve_possible_failure().err().unwrap().kind,
            ErrorKind::FailedQuota
        );
    }

    struct ConcurrencyMock {
        active: AtomicUsize,
        maximum: AtomicUsize,
    }

    impl Transport for ConcurrencyMock {
        fn get(
            &self,
            endpoint: &str,
            _params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            let body = if endpoint == "ssuserInfos.php" {
                "<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>20</maxrequestsperday><maxrequestskoperday>20</maxrequestskoperday></ssuser></Data>".to_string()
            } else {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum.fetch_max(active, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                self.active.fetch_sub(1, Ordering::SeqCst);
                "<Data><jeux><jeu id=\"42\"><noms><nom region=\"wor\">Remote Game</nom></noms></jeu></jeux></Data>".to_string()
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: body.into_bytes(),
            })
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
    fn account_worker_limit_caps_parallel_lookups() {
        let root = temp("worker-limit");
        for index in 0..6 {
            std::fs::write(root.join(format!("Game {index}.rom")), [index]).unwrap();
        }
        let mock = Arc::new(ConcurrencyMock {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
        });
        let event = finish(
            start_with_transport(request(&root, settings(ImagePolicy::Off)), mock.clone()).unwrap(),
        );
        assert!(matches!(event, Event::Finished(_)));
        assert_eq!(mock.maximum.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn progress_identifies_the_active_system_game_and_operation() {
        let root = temp("progress-detail");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let job = start_with_transport(request(&root, settings(ImagePolicy::Off)), mock).unwrap();
        let mut saw_enumeration = false;
        let mut saw_detail = false;
        loop {
            match job.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::Progress(progress) => {
                    assert_eq!(progress.scope, "Nintendo");
                    if progress.current == "Nintendo"
                        && progress.activity == "Enumerating game folders"
                    {
                        saw_enumeration = true;
                    }
                    if progress.current == "Nintendo: Game"
                        && matches!(
                            progress.activity.as_str(),
                            "Checking local game" | "Hashing ROM" | "Matching by hash"
                        )
                    {
                        saw_detail = true;
                    }
                }
                Event::Finished(_) => break,
                Event::Cancelled(_) | Event::Failed { .. } => {
                    panic!("progress test scrape did not finish")
                }
            }
        }
        assert!(saw_enumeration);
        assert!(saw_detail);
        let _ = std::fs::remove_dir_all(root);
    }

    fn settings(image_policy: ImagePolicy) -> ScraperSettings {
        ScraperSettings {
            username: "user".into(),
            password: "password".into(),
            accepted_plaintext_warning: true,
            image_policy,
            ..Default::default()
        }
    }

    fn request(root: &Path, settings: ScraperSettings) -> Request {
        Request {
            scope: Scope::System {
                system_id: "NES".into(),
                place: crate::browse::Place::Dir(root.to_path_buf()),
                display_name: "Nintendo".into(),
            },
            scope_label: "Nintendo".into(),
            systems: vec![system(root)],
            names: DisplayNames::default(),
            settings,
            developer: Some(DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            }),
            artwork_pack_system_ids: HashSet::new(),
        }
    }

    fn game_request(root: &Path, settings: ScraperSettings) -> Request {
        Request {
            scope: Scope::Game {
                system_id: "NES".into(),
                launch: crate::browse::Launch::File(root.join("Game.rom")),
                title: "Game".into(),
            },
            scope_label: "Game".into(),
            systems: vec![system(root)],
            names: DisplayNames::default(),
            settings,
            developer: Some(DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            }),
            artwork_pack_system_ids: HashSet::new(),
        }
    }

    fn matched(id: &str, name: &str, with_media: bool) -> Match {
        Match {
            id: id.into(),
            name: name.into(),
            names: vec![name.into()],
            metadata: super::super::Metadata {
                name: Some(name.into()),
                desc: Some(format!("Description for {name}")),
                ..Default::default()
            },
            media: with_media.then(|| super::super::Media {
                url: format!("https://media.screenscraper.fr/{id}.png"),
                format: Some("png".into()),
            }),
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        }
    }

    fn finish(job: Job) -> Event {
        loop {
            match job.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                event @ (Event::Finished(_) | Event::Cancelled(_) | Event::Failed { .. }) => {
                    return event
                }
                Event::Progress(_) => {}
            }
        }
    }

    fn finish_search(job: SearchJob) -> SearchEvent {
        loop {
            match job.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                event @ (SearchEvent::Finished { .. }
                | SearchEvent::Cancelled
                | SearchEvent::Failed(_)) => return event,
                SearchEvent::Activity(_) => {}
            }
        }
    }

    fn finish_preview(job: PreviewJob) -> PreviewEvent {
        job.events.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn an_artwork_pack_scope_skips_before_credentials_and_transport() {
        let root = temp("pack-skip-explicit");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let mut request = request(&root, ScraperSettings::default());
        request.developer = None;
        request.artwork_pack_system_ids.insert("NES".into());
        let event = finish(start_with_transport(request, mock.clone()).unwrap());
        let Event::Finished(progress) = event else {
            panic!("Artwork Pack scope did not finish as an expected skip");
        };
        assert_eq!(progress.skipped_artwork_pack, 1);
        assert_eq!(progress.total, 0);
        assert_eq!(mock.api_calls.load(Ordering::Relaxed), 0);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 0);
        assert!(!root.join("gamelist.xml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_all_pack_global_scope_skips_without_a_login_or_client() {
        let root = temp("pack-skip-global");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let request = Request {
            scope: Scope::All,
            scope_label: "All Systems".into(),
            systems: vec![system(&root)],
            names: DisplayNames::default(),
            settings: ScraperSettings::default(),
            developer: None,
            artwork_pack_system_ids: HashSet::from(["NES".into()]),
        };
        let event = finish(start_with_transport(request, mock.clone()).unwrap());
        let Event::Finished(progress) = event else {
            panic!("all-Pack global scrape did not finish as an expected skip");
        };
        assert_eq!(progress.skipped_artwork_pack, 1);
        assert_eq!(progress.completed, 0);
        assert_eq!(mock.api_calls.load(Ordering::Relaxed), 0);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_mixed_global_run_scrapes_gamelist_and_reports_pack_systems_skipped() {
        let root = temp("pack-skip-mixed");
        let pack_root = root.join("pack");
        let gamelist_root = root.join("gamelist");
        std::fs::create_dir_all(&pack_root).unwrap();
        std::fs::create_dir_all(&gamelist_root).unwrap();
        std::fs::write(pack_root.join("Pack Game.rom"), b"pack").unwrap();
        std::fs::write(gamelist_root.join("Gamelist Game.rom"), b"gamelist").unwrap();
        let pack_system = system(&pack_root);
        let mut gamelist_system = system(&gamelist_root);
        gamelist_system.def.id = "Genesis".to_string();
        gamelist_system.def.name = "Genesis".to_string();
        gamelist_system.def.rbf = "_Console/Genesis".to_string();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let request = Request {
            scope: Scope::All,
            scope_label: "All Systems".into(),
            systems: vec![pack_system, gamelist_system],
            names: DisplayNames::default(),
            settings: settings(ImagePolicy::Off),
            developer: Some(DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            }),
            artwork_pack_system_ids: HashSet::from(["NES".into()]),
        };

        let event = finish(start_with_transport(request, mock.clone()).unwrap());

        let Event::Finished(progress) = event else {
            panic!("mixed global scrape did not finish");
        };
        assert_eq!(progress.skipped_artwork_pack, 1);
        assert_eq!(progress.total, 1);
        assert_eq!(progress.completed, 1);
        assert!(!pack_root.join("gamelist.xml").exists());
        assert!(gamelist_root.join("gamelist.xml").is_file());
        assert_eq!(mock.api_calls.load(Ordering::Relaxed), 2);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_explicit_one_game_match_bypasses_lookup_but_keeps_the_normal_write_path() {
        let root = temp("selected-match");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let event = finish(
            start_with_transport_and_cancel_selected(
                game_request(&root, settings(ImagePolicy::Off)),
                mock.clone(),
                Arc::new(AtomicBool::new(false)),
                Some(matched("77", "Chosen Game", false)),
            )
            .unwrap(),
        );
        let Event::Finished(progress) = event else {
            panic!("selected match did not finish");
        };
        assert_eq!(progress.updated, 1);
        assert_eq!(progress.not_found, 0);
        assert_eq!(progress.ambiguous, 0);
        assert!(progress.manual_matches.is_empty());
        assert_eq!(
            mock.api_calls.load(Ordering::Relaxed),
            1,
            "only the account preflight is made; no game lookup is repeated"
        );
        let gamelist = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert!(gamelist.contains("<name>Chosen Game</name>"));
        assert!(gamelist.contains("<desc>Description for Chosen Game</desc>"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_explicit_match_is_rejected_for_every_multi_game_scope() {
        let root = temp("selected-match-batch");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let event = finish(
            start_with_transport_and_cancel_selected(
                request(&root, settings(ImagePolicy::Off)),
                mock.clone(),
                Arc::new(AtomicBool::new(false)),
                Some(matched("77", "Chosen Game", false)),
            )
            .unwrap(),
        );
        let Event::Failed { error, .. } = event else {
            panic!("batch accepted a manually selected match");
        };
        assert_eq!(error.kind, ErrorKind::Configuration);
        assert_eq!(mock.api_calls.load(Ordering::Relaxed), 0);
        assert!(!root.join("gamelist.xml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    struct AmbiguousMock;

    impl Transport for AmbiguousMock {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            let body = match endpoint {
                "ssuserInfos.php" => "<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>20</maxrequestsperday><maxrequestskoperday>20</maxrequestskoperday></ssuser></Data>".to_string(),
                "jeuInfos.php" => {
                    let md5 = params
                        .iter()
                        .find(|(key, _)| key == "md5")
                        .map(|(_, value)| value.as_str())
                        .unwrap_or_default();
                    format!(
                        "<Data><jeux>\
                         <jeu id='1'><noms><nom region='wor'>Game</nom></noms><rom><rommd5>{md5}</rommd5></rom></jeu>\
                         <jeu id='2'><noms><nom region='wor'>Game</nom></noms><rom><rommd5>{md5}</rommd5></rom></jeu>\
                         </jeux></Data>"
                    )
                }
                other => panic!("unexpected endpoint {other}"),
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: body.into_bytes(),
            })
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
    fn unresolved_candidates_are_retained_only_for_a_one_game_scrape() {
        let root = temp("manual-candidates");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();

        let Event::Finished(single) = finish(
            start_with_transport(
                game_request(&root, settings(ImagePolicy::Off)),
                Arc::new(AmbiguousMock),
            )
            .unwrap(),
        ) else {
            panic!("single-game ambiguity did not finish safely");
        };
        assert_eq!(single.ambiguous, 1);
        assert_eq!(single.manual_matches.len(), 2);
        assert!(!root.join("gamelist.xml").exists());

        let Event::Finished(batch) = finish(
            start_with_transport(
                request(&root, settings(ImagePolicy::Off)),
                Arc::new(AmbiguousMock),
            )
            .unwrap(),
        ) else {
            panic!("batch ambiguity did not finish safely");
        };
        assert_eq!(batch.ambiguous, 1);
        assert!(
            batch.manual_matches.is_empty(),
            "batch runs count and skip ambiguity without retaining interactive state"
        );
        assert!(!root.join("gamelist.xml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    struct MixedBatchMock;

    impl Transport for MixedBatchMock {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            let parameter = |name: &str| {
                params
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default()
            };
            let body = match endpoint {
                "ssuserInfos.php" => "<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>20</maxrequestsperday><maxrequestskoperday>20</maxrequestskoperday></ssuser></Data>".to_string(),
                "jeuInfos.php" if parameter("romnom") == "Missing.rom" => {
                    "<Data><jeux/></Data>".to_string()
                }
                "jeuRecherche.php" if parameter("recherche") == "Missing" => {
                    "<Data><jeux/></Data>".to_string()
                }
                "jeuInfos.php" => {
                    let md5 = parameter("md5");
                    format!("<Data><jeux><jeu id='42'><noms><nom region='wor'>Found</nom></noms><rom><rommd5>{md5}</rommd5></rom></jeu></jeux></Data>")
                }
                other => panic!("unexpected endpoint {other}"),
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: body.into_bytes(),
            })
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
    fn a_batch_skips_an_unresolved_game_and_still_writes_the_later_match() {
        let root = temp("batch-continues-after-miss");
        std::fs::write(root.join("Missing.rom"), b"missing").unwrap();
        std::fs::write(root.join("Found.rom"), b"found").unwrap();
        let Event::Finished(progress) = finish(
            start_with_transport(
                request(&root, settings(ImagePolicy::Off)),
                Arc::new(MixedBatchMock),
            )
            .unwrap(),
        ) else {
            panic!("batch stopped at an unresolved game");
        };
        assert_eq!(progress.total, 2);
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.not_found, 1);
        assert_eq!(progress.updated, 1);
        assert!(progress.manual_matches.is_empty());
        let gamelist = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert!(gamelist.contains("<path>./Found.rom</path>"));
        assert!(!gamelist.contains("Missing.rom"));
        let _ = std::fs::remove_dir_all(root);
    }

    struct SearchMock;

    impl Transport for SearchMock {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            let body = match endpoint {
                "ssuserInfos.php" => "<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>20</maxrequestsperday><maxrequestskoperday>20</maxrequestskoperday></ssuser></Data>",
                "jeuRecherche.php" => match params
                    .iter()
                    .find(|(key, _)| key == "recherche")
                    .map(|(_, value)| value.as_str())
                {
                    Some("Nothing Here") => "<Data><jeux/></Data>",
                    Some("Edited Title") => "<Data><jeux><jeu id='30'><noms><nom region='wor'>Edited Title Result</nom></noms></jeu></jeux></Data>",
                    _ => "<Data><jeux><jeu id='20'><noms><nom region='wor'>Adventure Island</nom></noms><medias><media type='ss' region='wor' format='png'>https://media.screenscraper.fr/20.png</media></medias></jeu><jeu id='10'><noms><nom region='wor'>Adventure Time</nom></noms></jeu></jeux></Data>",
                },
                other => panic!("unexpected endpoint {other}"),
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: body.as_bytes().to_vec(),
            })
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
    fn manual_search_returns_all_candidates_and_reports_account_usage() {
        let request = SearchRequest {
            system_id: 3,
            term: "Adventure".into(),
            settings: settings(ImagePolicy::Off),
            developer: DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
        };
        let event = finish_search(
            start_search_with_transport(
                request,
                Arc::new(SearchMock),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap(),
        );
        let SearchEvent::Finished {
            matches,
            account,
            requests_started,
            failed_searches,
        } = event
        else {
            panic!("manual search did not finish");
        };
        assert_eq!(
            matches
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["20", "10"]
        );
        assert_eq!(account.max_download_speed, Some(256));
        assert_eq!(requests_started, 2, "account preflight plus search");
        assert_eq!(failed_searches, 0);
    }

    #[test]
    fn manual_search_can_return_no_results_then_succeed_with_an_edited_term() {
        let make_request = |term: &str| SearchRequest {
            system_id: 3,
            term: term.into(),
            settings: settings(ImagePolicy::Off),
            developer: DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
        };
        let run = |term: &str| {
            finish_search(
                start_search_with_transport(
                    make_request(term),
                    Arc::new(SearchMock),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap(),
            )
        };

        let SearchEvent::Finished {
            matches,
            failed_searches,
            ..
        } = run("Nothing Here")
        else {
            panic!("no-result search did not complete normally");
        };
        assert!(matches.is_empty());
        assert_eq!(failed_searches, 1);

        let SearchEvent::Finished { matches, .. } = run("Edited Title") else {
            panic!("edited search did not complete");
        };
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Edited Title Result");
    }

    #[test]
    fn manual_search_is_cancellable_before_any_external_request() {
        let cancelled = Arc::new(AtomicBool::new(true));
        let event = finish_search(
            start_search_with_transport(
                SearchRequest {
                    system_id: 3,
                    term: "Adventure".into(),
                    settings: settings(ImagePolicy::Off),
                    developer: DeveloperCredentials {
                        developer_id: "developer".into(),
                        developer_password: "private".into(),
                    },
                },
                Arc::new(SearchMock),
                cancelled,
            )
            .unwrap(),
        );
        assert!(matches!(event, SearchEvent::Cancelled));
    }

    #[test]
    fn preview_job_decodes_success_and_distinguishes_missing_invalid_and_cancelled_media() {
        let request = |id: &str| PreviewRequest {
            match_id: id.into(),
            media: super::super::Media {
                url: format!("https://media.screenscraper.fr/{id}.png"),
                format: Some("png".into()),
            },
            settings: settings(ImagePolicy::MissingOnly),
            developer: DeveloperCredentials {
                developer_id: "developer".into(),
                developer_password: "private".into(),
            },
            max_download_speed: 256,
            max_edge: 128,
            ground: [0, 0, 0],
        };
        let transport = |bad_media, no_media| {
            Arc::new(Mock {
                api_calls: AtomicUsize::new(0),
                media_calls: AtomicUsize::new(0),
                quota_empty: false,
                bad_media,
                no_media,
            }) as Arc<dyn Transport>
        };

        let success = finish_preview(
            start_preview_with_transport(
                request("success"),
                transport(false, false),
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap(),
        );
        let PreviewEvent::Finished { match_id, image } = success else {
            panic!("valid preview did not decode");
        };
        assert_eq!(match_id, "success");
        assert_eq!((image.width, image.height), (1, 1));

        assert!(matches!(
            finish_preview(
                start_preview_with_transport(
                    request("missing"),
                    transport(false, true),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap()
            ),
            PreviewEvent::Missing { match_id } if match_id == "missing"
        ));
        assert!(matches!(
            finish_preview(
                start_preview_with_transport(
                    request("invalid"),
                    transport(true, false),
                    Arc::new(AtomicBool::new(false)),
                )
                .unwrap()
            ),
            PreviewEvent::Failed { match_id, error }
                if match_id == "invalid" && error.kind == ErrorKind::MalformedResponse
        ));

        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            finish_preview(
                start_preview_with_transport(
                    request("cancelled"),
                    transport(false, false),
                    cancelled,
                )
                .unwrap()
            ),
            PreviewEvent::Cancelled
        ));
    }

    #[test]
    fn cancellation_during_initial_enumeration_is_not_reported_as_failure() {
        let root = temp("cancel-before-enumeration");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let cancelled = Arc::new(AtomicBool::new(true));
        let event = finish(
            start_with_transport_and_cancel(
                request(&root, settings(ImagePolicy::Off)),
                mock.clone(),
                cancelled,
            )
            .unwrap(),
        );
        assert!(matches!(event, Event::Cancelled(_)));
        assert_eq!(mock.api_calls.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn metadata_only_never_downloads_media_and_preserves_unknown_xml() {
        let root = temp("metadata");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><!--keep--><game><path>./Game.rom</path><rating>0.8</rating></game></gameList>",
        )
        .unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let event = finish(
            start_with_transport(request(&root, settings(ImagePolicy::Off)), mock.clone()).unwrap(),
        );
        let Event::Finished(progress) = event else {
            panic!("scrape did not finish");
        };
        assert_eq!(progress.updated, 1);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 0);
        let text = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert!(text.contains("<!--keep-->"));
        assert!(text.contains("<rating>0.8</rating>"));
        assert!(text.contains("<name>Remote Game</name>"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_image_is_downloaded_to_a_non_destructive_content_name() {
        let root = temp("image");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let event = finish(
            start_with_transport(
                request(&root, settings(ImagePolicy::MissingOnly)),
                mock.clone(),
            )
            .unwrap(),
        );
        let Event::Finished(progress) = event else {
            panic!("scrape did not finish");
        };
        assert_eq!(progress.updated, 1);
        assert_eq!(progress.updated_systems, vec!["NES"]);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 1);
        let text = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert!(text.contains("<image>./media/screenscraper/3-42-"));
        let images: Vec<_> = std::fs::read_dir(root.join("media/screenscraper"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(std::fs::read(images[0].path()).unwrap(), PNG);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_content_named_image_is_repaired_without_rewriting_the_gamelist() {
        let root = temp("repair-content-image");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let checksum = Sha1::digest(PNG)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let relative = format!("./media/screenscraper/3-42-{checksum}.png");
        let original = format!(
            "<gameList><!--keep--><game><path>./Game.rom</path><image>{relative}</image></game></gameList>"
        );
        std::fs::write(root.join("gamelist.xml"), &original).unwrap();

        let mut scrape_settings = settings(ImagePolicy::MissingOnly);
        scrape_settings.metadata_policy = MetadataPolicy::Off;
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let event =
            finish(start_with_transport(request(&root, scrape_settings), mock.clone()).unwrap());
        let Event::Finished(progress) = event else {
            panic!("repair scrape did not finish");
        };

        assert_eq!(progress.updated, 1);
        assert_eq!(progress.unchanged, 0);
        assert_eq!(progress.updated_systems, vec!["NES"]);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("gamelist.xml")).unwrap(),
            original
        );
        assert!(root.join(relative.trim_start_matches("./")).is_file());
        assert!(!std::fs::read_dir(&root).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("gamelist.xml.degauss-scraper-")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_known_empty_daily_quota_stops_before_game_requests() {
        let root = temp("quota");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: true,
            bad_media: false,
            no_media: false,
        });
        let event =
            finish(start_with_transport(request(&root, settings(ImagePolicy::Off)), mock).unwrap());
        let Event::Failed { error, progress } = event else {
            panic!("quota exhaustion was not reported");
        };
        assert_eq!(error.kind, ErrorKind::DailyQuota);
        assert_eq!(progress.requests_started, 0);
        assert!(!root.join("gamelist.xml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    struct ZeroMediaSpeed {
        api_calls: AtomicUsize,
    }

    impl Transport for ZeroMediaSpeed {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            self.api_calls.fetch_add(1, Ordering::Relaxed);
            let body = match endpoint {
                "ssuserInfos.php" => b"<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>0</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>10</maxrequestsperday><maxrequestskoperday>10</maxrequestskoperday></ssuser></Data>".to_vec(),
                "jeuInfos.php" => {
                    let md5 = params
                        .iter()
                        .find(|(key, _)| key == "md5")
                        .map(|(_, value)| value.as_str())
                        .unwrap_or_default();
                    format!(
                        "<Data><jeux><jeu id=\"42\"><noms><nom region=\"wor\">Remote Game</nom></noms><synopsis><synopsis langue=\"en\">Remote description</synopsis></synopsis><rom><rommd5>{md5}</rommd5></rom></jeu></jeux></Data>"
                    )
                    .into_bytes()
                }
                other => panic!("unexpected endpoint {other}"),
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body,
            })
        }

        fn get_media(
            &self,
            _url: &str,
            _limit: u64,
            _max_kib_per_second: Option<u64>,
        ) -> Result<HttpResponse> {
            panic!("media must not start with a zero download allowance")
        }
    }

    #[test]
    fn a_zero_media_allowance_stops_before_spending_a_game_lookup() {
        let root = temp("zero-media-speed");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let transport = Arc::new(ZeroMediaSpeed {
            api_calls: AtomicUsize::new(0),
        });
        let mut scraper_settings = settings(ImagePolicy::MissingOnly);
        scraper_settings.metadata_policy = MetadataPolicy::Off;
        let event = finish(
            start_with_transport(request(&root, scraper_settings), transport.clone()).unwrap(),
        );
        let Event::Failed { error, progress } = event else {
            panic!("zero media allowance was not reported")
        };
        assert_eq!(error.kind, ErrorKind::RateLimited);
        assert_eq!(progress.requests_started, 0);
        assert_eq!(transport.api_calls.load(Ordering::Relaxed), 1);
        assert!(!root.join("gamelist.xml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_zero_media_allowance_does_not_block_metadata_when_images_are_complete() {
        let root = temp("zero-media-speed-metadata");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        std::fs::write(root.join("cover.png"), PNG).unwrap();
        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><game><path>./Game.rom</path><image>./cover.png</image></game></gameList>",
        )
        .unwrap();
        let transport = Arc::new(ZeroMediaSpeed {
            api_calls: AtomicUsize::new(0),
        });
        let event = finish(
            start_with_transport(
                request(&root, settings(ImagePolicy::MissingOnly)),
                transport.clone(),
            )
            .unwrap(),
        );
        let Event::Finished(progress) = event else {
            panic!("metadata scrape did not finish")
        };
        assert_eq!(progress.updated, 1);
        assert_eq!(progress.no_media, 0);
        assert_eq!(transport.api_calls.load(Ordering::Relaxed), 2);
        assert!(std::fs::read_to_string(root.join("gamelist.xml"))
            .unwrap()
            .contains("<desc>Remote description</desc>"));
        let _ = std::fs::remove_dir_all(root);
    }

    struct OneRequestLeft {
        game_calls: AtomicUsize,
    }

    impl Transport for OneRequestLeft {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            let body = if endpoint == "ssuserInfos.php" {
                "<Data><ssuser><niveau>1</niveau><maxthreads>2</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>1</maxrequestsperday><maxrequestskoperday>10</maxrequestskoperday></ssuser></Data>".to_string()
            } else {
                self.game_calls.fetch_add(1, Ordering::Relaxed);
                let md5 = params
                    .iter()
                    .find(|(key, _)| key == "md5")
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default();
                format!("<Data><jeux><jeu id=\"42\"><noms><nom region=\"wor\">Remote Game</nom></noms><rom><rommd5>{md5}</rommd5></rom></jeu></jeux></Data>")
            };
            Ok(HttpResponse {
                status: 200,
                content_type: Some("application/xml".into()),
                body: body.into_bytes(),
            })
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
    fn the_last_claimed_daily_request_finishes_and_is_flushed() {
        let root = temp("last-request");
        std::fs::write(root.join("First.rom"), b"first").unwrap();
        std::fs::write(root.join("Second.rom"), b"second").unwrap();
        let mock = Arc::new(OneRequestLeft {
            game_calls: AtomicUsize::new(0),
        });

        let event = finish(
            start_with_transport(request(&root, settings(ImagePolicy::Off)), mock.clone()).unwrap(),
        );
        let Event::Failed { error, progress } = event else {
            panic!("daily exhaustion was not reported");
        };
        assert_eq!(error.kind, ErrorKind::DailyQuota);
        assert_eq!(mock.game_calls.load(Ordering::Relaxed), 1);
        assert_eq!(progress.updated, 1);
        let text = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert_eq!(text.matches("<game>").count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_image_fails_that_game_without_stopping_an_images_only_run() {
        let root = temp("bad-image");
        std::fs::write(root.join("First.rom"), b"first").unwrap();
        std::fs::write(root.join("Second.rom"), b"second").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: true,
            no_media: false,
        });
        let mut settings = settings(ImagePolicy::MissingOnly);
        settings.metadata_policy = super::super::MetadataPolicy::Off;
        let event = finish(start_with_transport(request(&root, settings), mock.clone()).unwrap());
        let Event::Finished(progress) = event else {
            panic!("one bad image stopped the complete scrape");
        };
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.failed, 2);
        assert_eq!(progress.updated, 0);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 2);
        assert!(!root.join("gamelist.xml").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_remote_media_still_applies_requested_metadata() {
        let root = temp("missing-remote-media");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: true,
        });
        let event = finish(
            start_with_transport(
                request(&root, settings(ImagePolicy::MissingOnly)),
                mock.clone(),
            )
            .unwrap(),
        );
        let Event::Finished(progress) = event else {
            panic!("a missing remote image stopped metadata scraping");
        };
        assert_eq!(progress.updated, 1);
        assert_eq!(progress.no_media, 1);
        assert_eq!(progress.failed, 0);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 1);
        let text = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert!(text.contains("<name>Remote Game</name>"));
        assert!(!text.contains("<image>"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_images_only_miss_does_not_create_a_path_only_gamelist_entry() {
        let root = temp("image-only-miss");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: true,
        });
        let mut scraper_settings = settings(ImagePolicy::MissingOnly);
        scraper_settings.metadata_policy = super::super::MetadataPolicy::Off;
        let event =
            finish(start_with_transport(request(&root, scraper_settings), mock.clone()).unwrap());
        let Event::Finished(progress) = event else {
            panic!("a remote image miss stopped the scrape");
        };
        assert_eq!(progress.no_media, 1);
        assert_eq!(progress.unchanged, 1);
        assert_eq!(progress.updated, 0);
        assert!(!root.join("gamelist.xml").exists());
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn completed_missing_only_content_makes_no_network_request() {
        let root = temp("complete-no-network");
        std::fs::write(root.join("Game.rom"), b"game").unwrap();
        std::fs::write(root.join("art.png"), PNG).unwrap();
        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><game><path>./Game.rom</path><name>Name</name><desc>Description</desc><publisher>Publisher</publisher><developer>Developer</developer><releasedate>19910000T000000</releasedate><players>1</players><genre>Action</genre><lang>en</lang><image>./art.png</image></game></gameList>",
        )
        .unwrap();
        let mock = Arc::new(Mock {
            api_calls: AtomicUsize::new(0),
            media_calls: AtomicUsize::new(0),
            quota_empty: false,
            bad_media: false,
            no_media: false,
        });
        let event = finish(
            start_with_transport(
                request(&root, settings(ImagePolicy::MissingOnly)),
                mock.clone(),
            )
            .unwrap(),
        );
        let Event::Finished(progress) = event else {
            panic!("no-op scrape did not finish");
        };
        assert_eq!(progress.completed, 1);
        assert_eq!(progress.unchanged, 1);
        assert_eq!(progress.updated, 0);
        assert_eq!(mock.api_calls.load(Ordering::Relaxed), 0);
        assert_eq!(mock.media_calls.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    struct FailAfterOneMatch {
        game_calls: AtomicUsize,
    }

    impl Transport for FailAfterOneMatch {
        fn get(
            &self,
            endpoint: &str,
            params: &[(String, String)],
            _limit: u64,
        ) -> Result<HttpResponse> {
            if endpoint == "ssuserInfos.php" {
                return Ok(HttpResponse {
                    status: 200,
                    content_type: Some("application/xml".into()),
                    body: b"<Data><ssuser><niveau>1</niveau><maxthreads>1</maxthreads><maxdownloadspeed>256</maxdownloadspeed><requeststoday>0</requeststoday><requestskotoday>0</requestskotoday><maxrequestspermin>60</maxrequestspermin><maxrequestsperday>10</maxrequestsperday><maxrequestskoperday>10</maxrequestskoperday></ssuser></Data>".to_vec(),
                });
            }
            let at = self.game_calls.fetch_add(1, Ordering::Relaxed);
            if at == 0 {
                let md5 = params
                    .iter()
                    .find(|(key, _)| key == "md5")
                    .map(|(_, value)| value.as_str())
                    .unwrap_or_default();
                return Ok(HttpResponse {
                    status: 200,
                    content_type: Some("application/xml".into()),
                    body: format!("<Data><jeux><jeu id=\"42\"><noms><nom region=\"wor\">Remote Game</nom></noms><rom><rommd5>{md5}</rommd5></rom></jeu></jeux></Data>").into_bytes(),
                });
            }
            Ok(HttpResponse {
                status: 503,
                content_type: Some("application/xml".into()),
                body: Vec::new(),
            })
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
    fn completed_results_are_flushed_before_a_later_fatal_error() {
        let root = temp("partial-before-fatal");
        std::fs::write(root.join("First.rom"), b"first").unwrap();
        std::fs::write(root.join("Second.rom"), b"second").unwrap();
        let mock = Arc::new(FailAfterOneMatch {
            game_calls: AtomicUsize::new(0),
        });
        let event =
            finish(start_with_transport(request(&root, settings(ImagePolicy::Off)), mock).unwrap());
        let Event::Failed { error, progress } = event else {
            panic!("the fatal server response was not reported");
        };
        assert_eq!(error.kind, ErrorKind::Server);
        assert_eq!(progress.updated, 1);
        assert_eq!(progress.failed, 1);
        let text = std::fs::read_to_string(root.join("gamelist.xml")).unwrap();
        assert_eq!(text.matches("<game>").count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_conflicting_content_named_image_is_never_trusted_or_overwritten() {
        let root = temp("media-conflict");
        let target = Target {
            metadata_fallback: true,
            system_id: "NES".into(),
            affected_system_ids: vec!["NES".into()],
            system_name: "Nintendo".into(),
            screen_scraper_system_id: 3,
            title: "Game".into(),
            query: "Game".into(),
            match_path: None,
            folder: root.clone(),
            relative_path: "./Game.rom".into(),
        };
        let matched = Match {
            id: "42".into(),
            name: "Game".into(),
            names: vec!["Game".into()],
            metadata: super::super::Metadata::default(),
            media: None,
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        };
        let response = HttpResponse {
            status: 200,
            content_type: Some("image/png".into()),
            body: PNG.to_vec(),
        };

        let installed = install_media(&target, &matched, response.clone()).unwrap();
        std::fs::write(&installed.absolute_path, b"different local bytes").unwrap();
        let error = install_media(&target, &matched, response).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Local);
        assert_eq!(
            std::fs::read(&installed.absolute_path).unwrap(),
            b"different local bytes"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_removes_only_new_media_proven_unreferenced() {
        let root = temp("media-cleanup");
        let directory = root.join("media/screenscraper");
        std::fs::create_dir_all(&directory).unwrap();
        let absolute = directory.join("new.png");
        let relative = "./media/screenscraper/new.png".to_string();
        let target = Target {
            metadata_fallback: true,
            system_id: "NES".into(),
            affected_system_ids: vec!["NES".into()],
            system_name: "Nintendo".into(),
            screen_scraper_system_id: 3,
            title: "Game".into(),
            query: "Game".into(),
            match_path: None,
            folder: root.clone(),
            relative_path: "./Game.rom".into(),
        };
        let pending = |created| {
            vec![Pending {
                target: target.clone(),
                metadata: super::super::Metadata::default(),
                media: Some(InstalledMedia {
                    relative_path: relative.clone(),
                    absolute_path: absolute.clone(),
                    created,
                }),
            }]
        };
        let mut progress = Progress::default();

        std::fs::write(&absolute, PNG).unwrap();
        cleanup_unreferenced_media(&root.join("gamelist.xml"), &pending(true), &mut progress);
        assert!(!absolute.exists());

        std::fs::write(&absolute, PNG).unwrap();
        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><game><path>./Game.rom</path><image>./MEDIA/SCREENSCRAPER/NEW.PNG</image></game></gameList>",
        )
        .unwrap();
        cleanup_unreferenced_media(&root.join("gamelist.xml"), &pending(true), &mut progress);
        assert!(absolute.exists());

        std::fs::write(
            root.join("gamelist.xml"),
            "<gameList><game><path>./Game.rom</path></game></gameList>",
        )
        .unwrap();
        cleanup_unreferenced_media(&root.join("gamelist.xml"), &pending(false), &mut progress);
        assert!(absolute.exists());
        assert_eq!(progress.failed, 0);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
#[path = "worker_live_tests.rs"]
mod live_tests;
