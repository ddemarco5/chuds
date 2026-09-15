use serde::{Deserialize, Serialize};

use crate::game::domain::item::Item;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
