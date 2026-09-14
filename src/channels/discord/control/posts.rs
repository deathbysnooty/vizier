//! What a scheduled post says and looks like: the extra placeholders
//! (`{date}`, `{standings}`, `{countdown:…}`, `{random:…}`…), the picture it
//! takes, plain text or a card, AI-written text and its clean-up, and the
//! ready-made templates the panel starts from. Nothing here talks to Discord,
//! so all of it is tested directly; the scheduler turns an [`Outgoing`] into a
//! message.

use chrono::{Datelike, NaiveDate, TimeZone};
use serde::Serialize;

use super::reminders::{ImageOrder, Order, Reminder, Schedule, Style};

/// Longest AI-written post.
pub const AI_MAX_CHARS: usize = 400;
/// How many AI-written posts are remembered so the next one differs.
pub const AI_RECENT: usize = 5;
/// The card colour when none is set: the bot's own role colour.
pub const DEFAULT_COLOUR: u32 = 0x8B93FF;

fn ist() -> chrono::FixedOffset {
    super::super::stats::ist()
}

// --- placeholders ----------------------------------------------------------------------

/// One house in the month's table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Standing {
    pub crest: &'static str,
    pub name: &'static str,
    pub points: i64,
}

/// This month's House Cup, highest first (ties keep the houses' usual order).
pub fn standings_now(now: i64) -> Vec<Standing> {
    let totals = super::super::house::db()
        .and_then(|db| super::super::points::house_totals(&db.lock(), super::super::points::month_start(now)).ok())
        .unwrap_or_default();
    standings_from(&totals)
}

pub fn standings_from(totals: &std::collections::HashMap<&'static str, i64>) -> Vec<Standing> {
    let mut out: Vec<Standing> = super::super::house::HOUSES
        .iter()
        .map(|h| Standing { crest: h.crest, name: h.name, points: totals.get(h.key).copied().unwrap_or(0) })
        .collect();
    out.sort_by(|a, b| b.points.cmp(&a.points));
    out
}

/// Whether any of the texts needs the House Cup read.
pub fn needs_houses<'a>(texts: impl IntoIterator<Item = &'a str>) -> bool {
    texts.into_iter().any(|t| t.contains("{leader}") || t.contains("{standings}"))
}

fn leader(table: &[Standing]) -> String {
    let Some(top) = table.first() else {
        return "nobody yet".to_string();
    };
    if top.points <= 0 {
        return "nobody yet".to_string();
    }
    table
        .iter()
        .filter(|s| s.points == top.points)
        .map(|s| format!("{} {}", s.crest, s.name))
        .collect::<Vec<_>>()
        .join(" & ")
}

fn standings_text(table: &[Standing]) -> String {
    table.iter().map(|s| format!("{} {}", s.crest, s.points)).collect::<Vec<_>>().join(" · ")
}

/// "6 days", "1 day", "today", from India today to a date; `None` when it isn't one.
fn countdown(date: &str, now: i64) -> Option<String> {
    let target = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
    let today = ist().timestamp_opt(now, 0).single()?.date_naive();
    let days = (target - today).num_days();
    Some(match days {
        d if d <= 0 => "today".to_string(),
        1 => "1 day".to_string(),
        d => format!("{} days", d),
    })
}

/// Where the brace that closes an opened one is, allowing braces inside it.
fn closing_brace(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Fills `{date}` (Mon 14 Sep), `{day}` (Monday), `{time}` (21:00, India),
/// `{leader}`, `{standings}`, `{countdown:YYYY-MM-DD}` and `{random:a|b|c}`.
/// `roll` gives numbers in `[0, 1)` for the random picks. Anything else in
/// braces is left as written, for the scheduler's own `{name}` and friends.
pub fn fill_extras(text: &str, now: i64, table: &[Standing], roll: &mut dyn FnMut() -> f64) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = closing_brace(after) else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &after[..end];
        if let Some(options) = inner.strip_prefix("random:") {
            let options: Vec<&str> = options.split('|').collect();
            let pick = ((roll().clamp(0.0, 1.0) * options.len() as f64) as usize).min(options.len() - 1);
            out.push_str(options[pick].trim());
        } else if let Some(date) = inner.strip_prefix("countdown:") {
            match countdown(date, now) {
                Some(words) => out.push_str(&words),
                None => out.push_str(&rest[start..start + end + 2]),
            }
        } else {
            out.push_str(&rest[start..start + end + 2]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);

    let local = ist().timestamp_opt(now, 0).single();
    let fmt = |f: &str| local.map(|t| t.format(f).to_string()).unwrap_or_default();
    let mut out = out.replace("{date}", &fmt("%a %-d %b")).replace("{day}", &fmt("%A")).replace("{time}", &fmt("%H:%M"));
    if out.contains("{leader}") {
        out = out.replace("{leader}", &leader(table));
    }
    if out.contains("{standings}") {
        out = out.replace("{standings}", &standings_text(table));
    }
    out
}

/// The values the panel's preview fills in by itself.
pub fn preview_values(now: i64, table: &[Standing]) -> serde_json::Value {
    let local = ist().timestamp_opt(now, 0).single();
    let fmt = |f: &str| local.map(|t| t.format(f).to_string()).unwrap_or_default();
    serde_json::json!({
        "date": fmt("%a %-d %b"),
        "day": fmt("%A"),
        "time": fmt("%H:%M"),
        "today": fmt("%Y-%m-%d"),
        "leader": leader(table),
        "standings": standings_text(table),
    })
}

// --- pictures ------------------------------------------------------------------------------

/// Which picture goes with the post: in turn by how many have gone, at random
/// from `roll`, or always the first.
pub fn pick_image(images: &[String], order: ImageOrder, sent_count: i64, roll: f64) -> Option<&str> {
    let images: Vec<&str> = images.iter().map(String::as_str).filter(|i| !i.trim().is_empty()).collect();
    if images.is_empty() {
        return None;
    }
    let index = match order {
        ImageOrder::Same => 0,
        ImageOrder::Rotate => sent_count.rem_euclid(images.len() as i64) as usize,
        ImageOrder::Random => ((roll.clamp(0.0, 1.0) * images.len() as f64) as usize).min(images.len() - 1),
    };
    Some(images[index])
}

// --- the message ----------------------------------------------------------------------------

/// A card's parts, as Discord's embed takes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Card {
    pub title: String,
    pub description: String,
    pub colour: u32,
    /// `attachment://<file>` when the post has a picture.
    pub image: Option<String>,
    pub footer: String,
}

/// A post, ready to send.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outgoing {
    pub content: String,
    pub card: Option<Card>,
    /// The attached picture's file name.
    pub attachment: Option<String>,
}

/// "#8b93ff" or "8B93FF" as a colour.
pub fn parse_colour(raw: &str) -> Option<u32> {
    let hex = raw.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u32::from_str_radix(hex, 16).ok()).flatten()
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// Puts a post together. `text`, `title` and `footer` are already filled in;
/// `attachment` is the picture's file name, if one goes with it.
pub fn compose(style: Style, text: &str, title: &str, colour: &str, footer: &str, attachment: Option<&str>) -> Outgoing {
    match style {
        Style::Plain => Outgoing { content: clip(text.trim(), 2000), card: None, attachment: attachment.map(String::from) },
        Style::Card => Outgoing {
            content: String::new(),
            card: Some(Card {
                title: clip(title.trim(), 256),
                description: clip(text.trim(), 4096),
                colour: parse_colour(colour).unwrap_or(DEFAULT_COLOUR),
                image: attachment.map(|name| format!("attachment://{}", name)),
                footer: clip(footer.trim(), 2048),
            }),
            attachment: attachment.map(String::from),
        },
    }
}

impl Outgoing {
    /// Whether there is anything to send.
    pub fn is_empty(&self) -> bool {
        self.content.trim().is_empty()
            && self.attachment.is_none()
            && self.card.as_ref().is_none_or(|c| c.title.is_empty() && c.description.is_empty() && c.footer.is_empty())
    }
}

// --- after a post -----------------------------------------------------------------------------

/// The message to delete once the new post is out, when the reminder tidies up.
pub fn previous_to_delete(r: &Reminder, new_message: u64) -> Option<u64> {
    if !r.delete_previous {
        return None;
    }
    r.last_message_id.trim().parse::<u64>().ok().filter(|id| *id != 0 && *id != new_message)
}

/// What the scheduler writes back after a post went out.
pub fn record_post(r: &mut Reminder, now: i64, message_id: u64, ai_text: Option<&str>) {
    r.last_sent = now;
    r.sent_count += 1;
    r.last_message_id = message_id.to_string();
    if let Some(text) = ai_text {
        r.ai_recent.push(text.to_string());
        let extra = r.ai_recent.len().saturating_sub(AI_RECENT);
        r.ai_recent.drain(..extra);
    }
}

// --- AI-written posts ---------------------------------------------------------------------------

/// The post's text: the AI's reply, cleaned, when it gave something usable,
/// otherwise the line. The second value is the AI's text, to remember.
pub fn choose_text(ai_reply: Option<&str>, line: Option<String>) -> (String, Option<String>) {
    match ai_reply.and_then(sanitise_ai) {
        Some(text) => (text.clone(), Some(text)),
        None => (line.unwrap_or_default(), None),
    }
}

/// The prompt for one AI-written post. `instruction` has its placeholders filled.
pub fn ai_prompt(instruction: &str, recent: &[String], now: i64) -> String {
    let local = ist().timestamp_opt(now, 0).single();
    let when = local.map(|t| t.format("%A %-d %B, %H:%M").to_string()).unwrap_or_default();
    let mut prompt = format!(
        "You are Loduchand, the bot of a friendly Discord server, writing one scheduled post for a channel.\n\n\
         What to write: {}\n\n\
         It is {} India time.\n",
        instruction.trim(),
        when
    );
    let recent: Vec<&String> = recent.iter().rev().take(AI_RECENT).collect();
    if !recent.is_empty() {
        prompt.push_str("\nRecent posts from this same schedule. Write something clearly different:\n");
        for r in recent {
            prompt.push_str(&format!("- {}\n", r.replace('\n', " ")));
        }
    }
    prompt.push_str(&format!(
        "\nReply with only the post itself, ready to send: under {} characters, no @mentions, no pings, \
         no quotation marks around it, no explanation.",
        AI_MAX_CHARS - 50
    ));
    prompt
}

/// Cleans what the model wrote: no pings of any kind, no wrapping quotes or
/// code fences, at most [`AI_MAX_CHARS`]. `None` when nothing usable is left.
pub fn sanitise_ai(raw: &str) -> Option<String> {
    let mut text = raw.trim().to_string();
    if text.starts_with("```") {
        text = text.trim_start_matches('`').trim_end_matches('`').to_string();
        if let Some((first, rest)) = text.split_once('\n') {
            if !first.trim().contains(' ') && first.trim().len() < 12 {
                text = rest.to_string();
            }
        }
    }
    let text = text.trim();
    let text = ["\"", "“", "'"]
        .iter()
        .find_map(|q| {
            let close = if *q == "“" { "”" } else { q };
            text.strip_prefix(q).and_then(|t| t.strip_suffix(close))
        })
        .unwrap_or(text);

    // <@123> and <@!123> read as "someone", <@&123> goes; @everyone and @here lose the @.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let digits = after.trim_start_matches(['!', '&']);
        let skipped = after.len() - digits.len();
        let n = digits.bytes().take_while(u8::is_ascii_digit).count();
        if n > 0 && digits[n..].starts_with('>') {
            if !after.starts_with('&') {
                out.push_str("someone");
            }
            rest = &after[skipped + n + 1..];
        } else {
            out.push_str("<@");
            rest = after;
        }
    }
    out.push_str(rest);
    let mut out = out;
    for word in ["everyone", "here"] {
        let lower = out.to_ascii_lowercase();
        let needle = format!("@{}", word);
        let mut cleaned = String::with_capacity(out.len());
        let mut last = 0;
        for (i, _) in lower.match_indices(&needle) {
            cleaned.push_str(&out[last..i]);
            last = i + 1;
        }
        cleaned.push_str(&out[last..]);
        out = cleaned;
    }
    let lines: Vec<String> = out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let mut out = lines.join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    let out = out.trim();
    if out.is_empty() {
        return None;
    }
    Some(clip(out, AI_MAX_CHARS))
}

// --- templates ---------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct Template {
    pub key: &'static str,
    pub icon: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub reminder: Reminder,
}

/// Ready-made posts to start from. The channel is left for the admin to pick.
pub fn templates(now: i64) -> Vec<Template> {
    // The countdown aims at the coming Saturday.
    let today = ist().timestamp_opt(now, 0).single().map(|t| t.date_naive()).unwrap_or_default();
    let ahead = (5 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
    let saturday = (today + chrono::Duration::days(if ahead == 0 { 7 } else { ahead })).format("%Y-%m-%d").to_string();
    let base = |name: &str, schedule: Schedule| Reminder { name: name.to_string(), enabled: true, schedule, ..Default::default() };
    vec![
        Template {
            key: "good_morning",
            icon: "☀️",
            name: "Good morning with a picture",
            about: "A cheerful morning line with a picture from your library. Add a few and they take turns.",
            reminder: Reminder {
                lines: vec![
                    "Good morning, legends! ☀️ {day} is here. Chai ready hai?".into(),
                    "Suprabhat, {date}! Aaj ka plan kya hai? ☕".into(),
                    "Rise and shine! It's {day}. {random:Be kind|Drink water|Touch some grass} today. 🌼".into(),
                ],
                reactions: vec!["☀️".into()],
                image_order: ImageOrder::Rotate,
                ..base("Good morning", Schedule::Daily { times: vec!["08:00".into()] })
            },
        },
        Template {
            key: "question_of_the_day",
            icon: "❓",
            name: "Question of the day (AI)",
            about: "The AI writes a fresh, light question every day and remembers its last few so it doesn't repeat.",
            reminder: Reminder {
                style: Style::Card,
                title: "❓ Question of the day · {date}".into(),
                colour: "#e0a43a".into(),
                footer: "Answer below 👇".into(),
                ai_prompt: "A fun, easy question for everyone to answer in chat: this-or-that, would-you-rather, food, \
                            films, cricket, college life or games. One question only, a short emoji is fine."
                    .into(),
                lines: vec![
                    "Chai or coffee, and why is it chai?".into(),
                    "What's one song you've had on repeat this week?".into(),
                    "Would you rather have unlimited Swiggy or unlimited data?".into(),
                ],
                order: Order::Random,
                reactions: vec!["👀".into()],
                ..base("Question of the day", Schedule::Daily { times: vec!["18:00".into()] })
            },
        },
        Template {
            key: "house_cup_daily",
            icon: "🏆",
            name: "Daily House Cup standings",
            about: "A card with this month's table, posted every night.",
            reminder: Reminder {
                style: Style::Card,
                title: "🏆 House Cup · {date}".into(),
                colour: "#c28b2c".into(),
                footer: "Points reset on the 1st · /houses for more".into(),
                lines: vec![
                    "{standings}\n\n{leader} lead the month. Everyone else, it's not over yet!".into(),
                    "{standings}\n\nTop of the table tonight: {leader}.".into(),
                ],
                delete_previous: true,
                ..base("House Cup standings", Schedule::Daily { times: vec!["22:00".into()] })
            },
        },
        Template {
            key: "weekend_countdown",
            icon: "🎉",
            name: "Weekend event countdown",
            about: "Counts down to Saturday's game night. Change the date in {countdown:…} and the end to your event's.",
            reminder: Reminder {
                lines: vec![
                    format!("🎉 Game night is in **{{countdown:{}}}**! Keep Saturday 9 PM free.", saturday),
                    format!("Only **{{countdown:{}}}** to game night. Who's in? 🙋", saturday),
                ],
                reactions: vec!["🙋".into()],
                delete_previous: true,
                ends: format!("{}T23:59:00+05:30", saturday),
                ..base("Game night countdown", Schedule::Daily { times: vec!["19:00".into()] })
            },
        },
        Template {
            key: "hydration",
            icon: "💧",
            name: "Hydration and break reminder",
            about: "A gentle nudge every two hours in the day. Upload a few GIFs; one is picked at random each time.",
            reminder: Reminder {
                lines: vec![
                    "💧 {random:Pani pee lo, doston|Hydration check|Sip sip, hooray} — it's {time}.".into(),
                    "🧘 Break time! {random:Stretch your back|Look away from the screen for a minute|Stand up and walk around}.".into(),
                ],
                order: Order::Random,
                image_order: ImageOrder::Random,
                active_from: "10:00".into(),
                active_to: "23:00".into(),
                delete_previous: true,
                ..base("Hydration check", Schedule::Every { minutes: 120 })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 2026-09-14 16:00 India time, a Monday.
    const NOW: i64 = 1_789_381_800;

    fn table() -> Vec<Standing> {
        let totals: HashMap<&'static str, i64> =
            [("gryffindor", 397), ("slytherin", 249), ("ravenclaw", 413), ("hufflepuff", 249)].into_iter().collect();
        standings_from(&totals)
    }

    fn rolls(values: &[f64]) -> impl FnMut() -> f64 + '_ {
        let mut i = 0;
        move || {
            let v = values[i % values.len()];
            i += 1;
            v
        }
    }

    #[test]
    fn house_placeholders_read_the_table() {
        let t = table();
        let mut roll = rolls(&[0.0]);
        assert_eq!(fill_extras("{standings}", NOW, &t, &mut roll), "🦅 413 · 🦁 397 · 🐍 249 · 🦡 249");
        assert_eq!(fill_extras("{leader} lead!", NOW, &t, &mut roll), "🦅 Ravenclaw lead!");
        let tied: HashMap<&'static str, i64> = [("gryffindor", 10), ("ravenclaw", 10)].into_iter().collect();
        assert_eq!(fill_extras("{leader}", NOW, &standings_from(&tied), &mut roll), "🦁 Gryffindor & 🦅 Ravenclaw");
        assert_eq!(fill_extras("{leader}", NOW, &standings_from(&HashMap::new()), &mut roll), "nobody yet");
        assert!(needs_houses(["x", "a {standings}"]) && !needs_houses(["{date}"]));
    }

    #[test]
    fn dates_countdowns_and_random_picks() {
        let t = table();
        let mut roll = rolls(&[0.0]);
        assert_eq!(fill_extras("{date} · {day} · {time}", NOW, &t, &mut roll), "Mon 14 Sep · Monday · 16:00");
        assert_eq!(fill_extras("in {countdown:2026-09-20}", NOW, &t, &mut roll), "in 6 days");
        assert_eq!(fill_extras("{countdown:2026-09-15}", NOW, &t, &mut roll), "1 day");
        assert_eq!(fill_extras("{countdown:2026-09-14}", NOW, &t, &mut roll), "today");
        assert_eq!(fill_extras("{countdown:2026-01-01}", NOW, &t, &mut roll), "today", "a past date reads as today");
        assert_eq!(fill_extras("{countdown:soon}", NOW, &t, &mut roll), "{countdown:soon}", "left as written");

        let mut low = rolls(&[0.0]);
        let mut high = rolls(&[0.99]);
        assert_eq!(fill_extras("{random:a|b|c}!", NOW, &t, &mut low), "a!");
        assert_eq!(fill_extras("{random:a| b |c}!", NOW, &t, &mut high), "c!");
        let mut mid = rolls(&[0.5, 0.0]);
        assert_eq!(fill_extras("{random: b |x} {random:y|z}", NOW, &t, &mut mid), "x y", "each pick rolls again");
        assert_eq!(fill_extras("{random:only}", NOW, &t, &mut mid), "only");
        // Picks can hold other placeholders, and the scheduler's own stay for it.
        let mut roll = rolls(&[0.0]);
        assert_eq!(fill_extras("{random:{day}|x} {mention} {name} {oops", NOW, &t, &mut roll), "Monday {mention} {name} {oops");
    }

    #[test]
    fn pictures_take_turns_pick_at_random_or_stay() {
        let imgs: Vec<String> = ["a", "", "b", "c"].iter().map(|s| s.to_string()).collect();
        let turn: Vec<&str> = (0..4).map(|n| pick_image(&imgs, ImageOrder::Rotate, n, 0.0).unwrap()).collect();
        assert_eq!(turn, vec!["a", "b", "c", "a"]);
        assert_eq!(pick_image(&imgs, ImageOrder::Random, 0, 0.999), Some("c"));
        assert_eq!(pick_image(&imgs, ImageOrder::Same, 7, 0.9), Some("a"));
        assert_eq!(pick_image(&[], ImageOrder::Rotate, 0, 0.0), None);
    }

    #[test]
    fn cards_and_plain_posts_are_built_differently() {
        let plain = compose(Style::Plain, "  hello  ", "ignored", "#ff0000", "ignored", Some("ab12.png"));
        assert_eq!(plain, Outgoing { content: "hello".into(), card: None, attachment: Some("ab12.png".into()) });

        let card = compose(Style::Card, "the table", "🏆 Cup", "#C28B2C", "reset on the 1st", Some("ab12.png"));
        assert_eq!(card.content, "");
        assert_eq!(card.attachment.as_deref(), Some("ab12.png"));
        let c = card.card.unwrap();
        assert_eq!((c.title.as_str(), c.description.as_str(), c.footer.as_str()), ("🏆 Cup", "the table", "reset on the 1st"));
        assert_eq!(c.colour, 0xC28B2C);
        assert_eq!(c.image.as_deref(), Some("attachment://ab12.png"));

        let bare = compose(Style::Card, "x", "", "not a colour", "", None);
        assert_eq!(bare.card.as_ref().unwrap().colour, DEFAULT_COLOUR);
        assert!(bare.card.as_ref().unwrap().image.is_none() && bare.attachment.is_none());
        assert!(compose(Style::Plain, " ", "", "", "", None).is_empty());
        assert!(!compose(Style::Plain, " ", "", "", "", Some("p.png")).is_empty(), "a picture alone is a post");
        let long = compose(Style::Plain, &"x".repeat(2500), "", "", "", None);
        assert_eq!(long.content.chars().count(), 2000);
        assert_eq!(parse_colour("8b93ff"), Some(0x8B93FF));
        assert_eq!(parse_colour("#fff"), None);
    }

    #[test]
    fn ai_text_is_cleaned_of_pings_and_capped() {
        assert_eq!(sanitise_ai("\"Hey @everyone, <@123> and <@!45> and <@&678> fans: what's up @Here?\"").unwrap(), "Hey everyone, someone and someone and fans: what's up Here?");
        assert_eq!(sanitise_ai("as dramatic as <@2010>'s").unwrap(), "as dramatic as someone's");
        assert_eq!(sanitise_ai("```text\nChai or coffee?\n```").unwrap(), "Chai or coffee?");
        assert_eq!(sanitise_ai("“Quoted”").unwrap(), "Quoted");
        assert_eq!(sanitise_ai("<@not a mention> <#123> fine").unwrap(), "<@not a mention> <#123> fine");
        assert_eq!(sanitise_ai("   "), None);
        assert_eq!(sanitise_ai("<@&1>"), None);
        let long = sanitise_ai(&"word ".repeat(200)).unwrap();
        assert!(long.chars().count() <= AI_MAX_CHARS && long.ends_with('…'));
    }

    #[test]
    fn lines_step_in_when_the_ai_gives_nothing_usable() {
        assert_eq!(choose_text(Some("  Chai or coffee? <@&5>"), Some("line".into())), ("Chai or coffee?".into(), Some("Chai or coffee?".into())));
        assert_eq!(choose_text(Some("   "), Some("line".into())), ("line".into(), None));
        assert_eq!(choose_text(Some("<@&12>"), Some("line".into())), ("line".into(), None), "only a ping is nothing");
        assert_eq!(choose_text(None, Some("line".into())), ("line".into(), None), "the call failed");
        assert_eq!(choose_text(None, None), (String::new(), None));
    }

    #[test]
    fn the_ai_prompt_carries_the_ask_and_recent_posts() {
        let recent: Vec<String> = (1..=7).map(|n| format!("post {n}")).collect();
        let p = ai_prompt("A question about food", &recent, NOW);
        assert!(p.contains("A question about food") && p.contains("Monday 14 September, 16:00"));
        assert!(p.contains("post 7") && p.contains("post 3") && !p.contains("post 2"), "only the last five");
        assert!(!ai_prompt("x", &[], NOW).contains("Recent posts"));
    }

    #[test]
    fn delete_previous_bookkeeping() {
        let mut r = Reminder { delete_previous: true, last_message_id: "111".into(), sent_count: 4, ..Default::default() };
        assert_eq!(previous_to_delete(&r, 222), Some(111));
        assert_eq!(previous_to_delete(&r, 111), None, "never the post just sent");
        record_post(&mut r, NOW, 222, None);
        assert_eq!((r.last_sent, r.sent_count, r.last_message_id.as_str()), (NOW, 5, "222"));
        assert!(r.ai_recent.is_empty());
        r.delete_previous = false;
        assert_eq!(previous_to_delete(&r, 333), None);
        r.delete_previous = true;
        r.last_message_id.clear();
        assert_eq!(previous_to_delete(&r, 333), None);
        for n in 0..7 {
            record_post(&mut r, NOW + n, 400 + n as u64, Some(&format!("ai {n}")));
        }
        assert_eq!(r.ai_recent, vec!["ai 2", "ai 3", "ai 4", "ai 5", "ai 6"]);
        assert_eq!(r.last_message_id, "406");
    }

    #[test]
    fn templates_are_complete_reminders() {
        let all = templates(NOW);
        assert_eq!(all.len(), 5);
        let mut keys: Vec<&str> = all.iter().map(|t| t.key).collect();
        keys.dedup();
        assert_eq!(keys.len(), 5);
        for t in &all {
            let r = &t.reminder;
            assert!(!r.name.is_empty() && r.enabled && r.channel_id.is_empty(), "{}", t.key);
            assert!(!r.lines.is_empty(), "{} needs fallback lines", t.key);
            assert!(r.reactions.iter().all(|e| super::super::autoreplies::reaction(e).is_some()), "{}", t.key);
            assert!(r.colour.is_empty() || parse_colour(&r.colour).is_some(), "{}", t.key);
            let json = serde_json::to_value(r).unwrap();
            let back: Reminder = serde_json::from_value(json).unwrap();
            assert_eq!(&back, r);
        }
        let qotd = all.iter().find(|t| t.key == "question_of_the_day").unwrap();
        assert!(!qotd.reminder.ai_prompt.is_empty() && qotd.reminder.style == Style::Card);
        let countdown = all.iter().find(|t| t.key == "weekend_countdown").unwrap();
        assert!(countdown.reminder.lines[0].contains("{countdown:2026-09-19}"), "the coming Saturday");
        assert_eq!(countdown.reminder.ends, "2026-09-19T23:59:00+05:30");
        let cup = all.iter().find(|t| t.key == "house_cup_daily").unwrap();
        assert!(cup.reminder.lines.iter().all(|l| l.contains("{standings}")));
    }
}
