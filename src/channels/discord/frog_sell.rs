//! `/sellset`: hand in one copy of every card in play for house points.
//!
//! The only way cards turn into points after they're caught. The member sees
//! which copies go - their highest-numbered one of each by default, so they keep
//! their low numbers - can swap in other copies where they have doubles, and
//! confirms. Confirming re-checks those exact copies in one transaction, marks
//! them spent and pays `VIZIER_FROG_SET_BONUS` through the ledger under
//! `frogsell:<sale>`. Each sale uses up a whole set.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, CommandInteraction, ComponentInteraction, ComponentInteractionDataKind, Context, CreateActionRow,
    CreateAllowedMentions, CreateButton, CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
};

use super::control;
use super::frog_store::{self as store, Card, SaleError};
use super::points::{Outcome, Source};

const DRAFT_SECS: i64 = 15 * 60;

fn points() -> i64 {
    control::number("VIZIER_FROG_SET_BONUS", 35).min(1000) as i64
}

// --- choosing copies ------------------------------------------------------------------------

/// The copies to hand in: the default plan, with any picked copy taking its
/// card's place (the highest-numbered pick wins if two of one card are picked).
pub fn choose_copies(plan: &[Card], owned: &[Card], picked: &[i64]) -> Vec<Card> {
    plan.iter()
        .map(|default| {
            owned
                .iter()
                .filter(|c| c.wizard_id == default.wizard_id && picked.contains(&c.serial))
                .max_by_key(|c| c.serial)
                .cloned()
                .unwrap_or_else(|| default.clone())
        })
        .collect()
}

/// Copies worth offering a choice between: every copy of a card in the plan the
/// member owns more than one of, rarest first, up to Discord's 25.
pub fn duplicate_copies(plan: &[Card], owned: &[Card]) -> Vec<Card> {
    let mut out: Vec<Card> = owned
        .iter()
        .filter(|c| plan.iter().any(|p| p.wizard_id == c.wizard_id) && owned.iter().filter(|o| o.wizard_id == c.wizard_id).count() > 1)
        .cloned()
        .collect();
    super::frog_trade::sort_for_trade(&mut out);
    out.truncate(25);
    out
}

pub fn plan_text(chosen: &[Card], points: i64, house: Option<(&str, &str)>) -> String {
    let lines = chosen.iter().map(super::frog_trade::card_line).collect::<Vec<_>>().join("\n");
    let to = house.map(|(crest, name)| format!(" for {} {}", crest, name)).unwrap_or_default();
    format!(
        "🏆 **Sell a full set**\nYou hand in one copy of every card{}:\n{}\n\n**+{} points**{} · the copies are gone for good",
        if chosen.len() > 1 { " (your highest numbers unless you pick others)" } else { "" },
        lines,
        points,
        to
    )
}

pub fn missing_text(missing: &[store::Wizard]) -> String {
    if missing.is_empty() {
        return "There are no cards in play to collect right now.".to_string();
    }
    let names = missing.iter().map(|w| format!("{} {}", w.rarity.emoji(), w.name)).collect::<Vec<_>>().join(", ");
    format!("You need one of every card in play to sell a set. Still missing: {}", names)
}

pub fn sold_line(user: u64, points: i64, house: Option<(&str, &str)>) -> String {
    let to = house.map(|(crest, name)| format!(" for {} {}", crest, name)).unwrap_or_default();
    format!("🏆 <@{}> sold a full set of Chocolate Frog cards · +{}{}", user, points, to)
}

// --- Discord --------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Draft {
    user: u64,
    picked: Vec<i64>,
    created: i64,
}

static DRAFTS: LazyLock<Mutex<HashMap<String, Draft>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn command() -> CreateCommand {
    CreateCommand::new("sellset").description("hand in a full set of Chocolate Frog cards for house points")
}

fn house_member(user: u64) -> bool {
    super::house::house_of(user).is_some() && !super::house::opted_out(user)
}

fn house_words(user: u64) -> Option<(&'static str, &'static str)> {
    super::house::house_of(user).map(|h| (h.crest, h.name))
}

/// The builder for a member's sale, or the reason there isn't one.
fn view(id: &str, d: &Draft) -> CreateInteractionResponseMessage {
    let (plan, owned) = match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::sale_plan(&conn, d.user), store::cards_of(&conn, d.user))
        }
        None => (Err(Vec::new()), Vec::new()),
    };
    let base = CreateInteractionResponseMessage::new().ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let plan = match plan {
        Ok(plan) => plan,
        Err(missing) => return base.content(missing_text(&missing)).components(Vec::new()),
    };
    let chosen = choose_copies(&plan, &owned, &d.picked);
    let mut rows = Vec::new();
    let doubles = duplicate_copies(&plan, &owned);
    if !doubles.is_empty() {
        let options = doubles
            .iter()
            .map(|c| CreateSelectMenuOption::new(super::frog_trade::card_line(c), c.serial.to_string()).default_selection(chosen.iter().any(|x| x.serial == c.serial)))
            .collect::<Vec<_>>();
        let max = options.len() as u8;
        rows.push(CreateActionRow::SelectMenu(
            CreateSelectMenu::new(format!("sellpick:{}", id), CreateSelectMenuKind::String { options })
                .placeholder("Hand in different copies (optional)")
                .min_values(0)
                .max_values(max),
        ));
    }
    rows.push(CreateActionRow::Buttons(vec![
        CreateButton::new(format!("sellgo:{}", id)).label(format!("Sell set for {} points", points())).style(ButtonStyle::Success),
        CreateButton::new(format!("sellno:{}", id)).label("Cancel").style(ButtonStyle::Secondary),
    ]));
    base.content(plan_text(&chosen, points(), house_words(d.user))).components(rows)
}

pub async fn sellset_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let refuse = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text.to_string()).ephemeral(true))
    };
    if !control::on("VIZIER_FROGS", false) {
        let _ = command.create_response(&ctx.http, refuse("Chocolate Frogs are switched off right now.")).await;
        return;
    }
    if !house_member(user) {
        let _ = command.create_response(&ctx.http, refuse("Only house members can sell sets: the points go to your house.")).await;
        return;
    }
    let now = Utc::now().timestamp();
    let id: String = rand::random::<[u8; 4]>().iter().map(|b| format!("{:02x}", b)).collect();
    let draft = Draft { user, picked: Vec::new(), created: now };
    let message = view(&id, &draft);
    {
        let mut drafts = DRAFTS.lock();
        drafts.retain(|_, d| now - d.created < DRAFT_SECS);
        drafts.insert(id, draft);
    }
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("frog: /sellset for {} not shown: {}", user, err);
    }
}

async fn update(ctx: &Context, component: &ComponentInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await {
        tracing::warn!("frog: sale view not updated: {}", err);
    }
}

fn closed(text: impl Into<String>) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new().content(text).components(Vec::new()).allowed_mentions(CreateAllowedMentions::new())
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let custom = component.data.custom_id.clone();
    let user = component.user.id.get();
    let (action, id) = match custom.split_once(':') {
        Some((a, id)) => (a.to_string(), id.to_string()),
        None => return,
    };
    let current = DRAFTS.lock().get(&id).cloned();
    let Some(mut draft) = current.filter(|d| d.user == user) else {
        return update(ctx, component, closed("This sale has closed. Run /sellset again.")).await;
    };
    match action.as_str() {
        "sellpick" => {
            draft.picked = match &component.data.kind {
                ComponentInteractionDataKind::StringSelect { values } => values.iter().filter_map(|v| v.parse().ok()).collect(),
                _ => Vec::new(),
            };
            let message = view(&id, &draft);
            DRAFTS.lock().insert(id, draft);
            update(ctx, component, message).await;
        }
        "sellno" => {
            DRAFTS.lock().remove(&id);
            update(ctx, component, closed("No sale. Your cards stay with you.")).await;
        }
        "sellgo" => {
            DRAFTS.lock().remove(&id);
            let Some(db) = store::db() else {
                return update(ctx, component, closed("The card store isn't open.")).await;
            };
            let planned = {
                let conn = db.lock();
                store::sale_plan(&conn, user).map(|plan| choose_copies(&plan, &store::cards_of(&conn, user), &draft.picked))
            };
            let chosen: Vec<i64> = match planned {
                Ok(cards) => cards.iter().map(|c| c.serial).collect(),
                Err(missing) => return update(ctx, component, closed(missing_text(&missing))).await,
            };
            let bonus = points();
            let result = {
                let mut conn = db.lock();
                store::sell_set(&mut conn, user, &chosen, bonus, Utc::now().timestamp())
            };
            match result {
                Ok(Ok(sale)) => {
                    let paid = super::house::award_person(user, Source::Frog, bonus, "Chocolate Frog: sold a full set", None, Some(format!("frogsell:{}", sale)), None);
                    let granted = match &paid {
                        Some((_, Outcome::Granted(n))) => *n,
                        _ => 0,
                    };
                    tracing::info!("frog: {} sold a full set (sale {}, {:?}), paid {}", user, sale, chosen, granted);
                    let house = house_words(user);
                    let mine = if granted > 0 {
                        format!("🏆 Sold! Your set is handed in · **+{}**{}", granted, house.map(|(c, n)| format!(" for {} {}", c, n)).unwrap_or_default())
                    } else {
                        "🏆 Sold! Your set is handed in, but no points fitted under today's frog limit.".to_string()
                    };
                    update(ctx, component, closed(mine)).await;
                    if granted > 0 {
                        let line = CreateMessage::new().content(sold_line(user, granted, house)).allowed_mentions(CreateAllowedMentions::new());
                        let _ = tokio::time::timeout(Duration::from_secs(20), component.channel_id.send_message(&ctx.http, line)).await;
                    }
                }
                Ok(Err(SaleError::NotOwned(gone))) => {
                    let list = gone.iter().map(|s| store::serial_label(*s)).collect::<Vec<_>>().join(", ");
                    update(ctx, component, closed(format!("Couldn't sell: {} isn't yours any more. Nothing was handed in.", list))).await;
                }
                Ok(Err(SaleError::NotASet)) => {
                    update(ctx, component, closed("The cards in play just changed, so that isn't a full set any more. Nothing was handed in; run /sellset again.")).await;
                }
                Err(err) => {
                    tracing::warn!("frog: sale for {} failed: {}", user, err);
                    update(ctx, component, closed("Something went wrong. Nothing was handed in; try again.")).await;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::frog_store::Rarity;

    fn card(serial: i64, wizard_id: i64, name: &str) -> Card {
        Card {
            serial,
            edition: serial,
            user_id: 7,
            wizard_id,
            wizard_name: name.into(),
            rarity: Rarity::Common,
            drop_id: None,
            ts: 0,
            origin: "caught".into(),
            original_owner: 7,
            status: "owned".into(),
            spent_reason: String::new(),
            spent_ts: None,
        }
    }

    #[test]
    fn the_highest_copy_goes_unless_another_is_picked() {
        let owned = vec![card(42, 1, "Rubeus Hagrid"), card(3, 1, "Rubeus Hagrid"), card(17, 1, "Rubeus Hagrid"), card(9, 2, "Luna Lovegood")];
        let plan = vec![card(42, 1, "Rubeus Hagrid"), card(9, 2, "Luna Lovegood")];
        assert_eq!(choose_copies(&plan, &owned, &[]).iter().map(|c| c.serial).collect::<Vec<_>>(), vec![42, 9]);
        assert_eq!(choose_copies(&plan, &owned, &[3]).iter().map(|c| c.serial).collect::<Vec<_>>(), vec![3, 9]);
        assert_eq!(choose_copies(&plan, &owned, &[3, 17]).iter().map(|c| c.serial).collect::<Vec<_>>(), vec![17, 9], "two picks of one card: the higher");
        assert_eq!(choose_copies(&plan, &owned, &[999]).iter().map(|c| c.serial).collect::<Vec<_>>(), vec![42, 9], "a copy they don't own is ignored");
        let doubles = duplicate_copies(&plan, &owned);
        assert_eq!(doubles.iter().map(|c| c.serial).collect::<Vec<_>>(), vec![3, 17, 42], "only cards with doubles");
    }

    /// The owner's rule: points paid when a card was caught are never taken back.
    /// Trading and selling write no negative ledger rows - nothing in the card
    /// store or trades touches the ledger at all, and a sale's one write is its
    /// price, which can't be below zero.
    #[test]
    fn selling_or_trading_never_takes_points_back() {
        let code = |source: &'static str| source.split_once("#[cfg(test)]").map(|(c, _)| c).unwrap_or(source);
        let writers = [concat!("award", "_person"), concat!("points::", "write"), concat!("INSERT INTO ", "ledger"), concat!("UPDATE ", "ledger"), concat!("DELETE FROM ", "ledger")];
        for (name, source) in [("frog_store", include_str!("frog_store.rs")), ("frog_trade", include_str!("frog_trade.rs"))] {
            for w in writers {
                assert!(!code(source).contains(w), "{name} writes to the ledger via {w}");
            }
        }
        let sell = code(include_str!("frog_sell.rs"));
        assert_eq!(sell.matches(concat!("award", "_person(")).count(), 1, "one ledger write: the sale's price");
        assert!(sell.contains(concat!("award", "_person(user, Source::Frog, bonus,")), "and it pays the price");
        assert!(sell.contains("let bonus = points();"));
        assert!(!sell.contains(concat!("-", "bonus")) && !sell.contains(concat!("bonus", " * -")));
        // The price is read as an unsigned setting, so it is never negative.
        assert!(points() >= 0);
        assert_eq!(points(), 35, "the owner's default");
    }

    #[test]
    fn the_sale_reads_clearly() {
        let chosen = vec![card(42, 1, "Rubeus Hagrid"), card(9, 2, "Luna Lovegood")];
        let text = plan_text(&chosen, 15, Some(("🦁", "Gryffindor")));
        assert!(text.starts_with("🏆 **Sell a full set**\nYou hand in one copy of every card (your highest numbers unless you pick others):\n🥛 Rubeus Hagrid #42 · No. 0042\n🥛 Luna Lovegood #9 · No. 0009"), "{text}");
        assert!(text.ends_with("**+15 points** for 🦁 Gryffindor · the copies are gone for good"), "{text}");
        assert_eq!(sold_line(7, 15, Some(("🦁", "Gryffindor"))), "🏆 <@7> sold a full set of Chocolate Frog cards · +15 for 🦁 Gryffindor");
        let conn = super::super::frog_store::tests::memory();
        let missing: Vec<store::Wizard> = store::wizards(&conn).into_iter().take(2).collect();
        assert_eq!(missing_text(&missing), "You need one of every card in play to sell a set. Still missing: 🥛 Rubeus Hagrid, 🥛 Luna Lovegood");
    }
}
