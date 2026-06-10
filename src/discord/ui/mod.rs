pub mod gear;
pub mod merchant;

use poise::serenity_prelude::{ComponentInteraction, Http};

use crate::discord::components_v2::{
    components_v2_flags, Component, ComponentsV2Message, InteractionCreateResponse,
    InteractionUpdateResponse, TextDisplay,
};

pub fn no_chud_message() -> ComponentsV2Message {
    ComponentsV2Message {
        flags: components_v2_flags(),
        components: vec![Component::Text(TextDisplay::new("You don't have a chud."))],
    }
}

pub fn push_status_notice(components: &mut Vec<Component>, notice: Option<&str>) {
    if let Some(notice) = notice {
        components.push(Component::Text(TextDisplay::new(format!("```{notice}```"))));
    }
}

pub async fn edit_ephemeral_message(
    http: &Http,
    interaction_token: &str,
    message: &ComponentsV2Message,
) -> anyhow::Result<()> {
    http.edit_original_interaction_response(interaction_token, message, vec![])
        .await?;
    Ok(())
}

pub async fn respond_ephemeral_create(
    http: &Http,
    interaction: &ComponentInteraction,
    message: &ComponentsV2Message,
) {
    let payload = InteractionCreateResponse::ephemeral(message.clone());
    if let Err(e) = http
        .create_interaction_response(interaction.id, &interaction.token, &payload, vec![])
        .await
    {
        tracing::debug!(err = %e, "ephemeral create response failed");
    }
}

pub async fn respond_ephemeral_update(
    http: &Http,
    interaction: &ComponentInteraction,
    message: &ComponentsV2Message,
) {
    let payload = InteractionUpdateResponse::update(message.clone());
    if let Err(e) = http
        .create_interaction_response(interaction.id, &interaction.token, &payload, vec![])
        .await
    {
        tracing::debug!(err = %e, "ephemeral update response failed (ephemeral may be dismissed)");
    }
}

pub use gear::{
    build_gear_message, build_gear_open_message, derive_gear_mode,
    gear_equipment_locked_notice, handle_gear_button, GearInteractionMode,
};
pub use merchant::{
    handle_merchant_shop_button, handle_shop_buy, handle_shop_select, update_merchant_message,
};
