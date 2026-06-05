use std::sync::LazyLock;

use serde::Deserialize;

use crate::game::domain::item::Item;

#[derive(Deserialize)]
struct StartingItemsFile {
    items: Vec<Item>,
}

static CATALOG: LazyLock<Vec<Item>> = LazyLock::new(|| {
    serde_yaml::from_str::<StartingItemsFile>(include_str!("starting_items.yaml"))
        .expect("invalid starting_items.yaml")
        .items
});

pub fn catalog() -> &'static [Item] {
    &CATALOG
}
