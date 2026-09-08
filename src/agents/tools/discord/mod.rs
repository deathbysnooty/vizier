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
    #[schemars(description = "text to look for, matched case-insensitively anywhere in a message")]
    query: String,

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
        "Search past discord messages this bot has seen, across the channels it monitors. \
         Use it when asked what someone said before, or to check whether a topic came up earlier. \
         Returns real quotes with author, channel, timestamp and message id. \
         Returns nothing when there is no match - never invent a quote instead."
            .into()
    }

    async fn call(
        &self,
        args: Self::Input,
        _ctx: &ToolContext,
    ) -> anyhow::Result<Self::Output, VizierError> {
        let needle = args.query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok("Empty query - nothing to search for.".to_string());
        }
        let limit = args.limit.unwrap_or(10).clamp(1, 50);
        let user_filter = args.user.as_ref().map(|u| u.trim().to_lowercase());

        let channels: Vec<u64> = match args.channel_id {
            Some(c) => vec![c],
            None => searchable_channels(),
        };
        if channels.is_empty() {
            return Ok("No monitored channels are configured, so there is nothing to search."
                .to_string());
        }

        let mut hits: Vec<(chrono::DateTime<Utc>, String)> = vec![];

        for channel_id in channels {
            let session = VizierSession(
                self.agent_id.clone(),
                VizierChannelId::DiscordChanel(channel_id),
                None,
            );
            let entries = match self
                .storage
                .list_session_history(session, None, Some(5000))
                .await
            {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("history search failed for channel {}: {:?}", channel_id, err);
                    continue;
                }
            };

            for entry in entries {
                let crate::schema::SessionHistoryContent::Request(req) = entry.content else {
                    continue;
                };
                let text = req.content.to_string();
                if !text.to_lowercase().contains(&needle) {
                    continue;
                }
                if let Some(ref want) = user_filter {
                    if !req.user.to_lowercase().contains(want) {
                        continue;
                    }
                }
                let msg_id = match req.platform_message_id {
                    Some(crate::schema::PlatformMessageId::Discord(id)) => id.to_string(),
                    _ => "-".to_string(),
                };
                hits.push((
                    entry.timestamp,
                    format!(
                        "[{}] {} in <#{}> (msg {}): {}",
                        entry.timestamp.format("%Y-%m-%d %H:%M UTC"),
                        req.user,
                        channel_id,
                        msg_id,
                        text.replace('\n', " ")
                    ),
                ));
            }
        }

        if hits.is_empty() {
            return Ok(format!(
                "No messages found matching \"{}\". Say so plainly - do not guess at what was said.",
                args.query
            ));
        }

        // Newest first, so the most recent mention leads.
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        let total = hits.len();
        let shown: Vec<String> = hits.into_iter().take(limit).map(|(_, s)| s).collect();

        Ok(format!(
            "{} match(es), showing {}:\n{}",
            total,
            shown.len(),
            shown.join("\n")
        ))
    }
}
