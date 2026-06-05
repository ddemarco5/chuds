use serde::{Deserialize, Serialize};

use crate::game::domain::player::Player;
use crate::game::domain::quest::TrialStats;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatChoice {
    Strength,
    Smarts,
    Stealth,
}

impl StatChoice {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Strength => "strength",
            Self::Smarts => "smarts",
            Self::Stealth => "stealth",
        }
    }

    pub fn player_stat(&self, player: &Player) -> u8 {
        match self {
            Self::Strength => player.strength,
            Self::Smarts => player.smarts,
            Self::Stealth => player.stealth,
        }
    }

    pub fn trial_required(&self, stats: &TrialStats) -> u8 {
        match self {
            Self::Strength => stats.strength,
            Self::Smarts => stats.smarts,
            Self::Stealth => stats.stealth,
        }
    }
}

pub struct TrialOutcome {
    pub stats: TrialStats,
    pub stat_used: StatChoice,
    pub raw_roll: u8,
    pub roll_modifier: i16,
    pub floor_applied: bool,
    pub player_roll: u8,
    pub trial_roll: u8,
    pub player_stat: u8,
    pub required: u8,
    pub passed: bool,
}

pub struct PlayedQuest {
    pub outcomes: Vec<TrialOutcome>,
}
