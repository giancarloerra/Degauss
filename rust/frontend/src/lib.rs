// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0

#[macro_use]
mod bind;
pub mod image_overrides;
mod media_art_db;
mod media_browse_db;
mod media_db;
mod media_favorites_db;
mod media_history_db;
mod media_image_cache;
mod media_meta_cache;
mod media_meta_db;
mod media_search_db;
mod mister_runtime;
mod models;
pub mod system_logos;
pub mod system_name_overrides;
pub mod system_names;
pub mod system_region;

/// Called from the Qt message handler in main.cpp. `level` is `QtMsgType`
/// cast to u8. `msg_ptr`/`msg_len` are a UTF-8 slice owned by the caller.
/// Routes Qt log output through the tracing registry so it lands in the
/// same stderr + file sinks as Rust log messages.
///
/// # Safety
///
/// `msg_ptr` must point to `msg_len` bytes of valid UTF-8 that remain live
/// for the duration of this call. The Qt message handler always provides a
/// valid `QString::toUtf8()` slice, so this invariant holds in practice.
#[no_mangle]
pub unsafe extern "C" fn zaparoo_log_qt(level: u8, msg_ptr: *const u8, msg_len: usize) {
    // SAFETY: Caller guarantees `msg_ptr`..`msg_ptr + msg_len` is a valid
    // UTF-8 byte slice (Qt's message handler passes QString::toUtf8()).
    let msg =
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(msg_ptr, msg_len)) };
    match level {
        0 /* QtDebugMsg    */ => tracing::debug!(target: "qt", "{}", msg),
        4 /* QtInfoMsg     */ => tracing::info!(target: "qt", "{}", msg),
        1 /* QtWarningMsg  */ => tracing::warn!(target: "qt", "{}", msg),
        2 /* QtCriticalMsg */ => tracing::error!(target: "qt", "{}", msg),
        3 /* QtFatalMsg    */ => tracing::error!(target: "qt", "FATAL: {}", msg),
        _ => tracing::info!(target: "qt", "{}", msg),
    }
}

use std::ffi::{c_char, c_int, CString};
use std::io::Write as _;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use zaparoo_core::{
    client::Client,
    config::load_config,
    logger::{debug_logging_enabled, install},
    persist, platform,
    platform_paths::{config_file_path, custom_dir, log_file_path, stderr_log_path},
    store::Store,
};

/// Pre-opened append-mode fd to `frontend.log`, used by the native-crash
/// signal handler. A signal handler must be async-signal-safe — it cannot
/// allocate, format, or call `tracing::*` — so the fd is opened during
/// init and the handler writes a fixed marker via `libc::write(2)`.
/// `-1` means "not yet installed"; the handler no-ops in that case.
static CRASH_FD: AtomicI32 = AtomicI32::new(-1);

/// Routes our own stderr to a file before any other init runs. The
/// `MiSTer` wrapper launches the binary with `2>/dev/null`, so the
/// chained default panic hook (`thread '...' panicked at ...`), libc
/// `abort()` diagnostics, glibc backtrace prints, and any kernel
/// signal-default output would otherwise vanish. After this call every
/// byte written to fd 2 lands in `frontend.stderr.log` for the process
/// lifetime.
fn redirect_stderr() {
    let path = stderr_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let raw = file.as_raw_fd();
    // SAFETY: dup2 with two valid fds. STDERR_FILENO is the constant 2
    // and `raw` is the fd of the just-opened file. On failure dup2
    // returns -1 which we don't act on (worst case: stderr still points
    // at the original /dev/null).
    unsafe {
        libc::dup2(raw, libc::STDERR_FILENO);
    }
    // `file` drops at end of scope, which closes `raw`. The kernel
    // keeps the underlying open file description alive because
    // STDERR_FILENO still refers to it after the dup2 above — fd 2
    // remains valid for the process lifetime, fed by every subsequent
    // write to stderr.
    drop(file);

    // Boot marker so successive runs are visually separated in the
    // append-mode file. Direct write to stderr (now redirected) instead
    // of `eprintln!` so we don't trip clippy::print_stderr.
    let pid = std::process::id();
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let _ = writeln!(
        std::io::stderr().lock(),
        "\n=== frontend pid={pid} starting at {timestamp} ==="
    );
}

/// Resolved language override, cached after [`zaparoo_rust_init`] so the
/// C++ side can pull it via [`zaparoo_rust_language_code`] without re-
/// loading config. An empty string means "use `QLocale::system()`".
static LANGUAGE_CODE: OnceLock<CString> = OnceLock::new();
static CRT_NATIVE_PATH_ENABLED: OnceLock<bool> = OnceLock::new();
static VIDEO_WIDTH: OnceLock<u32> = OnceLock::new();
static VIDEO_HEIGHT: OnceLock<u32> = OnceLock::new();
static CRT_H_OFFSET: OnceLock<i32> = OnceLock::new();
static CRT_V_OFFSET: OnceLock<i32> = OnceLock::new();
static DEBUG_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
static STARTUP_TRACE_ORIGIN: OnceLock<Instant> = OnceLock::new();

pub(crate) fn startup_trace(stage: impl AsRef<str>) {
    if !DEBUG_LOGGING_ENABLED.get().copied().unwrap_or(false) {
        return;
    }
    let stage = stage.as_ref();
    let elapsed_ms = STARTUP_TRACE_ORIGIN
        .get()
        .map_or(0, |origin| origin.elapsed().as_millis());
    tracing::debug!(target: "startup", elapsed_ms, stage, "startup trace");
}

/// Returns the resolved UI language override as a NUL-terminated UTF-8
/// string. An empty string signals "follow `QLocale::system()`"; any
/// other value is a BCP-47 tag passed straight to `QLocale(code)` in
/// C++. The pointer is valid for the process lifetime.
///
/// Called before [`zaparoo_rust_init`] returns an empty string, since
/// the `OnceLock` has not yet been populated.
#[no_mangle]
pub extern "C" fn zaparoo_rust_language_code() -> *const c_char {
    LANGUAGE_CODE
        .get()
        .map_or_else(|| c"".as_ptr(), |s| s.as_ptr())
}

#[no_mangle]
pub extern "C" fn zaparoo_rust_crt_native_path_enabled() -> bool {
    CRT_NATIVE_PATH_ENABLED.get().copied().unwrap_or(false)
}

/// Configured CRT logical video width from `[video] width` in
/// `frontend.toml`. Used by the desktop preview wrapper to size the
/// inner scene at the intended 240p (or whatever the user has set)
/// before integer-upscaling for display. Cached after
/// [`zaparoo_rust_init`]; reads before init return `0`.
#[no_mangle]
pub extern "C" fn zaparoo_rust_video_width() -> u32 {
    VIDEO_WIDTH.get().copied().unwrap_or(0)
}

/// Configured CRT logical video height from `[video] height` in
/// `frontend.toml`. See [`zaparoo_rust_video_width`] for usage.
#[no_mangle]
pub extern "C" fn zaparoo_rust_video_height() -> u32 {
    VIDEO_HEIGHT.get().copied().unwrap_or(0)
}

/// Persisted native CRT horizontal centering trim (pixels, + = right),
/// already clamped to the core's honored range. Pulled by
/// `native_video_writer.cpp` at init so word1 carries the saved
/// calibration from the first published frame. Reads before
/// [`zaparoo_rust_init`] return `0`.
#[no_mangle]
pub extern "C" fn zaparoo_rust_crt_h_offset() -> i32 {
    CRT_H_OFFSET.get().copied().unwrap_or(0)
}

/// Persisted native CRT vertical centering trim (lines, + = down). See
/// [`zaparoo_rust_crt_h_offset`].
#[no_mangle]
pub extern "C" fn zaparoo_rust_crt_v_offset() -> i32 {
    CRT_V_OFFSET.get().copied().unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn zaparoo_rust_debug_logging_enabled() -> bool {
    DEBUG_LOGGING_ENABLED.get().copied().unwrap_or(false)
}

/// Records a startup trace stage from the C++ bootstrap path.
///
/// # Safety
///
/// `stage_ptr` must point to `stage_len` bytes of valid UTF-8 for the
/// duration of this call, unless it is null or `stage_len` is zero.
#[no_mangle]
pub unsafe extern "C" fn zaparoo_rust_trace_startup(stage_ptr: *const u8, stage_len: usize) {
    if stage_ptr.is_null() || stage_len == 0 {
        startup_trace("cpp:<empty>");
        return;
    }
    // SAFETY: Caller guarantees a valid UTF-8 slice for this call.
    let stage =
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(stage_ptr, stage_len)) };
    startup_trace(stage);
}

/// Installs a panic hook that routes Rust panics through the tracing
/// registry so they land in the same stderr + JSONL sinks as normal log
/// output. Without this, a panic on a tokio worker goes to raw stderr
/// only — invisible on `MiSTer` where stderr is not captured. The hook
/// chains to the previous default, preserving abort-on-panic semantics.
///
/// Also writes the panic line synchronously to `frontend.log` via
/// blocking `OpenOptions::append`. The async tracing-appender writer
/// can lose its buffered tail if the process aborts (e.g. SIGABRT after
/// the chained default hook); the blocking append is durable before we
/// hand off.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let location = info.location().map_or_else(
            || "<unknown>".to_string(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        // `std::thread::current()` and `Backtrace::capture()` both touch
        // TLS internally; on a thread mid-destruction they would
        // re-panic and turn this hook into an abort. Catch-unwind those
        // and substitute fallback strings so the hook always finishes.
        let thread_name = std::panic::catch_unwind(|| {
            std::thread::current()
                .name()
                .map_or_else(|| "<unnamed>".to_string(), str::to_string)
        })
        .unwrap_or_else(|_| "<tls-gone>".to_string());
        let backtrace = std::panic::catch_unwind(std::backtrace::Backtrace::capture)
            .map_or_else(|_| String::new(), |bt| bt.to_string());
        let line = format!("thread '{thread_name}' panicked at {location}: {msg}\n{backtrace}");

        // Best-effort blocking write to frontend.log. Each call opens a
        // fresh fd in append mode so we don't depend on any state that
        // a partially-corrupted process might have torn down. No TLS
        // access here — File I/O via libc::write goes straight to the
        // kernel.
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path())
        {
            let _ = writeln!(f, "PANIC: {line}");
            let _ = f.flush();
            let _ = f.sync_data();
        }

        // Mirror to stderr via the libc-backed Stderr handle (no TLS).
        // We deliberately do not call the default panic hook or
        // `tracing::error!` — both reach into TLS for thread-name
        // formatting and dispatcher lookup, which on a teardown-phase
        // thread would re-panic into SIGABRT before the original cause
        // is even visible.
        let mut stderr = std::io::stderr();
        let _ = std::io::Write::write_all(&mut stderr, b"thread '");
        let _ = std::io::Write::write_all(&mut stderr, thread_name.as_bytes());
        let _ = std::io::Write::write_all(&mut stderr, b"' panicked at ");
        let _ = std::io::Write::write_all(&mut stderr, location.as_bytes());
        let _ = std::io::Write::write_all(&mut stderr, b": ");
        let _ = std::io::Write::write_all(&mut stderr, msg.as_bytes());
        let _ = std::io::Write::write_all(&mut stderr, b"\n");
    }));
}

/// Native-crash signal handler. Writes a fixed boundary marker to the
/// pre-opened `frontend.log` fd, then re-raises with the default
/// disposition so the kernel's normal handling (core dump, exit code)
/// runs. Async-signal-safe — no allocations, no formatting, no tracing.
extern "C" fn crash_handler(signum: c_int) {
    let fd = CRASH_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let prefix = b"\n*** native crash: signal ";
        let name = signal_name(signum);
        let suffix = b" - see frontend.stderr.log for details ***\n";
        // SAFETY: write(2) is async-signal-safe per POSIX. The fd was
        // opened in init and stored in CRASH_FD; if it's still >= 0 we
        // can write to it. Buffer pointers are static byte literals or
        // an &'static str returned by signal_name.
        unsafe {
            libc::write(fd, prefix.as_ptr().cast(), prefix.len());
            libc::write(fd, name.as_ptr().cast(), name.len());
            libc::write(fd, suffix.as_ptr().cast(), suffix.len());
        }
    }
    // Reset to default disposition and re-raise so the kernel's normal
    // signal handling runs after we logged the boundary marker.
    // SAFETY: signal() and raise() are async-signal-safe per POSIX.
    unsafe {
        libc::signal(signum, libc::SIG_DFL);
        libc::raise(signum);
    }
}

const fn signal_name(signum: c_int) -> &'static str {
    match signum {
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGBUS => "SIGBUS",
        libc::SIGABRT => "SIGABRT",
        libc::SIGILL => "SIGILL",
        libc::SIGFPE => "SIGFPE",
        _ => "UNKNOWN",
    }
}

/// Installs a `SIGSEGV/SIGBUS/SIGABRT/SIGILL/SIGFPE` handler that writes
/// a boundary marker to `frontend.log` before re-raising. Catches native
/// crashes from cxx-qt FFI, static Qt platform code, and `qFatal()`
/// aborts that would otherwise bypass the Rust panic hook entirely.
fn install_crash_signal_handler() {
    let path = log_file_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let raw = file.as_raw_fd();
    CRASH_FD.store(raw, Ordering::Relaxed);
    // Leak the File so its drop doesn't close the fd we just stored in
    // CRASH_FD. The signal handler reads this fd for the process lifetime.
    std::mem::forget(file);

    for &sig in &[
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGABRT,
        libc::SIGILL,
        libc::SIGFPE,
    ] {
        // SAFETY: signal() with a valid signal number and an async-
        // signal-safe handler. The handler resets to SIG_DFL and
        // re-raises, so this doesn't permanently swallow the signal.
        unsafe {
            libc::signal(sig, crash_handler as *const () as libc::sighandler_t);
        }
    }
}

/// Called by the C++ main before `QGuiApplication` is constructed.
/// Sets up logging, tokio runtime, `MiSTer` pre-Qt env/vmode, WebSocket
/// client, `Store` (which owns the endpoint cache), and model globals.
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn zaparoo_rust_init(crt_native_path_forced: bool) -> c_int {
    let _ = STARTUP_TRACE_ORIGIN.set(Instant::now());
    // FIRST: redirect our own stderr to a file so any panic, abort, or
    // glibc diagnostic in the rest of init lands on disk instead of in
    // the wrapper's /dev/null. Independent of tracing — must run before
    // logger setup so even a panic during `install(&config)` is captured.
    redirect_stderr();

    let config_path = config_file_path();
    let mut config = load_config(&config_path);
    let debug_logging = debug_logging_enabled(&config);
    let _ = DEBUG_LOGGING_ENABLED.set(debug_logging);
    startup_trace("rust:init config loaded");

    // CRT path always renders to one of the native writer's mode
    // geometries (352x240 NTSC, 352x288 PAL, 720x480 480i), selected by
    // the persisted video standard. User-configured [video] dimensions
    // still apply to the normal MiSTer path, but `--crt` overrides them
    // so startup `vmode`, the desktop preview canvas, and the writer's
    // fb0 validation all agree. frontend.toml is the durable source for
    // the standard and offsets (state.toml lives on tmpfs on MiSTer and
    // mirrors it).
    if crt_native_path_forced {
        let standard = zaparoo_core::config::normalize_crt_video_standard(
            config.settings.crt_video_standard.as_deref().unwrap_or(""),
        );
        let (width, height) = zaparoo_core::config::crt_video_dimensions(standard);
        config.video_width = width;
        config.video_height = height;
        let (h_offset, v_offset) = zaparoo_core::config::clamp_crt_offsets(
            config.settings.crt_h_offset.unwrap_or(0),
            config.settings.crt_v_offset.unwrap_or(0),
        );
        let _ = CRT_H_OFFSET.set(h_offset);
        let _ = CRT_V_OFFSET.set(v_offset);
    }

    // Cache the language override so `zaparoo_rust_language_code` (called
    // from main.cpp before the QML engine loads) can return it without
    // re-parsing the TOML. `CString::new` only fails on interior NULs,
    // which a valid BCP-47 tag or the empty sentinel cannot contain —
    // fall back to empty ("use QLocale::system()") if a user manages it.
    let _ = LANGUAGE_CODE.set(CString::new(config.language.clone()).unwrap_or_default());
    let _ = CRT_NATIVE_PATH_ENABLED.set(crt_native_path_forced);
    let _ = VIDEO_WIDTH.set(config.video_width);
    let _ = VIDEO_HEIGHT.set(config.video_height);
    startup_trace("rust:init config cached");

    // Leak the guard — it must live for the process lifetime to keep the
    // file-appender thread running. The OS reclaims it on exit.
    let guard = install(&config);
    Box::leak(Box::new(guard));
    startup_trace("rust:logger installed");

    // Install after logging so panics go through the same sinks; before
    // tokio / client setup so a panic during those lines is captured.
    install_panic_hook();
    startup_trace("rust:panic hook installed");

    // Catches native crashes (SIGSEGV/SIGBUS/SIGABRT/SIGILL/SIGFPE) that
    // never enter the Rust panic hook — cxx-qt FFI faults, static-Qt
    // platform code, qFatal() aborts. Writes a boundary marker to
    // frontend.log before re-raising with the default disposition.
    install_crash_signal_handler();
    startup_trace("rust:crash handler installed");

    tracing::info!("Degauss starting");
    tracing::info!(
        "build {}",
        models::build_info::provenance_string(env!("CARGO_PKG_VERSION"))
    );

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("failed to build tokio runtime: {e}");
            return 1;
        }
    };
    let handle = runtime.handle().clone();
    startup_trace("rust:tokio runtime built");

    mister_runtime::apply_pre_qt_setup(&config, crt_native_path_forced);
    startup_trace("rust:mister pre-qt setup done");

    let client = Client::new(config.core_endpoint.clone(), &handle);
    startup_trace("rust:client created");
    platform::spawn_fetcher(client.clone(), &handle);
    startup_trace("rust:platform fetcher spawned");
    let store = Store::new(client.clone(), handle.clone());
    startup_trace("rust:store created");

    // Load persisted UI state up front so per-screen singletons can seed
    // their properties from a consistent snapshot during Initialize.
    let persist_state = Arc::new(Mutex::new(persist::load()));
    startup_trace("rust:persist loaded");

    // Hidden browse filters are durable preferences, not volatile
    // navigation state. Source them only from frontend.toml so they
    // survive MiSTer's /tmp lifecycle without polluting state.toml.
    let hidden_browse_prefs = Arc::new(Mutex::new(models::HiddenBrowsePrefs {
        hidden_categories: config.settings.hidden_categories.clone(),
        hidden_system_ids: config.settings.hidden_system_ids.clone(),
    }));
    startup_trace("rust:hidden browse prefs loaded");

    // Register the customization root without scanning it. Hub and system
    // image scans run asynchronously after first paint; system display-name
    // overrides are already in the parsed config and need no filesystem work.
    let custom_root = config
        .custom_dir
        .clone()
        .map_or_else(custom_dir, std::path::PathBuf::from);
    image_overrides::configure(&custom_root);
    system_name_overrides::set(config.system_names.clone());
    startup_trace("rust:image override root and name overrides configured");

    // init_globals takes the owning `Runtime` — the static holder is
    // what `zaparoo_rust_shutdown` later drains via `shutdown_timeout`.
    // Other subsystems reach the runtime through `global_handle()`.
    let core_is_local = core_endpoint_is_loopback(&config.core_endpoint);
    models::init_globals(
        runtime,
        store,
        persist_state,
        hidden_browse_prefs,
        config.key_to_action.clone(),
        core_is_local,
    );
    startup_trace("rust:model globals initialized");

    0
}

/// Drains the tokio runtime with a 2-second deadline. Called from the
/// C++ side via `QGuiApplication::aboutToQuit` so workers exit while
/// the main thread is still alive and every thread's TLS storage is
/// still addressable. Without this, static `Runtime` drop ran during
/// `__cxa_finalize` after main returned, racing tokio worker exits
/// with TLS teardown — any final tracing event from a finishing
/// worker hit `LocalKey::with` and double-panicked into SIGABRT.
///
/// Idempotent — safe to call more than once.
#[no_mangle]
pub extern "C" fn zaparoo_rust_shutdown() {
    models::shutdown_runtime(Duration::from_secs(2));
}

/// Called by the C++ main immediately after Rust init. Fires the Zaparoo Core
/// service start (`MiSTer` only, no-op on desktop) early enough to overlap Core
/// boot with Qt/font/QML setup.
#[no_mangle]
pub extern "C" fn zaparoo_rust_post_qt_start() {
    mister_runtime::ensure_core_service_running();
    startup_trace("rust:core service start requested");
}

fn core_endpoint_is_loopback(endpoint: &str) -> bool {
    let Some(host) = endpoint_host(endpoint) else {
        return false;
    };
    let host = host.trim().trim_matches('.').to_lowercase();
    if host == "localhost" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

fn endpoint_host(endpoint: &str) -> Option<&str> {
    let after_scheme = endpoint
        .split_once("://")
        .map_or(endpoint, |(_, rest)| rest);
    let authority = after_scheme.split('/').next()?.rsplit('@').next()?;
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split_once(']').map(|(host, _)| host);
    }
    authority.split(':').next().filter(|host| !host.is_empty())
}

#[cfg(test)]
mod tests {
    use super::core_endpoint_is_loopback;

    #[test]
    fn loopback_core_endpoints_are_local() {
        for endpoint in [
            "ws://127.0.0.1:7497/api/v0.1",
            "ws://127.12.0.2:7497/api/v0.1",
            "ws://localhost:7497/api/v0.1",
            "ws://[::1]:7497/api/v0.1",
        ] {
            assert!(core_endpoint_is_loopback(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn remote_core_endpoints_are_not_local() {
        for endpoint in [
            "ws://10.0.0.50:7497/api/v0.1",
            "ws://mister.local:7497/api/v0.1",
            "ws://192.168.1.9:7497/api/v0.1",
            "",
        ] {
            assert!(!core_endpoint_is_loopback(endpoint), "{endpoint}");
        }
    }
}
