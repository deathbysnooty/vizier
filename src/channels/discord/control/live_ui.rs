//! The live page's own files: the page from disk when there is one, and the
//! art, the models and the textures from disk only.
//!
//! The Hall of Dragons is one HTML file and about a dozen megabytes of pictures,
//! models and baked textures. The HTML is built into the binary with
//! `include_str!`, exactly as the panel's pages are (see [`super::ui`]), so a
//! release always carries a complete page; the heavy files are NOT, because a
//! binary is a bad place to keep two megabytes of dragon.
//!
//! So both are looked for in `<workspace>/.runtime/live/` — the same runtime
//! directory control.db, the message log and the panel's `ui` directory live
//! in — and the built-in copy of the page is used when the directory has
//! nothing to say. Dropping an edited `index.html` in there changes the page
//! within a second or two, with no rebuild and no restart
//! (`scripts/push-live.sh`), and an empty or half-written file falls back to
//! the built-in one rather than blanking the page.
//!
//! WHAT MAY BE SERVED. Only the extensions the page actually asks for, and only
//! plain path components: no `..`, no absolute path, no hidden file, nothing
//! deeper than four levels. The runtime directory is a working directory with a
//! database or two in it, so an allowed list of extensions is the floor, not the
//! path check.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

/// The page itself, built into the binary.
pub const INDEX_HTML: &str = include_str!("live/index.html");

/// The file name the page is served under, in the runtime directory and here.
pub const PAGE: &str = "index.html";

/// The biggest file we will read into memory to answer one request. The dragon
/// is under two megabytes and the largest texture under one; this is a ceiling
/// against somebody dropping a video in the directory, not a budget.
const MOST: u64 = 24 * 1024 * 1024;

/// What the page is allowed to ask for, and what we say it is.
const KINDS: &[(&str, &str)] = &[
    ("css", "text/css; charset=utf-8"),
    ("glb", "model/gltf-binary"),
    ("gltf", "model/gltf+json"),
    ("hdr", "image/vnd.radiance"),
    ("html", "text/html; charset=utf-8"),
    ("ico", "image/x-icon"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("js", "text/javascript; charset=utf-8"),
    ("json", "application/json"),
    ("ktx2", "image/ktx2"),
    ("mp3", "audio/mpeg"),
    ("png", "image/png"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
    ("woff2", "font/woff2"),
];

/// Where the files are, or none before [`open`] (in a test, say).
static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
/// The page's text, through the same small cache the panel's pages use.
static PAGE_FILE: OnceLock<super::ui::Files> = OnceLock::new();

/// Point the live page at `<workspace>/.runtime/live` and say in the log what
/// is there, so a deploy can be checked. The directory need not exist: one
/// pushed later is picked up without a restart.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &[".runtime", "live"]);
    let exists = dir.is_dir();
    if DIR.set(Some(dir.clone())).is_err() {
        tracing::warn!("live: directory already set; leaving it alone");
        return;
    }
    let _ = PAGE_FILE.set(super::ui::Files::new(Some(dir.clone())));
    if !exists {
        tracing::info!(
            "live: no page directory at {}; serving the built-in page with no art. Push one with scripts/push-live.sh",
            dir.display()
        );
        return;
    }
    let own = match std::fs::read(dir.join(PAGE)) {
        Ok(bytes) if !bytes.is_empty() => " and its own index.html",
        _ => " (built-in index.html)",
    };
    tracing::info!("live: serving the page's files from {}{}", dir.display(), own);
}

/// Where the files are. None until [`open`], and none in a test.
pub fn dir() -> Option<&'static PathBuf> {
    DIR.get().and_then(|d| d.as_ref())
}

/// The page: the copy in the runtime directory if there is a usable one, and
/// the one built into the binary otherwise.
pub fn page() -> Cow<'static, str> {
    match PAGE_FILE.get() {
        Some(files) => files.get(PAGE, INDEX_HTML, Instant::now()),
        None => Cow::Borrowed(INDEX_HTML),
    }
}

/// What a file of this name is, if we serve that sort of file at all.
pub fn kind(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    KINDS.iter().find(|(name, _)| *name == ext).map(|(_, kind)| *kind)
}

/// `path` as a relative path under the live directory, or none when it is not
/// one plain path: every component a plain name, nothing hidden, nothing that
/// climbs out, and an extension we are willing to serve.
pub fn plain(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.len() > 200 || path.contains('\\') || path.contains('\0') || path.contains("//") {
        return None;
    }
    kind(path)?;
    let mut out = PathBuf::new();
    let mut depth = 0;
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.starts_with('.') {
            return None;
        }
        if !part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
            return None;
        }
        depth += 1;
        if depth > 4 {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// One of the page's files from the runtime directory: what it is, and its
/// bytes. None when there is no directory, the name is not one we would open,
/// or nothing is there under it.
pub fn asset(path: &str) -> Option<(&'static str, Vec<u8>)> {
    asset_in(dir()?, path)
}

/// The same, from a directory handed in, so a test needs no global state.
fn asset_in(dir: &std::path::Path, path: &str) -> Option<(&'static str, Vec<u8>)> {
    let kind = kind(path)?;
    let full = dir.join(plain(path)?);
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MOST {
        if meta.len() > MOST {
            tracing::warn!("live: {} is {} bytes; too big to serve", full.display(), meta.len());
        }
        return None;
    }
    let bytes = std::fs::read(&full).ok()?;
    Some((kind, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_page_is_the_hall() {
        assert!(INDEX_HTML.contains("The Hall of Dragons"), "the page built into the binary is the hall");
        assert!(INDEX_HTML.len() > 20_000, "and the whole of it, not a stub");
    }

    #[test]
    fn only_the_pages_own_sort_of_file_is_served() {
        for name in ["index.html", "models-v5.js", "eggs/egg-1.png", "dragon.glb", "guide/hatch.png", "a/b/c/d.png"] {
            assert!(plain(name).is_some(), "{} is one of the page's files", name);
            assert!(kind(name).is_some(), "{} has a type", name);
        }
        // The runtime directory is a working directory: nothing else in it is
        // the page's business, whatever the path looks like.
        for name in [
            "control.db",
            "eggs.db",
            "house.db-wal",
            "notes.txt",
            "secrets.env",
            "ui/app.js.map",
            "a/b/c/d/e.png",
            "",
        ] {
            assert!(plain(name).is_none(), "{} must not be served", name);
        }
    }

    #[test]
    fn a_name_that_leaves_the_directory_is_refused() {
        for name in [
            "../control.db",
            "../../etc/passwd.png",
            "eggs/../../control.db",
            "/etc/passwd.png",
            "..\\control.db",
            "./egg.png",
            ".hidden.png",
            "eggs//egg-1.png",
            "eggs/.hidden/egg-1.png",
            "egg%2e%2e.png",
        ] {
            assert!(plain(name).is_none(), "{} must never be opened", name);
        }
    }

    #[test]
    fn art_comes_off_the_disk_and_a_missing_file_is_simply_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("eggs")).unwrap();
        std::fs::write(dir.path().join("eggs/egg-1.png"), b"\x89PNG not really").unwrap();
        std::fs::write(dir.path().join("eggs/egg-2.png"), b"").unwrap();
        std::fs::write(dir.path().join("control.db"), b"not the page's business").unwrap();

        let (kind, bytes) = asset_in(dir.path(), "eggs/egg-1.png").expect("the egg");
        assert_eq!(kind, "image/png");
        assert_eq!(bytes, b"\x89PNG not really");
        assert!(asset_in(dir.path(), "eggs/egg-5.png").is_none(), "nothing there is nothing, not an error");
        assert!(asset_in(dir.path(), "eggs/egg-2.png").is_none(), "a half-written file is not served");
        assert!(asset_in(dir.path(), "control.db").is_none(), "and the databases beside it are never served");
        assert!(asset_in(dir.path(), "../control.db").is_none());
    }

    #[test]
    fn the_page_itself_comes_off_the_disk_when_there_is_one() {
        let dir = tempfile::tempdir().unwrap();
        let files = super::super::ui::Files::new(Some(dir.path().to_path_buf()));
        // Nothing there: the built-in page, which is never a blank page.
        assert_eq!(files.get(PAGE, INDEX_HTML, Instant::now()), INDEX_HTML);
        std::fs::write(dir.path().join(PAGE), "<!doctype html><title>edited</title>").unwrap();
        // Far enough on that the cache looks at the disk again.
        let later = Instant::now() + std::time::Duration::from_secs(2);
        assert!(files.get(PAGE, INDEX_HTML, later).contains("edited"), "an edit needs no release");
    }
}
