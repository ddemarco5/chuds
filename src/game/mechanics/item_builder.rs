use rand::Rng;

use crate::game::domain::item::{parse_stat_contribution, ItemSeed, ItemStats, ItemType};

pub const ITEM_DROP_CHANCE: f64 = 0.08;

const GEAR_SUBTYPES: &[&str] = &["helmet", "chest", "legs", "feet", "hands"];
const MISC_SUBTYPES: &[&str] = &["trinket"]; // we want to add consumables and others in the future

pub fn roll_item_drop(rng: &mut impl Rng) -> bool {
    rng.gen_bool(ITEM_DROP_CHANCE)
}

pub fn roll_item(difficulty: u8, rng: &mut impl Rng) -> ItemSeed {
    let item_type = roll_item_type(rng);
    let subtype = roll_subtype(item_type, rng);
    let stats = roll_item_stats(difficulty, rng);
    ItemSeed {
        item_type,
        subtype,
        stats,
        trigger: None,
        effect: None,
    }
}

fn roll_item_type(rng: &mut impl Rng) -> ItemType {
    match rng.gen_range(0..3) {
        0 => ItemType::Gear,
        1 => ItemType::Weapon,
        _ => ItemType::Misc,
    }
}

fn roll_subtype(item_type: ItemType, rng: &mut impl Rng) -> String {
    match item_type {
        ItemType::Gear => GEAR_SUBTYPES[rng.gen_range(0..GEAR_SUBTYPES.len())].to_string(),
        ItemType::Weapon => String::new(),
        ItemType::Misc => MISC_SUBTYPES[rng.gen_range(0..MISC_SUBTYPES.len())].to_string(),
    }
}

fn roll_item_stats(difficulty: u8, rng: &mut impl Rng) -> ItemStats {
    let cap = (difficulty / 2).max(1);
    let mut stats = ItemStats {
        strength: roll_single_stat(cap, rng),
        smarts: roll_single_stat(cap, rng),
        stealth: roll_single_stat(cap, rng),
    };
    if !has_positive_stat(&stats) {
        let positive = roll_positive_stat(cap, rng);
        match rng.gen_range(0..3) {
            0 => stats.strength = positive,
            1 => stats.smarts = positive,
            _ => stats.stealth = positive,
        }
    }
    stats
}

fn has_positive_stat(stats: &ItemStats) -> bool {
    parse_stat_contribution(&stats.strength) > 0
        || parse_stat_contribution(&stats.smarts) > 0
        || parse_stat_contribution(&stats.stealth) > 0
}

fn roll_positive_stat(cap: u8, rng: &mut impl Rng) -> String {
    if rng.gen_bool(0.7) {
        format!("+{}", rng.gen_range(1..=cap))
    } else {
        rng.gen_range(1..=cap).to_string()
    }
}

fn roll_single_stat(cap: u8, rng: &mut impl Rng) -> String {
    let roll = rng.gen_range(0.0..1.0);
    if roll < 0.50 {
        "-".to_string()
    } else if roll < 0.85 {
        let magnitude = rng.gen_range(1..=cap);
        if rng.gen_bool(0.5) {
            format!("+{magnitude}")
        } else {
            format!("-{magnitude}")
        }
    } else {
        rng.gen_range(1..=cap).to_string()
    }
}
