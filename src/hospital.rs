use std::path::Path;

use anyhow::Context;
use rand_distr::{Distribution, Normal};
use serde::{Deserialize, Serialize};

const HOSPITAL_PATH: &str = "data/hospital.yaml";

/// Base heal price per tick of recovery. Increase to make hospital stays more expensive.
const HEAL_PRICE_PER_TICK_BASE: f64 = 5.0;
/// Price jitter: multiplier sampled from Normal(mean=1.0, stddev=HEAL_PRICE_JITTER_STDDEV).
/// 0.20 means ~68% of prices within ±20% of base, ~95% within ±40%. Raise to widen price variance.
const HEAL_PRICE_JITTER_STDDEV: f64 = 0.20;

/// A chud recovering in the hospital.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HospitalEntry {
    pub discord_user_id: u64,
    pub chud_name: String,
    pub ticks_remaining: u32,
    pub heal_price: u32,
}

/// Hospital state containing all recovering chuds.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Hospital {
    pub entries: Vec<HospitalEntry>,
}

impl Hospital {
    /// Load the hospital from disk, or return empty if no file exists.
    pub fn load() -> anyhow::Result<Self> {
        if !Path::new(HOSPITAL_PATH).exists() {
            return Ok(Self::default());
        }
        let yaml = std::fs::read_to_string(HOSPITAL_PATH).context("reading hospital")?;
        serde_yaml::from_str(&yaml).context("parsing hospital")
    }

    /// Save the hospital to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all("data")?;
        let yaml = serde_yaml::to_string(self).context("serializing hospital")?;
        std::fs::write(HOSPITAL_PATH, yaml).context("writing hospital")
    }
    /// Admit a chud to the hospital. Returns the admission message if successful.
    pub fn admit(&mut self, discord_user_id: u64, chud_name: String, ticks: u32) -> Option<String> {
        // Remove existing entry if present (shouldn't happen, but be safe)
        self.entries.retain(|e| e.discord_user_id != discord_user_id);

        // Calculate heal price with jitter based on ticks
        let base_price = HEAL_PRICE_PER_TICK_BASE * ticks as f64;
        let mut rng = rand::thread_rng();
        let normal = Normal::new(1.0, HEAL_PRICE_JITTER_STDDEV).expect("valid normal distribution");
        let multiplier = normal.sample(&mut rng).max(0.0);
        let heal_price = (base_price * multiplier).round() as u32;

        self.entries.push(HospitalEntry {
            discord_user_id,
            chud_name: chud_name.clone(),
            ticks_remaining: ticks,
            heal_price,
        });

        Some(crate::chud_msg!("chud_hospital_admit", chud_name))
    }

    /// Release a chud from the hospital manually. Returns the release message if found.
    pub fn release(&mut self, discord_user_id: u64) -> Option<String> {
        let idx = self.entries.iter().position(|e| e.discord_user_id == discord_user_id)?;
        let entry = self.entries.remove(idx);
        Some(crate::chud_msg!("chud_hospital_release", entry.chud_name))
    }

    /// Process one tick: decrement all entries, return messages for those released.
    pub fn tick(&mut self) -> Vec<String> {
        // Decrement all entries
        for entry in &mut self.entries {
            entry.ticks_remaining = entry.ticks_remaining.saturating_sub(1);
        }

        // Collect IDs of entries ready for release (ticks_remaining == 0)
        let to_release: Vec<u64> = self
            .entries
            .iter()
            .filter(|e| e.ticks_remaining == 0)
            .map(|e| e.discord_user_id)
            .collect();

        // Use release() for each to avoid duplicate code
        to_release
            .into_iter()
            .filter_map(|id| self.release(id))
            .collect()
    }

    /// Check if a chud is currently hospitalized.
    pub fn is_hospitalized(&self, discord_user_id: u64) -> bool {
        self.entries.iter().any(|e| e.discord_user_id == discord_user_id)
    }

    /// Get the remaining ticks for a hospitalized chud.
    pub fn get_ticks_remaining(&self, discord_user_id: u64) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.discord_user_id == discord_user_id)
            .map(|e| e.ticks_remaining)
    }

    /// Get the chud name for a hospitalized chud.
    pub fn get_chud_name(&self, discord_user_id: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.discord_user_id == discord_user_id)
            .map(|e| e.chud_name.as_str())
    }

    /// Get the heal price for a hospitalized chud.
    pub fn get_heal_price(&self, discord_user_id: u64) -> Option<u32> {
        self.entries
            .iter()
            .find(|e| e.discord_user_id == discord_user_id)
            .map(|e| e.heal_price)
    }
}
