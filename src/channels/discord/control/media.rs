//! Pictures for scheduled posts, uploaded on the panel and kept in control.db so
//! a post can attach them without depending on an outside link staying up.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::DB;

/// Largest picture accepted.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Most pictures the library holds.
pub const MAX_FILES: i64 = 100;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MediaInfo {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub created_ts: i64,
    /// Discord ids travel as strings.
    pub created_by: String,
}

impl MediaInfo {
    /// The file name Discord gets: simple, unique, with the right extension, so
    /// an embed can point at it as `attachment://<name>`.
    pub fn attachment_name(&self) -> String {
        format!("{}.{}", self.id, extension(&self.mime))
    }
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS media (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, mime TEXT NOT NULL, bytes BLOB NOT NULL,
        created_ts INTEGER NOT NULL, created_by INTEGER NOT NULL);
";

pub(super) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// What the bytes really are, from their first bytes: only PNG, JPEG, GIF and
/// WebP pass, whatever the file claims to be.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// A display name kept to letters, digits and a little punctuation.
fn clean_name(raw: &str, mime: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    let mut out: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '-' })
        .take(60)
        .collect::<String>()
        .trim()
        .trim_matches('-')
        .to_string();
    if out.is_empty() {
        out = "picture".to_string();
    }
    format!("{}.{}", out, extension(mime))
}

/// Checks and stores a picture; the error says what to fix.
pub fn save(name: &str, bytes: &[u8], by: u64) -> Result<MediaInfo, String> {
    if bytes.is_empty() {
        return Err("That file is empty.".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("Pictures can be at most {} MB.", MAX_BYTES / (1024 * 1024)));
    }
    let mime = sniff(bytes).ok_or("Only PNG, JPG, GIF and WebP pictures can be used.")?;
    let db = DB.get().ok_or("The picture library isn't available right now.")?;
    let conn = db.lock();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if count >= MAX_FILES {
        return Err(format!("The library already has {} pictures. Delete some you no longer use first.", MAX_FILES));
    }
    let info = MediaInfo {
        id: rand::random::<[u8; 8]>().iter().map(|b| format!("{:02x}", b)).collect(),
        name: clean_name(name, mime),
        mime: mime.to_string(),
        size: bytes.len() as i64,
        created_ts: Utc::now().timestamp(),
        created_by: by.to_string(),
    };
    conn.execute(
        "INSERT INTO media (id, name, mime, bytes, created_ts, created_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![info.id, info.name, info.mime, bytes, info.created_ts, by as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(info)
}

fn info_row(r: &rusqlite::Row) -> rusqlite::Result<MediaInfo> {
    Ok(MediaInfo {
        id: r.get(0)?,
        name: r.get(1)?,
        mime: r.get(2)?,
        size: r.get(3)?,
        created_ts: r.get(4)?,
        created_by: r.get::<_, i64>(5)?.to_string(),
    })
}

/// Every picture, newest first, without the bytes.
pub fn list() -> Vec<MediaInfo> {
    let Some(db) = DB.get() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) =
        conn.prepare("SELECT id, name, mime, length(bytes), created_ts, created_by FROM media ORDER BY created_ts DESC, id")
    else {
        return Vec::new();
    };
    stmt.query_map([], info_row).map(|r| r.flatten().collect()).unwrap_or_default()
}

pub fn info(id: &str) -> Option<MediaInfo> {
    let db = DB.get()?;
    db.lock()
        .query_row("SELECT id, name, mime, length(bytes), created_ts, created_by FROM media WHERE id = ?1", params![id], info_row)
        .optional()
        .ok()
        .flatten()
}

/// A picture with its bytes.
pub fn get(id: &str) -> Option<(MediaInfo, Vec<u8>)> {
    let db = DB.get()?;
    db.lock()
        .query_row(
            "SELECT id, name, mime, length(bytes), created_ts, created_by, bytes FROM media WHERE id = ?1",
            params![id],
            |r| Ok((info_row(r)?, r.get::<_, Vec<u8>>(6)?)),
        )
        .optional()
        .ok()
        .flatten()
}

pub fn delete(id: &str) -> bool {
    DB.get().is_some_and(|db| db.lock().execute("DELETE FROM media WHERE id = ?1", params![id]).unwrap_or(0) > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_real_pictures_pass_the_sniff() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n rest"), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None, "SVG can carry script");
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff(b"GIF8"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn names_are_tidied_and_take_the_real_extension() {
        assert_eq!(clean_name("Good Morning!!.PNG", "image/png"), "Good Morning.png");
        assert_eq!(clean_name("../../etc/passwd.gif", "image/jpeg"), "passwd.jpg");
        assert_eq!(clean_name("", "image/webp"), "picture.webp");
        assert_eq!(clean_name("<script>.gif", "image/gif"), "script.gif");
    }
}
