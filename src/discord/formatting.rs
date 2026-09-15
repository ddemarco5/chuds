use crate::chud_msg;
use crate::discord::components_v2::{
    components_v2_flags, Component, ComponentsV2Message, Container, ContainerChild, Separator,
    TextDisplay,
};
use crate::game::domain::graveyard::GraveyardEntry;
use crate::game::domain::item::{Item, ItemType};
use crate::game::domain::player::Player;
use crate::game::domain::quest_result::{CompletedTrial, QuestResult};
use crate::game::domain::stash::STASH_CAPACITY;
use crate::game::engine::KillResult;
use crate::game::mechanics::simulation::effective_stats;
use crate::game::generation::generators::collapse_whitespace;
use crate::game::persistence::item_registry::ItemRegistry;

#[derive(Debug, Clone, Copy)]
pub struct PlayerStatsBlockOptions {
    pub include_stash: bool,
    pub include_cash: bool,
}

pub fn format_equipment_summary(player: &Player, registry: &ItemRegistry) -> String {
    let chud = player.chud_ref();
    let mut lines = Vec::new();
    {
        let mut push_slot = |label: String, id: Option<u32>| {
            if let Some(item) = id.and_then(|id| registry.get(id)) {
                lines.push(format!(
                    "{label}: **{}** ({}) [{}]",
                    item.name,
                    format_item_slot_label(item),
                    item.stats.format_triplet(),
                ));
            }
        };
        push_slot("Gear".into(), chud.equipment.gear);
        push_slot("Weapon".into(), chud.equipment.weapon);
        for (i, slot) in chud.equipment.misc.iter().enumerate() {
            push_slot(format!("Misc {}", i + 1), *slot);
        }
    }
    lines.join("\n")
}

pub fn format_player_stats_block(
    player: &Player,
    registry: &ItemRegistry,
    options: PlayerStatsBlockOptions,
) -> String {
    let chud = player.chud_ref();
    let equipment = format_equipment_summary(player, registry);
    let effective = effective_stats(player, registry);
    let stats_line = player.format_effective_stats_line(effective);
    let job_record = player.format_job_record();

    let mut out = format!("**{}**\n-# {}", chud.name, chud.description);

    if !equipment.is_empty() {
        out.push_str("\n\n");
        out.push_str(&equipment);
    }

    out.push_str("\n\n");

    if options.include_stash {
        let stash_line = if player.stash.is_empty() {
            format!("Stash: empty (0/{STASH_CAPACITY}) - use `/gear` to manage loadout")
        } else {
            format!(
                "Stash: {}/{} items - use `/gear` to manage loadout",
                player.stash.len(),
                STASH_CAPACITY
            )
        };
        out.push_str(&stash_line);
        out.push('\n');
    }

    out.push_str(&stats_line);
    out.push('\n');
    out.push_str(&job_record);

    if options.include_cash {
        out.push_str(&format!(
            "\n\nYou've got ${} worth of loose change.",
            player.cash
        ));
    }

    out
}

/// Collapse whitespace so Discord `*italic*` markers stay on one line.
fn markdown_italic_line(text: &str) -> String {
    collapse_whitespace(text)
}

/// Prefix each line with Discord subtext (`-#`).
pub fn subtext_lines(text: &str) -> String {
    text.lines()
        .map(|line| format!("-# {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Accent colors for quest outcome containers (RGB integers).
const ACCENT_PASSED: u32 = 0x57F287;
const ACCENT_FAILED: u32 = 0xED4245;
const ACCENT_DEATH: u32 = 0x2F3136;

/// Preamble, outcome, and ordered text segments for long DM delivery.
pub struct DmCompletionContent {
    pub preamble: String,
    pub summary: String,
    passed: bool,
    died: bool,
    segments: Vec<String>,
}

impl DmCompletionContent {
    pub fn plain(&self) -> String {
        format!("{}\n\n{}", self.preamble, self.summary)
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }
}

pub fn build_dm_completion_content(
    player_name: &str,
    result: &QuestResult,
    player: &Player,
    registry: &ItemRegistry,
    level_up: &crate::game::domain::player::LevelUp,
    reward: u32,
    item_awarded: Option<&Item>,
    item_award_disposition: Option<crate::game::engine::ItemAwardDisposition>,
    hospitalized: bool,
    kill: Option<&KillResult>,
) -> DmCompletionContent {
    let segments = build_dm_preamble_segments(player_name, result, player, registry);
    let preamble = segments.join("");
    let (summary, passed, died) = if let Some(kill) = kill {
        let footer = format_death_footer(kill);
        let summary = format_completion_outcome_box("DIED", &result.summary, &footer, None);
        (summary, false, true)
    } else {
        let outcome = if result.passed { "PASSED" } else { "FAILED" };
        let footer = format_completion_footer(
            player_name,
            player,
            level_up,
            result.passed,
            reward,
            item_awarded,
            item_award_disposition,
        );
        let hospital_msg = hospitalized.then(|| chud_msg!("dm_hospitalized", player_name));
        let summary = format_completion_outcome_box(
            outcome,
            &result.summary,
            &footer,
            hospital_msg.as_deref(),
        );
        (summary, result.passed, false)
    };
    DmCompletionContent {
        preamble,
        summary,
        passed,
        died,
        segments,
    }
}

fn summary_container(summary: &str, passed: bool, died: bool) -> Component {
    let accent = if died {
        ACCENT_DEATH
    } else if passed {
        ACCENT_PASSED
    } else {
        ACCENT_FAILED
    };
    Component::Container(Container::with_accent(
        accent,
        vec![ContainerChild::Text(TextDisplay::new(summary.to_string()))],
    ))
}

pub fn build_dm_completion_components(content: &DmCompletionContent) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![
        Component::Text(TextDisplay::new(content.preamble.clone())),
        summary_container(&content.summary, content.passed, content.died),
    ])
}

/// Outcome block only — same container as the short DM path.
pub fn build_dm_summary_components(content: &DmCompletionContent) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![summary_container(
        &content.summary,
        content.passed,
        content.died,
    )])
}

/// Single-notice DM — same accent container as the job pass/fail summary.
pub fn build_dm_notice_components(message: &str, passed: bool) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![summary_container(message, passed, false)])
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

fn format_dm_header(player: &Player, registry: &ItemRegistry) -> String {
    let effective = effective_stats(player, registry);
    format!(
        "{} -- *{}*\n\n",
        player.chud_ref().name,
        player.format_effective_stats(effective),
    )
}

fn format_dm_quest(result: &QuestResult) -> String {
    format!(
        "**{}** - _{}_\n{}\n",
        result.quest_title, result.quest_giver, result.quest_description
    )
}

fn format_dm_trial(player_name: &str, trial: &CompletedTrial, index: usize) -> String {
    let pass_str = if trial.passed { "\u{2705}" } else { "\u{274c}" };
    let brain = if trial.chose_optimal { " \u{1F9E0}" } else { "" };
    format!(
        "\n**Trial {}** - {}\n{} {} | {} rolled {} vs {}{}\n*{}*\n",
        index + 1,
        trial.situation,
        pass_str,
        trial.stat_used.label(),
        player_name,
        trial.format_player_roll(),
        trial.trial_roll,
        brain,
        markdown_italic_line(&trial.narrative),
    )
}

fn build_dm_preamble_segments(
    player_name: &str,
    result: &QuestResult,
    player: &Player,
    registry: &ItemRegistry,
) -> Vec<String> {
    let mut segments = vec![format_dm_header(player, registry), format_dm_quest(result)];
    for (i, trial) in result.trials.iter().enumerate() {
        segments.push(format_dm_trial(player_name, trial, i));
    }
    segments
}

fn format_completion_footer(
    player_name: &str,
    player: &Player,
    level_up: &crate::game::domain::player::LevelUp,
    passed: bool,
    reward: u32,
    item_awarded: Option<&Item>,
    item_award_disposition: Option<crate::game::engine::ItemAwardDisposition>,
) -> String {
    let mut out = String::new();
    if passed && reward > 0 {
        out.push_str(&chud_msg!("chud_brings_money", reward));
    }
    if let Some(item) = item_awarded {
        let slot = format_item_slot_label(item);
        if !out.is_empty() {
            out.push('\n');
        }
        match item_award_disposition {
            Some(crate::game::engine::ItemAwardDisposition::Sold(gold)) => {
                out.push_str(&format!(
                    "**Stash full — sold {} for ${gold}:** ({slot})\nStats: {}\n_{}_",
                    item.name,
                    item.stats.format_triplet(),
                    item.description,
                ));
            }
            Some(crate::game::engine::ItemAwardDisposition::Equipped) => {
                out.push_str(&format!(
                    "**Stash full — auto-equipped:** {} ({slot})\nStats: {}\n_{}_",
                    item.name,
                    item.stats.format_triplet(),
                    item.description,
                ));
            }
            _ => {
                out.push_str(&format!(
                    "**Item stashed:** {} ({slot})\nStats: {}\n_{}_\nUse `/gear` to equip.",
                    item.name,
                    item.stats.format_triplet(),
                    item.description,
                ));
            }
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
            fmt_stat(level_up.str_up, player.chud_ref().strength),
            fmt_stat(level_up.smt_up, player.chud_ref().smarts),
            fmt_stat(level_up.sth_up, player.chud_ref().stealth),
            fmt_stat(level_up.exp_up, player.chud_ref().experience),
        ));
    }
    out
}

fn format_death_footer(kill: &KillResult) -> String {
    let mut out = format!("*{}*", kill.epitaph);
    if kill.benefits_awarded > 0 {
        out.push('\n');
        out.push_str(&chud_msg!("dm_starting_benefits", kill.benefits_awarded));
    }
    out
}

pub struct DmDeathContent {
    pub body: String,
}

impl DmDeathContent {
    pub fn plain(&self) -> String {
        self.body.clone()
    }
}

pub fn build_dm_death_content(kill: &KillResult, summary: Option<&str>) -> DmDeathContent {
    let mut sections = vec![chud_msg!("dm_died", kill.chud_name)];
    if let Some(summary) = summary.filter(|s| !s.is_empty()) {
        sections.push(summary.to_string());
    }
    sections.push(format!("*{}*", kill.epitaph));
    if kill.benefits_awarded > 0 {
        sections.push(chud_msg!("dm_starting_benefits", kill.benefits_awarded));
    }
    DmDeathContent {
        body: sections.join("\n\n"),
    }
}

pub fn build_dm_death_components(content: &DmDeathContent) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![Component::Container(Container::with_accent(
        ACCENT_DEATH,
        vec![ContainerChild::Text(TextDisplay::new(content.body.clone()))],
    ))])
}

fn format_gravestone_name(name: &str) -> String {
    // CV2 Text Display has no center alignment; `#` heading gives the name prominence.
    format!("# **{}**", name.to_uppercase())
}

fn format_gravestone_epitaph(epitaph: &str) -> String {
    epitaph.to_uppercase()
}

pub fn build_graveyard_components(entries: &[GraveyardEntry]) -> ComponentsV2Message {
    let mut inner: Vec<ContainerChild> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            inner.push(ContainerChild::Separator(Separator::section()));
        }
        inner.push(ContainerChild::Text(TextDisplay::new(format_gravestone_name(
            &entry.chud.name,
        ))));
        inner.push(ContainerChild::Text(TextDisplay::new(format_gravestone_epitaph(
            &entry.epitaph,
        ))));
    }
    let mut message = ComponentsV2Message::channel_box(inner, None);
    message.flags = components_v2_flags();
    message
}

fn rarity_prefix(rarity: &str) -> &'static str {
    match rarity {
        "uncommon" => "\u{25C7} ",
        "rare" => "\u{25C6} ",
        "exceptional" => "\u{2605} ",
        _ => "",
    }
}

pub fn format_item_display_name(item: &Item) -> String {
    format!("{}{}", rarity_prefix(&item.rarity), item.name)
}

/// Player-facing slot label — not the LLM body-part subtype.
pub fn format_item_slot_label(item: &Item) -> String {
    match item.item_type {
        ItemType::Misc if !item.subtype.is_empty() => item.subtype.clone(),
        _ => item_type_label(item.item_type).to_string(),
    }
}

/// Player-facing slot label in parentheses, e.g. `(gear)` or `(trinket)`.
pub fn format_item_slot_parenthetical(item: &Item) -> String {
    format!(" ({})", format_item_slot_label(item))
}

pub fn format_item_block(item: &Item) -> String {
    format!(
        "**{}**{} [{}] - ${}\n{}",
        format_item_display_name(item),
        format_item_slot_parenthetical(item),
        item.stats.format_triplet(),
        item.value,
        item.description,
    )
}

fn item_type_label(item_type: ItemType) -> &'static str {
    match item_type {
        ItemType::Gear => "gear",
        ItemType::Weapon => "weapon",
        ItemType::Misc => "misc",
    }
}

pub fn oxford_join(names: &[&str]) -> String {
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

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Rough fraction for scout success odds (denominator capped at 10).
fn format_chance_fraction(chance: f64) -> String {
    let mut best_num = 1u32;
    let mut best_den = 1u32;
    let mut best_err = f64::MAX;
    for den in 1..=10 {
        let num = (chance * den as f64).round() as u32;
        if num == 0 {
            continue;
        }
        let err = (num as f64 / den as f64 - chance).abs();
        if err < best_err {
            best_err = err;
            best_num = num;
            best_den = den;
        }
    }
    let g = gcd(best_num, best_den);
    format!("{}/{}", best_num / g, best_den / g)
}

pub fn scout_returned_message_key(chance: f64) -> &'static str {
    match scout_mood_from_chance(chance) {
        "impossible" => "scout_returned_impossible",
        "terrified" => "scout_returned_terrified",
        "nervous" => "scout_returned_nervous",
        "confident" => "scout_returned_confident",
        _ => "scout_returned_cocky",
    }
}

fn scout_chance_message_key(chance: f64) -> &'static str {
    match scout_mood_from_chance(chance) {
        "impossible" => "scout_chance_impossible",
        "terrified" => "scout_chance_terrified",
        "nervous" => "scout_chance_nervous",
        "confident" => "scout_chance_confident",
        _ => "scout_chance_cocky",
    }
}

pub fn scout_mood_from_chance(chance: f64) -> &'static str {
    if chance == 0.0 {
        "impossible"
    } else if chance <= 0.25 {
        "terrified"
    } else if chance <= 0.50 {
        "nervous"
    } else if chance <= 0.75 {
        "confident"
    } else {
        "cocky"
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
    let mut out = chud_msg!("scout_scouted", player_name, quest_title);

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

    let chance_key = scout_chance_message_key(chance);
    if scout_mood_from_chance(chance) == "impossible" {
        out.push_str(&format!("\n{}", chud_msg!(chance_key)));
    } else {
        out.push_str(&format!(
            "\n{}",
            chud_msg!(chance_key, format_chance_fraction(chance))
        ));
    }

    out
}
