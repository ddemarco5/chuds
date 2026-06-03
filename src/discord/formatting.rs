use crate::chud_msg;
use crate::discord::components_v2::{
    Component, ComponentsV2Message, Container, ContainerChild, TextDisplay,
};
use crate::game::domain::item::{Item, ItemType};
use crate::game::domain::player::Player;
use crate::game::domain::quest_result::QuestResult;

/// Collapse whitespace so Discord `*italic*` markers stay on one line.
fn markdown_italic_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Components V2 DM for a finished job, plus a plain-text copy for length-limit fallback.
pub fn build_dm_completion_report(
    player_name: &str,
    result: &QuestResult,
    player: &Player,
    level_up: &crate::game::domain::player::LevelUp,
    reward: u32,
    item_awarded: Option<&Item>,
    item_auto_sold_gold: Option<u32>,
    hospitalized: bool,
) -> (ComponentsV2Message, String) {
    let preamble = format_completion_preamble(player_name, result, player);
    let outcome = if result.passed { "PASSED" } else { "FAILED" };
    let footer = format_completion_footer(
        player_name,
        player,
        level_up,
        result.passed,
        reward,
        item_awarded,
        item_auto_sold_gold,
    );
    let hospital_msg = hospitalized.then(|| chud_msg!("dm_hospitalized", player_name));
    let outcome_box = format_completion_outcome_box(
        outcome,
        &result.summary,
        &footer,
        hospital_msg.as_deref(),
    );
    let plain = format!("{preamble}\n\n{outcome_box}");

    let message = ComponentsV2Message::channel(vec![
        Component::Text(TextDisplay::new(preamble)),
        Component::Container(Container::new(vec![ContainerChild::Text(TextDisplay::new(
            outcome_box,
        ))])),
    ]);
    (message, plain)
}

/// Header, summary, rewards, and optional hospital note — one multiline block inside the container.
fn format_completion_outcome_box(
    outcome: &str,
    summary: &str,
    footer: &str,
    hospital_msg: Option<&str>,
) -> String {
    let mut sections = vec![format!("**{outcome}**"), summary.to_string()];
    if !footer.is_empty() {
        sections.push(footer.to_string());
    }
    if let Some(msg) = hospital_msg {
        sections.push(msg.to_string());
    }
    sections.join("\n\n")
}

fn format_completion_preamble(player_name: &str, result: &QuestResult, player: &Player) -> String {
    let mut out = format!("{}\n\n", player.format_stats());
    out.push_str(&format!(
        "**{}** - {}\n{}\n",
        result.quest_title, result.quest_giver, result.quest_description
    ));
    for (i, trial) in result.trials.iter().enumerate() {
        let pass_str = if trial.passed { "\u{2705}" } else { "\u{274c}" };
        let brain = if trial.chose_optimal { " \u{1F9E0}" } else { "" };
        out.push_str(&format!(
            "\n**Trial {}** - {}\n{} {} | {} rolled {} vs {}{}\n*{}*\n",
            i + 1,
            trial.situation,
            pass_str,
            trial.stat_used.label(),
            player_name,
            trial.format_player_roll(),
            trial.trial_roll,
            brain,
            markdown_italic_line(&trial.narrative),
        ));
    }
    out
}

fn format_completion_footer(
    player_name: &str,
    player: &Player,
    level_up: &crate::game::domain::player::LevelUp,
    passed: bool,
    reward: u32,
    item_awarded: Option<&Item>,
    item_auto_sold_gold: Option<u32>,
) -> String {
    let mut out = String::new();
    if passed && reward > 0 {
        out.push_str(&chud_msg!("chud_brings_money", reward));
    }
    if let Some(item) = item_awarded {
        let subtype = if item.subtype.is_empty() {
            String::new()
        } else {
            format!(", {}", item.subtype)
        };
        if !out.is_empty() {
            out.push('\n');
        }
        if let Some(gold) = item_auto_sold_gold {
            out.push_str(&format!(
                "**Stash full — sold {} for ${gold}:** ({}{subtype})\nStats: {}\n_{}_",
                item.name,
                item_type_label(item.item_type),
                item.stats.format_triplet(),
                item.description,
            ));
        } else {
            out.push_str(&format!(
                "**Item stashed:** {} ({}{subtype})\nStats: {}\n_{}_\nUse /gear to equip.",
                item.name,
                item_type_label(item.item_type),
                item.stats.format_triplet(),
                item.description,
            ));
        }
    }
    if level_up.any() {
        fn fmt_stat(levelled: bool, val: u8) -> String {
            if levelled {
                format!("{} -> **{}**", val - 1, val)
            } else {
                val.to_string()
            }
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "{} has improved! - Strength {}, Smarts {}, Stealth {}, Experience {}",
            player_name,
            fmt_stat(level_up.str_up, player.strength),
            fmt_stat(level_up.smt_up, player.smarts),
            fmt_stat(level_up.sth_up, player.stealth),
            fmt_stat(level_up.exp_up, player.experience),
        ));
    }
    out
}

fn item_type_label(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Gear => "gear",
        ItemType::Weapon => "weapon",
        ItemType::Misc => "misc",
    }
}

fn oxford_join(names: &[&str]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        2 => format!("{} and {}", names[0], names[1]),
        _ => {
            let (last, rest) = names.split_last().unwrap();
            format!("{}, and {}", rest.join(", "), last)
        }
    }
}

pub fn format_dm_scouting_report(
    player_name: &str,
    quest_title: &str,
    quest_days: u32,
    chance: f64,
    active_player_name: Option<&str>,
    scouting_player_names: &[&str],
) -> String {
    let feeling = if chance == 0.0 {
        "don't want to talk about"
    } else if chance <= 0.25 {
        "are scared of"
    } else if chance <= 0.50 {
        "feel apprehensive about"
    } else if chance <= 0.75 {
        "think they can do"
    } else {
        "say they'll fuckin demolish"
    };

    let mut out = format!(
        "**{}** checked \"{}\", they {} it.",
        player_name, quest_title, feeling
    );

    if let Some(name) = active_player_name {
        out.push_str(&format!("\nOh, and they also saw {} on the job there.", name));
    }

    if !scouting_player_names.is_empty() {
        let names_str = oxford_join(scouting_player_names);
        out.push_str(&format!("\nThey also spotted {} checking it out.", names_str));
    }

    if quest_days > 0 {
        out.push_str(&format!("\n{}", chud_msg!("scout_days", quest_days)));
    }

    out
}
