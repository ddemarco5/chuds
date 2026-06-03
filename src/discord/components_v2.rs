//! Discord [Components V2](https://docs.discord.com/developers/components/reference) payloads.
//!
//! Serenity 0.12 only ships legacy action-row builders; these types serialize the layout
//! components (Text Display, Separator, Action Row) used by `/gear`.

use serde::Serialize;

pub const FLAG_IS_COMPONENTS_V2: u64 = 1 << 15;
pub const FLAG_EPHEMERAL: u64 = 1 << 6;

const TYPE_ACTION_ROW: u8 = 1;
const TYPE_BUTTON: u8 = 2;
const TYPE_TEXT_DISPLAY: u8 = 10;
const TYPE_SEPARATOR: u8 = 14;

/// Message body for a Components V2 interaction response (no legacy `content` field).
#[derive(Debug, Clone, Serialize)]
pub struct ComponentsV2Message {
    pub flags: u64,
    pub components: Vec<Component>,
}

impl ComponentsV2Message {
    pub fn ephemeral(mut self) -> Self {
        self.flags |= FLAG_EPHEMERAL;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Component {
    Text(TextDisplay),
    ActionRow(ActionRow),
    Separator(Separator),
}

#[derive(Debug, Clone, Serialize)]
pub struct TextDisplay {
    #[serde(rename = "type")]
    kind: u8,
    content: String,
}

impl TextDisplay {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            kind: TYPE_TEXT_DISPLAY,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Separator {
    #[serde(rename = "type")]
    kind: u8,
    divider: bool,
    spacing: u8,
}

impl Separator {
    pub fn section() -> Self {
        Self {
            kind: TYPE_SEPARATOR,
            divider: true,
            spacing: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionRow {
    #[serde(rename = "type")]
    kind: u8,
    components: Vec<Button>,
}

impl ActionRow {
    pub fn one_button(button: Button) -> Self {
        Self {
            kind: TYPE_ACTION_ROW,
            components: vec![button],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Button {
    #[serde(rename = "type")]
    kind: u8,
    custom_id: String,
    label: String,
    style: u8,
}

impl Button {
    pub fn secondary(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            kind: TYPE_BUTTON,
            custom_id: custom_id.into(),
            label: label.into(),
            style: 2,
        }
    }
}

/// Wrapper for `UPDATE_MESSAGE` interaction responses.
#[derive(Debug, Clone, Serialize)]
pub struct InteractionUpdateResponse {
    #[serde(rename = "type")]
    kind: u8,
    data: ComponentsV2Message,
}

impl InteractionUpdateResponse {
    pub fn update(data: ComponentsV2Message) -> Self {
        Self { kind: 7, data }
    }
}

pub fn components_v2_flags() -> u64 {
    FLAG_IS_COMPONENTS_V2 | FLAG_EPHEMERAL
}
