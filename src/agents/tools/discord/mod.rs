use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, Http, MessageId};

use crate::agents::tools::{ToolContext, VizierTool};
use crate::error::{VizierError, throw_vizier_error};
use crate::schema::{AgentId, TopicId, VizierChannelId, VizierResponse, VizierResponseContent, VizierSession};
use crate::storage::{VizierStorage, history::HistoryStorage, state::StateStorage};

#[derive(Debug, Deserialize, Serialize)]
struct ChannelState {
    active_topic: Option<TopicId>,
}

/// A Discord id, taken as a string OR a number.
///
/// Discord ids are 19 digits, which is more precision than a double carries, and
/// some providers pass tool arguments through floating point: a channel id came
/// back as ...593500 instead of ...593443 and the message went to the wrong
/// place. Quoting the id keeps every digit, so the schema asks for a string and
/// a bare number is still accepted for older callers.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Snowflake(u64);

impl Snowflake {
    fn get(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Snowflake {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(text) => {
                text.trim().parse::<u64>().map(Snowflake).map_err(serde::de::Error::custom)
            }
            serde_json::Value::Number(number) => number
                .as_u64()
                .map(Snowflake)
                .ok_or_else(|| serde::de::Error::custom(format!("{} is not a discord id", number))),
            other => Err(serde::de::Error::custom(format!("expected a discord id, got {}", other))),
        }
    }
}

impl schemars::JsonSchema for Snowflake {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Snowflake".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // Advertised as a string so the id arrives with all 19 digits intact.
        String::json_schema(generator)
    }
}

pub fn new_discord_tools(
    discord_token: String,
    agent_id: AgentId,
    storage: Arc<VizierStorage>,
) -> (SendDiscordMessage, ReactDiscordMessage, GetDiscordMessage, SearchDiscordHistory) {
    let http = Arc::new(Http::new(&discord_token));

    (
        SendDiscordMessage { http: http.clone(), agent_id: agent_id.clone(), storage: storage.clone() },
        ReactDiscordMessage { http: http.clone() },
        GetDiscordMessage { http: http.clone() },
        SearchDiscordHistory { agent_id: agent_id.clone(), storage: storage.clone() },
    )
}

pub struct SendDiscordMessage {
    http: Arc<Http>,
    agent_id: AgentId,
    storage: Arc<VizierStorage>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SendDiscoedMessageArgs {
    #[schemars(description = "id of target discord channel, as a string")]
    channel_id: Snowflake,

    #[schemars(description = "content of the message")]
    content: String,
}

#[async_trait::async_trait]
impl VizierTool for SendDiscordMessage {
    type Input = SendDiscoedMessageArgs;
    type Output = String;

    fn name() -> String {
        "discord_send_message".to_string()
    }

    fn description(&self) -> String {
        "send a discord message to a channel, avoid using this when user interact with you directly from discord".into()
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> anyhow::Result<Self::Output, VizierError> {
        let channel_id = args.channel_id.get();
        let content = args.content.clone();

        crate::utils::discord::send_message(
            self.http.clone(),
            &ChannelId::new(channel_id),
            args.content,
        )
        .await
        .map_err(|err| VizierError(err.to_string()))?;

        let channel = VizierChannelId::DiscordChanel(channel_id);
        let key = format!("{}__{}", self.agent_id, channel.to_slug());
        let topic_id = if let Ok(Some(value)) = self.storage.get_state(key).await {
            let state: ChannelState = serde_json::from_value(value).unwrap_or(ChannelState { active_topic: None });
            state.active_topic
        } else {
            None
        };

        let session = VizierSession(self.agent_id.clone(), channel, topic_id);
        let response = VizierResponse {
            timestamp: Utc::now(),
            content: VizierResponseContent::Message { content, stats: None },
            attachments: vec![],
        };
        self.storage
            .save_session_history(session, crate::schema::SessionHistoryContent::Response(response))
            .await
            .map_err(|e| VizierError(e.to_string()))?;

        Ok(format!("Message sent to channel {}", channel_id))
    }
}

pub struct ReactDiscordMessage {
    http: Arc<Http>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ReactDiscoedMessageArgs {
    #[schemars(description = "id of the target discord channel, as a string")]
    channel_id: Snowflake,

    #[schemars(description = "id of the target discord message, as a string")]
    message_id: Snowflake,

    #[schemars(description = "an emoji")]
    emoji: char,
}

#[async_trait::async_trait]
impl VizierTool for ReactDiscordMessage {
    type Input = ReactDiscoedMessageArgs;
    type Output = String;

    fn name() -> String {
        "discord_react_message".to_string()
    }

    fn description(&self) -> String {
        "emoji react to a discord message".into()
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> anyhow::Result<Self::Output, VizierError> {
        let channel = ChannelId::new(args.channel_id.get());
        let message_id = MessageId::new(args.message_id.get());

        let message = channel
            .message(self.http.clone(), message_id)
            .await
            .map_err(|err| VizierError(err.to_string()))?;

        message
            .react(self.http.clone(), args.emoji)
            .await
            .map_err(|err| VizierError(err.to_string()))?;

        Ok(format!("Reacted with {} to message {}", args.emoji, args.message_id.get()))
    }
}

pub struct GetDiscordMessage {
    http: Arc<Http>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetDiscordMessageArgs {
    #[schemars(description = "id of the target discord channel, as a string")]
    channel_id: Snowflake,

    #[schemars(description = "id of the target discord message, as a string")]
    message_id: Snowflake,
}

#[async_trait::async_trait]
impl VizierTool for GetDiscordMessage {
    type Input = GetDiscordMessageArgs;
    type Output = String;

    fn name() -> String {
        "discord_get_message_by_id".to_string()
    }

    fn description(&self) -> String {
        "get message by message id".into()
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> anyhow::Result<Self::Output, VizierError> {
        let channel = ChannelId::new(args.channel_id.get());
        let message_id = MessageId::new(args.message_id.get());

        let response = channel.message(self.http.clone(), message_id).await;

        match response {
            Ok(message) => Ok(format!(
                "{}: {}",
                message.author.display_name(),
                message.content
            )),
            Err(err) => throw_vizier_error("discord_react_message ", err),
        }
    }
}

/// Channels the history search is allowed to look at.
///
/// Mirrors `VIZIER_DISCORD_CHANNELS` (comma-separated ids), the same allowlist
/// that gates ingestion, so the tool can never surface a channel the bot was
/// not asked to monitor.
fn searchable_channels() -> Vec<u64> {
    std::env::var("VIZIER_DISCORD_CHANNELS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

pub struct SearchDiscordHistory {
    agent_id: AgentId,
    storage: Arc<VizierStorage>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SearchDiscordHistoryArgs {
    #[schemars(
        description = "optional: words to look for, matched case-insensitively anywhere in a message. Plain words only - no quotes or boolean syntax. Separate alternatives with commas and a message matching any of them is returned. Omit to fetch everything by a person."
    )]
    query: Option<String>,

    #[schemars(
        description = "optional: only messages from this person, matched against their discord display name or user id"
    )]
    user: Option<String>,

    #[schemars(description = "optional: maximum number of matches to return, default 10, max 50")]
    limit: Option<usize>,
}

#[async_trait::async_trait]
impl VizierTool for SearchDiscordHistory {
    type Input = SearchDiscordHistoryArgs;
    type Output = String;

    fn name() -> String {
        "discord_search_history".to_string()
    }

    fn description(&self) -> String {
        "Search what people actually said in this discord server, across the channels this bot \
         monitors. This is the ONLY way to find out what someone said - use it for any question \
         about what a person has been saying, what they said earlier, or whether a topic came up. \
         Pass `user` alone to get everything from one person, `query` alone to search text, or \
         both. Returns real quotes with author, channel, timestamp and message id, and returns \
         nothing when there is no match - never invent a quote instead. Do not use memory tools \
         for this; memory holds only your own notes, not what members said."
            .into()
    }

    async fn call(
        &self,
        args: Self::Input,
        _ctx: &ToolContext,
    ) -> anyhow::Result<Self::Output, VizierError> {
        let user_given = args
            .user
            .as_deref()
            .map(|u| !u.trim().is_empty())
            .unwrap_or(false);
        let query_given = args
            .query
            .as_deref()
            .map(|q| !q.trim().is_empty())
            .unwrap_or(false);
        if !user_given && !query_given {
            return Ok("Give either something to search for, or a person to search by.".to_string());
        }

        // Always every monitored channel. There is deliberately no channel
        // argument: the model cannot know real channel ids, and guesses that were
        // a few digits off surfaced to users as "that channel is not monitored"
        // about channels that are.
        let channels: Vec<u64> = searchable_channels();
        if channels.is_empty() {
            return Ok("No monitored channels are configured, so there is nothing to search."
                .to_string());
        }
        let slugs: Vec<String> = channels
            .iter()
            .map(|c| VizierChannelId::DiscordChanel(*c).to_slug())
            .collect();

        let limit = args.limit.unwrap_or(15).clamp(1, 50);
        let rows = self
            .storage
            .search_user_messages(
                &self.agent_id,
                &slugs,
                args.query.as_deref(),
                args.user.as_deref(),
                limit,
            )
            .await
            .map_err(|e| VizierError(e.to_string()))?;

        if rows.is_empty() {
            return Ok(format!(
                "No messages found for {}. Say so plainly - do not guess at what was said.",
                match (&args.query, &args.user) {
                    (Some(q), Some(u)) => format!("\"{}\" from {}", q, u),
                    (Some(q), None) => format!("\"{}\"", q),
                    (None, Some(u)) => format!("messages by {}", u),
                    (None, None) => "that search".to_string(),
                }
            ));
        }

        let total = rows.len();
        let mut lines: Vec<String> = rows
            .into_iter()
            .map(|(ts, channel, user, text, mid)| {
                let when = chrono::DateTime::from_timestamp_millis(ts)
                    .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| ts.to_string());
                let cid = channel.trim_start_matches("discord__");
                format!(
                    "[{}] {} in <#{}> (msg {}): {}",
                    when,
                    user,
                    cid,
                    if mid.is_empty() { "-" } else { &mid },
                    text.replace('\n', " ")
                )
            })
            .collect();
        // Oldest first reads better once the model relays it.
        lines.reverse();

        Ok(format!("{} message(s) found:\n{}", total, lines.join("\n")))
    }

}

// --- members' reminders ------------------------------------------------------------

/// The Discord channel a tool call came from, when it came from Discord.
fn session_channel(ctx: &ToolContext) -> Option<u64> {
    match ctx.session.1 {
        VizierChannelId::DiscordChanel(id) => Some(id),
        _ => None,
    }
}

pub struct SetMemberReminder;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SetMemberReminderArgs {
    #[schemars(description = "Discord id of the member to remind, as a string. Usually the person asking; an admin may name someone else (use the id of the member they mentioned)")]
    user_id: Snowflake,

    #[schemars(description = "Discord id of the person who asked for this reminder, as a string - the number in their (DiscordId: ...)")]
    requested_by: Snowflake,

    #[schemars(description = "When, in India time, written the way people say it: 'in 2 hours', '30m', '1h30m', 'at 9pm', '21:30', 'tomorrow 9am', '2026-09-20 18:00'")]
    when: String,

    #[schemars(description = "What to remind them about, short and in their words (e.g. 'call mom', 'join the quiz')")]
    what: String,
}

#[async_trait::async_trait]
impl VizierTool for SetMemberReminder {
    type Input = SetMemberReminderArgs;
    type Output = String;

    fn name() -> String {
        "set_member_reminder".to_string()
    }

    fn description(&self) -> String {
        "Save a reminder when a Discord member asks you to remind them about something (\"remind me in 2 hours to...\", \"remind me tomorrow at 9\"). \
         The bot pings that member in this channel at the time - no need to schedule a task or send anything yourself. \
 Members can only remind themselves; a bot admin can ask you to remind another member (set user_id to that member). Tell them the time it returns."
            .into()
    }

    async fn call(&self, args: Self::Input, ctx: &ToolContext) -> anyhow::Result<Self::Output, VizierError> {
        use crate::channels::discord::control::memos;
        let channel = session_channel(ctx).ok_or_else(|| VizierError("reminders only work from a Discord channel".into()))?;
        let now = Utc::now().timestamp();
        let due = memos::parse_when(&args.when, now).ok_or_else(|| {
            VizierError(format!(
                "could not read '{}' as a time; use e.g. 'in 2 hours', 'at 9pm', 'tomorrow 9am' or 'YYYY-MM-DD HH:MM' (India time)",
                args.when
            ))
        })?;
        let memo =
            memos::create(args.user_id.get(), channel, &args.what, due, "chat", args.requested_by.get()).map_err(VizierError)?;
        Ok(format!(
            "Reminder #{} saved for {} ({}): \"{}\". The bot will ping them then.",
            memo.id,
            memos::describe(memo.due_ts, now),
            format!("<t:{}:R>", memo.due_ts),
            memo.text
        ))
    }
}

pub struct ListMemberReminders;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListMemberRemindersArgs {
    #[schemars(description = "Discord id of the member, as a string")]
    user_id: Snowflake,
}

#[async_trait::async_trait]
impl VizierTool for ListMemberReminders {
    type Input = ListMemberRemindersArgs;
    type Output = String;

    fn name() -> String {
        "list_member_reminders".to_string()
    }

    fn description(&self) -> String {
        "List a Discord member's waiting reminders (when they ask what reminders they have)".into()
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> anyhow::Result<Self::Output, VizierError> {
        use crate::channels::discord::control::memos;
        let now = Utc::now().timestamp();
        let mine = memos::pending_for(args.user_id.get());
        if mine.is_empty() {
            return Ok("They have no reminders waiting.".into());
        }
        Ok(mine.iter().map(|m| format!("#{} {}: {}", m.id, memos::describe(m.due_ts, now), m.text)).collect::<Vec<_>>().join("\n"))
    }
}

pub struct CancelMemberReminder;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct CancelMemberReminderArgs {
    #[schemars(description = "Discord id of the member who owns the reminder, as a string")]
    user_id: Snowflake,

    #[schemars(description = "The reminder number from list_member_reminders")]
    reminder_id: i64,
}

#[async_trait::async_trait]
impl VizierTool for CancelMemberReminder {
    type Input = CancelMemberReminderArgs;
    type Output = String;

    fn name() -> String {
        "cancel_member_reminder".to_string()
    }

    fn description(&self) -> String {
        "Cancel one of a Discord member's own waiting reminders when they ask".into()
    }

    async fn call(&self, args: Self::Input, _ctx: &ToolContext) -> anyhow::Result<Self::Output, VizierError> {
        use crate::channels::discord::control::memos;
        Ok(if memos::cancel(args.reminder_id, Some(args.user_id.get())) {
            format!("Reminder #{} cancelled.", args.reminder_id)
        } else {
            "No waiting reminder with that number belongs to them.".into()
        })
    }
}

