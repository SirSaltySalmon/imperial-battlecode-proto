//! Retaker: defends by taking sites back.
//!
//! Defence: one anchor per site and the rest stacked on a site chosen per round. After a plant the team
//! gathers short of the charge and goes in together; the nearest defender defuses. Attack: walks in
//! silently and runs only when the clock demands it.
//!
//! The play itself is in `bots/squad`; this crate only names the style.
use ibc::Bot;
use squad::{Attack, Buy, Defence, Squad, Style};

const STYLE: Style = Style { attack: Attack::Slow, defence: Defence::Retake, buy: Buy::Best, mixed_arms: false };

pub struct Retaker(Squad);

impl Bot for Retaker {
    fn new() -> Self {
        Retaker(Squad::new(STYLE))
    }

    fn turn(&mut self) {
        self.0.turn()
    }
}
