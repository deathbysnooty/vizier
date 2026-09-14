//! The control panel on the web: a small server inside the bot that shows every
//! feature from the catalog and changes its settings, reminders and the bot
//! itself. Vizier's own web UI is not involved; this one is self-contained.
//!
//! Signing in: an admin runs `/panel` in Discord and gets a one-use link
//! `{VIZIER_PANEL_URL}/login#t=<token>`. The token rides in the fragment, so it
//! never reaches a server log; the page posts it to `/api/login`, which swaps it
//! for a session cookie. Every other `/api` call needs that session and the user
//! still being an admin, and a change also needs the `X-Panel: 1` header.
//!
//! Only keys the catalog describes can be read or written: the environment also
//! holds tokens and API keys, and none of those may ever leave the process.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use serenity::all::{
    ChannelType, CommandInteraction, Context, CreateActionRow, CreateButton, CreateCommand,
    CreateInteractionResponse, CreateInteractionResponseMessage, GuildId, UserId,
};

use super::catalog::{self, Kind, Section, Setting};
use super::reminders::{self, Reminder, Schedule};

pub const BOT_NAME: &str = "Loduchand";
const COOKIE: &str = "mlci_panel";
const LOGIN_PER_MINUTE: usize = 10;

const INDEX_HTML: &str = include_str!("ui/index.html");
const APP_CSS: &str = include_str!("ui/app.css");
const APP_JS: &str = include_str!("ui/app.js");
const FAVICON: &str = include_str!("ui/favicon.svg");

// --- what the panel knows about Discord ----------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct GuildInfo {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub members: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// Text and announcement channels: somewhere the bot can post.
    Text,
    /// Voice and stage channels.
    Voice,
    Forum,
    Category,
    Other,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    /// The category's name, if the channel sits in one.
    pub category: Option<String>,
    /// Where the category sits, for grouping in Discord's order.
    pub category_position: i64,
    pub position: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoleInfo {
    pub id: String,
    pub name: String,
    /// `#rrggbb`, or none for a role without a colour.
    pub color: Option<String>,
    pub position: i64,
    pub managed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemberInfo {
    pub id: String,
    /// What the server shows: nickname, then global name, then username.
    pub name: String,
    pub username: String,
    pub avatar: String,
    pub bot: bool,
}

/// Everything the panel reads from Discord, behind a trait so the server can be
/// tested and demonstrated without a gateway connection.
#[async_trait::async_trait]
pub trait PanelData: Send + Sync + 'static {
    fn guild(&self) -> Option<GuildInfo>;
    fn channels(&self) -> Vec<ChannelInfo>;
    fn roles(&self) -> Vec<RoleInfo>;
    async fn search_members(&self, query: &str, limit: usize) -> Vec<MemberInfo>;
    async fn member(&self, id: u64) -> Option<MemberInfo>;
    /// Who may use the panel.
    fn admins(&self) -> Vec<u64>;
    /// Ends the process shortly after the response has gone; systemd starts it again.
    fn restart(&self);
    /// The server's custom emoji.
    fn emojis(&self) -> Vec<EmojiInfo> {
        Vec::new()
    }
    /// A member from the cache only: for pages that name many people and must
    /// not call Discord for each.
    fn cached_member(&self, _id: u64) -> Option<MemberInfo> {
        None
    }
    /// The house points standings for a period (`houses::Period`), at `now`.
    fn house_cup(&self, _period: houses::Period, _now: i64) -> Option<houses::HouseCup> {
        None
    }
    /// The AI agent's tone and limits, or none when the agent isn't reachable.
    async fn agent_settings(&self) -> Option<agent::AgentSettings> {
        None
    }
    async fn save_agent_settings(&self, _settings: &agent::AgentSettings) -> anyhow::Result<()> {
        anyhow::bail!("the agent isn't reachable")
    }
    /// A member with their join date, account age and roles.
    async fn member_detail(&self, _id: u64) -> Option<members::MemberDetail> {
        None
    }
    /// Points, activity, games and join history for a member, at `now`.
    async fn member_stats(&self, _id: u64, _now: i64) -> members::MemberStats {
        members::MemberStats { hours: vec![0; 24], ..Default::default() }
    }
    /// Their latest messages in the AI's history, newest first.
    async fn member_seen(&self, _id: u64, _now: i64) -> Vec<members::SeenMessage> {
        Vec::new()
    }
    /// Every memory the AI agent can read.
    async fn memories(&self) -> Vec<members::MemoryEntry> {
        Vec::new()
    }
    /// Who scored on today (0) or yesterday (1), with their chat and voice counts.
    fn scorers(&self, _days_back: i64, _now: i64) -> Option<Vec<scorers::ScorerData>> {
        None
    }
    /// Everyone's messages, voice and game points over the last 30 days.
    async fn activity(&self, _now: i64) -> Vec<super::profiles::Activity> {
        Vec::new()
    }
    /// A member's stored messages over the last 30 days, newest first.
    async fn member_messages(&self, _id: u64, _now: i64) -> Vec<super::profiles::RawMessage> {
        Vec::new()
    }
    /// The parent channel of a thread the cache knows.
    fn thread_parent(&self, _channel: u64) -> Option<u64> {
        None
    }
    /// Channels whose messages never go into an analysis.
    fn sensitive_channels(&self) -> Vec<u64> {
        vec![super::super::weekly::SAFE_CORNER]
    }
    /// Every 1v1 since `since` as (winner, loser, ts).
    fn duels(&self, _since: i64) -> Vec<(u64, u64, i64)> {
        Vec::new()
    }
    /// Messages per member since an India day: (member, 00-05h, 05-09h, all).
    fn hour_counts(&self, _since_day: &str) -> Vec<(u64, i64, i64, i64)> {
        Vec::new()
    }
    /// The bot's own user id.
    fn bot_id(&self) -> Option<u64> {
        None
    }
    /// The bot's stored requests since `since`, for filling in insights.
    async fn history_requests(
        &self,
        _since: i64,
        _progress: Arc<dyn Fn(usize) + Send + Sync>,
    ) -> anyhow::Result<Vec<super::insights::StoredRequest>> {
        anyhow::bail!("no stored history here")
    }
    /// One completion from the bot's model: the answer and the model's name.
    async fn ask_model(&self, _prompt: String) -> anyhow::Result<(String, String)> {
        anyhow::bail!("no model here")
    }
    // Richer scheduled posts.
    /// This month's House Cup table, for `{leader}` and `{standings}`.
    fn house_standings(&self, _now: i64) -> Vec<super::posts::Standing> {
        Vec::new()
    }
    /// Posts a reminder once, now, without touching its bookkeeping.
    async fn send_test_post(&self, _reminder: &Reminder) -> Result<(), String> {
        Err("Discord isn't connected right now.".into())
    }
    // Special welcomes.
    /// A member's joins and leaves from the join log.
    async fn join_summary(&self, _id: u64) -> Option<members::JoinSummary> {
        None
    }
    /// Any Discord user by id, whether or not they are in the server.
    async fn user(&self, _id: u64) -> Option<MemberInfo> {
        None
    }
    /// Posts text in a channel with no pings at all: a special welcome's test.
    async fn send_unpinged(&self, _channel: u64, _text: String) -> Result<(), String> {
        Err("Discord isn't connected right now.".into())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EmojiInfo {
    pub id: String,
    pub name: String,
    pub animated: bool,
    pub url: String,
}

mod agent;
mod houses;
mod insights;
mod media;
mod members;
mod memos;
mod posts;
mod profiles;
mod rules;
mod scorers;
mod welcomes;

// --- the live implementation ------------------------------------------------------

static CTX: OnceLock<Context> = OnceLock::new();

struct LiveData;

fn guild_id(ctx: &Context) -> Option<GuildId> {
    ctx.cache.guilds().first().copied()
}

fn member_info(m: &serenity::all::Member) -> MemberInfo {
    MemberInfo {
        id: m.user.id.get().to_string(),
        name: m.display_name().to_string(),
        username: m.user.name.clone(),
        avatar: m.user.face(),
        bot: m.user.bot,
    }
}

#[async_trait::async_trait]
impl PanelData for LiveData {
    fn guild(&self) -> Option<GuildInfo> {
        let ctx = CTX.get()?;
        let g = ctx.cache.guild(guild_id(ctx)?)?;
        Some(GuildInfo {
            id: g.id.get().to_string(),
            name: g.name.clone(),
            icon: g.icon_url(),
            members: g.member_count,
        })
    }

    fn channels(&self) -> Vec<ChannelInfo> {
        let Some(ctx) = CTX.get() else { return Vec::new() };
        let Some(g) = guild_id(ctx).and_then(|id| ctx.cache.guild(id)) else { return Vec::new() };
        g.channels
            .values()
            .map(|c| {
                let parent = c.parent_id.and_then(|p| g.channels.get(&p));
                ChannelInfo {
                    id: c.id.get().to_string(),
                    name: c.name.clone(),
                    kind: match c.kind {
                        ChannelType::Text | ChannelType::News => ChannelKind::Text,
                        ChannelType::Voice | ChannelType::Stage => ChannelKind::Voice,
                        ChannelType::Forum => ChannelKind::Forum,
                        ChannelType::Category => ChannelKind::Category,
                        _ => ChannelKind::Other,
                    },
                    category: parent.map(|p| p.name.clone()),
                    category_position: parent.map(|p| p.position as i64).unwrap_or(-1),
                    position: c.position as i64,
                }
            })
            .collect()
    }

    fn roles(&self) -> Vec<RoleInfo> {
        let Some(ctx) = CTX.get() else { return Vec::new() };
        let Some(g) = guild_id(ctx).and_then(|id| ctx.cache.guild(id)) else { return Vec::new() };
        g.roles
            .values()
            .filter(|r| r.id.get() != g.id.get())
            .map(|r| RoleInfo {
                id: r.id.get().to_string(),
                name: r.name.clone(),
                color: (r.colour.0 != 0).then(|| format!("#{:06x}", r.colour.0)),
                position: r.position as i64,
                managed: r.managed,
            })
            .collect()
    }

    async fn search_members(&self, query: &str, limit: usize) -> Vec<MemberInfo> {
        let Some(ctx) = CTX.get() else { return Vec::new() };
        let Some(gid) = guild_id(ctx) else { return Vec::new() };
        let q = query.to_lowercase();
        let mut found: Vec<MemberInfo> = match ctx.cache.guild(gid) {
            Some(g) => g
                .members
                .values()
                .filter(|m| {
                    q.is_empty()
                        || m.user.name.to_lowercase().contains(&q)
                        || m.display_name().to_lowercase().contains(&q)
                })
                .take(limit)
                .map(member_info)
                .collect(),
            None => Vec::new(),
        };
        if found.len() < limit && !q.is_empty() {
            if let Ok(more) = gid.search_members(&ctx.http, query, Some(limit as u64)).await {
                let seen: HashSet<String> = found.iter().map(|m| m.id.clone()).collect();
                found.extend(more.iter().map(member_info).filter(|m| !seen.contains(&m.id)));
            }
        }
        found.truncate(limit);
        found
    }

    async fn member(&self, id: u64) -> Option<MemberInfo> {
        let ctx = CTX.get()?;
        let gid = guild_id(ctx)?;
        let user = UserId::new(id);
        let cached = ctx.cache.guild(gid).and_then(|g| g.members.get(&user).map(member_info));
        match cached {
            Some(m) => Some(m),
            None => gid.member(ctx, user).await.ok().map(|m| member_info(&m)),
        }
    }

    fn admins(&self) -> Vec<u64> {
        super::super::admin_ids()
    }

    fn restart(&self) {
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tracing::warn!("panel: restarting at an admin's request");
            std::process::exit(0);
        });
    }

    fn emojis(&self) -> Vec<EmojiInfo> {
        let Some(ctx) = CTX.get() else { return Vec::new() };
        let Some(g) = guild_id(ctx).and_then(|id| ctx.cache.guild(id)) else { return Vec::new() };
        let mut list: Vec<EmojiInfo> = g
            .emojis
            .values()
            .filter(|e| e.available)
            .map(|e| EmojiInfo { id: e.id.get().to_string(), name: e.name.clone(), animated: e.animated, url: e.url() })
            .collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        list
    }

    fn cached_member(&self, id: u64) -> Option<MemberInfo> {
        let ctx = CTX.get()?;
        let g = ctx.cache.guild(guild_id(ctx)?)?;
        g.members.get(&UserId::new(id)).map(member_info)
    }

    fn house_cup(&self, period: houses::Period, now: i64) -> Option<houses::HouseCup> {
        houses::read_live(period, now)
    }

    async fn agent_settings(&self) -> Option<agent::AgentSettings> {
        use crate::storage::agent::AgentStorage;
        let (deps, agent_id) = AGENT.get()?;
        let config = deps.storage.get_agent(agent_id).await.ok().flatten()?;
        let core = deps
            .storage
            .get_agent_core(agent_id)
            .await
            .ok()
            .flatten()
            .or(config.core.clone())
            .unwrap_or_else(|| crate::constant::CORE_MD.to_string());
        Some(agent::AgentSettings {
            name: config.name,
            description: config.description.unwrap_or_default(),
            system_prompt: config.system_prompt.unwrap_or_default(),
            core,
            model: config.model,
            thinking_depth: config.thinking_depth,
            silent_read_initiative_chance: config.silent_read_initiative_chance,
            max_tokens: config.max_tokens,
        })
    }

    async fn member_detail(&self, id: u64) -> Option<members::MemberDetail> {
        let ctx = CTX.get()?;
        let gid = guild_id(ctx)?;
        let user = UserId::new(id);
        let cached = ctx.cache.guild(gid).and_then(|g| g.members.get(&user).cloned());
        let member = match cached {
            Some(m) => m,
            None => gid.member(ctx, user).await.ok()?,
        };
        Some(members::MemberDetail {
            info: member_info(&member),
            joined_at: member.joined_at.map(|t| t.unix_timestamp()),
            created_at: user.created_at().unix_timestamp(),
            role_ids: member.roles.iter().map(|r| r.get().to_string()).collect(),
        })
    }

    async fn member_stats(&self, id: u64, now: i64) -> members::MemberStats {
        members::read_stats(AGENT.get().map(|(deps, _)| deps), id, now).await
    }

    async fn member_seen(&self, id: u64, now: i64) -> Vec<members::SeenMessage> {
        match AGENT.get() {
            Some((deps, agent_id)) => members::read_seen(deps, agent_id, id, now).await,
            None => Vec::new(),
        }
    }

    async fn memories(&self) -> Vec<members::MemoryEntry> {
        match AGENT.get() {
            Some((deps, agent_id)) => members::read_memories(deps, agent_id).await,
            None => Vec::new(),
        }
    }

    fn scorers(&self, days_back: i64, now: i64) -> Option<Vec<scorers::ScorerData>> {
        scorers::read_live(days_back, now)
    }

    async fn activity(&self, now: i64) -> Vec<super::profiles::Activity> {
        tokio::task::spawn_blocking(move || profiles::read_activity_live(now)).await.unwrap_or_default()
    }

    async fn member_messages(&self, id: u64, now: i64) -> Vec<super::profiles::RawMessage> {
        let Some((deps, agent_id)) = AGENT.get() else { return Vec::new() };
        let Some(conn) = members::history_conn(deps) else { return Vec::new() };
        let agent = agent_id.clone();
        tokio::task::spawn_blocking(move || profiles::query_messages(&conn.lock(), &agent, id, now).unwrap_or_default())
            .await
            .unwrap_or_default()
    }

    fn thread_parent(&self, channel: u64) -> Option<u64> {
        let ctx = CTX.get()?;
        let g = ctx.cache.guild(guild_id(ctx)?)?;
        g.threads.iter().find(|t| t.id.get() == channel).and_then(|t| t.parent_id).map(|p| p.get())
    }

    fn duels(&self, since: i64) -> Vec<(u64, u64, i64)> {
        super::super::battle::duels_since(since)
    }

    fn hour_counts(&self, since_day: &str) -> Vec<(u64, i64, i64, i64)> {
        let Some(db) = super::super::stats::db() else { return Vec::new() };
        let conn = db.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT user_id, SUM(CASE WHEN hour < 5 THEN count ELSE 0 END), SUM(CASE WHEN hour >= 5 AND hour < 9 THEN count ELSE 0 END),
             SUM(count) FROM msg_counts WHERE day >= ?1 GROUP BY user_id",
        ) else {
            return Vec::new();
        };
        stmt.query_map(rusqlite::params![since_day], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    fn bot_id(&self) -> Option<u64> {
        CTX.get().map(|ctx| ctx.cache.current_user().id.get())
    }

    async fn history_requests(
        &self,
        since: i64,
        progress: Arc<dyn Fn(usize) + Send + Sync>,
    ) -> anyhow::Result<Vec<super::insights::StoredRequest>> {
        let (deps, agent_id) = AGENT.get().ok_or_else(|| anyhow::anyhow!("the agent isn't reachable"))?;
        insights::read_live_history(deps, agent_id, since, progress).await
    }

    async fn ask_model(&self, prompt: String) -> anyhow::Result<(String, String)> {
        use crate::storage::agent::AgentStorage;
        let (deps, agent_id) = AGENT.get().ok_or_else(|| anyhow::anyhow!("the agent isn't reachable"))?;
        let model = deps.storage.get_agent(agent_id).await.ok().flatten().map(|c| c.model).unwrap_or_default();
        let answer = super::super::weekly::ask_model(deps, agent_id, prompt).await?;
        Ok((answer, model))
    }

    fn house_standings(&self, now: i64) -> Vec<super::posts::Standing> {
        super::posts::standings_now(now)
    }

    async fn send_test_post(&self, reminder: &Reminder) -> Result<(), String> {
        let ctx = CTX.get().ok_or("Discord isn't connected right now.")?;
        super::scheduler::send_test(ctx, reminder).await
    }

    async fn join_summary(&self, id: u64) -> Option<members::JoinSummary> {
        let (deps, _) = AGENT.get()?;
        let log = super::super::joinlog_get(&deps.storage, id).await;
        (log.joins > 0 || log.leaves > 0).then(|| members::JoinSummary {
            joins: log.joins,
            leaves: log.leaves,
            first_join: log.first_join,
            last_join: log.last_join,
            last_leave: log.last_leave,
        })
    }

    async fn user(&self, id: u64) -> Option<MemberInfo> {
        let ctx = CTX.get()?;
        let user = tokio::time::timeout(Duration::from_secs(10), UserId::new(id).to_user(ctx)).await.ok()?.ok()?;
        Some(MemberInfo {
            id: user.id.get().to_string(),
            name: user.display_name().to_string(),
            username: user.name.clone(),
            avatar: user.face(),
            bot: user.bot,
        })
    }

    async fn send_unpinged(&self, channel: u64, text: String) -> Result<(), String> {
        let ctx = CTX.get().ok_or("Discord isn't connected right now.")?;
        super::welcomes::post_unpinged(ctx, channel, text).await
    }

    async fn save_agent_settings(&self, s: &agent::AgentSettings) -> anyhow::Result<()> {
        use crate::storage::agent::AgentStorage;
        let (deps, agent_id) = AGENT.get().ok_or_else(|| anyhow::anyhow!("the agent isn't reachable"))?;
        // Read fresh and change only these fields: everything else in the
        // config (provider, tokens, tools, owner) stays exactly as stored.
        let mut config =
            deps.storage.get_agent(agent_id).await?.ok_or_else(|| anyhow::anyhow!("agent {} not found", agent_id))?;
        let blank = |v: &str| (!v.trim().is_empty()).then(|| v.to_string());
        config.name = s.name.clone();
        config.description = blank(&s.description);
        config.system_prompt = blank(&s.system_prompt);
        config.model = s.model.clone();
        config.thinking_depth = s.thinking_depth;
        config.silent_read_initiative_chance = s.silent_read_initiative_chance;
        config.max_tokens = s.max_tokens;
        deps.storage.update_agent(agent_id, &config).await?;
        let stored_core = deps.storage.get_agent_core(agent_id).await.ok().flatten();
        if stored_core.as_deref() != Some(s.core.as_str()) {
            deps.storage.set_agent_core(agent_id, &s.core).await?;
        }
        Ok(())
    }
}

/// The agent's dependencies and id, for the Bot behaviour page.
static AGENT: OnceLock<(crate::dependencies::VizierDependencies, String)> = OnceLock::new();

/// One completion from the bot's model, for the scheduler's AI-written posts.
pub(crate) async fn ask_bot_model(prompt: String) -> anyhow::Result<String> {
    let (deps, agent_id) = AGENT.get().ok_or_else(|| anyhow::anyhow!("the agent isn't reachable"))?;
    super::super::weekly::ask_model(deps, agent_id, prompt).await
}

/// Starts the panel once per process and (re)registers `/panel`. Called from
/// `ready`, which fires again on every reconnect.
pub fn start(ctx: &Context, deps: &crate::dependencies::VizierDependencies, agent_id: &str) {
    let _ = AGENT.set((deps.clone(), agent_id.to_string()));
    let http = ctx.http.clone();
    tokio::spawn(async move {
        let _ = serenity::all::Command::create_global_command(http, command()).await;
    });
    if CTX.set(ctx.clone()).is_err() {
        return;
    }
    let bind = super::var("VIZIER_PANEL_BIND").unwrap_or_else(|| "127.0.0.1:8787".to_string());
    let panel = Panel::new(Arc::new(LiveData), catalog::sections);
    let app = router(panel.clone());
    // Insights: fill the reply counts in from the stored history once, after the
    // cache has had time to learn the channels.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        if super::insights::meta_get("backfill_done").is_none() {
            insights::start_rebuild(panel);
        }
    });
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(&bind).await {
            Ok(listener) => {
                tracing::info!("panel: listening on {}", bind);
                if let Err(err) =
                    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await
                {
                    tracing::error!("panel: server stopped: {}", err);
                }
            }
            Err(err) => tracing::error!("panel: cannot listen on {}: {}", bind, err),
        }
    });
}

// --- /panel ------------------------------------------------------------------------

pub fn command() -> CreateCommand {
    CreateCommand::new("panel").description("admin only: a sign-in link for the Loduchand control panel")
}

pub async fn on_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let mut message = CreateInteractionResponseMessage::new().ephemeral(true);
    if !super::super::admin_ids().contains(&user) {
        message = message.content("The control panel is for server admins only.");
    } else {
        match super::var("VIZIER_PANEL_URL").map(|u| u.trim_end_matches('/').to_string()) {
            None => {
                message = message.content("The panel has no public address yet: set `VIZIER_PANEL_URL`.");
            }
            Some(base) => match super::create_link(user) {
                None => message = message.content("Couldn't make a sign-in link just now. Try again."),
                Some(token) => {
                    let link = format!("{}/login#t={}", base, token);
                    message = message
                        .content(format!(
                            "Here's your sign-in link for the control panel. It works once, for {} minutes. \
                             Don't share it: whoever opens it can change the bot.",
                            super::LINK_SECS / 60
                        ))
                        .components(vec![CreateActionRow::Buttons(vec![
                            CreateButton::new_link(link).label("Open the control panel"),
                        ])]);
                }
            },
        }
    }
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

// --- the server ------------------------------------------------------------------------

#[derive(Clone)]
pub struct Panel {
    data: Arc<dyn PanelData>,
    catalog: fn() -> Vec<Section>,
    started: Instant,
    started_ts: i64,
    logins: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

impl Panel {
    pub fn new(data: Arc<dyn PanelData>, catalog: fn() -> Vec<Section>) -> Self {
        Self {
            data,
            catalog,
            started: Instant::now(),
            started_ts: chrono::Utc::now().timestamp(),
            logins: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn find(&self, key: &str) -> Option<(Section, Setting)> {
        (self.catalog)().into_iter().find_map(|section| {
            let setting = section.settings.iter().find(|s| s.key == key).cloned()?;
            Some((section, setting))
        })
    }

    /// True when the address may try another sign-in now.
    fn allow_login(&self, ip: &str) -> bool {
        let now = Instant::now();
        let mut logins = self.logins.lock();
        logins.retain(|_, hits| {
            while hits.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
                hits.pop_front();
            }
            !hits.is_empty()
        });
        let hits = logins.entry(ip.to_string()).or_default();
        if hits.len() >= LOGIN_PER_MINUTE {
            return false;
        }
        hits.push_back(now);
        true
    }
}

/// An error the API sends as `{"error": "..."}`.
#[derive(Debug)]
pub struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.into())
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, msg.into())
    }
    fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!("panel: {}", err);
        Self(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong saving that. Try again.".into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult = Result<Response, ApiError>;

fn ok(value: impl Serialize) -> ApiResult {
    Ok(axum::Json(value).into_response())
}

/// The signed-in admin, put on the request by [`require_admin`].
#[derive(Clone, Copy)]
struct Caller(u64);

pub fn router(panel: Panel) -> Router {
    let api = Router::new()
        .route("/me", get(me))
        .route("/status", get(status))
        .route("/catalog", get(catalog_json))
        .route("/settings/{key}", put(put_setting))
        .route("/discord/channels", get(channels))
        .route("/discord/roles", get(roles))
        .route("/discord/members", get(members))
        .route("/discord/members/{id}", get(member))
        .route("/reminders", get(list_reminders).post(create_reminder))
        .route("/reminders/{id}", get(get_reminder).put(update_reminder).delete(delete_reminder))
        .route("/reminders/{id}/toggle", post(toggle_reminder))
        // Richer scheduled posts, members' own reminders and the picture library.
        .route("/reminders/templates", get(posts::templates))
        .route("/reminders/placeholders", get(posts::placeholders))
        .route("/reminders/ai-preview", post(posts::ai_preview))
        .route("/reminders/{id}/test", post(posts::send_test))
        .route("/welcomes", get(welcomes::list).post(welcomes::create))
        .route("/welcomes/preview", post(welcomes::preview))
        .route("/welcomes/lookup", get(welcomes::lookup))
        .route("/welcomes/{id}", put(welcomes::update).delete(welcomes::delete))
        .route("/welcomes/{id}/test", post(welcomes::send_test))
        .route("/memos", get(memos::list).post(memos::create))
        .route("/memos/when", get(memos::when))
        .route("/memos/{id}/cancel", post(memos::cancel))
        .route("/media", get(media::list).post(media::upload).layer(axum::extract::DefaultBodyLimit::max(media::UPLOAD_LIMIT)))
        .route("/media/{id}", get(media::serve).delete(media::delete))
        .route("/audit", get(audit))
        .route("/restart", post(restart))
        .route("/discord/emojis", get(emojis))
        .route("/autoreplies", get(rules::list).post(rules::create))
        .route("/autoreplies/test", post(rules::test))
        .route("/autoreplies/{id}", get(rules::get).put(rules::update).delete(rules::delete))
        .route("/autoreplies/{id}/toggle", post(rules::toggle))
        .route("/houses", get(houses::get))
        .route("/agent", get(agent::get).put(agent::put))
        .route("/houses/scorers", get(scorers::get))
        .route("/insights", get(insights::overview))
        .route("/insights/pair", get(insights::pair))
        .route("/insights/rebuild", get(insights::rebuild_status).post(insights::rebuild))
        .route("/members/{id}/connections", get(insights::connections))
        .route("/profiles/active", get(profiles::active))
        .route("/profiles/analyse", post(profiles::start_job))
        .route("/profiles/job", get(profiles::get_job).delete(profiles::cancel_job))
        .route("/profiles/{id}", get(profiles::get).put(profiles::put).delete(profiles::delete))
        .route("/profiles/{id}/apply", post(profiles::apply))
        .route("/profiles/{id}/apply/preview", post(profiles::apply_preview))
        .route("/profiles/{id}/prompt", get(profiles::prompt_preview))
        .route("/members", get(members::search))
        .route("/members/notes", get(members::noted))
        .route("/members/notes/enable", post(members::enable_notes))
        .route("/members/{id}", get(members::profile))
        .route("/members/{id}/seen", get(members::seen))
        .route("/members/{id}/memories", get(members::memories))
        .route("/members/{id}/note", put(members::save_note).delete(members::delete_note))
        .route("/members/{id}/note/preview", get(members::preview_saved).post(members::preview_draft))
        .route_layer(middleware::from_fn_with_state(panel.clone(), require_admin))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .fallback(|| async { ApiError::not_found("No such endpoint.") })
        .layer(middleware::from_fn(require_panel_header))
        .layer(middleware::from_fn(no_store));

    Router::new()
        .route("/", get(page))
        .route("/login", get(page))
        .route("/assets/app.css", get(|| async { asset("text/css; charset=utf-8", APP_CSS) }))
        .route("/assets/app.js", get(|| async { asset("text/javascript; charset=utf-8", APP_JS) }))
        .route("/assets/favicon.svg", get(|| async { asset("image/svg+xml", FAVICON) }))
        .nest("/api", api)
        .fallback(|| async { (StatusCode::NOT_FOUND, "Not found") })
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(panel)
}

async fn page() -> Response {
    let mut res = Response::new(Body::from(INDEX_HTML));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

fn asset(kind: &'static str, body: &'static str) -> Response {
    let mut res = Response::new(Body::from(body));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' https://cdn.discordapp.com data:; style-src 'self' 'unsafe-inline'; \
             frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    res
}

async fn no_store(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    // Pictures from the library never change under their id, so they may say otherwise.
    if !res.headers().contains_key(header::CACHE_CONTROL) {
        res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

/// A change must say it comes from the panel's own page. A cross-site form can't
/// set a custom header, so this backs up the SameSite cookie.
async fn require_panel_header(req: Request, next: Next) -> Response {
    let reads = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !reads && req.headers().get("x-panel").and_then(|v| v.to_str().ok()) != Some("1") {
        return ApiError(StatusCode::FORBIDDEN, "Missing the X-Panel header.".into()).into_response();
    }
    next.run(req).await
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_string())
        .filter(|v| !v.is_empty())
}

async fn require_admin(State(panel): State<Panel>, mut req: Request, next: Next) -> Response {
    let Some(user) = session_cookie(req.headers()).and_then(|s| super::session_user(&s)) else {
        return ApiError(StatusCode::UNAUTHORIZED, "Sign in again: run /panel in Discord.".into()).into_response();
    };
    if !panel.data.admins().contains(&user) {
        return ApiError(StatusCode::FORBIDDEN, "You're no longer an admin, so the panel is closed to you.".into())
            .into_response();
    }
    req.extensions_mut().insert(Caller(user));
    next.run(req).await
}

fn client_ip(req_headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    // Caddy, in front, writes the real client as the last X-Forwarded-For entry.
    req_headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| peer.map(|p| p.ip().to_string()))
        .unwrap_or_else(|| "local".to_string())
}

fn cookie_header(value: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{}={}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
        COOKIE, value, max_age
    ))
    .expect("cookie is ascii")
}

// --- sign in and out -----------------------------------------------------------------

#[derive(Deserialize)]
struct LoginBody {
    token: String,
}

async fn login(State(panel): State<Panel>, req: Request) -> ApiResult {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = client_ip(req.headers(), peer);
    if !panel.allow_login(&ip) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Too many sign-in attempts. Wait a minute.".into()));
    }
    let bytes = axum::body::to_bytes(req.into_body(), 4096).await.map_err(|_| ApiError::bad("Bad request."))?;
    let body: LoginBody = serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("Bad request."))?;
    let token = body.token.trim();
    if token.is_empty() || token.len() > 128 {
        return Err(ApiError::bad("That sign-in link isn't valid. Run /panel in Discord for a new one."));
    }
    let Some((session, user)) = super::redeem_link(token) else {
        return Err(ApiError::bad(
            "That sign-in link has expired or was already used. Run /panel in Discord for a new one.",
        ));
    };
    if !panel.data.admins().contains(&user) {
        super::end_session(&session);
        return Err(ApiError(StatusCode::FORBIDDEN, "Only server admins can use the panel.".into()));
    }
    tracing::info!("panel: {} signed in", user);
    let mut res = axum::Json(me_json(&panel, user, &session).await).into_response();
    res.headers_mut().insert(header::SET_COOKIE, cookie_header(&session, super::SESSION_SECS));
    Ok(res)
}

async fn logout(headers: HeaderMap) -> Response {
    if let Some(session) = session_cookie(&headers) {
        super::end_session(&session);
    }
    let mut res = axum::Json(json!({ "ok": true })).into_response();
    res.headers_mut().insert(header::SET_COOKIE, cookie_header("", 0));
    res
}

async fn me_json(panel: &Panel, user: u64, session: &str) -> Value {
    let member = panel.data.member(user).await;
    json!({
        "id": user.to_string(),
        "name": member.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "Admin".to_string()),
        "avatar": member.map(|m| m.avatar),
        "session_expires": super::session_expires(session),
    })
}

async fn me(State(panel): State<Panel>, headers: HeaderMap, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let session = session_cookie(&headers).unwrap_or_default();
    ok(me_json(&panel, user, &session).await)
}

// --- status and catalog -------------------------------------------------------------

fn first_toggle(section: &Section) -> Option<&Setting> {
    section.settings.iter().find(|s| matches!(s.kind, Kind::Toggle))
}

fn toggle_default(setting: &Setting) -> bool {
    matches!(setting.default.to_ascii_lowercase().as_str(), "on" | "true" | "yes" | "1")
}

async fn status(State(panel): State<Panel>) -> ApiResult {
    let sections: Vec<Value> = (panel.catalog)()
        .iter()
        .map(|s| {
            let toggle = first_toggle(s);
            json!({
                "id": s.id,
                "title": s.title,
                "icon": s.icon,
                "toggle_key": toggle.map(|t| t.key),
                "enabled": toggle.map(|t| super::on(t.key, toggle_default(t))),
                "settings": s.settings.len(),
                "commands": s.commands.len(),
            })
        })
        .collect();
    let all = reminders::list();
    ok(json!({
        "bot": {
            "name": BOT_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": panel.started.elapsed().as_secs(),
            "started": panel.started_ts,
        },
        "guild": panel.data.guild(),
        "sections": sections,
        "reminders": { "total": all.len(), "active": all.iter().filter(|r| r.enabled).count() },
        "panel_url": super::var("VIZIER_PANEL_URL"),
        "notes_to_review": super::members::list().iter().filter(|n| n.awaits_review()).count(),
    }))
}

fn setting_json(s: &Setting) -> Value {
    json!({
        "key": s.key,
        "label": s.label,
        "help": s.help,
        "kind": s.kind,
        "default": s.default,
        "live": s.live,
        "value": super::var(s.key),
        "origin": super::origin(s.key),
        // Only ever for keys the catalog lists.
        "env_value": std::env::var(s.key).ok().filter(|v| !v.trim().is_empty()),
    })
}

async fn catalog_json(State(panel): State<Panel>) -> ApiResult {
    let sections: Vec<Value> = (panel.catalog)()
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "title": s.title,
                "icon": s.icon,
                "about": s.about,
                "settings": s.settings.iter().map(setting_json).collect::<Vec<_>>(),
                "commands": s.commands,
            })
        })
        .collect();
    ok(json!({ "sections": sections }))
}

// --- settings --------------------------------------------------------------------------

#[derive(Deserialize)]
struct SettingBody {
    #[serde(default)]
    value: Option<Value>,
}

fn parse_id(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    (!raw.is_empty() && raw.len() <= 20 && raw.bytes().all(|b| b.is_ascii_digit())).then(|| raw.parse().ok()).flatten()
}

/// "HH:MM", made two-digit. `None` when it isn't a time of day.
pub fn parse_time(raw: &str) -> Option<String> {
    let (h, m) = raw.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60 && raw.trim().len() <= 5).then(|| format!("{:02}:{:02}", h, m))
}

fn check_channel(panel: &Panel, raw: &str, voice: bool) -> Result<String, String> {
    let id = parse_id(raw).ok_or_else(|| format!("\"{}\" isn't a channel id.", raw.trim()))?;
    let channels = panel.data.channels();
    if channels.is_empty() {
        return Ok(id.to_string());
    }
    let Some(channel) = channels.iter().find(|c| c.id == id.to_string()) else {
        return Err(format!("There's no channel {} in the server.", id));
    };
    let fits = if voice { channel.kind == ChannelKind::Voice } else { channel.kind == ChannelKind::Text };
    if !fits {
        return Err(format!(
            "#{} is not a {} channel.",
            channel.name,
            if voice { "voice" } else { "text" }
        ));
    }
    Ok(id.to_string())
}

fn split_list(raw: &str) -> impl Iterator<Item = &str> {
    raw.split(',').map(str::trim).filter(|p| !p.is_empty())
}

/// Checks a value against the setting's kind and returns it the way it's stored.
async fn validate(panel: &Panel, setting: &Setting, raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.len() > 4000 {
        return Err("That's too long.".into());
    }
    let empty_ok = matches!(
        setting.kind,
        Kind::Text | Kind::Channel | Kind::Channels | Kind::WeightedChannels | Kind::VoiceChannel | Kind::Role | Kind::Users | Kind::Time
    );
    if value.is_empty() {
        return if empty_ok { Ok(String::new()) } else { Err(format!("{} needs a value.", setting.label)) };
    }
    match &setting.kind {
        Kind::Toggle => match value.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Ok("on".into()),
            "off" | "false" | "no" | "0" => Ok("off".into()),
            _ => Err("A switch is either on or off.".into()),
        },
        Kind::Channel => check_channel(panel, value, false),
        Kind::VoiceChannel => check_channel(panel, value, true),
        Kind::Channels => {
            let mut out = Vec::new();
            for part in split_list(value) {
                let id = check_channel(panel, part, false)?;
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            Ok(out.join(","))
        }
        Kind::WeightedChannels => {
            let mut out: Vec<String> = Vec::new();
            for part in split_list(value) {
                let (id, weight) = match part.split_once(':') {
                    Some((id, w)) => {
                        let w: u32 = w.trim().parse().map_err(|_| format!("\"{}\" needs a whole-number weight.", part))?;
                        (id, w)
                    }
                    None => (part, 1),
                };
                if !(1..=1000).contains(&weight) {
                    return Err("Weights go from 1 to 1000.".into());
                }
                let id = check_channel(panel, id, false)?;
                if out.iter().any(|o| o.starts_with(&format!("{}:", id))) {
                    return Err("A channel is listed twice.".into());
                }
                out.push(format!("{}:{}", id, weight));
            }
            Ok(out.join(","))
        }
        Kind::Role => {
            let id = parse_id(value).ok_or("That isn't a role id.")?;
            let roles = panel.data.roles();
            if !roles.is_empty() && !roles.iter().any(|r| r.id == id.to_string()) {
                return Err(format!("There's no role {} in the server.", id));
            }
            Ok(id.to_string())
        }
        Kind::Users => {
            let mut out = Vec::new();
            for part in split_list(value) {
                let id = parse_id(part).ok_or_else(|| format!("\"{}\" isn't a member id.", part))?;
                if panel.data.member(id).await.is_none() {
                    return Err(format!("There's no member {} in the server.", id));
                }
                if !out.contains(&id.to_string()) {
                    out.push(id.to_string());
                }
            }
            Ok(out.join(","))
        }
        Kind::Number { min, max, unit } => {
            let n: i64 = value.parse().map_err(|_| format!("{} must be a whole number.", setting.label))?;
            if n < *min || n > *max {
                return Err(format!("{} must be between {} and {} {}.", setting.label, min, max, unit).trim_end().replace(" .", "."));
            }
            Ok(n.to_string())
        }
        Kind::Decimal { min, max, unit } => {
            let n: f64 = value.parse().map_err(|_| format!("{} must be a number.", setting.label))?;
            if !n.is_finite() || n < *min || n > *max {
                return Err(format!("{} must be between {} and {} {}.", setting.label, min, max, unit).trim_end().replace(" .", "."));
            }
            Ok(value.to_string())
        }
        Kind::Text => Ok(value.to_string()),
        Kind::Time => parse_time(value).ok_or_else(|| "Use a time like 09:30.".to_string()),
        Kind::Times => {
            let mut times = Vec::new();
            for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let t = parse_time(part).ok_or_else(|| format!("\"{}\" isn't a time like 09:30.", part))?;
                if !times.contains(&t) {
                    times.push(t);
                }
            }
            if times.is_empty() {
                return Err("Add at least one time.".into());
            }
            if times.len() > 24 {
                return Err("That's more than 24 times a day.".into());
            }
            times.sort();
            Ok(times.join(","))
        }
        Kind::Choice { options } => options
            .iter()
            .find(|(v, _)| *v == value)
            .map(|(v, _)| v.to_string())
            .ok_or_else(|| "Pick one of the listed options.".to_string()),
    }
}

async fn put_setting(
    State(panel): State<Panel>,
    Path(key): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let Some((_, setting)) = panel.find(&key) else {
        return Err(ApiError::not_found("The panel doesn't manage that setting."));
    };
    let body: SettingBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Send {\"value\": ...}."))?;
    let stored = match body.value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(validate(&panel, &setting, &s).await.map_err(ApiError::bad)?),
        Some(Value::Bool(b)) => Some(validate(&panel, &setting, if b { "on" } else { "off" }).await.map_err(ApiError::bad)?),
        Some(Value::Number(n)) => Some(validate(&panel, &setting, &n.to_string()).await.map_err(ApiError::bad)?),
        Some(_) => return Err(ApiError::bad("A setting's value is text.")),
    };
    super::set(setting.key, stored.as_deref(), user).map_err(ApiError::internal)?;
    tracing::info!("panel: {} set {} to {:?}", user, setting.key, stored);
    ok(setting_json(&setting))
}

// --- Discord lookups -------------------------------------------------------------------

async fn channels(State(panel): State<Panel>) -> ApiResult {
    let mut list = panel.data.channels();
    list.sort_by_key(|c| {
        let group = if c.kind == ChannelKind::Category { (c.position, -1) } else { (c.category_position, c.position) };
        (group.0, group.1, c.kind == ChannelKind::Voice, c.position)
    });
    ok(list)
}

async fn roles(State(panel): State<Panel>) -> ApiResult {
    let mut list = panel.data.roles();
    list.sort_by_key(|r| std::cmp::Reverse(r.position));
    ok(list)
}

#[derive(Deserialize)]
struct MemberQuery {
    #[serde(default)]
    q: String,
}

async fn members(State(panel): State<Panel>, Query(q): Query<MemberQuery>) -> ApiResult {
    let query: String = q.q.trim().chars().take(64).collect();
    ok(panel.data.search_members(&query, 25).await)
}

async fn member(State(panel): State<Panel>, Path(id): Path<String>) -> ApiResult {
    let id = parse_id(&id).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
    match panel.data.member(id).await {
        Some(m) => ok(m),
        None => Err(ApiError::not_found("No such member in the server.")),
    }
}

// --- reminders ----------------------------------------------------------------------------

fn parse_moment(raw: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(raw.trim()).ok()
}

/// Checks a reminder from the panel and tidies it into the stored shape.
async fn check_reminder(panel: &Panel, mut r: Reminder) -> Result<Reminder, ApiError> {
    r.name = r.name.trim().to_string();
    if r.name.is_empty() {
        return Err(ApiError::bad("Give the reminder a name."));
    }
    if r.name.chars().count() > 100 {
        return Err(ApiError::bad("Keep the name under 100 characters."));
    }
    r.channel_id = check_channel(panel, &r.channel_id, false).map_err(|e| {
        ApiError::bad(if r.channel_id.trim().is_empty() { "Pick a channel to post in.".to_string() } else { e })
    })?;
    r.lines = r.lines.iter().map(|l| l.trim_end().to_string()).filter(|l| !l.trim().is_empty()).collect();
    posts::check_extras(&mut r)?;
    if r.lines.is_empty() && r.ai_prompt.is_empty() && r.images.is_empty() {
        return Err(ApiError::bad("Add at least one message line."));
    }
    if r.lines.len() > 200 {
        return Err(ApiError::bad("That's more than 200 lines."));
    }
    if let Some(n) = r.lines.iter().position(|l| l.chars().count() > 1800) {
        return Err(ApiError::bad(format!("Line {} is longer than 1800 characters.", n + 1)));
    }
    match &mut r.schedule {
        Schedule::Every { minutes } => {
            if !(5..=10080).contains(minutes) {
                return Err(ApiError::bad("Post at most every 5 minutes and at least once a week."));
            }
        }
        Schedule::Daily { times } => {
            let mut clean = Vec::new();
            for t in times.iter() {
                let t = parse_time(t).ok_or_else(|| ApiError::bad(format!("\"{}\" isn't a time like 09:30.", t)))?;
                if !clean.contains(&t) {
                    clean.push(t);
                }
            }
            if clean.is_empty() {
                return Err(ApiError::bad("Add at least one time of day."));
            }
            clean.sort();
            *times = clean;
        }
    }
    let (from, to) = (r.active_from.trim().to_string(), r.active_to.trim().to_string());
    if from.is_empty() != to.is_empty() {
        return Err(ApiError::bad("Active hours need both a start and an end, or neither."));
    }
    if !from.is_empty() {
        r.active_from = parse_time(&from).ok_or_else(|| ApiError::bad("Active hours start isn't a time."))?;
        r.active_to = parse_time(&to).ok_or_else(|| ApiError::bad("Active hours end isn't a time."))?;
    } else {
        r.active_from.clear();
        r.active_to.clear();
    }
    r.user_id = r.user_id.trim().to_string();
    if !r.user_id.is_empty() {
        let id = parse_id(&r.user_id).ok_or_else(|| ApiError::bad("The member isn't a valid id."))?;
        r.user_id = id.to_string();
    }
    r.user_name = r.user_name.trim().chars().take(100).collect();
    for (field, label) in [(&mut r.since, "The \"since\" date"), (&mut r.ends, "The end date")] {
        *field = field.trim().to_string();
        if !field.is_empty() && parse_moment(field).is_none() {
            return Err(ApiError::bad(format!("{} isn't a valid date and time.", label)));
        }
    }
    if r.stop_when_back && r.user_id.is_empty() {
        return Err(ApiError::bad("Pick the member to watch for before choosing to stop when they're back."));
    }
    r.welcome_line = r.welcome_line.trim().to_string();
    if r.welcome_line.chars().count() > 1800 {
        return Err(ApiError::bad("The welcome line is longer than 1800 characters."));
    }
    Ok(r)
}

fn reminder_from(body: &[u8]) -> Result<Reminder, ApiError> {
    let mut value: Value = serde_json::from_slice(body).map_err(|_| ApiError::bad("That isn't valid JSON."))?;
    if let Some(obj) = value.as_object_mut() {
        obj.entry("id").or_insert(json!(0));
        obj.entry("enabled").or_insert(json!(true));
    }
    serde_json::from_value(value).map_err(|e| ApiError::bad(format!("The reminder is missing something: {}", e)))
}

fn reminder_id(raw: &str) -> Result<i64, ApiError> {
    raw.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| ApiError::not_found("No such reminder."))
}

async fn list_reminders() -> ApiResult {
    ok(reminders::list())
}

async fn get_reminder(Path(id): Path<String>) -> ApiResult {
    reminders::get(reminder_id(&id)?).map(|r| axum::Json(r).into_response()).ok_or_else(|| ApiError::not_found("No such reminder."))
}

async fn create_reminder(
    State(panel): State<Panel>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let mut r = check_reminder(&panel, reminder_from(&body)?).await?;
    r.id = 0;
    r.last_sent = 0;
    r.sent_count = 0;
    r.last_message_id.clear();
    r.ai_recent.clear();
    let id = reminders::save(&r, user).map_err(ApiError::internal)?;
    let saved = reminders::get(id).ok_or_else(|| ApiError::internal("reminder vanished after saving"))?;
    Ok((StatusCode::CREATED, axum::Json(saved)).into_response())
}

async fn update_reminder(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let id = reminder_id(&id)?;
    let old = reminders::get(id).ok_or_else(|| ApiError::not_found("No such reminder."))?;
    let mut r = check_reminder(&panel, reminder_from(&body)?).await?;
    // The scheduler keeps these; the page can't rewrite them.
    r.id = id;
    r.last_sent = old.last_sent;
    r.sent_count = old.sent_count;
    // A post in another channel can't be tidied from this one.
    r.last_message_id = if r.channel_id == old.channel_id { old.last_message_id } else { String::new() };
    r.ai_recent = old.ai_recent;
    reminders::save(&r, user).map_err(ApiError::internal)?;
    ok(reminders::get(id))
}

async fn toggle_reminder(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let mut r = reminders::get(reminder_id(&id)?).ok_or_else(|| ApiError::not_found("No such reminder."))?;
    r.enabled = !r.enabled;
    reminders::save(&r, user).map_err(ApiError::internal)?;
    ok(r)
}

async fn delete_reminder(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = reminder_id(&id)?;
    if reminders::get(id).is_none() {
        return Err(ApiError::not_found("No such reminder."));
    }
    reminders::delete(id, user).map_err(ApiError::internal)?;
    ok(json!({ "ok": true }))
}

// --- audit and restart ---------------------------------------------------------------------

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<usize>,
}

/// The name and switch of a stored reminder or auto-response body.
fn rule_name(body: Option<&str>) -> Option<(String, bool)> {
    let v: Value = serde_json::from_str(body?).ok()?;
    Some((v.get("name")?.as_str()?.to_string(), v.get("enabled").and_then(Value::as_bool).unwrap_or(true)))
}

async fn emojis(State(panel): State<Panel>) -> ApiResult {
    ok(panel.data.emojis())
}

async fn audit(State(panel): State<Panel>, Query(q): Query<AuditQuery>) -> ApiResult {
    let entries = super::audit(q.limit.unwrap_or(100).clamp(1, 1000));
    let sections = (panel.catalog)();
    let mut people: HashMap<String, Option<MemberInfo>> = HashMap::new();
    for e in &entries {
        if !people.contains_key(&e.user_id) {
            let found = match e.user_id.parse::<u64>() {
                Ok(id) if id != 0 => panel.data.member(id).await,
                _ => None,
            };
            people.insert(e.user_id.clone(), found);
        }
    }
    let out: Vec<Value> = entries
        .iter()
        .map(|e| {
            let who = people.get(&e.user_id).cloned().flatten();
            let base = json!({
                "ts": e.ts,
                "user_id": e.user_id,
                "user_name": who.as_ref().map(|m| m.name.clone()).or_else(|| {
                    (e.user_id == super::members::AUTO_FILL_BY.to_string()).then(|| "Auto-fill (analysis)".to_string())
                }),
                "system": e.user_id == super::members::AUTO_FILL_BY.to_string(),
                "user_avatar": who.map(|m| m.avatar),
                "key": e.key,
            });
            let mut obj = base.as_object().cloned().unwrap_or_default();
            let rule = e
                .key
                .strip_prefix("reminder:")
                .map(|id| (id, "Reminder", json!({ "id": "reminders", "title": "Reminders", "icon": "⏰" })))
                .or_else(|| {
                    e.key.strip_prefix("autoreply:").map(|id| {
                        (id, "Auto-response", json!({ "id": "autoreplies", "title": "Auto-responses", "icon": "💬" }))
                    })
                });
            if let Some((rid, noun, section)) = rule {
                let old = rule_name(e.old.as_deref());
                let new = rule_name(e.new.as_deref());
                let name = new.as_ref().or(old.as_ref()).map(|n| n.0.clone()).unwrap_or_else(|| format!("#{}", rid));
                let change = match (&old, &new) {
                    (None, Some(_)) => "Created",
                    (Some(_), None) => "Deleted",
                    (Some(o), Some(n)) if o.1 != n.1 => if n.1 { "Switched on" } else { "Switched off" },
                    _ => "Edited",
                };
                obj.insert("label".into(), json!(format!("{} “{}”", noun, name)));
                obj.insert("section".into(), section);
                obj.insert("change".into(), json!(change));
                obj.insert("old".into(), Value::Null);
                obj.insert("new".into(), Value::Null);
            } else if e.key.starts_with("welcome:") {
                obj.extend(welcomes::audit_entry(&panel, e));
            } else if e.key.starts_with("memo:") || e.key.starts_with("media:") {
                let entry = posts::audit_entry(&panel, e);
                obj.extend(entry);
            } else if let Some(uid) = e.key.strip_prefix("profile:") {
                let note: Value = e.new.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
                let name = uid
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| panel.data.cached_member(id))
                    .map(|m| m.name)
                    .or_else(|| note.get("name").and_then(Value::as_str).map(String::from))
                    .unwrap_or_else(|| uid.to_string());
                let fields: Vec<&str> = note
                    .get("fields")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(super::profiles::field_label).collect())
                    .unwrap_or_default();
                let with = |what: &str| if fields.is_empty() { what.to_string() } else { format!("{}: {}", what, fields.join(", ")) };
                let change = match note.get("action").and_then(Value::as_str).unwrap_or("") {
                    "generated" => "Generated".to_string(),
                    "edited" => with("Edited"),
                    "reviewed" => "Marked reviewed".to_string(),
                    "draft" => "Back to draft".to_string(),
                    "applied" => with("Added to notes"),
                    "deleted" => "Deleted".to_string(),
                    other => other.to_string(),
                };
                obj.insert("label".into(), json!(format!("Analysis of @{}", name)));
                obj.insert("member_id".into(), json!(uid));
                obj.insert("section".into(), json!({ "id": "members", "title": "Members", "icon": "👤" }));
                obj.insert("change".into(), json!(change));
                obj.insert("old".into(), Value::Null);
                obj.insert("new".into(), Value::Null);
            } else if let Some(uid) = e.key.strip_prefix("member:") {
                let body = |b: &Option<String>| b.as_deref().and_then(|t| serde_json::from_str::<super::members::MemberNote>(t).ok());
                let (old, new) = (body(&e.old), body(&e.new));
                let name = uid
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| panel.data.cached_member(id))
                    .map(|m| m.name)
                    .or_else(|| new.as_ref().or(old.as_ref()).map(|n| n.name.clone()))
                    .unwrap_or_else(|| uid.to_string());
                let auto = e.user_id == super::members::AUTO_FILL_BY.to_string();
                let change = match (&old, &new) {
                    (_, Some(_)) if auto => "Filled in from the analysis (off until reviewed)",
                    (Some(o), Some(n)) if !o.use_in_replies && n.use_in_replies && o.notes == n.notes => "Reviewed and switched on",
                    (None, Some(_)) => "Created",
                    (Some(_), None) => "Deleted",
                    (Some(o), Some(n)) if o.use_in_replies != n.use_in_replies => {
                        if n.use_in_replies { "Switched on" } else { "Switched off" }
                    }
                    (Some(o), Some(n)) if o.tone != n.tone && o.notes == n.notes => "Tone changed",
                    _ => "Edited",
                };
                obj.insert("label".into(), json!(format!("Notes for @{}", name)));
                obj.insert("member_id".into(), json!(uid));
                obj.insert("section".into(), json!({ "id": "members", "title": "Members", "icon": "👤" }));
                obj.insert("change".into(), json!(change));
                obj.insert("old".into(), Value::Null);
                obj.insert("new".into(), Value::Null);
            } else if let Some(field) = e.key.strip_prefix("agent:") {
                obj.insert("label".into(), json!(agent::label(field)));
                obj.insert("section".into(), json!({ "id": "agent", "title": "Bot behaviour", "icon": "🤖" }));
                if matches!(field, "system_prompt" | "core") {
                    // Long texts: say by how much, not the whole thing.
                    let len = |v: &Option<String>| v.as_deref().map(|t| t.chars().count()).unwrap_or(0);
                    obj.insert(
                        "change".into(),
                        json!(format!("Edited · {} → {} characters", len(&e.old), len(&e.new))),
                    );
                    obj.insert("old".into(), Value::Null);
                    obj.insert("new".into(), Value::Null);
                } else {
                    let shown = |v: &Option<String>| -> Value {
                        match (field, v.as_deref()) {
                            ("silent_read_initiative_chance", Some(t)) => {
                                json!(t.parse::<f64>().map(|n| format!("{}%", (n * 1000.0).round() / 10.0)).unwrap_or(t.to_string()))
                            }
                            ("max_tokens", None) => json!("provider default"),
                            (_, Some(t)) => json!(t),
                            (_, None) => json!(""),
                        }
                    };
                    obj.insert("kind".into(), json!({ "type": "text" }));
                    obj.insert("old".into(), shown(&e.old));
                    obj.insert("new".into(), shown(&e.new));
                    obj.insert("change".into(), json!("Changed"));
                }
            } else {
                let found = sections.iter().find_map(|s| s.settings.iter().find(|x| x.key == e.key).map(|x| (s, x)));
                match found {
                    Some((section, setting)) => {
                        obj.insert("label".into(), json!(setting.label));
                        obj.insert("kind".into(), json!(setting.kind));
                        obj.insert("section".into(), json!({ "id": section.id, "title": section.title, "icon": section.icon }));
                        obj.insert("old".into(), json!(e.old));
                        obj.insert("new".into(), json!(e.new));
                        obj.insert(
                            "change".into(),
                            json!(if e.new.is_none() { "Reset to .env" } else { "Changed" }),
                        );
                    }
                    None => {
                        // A key the catalog no longer lists: say that it changed, never what to.
                        obj.insert("label".into(), json!(e.key));
                        obj.insert("section".into(), Value::Null);
                        obj.insert("old".into(), Value::Null);
                        obj.insert("new".into(), Value::Null);
                        obj.insert("change".into(), json!("Changed"));
                    }
                }
            }
            Value::Object(obj)
        })
        .collect();
    ok(out)
}

async fn restart(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    tracing::warn!("panel: restart requested by {}", user);
    panel.data.restart();
    ok(json!({ "ok": true, "back_in_secs": 15 }))
}

#[cfg(test)]
mod tests;
