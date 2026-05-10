use std::io::{self, BufRead, Write};

use crate::quest_result::QuestResult;

const MIN_STAT: u8 = 1;
const MAX_STAT: u8 = 10;
/// Threshold = STAT_LEVEL_BASE * current_stat. Scales cost upward as the stat grows.
pub const STAT_LEVEL_BASE: u32 = 8;
/// Threshold = EXP_LEVEL_BASE * current_experience. Scales cost upward as experience grows.
pub const EXP_LEVEL_BASE: u32 = 5;

pub struct Player {
    pub name: String,
    pub description: String,
    pub strength: u8,
    pub smarts: u8,
    pub stealth: u8,
    pub experience: u8,
    pub str_successes: u32,
    pub smt_successes: u32,
    pub sth_successes: u32,
    pub quest_successes: u32,
}

fn apply_wins(val: &mut u8, counter: &mut u32, wins: u32, base: u32) -> bool {
    *counter += wins;
    let threshold = base * *val as u32;
    if *counter >= threshold && *val < MAX_STAT {
        *val += 1;
        *counter -= threshold;
        true
    } else {
        false
    }
}

impl Player {
    pub fn print_stats(&self) {
        println!("Stats: str={} smt={} sth={} exp={}",
            self.strength, self.smarts, self.stealth, self.experience);
    }

    pub fn record_quest(&mut self, result: &QuestResult) {
        let (str_wins, smt_wins, sth_wins) = result.stat_wins();

        let str_up = apply_wins(&mut self.strength,  &mut self.str_successes, str_wins, STAT_LEVEL_BASE);
        let smt_up = apply_wins(&mut self.smarts,    &mut self.smt_successes, smt_wins, STAT_LEVEL_BASE);
        let sth_up = apply_wins(&mut self.stealth,   &mut self.sth_successes, sth_wins, STAT_LEVEL_BASE);

        let exp_up = if result.passed {
            apply_wins(&mut self.experience, &mut self.quest_successes, 1, EXP_LEVEL_BASE)
        } else { false };

        if str_up || smt_up || sth_up || exp_up {
            println!();
            if str_up { println!("  [LEVEL UP] Strength   -> {}", self.strength); }
            if smt_up { println!("  [LEVEL UP] Smarts     -> {}", self.smarts); }
            if sth_up { println!("  [LEVEL UP] Stealth    -> {}", self.stealth); }
            if exp_up { println!("  [LEVEL UP] Experience -> {}", self.experience); }
        }

        println!("Stats: str={} ({}/{}) smt={} ({}/{}) sth={} ({}/{}) exp={} ({}/{})",
            self.strength,  self.str_successes,  STAT_LEVEL_BASE * self.strength as u32,
            self.smarts,    self.smt_successes,  STAT_LEVEL_BASE * self.smarts as u32,
            self.stealth,   self.sth_successes,  STAT_LEVEL_BASE * self.stealth as u32,
            self.experience, self.quest_successes, EXP_LEVEL_BASE * self.experience as u32,
        );
    }
}

pub fn prompt_player() -> anyhow::Result<Player> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();

    println!("Create your Chud:");
    let name = prompt_text(&mut stdin, "  Name: ")?;
    let description = prompt_text(&mut stdin, "  Description: ")?;

    println!("Stats ({}-{} for each):", MIN_STAT, MAX_STAT);
    let strength   = prompt_stat(&mut stdin, "Strength")?;
    let smarts     = prompt_stat(&mut stdin, "Smarts")?;
    let stealth    = prompt_stat(&mut stdin, "Stealth")?;
    let experience = prompt_stat(&mut stdin, "Experience")?;

    Ok(Player {
        name, description, strength, smarts, stealth, experience,
        str_successes: 0, smt_successes: 0, sth_successes: 0, quest_successes: 0,
    })
}

fn prompt_stat<R: BufRead>(reader: &mut R, name: &str) -> anyhow::Result<u8> {
    loop {
        let raw = read_line(reader, &format!("  {} ({}-{}): ", name, MIN_STAT, MAX_STAT))?;
        match raw.trim().parse::<u8>() {
            Ok(v) if (MIN_STAT..=MAX_STAT).contains(&v) => return Ok(v),
            _ => println!("Please enter an integer between {} and {}.", MIN_STAT, MAX_STAT),
        }
    }
}

fn prompt_text<R: BufRead>(reader: &mut R, prompt: &str) -> anyhow::Result<String> {
    loop {
        let raw = read_line(reader, prompt)?;
        let trimmed = raw.trim().to_string();
        if !trimmed.is_empty() { return Ok(trimmed); }
        println!("Please enter a value.");
    }
}

fn read_line<R: BufRead>(reader: &mut R, prompt: &str) -> anyhow::Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    Ok(buf)
}
