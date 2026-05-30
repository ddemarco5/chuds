use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::game::domain::board::Board;
use crate::game::engine::{self, GenerationJob};
use crate::game::generation::quest_generator::QuestGenerator;
use crate::game::persistence::storage;

/// Side effect from processing a generation job that the Discord layer may need to react to.
pub enum WorkerEffect {
    QuestResultStored { quest_id: u32 },
    QuestAdded { quest_id: u32 },
    QuestCreationFailed,
    GenerateResultFailed { quest_id: u32 },
}

/// Process a single generation job. Mutates `board` and returns any side effects.
pub async fn process_job(
    job: GenerationJob,
    generator: &QuestGenerator,
    board: &mut Board,
    pending_quests: &AtomicUsize,
) -> anyhow::Result<Vec<WorkerEffect>> {
    match job {
        GenerationJob::QuestResult { board_quest, player } => {
            match engine::generate_result(generator, &board_quest, &player).await {
                Ok(result) => {
                    let quest_id = board_quest.id;
                    board.completed_results.insert(quest_id, result);
                    storage::save_board(board)?;
                    tracing::info!(quest_id, "generation complete, result stored");
                    Ok(vec![WorkerEffect::QuestResultStored { quest_id }])
                }
                Err(e) => {
                    tracing::error!(
                        quest_id = board_quest.id,
                        err = %e,
                        "generate_result failed; quest will remain active and be skipped each tick"
                    );
                    Ok(vec![WorkerEffect::GenerateResultFailed {
                        quest_id: board_quest.id,
                    }])
                }
            }
        }
        GenerationJob::QuestCreation { quest_data } => {
            match generator.generate_from_description(&quest_data).await {
                Ok(generated) => {
                    tracing::info!(title = %generated.quest_title, giver = %generated.quest_giver, "quest generated");
                    let id = board.add_quest(quest_data, generated);
                    pending_quests.fetch_sub(1, Ordering::SeqCst);
                    storage::save_board(board)?;
                    tracing::info!(quest_id = id, "quest added to board");
                    Ok(vec![WorkerEffect::QuestAdded { quest_id: id }])
                }
                Err(e) => {
                    pending_quests.fetch_sub(1, Ordering::SeqCst);
                    tracing::error!(err = %e, "quest creation failed");
                    Ok(vec![WorkerEffect::QuestCreationFailed])
                }
            }
        }
    }
}

/// Spawn the background generation worker task.
pub fn spawn_generation_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<GenerationJob>,
    board: Arc<tokio::sync::Mutex<Board>>,
    generator: Arc<QuestGenerator>,
    pending_quests: Arc<AtomicUsize>,
    on_effects: impl Fn(Vec<WorkerEffect>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
) {
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            let effects = {
                let mut b = board.lock().await;
                match process_job(job, &generator, &mut *b, &pending_quests).await {
                    Ok(effects) => effects,
                    Err(e) => {
                        tracing::error!(err = %e, "generation worker job failed");
                        vec![]
                    }
                }
            };
            if !effects.is_empty() {
                on_effects(effects).await;
            }
        }
    });
}
