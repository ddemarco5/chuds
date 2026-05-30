use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Chudmasters {
    #[serde(default)]
    pub ids: Vec<u64>,
}
