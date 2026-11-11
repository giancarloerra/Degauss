use std::collections::BTreeSet;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{
    sse::{Event, KeepAlive},
    IntoResponse, Response, Sse,
};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use super::api::{self, Command, Error, Reply};
use super::{auth, Config, Mode};

pub enum Request {
    Command(Box<Command>),
    Configure {
        revision: String,
        config: Config,
        password: Option<String>,
    },
    Upload {
        revision: String,
        target: String,
        body: Vec<u8>,
    },
    Media(String),
}

pub struct PendingRequest {
    pub request: Request,
    pub reply: oneshot::Sender<api::Result>,
}

pub struct Bridge {
    pub requests: mpsc::Receiver<PendingRequest>,
    sender: mpsc::SyncSender<PendingRequest>,
    events: broadcast::Sender<String>,
}

impl Default for Bridge {
    fn default() -> Self {
        let (sender, requests) = mpsc::sync_channel(16);
        let (events, _) = broadcast::channel(32);
        Self {
            requests,
            sender,
            events,
        }
    }
}

impl Bridge {
    pub fn has_listeners(&self) -> bool {
        self.events.receiver_count() > 0
    }
    pub fn event(&self, event: &Value) {
        // Having no connected browser is ordinary, not a failed delivery.
        if self.events.receiver_count() > 0 {
            let _ = self.events.send(event.to_string());
        }
    }
}

struct HttpState {
    auth: Arc<Mutex<auth::Auth>>,
    sender: mpsc::SyncSender<PendingRequest>,
    events: broadcast::Sender<String>,
    mode: Mode,
    hosts: BTreeSet<String>,
    login_after: Mutex<Instant>,
    requests: tokio::sync::Semaphore,
    streams: Arc<tokio::sync::Semaphore>,
}

pub struct Server {
    handle: axum_server::Handle<SocketAddr>,
    pub status: Arc<Mutex<Status>>,
    events: broadcast::Sender<String>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub enum Status {
    Starting,
    Listening,
    Failed(String),
}

impl Server {
    pub fn start(
        config: &Config,
        directory: PathBuf,
        addresses: &[IpAddr],
        bridge: &Bridge,
    ) -> Result<Self, String> {
        config.validate()?;
        if config.mode == Mode::Off {
            return Err("Web Access is Off.".into());
        }
        let auth = auth::Auth::load(directory.clone())?;
        let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], config.port)))
            .map_err(|e| format!("Opening web port {} failed: {e}", config.port))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Setting web socket mode failed: {e}"))?;
        let hosts = addresses
            .iter()
            .filter(|ip| ip.is_ipv4())
            .chain(std::iter::once(&IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)))
            .map(|ip| format!("{ip}:{}", config.port))
            .chain([
                format!("localhost:{}", config.port),
                format!("mister.local:{}", config.port),
            ])
            .collect();
        let state = Arc::new(HttpState {
            auth: Arc::new(Mutex::new(auth)),
            sender: bridge.sender.clone(),
            events: bridge.events.clone(),
            mode: config.mode,
            hosts,
            login_after: Mutex::new(Instant::now()),
            requests: tokio::sync::Semaphore::new(8),
            streams: Arc::new(tokio::sync::Semaphore::new(8)),
        });
        let status = Arc::new(Mutex::new(Status::Starting));
        let worker_status = status.clone();
        let handle = axum_server::Handle::new();
        let worker_handle = handle.clone();
        let mode = config.mode;
        let addresses = addresses.to_vec();
        let thread = std::thread::Builder::new()
            .name("degauss-web".into())
            .spawn(move || {
                let result: Result<(), String> = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .max_blocking_threads(2)
                        .build()
                        .map_err(|e| format!("Starting web runtime failed: {e}"))?;
                    runtime.block_on(async {
                        let app = router(state);
                        if mode == Mode::Https {
                            let (cert, key) = certificate(&directory, &addresses)?;
                            let _ = rustls::crypto::ring::default_provider().install_default();
                            let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
                                cert.into_bytes(),
                                key.into_bytes(),
                            )
                            .await
                            .map_err(|e| format!("Loading HTTPS certificate failed: {e}"))?;
                            *worker_status.lock().expect("web status") = Status::Listening;
                            axum_server::from_tcp_rustls(listener, tls)
                                .map_err(|e| e.to_string())?
                                .handle(worker_handle)
                                .serve(app.into_make_service())
                                .await
                                .map_err(|e| format!("HTTPS server stopped: {e}"))
                        } else {
                            *worker_status.lock().expect("web status") = Status::Listening;
                            axum_server::from_tcp(listener)
                                .map_err(|e| e.to_string())?
                                .handle(worker_handle)
                                .serve(app.into_make_service())
                                .await
                                .map_err(|e| format!("HTTP server stopped: {e}"))
                        }
                    })
                })();
                if let Err(error) = result {
                    *worker_status.lock().expect("web status") = Status::Failed(error);
                }
            })
            .map_err(|e| format!("Starting web worker failed: {e}"))?;
        Ok(Self {
            handle,
            status,
            events: bridge.events.clone(),
            thread: Some(thread),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Close SSE first, then let HTTP flush the accepted launch response.
        // The deadline bounds unrelated slow clients, not the launch timing.
        let _ = self.events.send("web-server-closing".into());
        self.handle.graceful_shutdown(Some(Duration::from_secs(1)));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn certificate(directory: &Path, addresses: &[IpAddr]) -> Result<(String, String), String> {
    let path = directory.join("identity.json");
    #[derive(serde::Serialize, Deserialize)]
    struct Identity {
        certificate: String,
        key: String,
        names: Vec<String>,
    }
    let mut names = vec![
        "mister.local".into(),
        "localhost".into(),
        "127.0.0.1".into(),
    ];
    names.extend(addresses.iter().map(ToString::to_string));
    names.sort();
    names.dedup();
    if path.exists() {
        let saved: Identity = serde_json::from_slice(
            &std::fs::read(&path).map_err(|e| format!("Reading HTTPS identity failed: {e}"))?,
        )
        .map_err(|e| format!("HTTPS identity is malformed: {e}"))?;
        if names.iter().all(|name| saved.names.contains(name)) {
            return Ok((saved.certificate, saved.key));
        }
        return Err("The network address changed. Renew the HTTPS certificate in Web Access, then trust the new certificate in the browser.".into());
    }
    let generated = rcgen::generate_simple_self_signed(names.clone())
        .map_err(|e| format!("Generating HTTPS certificate failed: {e}"))?;
    let identity = Identity {
        certificate: generated.cert.pem(),
        key: generated.signing_key.serialize_pem(),
        names,
    };
    auth::save(
        &path,
        &serde_json::to_string(&identity).map_err(|e| e.to_string())?,
    )?;
    Ok((identity.certificate, identity.key))
}

fn router(state: Arc<HttpState>) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    include_str!("../../web/index.html"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../web/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../web/style.css"),
                )
            }),
        )
        .route(
            "/logo.png",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    include_bytes!("../../assets/degauss-logo.png").as_slice(),
                )
            }),
        )
        .route(
            "/font.ttf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "font/ttf")],
                    include_bytes!("../../assets/fonts/DejaVuSans.ttf").as_slice(),
                )
            }),
        )
        .route(
            "/heading.ttf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "font/ttf")],
                    include_bytes!("../../assets/fonts/RobotoCondensed-Bold.ttf").as_slice(),
                )
            }),
        )
        .route(
            "/pixel.ttf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "font/ttf")],
                    include_bytes!("../../assets/fonts/Px437_DOS-V_re_JPN12.ttf").as_slice(),
                )
            }),
        )
        .route(
            "/pixel2.ttf",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "font/ttf")],
                    include_bytes!("../../assets/fonts/Tamzen6x12b.ttf").as_slice(),
                )
            }),
        )
        .route("/api/v1/login", post(login))
        .route("/api/v1/session", get(session))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/command", post(command))
        .route(
            "/api/v1/upload/{target}",
            post(upload).layer(DefaultBodyLimit::max(crate::scraper::MAX_UPLOAD_BYTES)),
        )
        .route("/api/v1/media/{id}", get(media))
        .route("/api/v1/events", get(events))
        .route(
            "/api/v1/openapi.json",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    include_str!("../../web/openapi.json"),
                )
            }),
        )
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .with_state(state)
}

fn failure(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": Error::new(code, message)}))).into_response()
}

fn request_host(request: &axum::extract::Request) -> Option<&str> {
    let header = match request.headers().get(header::HOST) {
        Some(value) => Some(value.to_str().ok()?),
        None => None,
    };
    // Hyper places HTTP/2's :authority in the URI, not in the Host header.
    match (request.uri().authority(), header) {
        (Some(authority), Some(host)) if authority.as_str() != host => None,
        (Some(authority), _) => Some(authority.as_str()),
        (None, host) => host,
    }
}

async fn boundary(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let host = request_host(&request);
    let Some(host) = host.filter(|host| state.hosts.contains(*host)) else {
        return failure(
            StatusCode::FORBIDDEN,
            "host",
            "Open the address shown on MiSTer's Web Access page.",
        );
    };
    if let Some(origin) = headers.get(header::ORIGIN) {
        if origin.to_str().ok() != Some(format!("{}://{host}", state.mode.scheme()).as_str()) {
            return failure(
                StatusCode::FORBIDDEN,
                "origin",
                "Cross-origin requests are not permitted.",
            );
        }
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|value| value == "cross-site")
    {
        return failure(
            StatusCode::FORBIDDEN,
            "origin",
            "Cross-site requests are not permitted.",
        );
    }
    // Reject unauthenticated API bodies before allocating or decoding them.
    let path = request.uri().path();
    if path.starts_with("/api/v1/") && path != "/api/v1/login" {
        if let Err(error) =
            authenticated(&state, headers, request.method() != axum::http::Method::GET)
        {
            return *error;
        }
    }
    // A browser requests a grid's images together, particularly over HTTP/2.
    // Bound active work without treating an ordinary image burst as a failure.
    // Wait before decoding bodies or sending commands to the frontend.
    let Ok(Ok(_permit)) =
        tokio::time::timeout(Duration::from_secs(5), state.requests.acquire()).await
    else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "No request slot became available within five seconds. No command was accepted.",
        );
    };
    let mut response = match tokio::time::timeout(Duration::from_secs(30), next.run(request)).await
    {
        Ok(response) => response,
        Err(_) => failure(
            StatusCode::REQUEST_TIMEOUT,
            "request-timeout",
            "The request did not complete in time. Check current state before retrying.",
        ),
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}

fn cookie_name(mode: Mode) -> &'static str {
    if mode == Mode::Https {
        "__Host-degauss"
    } else {
        "degauss-http"
    }
}

fn token(headers: &HeaderMap, mode: Mode) -> Option<&str> {
    // HTTP/2 browsers may send each cookie in a separate header field.
    for header in headers.get_all(header::COOKIE) {
        for part in header.to_str().ok()?.split(';') {
            if let Some((name, value)) = part.trim().split_once('=') {
                if name == cookie_name(mode) {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn authenticated(
    state: &HttpState,
    headers: &HeaderMap,
    mutation: bool,
) -> Result<auth::Session, Box<Response>> {
    let auth = state.auth.lock().map_err(|_| {
        Box::new(failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "authentication-state",
            "Authentication state failed.",
        ))
    })?;
    let session = token(headers, state.mode).and_then(|token| auth.session(token));
    let Some(session) = session else {
        return Err(Box::new(failure(
            StatusCode::UNAUTHORIZED,
            "authentication",
            "Sign in to Degauss.",
        )));
    };
    if mutation
        && headers
            .get("x-degauss-csrf")
            .and_then(|value| value.to_str().ok())
            != Some(session.csrf.as_str())
    {
        return Err(Box::new(failure(
            StatusCode::FORBIDDEN,
            "csrf",
            "The browser session changed. Sign in again.",
        )));
    }
    Ok(session)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    password: String,
    #[serde(default)]
    remember: bool,
}

async fn login(
    State(state): State<Arc<HttpState>>,
    input: Result<Json<Login>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(login) = match input {
        Ok(input) => input,
        Err(error) => return failure(error.status(), "json", error.body_text()),
    };
    {
        let mut after = state.login_after.lock().expect("login limit");
        if Instant::now() < *after {
            return failure(
                StatusCode::TOO_MANY_REQUESTS,
                "rate-limit",
                "Wait two seconds before trying again.",
            );
        }
        *after = Instant::now() + Duration::from_secs(2);
    }
    let auth = state.auth.clone();
    let result = tokio::task::spawn_blocking(move || {
        let credentials = auth
            .lock()
            .map_err(|_| "Authentication state failed.".to_string())?
            .credentials();
        let verified = credentials.verify(&login.password)?;
        auth.lock()
            .map_err(|_| "Authentication state failed.".to_string())?
            .establish(verified, login.remember)
    })
    .await;
    match result {
        Ok(Ok((token, session))) => {
            let mut cookie = format!(
                "{}={token}; Path=/; HttpOnly; SameSite=Strict",
                cookie_name(state.mode)
            );
            if state.mode == Mode::Https {
                cookie.push_str("; Secure");
            }
            if session.remember {
                cookie.push_str(&format!(
                    "; Max-Age={}",
                    session.expires.saturating_sub(auth::now())
                ));
            }
            (
                [(header::SET_COOKIE, cookie)],
                Json(json!({"csrf":session.csrf,"encrypted":state.mode == Mode::Https})),
            )
                .into_response()
        }
        Ok(Err(error)) if error == "Incorrect password." => {
            failure(StatusCode::UNAUTHORIZED, "authentication", error)
        }
        Ok(Err(error)) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "authentication-storage",
            error,
        ),
        Err(error) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "authentication-worker",
            error.to_string(),
        ),
    }
}

async fn session(State(state): State<Arc<HttpState>>, headers: HeaderMap) -> Response {
    match authenticated(&state, &headers, false) {
        Ok(session) => {
            Json(json!({"csrf":session.csrf,"encrypted":state.mode == Mode::Https})).into_response()
        }
        Err(error) => *error,
    }
}

async fn logout(State(state): State<Arc<HttpState>>, headers: HeaderMap) -> Response {
    if let Err(error) = authenticated(&state, &headers, true) {
        return *error;
    }
    let result = state
        .auth
        .lock()
        .expect("authentication")
        .logout(token(&headers, state.mode).expect("authenticated"));
    if let Err(error) = result {
        return failure(StatusCode::INTERNAL_SERVER_ERROR, "session-storage", error);
    }
    let secure = if state.mode == Mode::Https {
        "; Secure"
    } else {
        ""
    };
    (
        [(
            header::SET_COOKIE,
            format!(
                "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}",
                cookie_name(state.mode)
            ),
        )],
        Json(json!({"signed_out":true})),
    )
        .into_response()
}

async fn dispatch(state: &HttpState, request: Request) -> Response {
    let (reply, receive) = oneshot::channel();
    if state
        .sender
        .try_send(PendingRequest { request, reply })
        .is_err()
    {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Degauss is busy. No command was queued.",
        );
    }
    match tokio::time::timeout(Duration::from_secs(15), receive).await {
        Ok(Ok(Ok(Reply::Json(value)))) => Json(value).into_response(),
        Ok(Ok(Ok(Reply::Media { path }))) => {
            let result = tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let file = std::fs::File::open(&path)?;
                if !file.metadata()?.is_file() { return Err(std::io::Error::other("Not an artwork file")); }
                let mut bytes = Vec::new();
                file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 16 * 1024 * 1024 { return Err(std::io::Error::other("Artwork exceeds 16 MiB")); }
                // Managed category/system overrides have extensionless names.
                // Identify the same PNG/JPEG contents accepted by native artwork.
                let content_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    "image/png"
                } else if bytes.starts_with(&[0xff, 0xd8]) {
                    "image/jpeg"
                } else {
                    return Err(std::io::Error::other("Artwork is not PNG or JPEG"));
                };
                Ok((content_type, bytes))
            }).await;
            match result {
                Ok(Ok((content_type, bytes))) => ([(header::CONTENT_TYPE, content_type)], bytes).into_response(),
                Ok(Err(error)) => failure(StatusCode::NOT_FOUND, "artwork", error.to_string()),
                Err(error) => failure(StatusCode::INTERNAL_SERVER_ERROR, "artwork-worker", error.to_string()),
            }
        }
        Ok(Ok(Err(error))) => {
            let status = match error.code { "conflict" => StatusCode::CONFLICT, "locked" => StatusCode::FORBIDDEN, "busy" => StatusCode::SERVICE_UNAVAILABLE, _ => StatusCode::BAD_REQUEST };
            (status, Json(json!({"error":error}))).into_response()
        }
        Ok(Err(_)) => failure(StatusCode::SERVICE_UNAVAILABLE, "offline", "Degauss closed before acknowledging this request."),
        Err(_) => failure(StatusCode::GATEWAY_TIMEOUT, "timeout", "Degauss did not acknowledge this request in time. Check its current state before retrying."),
    }
}

async fn command(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    input: Result<Json<Command>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(command) = match input {
        Ok(input) => input,
        Err(error) => return failure(error.status(), "json", error.body_text()),
    };
    if let Err(error) = authenticated(&state, &headers, true) {
        return *error;
    }
    let request = if let Command::WebSettingsSave {
        revision,
        config,
        password,
    } = command
    {
        if let Err(error) = config.validate() {
            return failure(StatusCode::BAD_REQUEST, "configuration", &error);
        }
        let password = match password {
            Some(password) => {
                match tokio::task::spawn_blocking(move || auth::prepare_password(&password)).await {
                    Ok(Ok(prepared)) => Some(prepared),
                    Ok(Err(error)) => return failure(StatusCode::BAD_REQUEST, "password", &error),
                    Err(error) => {
                        return failure(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "password",
                            format!("Password worker failed: {error}"),
                        )
                    }
                }
            }
            None => None,
        };
        if let Err(error) = authenticated(&state, &headers, true) {
            return *error;
        }
        Request::Configure {
            revision,
            config,
            password,
        }
    } else {
        Request::Command(Box::new(command))
    };
    dispatch(&state, request).await
}

async fn media(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Response {
    if let Err(error) = authenticated(&state, &headers, false) {
        return *error;
    }
    dispatch(&state, Request::Media(id)).await
}

async fn upload(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    UrlPath(target): UrlPath<String>,
    input: Result<axum::body::Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let body = match input {
        Ok(body) => body,
        Err(error) => return failure(error.status(), "upload", error.body_text()),
    };
    if let Err(error) = authenticated(&state, &headers, true) {
        return *error;
    }
    let Some(revision) = headers
        .get("x-degauss-revision")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() < 128)
    else {
        return failure(
            StatusCode::BAD_REQUEST,
            "revision",
            "Refresh the editor before uploading artwork.",
        );
    };
    dispatch(
        &state,
        Request::Upload {
            revision: revision.into(),
            target,
            body: body.to_vec(),
        },
    )
    .await
}

async fn events(State(state): State<Arc<HttpState>>, headers: HeaderMap) -> Response {
    if let Err(error) = authenticated(&state, &headers, false) {
        return *error;
    }
    let Ok(permit) = state.streams.clone().try_acquire_owned() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Too many live browser connections. Close an unused tab.",
        );
    };
    let stream = BroadcastStream::new(state.events.subscribe())
        .take_while(move |result| {
            !matches!(result,Ok(value) if value=="web-server-closing")
                && authenticated(&state, &headers, false).is_ok()
        })
        .map(move |result| {
            let _keep_until_disconnect = &permit;
            let data = match result {
                Ok(data) => data,
                Err(_) => json!({"type":"resync"}).to_string(),
            };
            Ok::<_, Infallible>(Event::default().data(data))
        });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "degauss-web-network-{}",
                auth::random_token().unwrap()
            ));
            auth::set_password(&path, "network-test-password").unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    struct HttpReply {
        status: u16,
        headers: BTreeSet<(String, String)>,
        body: Vec<u8>,
    }
    impl HttpReply {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap()
        }
    }
    fn config(mode: Mode) -> Config {
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        Config {
            mode,
            port: reservation.local_addr().unwrap().port(),
        }
    }
    fn start(config: &Config, path: &Path, bridge: &Bridge) -> Server {
        let server = Server::start(config, path.to_path_buf(), &[], bridge).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = server.status.lock().unwrap().clone();
            match status {
                Status::Listening => return server,
                Status::Failed(error) => panic!("{error}"),
                Status::Starting => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    fn wire(method: &str, path: &str, port: u16, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut request=format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",body.len()).into_bytes();
        request.extend_from_slice(body);
        request
    }
    fn stream(port: u16) -> TcpStream {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }
    fn exchange(connection: &mut (impl Read + Write), request: &[u8]) -> HttpReply {
        connection.write_all(request).unwrap();
        connection.flush().unwrap();
        let mut reader = BufReader::new(connection);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let status = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        let mut headers = BTreeSet::new();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            let (key, value) = line.split_once(':').unwrap();
            headers.insert((key.to_ascii_lowercase(), value.trim().to_string()));
        }
        let size = headers
            .iter()
            .find(|(key, _)| key == "content-length")
            .map(|(_, value)| value.parse::<usize>().unwrap())
            .expect("bounded response length");
        let mut body = vec![0; size];
        reader.read_exact(&mut body).unwrap();
        HttpReply {
            status,
            headers,
            body,
        }
    }
    fn http(config: &Config, method: &str, path: &str, headers: &str, body: &[u8]) -> HttpReply {
        exchange(
            &mut stream(config.port),
            &wire(method, path, config.port, headers, body),
        )
    }
    fn session_headers(response: &HttpReply) -> String {
        format!(
            "Cookie: {}\r\nX-Degauss-CSRF: {}\r\nContent-Type: application/json\r\n",
            response
                .header("set-cookie")
                .unwrap()
                .split(';')
                .next()
                .unwrap(),
            response.json()["csrf"].as_str().unwrap()
        )
    }

    #[test]
    fn session_cookie_is_found_across_http2_fields_without_crossing_protocols() {
        for values in [
            vec!["degauss-http=plain; __Host-degauss=encrypted"],
            vec!["degauss-http=plain", "__Host-degauss=encrypted"],
            vec!["__Host-degauss=encrypted", "degauss-http=plain"],
        ] {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(header::COOKIE, HeaderValue::from_static(value));
            }
            assert_eq!(token(&headers, Mode::Https), Some("encrypted"));
            assert_eq!(token(&headers, Mode::Http), Some("plain"));
        }
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("degauss-http=plain"),
        );
        assert_eq!(token(&headers, Mode::Https), None);
    }

    #[test]
    fn host_validation_accepts_http2_authority_and_preserves_http1_restrictions() {
        use axum::http::{Request, Version};
        let allowed = BTreeSet::from(["localhost:8443".to_string()]);
        for (version, uri, header, expected) in [
            (Version::HTTP_11, "/", Some("localhost:8443"), true),
            (Version::HTTP_11, "/", Some("wrong.example:8443"), false),
            (Version::HTTP_11, "/", None, false),
            (Version::HTTP_2, "https://localhost:8443/", None, true),
            (Version::HTTP_2, "https://wrong.example:8443/", None, false),
            (
                Version::HTTP_2,
                "https://localhost:8443/",
                Some("localhost:8443"),
                true,
            ),
            (
                Version::HTTP_2,
                "https://localhost:8443/",
                Some("wrong.example:8443"),
                false,
            ),
            (
                Version::HTTP_2,
                "https://wrong.example:8443/",
                Some("localhost:8443"),
                false,
            ),
        ] {
            let mut builder = Request::builder().version(version).uri(uri);
            if let Some(header) = header {
                builder = builder.header(header::HOST, header);
            }
            let request = builder.body(axum::body::Body::empty()).unwrap();
            assert_eq!(
                request_host(&request).is_some_and(|host| allowed.contains(host)),
                expected,
                "{version:?} {uri} Host={header:?}"
            );
        }
    }

    #[test]
    fn http_authentication_request_boundary_and_restart_use_real_sockets() {
        let directory = TestDirectory::new();
        let config = config(Mode::Http);
        let bridge = Bridge::default();
        let server = start(&config, &directory.0, &bridge);
        let page = http(&config, "GET", "/", "", b"");
        assert_eq!(page.status, 200);
        assert!(page
            .header("content-security-policy")
            .unwrap()
            .contains("frame-ancestors 'none'"));
        assert_eq!(http(&config, "GET", "/api/v1/session", "", b"").status, 401);
        let mut wrong_host = TcpStream::connect(("127.0.0.1", config.port)).unwrap();
        let request = String::from_utf8(wire("GET", "/", config.port, "", b""))
            .unwrap()
            .replace(
                &format!("Host: localhost:{}", config.port),
                "Host: wrong.example",
            );
        assert_eq!(exchange(&mut wrong_host, request.as_bytes()).status, 403);
        assert_eq!(
            http(&config, "GET", "/api/v1/media/test", "", b"").status,
            401
        );
        assert_eq!(
            http(
                &config,
                "POST",
                "/api/v1/command",
                "Content-Type: application/json\r\n",
                br#"{"action":"state"}"#
            )
            .status,
            401
        );
        assert_eq!(
            http(
                &config,
                "POST",
                "/api/v1/login",
                "Content-Type: application/json\r\nOrigin: https://wrong.example\r\n",
                br#"{"password":"network-test-password"}"#
            )
            .status,
            403
        );
        let wrong = http(
            &config,
            "POST",
            "/api/v1/login",
            "Content-Type: application/json\r\n",
            br#"{"password":"incorrect"}"#,
        );
        assert_eq!(wrong.status, 401);
        assert_eq!(
            http(
                &config,
                "POST",
                "/api/v1/login",
                "Content-Type: application/json\r\n",
                br#"{"password":"network-test-password"}"#
            )
            .status,
            429
        );
        std::thread::sleep(Duration::from_secs(2));
        let signed = http(
            &config,
            "POST",
            "/api/v1/login",
            "Content-Type: application/json\r\n",
            br#"{"password":"network-test-password","remember":true}"#,
        );
        assert_eq!(signed.status, 200);
        let cookie = signed.header("set-cookie").unwrap();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        assert!(!cookie.contains("Secure"));
        assert!(cookie.contains("Max-Age="));
        let headers = session_headers(&signed);
        assert_eq!(
            http(&config, "GET", "/api/v1/session", &headers, b"").status,
            200
        );
        assert_eq!(
            http(
                &config,
                "POST",
                "/api/v1/command",
                &format!(
                    "Cookie: {}\r\nContent-Type: application/json\r\n",
                    cookie.split(';').next().unwrap()
                ),
                br#"{"action":"state"}"#
            )
            .status,
            403
        );
        assert_eq!(
            http(
                &config,
                "POST",
                "/api/v1/command",
                &headers,
                br#"{"action":"launch","id":"test","path":"/anything"}"#
            )
            .status,
            422
        );
        std::thread::scope(|scope| {
            let request = scope.spawn(|| {
                http(
                    &config,
                    "POST",
                    "/api/v1/command",
                    &headers,
                    br#"{"action":"state"}"#,
                )
            });
            let pending = bridge
                .requests
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            assert!(
                matches!(pending.request, Request::Command(command) if matches!(*command,Command::State))
            );
            assert!(pending
                .reply
                .send(Ok(Reply::Json(json!({"boundary":"app-queue"}))))
                .is_ok());
            assert_eq!(request.join().unwrap().json()["boundary"], "app-queue");
        });
        // The upload override must not inherit the ordinary one-MiB JSON limit.
        let bytes = vec![0u8; 1024 * 1024 + 1];
        std::thread::scope(|scope| {
            let request = scope.spawn(|| {
                http(
                    &config,
                    "POST",
                    "/api/v1/upload/logo",
                    &(headers.clone() + "X-Degauss-Revision: test\r\n"),
                    &bytes,
                )
            });
            let pending = bridge
                .requests
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            assert!(matches!(&pending.request,Request::Upload{body,..} if body.len()==bytes.len()));
            assert!(pending
                .reply
                .send(Err(Error::new("artwork", "Not an image")))
                .is_ok());
            assert_eq!(request.join().unwrap().status, 400);
        });
        for (name, bytes, expected) in [
            (
                "artwork.png",
                include_bytes!("../../assets/degauss-logo.png").as_slice(),
                200,
            ),
            (
                "417263616465",
                include_bytes!("../../assets/degauss-logo.png").as_slice(),
                200,
            ),
            ("not-art.png", b"This is not an image".as_slice(), 404),
        ] {
            let path = directory.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            std::thread::scope(|scope| {
                let request =
                    scope.spawn(|| http(&config, "GET", "/api/v1/media/art", &headers, b""));
                let pending = bridge
                    .requests
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                assert!(matches!(pending.request, Request::Media(ref id) if id=="art"));
                assert!(pending.reply.send(Ok(Reply::Media { path })).is_ok());
                let response = request.join().unwrap();
                assert_eq!(response.status, expected);
                if expected == 200 {
                    assert_eq!(response.body, bytes);
                } else {
                    assert!(response.json()["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("PNG or JPEG"));
                }
            });
        }
        // Real browsers submit more thumbnails than there are active slots.
        // Hold all eight slots first, then drain the burst: nothing is rejected
        // and only eight requests reach the native bridge at a time.
        let artwork = directory.0.join("artwork.png");
        std::thread::scope(|scope| {
            let requests: Vec<_> = (0..24)
                .map(|_| scope.spawn(|| http(&config, "GET", "/api/v1/media/art", &headers, b"")))
                .collect();
            let active: Vec<_> = (0..8)
                .map(|_| {
                    bridge
                        .requests
                        .recv_timeout(Duration::from_secs(3))
                        .unwrap()
                })
                .collect();
            assert!(matches!(
                bridge.requests.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            assert_eq!(
                http(&config, "GET", "/api/v1/media/art", "", b"").status,
                401
            );
            for pending in active {
                assert!(pending
                    .reply
                    .send(Ok(Reply::Media {
                        path: artwork.clone()
                    }))
                    .is_ok());
            }
            for _ in 8..24 {
                let pending = bridge
                    .requests
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                assert!(pending
                    .reply
                    .send(Ok(Reply::Media {
                        path: artwork.clone()
                    }))
                    .is_ok());
            }
            for request in requests {
                let response = request.join().unwrap();
                assert_eq!(response.status, 200);
                assert_eq!(
                    response.body,
                    include_bytes!("../../assets/degauss-logo.png")
                );
            }
        });
        assert!(Server::start(&config, directory.0.clone(), &[], &bridge).is_err());
        drop(server);
        let server = start(&config, &directory.0, &bridge);
        assert_eq!(
            http(&config, "GET", "/api/v1/session", &headers, b"").status,
            200
        );
        let mut stream = TcpStream::connect(("127.0.0.1", config.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(&wire("GET", "/api/v1/events", config.port, &headers, b""))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while !bridge.has_listeners() {
            assert!(Instant::now() < until, "Event stream did not subscribe");
            std::thread::sleep(Duration::from_millis(5));
        }
        bridge.event(&json!({"type":"state","revision":"socket-test"}));
        let mut received = String::new();
        let mut chunk = [0; 4096];
        while !received.contains("socket-test") {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0);
            received.push_str(std::str::from_utf8(&chunk[..count]).unwrap());
        }
        assert!(received.contains("text/event-stream"));
        assert_eq!(
            http(&config, "POST", "/api/v1/logout", &headers, b"{}").status,
            200
        );
        bridge.event(&json!({"type":"state","revision":"must-not-be-delivered"}));
        let mut remaining = String::new();
        stream.read_to_string(&mut remaining).unwrap();
        assert!(
            !remaining.contains("must-not-be-delivered"),
            "Signing out closes the event stream"
        );
        assert_eq!(
            http(&config, "GET", "/api/v1/session", &headers, b"").status,
            401
        );
        drop(server);
        assert!(TcpStream::connect(("127.0.0.1", config.port)).is_err());
    }

    fn tls(
        config: &Config,
        certificate: &str,
        name: &str,
    ) -> rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(certificate.as_bytes()).unwrap())
            .unwrap();
        let client = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        rustls::StreamOwned::new(
            rustls::ClientConnection::new(
                Arc::new(client),
                ServerName::try_from(name.to_string()).unwrap(),
            )
            .unwrap(),
            stream(config.port),
        )
    }
    #[test]
    fn https_uses_persistent_trusted_identity_and_secure_session_cookies() {
        let directory = TestDirectory::new();
        let config = config(Mode::Https);
        let bridge = Bridge::default();
        let server = start(&config, &directory.0, &bridge);
        let (certificate, key) = certificate(&directory.0, &[]).unwrap();
        let page = exchange(
            &mut tls(&config, &certificate, "localhost"),
            &wire("GET", "/", config.port, "", b""),
        );
        assert_eq!(page.status, 200);
        let signed = exchange(
            &mut tls(&config, &certificate, "127.0.0.1"),
            &wire(
                "POST",
                "/api/v1/login",
                config.port,
                "Content-Type: application/json\r\n",
                br#"{"password":"network-test-password"}"#,
            ),
        );
        assert_eq!(signed.status, 200);
        assert!(signed
            .header("set-cookie")
            .unwrap()
            .starts_with("__Host-degauss="));
        assert!(signed.header("set-cookie").unwrap().contains("; Secure"));
        let mut wrong = tls(&config, &certificate, "wrong.example");
        assert!(
            wrong
                .write_all(&wire("GET", "/", config.port, "", b""))
                .is_err()
                || wrong.read(&mut [0; 1]).is_err()
        );
        let untrusted = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();
        let socket = TcpStream::connect(("127.0.0.1", config.port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut untrusted = rustls::StreamOwned::new(
            rustls::ClientConnection::new(
                Arc::new(untrusted),
                ServerName::try_from("localhost").unwrap(),
            )
            .unwrap(),
            socket,
        );
        assert!(
            untrusted
                .write_all(&wire("GET", "/", config.port, "", b""))
                .is_err()
                || untrusted.read(&mut [0; 1]).is_err()
        );
        assert_eq!(
            super::certificate(&directory.0, &[]).unwrap(),
            (certificate.clone(), key)
        );
        assert!(super::certificate(&directory.0, &["192.0.2.20".parse().unwrap()]).is_err());
        drop(server);
        let server = start(&config, &directory.0, &bridge);
        assert_eq!(
            exchange(
                &mut tls(&config, &certificate, "localhost"),
                &wire("GET", "/", config.port, "", b"")
            )
            .status,
            200
        );
        drop(server);
    }
}
