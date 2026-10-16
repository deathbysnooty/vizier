//! The GIFs and the lines the gestures are made of.
//!
//! Plain files in `<workspace>/lovebank/`, one per gesture, the way the
//! Chocolate Frog reads its riddles from `riddlebank/`:
//!
//! * `hug.txt`, `kiss.txt`, `cuddle.txt`, `flirt.txt`, `gift.txt`,
//!   `jealous.txt`, `papers.txt`, `wedding.txt` — one URL per line.
//! * `flirt-lines.txt` — one line of text per line, for the gesture that needs
//!   words as well as a picture.
//!
//! Why files rather than a list in the code: the tone of a hug GIF matters more
//! than anything a program would choose, and the owner should be able to swap
//! the whole set for a different anime in five minutes. These are rsynced like
//! the other banks, so changing them needs no build and no restart beyond the
//! one that reads them.
//!
//! A missing file is not an error. The gesture still works and simply posts no
//! picture - said once in the log at startup, so somebody can see why.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// Every gesture that draws on the bank.
pub const KINDS: [&str; 8] = ["hug", "kiss", "cuddle", "flirt", "gift", "jealous", "papers", "wedding"];

static GIFS: OnceLock<HashMap<String, Vec<String>>> = OnceLock::new();
static LINES: OnceLock<Vec<String>> = OnceLock::new();

/// Reads the bank once, at startup.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["lovebank"]);
    let mut found: HashMap<String, Vec<String>> = HashMap::new();
    for kind in KINDS {
        let urls = read_lines(&dir.join(format!("{}.txt", kind)))
            .into_iter()
            .filter(|line| line.starts_with("http"))
            .collect::<Vec<_>>();
        if !urls.is_empty() {
            found.insert(kind.to_string(), urls);
        }
    }
    let lines = read_lines(&dir.join("flirt-lines.txt"));
    if found.is_empty() && lines.is_empty() {
        tracing::info!("love: no bank at {} - the gestures will post words without pictures", dir.display());
    } else {
        tracing::info!(
            "love: bank from {} - {} with pictures, {} flirt lines",
            dir.display(),
            found.len(),
            lines.len()
        );
    }
    let _ = GIFS.set(found);
    let _ = LINES.set(lines);
}

/// One file as its non-empty, non-comment lines.
fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|body| {
            body.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A picture for a gesture, picked at random. `None` where the bank has none,
/// which is not an error: the gesture posts its words and nothing else.
pub fn gif(kind: &str, roll: u64) -> Option<String> {
    let all = GIFS.get()?.get(kind)?;
    pick(all, roll).cloned()
}

/// A flirt line, picked at random.
pub fn flirt_line(roll: u64) -> Option<String> {
    pick(LINES.get()?, roll).cloned()
}

/// How many pictures a gesture has, for the panel to show.
pub fn counts() -> Vec<(&'static str, usize)> {
    let have = GIFS.get();
    KINDS.iter().map(|k| (*k, have.and_then(|m| m.get(*k)).map(Vec::len).unwrap_or(0))).collect()
}

fn pick<T>(all: &[T], roll: u64) -> Option<&T> {
    if all.is_empty() {
        return None;
    }
    all.get((roll % all.len() as u64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bank_file_drops_blanks_and_comments_and_keeps_order() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("hug.txt");
        std::fs::write(&path, "# anime hugs\n\nhttps://one.gif\n  https://two.gif  \n\n# end\n").expect("write");
        assert_eq!(read_lines(&path), vec!["https://one.gif", "https://two.gif"]);
    }

    #[test]
    fn a_missing_file_is_empty_rather_than_an_error() {
        assert!(read_lines(Path::new("/nowhere/at/all/hug.txt")).is_empty());
    }

    #[test]
    fn picking_covers_the_whole_list_and_never_runs_off_the_end() {
        let all = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let seen: std::collections::HashSet<&String> = (0..30).filter_map(|r| pick(&all, r)).collect();
        assert_eq!(seen.len(), 3, "every picture is reachable");
        assert!(pick::<String>(&[], 7).is_none(), "an empty bank picks nothing");
    }
}
