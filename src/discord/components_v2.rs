//! Discord [Components V2](https://docs.discord.com/developers/components/reference) payloads.
//!
//! Serenity 0.12 only ships legacy action-row builders; these types serialize layout and
//! content components until serenity-next is stable. Text Display (`type` 10) matches
//! [the reference](https://docs.discord.com/developers/components/reference#text-display):
//! markdown `content`, no legacy message `content` field, and flag `1 << 15` on the message.
//! For a visible bordered group, wrap content in a [Container](https://docs.discord.com/developers/components/reference#container) (type 17).

use serde::Serialize;

pub const FLAG_IS_COMPONENTS_V2: u64 = 1 << 15;
pub const FLAG_EPHEMERAL: u64 = 1 << 6;

const TYPE_ACTION_ROW: u8 = 1;
const TYPE_BUTTON: u8 = 2;
const TYPE_TEXT_DISPLAY: u8 = 10;
const TYPE_SEPARATOR: u8 = 14;
const TYPE_CONTAINER: u8 = 17;

/// Message body for a Components V2 interaction response (no legacy `content` field).
#[derive(Debug, Clone, Serialize)]
pub struct ComponentsV2Message {
    pub flags: u64,
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Component {
    Container(Container),
    Text(TextDisplay),
    ActionRow(ActionRow),
    Separator(Separator),
}

/// [Container](https://docs.discord.com/developers/components/reference#container) (type 17) — bordered box with optional accent bar.
#[derive(Debug, Clone, Serialize)]
pub struct Container {
    #[serde(rename = "type")]
    kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    accent_color: Option<u32>,
    components: Vec<ContainerChild>,
}

impl Container {
    pub fn new(components: Vec<ContainerChild>) -> Self {
        Self {
            kind: TYPE_CONTAINER,
            accent_color: None,
            components,
        }
    }

    pub fn with_accent(accent_color: u32, components: Vec<ContainerChild>) -> Self {
        Self {
            kind: TYPE_CONTAINER,
            accent_color: Some(accent_color),
            components,
        }
    }
}

/// Components allowed inside a [Container](https://docs.discord.com/developers/components/reference#container-container-child-components).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ContainerChild {
    Text(TextDisplay),
    ActionRow(ActionRow),
    Separator(Separator),
}

/// [Text Display](https://docs.discord.com/developers/components/reference#text-display) (type 10).
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
        Self::buttons(vec![button])
    }

    pub fn buttons(buttons: Vec<Button>) -> Self {
        Self {
            kind: TYPE_ACTION_ROW,
            components: buttons,
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

const STYLE_PRIMARY: u8 = 1;
const STYLE_SECONDARY: u8 = 2;
const STYLE_DANGER: u8 = 4;

impl Button {
    pub fn primary(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            kind: TYPE_BUTTON,
            custom_id: custom_id.into(),
            label: label.into(),
            style: STYLE_PRIMARY,
        }
    }

    pub fn secondary(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            kind: TYPE_BUTTON,
            custom_id: custom_id.into(),
            label: label.into(),
            style: STYLE_SECONDARY,
        }
    }

    pub fn danger(custom_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            kind: TYPE_BUTTON,
            custom_id: custom_id.into(),
            label: label.into(),
            style: STYLE_DANGER,
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

/// Ephemeral interaction / follow-up (gear UI).
pub fn components_v2_flags() -> u64 {
    FLAG_IS_COMPONENTS_V2 | FLAG_EPHEMERAL
}

/// Persistent channel messages (job board); no ephemeral flag.
pub fn channel_message_flags() -> u64 {
    FLAG_IS_COMPONENTS_V2
}

impl ComponentsV2Message {
    pub fn channel(components: Vec<Component>) -> Self {
        Self {
            flags: channel_message_flags(),
            components,
        }
    }

    /// Single top-level container (the visible “box” in the client).
    pub fn channel_box(inner: Vec<ContainerChild>, accent_color: Option<u32>) -> Self {
        let container = match accent_color {
            Some(color) => Container::with_accent(color, inner),
            None => Container::new(inner),
        };
        Self::channel(vec![Component::Container(container)])
    }

    /// Stable fingerprint for message-cache skip logic (full payload, not just text).
    pub fn cache_key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}
