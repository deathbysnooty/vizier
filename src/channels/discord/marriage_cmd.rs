//! The commands: `/marry`, `/divorce`, `/family` and `/love <gesture>`.
//!
//! WHY ONE `/love` WITH SUBCOMMANDS. Discord allows a hundred global commands
//! and this bot already builds ninety-three. Sixteen more would have put it
//! past the ceiling, and going past it does not fail politely: the bulk
//! registration is refused outright and NOTHING new appears, which is how
//! `/egg` and `/dragon` went missing on hatch day. Subcommands cost nothing
//! against that limit, so only the three a member types unprompted are
//! top-level.
use serenity::all::{
    ButtonStyle, CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateButton, CreateCommand, CreateCommandOption, CreateEmbed,
    CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
};

use super::marriage::{self, Refusal};
use super::marriage_store as store;

const ASK_MARRY: &str = "love:marry:";
const ASK_ADOPT: &str = "love:adopt:";
const SAY_NO: &str = "love:no:";

/// The gestures that are only words and a picture.
const GESTURES: [(&str, &str); 5] = [
    ("hug", "hugs"),
    ("kiss", "kisses"),
    ("cuddle", "curls up with"),
    ("flirt", "is flirting with"),
    ("gift", "sends a gift to"),
];

pub fn builders() -> Vec<CreateCommand> {
    let who = |name: &str, about: &str| {
        CreateCommandOption::new(CommandOptionType::User, name, about).required(true)
    };
    let sub = |name: &'static str, about: &'static str| {
        CreateCommandOption::new(CommandOptionType::SubCommand, name, about)
    };
    let mut love = CreateCommand::new("love").description("everything else: gestures, families and who suits whom");
    for (name, _) in GESTURES {
        let about: &'static str = match name {
            "hug" => "hug somebody",
            "kiss" => "kiss somebody",
            "cuddle" => "curl up with somebody",
            "flirt" => "flirt, badly",
            _ => "send somebody a rose",
        };
        love = love.add_option(sub(name, about).add_sub_option(who("who", "the lucky one")));
    }
    love = love
        .add_option(sub("adopt", "adopt somebody as your child").add_sub_option(who("who", "your would-be child")))
        .add_option(sub("disown", "end a parent and child link").add_sub_option(who("who", "the one to let go")))
        .add_option(sub("jealous", "who has your spouse been talking to").add_sub_option(who(
            "who",
            "your spouse",
        )))
        .add_option(sub("papers", "serve divorce papers, dramatically").add_sub_option(who("who", "your spouse")))
        .add_option(sub("vows", "write what you said at the wedding"))
        .add_option(sub("matchmake", "who the numbers say suits you"))
        .add_option(sub("anniversary", "how long you have been married"))
        .add_option(sub("couples", "the longest, the biggest and the newest"))
        .add_option(sub("tree", "everybody's family tree, as a page"))
        .add_option(sub("refuse", "stop somebody proposing to you").add_sub_option(who("who", "the one to stop")));

    vec![
        CreateCommand::new("marry").description("ask somebody to marry you").add_option(who("who", "the one you want")),
        CreateCommand::new("divorce").description("end a marriage").add_option(who("who", "your spouse")),
        CreateCommand::new("family").description("your family, as a card"),
        love,
    ]
}

/// The names a card uses: whatever Discord shows, never an id.
fn called(ctx: &Context, id: u64) -> String {
    ctx.cache.user(id).map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone())).unwrap_or_else(|| {
        // Not in the cache is normal for somebody quiet; the mention still works.
        format!("<@{}>", id)
    })
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

/// Where the cards go: the love channel if one is set, otherwise where the
/// command was used, so nothing is ever silently swallowed.
fn stage(command: &CommandInteraction) -> serenity::all::ChannelId {
    marriage::channel().map(serenity::all::ChannelId::new).unwrap_or(command.channel_id)
}

/// The option a subcommand was given, by name.
fn user_arg(command: &CommandInteraction) -> Option<u64> {
    let sub = command.data.options.first()?;
    let CommandDataOptionValue::SubCommand(inner) = &sub.value else { return None };
    inner.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    })
}

fn sub_name(command: &CommandInteraction) -> String {
    command.data.options.first().map(|o| o.name.clone()).unwrap_or_default()
}

/// A proposal: an offer with two buttons, and nothing happens until one is
/// pressed. The asker's id rides in the custom id so a restart cannot orphan it.
fn offer(kind: &str, asker: u64, target: u64, words: &str) -> (CreateEmbed, Vec<CreateActionRow>) {
    let embed = CreateEmbed::new()
        .title(if kind == ASK_MARRY { "💍 A proposal" } else { "🏡 An offer" })
        .description(words.to_string())
        .colour(0xe0729f)
        .footer(CreateEmbedFooter::new("Only the one being asked can answer."));
    let row = CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{}{}:{}", kind, asker, target)).label("Accept").style(ButtonStyle::Success),
        CreateButton::new(format!("{}{}:{}", SAY_NO, asker, target)).label("Decline").style(ButtonStyle::Secondary),
    ]);
    (embed, vec![row])
}

pub async fn on_command(ctx: &Context, command: &CommandInteraction) {
    if !marriage::on() {
        let _ = command.create_response(&ctx.http, whisper("This isn't switched on yet.")).await;
        return;
    }
    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("Something's wrong at my end - try again in a minute.")).await;
        return;
    };
    let me = command.user.id.get();
    let now = chrono::Utc::now().timestamp();
    match command.data.name.as_str() {
        "marry" => propose(ctx, command, db, me, now).await,
        "divorce" => end_it(ctx, command, db, me, now).await,
        "family" => show_family(ctx, command, db, me, now).await,
        "love" => on_love(ctx, command, db, me, now).await,
        _ => {}
    }
}

async fn propose(
    ctx: &Context,
    command: &CommandInteraction,
    db: &parking_lot::Mutex<rusqlite::Connection>,
    me: u64,
    now: i64,
) {
    let Some(target) = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    }) else {
        return;
    };
    let a_bot = ctx.cache.user(target).map(|u| u.bot).unwrap_or(false);
    let (already, refused, last) = {
        let conn = db.lock();
        (
            store::married(&conn, me, target),
            store::is_refused(&conn, me, target),
            conn.query_row(
                "SELECT MAX(until) FROM past WHERE (low = ?1 AND high = ?2) OR (low = ?2 AND high = ?1)",
                rusqlite::params![me.min(target) as i64, me.max(target) as i64],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten(),
        )
    };
    if let Err(why) = marriage::may_propose(me, target, a_bot, already, refused, last, now) {
        let _ = command.create_response(&ctx.http, whisper(why.says())).await;
        return;
    }
    let words = format!("<@{}> — **{}** is asking you to marry them.", target, called(ctx, me));
    let (embed, rows) = offer(ASK_MARRY, me, target, &words);
    let sent = stage(command)
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .embed(embed)
                .components(rows)
                .allowed_mentions(CreateAllowedMentions::new().users(vec![serenity::all::UserId::new(target)])),
        )
        .await;
    let answer = match sent {
        Ok(_) => "Asked. They have to answer it themselves.".to_string(),
        Err(err) => {
            tracing::warn!("love: proposal not posted: {err}");
            "I couldn't post that - check I can speak in the love channel.".to_string()
        }
    };
    let _ = command.create_response(&ctx.http, whisper(answer)).await;
}

async fn end_it(
    ctx: &Context,
    command: &CommandInteraction,
    db: &parking_lot::Mutex<rusqlite::Connection>,
    me: u64,
    now: i64,
) {
    let Some(target) = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    }) else {
        return;
    };
    // One-sided and immediate: needing permission to leave is how somebody
    // ends up stuck in a joke they stopped finding funny.
    let ended = {
        let conn = db.lock();
        store::divorce(&conn, me, target, me, now).unwrap_or(false)
    };
    if !ended {
        let _ = command.create_response(&ctx.http, whisper("You two aren't married.")).await;
        return;
    }
    let card = CreateEmbed::new()
        .title("💔 It is over")
        .description(format!("**{}** and **{}** are no longer married.", called(ctx, me), called(ctx, target)))
        .colour(0x6e5f7c);
    let _ = stage(command)
        .send_message(&ctx.http, CreateMessage::new().embed(card).allowed_mentions(CreateAllowedMentions::new()))
        .await;
    let _ = command.create_response(&ctx.http, whisper("Done.")).await;
}

async fn show_family(
    ctx: &Context,
    command: &CommandInteraction,
    db: &parking_lot::Mutex<rusqlite::Connection>,
    me: u64,
    now: i64,
) {
    let family = {
        let conn = db.lock();
        marriage::family_of(&conn, me)
    };
    let names = |ids: &[u64]| {
        if ids.is_empty() {
            "nobody".to_string()
        } else {
            ids.iter().map(|id| called(ctx, *id)).collect::<Vec<_>>().join(" · ")
        }
    };
    let spouses = if family.spouses.is_empty() {
        "nobody".to_string()
    } else {
        family
            .spouses
            .iter()
            .map(|u| format!("{} ({})", called(ctx, u.other), marriage::how_long(u.since, now)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let card = CreateEmbed::new()
        .title(format!("🏡 {}'s family", called(ctx, me)))
        .description(marriage::with_tree(me, String::new()).trim_start().to_string())
        .field("Married to", spouses, false)
        .field("Parents", names(&family.parents), true)
        .field("Children", names(&family.children), true)
        .colour(0xe0729f);
    let _ = command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().embed(card).ephemeral(true),
            ),
        )
        .await;
}

async fn on_love(
    ctx: &Context,
    command: &CommandInteraction,
    db: &parking_lot::Mutex<rusqlite::Connection>,
    me: u64,
    now: i64,
) {
    let which = sub_name(command);
    let target = user_arg(command);
    // The gestures that are only words and a picture, with one cooldown between
    // them all - so a channel cannot be filled by one person pressing buttons.
    if let Some((kind, verb)) = GESTURES.iter().find(|(k, _)| *k == which) {
        let Some(them) = target else { return };
        let wait = {
            let conn = db.lock();
            marriage::gesture_wait(store::last_gesture(&conn, "gesture", me), now)
        };
        if wait > 0 {
            let _ = command.create_response(&ctx.http, whisper(format!("Give it {}s.", wait))).await;
            return;
        }
        {
            let conn = db.lock();
            store::note_gesture(&conn, "gesture", me, Some(them), now);
        }
        let roll = (now as u64).wrapping_mul(2_654_435_761).wrapping_add(me);
        let line = if *kind == "flirt" {
            super::love_bank::flirt_line(roll)
                .map(|l| l.replace("{a}", &called(ctx, me)).replace("{b}", &format!("<@{}>", them)))
                .unwrap_or_else(|| format!("**{}** {} <@{}>.", called(ctx, me), verb, them))
        } else {
            format!("**{}** {} <@{}>.", called(ctx, me), verb, them)
        };
        let mut post = CreateMessage::new()
            .content(line)
            .allowed_mentions(CreateAllowedMentions::new().users(vec![serenity::all::UserId::new(them)]));
        if let Some(gif) = super::love_bank::gif(kind, roll) {
            post = post.embed(CreateEmbed::new().image(gif).colour(0xe0729f));
        }
        let _ = stage(command).send_message(&ctx.http, post).await;
        let _ = command.create_response(&ctx.http, whisper("Sent.")).await;
        return;
    }

    match which.as_str() {
        "adopt" => {
            let Some(them) = target else { return };
            let a_bot = ctx.cache.user(them).map(|u| u.bot).unwrap_or(false);
            let (kin, refused) = {
                let conn = db.lock();
                (store::all_kin(&conn), store::is_refused(&conn, me, them))
            };
            if let Err(why) = marriage::may_adopt(me, them, a_bot, &kin, refused) {
                let words = match why {
                    Refusal::Themselves => "That would make somebody their own ancestor.".to_string(),
                    other => other.says(),
                };
                let _ = command.create_response(&ctx.http, whisper(words)).await;
                return;
            }
            let words = format!("<@{}> — **{}** would like to adopt you.", them, called(ctx, me));
            let (embed, rows) = offer(ASK_ADOPT, me, them, &words);
            let _ = stage(command)
                .send_message(
                    &ctx.http,
                    CreateMessage::new().embed(embed).components(rows).allowed_mentions(
                        CreateAllowedMentions::new().users(vec![serenity::all::UserId::new(them)]),
                    ),
                )
                .await;
            let _ = command.create_response(&ctx.http, whisper("Asked.")).await;
        }
        "disown" => {
            let Some(them) = target else { return };
            let gone = {
                let conn = db.lock();
                store::disown(&conn, me, them).unwrap_or(false) || store::disown(&conn, them, me).unwrap_or(false)
            };
            let _ = command
                .create_response(
                    &ctx.http,
                    whisper(if gone { "Done." } else { "There is no link between you two." }),
                )
                .await;
        }
        "refuse" => {
            let Some(them) = target else { return };
            {
                let conn = db.lock();
                store::refuse(&conn, me, them, now);
            }
            // Quiet on purpose: telling them invites the confrontation this is
            // meant to prevent.
            let _ = command
                .create_response(&ctx.http, whisper("They can't propose to you again. They have not been told."))
                .await;
        }
        "anniversary" => {
            let mine = {
                let conn = db.lock();
                store::spouses(&conn, me)
            };
            let text = if mine.is_empty() {
                "You aren't married to anybody.".to_string()
            } else {
                mine.iter()
                    .map(|u| {
                        let mark = marriage::anniversary_today(u.since, now)
                            .map(|m| format!(" — **{}**", m))
                            .unwrap_or_default();
                        format!("**{}** · {}{}", called(ctx, u.other), marriage::how_long(u.since, now), mark)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let _ = command.create_response(&ctx.http, whisper(marriage::with_tree(me, text))).await;
        }
        "jealous" => {
            let Some(them) = target else { return };
            // Only about somebody you are actually married to: "who is my
            // spouse talking to" is a question, "who is that person talking
            // to" is surveillance.
            let theirs = {
                let conn = db.lock();
                store::married(&conn, me, them)
            };
            if !theirs {
                let _ = command.create_response(&ctx.http, whisper("You're not married to them.")).await;
                return;
            }
            let sheet = super::ship_sheet::db().and_then(|db| {
                let conn = db.lock();
                super::ship_sheet::get(&conn, them)
            });
            let Some(sheet) = sheet else {
                let _ = command
                    .create_response(&ctx.http, whisper("No sheet for them yet - the numbers are built overnight."))
                    .await;
                return;
            };
            let roll = (now as u64).wrapping_mul(2_654_435_761).wrapping_add(me);
            let words = match marriage::rival(&sheet.reply_partners, me, sheet.replies_sent) {
                Some((who, _, share)) => format!(
                    "👀 **{}** has been replying to **{}** more than anybody else — **{}%** of everything they say.",
                    called(ctx, them),
                    called(ctx, who),
                    share
                ),
                None => format!(
                    "💘 Nothing to report: **{}** replies to <@{}> more than to anybody else.",
                    called(ctx, them),
                    me
                ),
            };
            let mut post = CreateMessage::new()
                .content(words)
                .allowed_mentions(CreateAllowedMentions::new().users(vec![serenity::all::UserId::new(me)]));
            if let Some(gif) = super::love_bank::gif("jealous", roll) {
                post = post.embed(CreateEmbed::new().image(gif).colour(0xe0729f));
            }
            let _ = stage(command).send_message(&ctx.http, post).await;
            let _ = command.create_response(&ctx.http, whisper("Posted.")).await;
        }
        "papers" => {
            let Some(them) = target else { return };
            let theirs = {
                let conn = db.lock();
                store::married(&conn, me, them)
            };
            if !theirs {
                let _ = command.create_response(&ctx.http, whisper("You're not married to them.")).await;
                return;
            }
            // Served, not executed: the papers are theatre, /divorce is the act.
            let roll = (now as u64).wrapping_mul(6_364_136_223_846_793_005).wrapping_add(them);
            let card = CreateEmbed::new()
                .title("📜 Divorce papers, served")
                .description(format!(
                    "**{}** has served papers on <@{}>.\n-# Nothing is final: `/divorce` is what ends it.",
                    called(ctx, me),
                    them
                ))
                .colour(0x6e5f7c);
            let card = match super::love_bank::gif("papers", roll) {
                Some(gif) => card.image(gif),
                None => card,
            };
            let _ = stage(command)
                .send_message(
                    &ctx.http,
                    CreateMessage::new().embed(card).allowed_mentions(
                        CreateAllowedMentions::new().users(vec![serenity::all::UserId::new(them)]),
                    ),
                )
                .await;
            let _ = command.create_response(&ctx.http, whisper("Served.")).await;
        }
        "matchmake" => {
            let mine = super::ship_sheet::db().and_then(|db| {
                let conn = db.lock();
                super::ship_sheet::get(&conn, me)
            });
            let Some(sheet) = mine else {
                let _ = command
                    .create_response(
                        &ctx.http,
                        whisper("No sheet for you yet - the numbers are built overnight. Try tomorrow."),
                    )
                    .await;
                return;
            };
            // Who the numbers actually point at: the person they reply to most
            // who is not already their spouse. Real figures, so the card can
            // say WHY rather than inventing a percentage.
            let married: Vec<u64> = {
                let conn = db.lock();
                store::spouses(&conn, me).into_iter().map(|u| u.other).collect()
            };
            let best = sheet
                .reply_partners
                .iter()
                .filter(|(id, _)| *id != me && !married.contains(id))
                .max_by_key(|(_, n)| *n)
                .copied();
            let text = match best {
                Some((who, n)) if sheet.replies_sent > 0 => {
                    let share = ((n as f64 / sheet.replies_sent as f64) * 100.0).round() as i64;
                    let vc = sheet.vc_with(who);
                    let extra = if vc > 0 { format!(", and **{}** minutes in voice together", vc) } else { String::new() };
                    format!(
                        "💞 The numbers point at **{}**.\n**{}%** of everything you say is to them{}.\n-# Not a prophecy. `/marry` is still your own doing.",
                        called(ctx, who),
                        share,
                        extra
                    )
                }
                _ => "Nothing to go on yet — talk to more people and ask again tomorrow.".to_string(),
            };
            let _ = command.create_response(&ctx.http, whisper(text)).await;
        }
        "tree" | "couples" => {
            let (unions, kin) = {
                let conn = db.lock();
                (store::all_unions(&conn), store::all_kin(&conn))
            };
            let board = marriage::standing(&unions, &kin);
            let pair = |p: Option<((u64, u64), i64)>| {
                p.map(|((a, b), since)| {
                    format!("**{}** and **{}** · {}", called(ctx, a), called(ctx, b), marriage::how_long(since, now))
                })
            };
            let mut text = if board.married == 0 {
                "Nobody has married yet. Somebody has to go first.".to_string()
            } else {
                let mut lines = vec![format!(
                    "💒 **{}** married · **{}** household{} · the biggest is **{}** strong",
                    board.married,
                    board.households,
                    if board.households == 1 { "" } else { "s" },
                    board.biggest
                )];
                if let Some(line) = pair(board.oldest) {
                    lines.push(format!("🥇 Longest: {}", line));
                }
                if let Some(line) = pair(board.newest) {
                    lines.push(format!("🆕 Newest: {}", line));
                }
                if let Some((who, n)) = board.most_spouses {
                    lines.push(format!("💍 Most spouses: **{}** with **{}**", called(ctx, who), n));
                }
                lines.join("\n")
            };
            // The whole map, not one household: this is the command for
            // everybody's tree at once.
            if let Some(base) = super::month::live_url() {
                let root = base.strip_suffix("/live").unwrap_or(&base);
                text.push_str(&format!("\n-# 🌳 Everybody: {}/love", root));
            }
            let _ = command.create_response(&ctx.http, whisper(text)).await;
        }
        _ => {
            let _ = command.create_response(&ctx.http, whisper("That part isn't built yet.")).await;
        }
    }
}

pub fn owns_component(id: &str) -> bool {
    id.starts_with(ASK_MARRY) || id.starts_with(ASK_ADOPT) || id.starts_with(SAY_NO)
}

/// Who an offer was between, out of the button's own id.
fn pair_of(id: &str) -> Option<(u64, u64)> {
    let rest = id.rsplit(':').take(2).collect::<Vec<_>>();
    let (target, asker) = (rest.first()?.parse().ok()?, rest.get(1)?.parse().ok()?);
    Some((asker, target))
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let Some((asker, target)) = pair_of(&id) else { return };
    let presser = component.user.id.get();
    if presser != target {
        let _ = component
            .create_response(&ctx.http, whisper("This one isn't yours to answer."))
            .await;
        return;
    }
    let Some(db) = store::db() else { return };
    let now = chrono::Utc::now().timestamp();
    if id.starts_with(SAY_NO) {
        let _ = component.create_response(&ctx.http, whisper("Declined. Nothing was written down.")).await;
        return;
    }
    let married = id.starts_with(ASK_MARRY);
    let done = {
        let conn = db.lock();
        if married {
            store::marry(&conn, asker, target, now).unwrap_or(false)
        } else {
            store::adopt(&conn, asker, target, now).unwrap_or(false)
        }
    };
    if !done {
        let _ = component.create_response(&ctx.http, whisper("That has already happened.")).await;
        return;
    }
    let roll = (now as u64).wrapping_mul(6_364_136_223_846_793_005).wrapping_add(asker);
    let card = CreateEmbed::new()
        .title(if married { "💒 Married" } else { "🏡 Adopted" })
        .description(if married {
            format!("**{}** and **{}** are married. Congratulations, allegedly.", called(ctx, asker), called(ctx, target))
        } else {
            format!("**{}** is now **{}**'s child.", called(ctx, target), called(ctx, asker))
        })
        .colour(0xe0729f);
    let card = match (married, super::love_bank::gif("wedding", roll)) {
        (true, Some(gif)) => card.image(gif),
        _ => card,
    };
    let _ = component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().embed(card).allowed_mentions(
                    CreateAllowedMentions::new()
                        .users(vec![serenity::all::UserId::new(asker), serenity::all::UserId::new(target)]),
                ),
            ),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_are_four_and_love_carries_the_rest() {
        let all = builders();
        let names: Vec<String> =
            all.iter().map(|c| serde_json::to_value(c).expect("json")["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["marry", "divorce", "family", "love"], "four against Discord's hundred");
        let love = serde_json::to_value(&all[3]).expect("json");
        let subs = love["options"].as_array().expect("options");
        assert!(subs.len() >= 14, "every gesture is a subcommand, not a command: {}", subs.len());
        assert!(subs.iter().all(|o| o["type"] == 1), "all of them subcommands");
    }

    #[test]
    fn a_button_remembers_who_asked_and_who_was_asked() {
        assert_eq!(pair_of("love:marry:111:222"), Some((111, 222)));
        assert_eq!(pair_of("love:no:111:222"), Some((111, 222)));
        assert_eq!(pair_of("love:marry:nonsense"), None);
    }
}
