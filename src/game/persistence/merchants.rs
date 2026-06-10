use std::path::Path;

use anyhow::Context;

use crate::game::merchant::{
    dumpster_dave_template, MerchantCatalog, MerchantDefinition, MerchantVisit, DUMPSTER_DAVE_INDEX,
    DUMPSTER_DAVE_NAME,
};
use crate::game::persistence::item_registry::ItemRegistry;

const MERCHANTS_PATH: &str = "data/merchants.yaml";
const GUILD_HALL_PATH: &str = "data/guild_hall.yaml";

/// Load the merchant roster from `data/merchants.yaml`, migrating from legacy
/// `guild_hall.yaml` catalog data when present.
pub fn load_merchants() -> anyhow::Result<Option<MerchantCatalog>> {
    if Path::new(MERCHANTS_PATH).exists() {
        let yaml = std::fs::read_to_string(MERCHANTS_PATH).context("reading merchants")?;
        let catalog: MerchantCatalog =
            serde_yaml::from_str(&yaml).context("parsing merchants")?;
        return Ok(Some(catalog));
    }

    if let Some(catalog) = try_migrate_from_guild_hall()? {
        save_merchants(&catalog)?;
        tracing::info!(path = MERCHANTS_PATH, "migrated merchant catalog from guild_hall.yaml");
        return Ok(Some(catalog));
    }

    Ok(None)
}

fn try_migrate_from_guild_hall() -> anyhow::Result<Option<MerchantCatalog>> {
    if !Path::new(GUILD_HALL_PATH).exists() {
        return Ok(None);
    }
    let yaml = std::fs::read_to_string(GUILD_HALL_PATH).context("reading guild hall for migration")?;
    let root: serde_yaml::Value = serde_yaml::from_str(&yaml).context("parsing guild hall for migration")?;
    let Some(catalog_value) = root.get("merchant").and_then(|m| m.get("catalog")) else {
        return Ok(None);
    };
    let catalog: MerchantCatalog =
        serde_yaml::from_value(catalog_value.clone()).context("parsing legacy merchant catalog")?;
    Ok(Some(catalog))
}

/// Persist the merchant roster to `data/merchants.yaml`.
pub fn save_merchants(catalog: &MerchantCatalog) -> anyhow::Result<()> {
    std::fs::create_dir_all("data").context("creating data directory")?;
    let yaml = serde_yaml::to_string(catalog).context("serializing merchants")?;
    std::fs::write(MERCHANTS_PATH, yaml).context("writing merchants")
}

/// Ensure Dumpster Dave is at index 0, preserving his stock pool from any existing entry.
pub fn normalize_catalog(catalog: MerchantCatalog) -> MerchantCatalog {
    let pool = catalog
        .merchants
        .iter()
        .find(|m| m.name == DUMPSTER_DAVE_NAME)
        .map(|m| m.stock_pool.clone())
        .unwrap_or_default();

    let mut others: Vec<MerchantDefinition> = catalog
        .merchants
        .into_iter()
        .filter(|m| m.name != DUMPSTER_DAVE_NAME)
        .collect();

    let mut dave = dumpster_dave_template();
    dave.stock_pool = pool;
    others.insert(DUMPSTER_DAVE_INDEX, dave);

    MerchantCatalog { merchants: others }
}

/// Merge freshly generated merchants with Dumpster Dave's existing stock pool.
pub fn merge_generated_catalog(
    existing: Option<&MerchantCatalog>,
    generated: MerchantCatalog,
) -> MerchantCatalog {
    let pool = existing
        .and_then(|c| c.merchants.first())
        .filter(|m| m.name == DUMPSTER_DAVE_NAME)
        .map(|m| m.stock_pool.clone())
        .or_else(|| {
            existing.and_then(|c| {
                c.merchants
                    .iter()
                    .find(|m| m.name == DUMPSTER_DAVE_NAME)
                    .map(|m| m.stock_pool.clone())
            })
        })
        .unwrap_or_default();

    let mut dave = dumpster_dave_template();
    dave.stock_pool = pool;

    let mut merchants = vec![dave];
    merchants.extend(
        generated
            .merchants
            .into_iter()
            .filter(|m| m.name != DUMPSTER_DAVE_NAME),
    );

    normalize_catalog(MerchantCatalog { merchants })
}

/// True when every stock-pool ID in the roster exists in the item registry.
pub fn catalog_matches_registry(catalog: &MerchantCatalog, registry: &ItemRegistry) -> bool {
    !catalog.merchants.is_empty()
        && catalog
            .merchants
            .iter()
            .flat_map(|m| &m.stock_pool)
            .all(|id| registry.get(*id).is_some())
}

/// True when every visit stock ID exists in the item registry.
pub fn visit_matches_registry(visit: &MerchantVisit, registry: &ItemRegistry) -> bool {
    visit.stock.iter().all(|id| registry.get(*id).is_some())
}

/// Remove persisted merchant roster (fresh game reset).
pub fn clear_merchants() -> anyhow::Result<()> {
    if Path::new(MERCHANTS_PATH).exists() {
        std::fs::remove_file(MERCHANTS_PATH).context("deleting merchants file")?;
    }
    Ok(())
}
