// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Read-only folder listing against Core's media database. Core already
// maintains everything a browse page needs as plain indexed tables:
// `BrowseDirs`/`BrowseDirCounts` hold the pre-computed directory tree
// (built by Core's browse cache), and `Media` rows carry `ParentDir`,
// `SortName` and `IsMissing` behind the dedicated
// `idx_media_browse_sort` index. Reading those tables directly lets the
// frontend load an entire folder in one local query instead of paging
// it over the RPC one chunk at a time — the same read-only fast-path
// architecture as `media_art_db`, extended from artwork to listings.
//
// The payoff is structural, not just latency: a full local listing has
// no `has_next_page`, so the paged fetch machinery (background fill,
// tail prefetch, edge stalls, the append trickle competing with input
// on the UI thread) simply never engages for a folder served here.
//
// Boundaries, stated for the reader who wants to rip this out:
// - Strictly read-only, same contract as `media_art_db`: opened with
//   `SQLITE_OPEN_READ_ONLY`, never writes, checkpoints, or tunes.
// - Folder listings only (`path` non-empty). Root browsing, the letter
//   index, search, meta, and favorite writes stay on the RPC — roots
//   need Core's launcher routes, which do not live in the database.
// - Requires Core's browse cache to be serveable
//   (`DBConfig.BrowseIndexVersion` of "2" or "2-stale", the same gate
//   Core itself uses in `sqlBrowseCacheStatus`). Anything else returns
//   `None` and the caller uses the RPC exactly as before.
// - Known divergences from Core's `media.browse`, accepted for this
//   path: no singleton media-container aliasing (zip-as-dir folders
//   render as plain directories), no rank/date prefix sort-mode
//   detection (always `SortName ASC, DBID ASC`, Core's `name-asc`),
//   and no `disambiguatingTags` variant badges.
// - Schema drift: a guard at open verifies every table this module
//   touches and disables the layer loudly if Core renames one.
//
// `ZAPAROO_FRONTEND_DB_BROWSE=0` disables the layer at runtime for A/B
// comparison.

use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension};
use tracing::debug;

use zaparoo_core::media_types::{BrowseEntry, MediaBrowseResult, TagInfo};

use crate::media_db::{ReadDb, ReadDbSpec, MEDIA_DB_PATH};

static BROWSE_DB: OnceLock<Option<ReadDb>> = OnceLock::new();

static SPEC: ReadDbSpec = ReadDbSpec {
    domain: "media_browse_db",
    env_switch: "ZAPAROO_FRONTEND_DB_BROWSE",
    db_path: MEDIA_DB_PATH,
    required_tables: &[
        "Media",
        "Systems",
        "MediaTitles",
        "BrowseDirs",
        "BrowseDirCounts",
        "MediaTags",
        "Tags",
        "TagTypes",
        "DBConfig",
        "MediaProperties",
        "MediaTitleProperties",
    ],
    uri: false,
    prepare: None,
    active_msg: "direct folder listing active",
    fallback_msg: "listings use RPC only",
};

fn browse_db() -> Option<&'static ReadDb> {
    BROWSE_DB.get_or_init(|| ReadDb::init(&SPEC)).as_ref()
}

/// Whether the direct listing layer initialized. A `true` here does not
/// promise a given folder will resolve — `browse_folder` still returns
/// `None` per call when the browse cache is not serveable — it only
/// says the database is present, readable, and schema-compatible.
pub fn enabled() -> bool {
    browse_db().is_some()
}

/// List a folder straight from the database. Returns `None` whenever the
/// answer cannot be produced with full confidence — layer disabled, cache
/// not serveable, path unknown — and the caller falls through to the RPC
/// exactly as before this module existed.
///
/// `cancelled` is polled at each query stage and periodically inside the
/// row loop so a superseded browse (the user already navigated away)
/// stops consuming its blocking worker and the database instead of
/// running the full folder read to completion; a cancelled call returns
/// `None`, which the caller's ticket check discards anyway.
pub fn browse_folder(
    path: &str,
    systems: &[String],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<MediaBrowseResult> {
    let db = browse_db()?;
    if cancelled() {
        return None;
    }
    let started = Instant::now();
    let result = db.with_conn(
        || format!("folder listing path={path}"),
        |conn| list_folder(conn, path, systems, cancelled),
    )??;
    if cancelled() {
        return None;
    }
    debug!(
        ?path,
        entries = result.entries.len(),
        total_files = result.total_files,
        lookup_ms = started.elapsed().as_millis(),
        "media_browse_db: folder listed",
    );
    Some(result)
}

/// SQL fragment `?N,?N+1,...` for `len` placeholders starting at `start`.
fn placeholder_list(start: usize, len: usize) -> String {
    (start..start + len)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn tag_ids(
    conn: &Connection,
    tag_type: &str,
    tag_match: &str,
    like: bool,
) -> rusqlite::Result<Vec<i64>> {
    let sql = if like {
        "SELECT t.DBID FROM Tags t JOIN TagTypes tt ON tt.DBID = t.TypeDBID \
         WHERE tt.Type = ?1 AND t.Tag LIKE ?2 ORDER BY t.DBID"
    } else {
        "SELECT t.DBID FROM Tags t JOIN TagTypes tt ON tt.DBID = t.TypeDBID \
         WHERE tt.Type = ?1 AND t.Tag = ?2 ORDER BY t.DBID"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params![tag_type, tag_match], |r| r.get(0))?;
    rows.collect()
}

/// Display-name fallback for `Media.SortName == ''` (rows predating
/// Core's sortname migration): the filename without its extension,
/// mirroring Core's `sqlBrowseFilesFromMedia` fallback.
fn basename_no_ext(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => base.to_string(),
    }
}

fn list_folder(
    conn: &Connection,
    path: &str,
    systems: &[String],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> rusqlite::Result<Option<MediaBrowseResult>> {
    let cleaned = path.trim_end_matches('/');
    if cleaned.is_empty() {
        return Ok(None);
    }
    // Core stores both `BrowseDirs.Path` and `Media.ParentDir` with a
    // trailing slash (`browseCacheAncestorDirs`, `ParentDirForMediaPath`).
    let prefix = format!("{cleaned}/");

    // Same serveability gate as Core's `sqlBrowseCacheStatus`: version
    // "2" is fresh, "2-stale" is stale-but-served, anything else means
    // the cache is absent or mid-rebuild and the RPC must answer.
    let version: Option<String> = conn
        .query_row(
            "SELECT Value FROM DBConfig WHERE Name = 'BrowseIndexVersion'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if !matches!(version.as_deref(), Some("2" | "2-stale")) {
        return Ok(None);
    }

    let parent_id: Option<i64> = conn
        .query_row(
            "SELECT DBID FROM BrowseDirs WHERE Path = ?1",
            [&prefix],
            |r| r.get(0),
        )
        .optional()?;

    let mut entries: Vec<BrowseEntry> = Vec::new();
    if let Some(pid) = parent_id {
        dir_entries(conn, pid, cleaned, systems, &mut entries)?;
    }
    if cancelled() {
        return Ok(None);
    }
    let total_dirs = u32::try_from(entries.len()).unwrap_or(u32::MAX);
    let total_files = file_entries(conn, &prefix, systems, &mut entries, cancelled)?;
    if cancelled() {
        return Ok(None);
    }

    // A path the browse cache does not know, with no files either, is
    // one this module cannot vouch for (fresh folder mid-index, virtual
    // scheme, typo): let the RPC answer rather than asserting "empty".
    if parent_id.is_none() && total_files == 0 {
        return Ok(None);
    }

    Ok(Some(MediaBrowseResult {
        path: cleaned.to_string(),
        entries,
        total_files,
        total_dirs: Some(total_dirs),
        pagination: None,
    }))
}

/// Immediate child directories, mirroring Core's cache-path query:
/// recursive per-system file counts summed per child, empty dirs never
/// present, BINARY name order (dirs always precede files).
fn dir_entries(
    conn: &Connection,
    parent_id: i64,
    cleaned: &str,
    systems: &[String],
    entries: &mut Vec<BrowseEntry>,
) -> rusqlite::Result<()> {
    let mut sql = String::from(
        "SELECT d.Name, SUM(c.FileCount), GROUP_CONCAT(DISTINCT s.SystemID) \
         FROM BrowseDirCounts c \
         INNER JOIN BrowseDirs d ON c.ChildDirDBID = d.DBID \
         INNER JOIN Systems s ON c.SystemDBID = s.DBID \
         WHERE c.ParentDirDBID = ?1 AND c.ChildDirDBID != c.ParentDirDBID \
         AND d.IsVirtual = 0",
    );
    let mut params: Vec<Value> = vec![Value::Integer(parent_id)];
    if !systems.is_empty() {
        let _ = write!(
            sql,
            " AND s.SystemID IN ({})",
            placeholder_list(2, systems.len())
        );
        params.extend(systems.iter().map(|s| Value::Text(s.clone())));
    }
    sql.push_str(" GROUP BY d.DBID, d.Name ORDER BY d.Name ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        let name: String = r.get(0)?;
        let file_count: i64 = r.get(1)?;
        let sids: Option<String> = r.get(2)?;
        Ok((name, file_count, sids))
    })?;
    for row in rows {
        let (name, file_count, sids) = row?;
        let mut system_ids: Vec<String> = sids
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        system_ids.sort();
        system_ids.dedup();
        entries.push(BrowseEntry {
            path: format!("{cleaned}/{name}"),
            name,
            entry_type: "directory".to_string(),
            file_count: u32::try_from(file_count).unwrap_or(u32::MAX),
            system_ids,
            ..BrowseEntry::default()
        });
    }
    Ok(())
}

/// Media files with `ParentDir` equal to `prefix`, in Core's `name-asc`
/// order (`SortName ASC, DBID ASC`, BINARY collation). The favorite
/// heart and has-cover flag come from tag-id sets resolved the same way
/// Core does (`resolveUtilityTagDBIDs`, `resolveImagePropertyTagDBIDs`);
/// `favorite` is stored unpadded (`PadTagValue` only pads all-digit
/// values). Returns the number of files appended.
fn file_entries(
    conn: &Connection,
    prefix: &str,
    systems: &[String],
    entries: &mut Vec<BrowseEntry>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> rusqlite::Result<u32> {
    let fav_ids = tag_ids(conn, "user", "favorite", false)?;
    let img_ids = tag_ids(conn, "property", "image-%", true)?;

    let mut sql = String::from("SELECT s.SystemID, m.SortName, m.Path, m.DBID");
    let mut params: Vec<Value> = vec![Value::Text(prefix.to_string())];
    let mut next_ph = 2usize;
    if fav_ids.is_empty() {
        sql.push_str(", 0");
    } else {
        let _ = write!(
            sql,
            ", EXISTS(SELECT 1 FROM MediaTags x WHERE x.MediaDBID = m.DBID AND x.TagDBID IN ({}))",
            placeholder_list(next_ph, fav_ids.len())
        );
        params.extend(fav_ids.iter().map(|id| Value::Integer(*id)));
        next_ph += fav_ids.len();
    }
    if img_ids.is_empty() {
        // No image property tags exist yet (pre-scrape database). Match
        // the wire default: report covers as available so the image
        // path, not the listing, decides.
        sql.push_str(", 1");
    } else {
        let ph_a = placeholder_list(next_ph, img_ids.len());
        params.extend(img_ids.iter().map(|id| Value::Integer(*id)));
        next_ph += img_ids.len();
        let ph_b = placeholder_list(next_ph, img_ids.len());
        params.extend(img_ids.iter().map(|id| Value::Integer(*id)));
        next_ph += img_ids.len();
        let _ = write!(
            sql,
            ", (EXISTS(SELECT 1 FROM MediaProperties mp WHERE mp.MediaDBID = m.DBID \
             AND mp.TypeTagDBID IN ({ph_a})) \
             OR EXISTS(SELECT 1 FROM MediaTitleProperties tp \
             WHERE tp.MediaTitleDBID = m.MediaTitleDBID AND tp.TypeTagDBID IN ({ph_b})))"
        );
    }
    sql.push_str(
        " FROM Media m INNER JOIN Systems s ON m.SystemDBID = s.DBID \
         WHERE m.ParentDir = ?1 AND m.IsMissing = 0",
    );
    if !systems.is_empty() {
        let _ = write!(
            sql,
            " AND s.SystemID IN ({})",
            placeholder_list(next_ph, systems.len())
        );
        params.extend(systems.iter().map(|s| Value::Text(s.clone())));
    }
    sql.push_str(" ORDER BY m.SortName ASC, m.DBID ASC");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        let system_id: String = r.get(0)?;
        let sort_name: String = r.get(1)?;
        let file_path: String = r.get(2)?;
        let dbid: i64 = r.get(3)?;
        let favorite: bool = r.get(4)?;
        let has_cover: bool = r.get(5)?;
        Ok((system_id, sort_name, file_path, dbid, favorite, has_cover))
    })?;
    let mut total_files = 0u32;
    for row in rows {
        // Poll every 256 rows: often enough that a superseded read on a
        // huge folder stops within milliseconds, rare enough to cost
        // nothing on the happy path.
        if total_files.is_multiple_of(256) && cancelled() {
            return Ok(0);
        }
        let (system_id, sort_name, file_path, dbid, favorite, has_cover) = row?;
        total_files = total_files.saturating_add(1);
        let name = if sort_name.is_empty() {
            basename_no_ext(&file_path)
        } else {
            sort_name
        };
        let tags = if favorite {
            vec![TagInfo {
                tag: "favorite".to_string(),
                tag_type: "user".to_string(),
                label: String::new(),
            }]
        } else {
            Vec::new()
        };
        entries.push(BrowseEntry {
            media_id: Some(dbid),
            name,
            path: file_path,
            entry_type: "media".to_string(),
            system_id,
            tags,
            has_cover,
            ..BrowseEntry::default()
        });
    }
    Ok(total_files)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        reason = "tests should fail-fast on unexpected errors"
    )]

    use super::{basename_no_ext, list_folder};
    use rusqlite::Connection;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE DBConfig (Name text PRIMARY KEY, Value text);
             CREATE TABLE Systems (DBID INTEGER PRIMARY KEY, SystemID text, Name text);
             CREATE TABLE MediaTitles (DBID INTEGER PRIMARY KEY, SystemDBID integer, Name text);
             CREATE TABLE Media (
                 DBID INTEGER PRIMARY KEY, MediaTitleDBID integer, SystemDBID integer,
                 Path text, ParentDir text NOT NULL DEFAULT '',
                 IsMissing integer NOT NULL DEFAULT 0, SortName text NOT NULL DEFAULT ''
             );
             CREATE TABLE BrowseDirs (
                 DBID integer primary key, ParentDirDBID integer,
                 Path text unique not null, Name text not null, IsVirtual bool default false
             );
             CREATE TABLE BrowseDirCounts (
                 ParentDirDBID integer not null, ChildDirDBID integer not null,
                 SystemDBID integer not null, FileCount integer not null
             );
             CREATE TABLE TagTypes (DBID INTEGER PRIMARY KEY, Type text);
             CREATE TABLE Tags (DBID INTEGER PRIMARY KEY, TypeDBID integer, Tag text);
             CREATE TABLE MediaTags (MediaDBID INTEGER NOT NULL, TagDBID INTEGER NOT NULL);
             CREATE TABLE MediaProperties (
                 DBID INTEGER PRIMARY KEY, MediaDBID integer, TypeTagDBID integer,
                 Text text NOT NULL DEFAULT '', BlobDBID integer
             );
             CREATE TABLE MediaTitleProperties (
                 DBID INTEGER PRIMARY KEY, MediaTitleDBID integer, TypeTagDBID integer,
                 Text text NOT NULL DEFAULT '', BlobDBID integer
             );
             INSERT INTO DBConfig VALUES ('BrowseIndexVersion', '2');
             INSERT INTO Systems VALUES (1, 'C64', 'Commodore 64');
             INSERT INTO Systems VALUES (2, 'NES', 'NES');
             INSERT INTO MediaTitles VALUES (10, 1, 'Alpha');
             INSERT INTO MediaTitles VALUES (11, 1, 'Beta');
             INSERT INTO MediaTitles VALUES (12, 2, 'Gamma');
             INSERT INTO BrowseDirs VALUES (1, NULL, '/games/', 'games', 0);
             INSERT INTO BrowseDirs VALUES (2, 1, '/games/C64/', 'C64', 0);
             INSERT INTO BrowseDirs VALUES (3, 2, '/games/C64/Extras/', 'Extras', 0);
             INSERT INTO BrowseDirCounts VALUES (2, 3, 1, 7);
             INSERT INTO TagTypes VALUES (1, 'user');
             INSERT INTO TagTypes VALUES (2, 'property');
             INSERT INTO Tags VALUES (1, 1, 'favorite');
             INSERT INTO Tags VALUES (2, 2, 'image-screenshot');
             -- Files in /games/C64/: Beta sorts before Zeta; the third
             -- row has an empty SortName and must fall back to its stem.
             INSERT INTO Media VALUES (100, 11, 1, '/games/C64/beta.crt', '/games/C64/', 0, 'Beta');
             INSERT INTO Media VALUES (101, 10, 1, '/games/C64/zeta.crt', '/games/C64/', 0, 'Zeta');
             INSERT INTO Media VALUES (102, 10, 1, '/games/C64/Old Game.crt', '/games/C64/', 0, '');
             INSERT INTO Media VALUES (103, 12, 2, '/games/C64/nes-file.nes', '/games/C64/', 0, 'NesFile');
             INSERT INTO Media VALUES (104, 10, 1, '/games/C64/missing.crt', '/games/C64/', 1, 'Missing');
             INSERT INTO MediaTags VALUES (100, 1);
             INSERT INTO MediaProperties VALUES (1, 100, 2, '/art/beta.png', NULL);
             INSERT INTO MediaTitleProperties VALUES (1, 10, 2, '/art/alpha.png', NULL);",
        )
        .expect("seed schema");
        conn
    }

    #[test]
    fn dirs_precede_files_and_both_are_name_ordered() {
        let conn = test_db();
        let result = list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .expect("folder listed");
        let names: Vec<&str> = result.entries.iter().map(|e| e.name.as_str()).collect();
        // Files order by SortName in SQL, and the empty-SortName row
        // sorts before everything ('' < 'Beta' in BINARY order) with
        // its display-name fallback applied only afterwards — exactly
        // Core's behavior (`sql_browse.go` applies the filename
        // fallback post-query too).
        assert_eq!(names, ["Extras", "Old Game", "Beta", "NesFile", "Zeta"]);
        assert_eq!(result.entries[0].entry_type, "directory");
        assert_eq!(result.entries[0].file_count, 7);
        assert_eq!(result.total_dirs, Some(1));
        assert_eq!(result.total_files, 4);
        assert!(result.pagination.is_none());
    }

    #[test]
    fn favorite_tag_maps_to_user_favorite() {
        let conn = test_db();
        let result = list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .expect("folder listed");
        let beta = result
            .entries
            .iter()
            .find(|e| e.name == "Beta")
            .expect("beta present");
        assert_eq!(beta.tags.len(), 1);
        assert_eq!(beta.tags[0].tag, "favorite");
        assert_eq!(beta.tags[0].tag_type, "user");
        let zeta = result
            .entries
            .iter()
            .find(|e| e.name == "Zeta")
            .expect("zeta present");
        assert!(zeta.tags.is_empty());
    }

    #[test]
    fn has_cover_from_media_and_title_level_properties() {
        let conn = test_db();
        let result = list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .expect("folder listed");
        let by_name = |n: &str| {
            result
                .entries
                .iter()
                .find(|e| e.name == n)
                .unwrap_or_else(|| panic!("{n} present"))
        };
        // Beta: media-level property. Zeta: title 10's title-level
        // property. NesFile: title 12, no properties anywhere.
        assert!(by_name("Beta").has_cover);
        assert!(by_name("Zeta").has_cover);
        assert!(!by_name("NesFile").has_cover);
    }

    #[test]
    fn empty_sortname_falls_back_to_file_stem() {
        let conn = test_db();
        let result = list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .expect("folder listed");
        assert!(result.entries.iter().any(|e| e.name == "Old Game"));
    }

    #[test]
    fn system_filter_excludes_other_systems() {
        let conn = test_db();
        let result = list_folder(&conn, "/games/C64", &["C64".to_string()], &|| false)
            .expect("query ok")
            .expect("folder listed");
        assert!(result.entries.iter().all(|e| e.name != "NesFile"));
        assert_eq!(result.total_files, 3);
    }

    #[test]
    fn missing_rows_are_excluded() {
        let conn = test_db();
        let result = list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .expect("folder listed");
        assert!(result.entries.iter().all(|e| e.name != "Missing"));
    }

    #[test]
    fn unserveable_cache_version_defers_to_rpc() {
        let conn = test_db();
        conn.execute(
            "UPDATE DBConfig SET Value = '1' WHERE Name = 'BrowseIndexVersion'",
            [],
        )
        .expect("update version");
        assert!(list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .is_none());
    }

    #[test]
    fn unknown_empty_path_defers_to_rpc() {
        let conn = test_db();
        assert!(list_folder(&conn, "/games/Amiga", &[], &|| false)
            .expect("query ok")
            .is_none());
    }

    #[test]
    fn trailing_slash_input_lists_the_same_folder() {
        let conn = test_db();
        let a = list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .expect("listed");
        let b = list_folder(&conn, "/games/C64/", &[], &|| false)
            .expect("query ok")
            .expect("listed");
        assert_eq!(a.entries.len(), b.entries.len());
        assert_eq!(a.path, b.path);
    }

    #[test]
    fn stale_cache_version_still_serves() {
        let conn = test_db();
        conn.execute(
            "UPDATE DBConfig SET Value = '2-stale' WHERE Name = 'BrowseIndexVersion'",
            [],
        )
        .expect("update version");
        assert!(list_folder(&conn, "/games/C64", &[], &|| false)
            .expect("query ok")
            .is_some());
    }

    #[test]
    fn basename_fallback_strips_one_extension() {
        assert_eq!(basename_no_ext("/a/b/Game Name.crt"), "Game Name");
        assert_eq!(basename_no_ext("/a/b/noext"), "noext");
        assert_eq!(basename_no_ext("/a/b/.hidden"), ".hidden");
    }

    #[test]
    fn cancelled_read_returns_none() {
        let conn = test_db();
        // A read whose browse got superseded must bail with None no
        // matter how far it progressed; the caller's ticket check
        // discards the slot either way, so None is the only honest
        // answer a cancelled read can give.
        assert!(list_folder(&conn, "/games/C64", &[], &|| true)
            .expect("query ok")
            .is_none());
    }
}
