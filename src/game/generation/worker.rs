use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::game::domain::board::Board;
use crate::game::domain::item::ItemRegistry;
use crate::game::domain::job_queue::JobQueue;
use crate::game::engine::{self, GenerationJob};
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::quest_generator::QuestGenerator;
use crate::game::persistence::storage;

/// Side effect from processing a generation job that the Discord layer may need to react to.
pub enum WorkerEffect {
    QuestResultStored { quest_id: u32 },
    BoardRefilled { added: usize },
    QuestCreationFailed,
    GenerateResultFailed { quest_id: u32 },
}

/// Process a single generation job. Mutates `board` and `queue`, returns side effects.
pub async fn process_job(
    job: GenerationJob,
    generator: &QuestGenerator,
    item_generator: &ItemGenerator,
    item_registry: &ItemRegistry,
    board: &mut Board,
    queue: &mut JobQueue,
    max_jobs: usize,
    pending_quests: &AtomicUsize,
) -> anyhow::Result<Vec<WorkerEffect>> {
    match job {
        GenerationJob::QuestResult {
            board_quest,
            player,
            force_item_drop,
        } => {
            match engine::generate_result(
                generator,
                item_generator,
                item_registry,
                &board_quest,
                &player,
                force_item_drop,
            )
            .await
            {
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
                    engine::enqueue_quest(queue, quest_data, generated)?;
                    pending_quests.fetch_sub(1, Ordering::SeqCst);
                    let added = engine::refill_board_from_queue(board, queue, max_jobs);
                    storage::save_board(board)?;
                    storage::save_job_queue(queue)?;
                    tracing::info!(queued = queue.entries.len(), added, "quest queued");
                    Ok(vec![WorkerEffect::BoardRefilled { added }])
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
    job_queue: Arc<tokio::sync::Mutex<JobQueue>>,
    item_registry: Arc<tokio::sync::Mutex<ItemRegistry>>,
    generator: Arc<QuestGenerator>,
    item_generator: Arc<ItemGenerator>,
    max_jobs: usize,
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
                let mut q = job_queue.lock().await;
                let registry = item_registry.lock().await;
                match process_job(
                    job,
                    &generator,
                    &item_generator,
                    &registry,
                    &mut *b,
                    &mut *q,
                    max_jobs,
                    &pending_quests,
                )
                .await
                {
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
