use std::time::Instant;

use rig::client::AgentClientExt;
use rig::completion::{CompletionError, Prompt, PromptError};
use rig::completion::message::Message;
use rig::http_client::{HeaderMap, HeaderValue};
use rig::memory::ConversationMemory;
use rig::providers::openrouter;

use crate::game::generation::memory::GameConversationMemory;

pub const WORLD_BUILDING_CONTEXT: &str = "You are operating in a fantasy world that is lighthearted, full of satire, and often crude. Adventurers are known as 'Chuds' and are often exceptionally bizarre";

/// Per-attempt timeout for LLM requests. Raise if using slow free-tier models.
pub const PROMPT_TIMEOUT_SECS: u64 = 90;

pub type OpenRouterAgent = rig::agent::Agent;

pub fn build_client(api_key: &str) -> anyhow::Result<openrouter::Client> {
    let mut headers = HeaderMap::new();
    headers.insert("x-openrouter-title", HeaderValue::from_static("Chuds"));
    Ok(openrouter::Client::builder()
        .http_headers(headers)
        .api_key(api_key)
        .build()?)
}

pub fn build_agent(client: &openrouter::Client, system_context: &str) -> OpenRouterAgent {
    client
        .agent("openrouter/free")
        .preamble([WORLD_BUILDING_CONTEXT, system_context].join("\n\n").as_str())
        .build()
}

/// Collapse runs of whitespace (newlines, tabs, spaces) into single spaces.
pub fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn normalize_yaml_string_values(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::String(s) => *s = collapse_whitespace(s),
        serde_yaml::Value::Sequence(seq) => {
            for v in seq {
                normalize_yaml_string_values(v);
            }
        }
        serde_yaml::Value::Mapping(map) => {
            for v in map.values_mut() {
                normalize_yaml_string_values(v);
            }
        }
        _ => {}
    }
}

pub fn sanitize(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix("```yaml")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let stripped = if inner != s {
        if let Some(end) = inner.rfind("```") {
            inner[..end].trim()
        } else {
            inner.trim()
        }
    } else {
        s
    };
    stripped
        .replace('\u{2019}', "'")
        .replace('\u{2011}', "-")
}

pub fn truncate_for_log(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn yaml_correction(kind: &str, error: &str, raw_preview: &str) -> String {
    format!(
        "Your previous response was invalid ({kind}): {error}\n\
         Reply again with ONLY valid YAML matching the required fields. \
         Use block scalars (|) or double-quoted strings for long text or text containing colons.\n\
         Previous response (truncated):\n{raw_preview}"
    )
}

const YAML_PARSE_RETRIES: u32 = 5;

fn yaml_retry(
    retries: &mut u32,
    raw: &str,
    kind: &str,
    error: &str,
    conversation_id: &str,
) -> Option<String> {
    if *retries >= YAML_PARSE_RETRIES {
        return None;
    }
    *retries += 1;
    let preview = truncate_for_log(raw, 500);
    tracing::warn!(
        kind,
        error,
        conversation_id,
        retries = *retries,
        YAML_PARSE_RETRIES,
        "retrying LLM YAML"
    );
    tracing::debug!(raw_preview = %preview, "LLM response that failed YAML parse");
    Some(yaml_correction(kind, error, &preview))
}

fn parse_retry_delay(msg: &str) -> Option<u64> {
    let json_str = &msg[msg.find("with message: ")? + "with message: ".len()..];
    let body: serde_json::Value = serde_json::from_str(json_str).ok()?;
    if let Some(details) = body["error"]["details"].as_array() {
        for detail in details {
            if let Some(delay) = detail["retryDelay"].as_str() {
                return delay
                    .trim_end_matches('s')
                    .parse::<f64>()
                    .ok()
                    .map(|s| s.ceil() as u64);
            }
        }
    }
    if let Some(secs) = body["error"]["metadata"]["retry_after_seconds"].as_u64() {
        return Some(secs);
    }
    None
}

pub async fn prompt_with_retry(
    agent: &OpenRouterAgent,
    prompt: &str,
    history: &[Message],
    conversation_id: &str,
) -> anyhow::Result<String> {
    const MAX_RETRIES: u32 = 12;
    let mut retries = 0;
    loop {
        let started = Instant::now();
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(PROMPT_TIMEOUT_SECS),
            agent
                .prompt(prompt)
                .without_memory()
                .history(history.iter().cloned()),
        )
        .await
        {
            Err(_elapsed) => {
                let elapsed_ms = started.elapsed().as_millis();
                if retries < MAX_RETRIES {
                    retries += 1;
                    tracing::warn!(
                        conversation_id,
                        elapsed_ms,
                        PROMPT_TIMEOUT_SECS,
                        retries,
                        MAX_RETRIES,
                        "LLM request timed out, retrying"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "LLM request timed out after {} retries ({conversation_id})",
                    MAX_RETRIES
                ));
            }
            Ok(Ok(response)) => {
                tracing::debug!(
                    conversation_id,
                    elapsed_ms = started.elapsed().as_millis(),
                    retries,
                    "LLM request complete"
                );
                return Ok(sanitize(&response));
            }
            Ok(Err(e)) => {
                let elapsed_ms = started.elapsed().as_millis();
                let msg = e.to_string();
                let (wait_secs, label) = if msg.contains("404") {
                    (10, "404 model not found")
                } else if msg.contains("503") {
                    (10, "503 model overloaded")
                } else if msg.contains("500") {
                    (15, "500 internal server error")
                } else if msg.contains("429") {
                    (parse_retry_delay(&msg).unwrap_or(5) * 2, "429 quota exceeded")
                } else if matches!(
                    e,
                    PromptError::CompletionError(CompletionError::ResponseError(_))
                ) {
                    (5, "malformed OpenRouter response")
                } else {
                    return Err(e.into());
                };
                if retries >= MAX_RETRIES {
                    return Err(e.into());
                }
                tracing::debug!(conversation_id, elapsed_ms, err = %msg, "retryable LLM error body");
                retries += 1;
                tracing::warn!(
                    conversation_id,
                    elapsed_ms,
                    error = label,
                    wait_secs,
                    retries,
                    MAX_RETRIES,
                    "retryable LLM error, waiting before retry"
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(wait_secs)).await;
            }
        }
    }
}

pub async fn prompt_parse_retry<T: serde::de::DeserializeOwned>(
    agent: &OpenRouterAgent,
    memory: &GameConversationMemory,
    prompt: &str,
    expected_list: Option<(&str, usize)>,
    conversation_id: &str,
) -> anyhow::Result<T> {
    let mut retries = 0u32;
    let mut correction = String::new();
    loop {
        let history = memory.load(conversation_id).await.unwrap_or_default();
        let effective_prompt = if correction.is_empty() {
            prompt.to_string()
        } else {
            format!("{prompt}\n\n{correction}")
        };
        let raw = prompt_with_retry(agent, &effective_prompt, &history, conversation_id).await?;
        let parsed = serde_yaml::from_str::<serde_yaml::Value>(&raw);
        match parsed {
            Err(e) => {
                correction = yaml_retry(
                    &mut retries,
                    &raw,
                    "malformed YAML",
                    &e.to_string(),
                    conversation_id,
                )
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "LLM returned malformed YAML after {YAML_PARSE_RETRIES} retries: {e}"
                    )
                })?;
                continue;
            }
            Ok(mut value) => {
                normalize_yaml_string_values(&mut value);
                if let Some((list_key, expected)) = expected_list {
                    let actual = value
                        .get(list_key)
                        .and_then(|t| t.as_sequence())
                        .map(|s| s.len());
                    if actual != Some(expected) {
                        let (kind, msg, fail) = if list_key == "trials" {
                            (
                                "wrong trial count",
                                format!("expected {expected} trial strings, got {actual:?}"),
                                format!(
                                    "LLM returned wrong trial count after {YAML_PARSE_RETRIES} retries: expected {expected}, got {actual:?}"
                                ),
                            )
                        } else {
                            (
                                "wrong list count",
                                format!("expected {expected} {list_key} entries, got {actual:?}"),
                                format!(
                                    "LLM returned wrong {list_key} count after {YAML_PARSE_RETRIES} retries: expected {expected}, got {actual:?}"
                                ),
                            )
                        };
                        correction = yaml_retry(
                            &mut retries,
                            &raw,
                            kind,
                            &msg,
                            conversation_id,
                        )
                        .ok_or_else(|| anyhow::anyhow!(fail))?;
                        continue;
                    }
                }
                match serde_yaml::from_value(value) {
                    Ok(result) => {
                        let _ = memory
                            .append(
                                conversation_id,
                                vec![Message::user(prompt), Message::assistant(&raw)],
                            )
                            .await;
                        tracing::debug!(conversation_id, "committed to memory");
                        return Ok(result);
                    }
                    Err(e) => {
                        correction = yaml_retry(
                            &mut retries,
                            &raw,
                            "unexpected YAML structure",
                            &e.to_string(),
                            conversation_id,
                        )
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "LLM returned unexpected YAML structure after {YAML_PARSE_RETRIES} retries: {e}"
                            )
                        })?;
                        continue;
                    }
                }
            }
        }
    }
}
