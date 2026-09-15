use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rig::completion::message::Message;
use rig::memory::{ConversationMemory, MemoryError, MessageFilter};

/// Token budget for each agent's conversation history (approximate; 1 token ≈ 4 chars).
pub const MEMORY_TOKEN_BUDGET: usize = 50_000;

/// Identifies which agent conversation history to reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmMemorySlot {
    Description,
    Trials,
    StoryDescription,
    StoryTrials,
    Results,
    Item,
    Gravestone,
    Merchant,
    All,
}

fn char_budget_filter(msgs: Vec<Message>) -> Vec<Message> {
    let mut out = msgs;
    let char_budget = MEMORY_TOKEN_BUDGET * 4;
    let mut total: usize = out
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
        .sum();
    tracing::debug!(
        history_msgs = out.len(),
        history_chars = total,
        budget_chars = char_budget,
        "memory loaded"
    );
    let msgs_before = out.len();
    while total > char_budget && out.len() > 1 {
        let removed_size = serde_json::to_string(&out[0])
            .map(|s| s.len())
            .unwrap_or(0);
        out.remove(0);
        total = total.saturating_sub(removed_size);
    }
    if out.len() < msgs_before {
        tracing::warn!(
            trimmed = msgs_before - out.len(),
            history_msgs = out.len(),
            history_chars = total,
            budget_chars = char_budget,
            "memory trimmed"
        );
    }
    out
}

/// In-process conversation memory with char-budget trimming on load.
#[derive(Clone)]
pub struct GameConversationMemory {
    inner: Arc<Mutex<HashMap<String, Vec<Message>>>>,
    filter: Option<Arc<dyn MessageFilter>>,
}

impl GameConversationMemory {
    fn with_char_budget_filter(mut self) -> Self {
        self.filter = Some(Arc::new(char_budget_filter));
        self
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, Vec<Message>>>, MemoryError> {
        self.inner
            .lock()
            .map_err(|e| MemoryError::Internal(e.to_string()))
    }

    fn apply_filter(&self, messages: Vec<Message>) -> Vec<Message> {
        match &self.filter {
            Some(filter) => filter(messages),
            None => messages,
        }
    }

    /// Empties all stored conversations in this memory instance.
    pub fn clear_all(&self) {
        match self.lock() {
            Ok(mut guard) => guard.clear(),
            Err(e) => tracing::warn!(err = %e, "failed to lock memory for clear_all"),
        }
    }

    /// Snapshot each conversation after applying the char-budget filter.
    pub fn export_filtered_store(&self) -> HashMap<String, Vec<Message>> {
        let guard = match self.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(err = %e, "failed to lock memory for export");
                return HashMap::new();
            }
        };
        guard
            .iter()
            .map(|(id, msgs)| (id.clone(), self.apply_filter(msgs.clone())))
            .collect()
    }
}

pub fn make_memory() -> GameConversationMemory {
    GameConversationMemory {
        inner: Arc::new(Mutex::new(HashMap::new())),
        filter: None,
    }
    .with_char_budget_filter()
}

pub fn make_memory_from_store(store: HashMap<String, Vec<Message>>) -> GameConversationMemory {
    GameConversationMemory {
        inner: Arc::new(Mutex::new(store)),
        filter: None,
    }
    .with_char_budget_filter()
}

impl ConversationMemory for GameConversationMemory {
    fn load<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            let messages = {
                let guard = self.lock()?;
                guard.get(conversation_id).cloned().unwrap_or_default()
            };
            Ok(self.apply_filter(messages))
        })
    }

    fn append<'a>(
        &'a self,
        conversation_id: &'a str,
        messages: Vec<Message>,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            let mut guard = self.lock()?;
            guard
                .entry(conversation_id.to_string())
                .or_default()
                .extend(messages);
            Ok(())
        })
    }

    fn clear<'a>(
        &'a self,
        conversation_id: &'a str,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            let mut guard = self.lock()?;
            guard.remove(conversation_id);
            Ok(())
        })
    }
}
