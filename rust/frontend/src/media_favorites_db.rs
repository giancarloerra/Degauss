// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Read-only favorites listing against Core's media database. The
// favorites screen needs the complete favorite set for its sort and
// scope filter, which over the RPC means a chained sequence of
// `media.search` requests, one Core round trip per thousand rows. The
// same rows live in plain indexed tables (`MediaTags` against the
// `user:favorite` tag joined to `Media`), so one local query returns
// the whole set in milliseconds — the same read-only fast-path
// architecture as `media_art_db` and `media_browse_db`, extended from
// artwork and listings to favorites.
//
// The store's endpoint subscription remains the freshness signal: a
// favorite toggle still invalidates `media.favorites`, the refetched
// first page still applies, and this layer then upgrades the model to
// the full local set. Writes never come near this module.
//
// Known divergences from the RPC set, accepted for this path:
// - `zapScript` is not populated (it is not stored in the database);
//   launching uses the media path, which is the primary launch text
//   anyway. Portable card-write text falls back to the path too.
// - No `disambiguatingTags` variant badges, matching the listings
//   module's divergence.
// - Entries order by `SortName ASC, DBID ASC` rather than Core's
//   search order; the view's own sort modes are unaffected.
//
// `ZAPAROO_FRONTEND_DB_FAVORITES=0` disables the layer at runtime for
// A/B comparison.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};
use tracing::{debug, info, warn};

use zaparoo_core::media_types::{MediaItem, System, TagInfo};

/// Tables this module queries. Verified at open so a Core schema change
/// turns the layer off instead of producing wrong favorites.
const REQUIRED_TABLES: &[&str] = &["Media", "Systems", "MediaTags", "Tags", "TagTypes"];

struct FavoritesDb {
    conn: Mutex<Option<Connection>>,
    db_path: PathBuf,
}

static FAVORITES_DB: OnceLock<Option<FavoritesDb>> = OnceLock::new();

fn favorites_db() -> Option<&'static FavoritesDb> {
    FAVORITES_DB
        .get_or_init(|| {
            let disabled = std::env::var("ZAPAROO_FRONTEND_DB_FAVORITES")
                .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("off"));
            if disabled {
                info!("media_favorites_db: disabled via ZAPAROO_FRONTEND_DB_FAVORITES");
                return None;
            }
            let db_path = PathBuf::from(crate::media_art_db::MEDIA_DB_PATH);
            if !db_path.is_file() {
                return None;
            }
            match open_checked(&db_path) {
                Ok(conn) => {
                    info!(db = %db_path.display(), "media_favorites_db: direct favorites listing active");
                    Some(FavoritesDb {
                        conn: Mutex::new(Some(conn)),
                        db_path,
                    })
                }
                Err(e) => {
                    warn!("media_favorites_db: unavailable ({e}), favorites use RPC only");
                    None
                }
            }
        })
        .as_ref()
}

/// Whether the direct favorites layer initialized. Says the database is
/// present, readable, and schema-compatible; `favorites` can still
/// return `None` per call on query errors.
pub fn enabled() -> bool {
    favorites_db().is_some()
}

fn open_checked(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
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
    Ok(conn)
}

/// The complete favorite set, in one query. Returns `None` whenever the
/// answer cannot be produced with full confidence (layer disabled,
/// query error) and the caller falls through to the RPC full load; an
/// empty list is a real answer, not a fallback case.
///
/// `cancelled` is polled at each stage and periodically inside the row
/// loop so a superseded load stops consuming its blocking worker.
pub fn favorites(cancelled: &(dyn Fn() -> bool + Sync)) -> Option<Vec<MediaItem>> {
    let db = favorites_db()?;
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
                debug!("media_favorites_db: reopen failed ({e})");
                return None;
            }
        }
    }
    let conn = guard.as_ref()?;
    match list_favorites(conn, cancelled) {
        Ok(result) => {
            if cancelled() {
                return None;
            }
            if let Some(items) = &result {
                debug!(
                    entries = items.len(),
                    lookup_ms = started.elapsed().as_millis(),
                    "media_favorites_db: favorites listed",
                );
            }
            result
        }
        Err(e) => {
            warn!(
                "media_favorites_db: query failed ({e}), dropping connection; favorites fall back to RPC"
            );
            *guard = None;
            None
        }
    }
}

fn list_favorites(
    conn: &Connection,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> rusqlite::Result<Option<Vec<MediaItem>>> {
    // Exact-match `user:favorite`, the same resolution Core performs;
    // the tag is stored unpadded (PadTagValue only pads all-digit
    // values). No tag rows means no favorites exist yet — an empty
    // list, not a fallback.
    let mut stmt = conn.prepare(
        "SELECT t.DBID FROM Tags t JOIN TagTypes tt ON tt.DBID = t.TypeDBID \
         WHERE tt.Type = 'user' AND t.Tag = 'favorite' ORDER BY t.DBID",
    )?;
    let fav_ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if fav_ids.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if cancelled() {
        return Ok(None);
    }
    let placeholders = (1..=fav_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT s.SystemID, s.Name, m.SortName, m.Path, m.DBID \
         FROM Media m \
         INNER JOIN Systems s ON m.SystemDBID = s.DBID \
         INNER JOIN MediaTags mt ON mt.MediaDBID = m.DBID \
         WHERE mt.TagDBID IN ({placeholders}) AND m.IsMissing = 0 \
         ORDER BY m.SortName ASC, m.DBID ASC"
    );
    let params: Vec<Value> = fav_ids.iter().map(|id| Value::Integer(*id)).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        let system_id: String = r.get(0)?;
        let system_name: String = r.get(1)?;
        let sort_name: String = r.get(2)?;
        let path: String = r.get(3)?;
        let dbid: i64 = r.get(4)?;
        Ok((system_id, system_name, sort_name, path, dbid))
    })?;
    let mut out: Vec<MediaItem> = Vec::new();
    for row in rows {
        if out.len().is_multiple_of(256) && cancelled() {
            return Ok(None);
        }
        let (system_id, system_name, sort_name, path, dbid) = row?;
        let name = if sort_name.is_empty() {
            basename_no_ext(&path)
        } else {
            sort_name
        };
        out.push(MediaItem {
            media_id: Some(dbid),
            name,
            path,
            system: System {
                id: system_id,
                name: system_name,
                ..System::default()
            },
            tags: vec![TagInfo {
                tag: "favorite".to_string(),
                tag_type: "user".to_string(),
                label: String::new(),
            }],
            ..MediaItem::default()
        });
    }
    Ok(Some(out))
}

/// Display-name fallback for rows whose `SortName` was never migrated:
/// the file name with one extension stripped, matching Core's fallback.
fn basename_no_ext(path: &str) -> String {
    let file = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path);
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{basename_no_ext, list_favorites};
    use rusqlite::Connection;

    fn test_db() -> Connection {
        #[allow(clippy::expect_used, reason = "test setup")]
        let conn = Connection::open_in_memory().expect("open in-memory db");
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch(
            "CREATE TABLE Systems (DBID INTEGER PRIMARY KEY, SystemID TEXT NOT NULL, Name TEXT NOT NULL DEFAULT '');
             CREATE TABLE Media (
                 DBID INTEGER PRIMARY KEY,
                 SystemDBID INTEGER NOT NULL,
                 Path TEXT NOT NULL,
                 SortName TEXT NOT NULL DEFAULT '',
                 IsMissing INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE TagTypes (DBID INTEGER PRIMARY KEY, Type TEXT NOT NULL);
             CREATE TABLE Tags (DBID INTEGER PRIMARY KEY, TypeDBID INTEGER NOT NULL, Tag TEXT NOT NULL);
             CREATE TABLE MediaTags (MediaDBID INTEGER NOT NULL, TagDBID INTEGER NOT NULL);
             INSERT INTO Systems VALUES (1,'C64','Commodore 64'),(2,'NES','NES');
             INSERT INTO TagTypes VALUES (1,'user'),(2,'property');
             INSERT INTO Tags VALUES (10,1,'favorite'),(11,2,'image-boxart');
             INSERT INTO Media VALUES
                 (100,1,'/games/C64/Beta.crt','Beta',0),
                 (101,1,'/games/C64/Old Game.crt','',0),
                 (102,2,'/games/NES/Zeta.nes','Zeta',0),
                 (103,1,'/games/C64/Gone.crt','Gone',1),
                 (104,2,'/games/NES/NotFav.nes','NotFav',0);
             INSERT INTO MediaTags VALUES (100,10),(101,10),(102,10),(103,10),(104,11);",
        )
        .expect("seed schema");
        conn
    }

    #[test]
    fn returns_only_tagged_living_rows_in_name_order() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test assertion")]
        let items = list_favorites(&conn, &|| false)
            .expect("query ok")
            .expect("some");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        // Empty SortName sorts first in BINARY order with the stem
        // fallback applied afterwards, matching the listings module;
        // the missing row and the untagged row never appear.
        assert_eq!(names, ["Old Game", "Beta", "Zeta"]);
        assert_eq!(items[1].system.id, "C64");
        assert_eq!(items[1].system.name, "Commodore 64");
        assert_eq!(items[2].system.id, "NES");
        assert!(items.iter().all(|i| i.media_id.is_some()));
        assert!(items.iter().all(|i| i
            .tags
            .iter()
            .any(|t| t.tag == "favorite" && t.tag_type == "user")));
    }

    #[test]
    fn no_favorite_tag_yields_empty_list_not_fallback() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch("DELETE FROM Tags WHERE Tag='favorite'; DELETE FROM MediaTags;")
            .expect("clear tags");
        #[allow(clippy::expect_used, reason = "test assertion")]
        let items = list_favorites(&conn, &|| false)
            .expect("query ok")
            .expect("some");
        assert!(items.is_empty());
    }

    #[test]
    #[allow(clippy::expect_used, reason = "test assertion")]
    fn cancelled_read_returns_none() {
        let conn = test_db();
        assert!(list_favorites(&conn, &|| true).expect("query ok").is_none());
    }

    #[test]
    fn basename_fallback_strips_one_extension() {
        assert_eq!(basename_no_ext("/g/C64/Old Game.crt"), "Old Game");
        assert_eq!(basename_no_ext("plain"), "plain");
    }
}
