//! Guild ceilings and the additional headroom granted by temp voice perks.

use super::*;
use store::RolePerk;

pub(super) struct Caps {
    limit: u32,
    pub(super) bitrate_kbps: u32,
}

impl Caps {
    pub(super) fn limit(&self, requested: u32) -> u32 {
        match (self.limit, requested) {
            (0, _) => requested.min(99),
            (cap, 0) => cap,
            (cap, requested) => requested.min(cap),
        }
    }

    pub(super) fn bitrate(&self, bps: u32) -> u32 {
        bps.clamp(8_000, self.bitrate_kbps * 1_000)
    }
}

fn resolve(base: Caps, guild_max: u32, staff: bool, roles: &[RoleId], perks: &[RolePerk]) -> Caps {
    if staff {
        return Caps {
            limit: 0,
            bitrate_kbps: guild_max,
        };
    }
    let mut caps = base;
    for perk in perks
        .iter()
        .filter(|perk| roles.contains(&RoleId::new(perk.role_id)))
    {
        caps.limit = match (caps.limit, perk.limit) {
            (0, _) | (_, 0) => 0,
            (base, perk) => base.max(perk),
        };
        caps.bitrate_kbps = caps.bitrate_kbps.max(perk.bitrate / 1_000);
    }
    caps.bitrate_kbps = caps.bitrate_kbps.clamp(8, guild_max);
    caps
}

pub(super) fn for_member(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    staff: bool,
) -> Result<Caps> {
    let guild_max = actions::bitrate_cap(ctx, guild)?;
    let base = Caps {
        limit: control::number("VIZIER_TEMPVC_MAX_LIMIT", 0).min(99) as u32,
        bitrate_kbps: control::number("VIZIER_TEMPVC_MAX_BITRATE_KBPS", u64::from(guild_max))
            .min(u64::from(guild_max)) as u32,
    };
    let perks = if staff {
        Vec::new()
    } else {
        store::role_perks(&*db()?, guild.get())?
    };
    Ok(resolve(base, guild_max, staff, &member.roles, &perks))
}

fn has_role(guild: GuildId, roles: &[RoleId], required: Option<u64>) -> bool {
    required.is_none_or(|role| role == guild.get() || roles.iter().any(|id| id.get() == role))
}

pub(super) fn has_required_role(member: &Member, required: Option<u64>) -> bool {
    has_role(member.guild_id, &member.roles, required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_apply_per_dimension_and_respect_the_guild_max() {
        let base = || Caps {
            limit: 8,
            bitrate_kbps: 32,
        };
        let perks = [
            RolePerk {
                role_id: 1,
                limit: 20,
                bitrate: 24_000,
            },
            RolePerk {
                role_id: 2,
                limit: 4,
                bitrate: 256_000,
            },
            RolePerk {
                role_id: 3,
                limit: 0,
                bitrate: 384_000,
            },
        ];
        let ordinary = resolve(base(), 96, false, &[], &perks);
        assert_eq!(ordinary.limit(0), 8);
        assert_eq!(ordinary.limit(2), 2);
        assert_eq!(ordinary.bitrate(48_000), 32_000);
        let boosted = resolve(base(), 96, false, &[RoleId::new(1), RoleId::new(2)], &perks);
        assert_eq!(boosted.limit(99), 20);
        assert_eq!(boosted.bitrate(128_000), 96_000);
        let unlimited = resolve(base(), 96, false, &[RoleId::new(3)], &perks);
        assert_eq!(unlimited.limit(0), 0);
        let uncapped = resolve(
            Caps {
                limit: 0,
                bitrate_kbps: 96,
            },
            96,
            false,
            &[RoleId::new(1)],
            &perks,
        );
        assert_eq!(uncapped.limit(0), 0);
        let staff = resolve(base(), 96, true, &[], &perks);
        assert_eq!(staff.limit(99), 99);
        assert_eq!(staff.limit(0), 0);
        assert_eq!(staff.bitrate(256_000), 96_000);
    }

    #[test]
    fn hub_access_requires_the_configured_role() {
        let guild = GuildId::new(10);
        let roles = [RoleId::new(20)];
        assert!(has_role(guild, &roles, None));
        assert!(has_role(guild, &roles, Some(10)));
        assert!(has_role(guild, &roles, Some(20)));
        assert!(!has_role(guild, &roles, Some(30)));
        assert!(!has_role(guild, &[], Some(20)));
    }
}
