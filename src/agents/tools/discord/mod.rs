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
    #[schemars(description = "id of target discord channel")]
    channel_id: u64,

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
        let channel_id = args.channel_id;
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
    #[schemars(description = "id of the target discord channel")]
    channel_id: u64,

    #[schemars(description = "id of the target discord message")]
    message_id: u64,

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
        let channel = ChannelId::new(args.channel_id);
        let message_id = MessageId::new(args.message_id);

        let message = channel
            .message(self.http.clone(), message_id)
            .await
            .map_err(|err| VizierError(err.to_string()))?;

        message
            .react(self.http.clone(), args.emoji)
            .await
            .map_err(|err| VizierError(err.to_string()))?;

        Ok(format!("Reacted with {} to message {}", args.emoji, args.message_id))
    }
}

pub struct GetDiscordMessage {
    http: Arc<Http>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GetDiscordMessageArgs {
    #[schemars(description = "id of the target discord channel")]
    channel_id: u64,

    #[schemars(description = "id of the target discord message")]
    message_id: u64,
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
        let channel = ChannelId::new(args.channel_id);
        let message_id = MessageId::new(args.message_id);

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
        description = "optional: text to look for, matched case-insensitively anywhere in a message. Omit to fetch everything by a person."
    )]
    query: Option<String>,

    #[schemars(description = "optional: restrict to one discord channel id")]
    channel_id: Option<u64>,

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

        // An explicit channel narrows the search; it never widens it.
        let allowed = searchable_channels();
        if allowed.is_empty() {
            return Ok("No monitored channels are configured, so there is nothing to search."
                .to_string());
        }
        let channels: Vec<u64> = match args.channel_id {
            Some(c) if allowed.contains(&c) => vec![c],
            Some(_) => {
                return Ok(
                    "That channel is not one of the monitored channels, so its history \
                     cannot be searched."
                        .to_string(),
                );
            }
            None => allowed,
        };
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
