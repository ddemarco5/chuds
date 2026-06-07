use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use crate::game::domain::board::Board;
use crate::game::domain::job_queue::JobQueue;
use crate::game::domain::quest_result::QuestResult;
use crate::game::engine::{self, GenerationJob};
use crate::game::generation::item_generator::ItemGenerator;
use crate::game::generation::quest_generator::{GeneratedQuest, QuestData, QuestGenerator};
use crate::game::persistence::item_registry::ItemRegistry;
use crate::game::persistence::storage;

/// Side effect from processing a generation job that the Discord layer may need to react to.
pub enum WorkerEffect {
    QuestResultStored { quest_id: u32 },
    BoardRefilled { added: usize },
    QuestCreationFailed,
    GenerateResultFailed { quest_id: u32 },
}

enum GenerationOutcome {
    QuestCreated {
        quest_data: QuestData,
        generated: GeneratedQuest,
        story_index: Option<usize>,
    },
    QuestResult {
        quest_id: u32,
        result: QuestResult,
    },
    QuestCreationFailed,
    GenerateResultFailed { quest_id: u32 },
}

/// Run the slow LLM work for a generation job without holding shared runtime locks.
async fn run_generation(
    job: GenerationJob,
    generator: &QuestGenerator,
    item_generator: &ItemGenerator,
    item_registry: &Arc<tokio::sync::Mutex<ItemRegistry>>,
) -> GenerationOutcome {
    match job {
        GenerationJob::QuestResult {
            board_quest,
            player,
            force_item_drop,
        } => {
            let quest_id = board_quest.id;
            let prep = match {
                let registry = item_registry.lock().await;
                engine::prepare_quest_result(&registry, &board_quest, &player)
            } {
                Ok(prep) => prep,
                Err(e) => {
                    tracing::error!(
                        quest_id,
                        err = %e,
                        "generate_result prep failed; quest will remain active and be skipped each tick"
                    );
                    return GenerationOutcome::GenerateResultFailed { quest_id };
                }
            };
            match engine::finish_quest_result(
                generator,
                item_generator,
                &board_quest,
                &player,
                force_item_drop,
                prep,
            )
            .await
            {
                Ok(result) => GenerationOutcome::QuestResult { quest_id, result },
                Err(e) => {
                    tracing::error!(
                        quest_id,
                        err = %e,
                        "generate_result failed; quest will remain active and be skipped each tick"
                    );
                    GenerationOutcome::GenerateResultFailed { quest_id }
                }
            }
        }
        GenerationJob::QuestCreation {
            quest_data,
            story_index,
        } => match generator.generate_from_description(&quest_data).await {
            Ok(generated) => GenerationOutcome::QuestCreated {
                quest_data,
                generated,
                story_index,
            },
            Err(e) => {
                tracing::error!(err = %e, "quest creation failed");
                GenerationOutcome::QuestCreationFailed
            }
        },
    }
}

/// Apply a completed generation outcome under brief board/queue locks.
fn commit_generation(
    outcome: GenerationOutcome,
    board: &mut Board,
    queue: &mut JobQueue,
    max_jobs: usize,
    pending_quests: &AtomicUsize,
) -> anyhow::Result<Vec<WorkerEffect>> {
    match outcome {
        GenerationOutcome::QuestResult { quest_id, result } => {
            board.completed_results.insert(quest_id, result);
            storage::save_board(board)?;
            tracing::info!(quest_id, "generation complete, result stored");
            Ok(vec![WorkerEffect::QuestResultStored { quest_id }])
        }
        GenerationOutcome::QuestCreated {
            quest_data,
            generated,
            story_index,
        } => {
            let is_story = story_index.is_some();
            tracing::info!(
                title = %generated.quest_title,
                giver = %generated.quest_giver,
                story = is_story,
                "quest generated"
            );
            if is_story {
                board.add_quest(quest_data, generated, story_index);
                storage::save_board(board)?;
            } else {
                engine::enqueue_quest(queue, quest_data, generated)?;
            }
            pending_quests.fetch_sub(1, Ordering::SeqCst);
            let added = engine::refill_board(board, queue, max_jobs, None, None);
            storage::save_board(board)?;
            storage::save_job_queue(queue)?;
            tracing::info!(
                queued = queue.entries.len(),
                added,
                story = is_story,
                "quest placement complete"
            );
            Ok(vec![WorkerEffect::BoardRefilled { added }])
        }
        GenerationOutcome::QuestCreationFailed => {
            pending_quests.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![WorkerEffect::QuestCreationFailed])
        }
        GenerationOutcome::GenerateResultFailed { quest_id } => {
            Ok(vec![WorkerEffect::GenerateResultFailed { quest_id }])
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
            let outcome =
                run_generation(job, &generator, &item_generator, &item_registry).await;
            let effects = {
                let mut b = board.lock().await;
                let mut q = job_queue.lock().await;
                match commit_generation(outcome, &mut *b, &mut *q, max_jobs, &pending_quests) {
                    Ok(effects) => effects,
                    Err(e) => {
                        tracing::error!(err = %e, "generation worker commit failed");
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
