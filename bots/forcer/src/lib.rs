//! Forcer: cheap guns, close fights.
//!
//! Buys an SMG or a shotgun and light armour every round, never saves. Attack: runs at a site. Defence: half
//! the team holds the chokes in front of the sites and hunts whatever it hears.
//!
//! The play itself is in `bots/squad`; this crate only names the style.
use ibc::Bot;
use squad::{Attack, Buy, Defence, Squad, Style};

const STYLE: Style = Style { attack: Attack::Execute, defence: Defence::Push, buy: Buy::Force, mixed_arms: false };

pub struct Forcer(Squad);

impl Bot for Forcer {
    fn new() -> Self {
        Forcer(Squad::new(STYLE))
    }

    fn turn(&mut self) {
        self.0.turn()
    }
}
