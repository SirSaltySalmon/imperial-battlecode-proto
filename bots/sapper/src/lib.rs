//! Sapper: plays the objective.
//!
//! Attack: the whole team runs at one site (a different one from round to round), the carrier plants as soon
//! as a teammate covers it, and everyone holds an angle on the charge. Defence: spread over the sites and
//! hold. Buys the best it can afford, with a different weapon per unit slot.
//!
//! The play itself is in `bots/squad`; this crate only names the style.
use ibc::Bot;
use squad::{Attack, Buy, Defence, Squad, Style};

const STYLE: Style = Style { attack: Attack::Execute, defence: Defence::Hold, buy: Buy::Best, mixed_arms: true };

pub struct Sapper(Squad);

impl Bot for Sapper {
    fn new() -> Self {
        Sapper(Squad::new(STYLE))
    }

    fn turn(&mut self) {
        self.0.turn()
    }
}
