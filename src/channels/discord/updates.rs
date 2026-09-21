//! Telling the rooms the bot is going away, and telling them it is back.
//!
//! A deploy lands in the middle of whatever people were doing: a round is up,
//! somebody has just taken a hint, and then the bot simply stops answering. The
//! rounds themselves survive - every game writes its state to its own database
//! and picks it up again on boot - so the only thing missing was ever a word to
//! the room.
//!
//! Two messages, in every channel that has a game running in it:
//!
//! * going down, sent from the signal handler before the process exits;
//! * back up, sent once the games have started again.
//!
//! The "going down" one is best effort by nature. A `kill -9`, a crash or the
//! machine losing power leaves no chance to say anything, and that is worth
//! knowing rather than pretending otherwise: silence still happens, it is just
//! no longer the normal case.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, Http};

/// The http client, kept from `ready` so the shutdown path can reach Discord
/// without going through the channel task that is being torn down.
static HTTP: OnceLock<Arc<Http>> = OnceLock::new();

/// Where the note saying "this one was planned" is left for the next process.
static MARKER: OnceLock<PathBuf> = OnceLock::new();

/// How long one message may take. A shutdown cannot wait on a slow API call.
const SEND_WAIT: Duration = Duration::from_secs(5);

pub const GOING_DOWN: &str = "🔧 **The bot is being updated.** Back in a moment - please hang on.\n-# The round that is up is saved and will be here when it returns.";
pub const BACK_UP: &str = "✅ **Back up and running.** The game carries on from where it was.";

pub fn remember(http: Arc<Http>) {
    let _ = HTTP.set(http);
}

/// Where the restart note lives. Called at startup, with the same workspace
/// every store uses.
pub fn open(workspace: &str) {
    let _ = MARKER.set(crate::utils::build_path(workspace, &[".runtime"]).join("restarting"));
}

/// Leaves the note that says this shutdown was asked for. The next process
/// reads it, says the bot is back and takes it away; a crash leaves no note,
/// so a restart loop never fills the rooms with "back up" messages.
fn mark() {
    let Some(path) = MARKER.get() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(err) = std::fs::write(path, chrono::Utc::now().timestamp().to_string()) {
        tracing::warn!("updates: couldn't leave the restart note: {}", err);
    }
}

/// Whether the process before this one went down on purpose. Takes the note
/// away, so the answer is yes exactly once.
fn was_planned() -> bool {
    let Some(path) = MARKER.get() else { return false };
    path.exists() && std::fs::remove_file(path).is_ok()
}

/// Whether restarts are announced at all, `VIZIER_RESTART_NOTICES`. Off unless
/// switched on: the owner would rather a restart went by quietly than put two
/// messages into every game room each time.
fn announcing() -> bool {
    super::control::on("VIZIER_RESTART_NOTICES", false)
}

/// Every channel with a game live in it right now. A game that is switched off,
/// or has no channel set, is not in the list - there is nobody in there to tell.
pub fn game_channels() -> Vec<u64> {
    let mut out: Vec<u64> = [
        super::movie::live_channel(),
        super::guess::live_channel(),
        super::anagram::live_channel(),
        super::sudoku::live_channel(),
        super::chess::live_channel(),
        super::duel::live_channel(),
        super::npat::live_channel(),
        super::puzzle::live_channel(),
    ]
    .into_iter()
    .flatten()
    .collect();
    out.sort_unstable();
    out.dedup();
    out
}

async fn say(http: &Arc<Http>, channel: u64, text: &str) {
    let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    let send = ChannelId::new(channel).send_message(http, message);
    match tokio::time::timeout(SEND_WAIT, send).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => tracing::warn!("updates: couldn't tell {}: {}", channel, err),
        Err(_) => tracing::warn!("updates: telling {} took too long", channel),
    }
}

/// Tell every live game room the bot is going away. Called from the signal
/// handler, so it is deliberately bounded: each message has a few seconds and
/// the whole thing gives up rather than hold the process open.
pub async fn going_down() {
    mark();
    if !announcing() {
        return;
    }
    let Some(http) = HTTP.get() else { return };
    let channels = game_channels();
    if channels.is_empty() {
        return;
    }
    tracing::info!("updates: telling {} game channels the bot is going down", channels.len());
    for channel in channels {
        say(http, channel, GOING_DOWN).await;
    }
}

/// The note on its own, for a shutdown that cannot reach Discord in time. It
/// still counts as planned, so the next process says the bot is back.
pub fn mark_planned() {
    mark();
}

/// And tell them it is back, once the games have been started again - but only
/// when the process before this one said it was going. After a crash the rooms
/// were never told anything, and a bot that quietly reappears is better than
/// one announcing itself every time it falls over.
pub async fn back_up() {
    // The note is taken away either way, so switching the notices back on later
    // doesn't announce a restart that happened while they were off.
    if !was_planned() || !announcing() {
        return;
    }
    let Some(http) = HTTP.get() else { return };
    let channels = game_channels();
    if channels.is_empty() {
        return;
    }
    tracing::info!("updates: telling {} game channels the bot is live", channels.len());
    for channel in channels {
        say(http, channel, BACK_UP).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_messages_say_what_they_are_for() {
        // The going-down one has to promise the round survives, because that is
        // the thing people in the middle of one actually worry about.
        assert!(GOING_DOWN.contains("updated") && GOING_DOWN.contains("saved"));
        assert!(BACK_UP.contains("Back up"));
    }

    #[test]
    fn the_note_is_read_once_and_a_crash_leaves_none() {
        let dir = std::env::temp_dir().join(format!("vizier-updates-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("restarting");
        let _ = MARKER.set(path.clone());
        // Nothing left behind by a crash: nobody is told anything.
        assert!(!was_planned());
        mark();
        assert!(path.exists());
        // Asked for: said once, and the note is gone afterwards.
        assert!(was_planned());
        assert!(!was_planned());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_games_on_there_is_nobody_to_tell() {
        // Every game reads its own settings, and in a test run none is
        // configured, so the list is empty and both calls do nothing at all.
        assert!(game_channels().is_empty());
    }
}
