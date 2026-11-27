//! `World.trails`: where each bullet that ended in a tick flew during that tick, for drawing. A bullet that
//! hits within its first tick of flight is in no frame's `projectiles`, so the trail is the only record of it.

use ibc_engine::abi::*;
use ibc_engine::{fixed, Map, Match, Ruleset};

/// 20x9, two per team, open floor.
const ARENA: &str = "name: trails\nteam_size: 2\n---\n\
####################\n\
#a................d#\n\
#a................d#\n\
#..................#\n\
#..................#\n\
#..................#\n\
#..................#\n\
#................11#\n\
####################\n";

const EAST: i32 = 0;
const WEST: i32 = 32768;

fn c(tx: i32, ty: i32) -> (i32, i32) {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

/// A match in its first action tick with unit 0 at `at`, facing `facing`, and the others parked out of the way.
fn setup(at: (i32, i32), facing: i32) -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 1);
    while m.world.phase != PHASE_ACTION {
        m.step();
    }
    for (u, pos) in [(1, c(1, 7)), (2, c(18, 7)), (3, c(17, 7))] {
        m.world.units[u].pos = pos;
    }
    m.world.units[0].pos = at;
    m.world.units[0].facing = facing;
    m
}

/// Fire unit 0's sidearm at `aim` and step until the bullet is gone; returns the trail and how many frames
/// showed the bullet in flight.
fn shoot(m: &mut Match, aim: i32) -> (ibc_engine::world::Trail, usize) {
    m.step();
    assert_eq!(m.act(0, A_FIRE, aim, 0, 0), OK);
    let mut in_flight = 0;
    for _ in 0..12 {
        m.step();
        if let Some(t) = m.world.trails.iter().find(|t| t.owner == 0) {
            return (*t, in_flight);
        }
        in_flight += m
            .world
            .projectiles
            .iter()
            .filter(|p| p.owner == 0)
            .count()
            .min(1);
    }
    panic!("the bullet never ended");
}

#[test]
fn a_shot_into_a_near_wall_is_drawn_from_the_muzzle_to_the_wall() {
    let mut m = setup(c(3, 3), WEST);
    let from = m.world.units[0].pos;
    let (t, in_flight) = shoot(&mut m, WEST);
    assert_eq!(
        in_flight, 0,
        "it hit the wall in its first tick: no frame shows it in flight"
    );
    assert!(
        t.from.0 < from.0 && from.0 - t.from.0 <= TILE,
        "it starts at the muzzle: {:?} from {from:?}",
        t.from
    );
    assert!(
        (t.to.0 - TILE).abs() <= 2 && t.to.1 == from.1,
        "it stops at the wall's face: {:?}",
        t.to
    );
    assert!(t.vel.0 < 0 && t.vel.1 == 0);
    let frame: serde_json::Value = serde_json::from_str(&m.frame_json()).unwrap();
    assert_eq!(
        frame["trails"][0]["x1"],
        serde_json::json!(t.to.0),
        "frames carry it: {}",
        frame["trails"]
    );
    m.step();
    assert!(m.world.trails.is_empty(), "a trail lasts one tick");
}

#[test]
fn a_hit_ends_the_trail_at_the_victim() {
    let mut m = setup(c(3, 3), EAST);
    m.world.units[2].pos = c(6, 3);
    let victim = m.world.units[2].pos;
    let (t, in_flight) = shoot(&mut m, EAST);
    assert_eq!(in_flight, 0);
    assert!(
        m.world
            .events
            .iter()
            .any(|e| e.kind == EVENT_DAMAGE && e.target == 2),
        "it hit"
    );
    let reach = fixed::dist(t.to, victim);
    assert!(
        reach <= Ruleset::builtin().core.unit_radius,
        "it stops on the victim's body: {reach}"
    );
}

#[test]
fn a_long_flight_ends_with_a_trail_from_its_last_position() {
    let mut m = setup(c(2, 3), EAST);
    let rifle = Ruleset::builtin()
        .weapon_names
        .iter()
        .position(|n| n == "rifle")
        .unwrap();
    m.world.units[0].weapons[0] = rifle as i32;
    m.world.units[0].slot = 0;
    m.world.units[0].ammo[0] = 30;
    m.step();
    assert_eq!(m.act(0, A_FIRE, EAST, 0, 0), OK);
    let mut last = None;
    for _ in 0..12 {
        m.step();
        if let Some(t) = m.world.trails.iter().find(|t| t.owner == 0) {
            assert_eq!(
                Some(t.from),
                last,
                "it continues from where the last frame showed it"
            );
            assert!(
                (t.to.0 - 19 * TILE).abs() <= 2,
                "and ends at the far wall: {:?}",
                t.to
            );
            return;
        }
        last = m
            .world
            .projectiles
            .iter()
            .find(|p| p.owner == 0)
            .map(|p| p.pos);
    }
    panic!("the bullet never ended");
}
