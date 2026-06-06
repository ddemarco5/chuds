use serde::{Deserialize, Serialize};

/// The overall phase of the game flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GamePhase {
    /// Pre-game lobby: attract screen, players join, no simulation.
    Attract,
    /// Normal gameplay: ticks, generation, job board.
    #[default]
    Playing,
    /// Post-game: complete screen with final stats, no simulation.
    Complete,
}

/// Persisted game-flow session state, separate from board/merchant domain state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameSession {
    #[serde(default)]
    pub phase: GamePhase,
    /// Unix seconds for the attract-screen countdown (`<t:..:R>`). None until env hookup.
    // TODO: populate from an env var when the start-time feature lands.
    #[serde(default)]
    pub game_start_at: Option<i64>,
    /// Total ticks elapsed this game.
    // TODO: increment in execute_tick once the counter is hooked up.
    #[serde(default)]
    pub total_ticks: u32,
    /// Discord user ID of the chud that completed the final story mission.
    // TODO: set in quests.rs when the story series completes.
    #[serde(default)]
    pub final_completer_user_id: Option<u64>,
    /// Name of the chud that completed the final story mission.
    #[serde(default)]
    pub final_completer_chud_name: Option<String>,
}

impl GameSession {
    pub fn is_playing(&self) -> bool {
        self.phase == GamePhase::Playing
    }
}
