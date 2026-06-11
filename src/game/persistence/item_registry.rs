use serde::{Deserialize, Serialize};

use crate::game::domain::item::Item;
use crate::game::tuneable_rolls::roll_item_value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemRegistry {
    pub items: Vec<Item>,
    pub next_id: u32,
}

impl ItemRegistry {
    pub fn get(&self, id: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Roll values for legacy items saved before `value` existed (`value == 0`).
    pub fn backfill_missing_values(&mut self) -> bool {
        let mut backfilled = false;
        let mut rng = rand::thread_rng();
        for item in &mut self.items {
            if item.value == 0 {
                item.value = roll_item_value(&item.stats, &item.rarity, &mut rng);
                tracing::warn!(
                    item_id = item.id,
                    name = %item.name,
                    net_stat = item.stats.net_stat_value(),
                    value = item.value,
                    "backfilled missing item value on load"
                );
                backfilled = true;
            }
        }
        backfilled
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
