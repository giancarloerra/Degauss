// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Shared machinery for the direct read layer. Five domains read Core's
// databases in place — artwork, folder listings, favorites, play
// history, and detail metadata — and every one of them carries the
// same contract:
//
// - Strictly read-only: `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`,
//   a short busy timeout, never a write, checkpoint, or pragma tune, so
//   WAL readers cannot block Core's writer.
// - A schema guard at open: every table the domain queries is verified
//   in `sqlite_master`, and any mismatch disables that domain loudly,
//   leaving the RPC as its sole source. Core changing its schema turns
//   a domain off; it never produces wrong answers.
// - A per-domain kill switch (`ZAPAROO_FRONTEND_DB_<DOMAIN>=0`) for
//   A/B measurement and emergency disable.
// - Reopen-on-error: a failed query drops the cached connection and the
//   next call opens fresh, which covers Core swapping the database file
//   underneath us during recovery or reindex.
// - RPC fallback: every public read returns `Option`, and `None` always
//   means "the caller uses the RPC exactly as before this layer
//   existed" — never "empty result".
//
// This module owns that contract once; the domain modules own only
// their queries. To remove the whole layer, delete this module and the
// five `media_*_db` modules, and every consumer falls back to the RPC.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};
use tracing::{debug, info, warn};

/// Core's media database on the card. All domains except history read
/// this file; history reads `user.db` and attaches this one for media
/// id resolution.
pub(crate) const MEDIA_DB_PATH: &str = "/media/fat/zaparoo/media.db";

/// Everything static about one read domain. Held as a `&'static` by the
/// domain module and threaded through open/reopen so the guard, the
/// switch, and the log lines all speak with one voice.
pub struct ReadDbSpec {
    /// Log prefix, e.g. `"media_art_db"`.
    pub domain: &'static str,
    /// Kill-switch environment variable, e.g. `ZAPAROO_FRONTEND_DB_ART`.
    /// `0` or `off` disables the domain for the process lifetime.
    pub env_switch: &'static str,
    /// Database file this domain opens.
    pub db_path: &'static str,
    /// Tables the domain's queries touch, verified at every open.
    pub required_tables: &'static [&'static str],
    /// Whether to open with `SQLITE_OPEN_URI` (history's attach path).
    pub uri: bool,
    /// Post-open hook, run after the schema guard passes. Used by
    /// history to attach the media database; must be best-effort and
    /// log its own failures.
    pub prepare: Option<fn(&Connection)>,
    /// Log line fragment describing the active fast path.
    pub active_msg: &'static str,
    /// Log line fragment describing what the RPC keeps serving when the
    /// domain is unavailable.
    pub fallback_msg: &'static str,
}

/// One domain's connection slot. Lives in a `OnceLock<Option<ReadDb>>`
/// in the domain module: `None` in the slot means the domain never
/// initialized (switched off, file missing, guard failed) and stays off
/// for the process lifetime; `None` in `conn` means the last query
/// errored and the next call reopens.
pub struct ReadDb {
    conn: Mutex<Option<Connection>>,
    db_path: PathBuf,
    spec: &'static ReadDbSpec,
}

impl ReadDb {
    /// Initialize a domain: kill switch, file existence, checked open.
    /// Returns `None` (domain off, RPC serves) on any refusal, logging
    /// the reason at the appropriate level.
    pub fn init(spec: &'static ReadDbSpec) -> Option<ReadDb> {
        let disabled = std::env::var(spec.env_switch)
            .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("off"));
        if disabled {
            info!("{}: disabled via {}", spec.domain, spec.env_switch);
            return None;
        }
        let db_path = PathBuf::from(spec.db_path);
        if !db_path.is_file() {
            return None;
        }
        match open_checked(&db_path, spec) {
            Ok(conn) => {
                info!(db = %db_path.display(), "{}: {}", spec.domain, spec.active_msg);
                Some(ReadDb {
                    conn: Mutex::new(Some(conn)),
                    db_path,
                    spec,
                })
            }
            Err(e) => {
                warn!("{}: unavailable ({e}), {}", spec.domain, spec.fallback_msg);
                None
            }
        }
    }

    /// Run one query against the cached connection, reopening it first
    /// if a previous call dropped it. `Err` from `f` drops the
    /// connection (next call reopens fresh) and yields `None`, so the
    /// caller's RPC fallback engages; `ctx` is only rendered on that
    /// error path to keep the happy path allocation-free.
    pub fn with_conn<T>(
        &self,
        ctx: impl FnOnce() -> String,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        #[allow(clippy::unwrap_used, reason = "mutex poisoning is unrecoverable")]
        let mut guard = self.conn.lock().unwrap();
        if guard.is_none() {
            match open_checked(&self.db_path, self.spec) {
                Ok(conn) => *guard = Some(conn),
                Err(e) => {
                    debug!("{}: reopen failed ({e})", self.spec.domain);
                    return None;
                }
            }
        }
        #[allow(clippy::unwrap_used, reason = "populated just above")]
        let conn = guard.as_ref().unwrap();
        match f(conn) {
            Ok(v) => Some(v),
            Err(e) => {
                debug!(
                    "{}: query error ({e}) [{}], dropping connection",
                    self.spec.domain,
                    ctx()
                );
                *guard = None;
                None
            }
        }
    }
}

/// Open read-only and verify the tables the domain queries actually
/// exist, so a future Core schema change turns the layer off instead of
/// producing wrong answers.
pub(crate) fn open_checked(db_path: &Path, spec: &ReadDbSpec) -> Result<Connection, String> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if spec.uri {
        flags |= OpenFlags::SQLITE_OPEN_URI;
    }
    let conn = Connection::open_with_flags(db_path, flags).map_err(|e| {
        // A WAL-mode database opened read-only needs the -shm/-wal
        // sidecars to be accessible; SQLITE_READONLY_CANTINIT is the
        // specific "cannot initialize WAL under these permissions"
        // failure, worth naming so a permissions problem on the card
        // is diagnosable from the log rather than a generic open error.
        if let rusqlite::Error::SqliteFailure(ffi_err, _) = &e {
            if ffi_err.extended_code == rusqlite::ffi::SQLITE_READONLY_CANTINIT {
                return format!(
                    "WAL sidecar files are not accessible read-only \
                     (SQLITE_READONLY_CANTINIT) — check permissions on \
                     the database directory: {e}"
                );
            }
        }
        e.to_string()
    })?;
    conn.busy_timeout(std::time::Duration::from_millis(200))
        .map_err(|e| e.to_string())?;
    verify_tables(&conn, spec.required_tables)?;
    if let Some(prepare) = spec.prepare {
        prepare(&conn);
    }
    Ok(conn)
}

/// The schema guard shared by every domain: count the required tables
/// in `sqlite_master` and refuse anything but an exact match.
fn verify_tables(conn: &Connection, required: &[&str]) -> Result<(), String> {
    let placeholders = vec!["?"; required.len()].join(",");
    let sql = format!(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ({placeholders})"
    );
    let n: i64 = conn
        .query_row(&sql, rusqlite::params_from_iter(required.iter()), |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    let expected = i64::try_from(required.len()).unwrap_or(i64::MAX);
    if n != expected {
        return Err(format!(
            "expected {expected} known tables, found {n} — Core schema changed, refusing to guess"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        reason = "tests should fail-fast on unexpected errors"
    )]

    use super::*;

    const TEST_SPEC: ReadDbSpec = ReadDbSpec {
        domain: "test_db",
        env_switch: "ZAPAROO_FRONTEND_DB_TEST_NEVER_SET",
        db_path: "/nonexistent/never-used.db",
        required_tables: &["Media"],
        uri: false,
        prepare: None,
        active_msg: "active",
        fallback_msg: "rpc only",
    };

    #[test]
    fn open_checked_accepts_matching_schema() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ok.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch("CREATE TABLE Media (DBID INTEGER PRIMARY KEY);")
            .unwrap();
        assert!(open_checked(&p, &TEST_SPEC).is_ok());
    }

    #[test]
    fn open_checked_refuses_foreign_schema() {
        // A real file whose schema lacks the required tables must be
        // refused at open — the contract that turns a domain off across
        // Core schema changes instead of letting it guess.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("other.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (x);")
            .unwrap();
        let err = open_checked(&p, &TEST_SPEC).expect_err("must refuse");
        assert!(err.contains("refusing to guess"), "got: {err}");
    }
}
