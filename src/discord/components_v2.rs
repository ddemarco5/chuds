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
const TYPE_STRING_SELECT: u8 = 3;
const TYPE_TEXT_DISPLAY: u8 = 10;
const TYPE_SEPARATOR: u8 = 14;
const TYPE_CONTAINER: u8 = 17;

/// Discord's per-message component budget, nested children included.
pub const MAX_MESSAGE_COMPONENTS: usize = 40;

fn is_false(value: &bool) -> bool {
    !*value
}

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
    #[serde(skip_serializing_if = "is_false")]
    spoiler: bool,
    components: Vec<ContainerChild>,
}

impl Container {
    pub fn new(components: Vec<ContainerChild>) -> Self {
        Self {
            kind: TYPE_CONTAINER,
            accent_color: None,
            spoiler: false,
            components,
        }
    }

    pub fn with_accent(accent_color: u32, components: Vec<ContainerChild>) -> Self {
        Self {
            kind: TYPE_CONTAINER,
            accent_color: Some(accent_color),
            spoiler: false,
            components,
        }
    }

    pub fn spoiled(components: Vec<ContainerChild>) -> Self {
        Self {
            kind: TYPE_CONTAINER,
            accent_color: None,
            spoiler: true,
            components,
        }
    }

    pub fn with_accent_spoiled(accent_color: u32, components: Vec<ContainerChild>) -> Self {
        Self {
            kind: TYPE_CONTAINER,
            accent_color: Some(accent_color),
            spoiler: true,
            components,
        }
    }

    fn tree_size(&self) -> usize {
        1 + self
            .components
            .iter()
            .map(ContainerChild::tree_size)
            .sum::<usize>()
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

impl ContainerChild {
    fn tree_size(&self) -> usize {
        match self {
            Self::Text(_) | Self::Separator(_) => 1,
            Self::ActionRow(row) => 1 + row.components.len(),
        }
    }
}

impl Component {
    pub fn tree_size(&self) -> usize {
        match self {
            Self::Container(container) => container.tree_size(),
            Self::Text(_) | Self::Separator(_) => 1,
            Self::ActionRow(row) => 1 + row.components.len(),
        }
    }
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
#[serde(untagged)]
pub enum ActionRowChild {
    Button(Button),
    StringSelect(StringSelect),
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionRow {
    #[serde(rename = "type")]
    kind: u8,
    components: Vec<ActionRowChild>,
}

impl ActionRow {
    pub fn one_button(button: Button) -> Self {
        Self::buttons(vec![button])
    }

    pub fn buttons(buttons: Vec<Button>) -> Self {
        Self {
            kind: TYPE_ACTION_ROW,
            components: buttons.into_iter().map(ActionRowChild::Button).collect(),
        }
    }

    pub fn string_select(select: StringSelect) -> Self {
        Self {
            kind: TYPE_ACTION_ROW,
            components: vec![ActionRowChild::StringSelect(select)],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StringSelect {
    #[serde(rename = "type")]
    kind: u8,
    custom_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    placeholder: Option<String>,
    options: Vec<SelectOption>,
}

impl StringSelect {
    pub fn new(
        custom_id: impl Into<String>,
        placeholder: impl Into<String>,
        options: Vec<SelectOption>,
    ) -> Self {
        Self {
            kind: TYPE_STRING_SELECT,
            custom_id: custom_id.into(),
            placeholder: Some(placeholder.into()),
            options,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectOption {
    label: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<bool>,
}

impl SelectOption {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            default: None,
        }
    }

    pub fn with_default(mut self, is_default: bool) -> Self {
        self.default = Some(is_default);
        self
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
const STYLE_SUCCESS: u8 = 3;
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

/// Disable a button in a raw Components V2 tree and restyle it as a completed click.
pub fn mark_button_clicked(components: &mut serde_json::Value, custom_id: &str, label: &str) -> bool {
    let Some(arr) = components.as_array_mut() else {
        return false;
    };
    let mut found = false;
    for component in arr {
        found |= mark_button_clicked_node(component, custom_id, label);
    }
    found
}

fn mark_button_clicked_node(value: &mut serde_json::Value, custom_id: &str, label: &str) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let mut found = obj.get("type").and_then(|t| t.as_u64()) == Some(u64::from(TYPE_BUTTON))
        && obj.get("custom_id").and_then(|t| t.as_str()) == Some(custom_id);
    if found {
        obj.insert("disabled".to_string(), serde_json::Value::Bool(true));
        obj.insert("style".to_string(), serde_json::json!(STYLE_SUCCESS));
        obj.insert("label".to_string(), serde_json::Value::String(label.to_string()));
    }
    if let Some(children) = obj.get_mut("components") {
        found |= mark_button_clicked(children, custom_id, label);
    }
    found
}

/// Wrapper for `CHANNEL_MESSAGE_WITH_SOURCE` interaction responses (new ephemeral message).
#[derive(Debug, Clone, Serialize)]
pub struct InteractionCreateResponse {
    #[serde(rename = "type")]
    kind: u8,
    data: ComponentsV2Message,
}

impl InteractionCreateResponse {
    pub fn ephemeral(data: ComponentsV2Message) -> Self {
        Self { kind: 4, data }
    }
}

/// Wrapper for `UPDATE_MESSAGE` interaction responses (edit the triggering message).
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

    pub fn ephemeral(components: Vec<Component>) -> Self {
        Self {
            flags: components_v2_flags(),
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

    /// Pack components into one or more messages that stay under Discord's nested-component budget.
    pub fn channel_packed(components: Vec<Component>) -> Vec<Self> {
        let mut messages = Vec::new();
        let mut current: Vec<Component> = Vec::new();
        let mut current_size = 0;
        for component in components {
            let size = component.tree_size();
            if !current.is_empty() && current_size + size > MAX_MESSAGE_COMPONENTS {
                messages.push(Self::channel(std::mem::take(&mut current)));
                current_size = 0;
            }
            current_size += size;
            current.push(component);
        }
        if !current.is_empty() {
            messages.push(Self::channel(current));
        }
        messages
    }

    /// Stable fingerprint for message-cache skip logic (full payload, not just text).
    pub fn cache_key(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}
