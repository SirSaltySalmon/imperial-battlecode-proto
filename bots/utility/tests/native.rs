//! Plays the bot against itself on the engine through `ibc::native` (the same `Host` calls the wasm
//! runner makes) and checks that kits are drafted, abilities are used and the match is deterministic.
use ibc::{Bot, A_ABILITY};
use ibc_engine::{Input, Map, Match, Ruleset};
use utility::Utility;

struct Outcome {
    hashes: Vec<u64>,
    kits: Vec<i32>,
    /// Accepted `A_ABILITY` intents per slot.
    uses: [u32; 2],
    effects: usize,
}

fn play(map: &str, seed: u64) -> Outcome {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(map).unwrap(), seed);
    let mut bots: Vec<Option<Utility>> = (0..m.unit_count()).map(|_| None).collect();
    let mut out = Outcome { hashes: Vec::new(), kits: Vec::new(), uses: [0, 0], effects: 0 };
    while !m.finished() {
        for unit in m.turn_order() {
            let slot = &mut bots[unit as usize];
            ibc::native::with_host(&mut m, unit, || slot.get_or_insert_with(Utility::new).turn());
        }
        let intents: Vec<Option<(u32, i32, i32, i32)>> = m.world.units.iter().map(|u| u.intent.primary).collect();
        let before: Vec<[i32; 2]> = m.world.units.iter().map(|u| u.abil.charges).collect();
        let _: Vec<Input> = m.take_inputs();
        m.step();
        for (i, intent) in intents.iter().enumerate() {
            if let Some((A_ABILITY, slot, _, _)) = *intent {
                let slot = slot as usize;
                out.uses[slot] += (m.world.units[i].abil.charges[slot] < before[i][slot]) as u32;
            }
        }
        out.effects = out.effects.max(m.world.effects.len());
        out.hashes.push(m.state_hash());
        assert!(m.tick() < 20_000, "match did not finish");
    }
    out.kits = m.world.units.iter().map(|u| u.kit).collect();
    out
}

#[test]
fn drafts_kits_uses_abilities_and_is_deterministic() {
    for map in ["../../maps/crossfire.ibcmap", "../../maps/tunnels.ibcmap"] {
        let text = std::fs::read_to_string(map).unwrap();
        let a = play(&text, 3);
        assert!(a.kits.iter().all(|&k| k >= 1), "{map}: everyone drafted an ability kit: {:?}", a.kits);
        assert!(a.kits.iter().any(|&k| k != a.kits[0]), "{map}: the team fields several kits");
        assert!(a.uses[0] > 0, "{map}: no free ability was ever used");
        assert!(a.effects > 0, "{map}: no effect was ever in the world");
        assert_eq!(a.hashes, play(&text, 3).hashes, "{map}: two runs differ");
    }
}
