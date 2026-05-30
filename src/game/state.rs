use crate::game::domain::board::Board;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::job_queue::JobQueue;
use crate::game::persistence::storage;

#[derive(Debug, Default)]
pub struct GameState {
    pub board: Board,
    pub hospital: Hospital,
    pub job_queue: JobQueue,
}

impl GameState {
    pub fn load() -> anyhow::Result<Self> {
        Ok(Self {
            board: storage::load_board()?,
            hospital: storage::load_hospital()?,
            job_queue: storage::load_job_queue()?,
        })
    }

    pub fn save(&self) -> anyhow::Result<()> {
        storage::save_board(&self.board)?;
        storage::save_hospital(&self.hospital)?;
        storage::save_job_queue(&self.job_queue)?;
        Ok(())
    }
}
