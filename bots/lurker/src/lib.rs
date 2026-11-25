//! Lurker: splits the attack.
//!
//! Attack: three units run at the site while two walk the long way round past another site, silently, and
//! arrive behind the defenders who rotated. Defence: spread over the sites and hold.
//!
//! The play itself is in `bots/squad`; this crate only names the style.
use ibc::Bot;
use squad::{Attack, Buy, Defence, Squad, Style};

const STYLE: Style = Style { attack: Attack::Split, defence: Defence::Hold, buy: Buy::Best, mixed_arms: true };

pub struct Lurker(Squad);

impl Bot for Lurker {
    fn new() -> Self {
        Lurker(Squad::new(STYLE))
    }

    fn turn(&mut self) {
        self.0.turn()
    }
}
