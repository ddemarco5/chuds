use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::persistence::storage;

#[derive(Debug, Default)]
pub struct GameState {
    pub board: Board,
    pub hospital: Hospital,
}

impl GameState {
    pub fn load() -> anyhow::Result<Self> {
        Ok(Self {
            board: storage::load_board()?,
            hospital: storage::load_hospital()?,
        })
    }

    pub fn save(&self) -> anyhow::Result<()> {
        storage::save_board(&self.board)?;
        storage::save_hospital(&self.hospital)?;
        Ok(())
    }
}
