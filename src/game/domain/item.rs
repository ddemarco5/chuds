use serde::{Deserialize, Serialize};

use crate::game::domain::player::{ChudEquipment, Player};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    Gear,
    Weapon,
    Misc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemStats {
    pub strength: String,
    pub smarts: String,
    pub stealth: String,
}

impl ItemStats {
    pub fn zero() -> Self {
        Self {
            strength: "-".into(),
            smarts: "-".into(),
            stealth: "-".into(),
        }
    }

    pub fn total_floor(&self) -> (u8, u8, u8) {
        (
            parse_stat_floor(&self.strength),
            parse_stat_floor(&self.smarts),
            parse_stat_floor(&self.stealth),
        )
    }

    pub fn total_modifier(&self) -> (i16, i16, i16) {
        (
            parse_stat_modifier(&self.strength),
            parse_stat_modifier(&self.smarts),
            parse_stat_modifier(&self.stealth),
        )
    }

    pub fn format_triplet(&self) -> String {
        format!(
            "{},{},{}",
            format_stat_display(&self.strength),
            format_stat_display(&self.smarts),
            format_stat_display(&self.stealth),
        )
    }
}

/// Whether a stat string means no effect on that attribute.
pub fn is_neutral_stat(s: &str) -> bool {
    matches!(s.trim(), "" | "0" | "-")
}

/// Format a stat for display; neutral values show as "-".
pub fn format_stat_display(s: &str) -> String {
    if is_neutral_stat(s) {
        "-".to_string()
    } else {
        s.to_string()
    }
}

/// Bare positive integer = roll floor; signed prefix = post-roll modifier.
pub fn parse_stat_floor(s: &str) -> u8 {
    if is_neutral_stat(s) {
        return 0;
    }
    let s = s.trim();
    if s.starts_with('+') || s.starts_with('-') {
        0
    } else {
        s.parse().unwrap_or(0)
    }
}

pub fn parse_stat_modifier(s: &str) -> i16 {
    if is_neutral_stat(s) {
        return 0;
    }
    let s = s.trim();
    if s.starts_with('+') || s.starts_with('-') {
        s.parse().unwrap_or(0)
    } else {
        0
    }
}

/// Format a player roll for display: ↑/↓ with final when modified, ⌊⌋ when floor-only.
pub fn format_roll_breakdown(modifier: i16, floor_applied: bool, final_roll: u8) -> String {
    if modifier > 0 {
        format!("\u{2191}{final_roll}")
    } else if modifier < 0 {
        format!("\u{2193}{final_roll}")
    } else if floor_applied {
        format!("\u{230a}{final_roll}\u{230b}")
    } else {
        final_roll.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: u32,
    pub name: String,
    pub item_type: ItemType,
    pub subtype: String,
    pub stats: ItemStats,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemSeed {
    pub item_type: ItemType,
    pub subtype: String,
    pub stats: ItemStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
}

impl Item {
    /// e.g. "Wearing A pair of translucent clogs..."
    pub fn equipped_description_sentence(&self) -> Option<String> {
        let verb = match self.item_type {
            ItemType::Weapon => "Holding",
            ItemType::Gear => "Wearing",
            ItemType::Misc if self.subtype == "trinket" => "Wearing",
            ItemType::Misc => return None,
        };
        Some(format!("{verb} {}", self.description))
    }
}

impl ItemSeed {
    pub fn into_item(self, name: String, description: String) -> Item {
        Item {
            id: 0,
            name,
            item_type: self.item_type,
            subtype: self.subtype,
            stats: self.stats,
            description,
            trigger: self.trigger,
            effect: self.effect,
        }
    }
}

impl ChudEquipment {
    pub fn all_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.gear
            .into_iter()
            .chain(self.weapon)
            .chain(self.misc.into_iter().flatten())
    }
}

/// Equip an item on a chud, returning the replaced item id if any.
pub fn equip_item(player: &mut Player, item: Item) -> Option<u32> {
    let slot = match item.item_type {
        ItemType::Gear => &mut player.chud.equipment.gear,
        ItemType::Weapon => &mut player.chud.equipment.weapon,
        ItemType::Misc => {
            if player.chud.equipment.misc[0].is_none() {
                &mut player.chud.equipment.misc[0]
            } else if player.chud.equipment.misc[1].is_none() {
                &mut player.chud.equipment.misc[1]
            } else {
                &mut player.chud.equipment.misc[0]
            }
        }
    };
    slot.replace(item.id)
}
