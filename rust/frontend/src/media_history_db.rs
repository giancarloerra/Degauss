// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Read-only play-history listing against Core's user database. Recents
// over the RPC pages `media.history` twenty-five raw events at a time
// and dedupes client-side; the same events live in `user.db`'s
// `MediaHistory` table (indexed by `StartTime`), so one local window
// query returns the complete deduplicated list in milliseconds — the
// same read-only fast-path architecture as the media-database modules,
// applied to Core's second database file under the same contract.
//
// `media_id` is resolved best-effort by attaching the media database
// read-only and matching path plus system, exactly the resolution Core
// performs when serving `media.history`; an entry that does not
// resolve keeps `None`, which the cover pipeline already handles by
// resolving through `(system, path)`.
//
// Timestamps: `StartTime`/`EndTime` are unix seconds in the table and
// RFC3339 strings on the wire (the Resume feature parses them), so
// this module formats them the same way. A still-open session has
// `EndTime` NULL and maps to `ended_at: None`, which Resume relies on.
//
// `ZAPAROO_FRONTEND_DB_HISTORY=0` disables the layer at runtime for
// A/B comparison.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tracing::{debug, info, warn};

use zaparoo_core::media_types::MediaHistoryEntry;

/// Core's user database on the card, next to the media database.
const USER_DB_PATH: &str = "/media/fat/zaparoo/user.db";

/// Tables this module queries in `user.db`. Verified at open so a Core
/// schema change turns the layer off instead of producing wrong
/// history.
const REQUIRED_TABLES: &[&str] = &["MediaHistory"];

struct HistoryDb {
    conn: Mutex<Option<Connection>>,
    db_path: PathBuf,
}

static HISTORY_DB: OnceLock<Option<HistoryDb>> = OnceLock::new();

fn history_db() -> Option<&'static HistoryDb> {
    HISTORY_DB
        .get_or_init(|| {
            let disabled = std::env::var("ZAPAROO_FRONTEND_DB_HISTORY")
                .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("off"));
            if disabled {
                info!("media_history_db: disabled via ZAPAROO_FRONTEND_DB_HISTORY");
                return None;
            }
            let db_path = PathBuf::from(USER_DB_PATH);
            if !db_path.is_file() {
                return None;
            }
            match open_checked(&db_path) {
                Ok(conn) => {
                    info!(db = %db_path.display(), "media_history_db: direct history listing active");
                    Some(HistoryDb {
                        conn: Mutex::new(Some(conn)),
                        db_path,
                    })
                }
                Err(e) => {
                    warn!("media_history_db: unavailable ({e}), history uses RPC only");
                    None
                }
            }
        })
        .as_ref()
}

/// Whether the direct history layer initialized.
pub fn enabled() -> bool {
    history_db().is_some()
}

fn open_checked(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_millis(200))
        .map_err(|e| e.to_string())?;
    let placeholders = vec!["?"; REQUIRED_TABLES.len()].join(",");
    let sql = format!(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ({placeholders})"
    );
    let n: i64 = conn
        .query_row(
            &sql,
            rusqlite::params_from_iter(REQUIRED_TABLES.iter()),
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let expected = i64::try_from(REQUIRED_TABLES.len()).unwrap_or(i64::MAX);
    if n != expected {
        return Err(format!(
            "expected {expected} known tables, found {n} — Core schema changed, refusing to guess"
        ));
    }
    // Best-effort media-id resolution: attach the media database
    // read-only. Attachment failure is not fatal — entries then carry
    // `media_id: None` and the cover pipeline resolves by path.
    let media_uri = format!(
        "file:{}?mode=ro",
        crate::media_art_db::MEDIA_DB_PATH.replace('?', "%3F")
    );
    if let Err(e) = conn.execute("ATTACH DATABASE ?1 AS mediadb", [&media_uri]) {
        debug!("media_history_db: media db attach failed ({e}); media ids unresolved");
    }
    Ok(conn)
}

fn media_attached(conn: &Connection) -> bool {
    let Ok(mut stmt) = conn.prepare("PRAGMA database_list") else {
        return false;
    };
    let names = stmt.query_map([], |r| r.get::<_, String>(1));
    match names {
        Ok(rows) => rows.flatten().any(|n| n == "mediadb"),
        Err(_) => false,
    }
}

/// The complete deduplicated play history, newest first, in one query.
/// Returns `None` on any error (caller falls back to the RPC) and an
/// empty list when there is genuinely no history.
pub fn history(cancelled: &(dyn Fn() -> bool + Sync)) -> Option<Vec<MediaHistoryEntry>> {
    let db = history_db()?;
    if cancelled() {
        return None;
    }
    let started = Instant::now();
    #[allow(clippy::unwrap_used, reason = "Mutex poisoning is unrecoverable")]
    let mut guard = db.conn.lock().unwrap();
    if guard.is_none() {
        match open_checked(&db.db_path) {
            Ok(conn) => *guard = Some(conn),
            Err(e) => {
                debug!("media_history_db: reopen failed ({e})");
                return None;
            }
        }
    }
    let conn = guard.as_ref()?;
    match list_history(conn, media_attached(conn), cancelled) {
        Ok(result) => {
            if cancelled() {
                return None;
            }
            if let Some(items) = &result {
                debug!(
                    entries = items.len(),
                    lookup_ms = started.elapsed().as_millis(),
                    "media_history_db: history listed",
                );
            }
            result
        }
        Err(e) => {
            warn!(
                "media_history_db: query failed ({e}), dropping connection; history falls back to RPC"
            );
            *guard = None;
            None
        }
    }
}

fn list_history(
    conn: &Connection,
    resolve_media: bool,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> rusqlite::Result<Option<Vec<MediaHistoryEntry>>> {
    // Latest event per media path, mirroring the client-side
    // `dedupe_latest_by_path` the RPC pages go through; the DBID
    // tiebreak keeps same-second replays deterministic. The media id
    // resolves through a correlated scalar subquery rather than joins:
    // a path indexed under two systems (arcade classification twins)
    // would otherwise duplicate the history row and break the
    // deduplicated contract this function promises.
    let media_id_col = if resolve_media {
        "(SELECT m.DBID FROM mediadb.Media m \
          INNER JOIN mediadb.Systems s ON s.DBID = m.SystemDBID \
          WHERE m.Path = h.MediaPath AND s.SystemID = h.SystemID LIMIT 1)"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT h.SystemID, h.SystemName, h.MediaName, h.MediaPath, h.LauncherID, \
                h.StartTime, h.EndTime, h.PlayTime, {media_id_col} \
         FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY MediaPath \
               ORDER BY StartTime DESC, DBID DESC) AS rn \
               FROM MediaHistory WHERE IsDeleted = 0) h \
         WHERE h.rn = 1 \
         ORDER BY h.StartTime DESC, h.DBID DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let system_id: String = r.get(0)?;
        let system_name: String = r.get(1)?;
        let media_name: String = r.get(2)?;
        let media_path: String = r.get(3)?;
        let launcher_id: String = r.get(4)?;
        let start_time: i64 = r.get(5)?;
        let end_time: Option<i64> = r.get(6)?;
        let play_time: i64 = r.get(7)?;
        let media_id: Option<i64> = r.get(8)?;
        Ok((
            system_id,
            system_name,
            media_name,
            media_path,
            launcher_id,
            start_time,
            end_time,
            play_time,
            media_id,
        ))
    })?;
    let mut out: Vec<MediaHistoryEntry> = Vec::new();
    for row in rows {
        if out.len().is_multiple_of(256) && cancelled() {
            return Ok(None);
        }
        let (
            system_id,
            system_name,
            media_name,
            media_path,
            launcher_id,
            start_time,
            end_time,
            play_time,
            media_id,
        ) = row?;
        out.push(MediaHistoryEntry {
            media_id,
            system_id,
            system_name,
            media_name,
            media_path,
            launcher_id,
            started_at: rfc3339(start_time),
            ended_at: end_time.map(rfc3339),
            play_time: u64::try_from(play_time.max(0)).unwrap_or(0),
        });
    }
    Ok(Some(out))
}

/// Unix seconds to the RFC3339 form the wire uses. An out-of-range
/// value (garbage row) formats as the epoch rather than failing the
/// whole listing.
fn rfc3339(unix_seconds: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix_seconds)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::{list_history, rfc3339};
    use rusqlite::Connection;

    fn test_db() -> Connection {
        #[allow(clippy::expect_used, reason = "test setup")]
        let conn = Connection::open_in_memory().expect("open in-memory db");
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch(
            "CREATE TABLE MediaHistory (
                 DBID INTEGER PRIMARY KEY,
                 StartTime INTEGER NOT NULL,
                 EndTime INTEGER,
                 SystemID TEXT NOT NULL,
                 SystemName TEXT NOT NULL,
                 MediaPath TEXT NOT NULL,
                 MediaName TEXT NOT NULL,
                 LauncherID TEXT NOT NULL,
                 PlayTime INTEGER DEFAULT 0,
                 IsDeleted INTEGER DEFAULT 0
             );
             INSERT INTO MediaHistory
                 (DBID, StartTime, EndTime, SystemID, SystemName, MediaPath, MediaName, LauncherID, PlayTime, IsDeleted)
             VALUES
                 (1, 1000, 1600, 'C64', 'Commodore 64', '/g/C64/A.crt', 'A', 'l', 600, 0),
                 (2, 2000, 2100, 'C64', 'Commodore 64', '/g/C64/A.crt', 'A', 'l', 100, 0),
                 (3, 1500, NULL, 'NES', 'NES', '/g/NES/B.nes', 'B', 'l', 0, 0),
                 (4, 3000, 3100, 'C64', 'Commodore 64', '/g/C64/Del.crt', 'Del', 'l', 10, 1);",
        )
        .expect("seed schema");
        conn
    }

    #[test]
    fn dedupes_latest_per_path_newest_first_and_skips_deleted() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test assertion")]
        let items = list_history(&conn, false, &|| false)
            .expect("query ok")
            .expect("some");
        let names: Vec<&str> = items.iter().map(|i| i.media_name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        // The kept "A" row is the later session, whose end is known.
        assert_eq!(items[0].started_at, rfc3339(2000));
        assert_eq!(items[0].ended_at.as_deref(), Some(rfc3339(2100).as_str()));
        // The open session keeps ended_at None, which Resume relies on.
        assert!(items[1].ended_at.is_none());
        assert_eq!(items[0].play_time, 100);
    }

    #[test]
    fn media_ids_resolve_via_attached_media_db_with_system_match() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch(
            "ATTACH DATABASE ':memory:' AS mediadb;
             CREATE TABLE mediadb.Systems (DBID INTEGER PRIMARY KEY, SystemID TEXT NOT NULL);
             CREATE TABLE mediadb.Media (DBID INTEGER PRIMARY KEY, SystemDBID INTEGER NOT NULL, Path TEXT NOT NULL);
             INSERT INTO mediadb.Systems VALUES (1,'C64'),(2,'NES');
             INSERT INTO mediadb.Media VALUES (500,1,'/g/C64/A.crt'),(501,2,'/g/NES/B.nes');",
        )
        .expect("attach media");
        #[allow(clippy::expect_used, reason = "test assertion")]
        let items = list_history(&conn, true, &|| false)
            .expect("query ok")
            .expect("some");
        assert_eq!(items[0].media_id, Some(500));
        assert_eq!(items[1].media_id, Some(501));
    }

    #[test]
    fn twin_path_media_rows_do_not_duplicate_history() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch(
            "ATTACH DATABASE ':memory:' AS mediadb;
             CREATE TABLE mediadb.Systems (DBID INTEGER PRIMARY KEY, SystemID TEXT NOT NULL);
             CREATE TABLE mediadb.Media (DBID INTEGER PRIMARY KEY, SystemDBID INTEGER NOT NULL, Path TEXT NOT NULL);
             INSERT INTO mediadb.Systems VALUES (1,'C64'),(2,'C64Alt');
             -- The same file indexed under two systems: one media row per
             -- system, identical path. History references the C64 one.
             INSERT INTO mediadb.Media VALUES (500,1,'/g/C64/A.crt'),(600,2,'/g/C64/A.crt');",
        )
        .expect("attach media");
        #[allow(clippy::expect_used, reason = "test assertion")]
        let items = list_history(&conn, true, &|| false)
            .expect("query ok")
            .expect("some");
        let a_rows: Vec<_> = items
            .iter()
            .filter(|i| i.media_path == "/g/C64/A.crt")
            .collect();
        assert_eq!(
            a_rows.len(),
            1,
            "twin media rows must not duplicate history entries"
        );
        assert_eq!(
            a_rows[0].media_id,
            Some(500),
            "the id must match the history row's own system"
        );
    }

    #[test]
    fn unresolved_paths_keep_media_id_none() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch(
            "ATTACH DATABASE ':memory:' AS mediadb;
             CREATE TABLE mediadb.Systems (DBID INTEGER PRIMARY KEY, SystemID TEXT NOT NULL);
             CREATE TABLE mediadb.Media (DBID INTEGER PRIMARY KEY, SystemDBID INTEGER NOT NULL, Path TEXT NOT NULL);",
        )
        .expect("attach media");
        #[allow(clippy::expect_used, reason = "test assertion")]
        let items = list_history(&conn, true, &|| false)
            .expect("query ok")
            .expect("some");
        assert!(items.iter().all(|i| i.media_id.is_none()));
    }

    #[test]
    #[allow(clippy::expect_used, reason = "test assertion")]
    fn cancelled_read_returns_none() {
        let conn = test_db();
        assert!(list_history(&conn, false, &|| true)
            .expect("query ok")
            .is_none());
    }
}
