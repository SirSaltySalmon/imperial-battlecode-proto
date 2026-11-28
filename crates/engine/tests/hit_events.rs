//! What a unit learns when it is hit, driven through the same `Host` calls bots use: the unit that took the
//! hit alone reads where it came from (`EventInfo.dir`, `src_x`, `src_y`), no bot reads which enemy did it,
//! and the world record (replays, the viewer) keeps everything. Units are placed by hand between ticks.

use ibc_engine::abi::*;
use ibc_engine::{fixed, Map, Match, Ruleset};

/// 20x9, two per team, open floor.
const ARENA: &str = "name: hits\nteam_size: 2\n---\n\
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
const SOUTH: i32 = 16384;
const WEST: i32 = 32768;
const NORTH: i32 = 49152;

fn c(tx: i32, ty: i32) -> (i32, i32) {
    (tx * TILE + TILE / 2, ty * TILE + TILE / 2)
}

fn kit(name: &str) -> i32 {
    Ruleset::builtin().kit_names.iter().position(|n| n == name).expect("kit exists") as i32
}

fn ability(name: &str) -> AbilityRules {
    let r = Ruleset::builtin();
    r.abilities[r.ability_names.iter().position(|n| n == name).expect("ability exists")]
}

/// A match in its first action tick: unit 0 (team A) drafted `caster_kit`, everyone else plays kit 0. Units 1
/// and 3 are parked in corners looking at the wall.
fn setup(caster_kit: &str) -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 1);
    assert_eq!(m.act(0, A_DRAFT, kit(caster_kit), 0, 0), OK);
    while m.world.phase != PHASE_ACTION {
        m.step();
    }
    place(&mut m, 1, c(1, 7), SOUTH);
    place(&mut m, 3, c(18, 1), NORTH);
    m
}

fn place(m: &mut Match, unit: usize, pos: (i32, i32), facing: i32) {
    m.world.units[unit].pos = pos;
    m.world.units[unit].facing = facing;
}

fn steps(m: &mut Match, n: usize) {
    for _ in 0..n {
        m.step();
    }
}

fn row<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, index, &mut buf) >= 0);
    from_bytes(&buf).unwrap()
}

fn events(m: &Match, unit: u32) -> Vec<EventInfo> {
    (0..m.count(unit, T_EVENTS)).map(|i| row(m, unit, T_EVENTS, i as u32)).collect()
}

/// The one event of `kind` that happened to `target`, as `reader` reads it.
fn read(m: &Match, reader: u32, kind: i32, target: usize) -> EventInfo {
    let all = events(m, reader);
    let mut hits = all.iter().filter(|e| e.kind == kind && e.target == target as i32);
    let e = *hits.next().unwrap_or_else(|| panic!("unit {reader} gets kind {kind} on unit {target}: {all:?}"));
    assert!(hits.next().is_none(), "one such event this tick: {all:?}");
    e
}

/// The same event without where it came from: how everyone but its target reads it.
fn undirected(e: EventInfo) -> EventInfo {
    EventInfo { dir: DIR_HIDDEN, src_x: POS_HIDDEN, src_y: POS_HIDDEN, ..e }
}

fn angle(from: (i32, i32), to: (i32, i32)) -> i32 {
    fixed::atan2(to.1 - from.1, to.0 - from.0)
}

/// Steps from a use until the tick a thrown device lands `dist` away is done.
fn until_landed(a: &AbilityRules, dist: i32) -> usize {
    (a.delay + ((dist + a.speed - 1) / a.speed).max(1)) as usize
}

/// Unit 0 shoots unit 2 in the back down an open row; unit 2 and its teammate look away. Returns the match on
/// the tick the bullet lands and the bullet's velocity.
fn shot_in_the_back(victim_hp: i32) -> (Match, (i32, i32)) {
    let mut m = setup("standard");
    place(&mut m, 0, c(2, 3), EAST);
    // 9 tiles: inside the sidearm's 15-tile reach, and more than one tick of flight at 8 tiles a tick.
    place(&mut m, 2, c(11, 3), EAST);
    m.world.units[2].hp = victim_hp;
    m.step();
    let aim = angle(m.world.units[0].pos, m.world.units[2].pos);
    assert_eq!(m.act(0, A_FIRE, aim, 0, 0), OK);
    let mut vel = None;
    for _ in 0..12 {
        m.step();
        vel = vel.or(m.world.projectiles.first().map(|p| p.vel));
        if m.world.events.iter().any(|e| e.kind == EVENT_DAMAGE) {
            break;
        }
    }
    let vel = vel.expect("the bullet flew for at least a tick");
    assert!(m.world.events.iter().any(|e| e.kind == EVENT_DAMAGE && e.target == 2), "the bullet landed");
    assert!(!m.world.team_sees_point(TEAM_B, m.world.units[0].pos), "team B does not see the shooter");
    (m, vel)
}

#[test]
fn a_bullet_in_the_back_tells_only_the_victim_its_heading_and_nobody_the_shooter() {
    let (m, vel) = shot_in_the_back(100);
    let hit = read(&m, 2, EVENT_DAMAGE, 2);
    assert_eq!((hit.actor, hit.target, (hit.x, hit.y)), (-1, 2, m.world.units[2].pos), "the shooter is not named");
    assert!(hit.value > 0);
    assert_eq!(hit.dir, fixed::atan2(-vel.1, -vel.0), "back along the bullet's flight");
    let towards_shooter = angle(m.world.units[2].pos, m.world.units[0].pos);
    assert!(fixed::angle_diff(hit.dir, towards_shooter).abs() < FULL_TURN / 64, "it points back down the row");
    assert_eq!((hit.src_x, hit.src_y), (POS_HIDDEN, POS_HIDDEN), "the shooter's position is never told");

    assert_eq!(m.count(3, T_EVENTS), m.count(2, T_EVENTS), "teammates get the same rows");
    assert_eq!(read(&m, 3, EVENT_DAMAGE, 2), undirected(hit), "the victim's teammate does not learn the heading");

    // The shooter's own team sees the hit and keeps the id of its own unit, but not the heading either.
    let ours = read(&m, 0, EVENT_DAMAGE, 2);
    assert_eq!(ours, undirected(EventInfo { actor: 0, ..hit }), "an own team's hit keeps its actor");
    assert_eq!(read(&m, 1, EVENT_DAMAGE, 2), ours);

    // The record keeps the truth.
    let truth = m.world.events.iter().find(|e| e.kind == EVENT_DAMAGE).unwrap();
    assert_eq!(*truth, EventInfo { actor: 0, ..hit });
}

#[test]
fn a_kill_is_unchanged_and_names_its_killer_to_both_teams() {
    let (m, _) = shot_in_the_back(1);
    assert!(!m.world.units[2].alive);
    let pistol = m.world.units[0].weapons[m.world.units[0].slot];
    for reader in [0, 3] {
        let kill = read(&m, reader, EVENT_KILL, 2);
        assert_eq!((kill.actor, kill.value, (kill.x, kill.y)), (0, pistol, m.world.units[2].pos), "unit {reader}");
        assert_eq!((kill.dir, kill.src_x, kill.src_y), (DIR_HIDDEN, POS_HIDDEN, POS_HIDDEN));
    }
    assert_eq!(read(&m, 2, EVENT_KILL, 2), read(&m, 3, EVENT_KILL, 2), "a kill tells the victim nothing more");
    assert_eq!(read(&m, 3, EVENT_DAMAGE, 2).actor, -1, "the killing shot's damage stays anonymous");
}

#[test]
fn fire_tells_each_unit_it_burns_the_centre_of_the_fire() {
    let mut m = setup("seeker");
    let fire = ability("incendiary");
    let centre = c(8, 3);
    place(&mut m, 0, c(2, 3), EAST);
    place(&mut m, 2, (centre.0 + 700, centre.1 - 600), WEST);
    place(&mut m, 1, (centre.0 - 500, centre.1 + 400), WEST); // the thrower's careless teammate
    m.world.units[0].abil.charges[1] = 1;
    assert_eq!(m.act(0, A_ABILITY, 1, centre.0, centre.1), OK);
    // Thrown straight along the row, so it lands exactly on the target point.
    steps(&mut m, until_landed(&fire, 6 * TILE));
    let lit = m.world.effects.iter().find(|e| e.info.kind == EFFECT_FIRE).expect("burning");
    assert_eq!((lit.info.x, lit.info.y), centre);

    let burnt = read(&m, 2, EVENT_DAMAGE, 2);
    let at = m.world.units[2].pos;
    assert_eq!((burnt.actor, burnt.value, (burnt.x, burnt.y)), (-1, fire.power, at), "the thrower is not named");
    assert_eq!((burnt.src_x, burnt.src_y), centre, "the fire's centre, not the thrower");
    assert_eq!(burnt.dir, angle(at, centre));
    assert_eq!(read(&m, 3, EVENT_DAMAGE, 2), undirected(burnt), "the teammate far away learns no centre");
    assert_eq!(read(&m, 0, EVENT_DAMAGE, 2), undirected(EventInfo { actor: 0, ..burnt }), "the thrower's team");

    let own = read(&m, 1, EVENT_DAMAGE, 1);
    assert_eq!(own.actor, -1, "fire on a teammate is credited to nobody");
    assert_eq!(((own.src_x, own.src_y), own.dir), (centre, angle(m.world.units[1].pos, centre)));
    assert_eq!(read(&m, 0, EVENT_DAMAGE, 1), undirected(own));
}

#[test]
fn a_flash_tells_each_blinded_unit_where_it_popped_and_not_who_threw_it() {
    let mut m = setup("lancer");
    let flash = ability("flash");
    let pop = c(8, 3);
    place(&mut m, 0, c(2, 3), NORTH); // the thrower, looking away from its own flash
    place(&mut m, 1, c(2, 5), EAST); // its teammate, looking at it
    place(&mut m, 2, c(12, 3), WEST); // an enemy facing it head-on
    place(&mut m, 3, c(12, 5), EAST); // an enemy facing away
    m.world.units[0].abil.charges[1] = 1;
    m.step();
    assert_eq!(m.act(0, A_ABILITY, 1, pop.0, pop.1), OK);
    // Thrown straight along the row, so it pops exactly on the target point.
    steps(&mut m, until_landed(&flash, 6 * TILE));
    let blind: Vec<i32> = m.world.units.iter().map(|u| u.blind).collect();
    assert!(blind[1] > 0 && blind[2] > 0 && blind[0] == 0 && blind[3] == 0, "{blind:?}");

    let flashed = read(&m, 2, EVENT_EFFECT, 2);
    let at = m.world.units[2].pos;
    assert_eq!((flashed.actor, flashed.value, (flashed.x, flashed.y)), (-1, EFFECT_FLASH, at), "nobody named");
    assert_eq!(((flashed.src_x, flashed.src_y), flashed.dir), (pop, angle(at, pop)), "the pop, not the thrower");
    assert_eq!(read(&m, 3, EVENT_EFFECT, 2), undirected(flashed), "its teammate learns no direction");
    assert!(events(&m, 2).iter().all(|e| e.target != 3), "a unit facing away is not affected and not told");

    let own = read(&m, 1, EVENT_EFFECT, 1);
    assert_eq!((own.actor, own.value), (0, EFFECT_FLASH), "a teammate's flash keeps its thrower");
    assert_eq!(((own.src_x, own.src_y), own.dir), (pop, angle(m.world.units[1].pos, pop)));
    assert_eq!(read(&m, 0, EVENT_EFFECT, 1), undirected(own), "the thrower itself is not told the direction");
    assert!(events(&m, 0).iter().all(|e| e.target != 0), "the thrower was not blinded");
}

#[test]
fn a_concussion_reports_through_the_same_event() {
    let mut m = setup("trapper");
    let concussion = ability("concussion");
    let centre = c(10, 3);
    place(&mut m, 0, c(4, 3), EAST);
    place(&mut m, 2, (centre.0 + 900, centre.1), WEST);
    m.world.units[0].abil.charges[1] = 1;
    m.step();
    assert_eq!(m.act(0, A_ABILITY, 1, centre.0, centre.1), OK);
    steps(&mut m, concussion.delay as usize + 1);
    assert!(m.world.units[2].slowed > 0, "the pop reached the enemy");
    let hit = read(&m, 2, EVENT_EFFECT, 2);
    assert_eq!((hit.actor, hit.value, (hit.src_x, hit.src_y)), (-1, EFFECT_CONCUSSION, centre));
    assert_eq!(hit.dir, WEST, "straight back towards the centre");
    assert_eq!(read(&m, 3, EVENT_EFFECT, 2), undirected(hit));
}
