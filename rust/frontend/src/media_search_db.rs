// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Read-only same-title lookup against Core's media database, backing
// the frontend's only `media.search` consumer (alternate-version
// discovery). The RPC search slugifies the query words and matches
// `MediaTitles.Slug LIKE %variant%`; this module skips the query-side
// slug engine entirely and matches on slug EQUALITY with the selected
// row's own stored slug — the canonical same-title grouping the
// database already maintains, and the thing the search round trip was
// approximating for this consumer. Known divergence, accepted for this
// path: titles whose slugs differ but whose names normalize equal
// would be found by the LIKE search and not by this lookup; the
// consumer's own name-normalization filter runs on both paths, and a
// `None` here falls back to the RPC exactly as before this module
// existed. A future general search UI would need the full slug-variant
// engine and should not build on this module.
//
// Same contract as every read domain (see `media_db`): read-only,
// schema-guarded, kill switch `ZAPAROO_FRONTEND_DB_SEARCH=0`, RPC
// fallback on `None`.

use std::sync::OnceLock;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension};
use tracing::debug;

use zaparoo_core::media_types::{MediaItem, System};

use crate::media_db::{ReadDb, ReadDbSpec, MEDIA_DB_PATH};

static SEARCH_DB: OnceLock<Option<ReadDb>> = OnceLock::new();

static SPEC: ReadDbSpec = ReadDbSpec {
    domain: "media_search_db",
    env_switch: "ZAPAROO_FRONTEND_DB_SEARCH",
    db_path: MEDIA_DB_PATH,
    required_tables: &["Media", "MediaTitles", "Systems"],
    uri: false,
    prepare: None,
    active_msg: "direct same-title lookup active",
    fallback_msg: "alternate discovery uses RPC search only",
};

fn search_db() -> Option<&'static ReadDb> {
    SEARCH_DB.get_or_init(|| ReadDb::init(&SPEC)).as_ref()
}

/// Whether the direct same-title layer initialized.
pub fn enabled() -> bool {
    search_db().is_some()
}

/// Every non-missing media row in `system_id` sharing the selected
/// row's title slug, in `Media.DBID` order (the RPC search's stable
/// order). Returns `None` when the selected path is unknown to the
/// database or on any error — the caller falls back to the RPC search.
/// The result deliberately includes the selected row itself, matching
/// the RPC behaviour the consumer's own filters already handle.
pub fn same_title_media(system_id: &str, path: &str) -> Option<Vec<MediaItem>> {
    let db = search_db()?;
    let started = Instant::now();
    let items = db.with_conn(
        || format!("same-title lookup path={path}"),
        |conn| lookup_same_title(conn, system_id, path),
    )??;
    debug!(
        entries = items.len(),
        lookup_ms = started.elapsed().as_millis(),
        "media_search_db: same-title rows listed",
    );
    Some(items)
}

/// Async wrapper for call sites on the runtime: point queries, but
/// still file IO, so they run on the blocking pool.
pub async fn same_title_media_async(system_id: String, path: String) -> Option<Vec<MediaItem>> {
    if !enabled() {
        return None;
    }
    tokio::task::spawn_blocking(move || same_title_media(&system_id, &path))
        .await
        .ok()
        .flatten()
}

fn lookup_same_title(
    conn: &Connection,
    system_id: &str,
    path: &str,
) -> rusqlite::Result<Option<Vec<MediaItem>>> {
    // The selected row's stored slug. An unknown path is a `None`
    // answer (RPC decides), not an empty list: this module cannot vouch
    // for a row it cannot see (fresh scrape mid-index, virtual path).
    let slug: Option<String> = conn
        .query_row(
            "SELECT mt.Slug FROM Media m \
             INNER JOIN MediaTitles mt ON mt.DBID = m.MediaTitleDBID \
             INNER JOIN Systems s ON s.DBID = m.SystemDBID \
             WHERE m.Path = ?1 AND s.SystemID = ?2 LIMIT 1",
            rusqlite::params![path, system_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(slug) = slug else {
        return Ok(None);
    };
    let mut stmt = conn.prepare_cached(
        "SELECT mt.Name, m.Path, m.DBID FROM MediaTitles mt \
         INNER JOIN Systems s ON s.DBID = mt.SystemDBID \
         INNER JOIN Media m ON m.MediaTitleDBID = mt.DBID \
         WHERE s.SystemID = ?1 AND mt.Slug = ?2 AND m.IsMissing = 0 \
         ORDER BY m.DBID ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![system_id, slug], |r| {
        let name: String = r.get(0)?;
        let row_path: String = r.get(1)?;
        let dbid: i64 = r.get(2)?;
        Ok(MediaItem {
            media_id: Some(dbid),
            name,
            path: row_path,
            system: System {
                id: system_id.to_string(),
                ..System::default()
            },
            ..MediaItem::default()
        })
    })?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(Some(items))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        reason = "tests should fail-fast on unexpected errors"
    )]

    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE Systems (DBID INTEGER PRIMARY KEY, SystemID TEXT, Name TEXT);
             CREATE TABLE MediaTitles (
                 DBID INTEGER PRIMARY KEY, SystemDBID INTEGER, Slug TEXT, Name TEXT);
             CREATE TABLE Media (
                 DBID INTEGER PRIMARY KEY, MediaTitleDBID INTEGER, SystemDBID INTEGER,
                 Path TEXT, IsMissing INTEGER DEFAULT 0);
             INSERT INTO Systems VALUES (1, 'Arcade', 'Arcade');
             INSERT INTO MediaTitles VALUES (10, 1, 'streetfighter2', 'Street Fighter II');
             INSERT INTO MediaTitles VALUES (11, 1, 'streetfighter2', 'Street Fighter II (alt)');
             INSERT INTO MediaTitles VALUES (12, 1, 'finalfight', 'Final Fight');
             INSERT INTO Media VALUES (100, 10, 1, '/games/Arcade/sf2.zip', 0);
             INSERT INTO Media VALUES (101, 11, 1, '/games/Arcade/_alternatives/sf2 v2/sf2b.zip', 0);
             INSERT INTO Media VALUES (102, 12, 1, '/games/Arcade/ff.zip', 0);
             INSERT INTO Media VALUES (103, 10, 1, '/games/Arcade/sf2-gone.zip', 1);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn same_slug_rows_return_in_dbid_order_missing_excluded() {
        let conn = test_db();
        let items = lookup_same_title(&conn, "Arcade", "/games/Arcade/sf2.zip")
            .unwrap()
            .expect("known path answers");
        let paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
        // Selected row itself, then the alternate; the different title
        // and the IsMissing row are excluded.
        assert_eq!(
            paths,
            [
                "/games/Arcade/sf2.zip",
                "/games/Arcade/_alternatives/sf2 v2/sf2b.zip"
            ]
        );
        assert_eq!(items[0].media_id, Some(100));
        assert_eq!(items[1].name, "Street Fighter II (alt)");
        assert_eq!(items[1].system.id, "Arcade");
    }

    #[test]
    fn unknown_path_returns_none_for_rpc_fallback() {
        let conn = test_db();
        assert!(lookup_same_title(&conn, "Arcade", "/nope.zip")
            .unwrap()
            .is_none());
    }

    #[test]
    fn wrong_system_scope_returns_none() {
        // The same path under a different system id is not the selected
        // row; the RPC keeps authority over cross-system resolution.
        let conn = test_db();
        assert!(lookup_same_title(&conn, "NES", "/games/Arcade/sf2.zip")
            .unwrap()
            .is_none());
    }
}
