//! Shared vision, shared hearing and fogged events, driven through the same `Host` calls bots use. Units are
//! placed by hand between ticks.

use ibc_engine::abi::*;
use ibc_engine::{fixed, Map, Match, Ruleset};

/// 20x9, two per team. A wall column at x = 9 over rows 1-4; rows 5-7 are open end to end.
const ARENA: &str = "name: sight\nteam_size: 2\n---\n\
####################\n\
#a.......#........d#\n\
#a.......#........d#\n\
#........#.........#\n\
#........#.........#\n\
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

/// A match in its first action tick. Unit 1 (team A) and unit 3 (team B) are parked in corners facing a wall.
fn setup() -> Match {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 1);
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

fn row<T: Copy + Default>(m: &Match, unit: u32, table: u32, index: u32) -> T {
    let mut buf = Vec::new();
    assert!(m.get(unit, table, index, &mut buf) >= 0);
    from_bytes(&buf).unwrap()
}

fn table<T: Copy + Default>(m: &Match, unit: u32, t: u32) -> Vec<T> {
    (0..m.count(unit, t)).map(|i| row(m, unit, t, i as u32)).collect()
}

fn enemy(m: &Match, unit: u32, id: i32) -> Option<UnitInfo> {
    table::<UnitInfo>(m, unit, T_ENEMIES).into_iter().find(|e| e.id == id)
}

fn angle(from: (i32, i32), to: (i32, i32)) -> i32 {
    fixed::atan2(to.1 - from.1, to.0 - from.0)
}

#[test]
fn a_unit_fires_at_an_enemy_only_its_teammate_sees() {
    let mut m = setup();
    place(&mut m, 0, c(3, 6), EAST);
    place(&mut m, 1, c(3, 7), EAST);
    place(&mut m, 2, c(14, 6), WEST);
    m.world.units[0].blind = 100;
    m.step();
    let target = m.world.units[2].pos;
    assert!(!m.world.can_see_point(0, target), "the shooter is blind");
    assert!(m.world.can_see_point(1, target), "its teammate is not");
    let seen = enemy(&m, 0, 2).expect("the blind unit reads the enemy its teammate sees");
    assert_eq!((seen.x, seen.y), target);

    let hp = m.world.units[2].hp;
    let aim = angle(m.world.units[0].pos, (seen.x, seen.y));
    for _ in 0..6 {
        m.act(0, A_FIRE, aim, 0, 0);
        m.step();
    }
    assert!(m.world.units[2].hp < hp, "the shot aimed from shared sight lands");
}

#[test]
fn teammates_hear_the_same_sounds() {
    let mut m = setup();
    place(&mut m, 0, c(10, 6), EAST);
    place(&mut m, 2, c(12, 6), WEST);
    let radius = m.world.rules.core.step_sound_radius as i64;
    assert!(fixed::dist2(c(10, 6), m.world.units[3].pos) > radius * radius, "unit 3 is out of earshot");
    m.act(0, A_MOVE, EAST, 10_000, 0);
    m.step();
    let near: Vec<SoundInfo> = table(&m, 2, T_SOUNDS);
    let far: Vec<SoundInfo> = table(&m, 3, T_SOUNDS);
    assert!(near.iter().any(|s| s.kind == SOUND_STEP && s.team == TEAM_A), "the footstep is heard: {near:?}");
    assert_eq!(near, far, "hearing is shared by the team");
    assert!(table::<SoundInfo>(&m, 1, T_SOUNDS).is_empty(), "a team does not hear its own steps");
}

/// Unit 0 shoots unit 2 down the open row and is moved behind the wall while the bullet flies. Returns the
/// match on the tick the bullet lands.
fn shoot_then_hide(target_hp: i32) -> Match {
    let mut m = setup();
    place(&mut m, 0, c(3, 6), EAST);
    place(&mut m, 2, c(16, 6), WEST);
    m.world.units[2].hp = target_hp;
    m.step();
    let aim = angle(m.world.units[0].pos, m.world.units[2].pos);
    assert_eq!(m.act(0, A_FIRE, aim, 0, 0), OK);
    for _ in 0..4 {
        m.step();
        if !m.world.projectiles.is_empty() {
            break;
        }
    }
    assert!(!m.world.projectiles.is_empty() && m.world.events.is_empty(), "the bullet is in flight");
    place(&mut m, 0, c(3, 2), EAST);
    place(&mut m, 2, c(16, 6), NORTH);
    for _ in 0..4 {
        m.step();
        if m.world.events.iter().any(|e| e.kind == EVENT_DAMAGE) {
            break;
        }
    }
    assert!(m.world.events.iter().any(|e| e.kind == EVENT_DAMAGE && e.target == 2), "the bullet landed");
    let spot = m.world.units[2].pos;
    assert!(!m.world.team_sees_point(TEAM_A, spot), "team A cannot see where it landed");
    m
}

#[test]
fn a_damage_event_behind_a_wall_is_not_delivered() {
    let m = shoot_then_hide(100);
    let a: Vec<EventInfo> = table(&m, 0, T_EVENTS);
    assert!(a.iter().all(|e| e.kind != EVENT_DAMAGE), "team A does not learn of the hit: {a:?}");
    let b: Vec<EventInfo> = table(&m, 3, T_EVENTS);
    let hit = b.iter().find(|e| e.kind == EVENT_DAMAGE).expect("the victim's team knows it was hit");
    assert_eq!((hit.actor, hit.target, (hit.x, hit.y)), (0, 2, m.world.units[2].pos));
    assert!(hit.value > 0, "damage keeps its amount");
}

#[test]
fn a_kill_behind_a_wall_is_delivered_without_its_position() {
    let m = shoot_then_hide(1);
    assert!(!m.world.units[2].alive);
    let a: Vec<EventInfo> = table(&m, 0, T_EVENTS);
    let kill = a.iter().find(|e| e.kind == EVENT_KILL).expect("kills always reach both teams");
    assert_eq!((kill.actor, kill.target), (0, 2));
    assert_eq!((kill.x, kill.y), (POS_HIDDEN, POS_HIDDEN), "the position is blanked, not (0, 0)");
    let b: Vec<EventInfo> = table(&m, 3, T_EVENTS);
    let kill = b.iter().find(|e| e.kind == EVENT_KILL).expect("the victim's team gets the kill");
    assert_eq!((kill.x, kill.y), m.world.units[2].pos, "its own unit's position is known");
}

#[test]
fn enemy_hp_and_armor_read_zero() {
    let mut m = setup();
    place(&mut m, 0, c(3, 6), EAST);
    place(&mut m, 2, c(12, 6), WEST);
    m.world.units[2].armor = 50;
    m.step();
    let e = enemy(&m, 0, 2).expect("in sight");
    assert_eq!((e.hp, e.armor, e.credits), (0, 0, 0));
    assert_eq!(e.weapon, m.world.units[2].weapons[m.world.units[2].slot], "the weapon in hand is visible");
    assert!(e.weapon >= 0);
    let ally = table::<UnitInfo>(&m, 3, T_ALLIES).into_iter().find(|u| u.id == 2).unwrap();
    assert_eq!((ally.hp, ally.armor), (m.world.units[2].hp, 50), "teammates see everything");
}

/// A weapon on the floor at (8, 6), east of team A, with both team A units at x = 3 looking west.
fn drop_behind_team_a(m: &mut Match) {
    place(m, 0, c(3, 6), WEST);
    place(m, 1, c(3, 7), WEST);
    m.world.drops.push(DropInfo { kind: DROP_WEAPON, x: c(8, 6).0, y: c(8, 6).1, item: 0 });
}

#[test]
fn a_turn_in_the_buy_phase_shows_in_the_next_read() {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(ARENA).unwrap(), 1);
    while m.world.phase != PHASE_BUY {
        m.step();
    }
    drop_behind_team_a(&mut m);
    assert_eq!(m.count(1, T_DROPS), 0, "team A looks away from the drop");
    // Buy-phase acts apply at once, so the rows are listed afresh at every read (`World::perceived`).
    assert_eq!(m.act(0, A_FACE, EAST, 0, 0), OK);
    assert_eq!(m.count(1, T_DROPS), 1, "unit 0 has turned, and its teammate reads what it sees");
}

/// In the action phases a tick's perceived rows are listed at its first read and kept until the step, so an
/// edit to `world` after that read would not show. Debug builds refuse it rather than serve stale rows.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "edited after a read")]
fn editing_the_world_after_a_read_of_the_tick_is_caught() {
    let mut m = setup();
    drop_behind_team_a(&mut m);
    assert_eq!(m.count(0, T_DROPS), 0);
    place(&mut m, 0, c(3, 6), EAST);
    m.count(0, T_DROPS);
}
