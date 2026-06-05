use rand::seq::SliceRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::chud_msg;
use crate::game::domain::item::Item;
use crate::game::domain::player::Player;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;
use crate::starting_items;

pub const MERCHANT_VISIT_CHANCE: f64 = 0.10;
pub const MERCHANT_STOCK_SIZE: usize = 5;
pub const MERCHANT_STAY_TICKS: u8 = 2;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantStockSlot {
    pub item: Item,
    pub sold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerchantVisit {
    pub merchant_name: String,
    pub ticks_remaining: u8,
    pub stock: Vec<MerchantStockSlot>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MerchantState {
    pub visit: Option<MerchantVisit>,
    /// Runtime-only admin flag; not persisted across restarts.
    #[serde(skip, default)]
    pub spawn_next_tick: bool,
}

impl MerchantState {
    pub fn set_spawn_flag(&mut self) -> Result<(), &'static str> {
        if self.visit.is_some() {
            return Err("merchant already visiting");
        }
        self.spawn_next_tick = true;
        Ok(())
    }

    pub fn advance_tick(&mut self, force: bool) -> MerchantTickEvent {
        if force && self.visit.is_none() {
            self.visit = Some(spawn_visit());
            return MerchantTickEvent::Spawned;
        }

        if self.visit.is_none() {
            let mut rng = rand::thread_rng();
            if rng.gen_bool(MERCHANT_VISIT_CHANCE) {
                self.visit = Some(spawn_visit());
                return MerchantTickEvent::Spawned;
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
        registry: &mut ItemRegistry,
    ) -> Result<Item, BuyError> {
        let visit = self.visit.as_mut().ok_or(BuyError::SoldOut)?;
        let stock = visit
            .stock
            .get_mut(slot)
            .ok_or(BuyError::InvalidSlot)?;
        if stock.sold {
            return Err(BuyError::SoldOut);
        }

        let price = stock.item.value;
        if player.cash < price {
            return Err(BuyError::InsufficientFunds);
        }
        if !player.stash.has_room() {
            return Err(BuyError::StashFull);
        }

        let id = registry.add_item(stock.item.clone());
        let awarded = registry
            .get(id)
            .expect("item missing after add")
            .clone();

        player.cash = player.cash.saturating_sub(price);
        player.stash.push(id).map_err(|_| BuyError::StashFull)?;
        stock.sold = true;

        storage::save_player(player).expect("save player after merchant buy");
        storage::save_item_registry(registry).expect("save registry after merchant buy");
        storage::save_guild_hall(self).expect("save guild hall after merchant buy");

        Ok(awarded)
    }

    pub fn first_unsold_slot(&self) -> Option<usize> {
        let visit = self.visit.as_ref()?;
        visit.stock.iter().position(|s| !s.sold)
    }

    pub fn unsold_slots(&self) -> Vec<usize> {
        self.visit
            .as_ref()
            .map(|v| {
                v.stock
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| !s.sold)
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn spawn_visit() -> MerchantVisit {
    let catalog = starting_items::catalog();
    let mut rng = rand::thread_rng();
    let picks: Vec<Item> = catalog
        .choose_multiple(&mut rng, MERCHANT_STOCK_SIZE.min(catalog.len()))
        .cloned()
        .collect();
    let stock = picks
        .into_iter()
        .map(|item| MerchantStockSlot { item, sold: false })
        .collect();
    MerchantVisit {
        merchant_name: chud_msg!("merchant_names"),
        ticks_remaining: MERCHANT_STAY_TICKS,
        stock,
    }
}
