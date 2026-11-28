//! Saver: plays the economy.
//!
//! Buys a full loadout or nothing: when the rifle and armour are out of reach it keeps its credits for the
//! next round, unless the round must be won. Otherwise it attacks like the sapper and holds like the holder.
//!
//! The play itself is in `bots/squad`; this crate only names the style.
use ibc::Bot;
use squad::{Attack, Buy, Defence, Squad, Style};

const STYLE: Style =
    Style { attack: Attack::Execute, defence: Defence::Hold, buy: Buy::Eco, mixed_arms: false, loadout: None };

pub struct Saver(Squad);

impl Bot for Saver {
    fn new() -> Self {
        Saver(Squad::new(STYLE))
    }

    fn turn(&mut self) {
        self.0.turn()
    }
}
