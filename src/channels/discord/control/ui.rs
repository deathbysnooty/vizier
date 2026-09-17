//! The panel's own files: from disk when they are there, from the binary
//! otherwise.
//!
//! Every page the panel serves (`index.html`, `app.js`, the sudoku, chess and
//! Letter Duel pages) is built into the binary with `include_str!`, so a release always has
//! a complete panel. That also meant a one-line change to a stylesheet needed a
//! full release and a restart. So each of those files is looked for first in
//! `<workspace>/.runtime/ui/` — the same runtime directory control.db and the
//! message log live in — and the built-in copy is used when it isn't there.
//!
//! Dropping the files in that directory (see `scripts/push-ui.sh`) therefore
//! changes the panel within a second or two, with no rebuild and no restart.
//! The built-in copy stays the floor: a missing, unreadable or EMPTY file falls
//! back to it, so a half-written file can never blank the panel.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;

/// Every file the panel may serve from disk. The list is what startup reports
/// and what the push script copies; the tests keep it level with the `ui`
/// directory in the source tree.
pub const NAMES: &[&str] = &[
    "app.css",
    "app.js",
    "chess-watch.html",
    "chess-watch.js",
    "chess.html",
    "chess.js",
    "duel.html",
    "duel.js",
    "favicon.svg",
    "index.html",
    "puzzle.html",
    "puzzle.js",
    "sudoku.css",
    "sudoku.html",
    "sudoku.js",
];

/// How long a file is served without looking at the disk again. Short enough
/// that an edit shows up while you are still reaching for the reload key, long
/// enough that a busy page is not a string of `stat` calls.
const RECHECK: Duration = Duration::from_millis(500);

/// What one file looked like last time it was looked at.
struct Entry {
    /// Modified time and length: an edit changes at least one of them.
    stamp: Option<(SystemTime, u64)>,
    /// The file's text, or none when the built-in copy is to be used.
    text: Option<Arc<str>>,
    /// When the disk was last consulted.
    checked: Instant,
}

/// A directory of panel files, read through a small cache.
pub struct Files {
    /// Where to look, or none to always use the built-in copies.
    dir: Option<PathBuf>,
    cache: Mutex<HashMap<String, Entry>>,
}

/// A name we are willing to open: one plain file name, no way out of the
/// directory. Every call site passes a literal, but a name is a name.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name != ".."
        && name != "."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// The file's text, if it is there and has something in it.
fn read(path: &std::path::Path) -> Option<(SystemTime, u64, String)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    let stamp = (meta.modified().ok()?, meta.len());
    let text = std::fs::read_to_string(path).ok()?;
    // A file still being written can be there and yet say nothing; the built-in
    // copy is better than a blank panel.
    if text.trim().is_empty() {
        return None;
    }
    Some((stamp.0, stamp.1, text))
}

impl Files {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, cache: Mutex::new(HashMap::new()) }
    }

    /// The named file: the copy on disk if there is a usable one, otherwise
    /// `embedded`. `now` is the clock, so a test can move time on.
    pub fn get(&self, name: &str, embedded: &'static str, now: Instant) -> Cow<'static, str> {
        let Some(dir) = self.dir.as_ref() else { return Cow::Borrowed(embedded) };
        if !plain(name) {
            tracing::warn!("panel: {:?} is not a ui file name; using the built-in copy", name);
            return Cow::Borrowed(embedded);
        }
        let mut cache = self.cache.lock();
        // Seen recently enough: hand back what we have without touching the disk.
        if let Some(entry) = cache.get(name)
            && now.saturating_duration_since(entry.checked) < RECHECK
        {
            return pick(entry.text.as_ref(), embedded);
        }
        let found = read(&dir.join(name));
        let stamp = found.as_ref().map(|(m, l, _)| (*m, *l));
        // Unchanged since the last read: keep the text, just note the look.
        if let Some(entry) = cache.get_mut(name)
            && entry.stamp == stamp
        {
            entry.checked = now;
            return pick(entry.text.as_ref(), embedded);
        }
        let text = found.map(|(_, _, t)| Arc::<str>::from(t));
        let entry = cache.entry(name.to_string()).or_insert(Entry { stamp, text: None, checked: now });
        entry.stamp = stamp;
        entry.text = text;
        entry.checked = now;
        pick(entry.text.as_ref(), embedded)
    }

    /// The names this directory is actually serving right now.
    fn present(&self) -> Vec<&'static str> {
        let Some(dir) = self.dir.as_ref() else { return Vec::new() };
        NAMES.iter().copied().filter(|name| read(&dir.join(name)).is_some()).collect()
    }
}

fn pick(text: Option<&Arc<str>>, embedded: &'static str) -> Cow<'static, str> {
    match text {
        Some(text) => Cow::Owned(text.to_string()),
        None => Cow::Borrowed(embedded),
    }
}

static FILES: OnceLock<Files> = OnceLock::new();

fn files() -> &'static Files {
    // Before `open` — in a test, say — there is no directory and every file is
    // the built-in one.
    FILES.get_or_init(|| Files::new(None))
}

/// Point the panel at `<workspace>/.runtime/ui` and say in the log what is
/// being served from there, so a deploy can be checked. The directory need not
/// exist yet: one pushed later is picked up without a restart.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &[".runtime", "ui"]);
    let exists = dir.is_dir();
    if FILES.set(Files::new(Some(dir.clone()))).is_err() {
        tracing::warn!("panel: ui directory already set; leaving it alone");
        return;
    }
    if !exists {
        tracing::info!("panel: no ui directory at {}; serving the built-in files", dir.display());
        return;
    }
    let present = files().present();
    if present.is_empty() {
        tracing::info!("panel: ui directory {} holds no panel files; serving the built-in ones", dir.display());
    } else {
        tracing::info!(
            "panel: serving {} ui files from disk ({}) in {}",
            present.len(),
            present.join(", "),
            dir.display()
        );
    }
}

/// The named panel file, from disk when there is one. `embedded` is the copy
/// built into the binary and is what comes back otherwise.
pub fn file(name: &str, embedded: &'static str) -> Cow<'static, str> {
    files().get(name, embedded, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILT_IN: &str = "built-in";

    fn at(dir: &std::path::Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    /// Far enough on that the cache looks at the disk again.
    fn later(now: Instant) -> Instant {
        now + RECHECK + Duration::from_millis(1)
    }

    #[test]
    fn disk_copy_wins() {
        let dir = tempfile::tempdir().unwrap();
        at(dir.path(), "app.js", "from disk");
        let files = Files::new(Some(dir.path().to_path_buf()));
        assert_eq!(files.get("app.js", BUILT_IN, Instant::now()), "from disk");
    }

    #[test]
    fn missing_file_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let files = Files::new(Some(dir.path().to_path_buf()));
        assert_eq!(files.get("app.js", BUILT_IN, Instant::now()), BUILT_IN);
    }

    #[test]
    fn empty_file_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        at(dir.path(), "app.js", "");
        let files = Files::new(Some(dir.path().to_path_buf()));
        let now = Instant::now();
        assert_eq!(files.get("app.js", BUILT_IN, now), BUILT_IN, "an empty file must never blank the panel");
        // Nor a file that is only whitespace, as a half-written one can be.
        at(dir.path(), "app.js", "  \n ");
        assert_eq!(files.get("app.js", BUILT_IN, later(now)), BUILT_IN);
    }

    #[test]
    fn an_edit_is_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        at(dir.path(), "app.css", "first");
        let files = Files::new(Some(dir.path().to_path_buf()));
        let now = Instant::now();
        assert_eq!(files.get("app.css", BUILT_IN, now), "first");
        at(dir.path(), "app.css", "second edition");
        assert_eq!(files.get("app.css", BUILT_IN, later(now)), "second edition");
        // And a file taken away goes back to the built-in copy.
        std::fs::remove_file(dir.path().join("app.css")).unwrap();
        assert_eq!(files.get("app.css", BUILT_IN, later(later(now))), BUILT_IN);
    }

    #[test]
    fn a_fresh_read_is_not_done_every_time() {
        let dir = tempfile::tempdir().unwrap();
        at(dir.path(), "app.css", "first");
        let files = Files::new(Some(dir.path().to_path_buf()));
        let now = Instant::now();
        assert_eq!(files.get("app.css", BUILT_IN, now), "first");
        at(dir.path(), "app.css", "second edition");
        // Within the window the cached text is served: no disk read per request.
        assert_eq!(files.get("app.css", BUILT_IN, now + Duration::from_millis(1)), "first");
    }

    #[test]
    fn a_name_that_leaves_the_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.js");
        std::fs::write(&outside, "not ours").unwrap();
        let inner = dir.path().join("ui");
        std::fs::create_dir(&inner).unwrap();
        at(&inner, "app.js", "ours");
        let files = Files::new(Some(inner.clone()));
        let now = Instant::now();
        for name in ["../outside.js", "..", ".", "sub/app.js", "..\\outside.js", ""] {
            assert_eq!(files.get(name, BUILT_IN, now), BUILT_IN, "{name} must not be opened");
        }
        assert_eq!(files.get("app.js", BUILT_IN, now), "ours", "a plain name still works");
    }

    #[test]
    fn no_directory_behaves_as_before() {
        let none = Files::new(None);
        assert_eq!(none.get("app.js", BUILT_IN, Instant::now()), BUILT_IN);
        assert!(matches!(none.get("app.js", BUILT_IN, Instant::now()), Cow::Borrowed(_)), "no copying either");
        assert!(none.present().is_empty());

        // A directory that isn't there is the same thing, and may appear later.
        let gone = Files::new(Some(PathBuf::from("/nowhere/at/all/ui")));
        assert_eq!(gone.get("app.js", BUILT_IN, Instant::now()), BUILT_IN);
        assert!(gone.present().is_empty());
    }

    #[test]
    fn present_lists_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        at(dir.path(), "app.js", "js");
        at(dir.path(), "index.html", "html");
        at(dir.path(), "app.css", "");
        at(dir.path(), "notes.txt", "not a panel file");
        let files = Files::new(Some(dir.path().to_path_buf()));
        assert_eq!(files.present(), vec!["app.js", "index.html"]);
    }

    #[test]
    fn names_match_the_files_built_into_the_binary() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/channels/discord/control/ui");
        let mut on_disk: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        on_disk.sort();
        on_disk.retain(|n| !n.starts_with('.'));
        assert_eq!(on_disk, NAMES, "a new panel file must be listed in NAMES so it can be served from disk");
    }
}
