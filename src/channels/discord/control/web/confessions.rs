//! The Confessions page: every submission with its number, when it came, what
//! a mod decided, who sent it, who decided, and the text.
//!
//! This is the only place the whole record can be read at once, and it is the
//! most private page here. Three things follow from that.
//!
//! **It is mods-only, like every page in this panel** — the admin middleware
//! guards it, and there is no public corner of it. The confessions channel
//! itself never shows a name; this page is the other half of that bargain, and
//! it exists so moderation is possible at all.
//!
//! **Every look is one line in the activity log**, with what was searched for
//! and nothing of what came back. "Who read the confessions, and when" has to
//! have an answer, and the answer must not itself be a copy of them.
//!
//! **Nothing here writes.** Approving and rejecting happen on the buttons in
//! Discord, where the mod who pressed is on the record; this page cannot decide
//! anything, so a stolen session cannot post a confession.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::confess;
use super::super::super::confess_store::{self as store, Confession, Filter, Kind, Status};
use super::msglog::member_json;
use super::{ApiError, ApiResult, Caller, Panel, ok, search};

pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 500;

#[derive(Deserialize)]
pub struct Asked {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

/// What the page actually asked for, with anything unreadable refused in words
/// rather than quietly ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query_ {
    pub status: Option<Status>,
    pub q: Option<String>,
    pub limit: usize,
}

pub fn read_query(status: Option<&str>, q: Option<&str>, limit: Option<&str>) -> Result<Query_, String> {
    let status = match status.map(str::trim).filter(|s| !s.is_empty() && *s != "all") {
        None => None,
        Some(raw) => Some(Status::from_key(raw).ok_or_else(|| format!("\"{}\" isn't a state. Try pending, approved or rejected.", raw))?),
    };
    let limit = match limit.map(str::trim).filter(|v| !v.is_empty()) {
        None => DEFAULT_LIMIT,
        Some(raw) => raw.parse::<usize>().map_err(|_| format!("\"{}\" isn't a number of rows.", raw))?.clamp(1, MAX_LIMIT),
    };
    Ok(Query_ { status, q: q.map(str::trim).filter(|v| !v.is_empty()).map(String::from), limit })
}

/// The line the activity log keeps about one look. It says what was searched
/// for and never what came back — the whole point of the log is that it is not
/// itself a copy of the confessions.
pub fn audit_words(asked: &Query_) -> String {
    let mut out = String::from("Looked at the confessions");
    if let Some(status) = asked.status {
        out.push_str(&format!(" ({})", status.key()));
    }
    if let Some(ref q) = asked.q {
        out.push_str(&format!(" searching for \u{201c}{}\u{201d}", q));
    }
    out
}

fn row_json(panel: &Panel, c: &Confession, guild: u64) -> Value {
    json!({
        "number": c.number,
        "kind": c.kind,
        "answers": c.answers,
        "ts": c.created_ts,
        "status": c.status,
        "text": c.body,
        "submitter": member_json(panel, c.user_id, &c.user_name, ""),
        "by_mod": c.by_mod,
        "decided_by": (c.decided_by != 0).then(|| member_json(panel, c.decided_by, "", "")),
        "decided_ts": (c.decided_ts != 0).then_some(c.decided_ts),
        "reason": (!c.reason.trim().is_empty()).then(|| c.reason.clone()),
        "link": c.link(guild),
        "in_thread": (c.thread_id != 0).then(|| c.thread_id.to_string()),
    })
}

pub async fn list(
    State(panel): State<Panel>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    Query(q): Query<Asked>,
) -> ApiResult {
    let asked = read_query(q.status.as_deref(), q.q.as_deref(), q.limit.as_deref())
        .map_err(|why| ApiError(StatusCode::BAD_REQUEST, why))?;
    let db = store::db()
        .ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The confessions store isn't open. Restart the bot.".into()))?;

    let (rows, counts, next_number) = {
        let conn = db.lock();
        let filter = Filter { status: asked.status, q: asked.q.clone(), user: None, limit: asked.limit };
        let rows = store::list(&conn, &filter).map_err(|err| ApiError::internal(format!("confessions store: {}", err)))?;
        (rows, store::counts(&conn), store::next_number(&conn, confess::first_number()))
    };
    // Written before the rows go out, so a look is on the record even if the
    // reply never reaches them.
    search::log_quietly("confessions:list", user, &audit_words(&asked));

    let guild = panel.data.guild().and_then(|g| g.id.parse::<u64>().ok()).unwrap_or(0);
    ok(json!({
        "status": asked.status,
        "q": asked.q,
        "limit": asked.limit,
        "count": rows.len(),
        "totals": {
            "pending": counts.0,
            "approved": counts.1,
            "rejected": counts.2,
            "all": counts.0 + counts.1 + counts.2,
        },
        "next_number": next_number,
        "channels": {
            "posted": confess::channel().map(|c| c.to_string()),
            "review": confess::review_channel().map(|c| c.to_string()),
            "log": confess::log_channel().map(|c| c.to_string()),
        },
        "results": rows.iter().map(|c| row_json(&panel, c, guild)).collect::<Vec<_>>(),
    }))
}

/// How the Confessions page reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let label = match e.key.as_str() {
        "confess:approved" => "Approved a confession",
        "confess:rejected" => "Rejected a confession",
        "confess:who" => "Looked up who sent a confession",
        _ => "Looked at the confessions",
    };
    obj.insert("label".into(), json!(label));
    obj.insert("section".into(), json!({ "id": "confessions", "title": "Confessions", "icon": "🤫" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    // Never the text, on either side: the log must not become a second copy.
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

// --- test data ---------------------------------------------------------------

/// A handful for the fake server: one approved with a thread, one approved
/// reply inside it, one still waiting, and one rejected with a reason.
#[cfg(test)]
pub fn seed(conn: &mut rusqlite::Connection, now: i64) {
    use store::New;
    let one = |kind: Kind, answers: Option<i64>, body: &str, user: u64, name: &str, ts: i64| New {
        kind,
        answers,
        body: body.into(),
        user_id: user,
        user_name: name.into(),
        by_mod: false,
        created_ts: ts,
    };
    let _ = store::add(conn, &one(Kind::Confession, None, "I have never seen a single Star Wars film.", 1004, "Zoya", now - 900), 459);
    let _ = store::decide(conn, 459, Status::Approved, super::tests::ADMIN, now - 800, "");
    let _ = store::set_posted(conn, 459, 21, 990_101);
    let _ = store::set_thread(conn, 459, 990_102);
    let _ = store::add(conn, &one(Kind::Reply, Some(459), "Neither have I and I am not sorry.", 1005, "Arjun", now - 700), 459);
    let _ = store::decide(conn, 460, Status::Approved, super::tests::ADMIN, now - 600, "");
    let _ = store::set_posted(conn, 460, 990_102, 990_103);
    let _ = store::add(conn, &one(Kind::Confession, None, "I eat the crusts first and I think that is normal.", super::tests::MEMBER, "Rohan", now - 400), 459);
    let _ = store::add(conn, &one(Kind::Confession, None, "someone's phone number and address", 1006, "Troll", now - 300), 459);
    let _ = store::decide(conn, 462, Status::Rejected, super::tests::ADMIN, now - 200, "doxxing");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_is_read_or_refused_in_words() {
        let q = read_query(None, None, None).unwrap();
        assert_eq!(q, Query_ { status: None, q: None, limit: DEFAULT_LIMIT });
        assert_eq!(read_query(Some("all"), None, None).unwrap().status, None, "\"all\" is no filter at all");
        assert_eq!(read_query(Some("pending"), None, None).unwrap().status, Some(Status::Pending));
        assert_eq!(read_query(Some("rejected"), None, None).unwrap().status, Some(Status::Rejected));
        assert_eq!(read_query(None, Some("  wordle "), None).unwrap().q.as_deref(), Some("wordle"));
        assert_eq!(read_query(None, Some("   "), None).unwrap().q, None);
        assert_eq!(read_query(None, None, Some("5")).unwrap().limit, 5);
        assert_eq!(read_query(None, None, Some("9999")).unwrap().limit, MAX_LIMIT, "a silly number is capped, not refused");
        assert!(read_query(Some("posted"), None, None).unwrap_err().contains("isn't a state"));
        assert!(read_query(None, None, Some("lots")).unwrap_err().contains("isn't a number"));
    }

    /// The log says somebody looked and what they searched for — never a word
    /// of what came back.
    #[test]
    fn the_activity_log_says_who_looked_and_never_what_they_read() {
        assert_eq!(audit_words(&read_query(None, None, None).unwrap()), "Looked at the confessions");
        let asked = read_query(Some("pending"), Some("wordle"), None).unwrap();
        let words = audit_words(&asked);
        assert_eq!(words, "Looked at the confessions (pending) searching for \u{201c}wordle\u{201d}");

        let entry = |key: &str, new: &str| super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".into(),
            key: key.into(),
            new: Some(new.into()),
            old: None,
        };
        let looked = audit_entry(&entry("confessions:list", &words));
        assert_eq!(looked["label"], json!("Looked at the confessions"));
        assert_eq!(looked["section"]["id"], json!("confessions"));
        assert!(looked["new"].is_null() && looked["old"].is_null(), "the text is never in the log");
        assert_eq!(audit_entry(&entry("confess:approved", "#459"))["label"], json!("Approved a confession"));
        assert_eq!(audit_entry(&entry("confess:rejected", "#462"))["label"], json!("Rejected a confession"));
        let who = audit_entry(&entry("confess:who", "Looked up who sent #457"));
        assert_eq!(who["label"], json!("Looked up who sent a confession"));
        assert_eq!(who["change"], json!("Looked up who sent #457"));
    }
}
