use casino::money::Money;
use casino::slots::SlotMachine;

use crate::game::domain::player::Player;
use crate::game::persistence::storage;

#[derive(Debug)]
pub enum SlotsError {
    InsufficientFunds,
}

/// Default symbol weights — must match `SlotMachine::new_with_default_symbols`.
pub const DEFAULT_PAY_TABLE: &[(char, u32)] = &[
    ('🍋', 30),
    ('🍒', 30),
    ('🍊', 30),
    ('🍉', 30),
    ('🔔', 20),
    ('🍌', 20),
    ('🍫', 10),
    ('💰', 2),
    ('💎', 1),
];

pub struct SlotsSpin {
    pub reels: Vec<char>,
    pub pay_in: u32,
    pub payout: i64,
    pub wins: Vec<(char, usize, i64)>,
}

impl SlotsSpin {
    /// Five of the same symbol — mini-jackpot tier in the default pay table.
    pub fn mini_jackpot_symbol(&self) -> Option<char> {
        self.wins
            .iter()
            .find(|(_, count, _)| *count == 5)
            .map(|(symbol, _, _)| *symbol)
    }
}

/// Payout for `match_count` of the same symbol at the given pay-in (3–5).
pub fn symbol_payout(pay_in: u32, weight: u32, match_count: usize) -> i64 {
    if !(3..=5).contains(&match_count) {
        return 0;
    }
    let sym_value = (pay_in as f32 * 120.0 / weight as f32) as i64;
    sym_value * (match_count - 2) as i64
}

/// Per-symbol 5× payouts for the current bet, in pay-table order.
pub fn pay_table_5x_row(pay_in: u32) -> Vec<(char, i64)> {
    DEFAULT_PAY_TABLE
        .iter()
        .map(|&(symbol, weight)| (symbol, symbol_payout(pay_in, weight, 5)))
        .collect()
}

fn money_major(m: Money) -> i64 {
    m.to_string()
        .trim_start_matches('$')
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0)
}

pub fn spin(pay_in: u32) -> SlotsSpin {
    let machine = SlotMachine::new_with_default_symbols(pay_in as f32);
    let pulled = machine.pull();
    let reels: Vec<char> = pulled.iter().map(|c| **c).collect();
    let entries = machine.payout(pulled);

    let wins: Vec<(char, usize, i64)> = entries
        .iter()
        .map(|e| (e.symbol, e.count, money_major(e.payout)))
        .collect();
    let payout = money_major(entries.iter().fold(Money::ZERO, |acc, e| acc + e.payout));

    SlotsSpin {
        reels,
        pay_in,
        payout,
        wins,
    }
}

pub fn spin_for_player(player: &mut Player, pay_in: u32) -> Result<SlotsSpin, SlotsError> {
    if player.cash < pay_in {
        return Err(SlotsError::InsufficientFunds);
    }
    player.cash = player.cash.saturating_sub(pay_in);
    let result = spin(pay_in);
    let payout = result.payout.max(0) as u32;
    player.cash = player.cash.saturating_add(payout);
    storage::save_player(player).expect("save player after slots spin");
    Ok(result)
}
