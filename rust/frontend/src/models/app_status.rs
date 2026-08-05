// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// `Browse.AppStatus` — ephemeral connection / catalog health, exposed
// to QML so the UI can render a status surface (status pill, boot
// overlay) when Core is unreachable or the initial catalog fetch
// failed. State is not persisted: it is derived from two channels —
//
//   * `CatalogEndpoint`'s `ResourceStatus<CatalogData>` — collapses the
//     link state and the catalog fetch into the four banner states the
//     UI used to drive the pre-pill connection strip. Kept on
//     `connection_state` so existing callers still observe the
//     "everything together" view.
//
//   * The raw `Client::connection` watch (`ConnectionState`) — the
//     unmerged link lifecycle. Surfaced as `link_state` so the new
//     `CoreStatusPill` can distinguish "first-ever connect attempt"
//     from "lost a live link, retrying" without watching the catalog
//     resource separately.
//
// `connection_state` constants (legacy banner view):
//   0 DISCONNECTED — resource Idle (link not attempted yet)
//   1 CONNECTING   — resource Loading (handshake or RPC in flight)
//   2 READY        — resource Ready (catalog loaded)
//   3 ERROR        — resource Errored (link unreachable or RPC failed)
//
// `link_state` constants (raw `ConnectionState`):
//   0 DISCONNECTED — initial value before the connect loop's first attempt
//   1 CONNECTING   — first-ever `connect_async` in flight
//   2 CONNECTED    — ws link up
//   3 RECONNECTING — lost a previously-live link, retrying
//   4 UNREACHABLE  — `RETRY_ERROR_THRESHOLD` consecutive failures hit

use cxx_qt::{Initialize, Threading};
use cxx_qt_lib::QString;
use std::pin::Pin;
use zaparoo_core::client::ConnectionState;
use zaparoo_core::endpoints::catalog::CatalogEndpoint;
use zaparoo_core::remote_resource::ResourceStatus;
use zaparoo_core::systems_catalog::CatalogData;

pub const DISCONNECTED: i32 = 0;
pub const CONNECTING: i32 = 1;
pub const READY: i32 = 2;
pub const ERROR: i32 = 3;

pub const LINK_DISCONNECTED: i32 = 0;
pub const LINK_CONNECTING: i32 = 1;
pub const LINK_CONNECTED: i32 = 2;
pub const LINK_RECONNECTING: i32 = 3;
pub const LINK_UNREACHABLE: i32 = 4;

/// Minimum Zaparoo Core version this frontend supports. Older Core builds
/// still connect, but the startup warning in `Main.qml` nudges the user to
/// update because some features may not work. Bump this in lockstep with
/// the features the frontend relies on.
pub const MIN_CORE_VERSION: &str = "2.15.0";

pub struct AppStatusRust {
    connection_state: i32,
    last_error: QString,
    link_state: i32,
    /// Core version string reported by `version` (e.g. "2.15.0"). Empty
    /// until the first fetch resolves.
    core_version: QString,
    /// True when the connected Core is at or above `MIN_CORE_VERSION` (or
    /// is a dev build). Defaults true so the warning never flashes before
    /// we've actually checked.
    core_version_supported: bool,
    /// Sticky-true once a `version` fetch has resolved. The startup warning
    /// gate fires on this edge so it doesn't run before Core has answered.
    core_version_checked: bool,
    /// `MIN_CORE_VERSION` as a `QString`, exposed so the warning message has
    /// a single source of truth for the required version.
    min_core_version: QString,
    direct_meta: bool,
}

impl Default for AppStatusRust {
    fn default() -> Self {
        Self {
            connection_state: DISCONNECTED,
            last_error: QString::default(),
            link_state: LINK_DISCONNECTED,
            core_version: QString::default(),
            core_version_supported: true,
            core_version_checked: false,
            min_core_version: QString::from(MIN_CORE_VERSION),
            direct_meta: false,
        }
    }
}

/// Decide whether a Core version string meets `MIN_CORE_VERSION`.
///
/// Dev builds are always treated as supported: a version ending in `-dev`
/// or the literal `DEVELOPMENT` (what Core reports for unversioned local
/// builds). Anything that fails to parse as semver also returns true —
/// fail open rather than nag on an unexpected version shape.
fn version_supported(raw: &str) -> bool {
    let v = raw.trim();
    if v == "DEVELOPMENT" || v.ends_with("-dev") {
        return true;
    }
    match (
        semver::Version::parse(v),
        semver::Version::parse(MIN_CORE_VERSION),
    ) {
        (Ok(current), Ok(min)) => current >= min,
        _ => true,
    }
}

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("model_includes.h");

        type QString = cxx_qt_lib::QString;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(i32, connection_state)]
        #[qproperty(QString, last_error)]
        #[qproperty(i32, link_state)]
        #[qproperty(QString, core_version)]
        #[qproperty(bool, core_version_supported)]
        #[qproperty(bool, core_version_checked)]
        #[qproperty(QString, min_core_version)]
        /// True when the direct-database metadata layer initialized;
        /// QML shortens the detail settle debounce accordingly.
        #[qproperty(bool, direct_meta)]
        type AppStatus = super::AppStatusRust;
    }

    impl cxx_qt::Threading for AppStatus {}
    impl cxx_qt::Initialize for AppStatus {}
}

impl Initialize for ffi::AppStatus {
    fn initialize(mut self: Pin<&mut Self>) {
        let started = std::time::Instant::now();
        crate::startup_trace("rust:model AppStatus init start");
        // Seeded once: the layer's availability is decided at first
        // touch and stable for the process lifetime.
        let direct_meta = crate::media_meta_db::enabled();
        self.as_mut().set_direct_meta(direct_meta);
        bind_catalog_status(self.as_mut());
        bind_link_state(self.as_mut());
        bind_core_version(self);
        crate::startup_trace(format!(
            "rust:model AppStatus init end dur_ms={}",
            started.elapsed().as_millis()
        ));
    }
}

/// Subscribe to the catalog resource status and drive the legacy
/// `connection_state` / `last_error` properties. Same shape as the
/// previous `bind_to_endpoint!` invocation; hand-rolled so it can sit
/// alongside the second binding below.
fn bind_catalog_status(mut model: Pin<&mut ffi::AppStatus>) {
    let mut rx = crate::models::global_store()
        .subscribe::<CatalogEndpoint>(())
        .subscribe();
    let projected = project_catalog(&rx.borrow_and_update());
    apply_catalog_state(model.as_mut(), projected);

    let qt_thread = model.qt_thread();
    crate::models::global_handle().spawn(async move {
        while rx.changed().await.is_ok() {
            let projected = project_catalog(&rx.borrow_and_update());
            let _ = qt_thread.queue(move |m| apply_catalog_state(m, projected));
        }
    });
}

/// Subscribe to the raw `Client::connection` watch and drive the new
/// `link_state` property. The seed reads the current value through
/// `borrow_and_update` so the first QML frame reflects whatever
/// transition Core has already produced — usually `Connecting` by the
/// time the QML engine boots.
fn bind_link_state(mut model: Pin<&mut ffi::AppStatus>) {
    let client = crate::models::global_store().client();
    let mut rx = client.connection.subscribe();
    let projected = project_link(&rx.borrow_and_update());
    apply_link_state(model.as_mut(), projected);

    let qt_thread = model.qt_thread();
    crate::models::global_handle().spawn(async move {
        while rx.changed().await.is_ok() {
            let projected = project_link(&rx.borrow_and_update());
            let _ = qt_thread.queue(move |m| apply_link_state(m, projected));
        }
    });
}

/// Fetch Core's version on every successful connect and drive the
/// `core_version` / `core_version_supported` / `core_version_checked`
/// properties. Subscribing to the raw `Client::connection` watch (rather
/// than firing once at startup) means a reconnect to a different Core
/// re-evaluates the version — the warning follows whatever Core we're
/// actually talking to.
fn bind_core_version(mut model: Pin<&mut ffi::AppStatus>) {
    // Seed the minimum so the QML message has the required version even
    // before the first fetch lands.
    let min = QString::from(MIN_CORE_VERSION);
    if model.min_core_version != min {
        model.as_mut().set_min_core_version(min);
    }

    let client = crate::models::global_store().client();
    let mut rx = client.connection.subscribe();
    let qt_thread = model.qt_thread();
    crate::models::global_handle().spawn(async move {
        loop {
            // Bind the read in its own scope so the watch guard is dropped
            // before the await below.
            let connected = { matches!(&*rx.borrow_and_update(), ConnectionState::Connected) };
            if connected {
                // Flip `core_version_checked` whether or not the fetch
                // succeeds: the first-run modal chain in Main.qml waits on
                // it, so a failed `version` RPC must still resolve the gate.
                // On error we fail open (supported = true, empty version).
                let (version, supported) = match client.version().await {
                    Ok(result) => {
                        let supported = version_supported(&result.version);
                        (result.version, supported)
                    }
                    Err(_) => (String::new(), true),
                };
                let _ = qt_thread.queue(move |m| apply_core_version(m, &version, supported));
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Map `ResourceStatus<CatalogData>` onto the four banner states the QML
/// side knows about. The error message is whatever the resource layer
/// surfaced — link error (`Unreachable`) or RPC error (`Errored` while
/// the link is still up). The UI treats them the same.
fn project_catalog(status: &ResourceStatus<CatalogData>) -> (i32, String) {
    match status {
        ResourceStatus::Idle => (DISCONNECTED, String::new()),
        ResourceStatus::Loading => (CONNECTING, String::new()),
        ResourceStatus::Ready(_) => (READY, String::new()),
        ResourceStatus::Errored { message, .. } => (ERROR, message.clone()),
    }
}

/// Map a raw `ConnectionState` onto the five-state `link_state`. The
/// `Unreachable(msg)` payload is intentionally not surfaced here —
/// `last_error` already carries the error string from the catalog
/// projection, and exposing the same message twice would let the two
/// drift.
fn project_link(state: &ConnectionState) -> i32 {
    match state {
        ConnectionState::Disconnected => LINK_DISCONNECTED,
        ConnectionState::Connecting => LINK_CONNECTING,
        ConnectionState::Connected => LINK_CONNECTED,
        ConnectionState::Reconnecting => LINK_RECONNECTING,
        ConnectionState::Unreachable(_) => LINK_UNREACHABLE,
    }
}

/// Apply a freshly-derived `(state, err)` to the model, suppressing
/// `QProperty` setters whose value hasn't changed so QML doesn't see
/// spurious `Changed` signals on every reconnect.
fn apply_catalog_state(mut model: Pin<&mut ffi::AppStatus>, (state, err): (i32, String)) {
    if model.connection_state != state {
        model.as_mut().set_connection_state(state);
    }
    let qerr = QString::from(err.as_str());
    if model.last_error != qerr {
        model.as_mut().set_last_error(qerr);
    }
}

fn apply_link_state(mut model: Pin<&mut ffi::AppStatus>, state: i32) {
    if model.link_state != state {
        model.as_mut().set_link_state(state);
    }
}

fn apply_core_version(mut model: Pin<&mut ffi::AppStatus>, version: &str, supported: bool) {
    let qversion = QString::from(version);
    if model.core_version != qversion {
        model.as_mut().set_core_version(qversion);
    }
    if model.core_version_supported != supported {
        model.as_mut().set_core_version_supported(supported);
    }
    if !model.core_version_checked {
        model.as_mut().set_core_version_checked(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_catalog() -> CatalogData {
        CatalogData {
            systems: Vec::new(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn idle_maps_to_disconnected() {
        let (state, err) = project_catalog(&ResourceStatus::Idle);
        assert_eq!(state, DISCONNECTED);
        assert_eq!(err, "");
    }

    #[test]
    fn loading_maps_to_connecting() {
        let (state, err) = project_catalog(&ResourceStatus::Loading);
        assert_eq!(state, CONNECTING);
        assert_eq!(err, "");
    }

    #[test]
    fn ready_maps_to_ready_with_no_error() {
        let (state, err) = project_catalog(&ResourceStatus::Ready(empty_catalog()));
        assert_eq!(state, READY);
        assert_eq!(err, "");
    }

    #[test]
    fn errored_with_retrying_surfaces_message() {
        let (state, err) = project_catalog(&ResourceStatus::Errored {
            message: "rpc kaboom".into(),
            retrying: true,
        });
        assert_eq!(state, ERROR);
        assert_eq!(err, "rpc kaboom");
    }

    #[test]
    fn errored_without_retrying_surfaces_message() {
        let (state, err) = project_catalog(&ResourceStatus::Errored {
            message: "connection refused".into(),
            retrying: false,
        });
        assert_eq!(state, ERROR);
        assert_eq!(err, "connection refused");
    }

    #[test]
    fn version_at_or_above_min_is_supported() {
        assert!(version_supported("2.15.0"));
        assert!(version_supported("2.15.1"));
        assert!(version_supported("3.0.0"));
        assert!(version_supported(" 2.16.0 ")); // surrounding whitespace
    }

    #[test]
    fn version_below_min_is_unsupported() {
        assert!(!version_supported("2.14.9"));
        assert!(!version_supported("2.0.0"));
        assert!(!version_supported("1.9.9"));
    }

    #[test]
    fn dev_builds_are_always_supported() {
        assert!(version_supported("DEVELOPMENT"));
        assert!(version_supported("2.14.0-dev"));
        // A -dev suffix below the minimum is still exempt.
        assert!(version_supported("1.0.0-dev"));
    }

    #[test]
    fn unparseable_version_fails_open() {
        assert!(version_supported(""));
        assert!(version_supported("garbage"));
        assert!(version_supported("v2.15"));
    }

    #[test]
    fn link_state_covers_every_connection_variant() {
        assert_eq!(
            project_link(&ConnectionState::Disconnected),
            LINK_DISCONNECTED
        );
        assert_eq!(project_link(&ConnectionState::Connecting), LINK_CONNECTING);
        assert_eq!(project_link(&ConnectionState::Connected), LINK_CONNECTED);
        assert_eq!(
            project_link(&ConnectionState::Reconnecting),
            LINK_RECONNECTING
        );
        assert_eq!(
            project_link(&ConnectionState::Unreachable("boom".into())),
            LINK_UNREACHABLE
        );
    }
}
