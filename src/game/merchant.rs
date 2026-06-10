use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::game::domain::item::Item;
use crate::game::domain::item::ItemSeed;
use crate::game::domain::player::Player;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;
use crate::game::tuneable_rolls::roll_item;

pub const MERCHANT_VISIT_CHANCE: f64 = 0.20;
pub const MERCHANT_VISIT_STOCK_SIZE: usize = 5;
pub const MERCHANT_STAY_TICKS: (u8, u8) = (2, 5);

pub const MERCHANT_ROSTER_SIZE: usize = 5;
pub const MERCHANT_LOW_TIER_COUNT: usize = 3;
pub const MERCHANT_STOCK_POOL_SIZE: usize = 15;
pub const MERCHANT_LOW_DIFF_MIN: u8 = 1;
pub const MERCHANT_LOW_DIFF_MAX: u8 = 5;
pub const MERCHANT_HIGH_DIFF_MIN: u8 = 6;
pub const MERCHANT_HIGH_DIFF_MAX: u8 = 10;

pub const DUMPSTER_DAVE_INDEX: usize = 0;
pub const DUMPSTER_DAVE_NAME: &str = "Dumpster Dave";
pub const DUMPSTER_DAVE_THEME: &str = "A grease-stained salvage hawker who resells everything chuds pawn from the guild dumpster. His corner of the market smells like wet cardboard, brass polish, and items nobody will admit they owned.";

/// Static recycling-bin merchant; `stock_pool` is populated at runtime from player sells.
pub fn dumpster_dave_template() -> MerchantDefinition {
    MerchantDefinition {
        name: DUMPSTER_DAVE_NAME.to_string(),
        theme: DUMPSTER_DAVE_THEME.to_string(),
        stock_pool: Vec::new(),
    }
}

/// True when the catalog only contains Dumpster Dave (or is empty beyond him).
pub fn catalog_needs_generated_roster(catalog: &MerchantCatalog) -> bool {
    catalog.merchants.len() <= DUMPSTER_DAVE_INDEX + 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MerchantTickEvent {
    None,
    Spawned,
    Departed,
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyError {
    SoldOut,
    InsufficientFunds,
    StashFull,
    InvalidSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForcedSpawnError {
    AlreadyVisiting,
    NoCatalog,
    MerchantNotFound,
    MerchantEmpty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantDefinition {
    pub name: String,
    pub theme: String,
    pub stock_pool: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantCatalog {
    pub merchants: Vec<MerchantDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantVisit {
    pub merchant_index: usize,
    pub merchant_name: String,
    pub ticks_remaining: u8,
    pub stock: Vec<u32>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MerchantState {
    /// Loaded from `data/merchants.yaml`; not stored in `guild_hall.yaml`.
    #[serde(skip, default)]
    pub catalog: Option<MerchantCatalog>,
    pub visit: Option<MerchantVisit>,
    /// Runtime-only admin flag; not persisted across restarts.
    #[serde(skip, default)]
    pub spawn_next_tick: bool,
    /// When set with `spawn_next_tick`, force the next visit to this merchant name.
    #[serde(skip, default)]
    pub spawn_merchant_name: Option<String>,
}

impl MerchantState {
    pub fn set_spawn_flag(
        &mut self,
        merchant_name: Option<String>,
    ) -> Result<(), ForcedSpawnError> {
        if self.visit.is_some() {
            return Err(ForcedSpawnError::AlreadyVisiting);
        }
        if let Some(name) = merchant_name.as_deref() {
            let catalog = self.catalog.as_ref().ok_or(ForcedSpawnError::NoCatalog)?;
            let merchant = catalog
                .merchants
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(name))
                .ok_or(ForcedSpawnError::MerchantNotFound)?;
            if merchant.stock_pool.is_empty() {
                return Err(ForcedSpawnError::MerchantEmpty);
            }
        }
        self.spawn_next_tick = true;
        self.spawn_merchant_name = merchant_name;
        Ok(())
    }

    pub fn advance_tick(&mut self, force: bool) -> MerchantTickEvent {
        let preferred_name = if force {
            self.spawn_merchant_name.take()
        } else {
            None
        };

        if force && self.visit.is_none() {
            if let Some(visit) = self.try_spawn_visit(preferred_name.as_deref()) {
                self.visit = Some(visit);
                return MerchantTickEvent::Spawned;
            }
            return MerchantTickEvent::None;
        }

        if self.visit.is_none() {
            let mut rng = rand::thread_rng();
            if rng.gen_bool(MERCHANT_VISIT_CHANCE) {
                if let Some(visit) = self.try_spawn_visit(None) {
                    self.visit = Some(visit);
                    return MerchantTickEvent::Spawned;
                }
            }
            return MerchantTickEvent::None;
        }

        let visit = self.visit.as_mut().unwrap();
        visit.ticks_remaining = visit.ticks_remaining.saturating_sub(1);
        if visit.ticks_remaining == 0 {
            self.visit = None;
            return MerchantTickEvent::Departed;
        }
        MerchantTickEvent::Tick
    }

    pub fn buy(
        &mut self,
        slot: usize,
        player: &mut Player,
        registry: &ItemRegistry,
    ) -> Result<Item, BuyError> {
        let visit = self.visit.as_mut().ok_or(BuyError::SoldOut)?;
        let id = *visit.stock.get(slot).ok_or(BuyError::InvalidSlot)?;

        let item = registry.get(id).ok_or(BuyError::SoldOut)?;
        let price = item.value;
        if player.cash < price {
            return Err(BuyError::InsufficientFunds);
        }
        if !player.stash.has_room() {
            return Err(BuyError::StashFull);
        }

        player.cash = player.cash.saturating_sub(price);
        player.stash.push(id).map_err(|_| BuyError::StashFull)?;
        visit.stock.remove(slot);

        if let Some(catalog) = self.catalog.as_mut() {
            if let Some(merchant) = catalog.merchants.get_mut(visit.merchant_index) {
                merchant.stock_pool.retain(|pool_id| *pool_id != id);
            }
        }

        let awarded = registry.get(id).expect("item missing after buy").clone();

        storage::save_player(player).expect("save player after merchant buy");
        storage::save_merchant_state(self).expect("save merchant state after buy");

        Ok(awarded)
    }

    pub fn first_stock_slot(&self) -> Option<usize> {
        let visit = self.visit.as_ref()?;
        (!visit.stock.is_empty()).then_some(0)
    }

    /// Ensure a catalog exists with Dumpster Dave at index 0.
    pub fn ensure_catalog_with_dave(&mut self) {
        if self.catalog.is_none() {
            self.catalog = Some(MerchantCatalog {
                merchants: vec![dumpster_dave_template()],
            });
            return;
        }
        let catalog = self.catalog.as_mut().unwrap();
        if catalog.merchants.first().map(|m| m.name.as_str()) != Some(DUMPSTER_DAVE_NAME) {
            let pool = catalog
                .merchants
                .iter()
                .find(|m| m.name == DUMPSTER_DAVE_NAME)
                .map(|m| m.stock_pool.clone())
                .unwrap_or_default();
            let mut dave = dumpster_dave_template();
            dave.stock_pool = pool;
            catalog
                .merchants
                .retain(|m| m.name != DUMPSTER_DAVE_NAME);
            catalog.merchants.insert(DUMPSTER_DAVE_INDEX, dave);
        }
    }

    /// Move a sold item into Dumpster Dave's stock pool instead of deleting it from the registry.
    pub fn recycle_item(&mut self, item_id: u32) -> anyhow::Result<()> {
        self.ensure_catalog_with_dave();
        let dave = &mut self
            .catalog
            .as_mut()
            .unwrap()
            .merchants[DUMPSTER_DAVE_INDEX];
        if !dave.stock_pool.contains(&item_id) {
            dave.stock_pool.push(item_id);
        }
        if let Some(visit) = &mut self.visit {
            if visit.merchant_index == DUMPSTER_DAVE_INDEX {
                visit.stock.retain(|id| *id != item_id);
            }
        }
        storage::save_merchant_state(self)?;
        Ok(())
    }

    fn try_spawn_visit(&self, preferred_name: Option<&str>) -> Option<MerchantVisit> {
        let catalog = self.catalog.as_ref()?;
        let available: Vec<usize> = catalog
            .merchants
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.stock_pool.is_empty())
            .map(|(i, _)| i)
            .collect();
        if available.is_empty() {
            return None;
        }

        let mut rng = rand::thread_rng();
        let merchant_index = if let Some(name) = preferred_name {
            available
                .into_iter()
                .find(|&i| catalog.merchants[i].name.eq_ignore_ascii_case(name))?
        } else {
            *available.choose(&mut rng)?
        };
        let merchant = &catalog.merchants[merchant_index];
        let sample_count = MERCHANT_VISIT_STOCK_SIZE.min(merchant.stock_pool.len());
        let stock: Vec<u32> = merchant
            .stock_pool
            .choose_multiple(&mut rng, sample_count)
            .copied()
            .collect();
        let (min, max) = MERCHANT_STAY_TICKS;
        Some(MerchantVisit {
            merchant_index,
            merchant_name: merchant.name.clone(),
            ticks_remaining: rng.gen_range(min..=max),
            stock,
        })
    }
}

/// Roll item seeds for a merchant at the given roster index.
pub fn roll_merchant_stock_seeds(merchant_index: usize) -> Vec<ItemSeed> {
    let mut rng = rand::thread_rng();
    let (min, max) = if merchant_index < MERCHANT_LOW_TIER_COUNT {
        (MERCHANT_LOW_DIFF_MIN, MERCHANT_LOW_DIFF_MAX)
    } else {
        (MERCHANT_HIGH_DIFF_MIN, MERCHANT_HIGH_DIFF_MAX)
    };

    (0..MERCHANT_STOCK_POOL_SIZE)
        .map(|_| {
            let difficulty = rng.gen_range(min..=max);
            roll_item(difficulty, &mut rng, None, None)
        })
        .collect()
}
