//! Bullets in flight: falloff, walls, dodging, trades, and determinism of the whole thing.

mod combat_common;

use combat_common::*;
use ibc_engine::abi::*;
use ibc_engine::combat::{damage_at, fan};
use ibc_engine::fixed::{self, V2};
use ibc_engine::{Map, Match, Ruleset};

/// Two rooms joined by a doorway, with pillars and a pair of walls that touch only at a corner.
const WALLED: &str = "name: walled\nteam_size: 2\n---\n\
####################\n\
#a.......#........d#\n\
#a.......#........d#\n\
#...#....#...#.....#\n\
#....#.......#.....#\n\
#........#.........#\n\
#..##....#....##...#\n\
#........#.......11#\n\
#........#.......11#\n\
####################\n";

fn all_weapons() -> Vec<i32> {
    (0..Ruleset::builtin().weapons.len() as i32).collect()
}

fn fly_out(m: &mut Match) {
    for _ in 0..12 {
        m.step();
    }
    assert!(m.world.projectiles.is_empty());
}

#[test]
fn damage_is_full_then_falls_linearly_to_the_floor_at_range() {
    let r = Ruleset::builtin();
    for w in &r.weapons {
        assert_eq!(damage_at(w, 0), w.damage);
        assert_eq!(damage_at(w, w.falloff_start), w.damage);
        let floor = w.damage * w.falloff_min_pct / 100;
        assert_eq!(damage_at(w, w.range), floor, "weapon {}", w.id);
        if w.range > w.falloff_start {
            let mid = (w.falloff_start + w.range) / 2;
            let pct = 100 - (100 - w.falloff_min_pct) / 2;
            assert!(
                (damage_at(w, mid) - w.damage * pct / 100).abs() <= 1,
                "weapon {} at the midpoint",
                w.id
            );
        }
        let mut last = w.damage;
        for d in (0..=w.range + 2048).step_by(256) {
            let now = damage_at(w, d);
            assert!(
                now <= last && now >= floor && now > 0,
                "weapon {} at {d}",
                w.id
            );
            last = now;
        }
    }
    let rifle = r.weapons[weapon_id(&r, "rifle") as usize];
    // 20 tiles: 4096 past falloff_start of a 14336 span, 30 points to lose: 100 - 30*4096/14336 = 92%.
    assert_eq!(damage_at(&rifle, 20 * TILE), 22 * 92 / 100);
}

#[test]
fn a_hit_deals_the_falloff_damage_for_the_distance_flown() {
    for id in all_weapons() {
        for tiles in [3, 8, 14] {
            let mut m = start(&arena(34, 9, 1), 1);
            let wr = weapon(&m, id);
            place(&mut m, 0, at(4, 4), 0);
            place(&mut m, 1, at(4 + tiles, 4), 32768);
            arm(&mut m, 0, id);
            m.world.units[1].hp = 10_000;
            m.act(0, A_FIRE, 0, 0, 0);
            fly_out(&mut m);
            let dealt = 10_000 - m.world.units[1].hp;
            let radius = m.world.rules.core.unit_radius;
            let flown = tiles * TILE - 2 * radius; // rim to rim for the bullet on the aim line
            if flown >= wr.range {
                assert_eq!(dealt, 0, "weapon {id} reached {tiles} tiles");
                continue;
            }
            let one = damage_at(&wr, flown);
            // Every pellet that hits does at most a centre hit's damage; the centre pellet(s) always land.
            assert!(
                dealt >= one.min(damage_at(&wr, flown + radius)),
                "weapon {id} at {tiles} tiles dealt {dealt}"
            );
            assert!(
                dealt <= one * wr.pellets,
                "weapon {id} at {tiles} tiles dealt {dealt}"
            );
            if wr.pellets == 1 {
                assert!(
                    (dealt - one).abs() <= 1,
                    "weapon {id} at {tiles} tiles: {dealt} vs {one}"
                );
            }
            assert_eq!(m.world.units[0].stats.damage, dealt);
        }
    }
}

#[test]
fn a_weapon_reaches_exactly_its_range_in_every_direction() {
    let map = arena(100, 100, 1);
    for id in all_weapons() {
        for angle in [0, 16384, 5000, 8192, 23456, 40000, 60001] {
            for (past, hit) in [(-150, true), (60, false)] {
                let mut m = start(&map, 1);
                let wr = weapon(&m, id);
                let radius = m.world.rules.core.unit_radius;
                // The bullet on the aim line flies from rim to rim.
                // (`polar` is a fraction of a percent off in length, so correct towards the distance wanted.)
                let want = wr.range + 2 * radius + past;
                let mut d = fixed::polar(angle, want);
                for _ in 0..3 {
                    let len = fixed::dist((0, 0), d);
                    d = fixed::polar(angle, (want as i64 * want as i64 / len as i64) as i32);
                }
                let from = at(50, 50);
                place(&mut m, 0, from, angle);
                place(&mut m, 1, (from.0 + d.0, from.1 + d.1), 0);
                arm(&mut m, 0, id);
                m.act(0, A_FIRE, angle, 0, 0);
                fly_out(&mut m);
                let dealt = m.world.rules.core.max_hp - m.world.units[1].hp;
                assert_eq!(
                    dealt > 0,
                    hit,
                    "weapon {id} at angle {angle}, {past} past its range: dealt {dealt}"
                );
            }
        }
    }
}

fn in_wall(map: &Map, p: V2) -> bool {
    map.tile(p.0.div_euclid(TILE), p.1.div_euclid(TILE)) == TILE_WALL
}

#[test]
fn no_bullet_passes_through_a_wall() {
    let mut rng = Rng(0x1BC0_C0DE);
    let weapons = all_weapons();
    let (mut flights, mut hits, mut shielded) = (0, 0, 0);
    for case in 0..600 {
        let mut m = start(WALLED, case);
        let radius = m.world.rules.core.unit_radius;
        let (w, h) = (m.world.map.width, m.world.map.height);
        // Anywhere a unit can stand, often pressed right against a wall.
        let mut spot = |m: &Match, pressed: bool| loop {
            let mut p = (rng.below(w * TILE), rng.below(h * TILE));
            if pressed {
                p.0 = p.0 / TILE * TILE + [radius, TILE - radius, TILE / 2][rng.below(3) as usize];
            }
            if !m.world.map.circle_blocked(p, radius) {
                break p;
            }
        };
        let pos = spot(&m, case % 2 == 0);
        let targets = [spot(&m, false), spot(&m, case % 3 == 0)];
        let id = weapons[rng.below(weapons.len() as i32) as usize];
        let wr = weapon(&m, id);
        // Aim at a target, near enough that fans and near misses both happen.
        let angle =
            fixed::atan2(targets[0].1 - pos.1, targets[0].0 - pos.0) + rng.below(1200) - 600;
        place(&mut m, 0, pos, angle);
        arm(&mut m, 0, id);
        m.world.units[1].pos = (-50 * TILE, -50 * TILE);
        for (i, t) in targets.iter().enumerate() {
            place(&mut m, 2 + i, *t, 0);
            m.world.units[2 + i].hp = 100_000;
        }
        // The same question asked independently of the engine's slice-by-slice sweep: along the whole ray
        // of each pellet, does the target's circle come before the first wall?
        let rays: Vec<_> = fan(&wr, angle, angle, m.world.rules.core.fov_half, 0)
            .angles()
            .map(|a| {
                let len = radius + wr.range + wr.bullet_speed;
                let d = fixed::polar(a, len);
                let end = (pos.0 + d.0, pos.1 + d.1);
                let wall = m
                    .world
                    .map
                    .segment_wall(pos, end)
                    .map_or(i32::MAX, |hit| fixed::dist(pos, hit));
                (end, len, wall)
            })
            .collect();
        m.act(0, A_FIRE, angle, 0, 0);
        for _ in 0..wr.windup + 8 {
            m.step();
            for p in &m.world.projectiles {
                flights += 1;
                assert!(
                    !in_wall(&m.world.map, p.pos),
                    "case {case}: weapon {id} bullet inside a wall at {:?}",
                    p.pos
                );
                assert!(
                    m.world.map.segment_wall(pos, p.pos).is_none(),
                    "case {case}: a wall lies behind a bullet"
                );
            }
        }
        assert!(
            m.world.projectiles.is_empty(),
            "case {case}: bullets outlive their range"
        );
        for (i, t) in targets.iter().enumerate() {
            let exposed = rays.iter().any(|&(end, len, wall)| {
                fixed::segment_circle(pos, end, *t, radius + 8)
                    .is_some_and(|q| (q * len as i64 / fixed::ONE) as i32 <= wall.saturating_add(8))
            });
            let hit = m.world.units[2 + i].hp < 100_000;
            hits += hit as i32;
            shielded += (!exposed && m.world.map.segment_wall(pos, *t).is_some()) as i32;
            assert!(
                exposed || !hit,
                "case {case}: weapon {id} from {pos:?} at {angle} hit {t:?} through a wall"
            );
        }
    }
    assert!(
        flights > 0 && hits > 100 && shielded > 100,
        "barely exercised: {flights} {hits} {shielded}"
    );
}

#[test]
fn a_target_behind_a_wall_is_never_hit() {
    // The wall column x = 9 separates the rooms on rows 1..=3; the corner-touching walls are (4,3) and (5,4).
    let cases: [(V2, V2); 4] = [
        (at(7, 2), at(11, 2)),
        // Pressed against the wall on both sides: the muzzle must not reach through.
        (
            (9 * TILE - 300, 2 * TILE + 512),
            (10 * TILE + 300, 2 * TILE + 512),
        ),
        // Through the point where two walls touch diagonally.
        (
            (5 * TILE - 600, 4 * TILE + 600),
            (5 * TILE + 600, 4 * TILE - 600),
        ),
        (
            (5 * TILE + 700, 4 * TILE - 500),
            (5 * TILE - 500, 4 * TILE + 700),
        ),
    ];
    for id in all_weapons() {
        for (ci, (from, to)) in cases.iter().enumerate() {
            let straight = fixed::atan2(to.1 - from.1, to.0 - from.0);
            for sweep in -8..=8 {
                let mut m = start(WALLED, 1);
                assert!(
                    !m.world.map.circle_blocked(*from, 300)
                        && !m.world.map.circle_blocked(*to, 300)
                );
                let angle = straight + sweep * if ci == 0 { 300 } else { 60 };
                place(&mut m, 0, *from, angle);
                place(&mut m, 2, *to, 0);
                arm(&mut m, 0, id);
                m.act(0, A_FIRE, angle, 0, 0);
                fly_out(&mut m);
                assert_eq!(shots(&m, 0), 1);
                let hp = m.world.units[2].hp;
                assert_eq!(
                    hp, m.world.rules.core.max_hp,
                    "weapon {id}, case {ci}, sweep {sweep}"
                );
            }
        }
    }
}

/// Shooter at the left, target `tiles` to its right. The shooter fires once at `aim`; the target runs straight
/// down from the tick it could first see the wind-up if `strafe`. Returns the damage taken.
fn single_shot(id: i32, tiles: i32, strafe: bool, aim: impl Fn(&Match) -> i32) -> i32 {
    let mut m = start(&arena(34, 25, 1), 1);
    place(&mut m, 0, at(4, 6), 0);
    place(&mut m, 1, at(4 + tiles, 6), 32768);
    arm(&mut m, 0, id);
    let run = m.world.rules.core.run_speed;
    let a = aim(&m);
    m.act(0, A_FIRE, a, 0, 0);
    m.step();
    for _ in 0..12 {
        if strafe {
            assert!(m.world.units[1].alive);
            m.act(1, A_MOVE, 16384, run, 0);
        }
        m.step();
    }
    m.world.rules.core.max_hp - m.world.units[1].hp
}

#[test]
fn a_strafing_target_at_range_dodges_a_shot_aimed_where_it_stood() {
    let r = Ruleset::builtin();
    let singles: Vec<&WeaponRules> = r.weapons.iter().filter(|w| w.pellets == 1).collect();
    assert!(singles.len() >= 2);
    for w in singles {
        for tiles in [10, 15, 19] {
            assert!(tiles * TILE < w.range);
            let at_it = |_: &Match| 0;
            assert!(
                single_shot(w.id, tiles, false, at_it) > 0,
                "weapon {} hits a standing target at {tiles}",
                w.id
            );
            assert_eq!(
                single_shot(w.id, tiles, true, at_it),
                0,
                "weapon {} at {tiles} tiles was not dodged",
                w.id
            );
            // Nothing is random, so a shooter who knows where the target will be still hits: the target moves
            // for `windup - 1` whole ticks before the release and then for the flight time.
            let lead = |m: &Match| {
                let (me, foe) = (m.world.units[0].pos, m.world.units[1].pos);
                let run = m.world.rules.core.run_speed as i64;
                let radius = m.world.rules.core.unit_radius;
                let mut p = foe;
                for _ in 0..4 {
                    let fly = (fixed::dist(me, p) - 2 * radius).max(0) as i64;
                    let moved = run * (w.windup as i64 - 1) + run * fly / w.bullet_speed as i64;
                    p = (foe.0, foe.1 + moved as i32);
                }
                fixed::atan2(p.1 - me.1, p.0 - me.0)
            };
            assert!(
                single_shot(w.id, tiles, true, lead) > 0,
                "weapon {} at {tiles} tiles: a led shot lands",
                w.id
            );
        }
    }
}

#[test]
fn simultaneous_lethal_shots_trade() {
    let mut m = start(&arena(20, 9, 1), 1);
    place(&mut m, 0, at(4, 4), 0);
    place(&mut m, 1, at(10, 4), 32768);
    for u in 0..2 {
        arm(&mut m, u, 0);
        m.world.units[u].hp = 5;
    }
    m.act(0, A_FIRE, 0, 0, 0);
    m.act(1, A_FIRE, 32768, 0, 0);
    for _ in 0..3 {
        m.step();
    }
    assert!(
        !m.world.units[0].alive && !m.world.units[1].alive,
        "both bullets were already in the air"
    );
    assert_eq!(
        (m.world.units[0].stats.kills, m.world.units[1].stats.kills),
        (1, 1)
    );
}

/// Random but seeded play that leans on every combat action. Returns per-tick state hashes and
/// (kills, most weapons seen on the ground at once).
fn chaos(seed: u64) -> (Vec<u64>, (i32, usize)) {
    let mut m = Match::new(Ruleset::builtin(), Map::parse(WALLED).unwrap(), seed);
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let rules = Ruleset::builtin();
    let n_weapons = rules.weapons.len() as i32;
    let mut hashes = Vec::new();
    let mut most_drops = 0;
    let mut armed_round = 0;
    let mut paid = (0, 0); // (round, weapons other than the default sidearm, in hands or on the ground)
    for _ in 0..400 {
        if m.finished() {
            break;
        }
        let acting = m.world.phase == PHASE_ACTION || m.world.phase == PHASE_PLANTED;
        if acting && armed_round != m.world.round {
            armed_round = m.world.round;
            for u in 0..4 {
                let id = rng.below(n_weapons);
                arm(&mut m, u, id);
                // Start the fight close together so that bullets, deaths and drops all happen.
                let p = [at(2, 4), at(3, 5), at(6, 4), at(7, 5)][u];
                place(&mut m, u, p, rng.below(65536));
            }
        }
        for unit in m.turn_order() {
            let me = m.world.units[unit as usize].clone();
            let foe = m
                .world
                .units
                .iter()
                .find(|o| o.team != me.team && o.alive)
                .map(|o| o.pos)
                .unwrap_or(me.pos);
            let aim = fixed::atan2(foe.1 - me.pos.1, foe.0 - me.pos.0) + rng.below(2000) - 1000;
            // A move cancels a startup, so move only now and then or nobody ever shoots.
            if rng.below(5) == 0 {
                m.act(unit, A_MOVE, rng.below(65536), rng.below(900), 0);
            }
            if rng.below(4) == 0 {
                m.act(unit, A_FACE, aim, 0, 0);
            }
            match rng.below(16) {
                0..=8 => m.act(unit, A_FIRE, aim, 0, 0),
                9 | 10 => m.act(unit, A_RELOAD, 0, 0, 0),
                11 => m.act(unit, A_SWAP, rng.below(3), 0, 0),
                12 => m.act(unit, A_DROP, DROP_WEAPON, 0, 0),
                13 | 14 => m.act(unit, A_PICKUP, 0, 0, 0),
                _ => m.act(unit, A_CANCEL, 0, 0, 0),
            };
        }
        m.take_inputs();
        m.step();
        most_drops = most_drops.max(weapon_drops(&m).len());
        hashes.push(m.state_hash());

        let w = &m.world;
        let mut count = w
            .drops
            .iter()
            .filter(|d| d.kind == DROP_WEAPON && d.item != rules.econ.default_sidearm)
            .count();
        for u in &w.units {
            for s in 0..2 {
                let id = u.weapons[s];
                assert!(id >= -1 && id < n_weapons);
                if id < 0 {
                    assert_eq!(u.ammo[s], 0, "an empty slot holds no ammo");
                    continue;
                }
                let wr = rules.weapons[id as usize];
                assert_eq!(wr.slot as usize, s, "weapon {id} sits in its own slot");
                assert!(
                    (0..=wr.magazine).contains(&u.ammo[s]),
                    "ammo {} of weapon {id}",
                    u.ammo[s]
                );
                count += (id != rules.econ.default_sidearm) as usize;
            }
            let c = &u.combat;
            assert!(c.recoil >= 0 && c.cooldown >= 0 && c.reload >= 0 && c.windup >= 0);
            assert!(
                !(c.winding && c.reload > 0),
                "winding up and reloading at once"
            );
            assert!(u.alive || !c.winding, "the dead do not wind up");
        }
        for p in &w.projectiles {
            assert!(!in_wall(&w.map, p.pos));
        }
        if (w.phase == PHASE_ACTION || w.phase == PHASE_PLANTED) && armed_round == w.round {
            if paid.0 == w.round {
                assert_eq!(
                    count, paid.1,
                    "tick {}: weapons were created or destroyed",
                    w.tick
                );
            }
            paid = (w.round, count);
        }
    }
    let kills = m.world.units.iter().map(|u| u.stats.kills).sum();
    (hashes, (kills, most_drops))
}

#[test]
fn gunplay_is_deterministic() {
    for seed in [1, 2, 3, 4] {
        let (a, stats) = chaos(seed);
        let (b, _) = chaos(seed);
        assert_eq!(a, b, "seed {seed}: two runs differ");
        assert!(a.len() > 100);
        assert!(
            stats.0 > 0 && stats.1 > 0,
            "seed {seed}: the script must exercise kills and drops: {stats:?}"
        );
    }
    assert_ne!(
        chaos(1).0,
        chaos(2).0,
        "different seeds give different games"
    );
}
