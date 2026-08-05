// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Read-only media metadata against Core's media database. The detail
// pane and the game-info modal fetch `media.meta` per selection settle;
// everything the response carries — tags, scraped properties, the
// title record, available image types — lives in plain indexed tables,
// so a handful of point lookups replace the round trip. The same
// read-only fast-path architecture as the other direct modules; the
// RPC remains the fallback for any `None`.
//
// Known divergences from Core's `media.meta`, accepted for this path:
// - `TagInfo.label` is empty (labels come from Core's tag registry,
//   not the database); every consumer falls back to the tag value via
//   `tag_display_value`.
// - `MediaMetaProperty.content_type`/`blob_size` are not populated
//   (no consumer reads them from meta); `extension` derives from the
//   property's file path where one exists.
//
// `ZAPAROO_FRONTEND_DB_META=0` disables the layer at runtime for A/B
// comparison.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension};
use tracing::debug;

use zaparoo_core::media_types::{
    MediaMeta, MediaMetaParams, MediaMetaProperty, MediaMetaSystemRef, MediaMetaTitle, TagInfo,
};

use crate::media_db::{ReadDb, ReadDbSpec, MEDIA_DB_PATH};

static META_DB: OnceLock<Option<ReadDb>> = OnceLock::new();

static SPEC: ReadDbSpec = ReadDbSpec {
    domain: "media_meta_db",
    env_switch: "ZAPAROO_FRONTEND_DB_META",
    db_path: MEDIA_DB_PATH,
    required_tables: &[
        "Media",
        "MediaTitles",
        "Systems",
        "MediaTags",
        "MediaTitleTags",
        "Tags",
        "TagTypes",
        "MediaProperties",
        "MediaTitleProperties",
    ],
    uri: false,
    prepare: None,
    active_msg: "direct metadata active",
    fallback_msg: "metadata uses RPC only",
};

fn meta_db() -> Option<&'static ReadDb> {
    META_DB.get_or_init(|| ReadDb::init(&SPEC)).as_ref()
}

/// Whether the direct metadata layer initialized.
pub fn enabled() -> bool {
    meta_db().is_some()
}

/// Async wrapper for call sites living on the runtime: the lookup is a
/// few indexed point queries but still file IO, so it runs on the
/// blocking pool.
pub async fn media_meta_async(params: MediaMetaParams) -> Option<MediaMeta> {
    if !enabled() {
        return None;
    }
    tokio::task::spawn_blocking(move || media_meta(&params))
        .await
        .ok()
        .flatten()
}

/// Resolve one media row's metadata. Returns `None` whenever the answer
/// cannot be produced with full confidence (layer disabled, row not
/// found, query error) and the caller falls through to the RPC.
pub fn media_meta(params: &MediaMetaParams) -> Option<MediaMeta> {
    let db = meta_db()?;
    let started = Instant::now();
    let result = db.with_conn(
        || format!("meta lookup media_id={:?}", params.media_id),
        |conn| lookup_meta(conn, params),
    )??;
    debug!(
        lookup_ms = started.elapsed().as_millis(),
        "media_meta_db: metadata resolved",
    );
    Some(result)
}

fn lookup_meta(conn: &Connection, params: &MediaMetaParams) -> rusqlite::Result<Option<MediaMeta>> {
    // Keyed row: media id when the caller has one, else system + path —
    // the same resolution order the RPC accepts.
    let row = if let Some(media_id) = params.media_id {
        conn.query_row(
            "SELECT m.DBID, m.MediaTitleDBID, m.Path, m.ParentDir, m.IsMissing, s.SystemID \
             FROM Media m INNER JOIN Systems s ON s.DBID = m.SystemDBID \
             WHERE m.DBID = ?1",
            [media_id],
            map_media_row,
        )
        .optional()?
    } else if !params.system.is_empty() && !params.path.is_empty() {
        conn.query_row(
            "SELECT m.DBID, m.MediaTitleDBID, m.Path, m.ParentDir, m.IsMissing, s.SystemID \
             FROM Media m INNER JOIN Systems s ON s.DBID = m.SystemDBID \
             WHERE s.SystemID = ?1 AND m.Path = ?2",
            rusqlite::params![params.system, params.path],
            map_media_row,
        )
        .optional()?
    } else {
        None
    };
    let Some((media_dbid, title_dbid, path, parent_dir, is_missing, _system_id)) = row else {
        return Ok(None);
    };

    let tags = entity_tags(conn, "MediaTags", "MediaDBID", media_dbid)?;
    let properties = entity_properties(conn, "MediaProperties", "MediaDBID", media_dbid)?;
    let available_image_types = image_types_from(&properties);

    let title = conn
        .query_row(
            "SELECT t.Slug, t.SecondarySlug, t.Name, t.SlugLength, t.SlugWordCount, s.SystemID \
             FROM MediaTitles t INNER JOIN Systems s ON s.DBID = t.SystemDBID \
             WHERE t.DBID = ?1",
            [title_dbid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let title = match title {
        Some((slug, secondary_slug, name, slug_length, slug_word_count, system_id)) => {
            let title_tags = entity_tags(conn, "MediaTitleTags", "MediaTitleDBID", title_dbid)?;
            let title_properties =
                entity_properties(conn, "MediaTitleProperties", "MediaTitleDBID", title_dbid)?;
            let title_image_types = image_types_from(&title_properties);
            MediaMetaTitle {
                slug,
                secondary_slug,
                name,
                slug_length: u32::try_from(slug_length.max(0)).unwrap_or(0),
                slug_word_count: u32::try_from(slug_word_count.max(0)).unwrap_or(0),
                system: MediaMetaSystemRef {
                    id: system_id,
                    name: String::new(),
                },
                tags: title_tags,
                properties: title_properties,
                available_image_types: title_image_types,
            }
        }
        None => MediaMetaTitle::default(),
    };

    Ok(Some(MediaMeta {
        path,
        parent_dir,
        is_missing,
        tags,
        properties,
        available_image_types,
        title,
    }))
}

type MediaRow = (i64, i64, String, String, bool, String);

fn map_media_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

/// Tags attached to a media row or title, as wire `TagInfo` with the
/// label left empty (consumers fall back to the tag value).
fn entity_tags(
    conn: &Connection,
    join_table: &str,
    join_col: &str,
    dbid: i64,
) -> rusqlite::Result<Vec<TagInfo>> {
    let sql = format!(
        "SELECT tt.Type, t.Tag FROM {join_table} j \
         INNER JOIN Tags t ON t.DBID = j.TagDBID \
         INNER JOIN TagTypes tt ON tt.DBID = t.TypeDBID \
         WHERE j.{join_col} = ?1 ORDER BY t.DBID"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([dbid], |r| {
        Ok(TagInfo {
            tag_type: r.get(0)?,
            tag: r.get(1)?,
            label: String::new(),
        })
    })?;
    rows.collect()
}

/// Scraped properties for a media row or title, keyed the wire way:
/// `property:<tag>`. Extensions derive from the stored file path where
/// one exists; text-only properties keep `None`.
fn entity_properties(
    conn: &Connection,
    table: &str,
    join_col: &str,
    dbid: i64,
) -> rusqlite::Result<HashMap<String, MediaMetaProperty>> {
    let sql = format!(
        "SELECT t.Tag, p.Text FROM {table} p \
         INNER JOIN Tags t ON t.DBID = p.TypeTagDBID \
         INNER JOIN TagTypes tt ON tt.DBID = t.TypeDBID \
         WHERE p.{join_col} = ?1 AND tt.Type = 'property'"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([dbid], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (tag, text) = row?;
        let extension = extension_of(&text);
        out.insert(
            format!("property:{tag}"),
            MediaMetaProperty {
                text,
                content_type: String::new(),
                extension,
                blob_size: 0,
            },
        );
    }
    Ok(out)
}

/// Bare image types ("boxart", "screenshot", plain "image") derived
/// from the `image*` property tags, matching Core's wire form.
fn image_types_from(properties: &HashMap<String, MediaMetaProperty>) -> Vec<String> {
    let mut out: Vec<String> = properties
        .keys()
        .filter_map(|key| {
            let suffix = key.strip_prefix("property:image")?;
            if suffix.is_empty() {
                return Some("image".to_string());
            }
            let bare = suffix.trim_start_matches('-');
            (!bare.is_empty()).then(|| bare.to_string())
        })
        .collect();
    out.sort();
    out
}

/// Extension (no dot) of a file-path-shaped property value; `None` for
/// text-only values with no plausible extension.
fn extension_of(text: &str) -> Option<String> {
    let name = text.rsplit(['/', '\\']).next()?;
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.trim().to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(ext)
}

#[cfg(test)]
mod tests {
    use super::{extension_of, lookup_meta};
    use rusqlite::Connection;
    use zaparoo_core::media_types::MediaMetaParams;

    fn test_db() -> Connection {
        #[allow(clippy::expect_used, reason = "test setup")]
        let conn = Connection::open_in_memory().expect("open in-memory db");
        #[allow(clippy::expect_used, reason = "test setup")]
        conn.execute_batch(
            "CREATE TABLE Systems (DBID INTEGER PRIMARY KEY, SystemID TEXT NOT NULL);
             CREATE TABLE MediaTitles (
                 DBID INTEGER PRIMARY KEY, SystemDBID INTEGER NOT NULL,
                 Slug TEXT NOT NULL, Name TEXT NOT NULL, SecondarySlug TEXT,
                 SlugLength INTEGER DEFAULT 0, SlugWordCount INTEGER DEFAULT 0
             );
             CREATE TABLE Media (
                 DBID INTEGER PRIMARY KEY, MediaTitleDBID INTEGER NOT NULL,
                 SystemDBID INTEGER NOT NULL, Path TEXT NOT NULL,
                 ParentDir TEXT NOT NULL DEFAULT '', SortName TEXT NOT NULL DEFAULT '',
                 IsMissing INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE TagTypes (DBID INTEGER PRIMARY KEY, Type TEXT NOT NULL);
             CREATE TABLE Tags (DBID INTEGER PRIMARY KEY, TypeDBID INTEGER NOT NULL, Tag TEXT NOT NULL);
             CREATE TABLE MediaTags (MediaDBID INTEGER NOT NULL, TagDBID INTEGER NOT NULL);
             CREATE TABLE MediaTitleTags (MediaTitleDBID INTEGER NOT NULL, TagDBID INTEGER NOT NULL);
             CREATE TABLE MediaProperties (
                 DBID INTEGER PRIMARY KEY, MediaDBID INTEGER NOT NULL,
                 TypeTagDBID INTEGER NOT NULL, Text TEXT NOT NULL DEFAULT '', BlobDBID INTEGER
             );
             CREATE TABLE MediaTitleProperties (
                 DBID INTEGER PRIMARY KEY, MediaTitleDBID INTEGER NOT NULL,
                 TypeTagDBID INTEGER NOT NULL, Text TEXT NOT NULL DEFAULT '', BlobDBID INTEGER
             );
             INSERT INTO Systems VALUES (1,'C64');
             INSERT INTO MediaTitles VALUES (50,1,'beta','Beta','beta-alt',4,1);
             INSERT INTO Media VALUES (100,50,1,'/g/C64/Beta.crt','/g/C64/','Beta',0);
             INSERT INTO TagTypes VALUES (1,'user'),(2,'property'),(3,'region');
             INSERT INTO Tags VALUES
                 (10,1,'favorite'),(11,2,'image-boxart'),(12,2,'description'),
                 (13,3,'europe'),(14,2,'image-screenshot');
             INSERT INTO MediaTags VALUES (100,10),(100,13);
             INSERT INTO MediaTitleTags VALUES (50,13);
             INSERT INTO MediaProperties VALUES (1,100,11,'/art/beta-box.png',NULL);
             INSERT INTO MediaTitleProperties VALUES
                 (1,50,12,'A fine game.',NULL),(2,50,14,'/art/beta-shot.jpg',NULL);",
        )
        .expect("seed schema");
        conn
    }

    #[test]
    fn resolves_by_system_and_path_with_full_shape() {
        let conn = test_db();
        #[allow(clippy::expect_used, reason = "test assertion")]
        let meta = lookup_meta(&conn, &MediaMetaParams::for_media("C64", "/g/C64/Beta.crt"))
            .expect("query ok")
            .expect("resolved");
        assert_eq!(meta.path, "/g/C64/Beta.crt");
        assert_eq!(meta.parent_dir, "/g/C64/");
        assert!(!meta.is_missing);
        assert!(meta
            .tags
            .iter()
            .any(|t| t.tag == "favorite" && t.tag_type == "user"));
        assert!(meta
            .tags
            .iter()
            .any(|t| t.tag == "europe" && t.tag_type == "region"));
        let boxart = &meta.properties["property:image-boxart"];
        assert_eq!(boxart.extension.as_deref(), Some("png"));
        assert_eq!(meta.available_image_types, ["boxart"]);
        assert_eq!(meta.title.name, "Beta");
        assert_eq!(meta.title.slug, "beta");
        assert_eq!(meta.title.secondary_slug.as_deref(), Some("beta-alt"));
        assert_eq!(meta.title.system.id, "C64");
        assert_eq!(
            meta.title.properties["property:description"].text,
            "A fine game."
        );
        assert_eq!(meta.title.available_image_types, ["screenshot"]);
        assert!(meta.title.tags.iter().any(|t| t.tag == "europe"));
    }

    #[test]
    fn resolves_by_media_id() {
        let conn = test_db();
        let params = MediaMetaParams {
            media_id: Some(100),
            ..MediaMetaParams::default()
        };
        #[allow(clippy::expect_used, reason = "test assertion")]
        let meta = lookup_meta(&conn, &params)
            .expect("query ok")
            .expect("resolved");
        assert_eq!(meta.title.slug, "beta");
    }

    #[test]
    #[allow(clippy::expect_used, reason = "test assertion")]
    fn unknown_row_defers_to_rpc() {
        let conn = test_db();
        assert!(
            lookup_meta(&conn, &MediaMetaParams::for_media("C64", "/nope"))
                .expect("query ok")
                .is_none()
        );
    }

    #[test]
    #[allow(clippy::expect_used, reason = "test assertion")]
    fn extension_heuristic_rejects_prose() {
        assert_eq!(extension_of("/a/b.png").as_deref(), Some("png"));
        assert_eq!(extension_of("A fine game. Really"), None);
        assert_eq!(extension_of("noext"), None);
    }
}
