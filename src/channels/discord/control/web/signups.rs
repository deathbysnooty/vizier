//! The Sign-ups page: who said they're in, who said they're out, when, and the
//! totals — with a copy-friendly list.
//!
//! This list is the deliverable. Next month's rewards are handed out from it, so
//! the page is built to be trusted and to be taken away: every member is on it
//! with their id, and the same list comes back as plain lines ready to paste
//! somewhere else. Members who have left the server are on it too, marked, since
//! leaving does not un-press a button.
//!
//! It also warns, because a sign-up that nobody notices is worse than none: if
//! the role setting is empty or points at a role the bot can't hand out, or if
//! anybody is on the list without the role actually on them, the page says so in
//! the same words the log used at startup.
//!
//! Nothing here writes. Admins only, like every panel page, and a look is one
//! line in the activity log.

use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::super::signup::{self, RoleFacts, Verdict};
use super::super::super::signup_store::{self as store, Answer, Entry};
use super::msglog::member_json;
use super::{ApiError, ApiResult, Caller, Panel, ok, search};

/// The role list as the sign-up check wants it, from what the panel already knows.
fn facts(panel: &Panel) -> Vec<RoleFacts> {
    panel
        .data
        .roles()
        .into_iter()
        .filter_map(|r| r.id.parse().ok().map(|id| RoleFacts { id, name: r.name, position: r.position, managed: r.managed }))
        .collect()
}

fn store_db() -> Result<&'static parking_lot::Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The sign-up store isn't open. Restart the bot.".into()))
}

fn row_json(panel: &Panel, e: &Entry) -> Value {
    let mut member = member_json(panel, e.user_id, &e.name, "");
    if let Value::Object(ref mut map) = member {
        map.insert("answered_ts".into(), json!(e.ts));
        map.insert("first_ts".into(), json!(e.first_ts));
        // Only meaningful for a yes: there is nothing to wear after a no.
        map.insert("has_role".into(), json!(e.answer == Answer::In && e.worn));
    }
    member
}

/// One line per member, ready to paste: the id first because that is what a
/// reward script needs, then the name for a human reading it.
pub fn copy_list(rows: &[Entry]) -> String {
    rows.iter().map(|e| format!("{} {}", e.user_id, if e.name.is_empty() { "?" } else { e.name.as_str() })).collect::<Vec<_>>().join("\n")
}

/// Just the ids, one per line.
pub fn copy_ids(rows: &[Entry]) -> String {
    rows.iter().map(|e| e.user_id.to_string()).collect::<Vec<_>>().join("\n")
}

/// Everything the page needs to warn about, in the words the log used.
pub fn warnings(state: &Verdict, unworn: i64) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(why) = state.warning() {
        out.push(why);
    }
    if unworn > 0 {
        out.push(format!(
            "{} {} on the list without the role actually on them. Once the role is set and the bot can reach it, their next press puts it on — or give it out by hand from the list below.",
            unworn,
            if unworn == 1 { "member is" } else { "members are" }
        ));
    }
    out
}

pub async fn list(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let (ins, outs, counts, unworn, posts) = {
        let conn = store_db()?.lock();
        let ins = store::listed(&conn, Answer::In).map_err(|err| ApiError::internal(format!("sign-up store: {}", err)))?;
        let outs = store::listed(&conn, Answer::Out).map_err(|err| ApiError::internal(format!("sign-up store: {}", err)))?;
        let posts = store::posts(&conn).unwrap_or_default();
        (ins, outs, store::counts(&conn), store::unworn(&conn), posts)
    };
    // The role as it stands now, judged by exactly the function the startup
    // check and every press use, so the page never disagrees with the log.
    let state = signup::verdict(signup::role_id(), &facts(&panel), panel.data.bot_top_role());
    let channels = panel.data.channels();

    search::log_quietly("signups:list", user, "Looked at the sign-ups");
    ok(json!({
        "in": ins.iter().map(|e| row_json(&panel, e)).collect::<Vec<_>>(),
        "out": outs.iter().map(|e| row_json(&panel, e)).collect::<Vec<_>>(),
        "totals": { "in": counts.0, "out": counts.1, "answered": counts.0 + counts.1, "without_role": unworn },
        "role": state,
        "role_key": signup::ROLE_KEY,
        "warnings": warnings(&state, unworn),
        "copy": { "list": copy_list(&ins), "ids": copy_ids(&ins), "out_list": copy_list(&outs) },
        "posts": posts
            .iter()
            .map(|p| json!({
                "message_id": p.message_id.to_string(),
                "channel": {
                    "id": p.channel_id.to_string(),
                    "name": channels.iter().find(|c| c.id == p.channel_id.to_string()).map(|c| c.name.clone()),
                },
                "posted_by": member_json(&panel, p.posted_by, "", ""),
                "posted_ts": p.posted_ts,
                "title": p.title,
                "images": p.images,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// How the Sign-ups page reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let posted = e.key == "signup:post";
    obj.insert("label".into(), json!(if posted { "Posted a sign-up message" } else { "Looked at the sign-ups" }));
    obj.insert("section".into(), json!({ "id": "signups", "title": "Sign-ups", "icon": "✋" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

// --- test data ---------------------------------------------------------------

/// A handful of answers for the fake server: two in with the role, one in
/// without it (so the page has its warning to draw), and one out.
#[cfg(test)]
pub fn seed(conn: &rusqlite::Connection, now: i64) {
    let _ = store::record(conn, super::tests::ADMIN, "Kabir", Answer::In, true, now - 300);
    let _ = store::record(conn, super::tests::MEMBER, "Rohan", Answer::In, true, now - 200);
    let _ = store::record(conn, 1004, "Zoya", Answer::In, false, now - 100);
    let _ = store::record(conn, 1005, "Arjun", Answer::Out, false, now - 50);
    let _ = store::add_post(
        conn,
        &store::Post {
            message_id: 990_001,
            channel_id: 21,
            posted_by: super::tests::ADMIN,
            posted_ts: now - 400,
            title: "Next month".into(),
            body: "the mods have an idea for next month - would you be up for it?".into(),
            images: 2,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Entry> {
        vec![
            Entry { user_id: 11, name: "Zoya".into(), answer: Answer::In, first_ts: 100, ts: 100, worn: true },
            Entry { user_id: 12, name: "Kabir".into(), answer: Answer::In, first_ts: 200, ts: 200, worn: false },
            Entry { user_id: 13, name: String::new(), answer: Answer::In, first_ts: 300, ts: 300, worn: true },
        ]
    }

    /// The list has to be pasteable somewhere else, ids first.
    #[test]
    fn the_copy_list_is_one_member_per_line_with_the_id_first() {
        assert_eq!(copy_list(&rows()), "11 Zoya\n12 Kabir\n13 ?");
        assert_eq!(copy_ids(&rows()), "11\n12\n13");
        assert_eq!(copy_list(&[]), "", "an empty sheet copies as nothing, not as a blank line");
    }

    /// A sign-up nobody can act on must not look like a healthy one.
    #[test]
    fn the_page_warns_about_the_role_and_about_anyone_missing_it() {
        let fine = Verdict::Fine { id: "2".into(), name: "server games".into() };
        assert!(warnings(&fine, 0).is_empty(), "nothing to say when it all works");

        let one = warnings(&fine, 1);
        assert_eq!(one.len(), 1);
        assert!(one[0].starts_with("1 member is on the list without the role"), "{:?}", one);

        let both = warnings(&Verdict::Unset, 4);
        assert_eq!(both.len(), 2, "the setting and the members are two different problems");
        assert!(both[0].contains("VIZIER_GAMES_ROLE"), "{:?}", both);
        assert!(both[1].starts_with("4 members are"), "{:?}", both);

        let high = warnings(&Verdict::TooHigh { id: "4".into(), name: "Admin".into(), position: 20, bot_position: 7 }, 0);
        assert!(high[0].contains("Server Settings"), "the fix is on the page, not only in the log: {:?}", high);
    }

    /// The log says somebody looked and never what they read.
    #[test]
    fn the_activity_log_says_who_looked_and_who_posted() {
        let entry = |key: &str, new: &str| super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".into(),
            key: key.into(),
            new: Some(new.into()),
            old: None,
        };
        let looked = audit_entry(&entry("signups:list", "Looked at the sign-ups"));
        assert_eq!(looked["label"], json!("Looked at the sign-ups"));
        assert_eq!(looked["section"]["id"], json!("signups"));
        let posted = audit_entry(&entry("signup:post", "Next month"));
        assert_eq!(posted["label"], json!("Posted a sign-up message"));
        assert_eq!(posted["change"], json!("Next month"));
        assert!(posted["new"].is_null() && posted["old"].is_null());
    }
}
