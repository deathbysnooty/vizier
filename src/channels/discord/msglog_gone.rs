//! What happened to a message that is no longer in its channel: who deleted it,
//! and what Discord's own AutoMod stopped before anyone saw it.
//!
//! **AutoMod.** When Discord's built-in AutoMod blocks a message it posts an
//! alert in the server's log channel: a message of type 24 whose author is the
//! member who was blocked, with no text of its own and one embed of type
//! `auto_moderation_message`. The embed's description is the blocked text; its
//! fields name the rule, the channel the member tried to post in, the keyword
//! and what it matched. A blocked message never appears where it was aimed, so
//! this alert is the only record of it. [`automod_alert`] reads one; `msglog`
//! files it under the channel the member tried to post in and never keeps the
//! empty envelope.
//!
//! **Who deleted it.** Discord's delete event doesn't say. The guild audit log
//! does, sometimes: a moderator's or a bot's delete of someone else's message
//! writes a MESSAGE_DELETE entry (executor, the author as target, the channel),
//! and repeats by the same executor on the same author and channel are bundled
//! into that one entry with a count. Deleting your own message writes nothing.
//! So a delete is looked up a couple of seconds after it happens, and when no
//! entry accounts for it the record says "the author, or unknown" instead of
//! guessing. [`match_deleters`] is that matching; [`start`] is the live side,
//! one audit-log read for a burst of deletes, never more than one every few
//! seconds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::{Serialize, Serializer};
use serenity::all::{Embed, GuildId, Http, MessageType};
use tokio::sync::mpsc;

/// The embed type Discord gives an AutoMod alert.
pub const AUTOMOD_EMBED: &str = "auto_moderation_message";
/// An audit entry this close to a delete (either side) is new enough to be that delete's.
pub const FRESH_MS: i64 = 90_000;
/// How long after a delete the audit log is read: Discord writes the entry a moment later.
pub const LOOKUP_DELAY: Duration = Duration::from_secs(2);
/// The fewest seconds between two audit-log reads.
pub const LOOKUP_GAP: Duration = Duration::from_secs(4);
/// Lookups waiting before new ones are dropped (the delete is then "unknown").
const LOOKUP_QUEUE: usize = 512;
/// Seen counts are kept this long: Discord only bundles repeats within minutes.
const SEEN_KEEP_MS: i64 = 3_600_000;

// --- AutoMod alerts ---------------------------------------------------------------------------------

/// A message Discord's AutoMod blocked, read from the alert it posted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Alert {
    /// What the member tried to send.
    pub content: String,
    pub rule_name: Option<String>,
    /// Where the member tried to post it. None when the alert doesn't say.
    pub channel_id: Option<u64>,
    pub decision_id: Option<String>,
    pub keyword: Option<String>,
    pub matched: Option<String>,
    pub outcome: Option<String>,
}

/// The blocked message inside an AutoMod alert, or none for any other message.
pub fn automod_alert(kind: MessageType, embeds: &[Embed]) -> Option<Alert> {
    if kind != MessageType::AutoModAction {
        return None;
    }
    let e = embeds.iter().find(|e| e.kind.as_deref() == Some(AUTOMOD_EMBED))?;
    let field = |name: &str| e.fields.iter().find(|f| f.name == name).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty());
    Some(Alert {
        content: e.description.clone().unwrap_or_default(),
        rule_name: field("rule_name"),
        channel_id: field("channel_id").and_then(|v| v.parse().ok()).filter(|c| *c > 0),
        decision_id: field("decision_id"),
        keyword: field("keyword"),
        matched: field("keyword_matched_content"),
        outcome: field("decision_outcome"),
    })
}

// --- who deleted it -------------------------------------------------------------------------------------

/// One MESSAGE_DELETE or MESSAGE_BULK_DELETE audit entry, as much as matching needs.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditDelete {
    pub id: u64,
    pub executor: u64,
    pub executor_name: String,
    pub executor_bot: bool,
    /// The author of the deleted message; for a bulk delete, the channel.
    pub target: Option<u64>,
    /// The channel (single deletes only).
    pub channel: Option<u64>,
    /// How many deletes the entry stands for.
    pub count: u64,
    pub bulk: bool,
}

/// A member's message that was just deleted, waiting to be told who did it.
#[derive(Clone, Debug, PartialEq)]
pub struct Removal {
    pub message_id: u64,
    pub author_id: u64,
    pub channel_id: u64,
    pub ts_ms: i64,
    pub bulk: bool,
}

/// Who deleted a message, when the audit log says.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Deleter {
    #[serde(serialize_with = "as_string")]
    pub id: u64,
    pub name: String,
    pub bot: bool,
}

fn as_string<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

/// Who deleted each removal: `Some` when an audit entry accounts for it, `None`
/// when none does (the author deleting their own message, or unknown).
///
/// `seen` is each entry's count at the last look. An entry counts for as many
/// deletes as it has grown by since then; one never seen before counts in full
/// when it was made within [`FRESH_MS`] of these deletes, and not at all when it
/// is older (its count is only noted, so a later repeat can be told apart).
pub fn match_deleters(removals: &[Removal], entries: &[AuditDelete], seen: &mut HashMap<u64, u64>) -> Vec<(u64, Option<Deleter>)> {
    let Some(oldest) = removals.iter().map(|r| r.ts_ms).min() else { return Vec::new() };
    let newest = removals.iter().map(|r| r.ts_ms).max().unwrap_or(oldest);
    let mut order: Vec<&AuditDelete> = entries.iter().collect();
    order.sort_by(|x, y| y.id.cmp(&x.id));
    let mut left: Vec<u64> = order
        .iter()
        .map(|e| match seen.get(&e.id) {
            Some(before) => e.count.saturating_sub(*before),
            None => {
                let made = super::msglog::snowflake_ms(e.id);
                if made >= oldest - FRESH_MS && made <= newest + FRESH_MS { e.count.max(1) } else { 0 }
            }
        })
        .collect();
    let mut sorted: Vec<&Removal> = removals.iter().collect();
    sorted.sort_by_key(|r| (r.ts_ms, r.message_id));
    let mut out = Vec::with_capacity(removals.len());
    for r in sorted {
        let hit = order.iter().enumerate().position(|(i, e)| {
            left[i] > 0
                && e.bulk == r.bulk
                && if r.bulk { e.target == Some(r.channel_id) } else { e.target == Some(r.author_id) && e.channel == Some(r.channel_id) }
        });
        let who = hit.map(|i| {
            left[i] -= 1;
            let e = order[i];
            Deleter { id: e.executor, name: e.executor_name.clone(), bot: e.executor_bot }
        });
        out.push((r.message_id, who));
    }
    for e in entries {
        seen.insert(e.id, e.count);
    }
    seen.retain(|id, _| super::msglog::snowflake_ms(*id) >= newest - SEEN_KEEP_MS);
    out
}

// --- the live side --------------------------------------------------------------------------------------

struct Discord {
    http: Arc<Http>,
    guild: GuildId,
}

static DISCORD: OnceLock<Discord> = OnceLock::new();
static LOOKUPS: OnceLock<mpsc::Sender<Vec<Removal>>> = OnceLock::new();
static WARNED: AtomicBool = AtomicBool::new(false);

/// Remembers how to reach Discord, from the first event that can say.
pub fn remember(http: &Arc<Http>, guild: GuildId) {
    let _ = DISCORD.set(Discord { http: http.clone(), guild });
}

/// Asks who deleted these messages. Never waits: a full queue drops the ask and
/// the messages stay "unknown".
pub fn request(removals: Vec<Removal>) {
    if removals.is_empty() {
        return;
    }
    if let Some(tx) = LOOKUPS.get() {
        let _ = tx.try_send(removals);
    }
}

/// Starts the lookup task. Its answers go back to the message log's writer as
/// `Event::Deleters`, so they are applied in order with everything else.
pub fn start(events: mpsc::Sender<super::msglog::Event>) {
    let (tx, mut rx) = mpsc::channel::<Vec<Removal>>(LOOKUP_QUEUE);
    if LOOKUPS.set(tx).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut last: Option<tokio::time::Instant> = None;
        while let Some(first) = rx.recv().await {
            tokio::time::sleep(LOOKUP_DELAY).await;
            if let Some(at) = last {
                let since = at.elapsed();
                if since < LOOKUP_GAP {
                    tokio::time::sleep(LOOKUP_GAP - since).await;
                }
            }
            // Everything that arrived meanwhile goes in the same read.
            let mut removals = first;
            while let Ok(more) = rx.try_recv() {
                removals.extend(more);
            }
            last = Some(tokio::time::Instant::now());
            let entries = fetch(removals.iter().any(|r| r.bulk)).await;
            if events.send(super::msglog::Event::Deleters { removals, entries }).await.is_err() {
                break;
            }
        }
    });
}

async fn fetch(bulk: bool) -> Option<Vec<AuditDelete>> {
    use serenity::all::audit_log::{Action, MessageAction};
    let d = DISCORD.get()?;
    let mut out = Vec::new();
    let kinds: &[(MessageAction, bool)] = if bulk { &[(MessageAction::Delete, false), (MessageAction::BulkDelete, true)] } else { &[(MessageAction::Delete, false)] };
    for (action, is_bulk) in kinds {
        match d.guild.audit_logs(&d.http, Some(Action::Message(*action)), None, None, Some(50)).await {
            Ok(logs) => {
                for e in logs.entries {
                    let who = logs.users.get(&e.user_id);
                    out.push(AuditDelete {
                        id: e.id.get(),
                        executor: e.user_id.get(),
                        executor_name: who.map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone())).unwrap_or_else(|| e.user_id.get().to_string()),
                        executor_bot: who.is_some_and(|u| u.bot),
                        target: e.target_id.map(|t| t.get()),
                        channel: e.options.as_ref().and_then(|o| o.channel_id).map(|c| c.get()),
                        count: e.options.as_ref().and_then(|o| o.count).unwrap_or(1),
                        bulk: *is_bulk,
                    });
                }
            }
            Err(err) => {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    tracing::warn!("msglog: can't read the audit log to see who deleted a message (does the bot have View Audit Log?): {}", err);
                }
                return None;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    const EPOCH: i64 = 1_420_070_400_000;
    const T: i64 = 1_790_000_000_000;
    const MOD: u64 = 7001;
    const WICK: u64 = 7002;
    const MEMBER: u64 = 42;
    const OTHER: u64 = 43;
    const CHAN: u64 = 21;

    fn id_at(ms: i64, seq: u64) -> u64 {
        (((ms - EPOCH) as u64) << 22) | seq
    }

    fn entry(at: i64, seq: u64, executor: u64, target: u64, channel: Option<u64>, count: u64, bulk: bool) -> AuditDelete {
        AuditDelete {
            id: id_at(at, seq),
            executor,
            executor_name: if executor == WICK { "Wick".into() } else { "Meera".into() },
            executor_bot: executor == WICK,
            target: Some(target),
            channel,
            count,
            bulk,
        }
    }

    fn removal(id: u64, author: u64, at: i64, bulk: bool) -> Removal {
        Removal { message_id: id, author_id: author, channel_id: CHAN, ts_ms: at, bulk }
    }

    #[test]
    fn a_mod_delete_names_the_mod() {
        let mut seen = HashMap::new();
        let got = match_deleters(&[removal(1, MEMBER, T, false)], &[entry(T + 300, 1, MOD, MEMBER, Some(CHAN), 1, false)], &mut seen);
        assert_eq!(got, vec![(1, Some(Deleter { id: MOD, name: "Meera".into(), bot: false }))]);
    }

    #[test]
    fn a_bot_delete_names_the_bot() {
        let mut seen = HashMap::new();
        let entries = [entry(T + 200, 1, WICK, MEMBER, Some(CHAN), 1, false), entry(T - 5_000, 2, MOD, OTHER, Some(CHAN), 1, false)];
        let got = match_deleters(&[removal(1, MEMBER, T, false)], &entries, &mut seen);
        assert_eq!(got[0].1.as_ref().map(|d| (d.name.as_str(), d.bot)), Some(("Wick", true)));
    }

    #[test]
    fn a_self_delete_has_no_entry_and_is_never_guessed() {
        let mut seen = HashMap::new();
        // Only entries about someone else, another channel, or from long ago.
        let entries = [
            entry(T + 100, 1, MOD, OTHER, Some(CHAN), 1, false),
            entry(T + 100, 2, MOD, MEMBER, Some(99), 1, false),
            entry(T - 20 * 60_000, 3, MOD, MEMBER, Some(CHAN), 1, false),
        ];
        let got = match_deleters(&[removal(1, MEMBER, T, false)], &entries, &mut seen);
        assert_eq!(got, vec![(1, None)]);
        assert_eq!(match_deleters(&[removal(2, MEMBER, T, false)], &[], &mut seen), vec![(2, None)], "no entries at all");
    }

    #[test]
    fn a_bulk_delete_is_the_bulk_entry_for_that_channel() {
        let mut seen = HashMap::new();
        let removals = [removal(1, MEMBER, T, true), removal(2, OTHER, T, true), removal(3, MEMBER, T, true)];
        // A single delete of the same member is not a bulk delete.
        let entries = [entry(T + 400, 1, MOD, MEMBER, Some(CHAN), 1, false), entry(T + 500, 2, WICK, CHAN, None, 30, true)];
        let got = match_deleters(&removals, &entries, &mut seen);
        assert!(got.iter().all(|(_, d)| d.as_ref().is_some_and(|d| d.id == WICK)), "{got:?}");
    }

    #[test]
    fn a_bundled_entry_counts_only_what_it_grew_by() {
        let mut seen = HashMap::new();
        let first = entry(T, 1, MOD, MEMBER, Some(CHAN), 1, false);
        assert!(match_deleters(&[removal(1, MEMBER, T + 100, false)], &[first.clone()], &mut seen)[0].1.is_some());
        // Four minutes later the mod deletes two more of theirs: Discord bumps the
        // same entry to 3. Both are the mod's; a third delete in the same look is not.
        let bumped = AuditDelete { count: 3, ..first.clone() };
        let later = T + 4 * 60_000;
        let got = match_deleters(&[removal(2, MEMBER, later, false), removal(3, MEMBER, later + 10, false), removal(4, MEMBER, later + 20, false)], &[bumped.clone()], &mut seen);
        assert_eq!(got.iter().map(|(id, d)| (*id, d.is_some())).collect::<Vec<_>>(), vec![(2, true), (3, true), (4, false)]);
        // Nothing grew: a delete now is the member's own.
        assert_eq!(match_deleters(&[removal(5, MEMBER, later + 60_000, false)], &[bumped], &mut seen)[0].1, None);
        // An old entry seen for the first time with a count is only noted, never matched.
        let mut fresh_seen = HashMap::new();
        let old = entry(T - 10 * 60_000, 9, MOD, MEMBER, Some(CHAN), 4, false);
        assert_eq!(match_deleters(&[removal(6, MEMBER, T, false)], &[old.clone()], &mut fresh_seen)[0].1, None);
        let grown = AuditDelete { count: 5, ..old };
        assert!(match_deleters(&[removal(7, MEMBER, T + 1000, false)], &[grown], &mut fresh_seen)[0].1.is_some(), "but its next repeat is");
    }

    /// An alert as Discord's gateway delivers it: type 24, the blocked member as
    /// the author, no text, one `auto_moderation_message` embed.
    pub fn alert_json(channel_field: bool, keyword_field: bool) -> serde_json::Value {
        let mut fields = vec![
            serde_json::json!({ "name": "rule_name", "value": "Block slurs", "inline": false }),
            serde_json::json!({ "name": "decision_id", "value": "1419000000000000001", "inline": false }),
            serde_json::json!({ "name": "keyword_matched_content", "value": "chutiya", "inline": false }),
            serde_json::json!({ "name": "decision_outcome", "value": "blocked", "inline": false }),
            serde_json::json!({ "name": "quarantine_user", "value": "", "inline": false }),
        ];
        if channel_field {
            fields.insert(1, serde_json::json!({ "name": "channel_id", "value": "1400000000000000023", "inline": false }));
        }
        if keyword_field {
            fields.insert(2, serde_json::json!({ "name": "keyword", "value": "*chutiya*", "inline": false }));
        }
        serde_json::json!({
            "id": "1419100000000000000",
            "channel_id": "1516779799865987101",
            "guild_id": "900",
            "author": { "id": "42", "username": "gooning_enthusiast", "global_name": "gooner", "avatar": null, "discriminator": "0", "public_flags": 0 },
            "content": "",
            "timestamp": "2026-09-20T14:31:05.123000+00:00",
            "edited_timestamp": null,
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [{
                "type": "auto_moderation_message",
                "description": "tu chutiya hai bc, sab jaante hai",
                "fields": fields,
            }],
            "pinned": false,
            "type": 24,
            "flags": 0,
            "components": [],
        })
    }

    #[test]
    fn an_automod_alert_is_read_from_a_real_payload() {
        let msg: serenity::all::Message = serde_json::from_value(alert_json(true, true)).expect("a gateway message");
        assert_eq!(msg.kind, MessageType::AutoModAction);
        let a = automod_alert(msg.kind, &msg.embeds).expect("an alert");
        assert_eq!(a.content, "tu chutiya hai bc, sab jaante hai");
        assert_eq!(a.channel_id, Some(1_400_000_000_000_000_023), "the channel they tried to post in, not the log channel");
        assert_eq!(
            (a.rule_name.as_deref(), a.keyword.as_deref(), a.matched.as_deref(), a.outcome.as_deref(), a.decision_id.as_deref()),
            (Some("Block slurs"), Some("*chutiya*"), Some("chutiya"), Some("blocked"), Some("1419000000000000001"))
        );
        // A field Discord left out is just missing.
        let msg: serenity::all::Message = serde_json::from_value(alert_json(true, false)).unwrap();
        let a = automod_alert(msg.kind, &msg.embeds).unwrap();
        assert_eq!((a.keyword, a.rule_name.as_deref()), (None, Some("Block slurs")));
        let msg: serenity::all::Message = serde_json::from_value(alert_json(false, true)).unwrap();
        assert_eq!(automod_alert(msg.kind, &msg.embeds).unwrap().channel_id, None);
        // Anything else is not an alert.
        assert!(automod_alert(MessageType::Regular, &msg.embeds).is_none());
        assert!(automod_alert(MessageType::AutoModAction, &[]).is_none());
    }
}
