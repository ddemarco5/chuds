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

    pub fn total_bonus(&self) -> (i16, i16, i16) {
        (
            parse_stat_contribution(&self.strength),
            parse_stat_contribution(&self.smarts),
            parse_stat_contribution(&self.stealth),
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

/// Positive/negative prefix = offset; bare positive integer = floor (additive bonus).
pub fn parse_stat_contribution(s: &str) -> i16 {
    if is_neutral_stat(s) {
        return 0;
    }
    let s = s.trim();
    if s.starts_with('+') || s.starts_with('-') {
        s.parse().unwrap_or(0)
    } else {
        s.parse::<u8>().unwrap_or(0) as i16
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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ItemRegistry {
    pub items: Vec<Item>,
    pub next_id: u32,
}

impl ItemRegistry {
    pub fn get(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn add_item(&mut self, mut item: Item) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        item.id = id;
        self.items.push(item);
        id
    }

    pub fn remove(&mut self, id: u32) -> Option<Item> {
        if let Some(pos) = self.items.iter().position(|i| i.id == id) {
            Some(self.items.remove(pos))
        } else {
            None
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
