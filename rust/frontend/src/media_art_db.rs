// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Read-only artwork-path lookup against Core's media database. Core's
// scrapers already resolve every gamelist/artwork match and store the
// result as a plain file path in `MediaProperties` / `MediaTitleProperties`
// (`docs/scraper.md`: "Path-backed properties persist their text path");
// reading that answer back lets the frontend serve a cover with one
// indexed SELECT plus one file read, skipping Core's `media.image`
// pipeline (globally serialized through a size-1 semaphore) on the very
// first view — the same architecture Console Mode uses via its own art
// map.
//
// Boundaries, stated for the reader who wants to rip this out:
// - Strictly read-only. The connection is opened with
//   `SQLITE_OPEN_READ_ONLY`; this module never writes, checkpoints, or
//   pragma-tunes Core's database.
// - Blob-backed properties (`BlobDBID` set) are ignored here and left to
//   the RPC path. As of Core v2.16 no production scraper writes image
//   blobs (only tests do), so on a gamelist-scraped card this covers
//   effectively everything.
// - Schema drift: the query touches `Media`, `MediaProperties`,
//   `MediaTitleProperties`, `Tags`. A guard at open verifies those
//   tables exist and disables the layer loudly if Core ever renames
//   them, leaving the RPC path as the sole source exactly as before
//   this module existed.
// - Concurrency: Core runs the database in WAL mode (readers do not
//   block its writer and vice versa; Core ships
//   `concurrent_read_during_tx_test.go` for exactly this shape). A
//   short busy timeout plus drop-and-reopen on error keeps a mid-scrape
//   lookup from wedging anything; on any failure the caller falls
//   through to RPC.
//
// `ZAPAROO_FRONTEND_DB_ART=0` disables the layer at runtime for A/B
// comparison.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rusqlite::{Connection, OpenFlags};
use tracing::{debug, info, warn};

use crate::media_image_cache::MediaKey;

pub(crate) const MEDIA_DB_PATH: &str = "/media/fat/zaparoo/media.db";

/// Cap on candidates returned per lookup: bounds the file-read retries
/// the caller performs when leading candidates point at dead files.
const MAX_ART_CANDIDATES: usize = 5;

/// Property tags carrying artwork, in default preference order (mirrors
/// `CORE_DEFAULT_IMAGE_TYPES` in the image cache: the order Core itself
/// consults when no explicit type is requested).
const DEFAULT_TYPE_ORDER: &[&str] = &[
    "image",
    "thumbnail",
    "boxart",
    "boxart3d",
    "screenshot",
    "wheel",
    "titleshot",
    "map",
    "marquee",
    "fanart",
];

/// A resolved artwork file for a media row.
#[derive(Debug)]
pub struct ArtHit {
    pub path: PathBuf,
    /// Bare type name (e.g. `"screenshot"`), matching the `type_tag`
    /// convention of `media.image` responses.
    pub type_tag: String,
}

struct ArtDb {
    conn: Mutex<Option<Connection>>,
    db_path: PathBuf,
}

static ART_DB: OnceLock<Option<ArtDb>> = OnceLock::new();

fn art_db() -> Option<&'static ArtDb> {
    ART_DB
        .get_or_init(|| {
            let disabled = std::env::var("ZAPAROO_FRONTEND_DB_ART")
                .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("off"));
            if disabled {
                info!("media_art_db: disabled via ZAPAROO_FRONTEND_DB_ART");
                return None;
            }
            let db_path = PathBuf::from(MEDIA_DB_PATH);
            if !db_path.is_file() {
                return None;
            }
            match open_checked(&db_path) {
                Ok(conn) => {
                    info!(db = %db_path.display(), "media_art_db: direct art lookup active");
                    Some(ArtDb {
                        conn: Mutex::new(Some(conn)),
                        db_path,
                    })
                }
                Err(e) => {
                    warn!("media_art_db: unavailable ({e}), covers use RPC only");
                    None
                }
            }
        })
        .as_ref()
}

/// Open read-only and verify the tables this module queries actually
/// exist, so a future Core schema change turns the layer off instead of
/// producing wrong answers.
fn open_checked(db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
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
                     the media database directory: {e}"
                );
            }
        }
        e.to_string()
    })?;
    conn.busy_timeout(std::time::Duration::from_millis(200))
        .map_err(|e| e.to_string())?;
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN \
             ('Media','MediaProperties','MediaTitleProperties','Tags')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if n != 4 {
        return Err(format!(
            "expected 4 known tables, found {n} — Core schema changed, refusing to guess"
        ));
    }
    Ok(conn)
}

/// Resolve artwork candidates for `key`, best first. Callers try each
/// candidate's file in order — the database can reference files that no
/// longer exist (an artwork directory reorganized after an early scrape
/// leaves its old paths behind), so a single answer is not enough: the
/// next candidate is often the live one. Returns an empty vec on any miss; errors also
/// drop the cached connection so the next call reopens fresh (covers
/// Core swapping the database file underneath us during recovery).
pub fn resolve_art(key: &MediaKey, preferred_types: &[String]) -> Vec<ArtHit> {
    let Some(db) = art_db() else {
        return Vec::new();
    };
    #[allow(clippy::unwrap_used, reason = "mutex poisoning is unrecoverable")]
    let mut guard = db.conn.lock().unwrap();
    if guard.is_none() {
        match open_checked(&db.db_path) {
            Ok(c) => *guard = Some(c),
            Err(e) => {
                debug!("media_art_db: reopen failed ({e})");
                return Vec::new();
            }
        }
    }
    #[allow(clippy::unwrap_used, reason = "populated just above")]
    let conn = guard.as_ref().unwrap();
    match resolve_with(conn, key, preferred_types) {
        Ok(hits) => hits,
        Err(e) => {
            debug!(
                system_id = %key.system_id,
                path = %key.path,
                "media_art_db: lookup error ({e}), dropping connection",
            );
            *guard = None;
            Vec::new()
        }
    }
}

fn resolve_with(
    conn: &Connection,
    key: &MediaKey,
    preferred_types: &[String],
) -> Result<Vec<ArtHit>, rusqlite::Error> {
    // Collect every Media row this key can identify: the keyed row
    // first (by media_id when the key carries one), then any other row
    // sharing the same file path. Core v2.16's arcade hardware
    // classifications index one physical file under both `Arcade` and a
    // classification system with artwork bound to only one of the rows — same file,
    // same art, so a path twin is a legitimate source, and Core's own
    // gamelist scraper has no folder mapping for the classification
    // systems (a forced scrape processes 0 entries), leaving those rows
    // permanently artless on the Core side.
    let mut rows: Vec<(i64, Option<i64>)> = Vec::new();
    if let Some(id) = key.media_id {
        if let Some(row) = conn
            .query_row(
                "SELECT DBID, MediaTitleDBID FROM Media WHERE DBID = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map(Some)
            .or_else(ignore_no_rows)?
        {
            rows.push(row);
        }
    }
    if !key.path.is_empty() {
        let mut stmt =
            conn.prepare_cached("SELECT DBID, MediaTitleDBID FROM Media WHERE Path = ?1 LIMIT 4")?;
        let found = stmt.query_map([key.path.as_ref()], |r| Ok((r.get(0)?, r.get(1)?)))?;
        for row in found {
            let row: (i64, Option<i64>) = row?;
            if !rows.contains(&row) {
                rows.push(row);
            }
        }
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Gather ALL candidates across the identified rows, keeping the
    // natural precedence as a stable tiebreak: keyed row before twins,
    // media-level before title-level within a row. Then rank by type
    // preference across the whole set — this mirrors media.image, which
    // walks the requested types and finds the first level carrying one,
    // so a preferred `screenshot` at title level beats a non-preferred
    // `image` at media level.
    let mut candidates: Vec<(String, String)> = Vec::new();
    for (media_dbid, title_dbid) in rows {
        candidates.extend(image_properties(
            conn,
            "SELECT t.Tag, p.Text FROM MediaProperties p \
             JOIN Tags t ON t.DBID = p.TypeTagDBID \
             WHERE p.MediaDBID = ?1 AND p.BlobDBID IS NULL \
             AND t.Tag LIKE 'image-%' AND length(coalesce(p.Text,'')) > 0",
            media_dbid,
        )?);
        if let Some(tid) = title_dbid {
            candidates.extend(image_properties(
                conn,
                "SELECT t.Tag, p.Text FROM MediaTitleProperties p \
                 JOIN Tags t ON t.DBID = p.TypeTagDBID \
                 WHERE p.MediaTitleDBID = ?1 AND p.BlobDBID IS NULL \
                 AND t.Tag LIKE 'image-%' AND length(coalesce(p.Text,'')) > 0",
                tid,
            )?);
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Preference rank: position in the caller's list, then the default
    // order, then unknown-but-still-art types last. `sort_by_key` is
    // stable, so equal ranks keep the row/level precedence from above.
    let rank = |tag: &str| -> usize {
        if let Some(i) = preferred_types.iter().position(|w| w == tag) {
            return i;
        }
        if let Some(i) = DEFAULT_TYPE_ORDER.iter().position(|w| *w == tag) {
            return preferred_types.len() + i;
        }
        preferred_types.len() + DEFAULT_TYPE_ORDER.len()
    };
    candidates.sort_by_key(|(tag, _)| rank(tag));
    candidates.truncate(MAX_ART_CANDIDATES);
    Ok(candidates
        .into_iter()
        .map(|(tag, text)| ArtHit {
            path: absolute_art_path(&text),
            type_tag: tag,
        })
        .collect())
}

/// Property paths are `/media/fat/`-absolute on scraped cards. A
/// relative value would resolve
/// against the frontend's working directory, which is never right, so
/// root it at the card instead.
fn absolute_art_path(text: &str) -> PathBuf {
    let p = PathBuf::from(text);
    if p.is_absolute() {
        p
    } else {
        Path::new("/media/fat").join(p)
    }
}

/// Fetch `(bare_type, text_path)` image property rows for one entity.
fn image_properties(
    conn: &Connection,
    sql: &str,
    dbid: i64,
) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map([dbid], |r| {
        let tag: String = r.get(0)?;
        let text: String = r.get(1)?;
        Ok((tag, text))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (tag, text) = row?;
        let bare = tag.strip_prefix("image-").unwrap_or(&tag).to_string();
        out.push((bare, text));
    }
    Ok(out)
}

#[allow(clippy::unnecessary_wraps, reason = "or_else adapter shape")]
fn ignore_no_rows<T>(e: rusqlite::Error) -> Result<Option<T>, rusqlite::Error> {
    match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    }
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
            "CREATE TABLE Tags (DBID INTEGER PRIMARY KEY, Tag TEXT);
             CREATE TABLE Media (DBID INTEGER PRIMARY KEY, MediaTitleDBID INTEGER, Path TEXT);
             CREATE TABLE MediaProperties (
                 DBID INTEGER PRIMARY KEY, MediaDBID INTEGER,
                 TypeTagDBID INTEGER, Text TEXT, BlobDBID INTEGER);
             CREATE TABLE MediaTitleProperties (
                 DBID INTEGER PRIMARY KEY, MediaTitleDBID INTEGER,
                 TypeTagDBID INTEGER, Text TEXT, BlobDBID INTEGER);
             INSERT INTO Tags VALUES (1,'image-screenshot'),(2,'image-boxart'),(3,'genre');
             INSERT INTO Media VALUES (10, 100, '/media/fat/games/X/game.bin');
             INSERT INTO MediaProperties VALUES (1, 10, 1, '/art/shot.png', NULL);
             INSERT INTO MediaProperties VALUES (2, 10, 3, 'Action', NULL);
             INSERT INTO MediaTitleProperties VALUES (1, 100, 2, '/art/box.png', NULL);",
        )
        .unwrap();
        conn
    }

    fn key() -> MediaKey {
        MediaKey::new("X", "/media/fat/games/X/game.bin")
    }

    #[test]
    fn resolves_candidates_by_path_in_default_type_order() {
        let conn = test_db();
        let hits = resolve_with(&conn, &key(), &[]).unwrap();
        // Default order ranks boxart (title level) above screenshot.
        assert_eq!(hits[0].type_tag, "boxart");
        assert_eq!(hits[1].type_tag, "screenshot");
        assert_eq!(hits[1].path, PathBuf::from("/art/shot.png"));
    }

    #[test]
    fn preferred_type_at_title_level_beats_other_type_at_media_level() {
        // media.image walks the requested types in order and serves the
        // first level carrying one, so a preferred boxart at title level
        // outranks a non-preferred screenshot at media level.
        let conn = test_db();
        let hits = resolve_with(&conn, &key(), &["boxart".into()]).unwrap();
        assert_eq!(hits[0].type_tag, "boxart");
        assert_eq!(hits[0].path, PathBuf::from("/art/box.png"));
        // The screenshot stays available as the retry candidate.
        assert_eq!(hits[1].type_tag, "screenshot");
    }

    #[test]
    fn dead_media_level_path_keeps_live_screenshot_as_next_candidate() {
        // The dead-path shape: media-level `image` points at a file that
        // was deleted, title-level `screenshot` is the live art.
        // With a screenshot preference the live file ranks FIRST and the
        // dead one is merely a later candidate.
        let conn = test_db();
        conn.execute("UPDATE Tags SET Tag = 'image-image' WHERE DBID = 1", [])
            .unwrap();
        conn.execute(
            "UPDATE MediaProperties SET Text = '/art/deleted-dir/old.jpg' WHERE TypeTagDBID = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE Tags SET Tag = 'image-screenshot' WHERE DBID = 2",
            [],
        )
        .unwrap();
        let hits = resolve_with(&conn, &key(), &["screenshot".into()]).unwrap();
        assert_eq!(hits[0].type_tag, "screenshot");
        assert_eq!(hits[0].path, PathBuf::from("/art/box.png"));
        assert_eq!(hits[1].path, PathBuf::from("/art/deleted-dir/old.jpg"));
    }

    #[test]
    fn falls_back_to_title_level_when_media_has_none() {
        let conn = test_db();
        conn.execute("DELETE FROM MediaProperties WHERE TypeTagDBID = 1", [])
            .unwrap();
        let hits = resolve_with(&conn, &key(), &[]).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/art/box.png"));
        assert_eq!(hits[0].type_tag, "boxart");
    }

    #[test]
    fn blob_backed_rows_are_ignored() {
        let conn = test_db();
        conn.execute(
            "UPDATE MediaProperties SET BlobDBID = 7 WHERE TypeTagDBID = 1",
            [],
        )
        .unwrap();
        // Media-level art is now blob-backed -> skipped; falls to title.
        let hits = resolve_with(&conn, &key(), &[]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].type_tag, "boxart");
    }

    #[test]
    fn unknown_media_returns_empty() {
        let conn = test_db();
        let k = MediaKey::new("X", "/nope");
        assert!(resolve_with(&conn, &k, &[]).unwrap().is_empty());
    }

    #[test]
    fn artless_duplicate_row_borrows_from_path_twin() {
        // Core v2.16 arcade classifications: same file indexed under a
        // second system with no art bound. A key carrying the artless
        // row's media_id must still resolve via the path twin.
        let conn = test_db();
        conn.execute(
            "INSERT INTO Media VALUES (11, 200, '/media/fat/games/X/game.bin')",
            [],
        )
        .unwrap();
        let mut k = key();
        k.media_id = Some(11);
        let hits = resolve_with(&conn, &k, &["screenshot".into()]).unwrap();
        assert!(!hits.is_empty(), "twin must supply candidates");
        assert_eq!(hits[0].path, PathBuf::from("/art/shot.png"));
    }

    #[test]
    fn keyed_row_art_ranks_before_equal_type_twin() {
        // Same art type on both rows: stable ordering keeps the keyed
        // row's own art first.
        let conn = test_db();
        conn.execute(
            "INSERT INTO Media VALUES (11, 200, '/media/fat/games/X/game.bin')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO MediaProperties VALUES (9, 11, 1, '/art/own-shot.png', NULL)",
            [],
        )
        .unwrap();
        let mut k = key();
        k.media_id = Some(11);
        let hits = resolve_with(&conn, &k, &["screenshot".into()]).unwrap();
        assert_eq!(hits[0].path, PathBuf::from("/art/own-shot.png"));
    }

    #[test]
    fn relative_property_paths_root_at_the_card() {
        assert_eq!(
            absolute_art_path("games/X/media/screenshot/a.png"),
            PathBuf::from("/media/fat/games/X/media/screenshot/a.png")
        );
        assert_eq!(
            absolute_art_path("/media/fat/a.png"),
            PathBuf::from("/media/fat/a.png")
        );
    }

    #[test]
    fn schema_guard_rejects_foreign_database() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("other.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (x);")
            .unwrap();
        assert!(open_checked(&p).is_err());
    }
}
