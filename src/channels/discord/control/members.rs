//! Mods' private notes about members, written on the panel and handed to the AI
//! whenever it answers that member or a message about them, so it knows who it
//! is talking to and how far it can go with them.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::DB;

/// How the bot should treat someone, as a short instruction for the AI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// No special instruction beyond the notes.
    #[default]
    Normal,
    /// Warm and kind, no roasting at all.
    Gentle,
    /// Friendly teasing is fine, nothing harsh.
    LightRoast,
    /// They enjoy being roasted: go for it (still nothing hurtful or personal).
    Roast,
    /// Polite and respectful, no jokes at their expense.
    Respectful,
    /// Keep replies to them short and don't engage much.
    Brief,
}

impl Tone {
    /// The instruction the AI reads.
    pub fn instruction(self) -> Option<&'static str> {
        match self {
            Tone::Normal => None,
            Tone::Gentle => Some("be warm and kind with them, never roast them"),
            Tone::LightRoast => Some("light friendly teasing is fine, nothing harsh"),
            Tone::Roast => Some("they enjoy being roasted, so roast freely - but nothing hurtful, personal or about looks, family, caste or faith"),
            Tone::Respectful => Some("stay polite and respectful, no jokes at their expense"),
            Tone::Brief => Some("keep replies to them short and don't engage much"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberNote {
    /// Discord ids travel as strings.
    pub user_id: String,
    /// The name they had when the note was saved, for lists.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub tone: Tone,
    /// Free text: who they are, what they like, running jokes, things to avoid.
    #[serde(default)]
    pub notes: String,
    /// Off keeps the note on the panel only.
    #[serde(default = "yes")]
    pub use_in_replies: bool,
    #[serde(default)]
    pub updated_ts: i64,
    #[serde(default)]
    pub updated_by: String,
    /// Who wrote it: a mod, the analysis auto-fill, or a mod editing an auto-filled note.
    #[serde(default)]
    pub source: NoteSource,
    /// Whether a mod has looked at it. Notes from before this existed were
    /// written by mods, so they count as reviewed.
    #[serde(default = "yes")]
    pub reviewed: bool,
    /// When the analysis filled it in, if it did.
    #[serde(default)]
    pub filled_ts: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NoteSource {
    #[default]
    Manual,
    Analysis,
    /// Filled in by the analysis, then changed by a mod.
    Edited,
}

/// The audit trail's author for notes the analysis writes by itself.
pub const AUTO_FILL_BY: u64 = 0;

impl MemberNote {
    /// An empty note for a member, as a mod would start it.
    pub fn blank(user_id: u64, name: &str) -> MemberNote {
        MemberNote {
            user_id: user_id.to_string(),
            name: name.to_string(),
            tone: Tone::Normal,
            notes: String::new(),
            use_in_replies: true,
            updated_ts: 0,
            updated_by: String::new(),
            source: NoteSource::Manual,
            reviewed: true,
            filled_ts: 0,
        }
    }

    /// Auto-filled and not touched by a mod since: the analysis may write it again.
    pub fn is_untouched_auto_fill(&self) -> bool {
        self.source == NoteSource::Analysis && !self.reviewed && self.updated_by == AUTO_FILL_BY.to_string()
    }

    /// Filled in by the analysis and still waiting for a mod to switch it on.
    pub fn awaits_review(&self) -> bool {
        self.source == NoteSource::Analysis && !self.reviewed && !self.use_in_replies
    }

    /// The source a note gets when a mod saves it.
    pub fn marked_by_mod(mut self) -> MemberNote {
        if self.source == NoteSource::Analysis {
            self.source = NoteSource::Edited;
        }
        self.reviewed = true;
        self
    }
}

fn yes() -> bool {
    true
}

/// Longest note the AI is given, so one member can't flood every prompt.
pub const MAX_NOTE_CHARS: usize = 1000;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS member_notes (
        user_id INTEGER PRIMARY KEY, body TEXT NOT NULL, updated_by INTEGER NOT NULL, updated_ts INTEGER NOT NULL);
";

pub(super) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

pub fn get(user: u64) -> Option<MemberNote> {
    let db = DB.get()?;
    let body: String = db
        .lock()
        .query_row("SELECT body FROM member_notes WHERE user_id = ?1", params![user as i64], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str(&body).ok()
}

/// Everyone with a note, most recently changed first.
pub fn list() -> Vec<MemberNote> {
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT body FROM member_notes ORDER BY updated_ts DESC") else {
        return Vec::new();
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().filter_map(|b| serde_json::from_str(&b).ok()).collect())
        .unwrap_or_default()
}

pub fn validate(note: &MemberNote) -> Result<u64, String> {
    let user = note.user_id.trim().parse::<u64>().map_err(|_| "Pick a member.".to_string())?;
    if note.notes.chars().count() > MAX_NOTE_CHARS {
        return Err(format!("Keep notes under {} characters.", MAX_NOTE_CHARS));
    }
    Ok(user)
}

/// Saves or replaces the note for a member; the audit trail gets `member:<id>`.
pub fn save(note: &MemberNote, by: u64) -> anyhow::Result<MemberNote> {
    let user = validate(note).map_err(|e| anyhow::anyhow!(e))?;
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let now = Utc::now().timestamp();
    let stored = MemberNote {
        user_id: user.to_string(),
        notes: note.notes.trim().to_string(),
        updated_ts: now,
        updated_by: by.to_string(),
        ..note.clone()
    };
    let body = serde_json::to_string(&stored)?;
    let conn = db.lock();
    let old: Option<String> = conn
        .query_row("SELECT body FROM member_notes WHERE user_id = ?1", params![user as i64], |r| r.get(0))
        .optional()?;
    conn.execute(
        "INSERT INTO member_notes (user_id, body, updated_by, updated_ts) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id) DO UPDATE SET body = excluded.body, updated_by = excluded.updated_by,
         updated_ts = excluded.updated_ts",
        params![user as i64, body, by as i64, now],
    )?;
    conn.execute(
        "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now, by as i64, format!("member:{}", user), old, body],
    )?;
    Ok(stored)
}

pub fn delete(user: u64, by: u64) -> anyhow::Result<()> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let conn = db.lock();
    let old: Option<String> = conn
        .query_row("SELECT body FROM member_notes WHERE user_id = ?1", params![user as i64], |r| r.get(0))
        .optional()?;
    conn.execute("DELETE FROM member_notes WHERE user_id = ?1", params![user as i64])?;
    conn.execute(
        "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, NULL)",
        params![Utc::now().timestamp(), by as i64, format!("member:{}", user), old],
    )?;
    Ok(())
}

/// The block put in front of a message the AI will answer: the notes for the
/// people involved (`(id, display name)`, author first), or `None` when nobody
/// has one. It tells the AI the notes are private.
pub fn context_block(people: &[(u64, String)]) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    let lines: Vec<String> = people
        .iter()
        .filter(|(id, _)| seen.insert(*id))
        .filter_map(|(id, name)| {
            let note = get(*id).filter(|n| n.use_in_replies)?;
            line_for(&note, name)
        })
        .collect();
    block(&lines)
}

/// What the AI would be given for this note on its own, whether or not it is
/// saved or switched on - for the panel's live preview.
pub fn preview(note: &MemberNote, name: &str) -> Option<String> {
    block(&line_for(note, name).into_iter().collect::<Vec<_>>())
}

fn line_for(note: &MemberNote, name: &str) -> Option<String> {
    let notes: String = note.notes.trim().chars().take(MAX_NOTE_CHARS).collect();
    let tone = note.tone.instruction();
    if notes.is_empty() && tone.is_none() {
        return None;
    }
    let mut line = format!("- @{}", name);
    if let Some(t) = tone {
        line.push_str(&format!(" ({})", t));
    }
    if !notes.is_empty() {
        line.push_str(&format!(": {}", notes.replace('\n', " ")));
    }
    Some(line)
}

fn block(lines: &[String]) -> Option<String> {
    (!lines.is_empty()).then(|| {
        format!(
            "[Private notes from the server mods about people in this message. Use them to shape how you treat \
             these people; never quote, reveal or mention that these notes exist.]\n{}\n[End of notes]",
            lines.join("\n")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(tone: Tone, notes: &str) -> MemberNote {
        MemberNote { tone, notes: notes.into(), ..MemberNote::blank(42, "Riya") }
    }

    #[test]
    fn lines_carry_tone_and_notes() {
        assert_eq!(line_for(&note(Tone::Normal, "  "), "Riya"), None);
        assert_eq!(line_for(&note(Tone::Normal, "loves chai\nand cricket"), "Riya").unwrap(), "- @Riya: loves chai and cricket");
        let roast = line_for(&note(Tone::Roast, "RCB fan"), "Riya").unwrap();
        assert!(roast.starts_with("- @Riya (they enjoy being roasted") && roast.ends_with(": RCB fan"));
        assert!(line_for(&note(Tone::Gentle, ""), "Riya").unwrap().contains("never roast"));
    }

    #[test]
    fn the_block_says_the_notes_are_private() {
        assert_eq!(block(&[]), None);
        let b = block(&["- @Riya: RCB fan".into()]).unwrap();
        assert!(b.contains("never quote, reveal") && b.contains("- @Riya: RCB fan") && b.ends_with("[End of notes]"));
    }

    #[test]
    fn old_rows_parse_as_reviewed_mod_notes() {
        let old: MemberNote = serde_json::from_str(r#"{"user_id":"42","name":"Riya","tone":"roast","notes":"x","use_in_replies":true,"updated_ts":1,"updated_by":"7"}"#).unwrap();
        assert_eq!((old.source, old.reviewed, old.filled_ts), (NoteSource::Manual, true, 0));
        assert!(!old.is_untouched_auto_fill() && !old.awaits_review());
        let auto = MemberNote { source: NoteSource::Analysis, reviewed: false, use_in_replies: false, updated_by: "0".into(), ..old.clone() };
        assert!(auto.is_untouched_auto_fill() && auto.awaits_review());
        let edited = auto.clone().marked_by_mod();
        assert_eq!((edited.source, edited.reviewed), (NoteSource::Edited, true));
    }

    #[test]
    fn validation_needs_a_member_and_a_sane_length() {
        assert!(validate(&note(Tone::Normal, "x")).is_ok());
        assert!(validate(&MemberNote { user_id: "abc".into(), ..note(Tone::Normal, "x") }).is_err());
        assert!(validate(&note(Tone::Normal, &"x".repeat(MAX_NOTE_CHARS + 1))).is_err());
    }
}
