use std::collections::HashMap;
use std::fmt::Display;
use std::sync::LazyLock;

use rand::seq::SliceRandom;

type Messages = HashMap<String, Vec<String>>;

static MESSAGES: LazyLock<Messages> = LazyLock::new(|| {
    serde_yaml::from_str(include_str!("messages.yaml")).unwrap()
});

pub fn get(key: &str, args: &[&dyn Display]) -> String {
    let templates = MESSAGES
        .get(key)
        .unwrap_or_else(|| panic!("unknown message key: {key}"));
    let mut template = templates
        .choose(&mut rand::thread_rng())
        .unwrap()
        .clone();
    for arg in args {
        if let Some(pos) = template.find("{}") {
            template.replace_range(pos..pos + 2, &arg.to_string());
        }
    }
    template
}

#[macro_export]
macro_rules! chud_msg {
    ($key:expr $(, $arg:expr)*) => {
        $crate::messages::get($key, &[$( &($arg) as &dyn std::fmt::Display ),*])
    };
}
