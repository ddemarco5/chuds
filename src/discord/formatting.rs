use crate::chud_msg;
use crate::discord::components_v2::{
    components_v2_flags, Component, ComponentsV2Message, Container, ContainerChild, Separator,
    TextDisplay,
};
use crate::game::domain::graveyard::GraveyardEntry;
use crate::game::domain::item::{Item, ItemType};
use crate::game::domain::player::Player;
use crate::game::domain::quest_result::{CompletedTrial, QuestResult};
use crate::game::mechanics::quest_builder::StatChoice;
use crate::game::engine::KillResult;
use crate::game::mechanics::simulation::effective_stats;
use crate::game::generation::generators::collapse_whitespace;
use crate::game::persistence::item_registry::ItemRegistry;

#[derive(Debug, Clone, Copy)]
pub struct PlayerStatsBlockOptions {
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

fn format_chud_name_block(player: &Player) -> String {
    let chud = player.chud_ref();
    format!("**{}**\n-# {}", chud.name, chud.description)
}

fn format_chud_stats_and_jobs(player: &Player, registry: &ItemRegistry) -> String {
    let effective = effective_stats(player, registry);
    format!(
        "{}\n{}",
        player.format_effective_stats_line(effective),
        player.format_job_record(),
    )
}

/// Name, description, effective stats, and job record — the identity header for `/chud`.
pub fn format_chud_identity_block(player: &Player, registry: &ItemRegistry) -> String {
    format!(
        "{}\n\n{}",
        format_chud_name_block(player),
        format_chud_stats_and_jobs(player, registry),
    )
}

pub fn format_player_stats_block(
    player: &Player,
    registry: &ItemRegistry,
    options: PlayerStatsBlockOptions,
) -> String {
    let equipment = format_equipment_summary(player, registry);
    let mut out = format_chud_name_block(player);

    if !equipment.is_empty() {
        out.push_str("\n\n");
        out.push_str(&equipment);
    }

    out.push_str("\n\n");
    out.push_str(&format_chud_stats_and_jobs(player, registry));

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

struct TrialCardText {
    situation: String,
    result: String,
}

/// Header, briefing, spoiled trial cards, and spoiled accented outcome for job DMs.
pub struct DmCompletionContent {
    header: String,
    briefing: String,
    trials: Vec<TrialCardText>,
    summary: String,
    item: Option<String>,
    passed: bool,
    died: bool,
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
    let (summary, item, passed, died) = if let Some(kill) = kill {
        let footer = format_death_footer(kill);
        let summary = format_completion_outcome_box(Some("DIED"), &result.summary, &footer, None);
        (summary, None, false, true)
    } else {
        let footer = format_completion_rewards(player_name, player, level_up, result.passed, reward);
        let hospital_msg = hospitalized.then(|| chud_msg!("dm_hospitalized", player_name));
        let summary = format_completion_outcome_box(
            None,
            &result.summary,
            &footer,
            hospital_msg.as_deref(),
        );
        let item = item_awarded.map(|item| format_item_award(item, item_award_disposition));
        (summary, item, result.passed, false)
    };
    DmCompletionContent {
        header: format_dm_header(player, registry),
        briefing: format_dm_quest(result),
        trials: result
            .trials
            .iter()
            .enumerate()
            .map(|(i, trial)| format_dm_trial(player_name, trial, i))
            .collect(),
        summary,
        item,
        passed,
        died,
    }
}

fn outcome_accent(passed: bool, died: bool) -> u32 {
    if died {
        ACCENT_DEATH
    } else if passed {
        ACCENT_PASSED
    } else {
        ACCENT_FAILED
    }
}

fn spoiled_trial_container(trial: &TrialCardText) -> Component {
    Component::Container(Container::spoiled(vec![
        ContainerChild::Text(TextDisplay::new(trial.situation.clone())),
        ContainerChild::Separator(Separator::section()),
        ContainerChild::Text(TextDisplay::new(trial.result.clone())),
    ]))
}

fn spoiled_outcome_container(summary: &str, item: Option<&str>, passed: bool, died: bool) -> Component {
    let mut inner = vec![ContainerChild::Text(TextDisplay::new(summary.to_string()))];
    if let Some(item) = item.filter(|text| !text.is_empty()) {
        inner.push(ContainerChild::Separator(Separator::section()));
        inner.push(ContainerChild::Text(TextDisplay::new(item.to_string())));
    }
    Component::Container(Container::with_accent_spoiled(
        outcome_accent(passed, died),
        inner,
    ))
}

fn notice_container(message: &str, passed: bool) -> Component {
    Component::Container(Container::with_accent(
        outcome_accent(passed, false),
        vec![ContainerChild::Text(TextDisplay::new(message.to_string()))],
    ))
}

pub fn build_dm_completion_messages(content: &DmCompletionContent) -> Vec<ComponentsV2Message> {
    let mut components = vec![
        Component::Text(TextDisplay::new(content.header.clone())),
        Component::Text(TextDisplay::new(content.briefing.clone())),
    ];
    components.extend(content.trials.iter().map(spoiled_trial_container));
    components.extend(build_dm_summary_components(content).components);
    ComponentsV2Message::channel_packed(components)
}

/// Outcome block only — same spoiled accented container as the full report.
pub fn build_dm_summary_components(content: &DmCompletionContent) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![spoiled_outcome_container(
        &content.summary,
        content.item.as_deref(),
        content.passed,
        content.died,
    )])
}

/// Single-notice DM — accented container, not spoiled.
pub fn build_dm_notice_components(message: &str, passed: bool) -> ComponentsV2Message {
    ComponentsV2Message::channel(vec![notice_container(message, passed)])
}

/// Summary, gold, hospital, and level-up — one multiline block above any item divider.
/// Pass/fail is the container accent; `outcome` is only used for death (`DIED`).
fn format_completion_outcome_box(
    outcome: Option<&str>,
    summary: &str,
    footer: &str,
    hospital_msg: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    if let Some(outcome) = outcome {
        sections.push(format!("**{outcome}**"));
    }
    sections.push(summary.to_string());
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
        "{} -- *{}*",
        player.chud_ref().name,
        player.format_effective_stats(effective),
    )
}

fn format_dm_quest(result: &QuestResult) -> String {
    format!(
        "**{}** - _{}_\n{}",
        result.quest_title, result.quest_giver, result.quest_description
    )
}

fn format_trial_stat(trial: &CompletedTrial) -> String {
    let (emoji, name) = match trial.stat_used {
        StatChoice::Strength => ("\u{1F4AA}", "Strength"),
        StatChoice::Smarts => ("\u{1F9E0}", "Smarts"),
        StatChoice::Stealth => ("\u{1F977}", "Stealth"),
    };
    if trial.chose_optimal {
        format!("{emoji}\u{2726}{name}\u{2726}")
    } else {
        format!("{emoji}{name}")
    }
}

fn format_dm_trial(player_name: &str, trial: &CompletedTrial, index: usize) -> TrialCardText {
    let pass_str = if trial.passed { "\u{2705}" } else { "\u{274c}" };
    TrialCardText {
        situation: format!("**Trial {}** - {}", index + 1, trial.situation),
        result: format!(
            "{} used {} and rolled {} against {} -- {}\n\n{}",
            player_name,
            format_trial_stat(trial),
            trial.format_player_roll(),
            trial.trial_roll,
            pass_str,
            markdown_italic_line(&trial.narrative),
        ),
    }
}

fn format_completion_rewards(
    player_name: &str,
    player: &Player,
    level_up: &crate::game::domain::player::LevelUp,
    passed: bool,
    reward: u32,
) -> String {
    let mut out = String::new();
    if passed && reward > 0 {
        out.push_str(&chud_msg!("chud_brings_money", reward));
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

fn format_item_award(
    item: &Item,
    item_award_disposition: Option<crate::game::engine::ItemAwardDisposition>,
) -> String {
    let slot = format_item_slot_label(item);
    match item_award_disposition {
        Some(crate::game::engine::ItemAwardDisposition::Sold(gold)) => format!(
            "**Stash full — sold {} for ${gold}:** ({slot})\nStats: {}\n_{}_",
            item.name,
            item.stats.format_triplet(),
            item.description,
        ),
        Some(crate::game::engine::ItemAwardDisposition::Equipped) => format!(
            "**Stash full — auto-equipped:** {} ({slot})\nStats: {}\n_{}_",
            item.name,
            item.stats.format_triplet(),
            item.description,
        ),
        _ => format!(
            "**Item stashed:** {} ({slot})\nStats: {}\n_{}_",
            item.name,
            item.stats.format_triplet(),
            item.description,
        ),
    }
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
