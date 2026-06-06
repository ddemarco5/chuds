use crate::game::domain::board::Board;
use crate::game::domain::graveyard::Graveyard;
use crate::game::domain::guild_hall::GuildHall;
use crate::game::domain::hospital::Hospital;
use crate::game::domain::session::GameSession;
use crate::game::domain::starting_benefits::StartingBenefits;
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::persistence::storage;

#[derive(Debug, Default)]
pub struct GameState {
    pub board: Board,
    pub hospital: Hospital,
    pub graveyard: Graveyard,
    pub starting_benefits: StartingBenefits,
    pub job_queue: JobQueue,
    pub item_registry: ItemRegistry,
    pub guild_hall: GuildHall,
    pub session: GameSession,
}

impl GameState {
    pub fn load() -> anyhow::Result<Self> {
        Ok(Self {
            board: storage::load_board()?,
            hospital: storage::load_hospital()?,
            graveyard: storage::load_graveyard()?,
            starting_benefits: storage::load_starting_benefits()?,
            job_queue: storage::load_job_queue()?,
            item_registry: storage::load_item_registry()?,
            guild_hall: storage::load_guild_hall()?,
            session: storage::load_session()?,
        })
    }

    pub fn save(&self) -> anyhow::Result<()> {
        storage::save_board(&self.board)?;
        storage::save_hospital(&self.hospital)?;
        storage::save_graveyard(&self.graveyard)?;
        storage::save_starting_benefits(&self.starting_benefits)?;
        storage::save_job_queue(&self.job_queue)?;
        storage::save_item_registry(&self.item_registry)?;
        storage::save_guild_hall_full(&self.guild_hall)?;
        storage::save_session(&self.session)?;
        Ok(())
    }
}
