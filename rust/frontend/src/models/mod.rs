// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Globals set by main() before the QML engine is created. QML singletons
// subscribe to data through `STORE` rather than receiving it via
// constructor injection — this side-channel is the only place
// cxx-qt's `Default::default()` boundary lets us reach into.
//
// The QML singletons are constructed *after* init_globals runs, so any
// `expect`/`panic` below represents an internal wiring bug (double-init
// or use-before-init) and is correctly fatal.
//
// `STORE` owns the `Client` and runtime and hands out shared
// `RemoteResource`s per (endpoint, args). Singletons reach it via
// `global_store()` — `bind_to_endpoint!` is the standard caller and
// closes the sync-seed contract from `docs/cxx-qt-bridge.md`
// structurally.

#![allow(
    clippy::panic,
    clippy::expect_used,
    reason = "process-local init invariants: any violation is a wiring bug and must be fatal"
)]

pub mod alternate_versions;
pub mod app_state;
pub mod app_status;
pub mod browse;
pub mod build_info;
pub mod categories;
pub mod crt_video;
pub mod favorites;
pub mod favorites_state;
pub mod game_info;
pub mod games;
pub mod games_state;
pub mod hub_state;
pub mod image_overrides;
pub mod input;
pub mod log_upload;
pub mod media_status;
pub mod nav_timing;
pub mod notice;
pub mod platform;
pub mod qr_code;
pub mod random_pick;
pub mod recents;
pub mod recents_state;
pub mod runtime;
pub mod settings;
pub mod system_launchers;
pub mod system_status;
pub mod systems;
pub mod systems_state;
pub mod tag_utils;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::{Handle, Runtime};
use tracing::{error, warn};
use zaparoo_core::{
    config::save_hidden_browse_prefs, persist::PersistedState, platform_paths::config_file_path,
    store::Store,
};

// `RUNTIME_OWNER` holds the actual `Runtime` and is the only thing that
// can call `shutdown_timeout`. `HANDLE` is what callers spawn through —
// cloning a `Handle` is cheap and doesn't entangle Drop ordering. The
// split exists so `zaparoo_rust_shutdown` can take the owning `Runtime`
// out and shut it down on the main thread, draining workers while their
// TLS storage is still alive. Without this split, the static dropped at
// `__cxa_finalize` and worker exits raced with TLS teardown — any
// tracing event from a half-exited worker hit `LocalKey::with` and
// double-panicked into SIGABRT (exit 134).
static RUNTIME_OWNER: Mutex<Option<Runtime>> = Mutex::new(None);
static HANDLE: OnceLock<Handle> = OnceLock::new();
static STORE: OnceLock<Arc<Store>> = OnceLock::new();
#[derive(Debug, Clone, Default)]
pub struct HiddenBrowsePrefs {
    pub hidden_categories: Vec<String>,
    pub hidden_system_ids: Vec<String>,
}

static PERSIST_STATE: OnceLock<Arc<Mutex<PersistedState>>> = OnceLock::new();
static HIDDEN_BROWSE_PREFS: OnceLock<Arc<Mutex<HiddenBrowsePrefs>>> = OnceLock::new();
static INPUT_BINDINGS: OnceLock<HashMap<i32, String>> = OnceLock::new();
static CORE_IS_LOCAL: OnceLock<bool> = OnceLock::new();

pub fn init_globals(
    runtime: Runtime,
    store: Arc<Store>,
    persist_state: Arc<Mutex<PersistedState>>,
    hidden_browse_prefs: Arc<Mutex<HiddenBrowsePrefs>>,
    input_bindings: HashMap<i32, String>,
    core_is_local: bool,
) {
    #[cfg(feature = "update")]
    zaparoo_update::init_runtime(runtime.handle().clone());
    HANDLE
        .set(runtime.handle().clone())
        .unwrap_or_else(|_| panic!("HANDLE already initialized"));
    {
        let mut owner = RUNTIME_OWNER
            .lock()
            .unwrap_or_else(|_| panic!("RUNTIME_OWNER mutex poisoned"));
        assert!(owner.is_none(), "RUNTIME_OWNER already initialized");
        *owner = Some(runtime);
    }
    STORE
        .set(store)
        .unwrap_or_else(|_| panic!("STORE already initialized"));
    PERSIST_STATE
        .set(persist_state)
        .unwrap_or_else(|_| panic!("PERSIST_STATE already initialized"));
    HIDDEN_BROWSE_PREFS
        .set(hidden_browse_prefs)
        .unwrap_or_else(|_| panic!("HIDDEN_BROWSE_PREFS already initialized"));
    INPUT_BINDINGS
        .set(input_bindings)
        .unwrap_or_else(|_| panic!("INPUT_BINDINGS already initialized"));
    CORE_IS_LOCAL
        .set(core_is_local)
        .unwrap_or_else(|_| panic!("CORE_IS_LOCAL already initialized"));
}

pub fn global_handle() -> Handle {
    HANDLE.get().expect("HANDLE not initialized").clone()
}

/// Drains the tokio runtime on the calling thread with a hard deadline.
/// Idempotent: a second call after the runtime has been taken is a
/// silent no-op so the FFI shutdown entry point can be safely invoked
/// from both `aboutToQuit` and any future cleanup paths.
///
/// Must run on a thread whose TLS is still alive — i.e. before main
/// returns and before `__cxa_finalize`. That's what makes this the
/// right answer to the SIGABRT-on-quit race: workers finish (or get
/// cancelled) while every thread's `thread_local!` storage is still
/// addressable, so a final tracing event from a finishing task can't
/// land on a half-destroyed dispatcher TLS.
pub fn shutdown_runtime(timeout: Duration) {
    let runtime = {
        let mut owner = match RUNTIME_OWNER.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                error!("RUNTIME_OWNER mutex poisoned at shutdown; recovering");
                poisoned.into_inner()
            }
        };
        owner.take()
    };
    if let Some(runtime) = runtime {
        runtime.shutdown_timeout(timeout);
    }
}

pub fn global_store() -> Arc<Store> {
    STORE.get().expect("STORE not initialized").clone()
}

pub fn input_bindings() -> HashMap<i32, String> {
    INPUT_BINDINGS
        .get()
        .expect("INPUT_BINDINGS not initialized")
        .clone()
}

pub fn core_is_local() -> bool {
    *CORE_IS_LOCAL.get().expect("CORE_IS_LOCAL not initialized")
}

pub fn persist_state() -> Arc<Mutex<PersistedState>> {
    PERSIST_STATE
        .get()
        .expect("PERSIST_STATE not initialized")
        .clone()
}

fn hidden_browse_prefs() -> Arc<Mutex<HiddenBrowsePrefs>> {
    HIDDEN_BROWSE_PREFS
        .get()
        .expect("HIDDEN_BROWSE_PREFS not initialized")
        .clone()
}

pub fn with_hidden_browse_prefs_read<R>(f: impl FnOnce(&HiddenBrowsePrefs) -> R) -> R {
    let shared = hidden_browse_prefs();
    let guard = shared
        .lock()
        .inspect_err(|e| error!("hidden browse prefs mutex poisoned: {e}"))
        .expect("hidden browse prefs mutex poisoned");
    f(&guard)
}

pub fn with_hidden_browse_prefs_mut<R>(f: impl FnOnce(&mut HiddenBrowsePrefs) -> R) -> R {
    let shared = hidden_browse_prefs();
    let mut guard = shared
        .lock()
        .inspect_err(|e| error!("hidden browse prefs mutex poisoned: {e}"))
        .expect("hidden browse prefs mutex poisoned");
    let result = f(&mut guard);
    if let Err(e) = save_hidden_browse_prefs(
        &config_file_path(),
        &guard.hidden_categories,
        &guard.hidden_system_ids,
    ) {
        warn!("could not save hidden browse prefs: {e}");
    }
    result
}

pub fn try_with_persist_read<R>(f: impl FnOnce(&PersistedState) -> R) -> Option<R> {
    let shared = PERSIST_STATE.get()?.clone();
    let guard = shared
        .lock()
        .inspect_err(|e| error!("persist mutex poisoned: {e}"))
        .expect("persist mutex poisoned");
    Some(f(&guard))
}

/// Read the persisted state under a closure. Centralises the
/// lock + log + panic-on-poison chain so the 5+ persist call sites
/// can't drift in their error message or skip the log breadcrumb.
pub fn with_persist_read<R>(f: impl FnOnce(&PersistedState) -> R) -> R {
    let shared = persist_state();
    let guard = shared
        .lock()
        .inspect_err(|e| error!("persist mutex poisoned: {e}"))
        .expect("persist mutex poisoned");
    f(&guard)
}

/// Mutate the persisted state under a closure. Same poisoning
/// contract as `with_persist_read`.
pub fn with_persist_mut<R>(f: impl FnOnce(&mut PersistedState) -> R) -> R {
    let shared = persist_state();
    let mut guard = shared
        .lock()
        .inspect_err(|e| error!("persist mutex poisoned: {e}"))
        .expect("persist mutex poisoned");
    f(&mut guard)
}
