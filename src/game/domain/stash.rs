use serde::{Deserialize, Serialize};

pub const STASH_CAPACITY: usize = 5;

/// Unequipped items owned by a player (item registry ids).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Stash {
    items: Vec<u32>,
}

impl Stash {
    pub fn has_room(&self) -> bool {
        self.items.len() < STASH_CAPACITY
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, id: u32) -> bool {
        self.items.contains(&id)
    }

    pub fn items(&self) -> &[u32] {
        &self.items
    }

    pub fn remove(&mut self, id: u32) -> bool {
        if let Some(pos) = self.items.iter().position(|&owned| owned == id) {
            self.items.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn push(&mut self, id: u32) -> anyhow::Result<()> {
        if self.items.len() >= STASH_CAPACITY {
            anyhow::bail!("stash is full");
        }
        if self.items.contains(&id) {
            anyhow::bail!("item already in stash");
        }
        self.items.push(id);
        Ok(())
    }
}
