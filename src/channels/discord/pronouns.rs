//! Whose pronouns are whose, read off the server's own roles and never guessed.
//!
//! Everywhere the bot writes about a member in the third person — the nightly
//! topic pass's one line a day, a Kalesh summary, a Deep dive summary, a `/ship`
//! verdict — the model used to pick a gender out of the air: off a name, off the
//! way somebody types, off what the channel called them. That is exactly the
//! inference the bot is not allowed to make, and it was making it several times
//! a night.
//!
//! This server already answers the question properly. Members pick a role —
//! **Male**, **Female** or **Mystery** — and that role, and only that role, is
//! what the bot reads. Three settings hold the three role ids so a server that
//! renames or remakes a role can say so without a rebuild.
//!
//! The rule is deliberately blunt, because the failure mode of being clever here
//! is being wrong about somebody:
//!
//! - the Male role and nothing else → **he/him**
//! - the Female role and nothing else → **she/her**
//! - Mystery, both somehow, neither, or a member the bot cannot read at all →
//!   **they/them**
//!
//! Every prompt then carries [`RULE`] and a [`block`] naming each person, and the
//! model is told to use exactly what it is given and never work it out for
//! itself. Nobody's pronouns are stored: they are read from the guild cache when
//! a prompt is built and thrown away with the prompt.

use std::collections::HashMap;

use serenity::all::Context;

use super::control;

// --- the roles ------------------------------------------------------------------------------------

/// This server's Male role, unless a setting says otherwise.
pub const MALE_ROLE: u64 = 1_516_564_878_742_261_913;
/// This server's Female role.
pub const FEMALE_ROLE: u64 = 1_516_564_973_222_887_545;
/// This server's Mystery role: worn on purpose, and read as they/them.
pub const MYSTERY_ROLE: u64 = 1_516_565_038_930_985_051;

pub fn male_role() -> u64 {
    control::id("VIZIER_ROLE_MALE").unwrap_or(MALE_ROLE)
}

pub fn female_role() -> u64 {
    control::id("VIZIER_ROLE_FEMALE").unwrap_or(FEMALE_ROLE)
}

pub fn mystery_role() -> u64 {
    control::id("VIZIER_ROLE_MYSTERY").unwrap_or(MYSTERY_ROLE)
}

// --- what a member's roles say ----------------------------------------------------------------------

/// The pronouns a member is written about in. There is no fourth case on
/// purpose: anything the roles do not answer plainly is they/them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pronouns {
    He,
    She,
    /// Mystery, no role, two roles at once, or nobody the bot could look up.
    #[default]
    They,
}

impl Pronouns {
    /// "he/him", as the prompt gives them.
    pub const fn words(self) -> &'static str {
        match self {
            Pronouns::He => "he/him",
            Pronouns::She => "she/her",
            Pronouns::They => "they/them",
        }
    }

    /// "he", "she", "they".
    pub const fn subject(self) -> &'static str {
        match self {
            Pronouns::He => "he",
            Pronouns::She => "she",
            Pronouns::They => "they",
        }
    }

    /// "him", "her", "them".
    pub const fn object(self) -> &'static str {
        match self {
            Pronouns::He => "him",
            Pronouns::She => "her",
            Pronouns::They => "them",
        }
    }

    /// "his", "her", "their".
    pub const fn possessive(self) -> &'static str {
        match self {
            Pronouns::He => "his",
            Pronouns::She => "her",
            Pronouns::They => "their",
        }
    }
}

/// What a member's roles say, and nothing else. Mystery wins over everything,
/// because somebody wearing it has answered the question; two roles at once is
/// not a riddle worth solving, and neither is none.
pub fn from_roles(roles: &[u64]) -> Pronouns {
    if roles.contains(&mystery_role()) {
        return Pronouns::They;
    }
    let (male, female) = (male_role(), female_role());
    match (roles.contains(&male), roles.contains(&female)) {
        (true, false) => Pronouns::He,
        (false, true) => Pronouns::She,
        _ => Pronouns::They,
    }
}

/// One cached member's pronouns.
pub fn from_member(member: &serenity::all::Member) -> Pronouns {
    let roles: Vec<u64> = member.roles.iter().map(|r| r.get()).collect();
    from_roles(&roles)
}

/// Everybody the guild cache can read, by member id. Members it cannot read are
/// simply missing, which reads as they/them everywhere this map is used.
pub fn everyone(ctx: &Context) -> HashMap<u64, Pronouns> {
    let mut out: HashMap<u64, Pronouns> = HashMap::new();
    for guild in ctx.cache.guilds() {
        let Some(g) = ctx.cache.guild(guild) else { continue };
        for (id, member) in g.members.iter() {
            out.insert(id.get(), from_member(member));
        }
    }
    out
}

/// One member's pronouns off the live gateway cache. They/them when the bot is
/// not connected, or when the cache has never seen them.
pub fn of(user: u64) -> Pronouns {
    let Some(ctx) = control::web::context() else { return Pronouns::They };
    for guild in ctx.cache.guilds() {
        let Some(g) = ctx.cache.guild(guild) else { continue };
        if let Some(member) = g.members.get(&serenity::all::UserId::new(user)) {
            return from_member(member);
        }
    }
    Pronouns::They
}

/// The handful of members one prompt is about, off the live cache.
pub fn for_users(ids: impl IntoIterator<Item = u64>) -> HashMap<u64, Pronouns> {
    ids.into_iter().map(|id| (id, of(id))).collect()
}

// --- what the model is told ---------------------------------------------------------------------------

/// The line every set of instructions carries. It is one rule and it is absolute
/// on purpose: the model has the answer handed to it, so there is nothing left
/// for it to work out.
pub const RULE: &str = "Never guess, infer or imply anyone's gender - not from their name, not from how they write, \
not from what anyone calls them, not from anything else. Each person's pronouns are given to you below. Use exactly \
those, and use they/them for anybody whose pronouns are not given.";

/// The block that carries the answer, listing each person named in the prompt.
/// Always ends with a newline, and is never empty: with nobody to name it still
/// says that everybody is they/them.
pub fn block(people: &[(String, Pronouns)]) -> String {
    let mut out = String::from("Pronouns, from this server's own roles. Use exactly these; anybody not listed is they/them:\n");
    if people.is_empty() {
        out.push_str("- (nobody listed: they/them for everyone)\n");
        return out;
    }
    let mut seen: Vec<String> = Vec::new();
    for (name, p) in people {
        let key = name.trim().to_lowercase();
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push_str(&format!("- {}: {}\n", name.trim(), p.words()));
    }
    out
}

/// The same, for a list of members the caller knows by id and name.
pub fn block_for(named: &[(u64, String)], known: &HashMap<u64, Pronouns>) -> String {
    let people: Vec<(String, Pronouns)> = named.iter().map(|(id, name)| (name.clone(), known.get(id).copied().unwrap_or_default())).collect();
    block(&people)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each role says what it says, and everything else is they/them.
    #[test]
    fn each_role_gives_its_own_pronouns_and_everything_else_is_they_them() {
        assert_eq!(from_roles(&[MALE_ROLE]), Pronouns::He);
        assert_eq!(from_roles(&[FEMALE_ROLE]), Pronouns::She);
        assert_eq!(from_roles(&[MYSTERY_ROLE]), Pronouns::They, "Mystery is worn on purpose and means they/them");
        assert_eq!(from_roles(&[]), Pronouns::They, "no role at all is they/them, never a guess");
        assert_eq!(from_roles(&[999, 1_000]), Pronouns::They, "roles the bot cannot read say nothing");
        // Somebody wearing both: not a riddle, not a coin toss.
        assert_eq!(from_roles(&[MALE_ROLE, FEMALE_ROLE]), Pronouns::They);
        assert_eq!(from_roles(&[FEMALE_ROLE, MALE_ROLE, MYSTERY_ROLE]), Pronouns::They);
        // And a role among other roles still counts.
        assert_eq!(from_roles(&[42, MALE_ROLE, 7]), Pronouns::He);
        assert_eq!(from_roles(&[7, FEMALE_ROLE]), Pronouns::She);
        // Mystery beats the other two: somebody wearing it has already answered.
        assert_eq!(from_roles(&[FEMALE_ROLE, MYSTERY_ROLE]), Pronouns::They);
        assert_eq!(from_roles(&[MALE_ROLE, MYSTERY_ROLE]), Pronouns::They);
    }

    /// A member the bot cannot look up at all is they/them, never nothing.
    #[test]
    fn an_unknown_member_is_they_them() {
        assert_eq!(of(404), Pronouns::They, "no gateway in a test, and no guess either");
        assert_eq!(Pronouns::default(), Pronouns::They);
        assert_eq!(for_users([1u64, 2]).values().copied().collect::<Vec<_>>(), vec![Pronouns::They, Pronouns::They]);
    }

    #[test]
    fn the_words_are_the_words() {
        assert_eq!((Pronouns::He.words(), Pronouns::He.subject(), Pronouns::He.object(), Pronouns::He.possessive()), ("he/him", "he", "him", "his"));
        assert_eq!(
            (Pronouns::She.words(), Pronouns::She.subject(), Pronouns::She.object(), Pronouns::She.possessive()),
            ("she/her", "she", "her", "her")
        );
        assert_eq!(
            (Pronouns::They.words(), Pronouns::They.subject(), Pronouns::They.object(), Pronouns::They.possessive()),
            ("they/them", "they", "them", "their")
        );
    }

    /// The block names everybody it is given, and says the rest are they/them.
    #[test]
    fn the_block_names_everyone_and_the_rule_forbids_guessing() {
        let b = block(&[("gooner".into(), Pronouns::He), ("riya".into(), Pronouns::She), ("mystery one".into(), Pronouns::They)]);
        assert!(b.contains("- gooner: he/him"), "{b}");
        assert!(b.contains("- riya: she/her"), "{b}");
        assert!(b.contains("- mystery one: they/them"), "{b}");
        assert!(b.contains("they/them"), "and the default is spelled out for anyone missing");
        assert!(b.ends_with('\n'));
        // The same name twice is one line.
        let twice = block(&[("dev".into(), Pronouns::He), ("Dev".into(), Pronouns::She)]);
        assert_eq!(twice.lines().filter(|l| l.starts_with("- ")).count(), 1, "{twice}");
        // Nobody at all still answers the question.
        assert!(block(&[]).contains("they/them"));

        let r = RULE.to_lowercase();
        for must in ["never guess", "gender", "they/them", "exactly"] {
            assert!(r.contains(must), "the rule dropped “{must}”");
        }
    }

    /// The settings default to this server's three roles.
    #[test]
    fn the_settings_default_to_this_servers_roles() {
        assert_eq!((male_role(), female_role(), mystery_role()), (MALE_ROLE, FEMALE_ROLE, MYSTERY_ROLE));
    }

    /// `block_for` fills the gaps with they/them rather than leaving somebody out.
    #[test]
    fn a_member_with_no_roles_on_record_is_still_named_as_they_them() {
        let known: HashMap<u64, Pronouns> = [(11u64, Pronouns::He)].into_iter().collect();
        let b = block_for(&[(11, "gooner".into()), (22, "stranger".into())], &known);
        assert!(b.contains("- gooner: he/him"), "{b}");
        assert!(b.contains("- stranger: they/them"), "somebody the cache never saw is named, not skipped: {b}");
    }
}
