//! What the panel shows: every feature, its settings and its commands, with the
//! words an admin needs to use them. The panel draws itself from this, so a new
//! setting only has to be described here.

use serde::Serialize;

/// How a setting is edited.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// On or off, stored as "on"/"off".
    Toggle,
    /// One text channel id.
    Channel,
    /// Comma-separated text channel ids.
    Channels,
    /// `id:weight` pairs, comma-separated (e.g. how often the Snitch picks a channel).
    WeightedChannels,
    /// A voice channel id.
    VoiceChannel,
    Role,
    /// Comma-separated member ids.
    Users,
    Number { min: i64, max: i64, unit: &'static str },
    Decimal { min: f64, max: f64, unit: &'static str },
    Text,
    /// A time of day in India time, "HH:MM".
    Time,
    /// One of fixed values: (value, label).
    Choice { options: &'static [(&'static str, &'static str)] },
}

#[derive(Clone, Debug, Serialize)]
pub struct Setting {
    /// The stored key; the same name the environment variable had.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// What applies when neither the panel nor the environment sets it.
    pub default: &'static str,
    /// False when a change only takes effect after the bot restarts.
    pub live: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Command {
    /// Without the slash.
    pub name: &'static str,
    /// "Everyone", "Admins", "House captains".
    pub who: &'static str,
    /// e.g. "/fight @someone type:"
    pub usage: &'static str,
    pub what: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    /// What the feature does, in a few sentences.
    pub about: &'static str,
    pub settings: Vec<Setting>,
    pub commands: Vec<Command>,
}

/// Every section, in the order the panel lists them.
pub fn sections() -> Vec<Section> {
    vec![Section {
        id: "arena",
        title: "Arena",
        icon: "⚔️",
        about: "1v1 fights with /fight and battle royales with /battle.",
        settings: vec![Setting {
            key: "VIZIER_FIGHT_CHANNEL",
            label: "Fight channel",
            help: "Where challenges and fights are posted. If empty, a channel named fight-fight-fight is used.",
            kind: Kind::Channel,
            default: "",
            live: true,
        }],
        commands: vec![Command {
            name: "fight",
            who: "Everyone",
            usage: "/fight @someone type:",
            what: "Challenge someone to a 1v1.",
        }],
    }]
}

/// Every key the catalog describes.
pub fn keys() -> Vec<&'static str> {
    sections().iter().flat_map(|s| s.settings.iter().map(|x| x.key)).collect()
}

/// The description of one key, if the catalog has it.
pub fn find(key: &str) -> Option<Setting> {
    sections().into_iter().flat_map(|s| s.settings).find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_described() {
        let mut seen = std::collections::HashSet::new();
        for section in sections() {
            assert!(!section.about.trim().is_empty(), "{} has no description", section.id);
            for s in &section.settings {
                assert!(seen.insert(s.key), "{} is listed twice", s.key);
                assert!(!s.label.trim().is_empty() && !s.help.trim().is_empty(), "{} needs a label and help", s.key);
            }
        }
    }
}
