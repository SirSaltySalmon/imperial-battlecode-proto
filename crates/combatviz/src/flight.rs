//! Flight, falloff, walls, dodging a shot, and a simultaneous trade. Scripts follow `combat_ballistics`.

use crate::record::Catalog;
use crate::setup::{self, arena, weapon_id};
use crate::take::{self, Take};
use ibc_engine::abi::*;
use ibc_engine::fixed::{self, V2};
use ibc_engine::Match;

pub fn add(cat: &mut Catalog) {
    let rifle = weapon_id("rifle");
    let shotgun = weapon_id("shotgun");
    let marksman = weapon_id("marksman");
    let pistol = weapon_id("pistol");

    falloff(cat, rifle, "rifle", 3);
    falloff(cat, rifle, "rifle", 14);
    falloff(cat, shotgun, "shotgun", 3);
    falloff(cat, shotgun, "shotgun", 8);
    reach(
        cat,
        marksman,
        -150,
        "marksman-inside",
        "Marksman, just inside its range",
    );
    reach(
        cat,
        marksman,
        60,
        "marksman-past",
        "Marksman, just past its range",
    );
    walls(cat, rifle);
    for name in ["pistol", "marksman"] {
        let id = weapon_id(name);
        dodge(cat, id, name, 15, "stand");
        dodge(cat, id, name, 15, "strafe");
        dodge(cat, id, name, 15, "lead");
    }
    trade(cat, pistol);
}

fn falloff(cat: &mut Catalog, id: i32, name: &str, tiles: i32) {
    let mut m = setup::start(&arena(34, 9, 1), 1);
    setup::place(&mut m, 0, setup::at(4, 4), 0);
    setup::place(&mut m, 1, setup::at(4 + tiles, 4), 32768);
    setup::arm(&mut m, 0, id);
    m.world.units[1].hp = 10_000;
    let mut t = Take::begin(m);
    t.act(0, A_FIRE, 0, 0);
    fly(&mut t, 12);
    let dealt = 10_000 - t.m.world.units[1].hp;
    t.finish(
        cat,
        take::meta(
            "flight",
            &format!("falloff-{name}-{tiles}"),
            &format!("{name} at {tiles} tiles"),
            "combat_ballistics::a_hit_deals_the_falloff_damage_for_the_distance_flown. One pull at a stationary target. Damage is full up to falloff_start, then linear down to falloff_min_pct at range.",
            &format!("dealt {dealt}"),
        ),
    );
}

fn reach(cat: &mut Catalog, id: i32, past: i32, sid: &str, title: &str) {
    let mut m = setup::start(&arena(100, 100, 1), 1);
    let wr = m.world.rules.weapons[id as usize];
    let radius = m.world.rules.core.unit_radius;
    let want = wr.range + 2 * radius + past;
    let d = corrected(0, want);
    let from = setup::at(50, 50);
    setup::place(&mut m, 0, from, 0);
    setup::place(&mut m, 1, (from.0 + d.0, from.1 + d.1), 0);
    setup::arm(&mut m, 0, id);
    let full = m.world.rules.core.max_hp;
    let mut t = Take::begin(m);
    t.act(0, A_FIRE, 0, 0);
    fly(&mut t, 12);
    let dealt = full - t.m.world.units[1].hp.max(0);
    let which = if past < 0 { "inside" } else { "past" };
    t.finish(
        cat,
        take::meta(
            "flight",
            sid,
            title,
            "combat_ballistics::a_weapon_reaches_exactly_its_range_in_every_direction. The bullet on the aim line flies from rim to rim. A target 150 units inside range is hit; one 60 units past it is not.",
            &format!("dealt {dealt} ({which} range)"),
        ),
    );
}

fn walls(cat: &mut Catalog, rifle: i32) {
    let cases: [(&str, &str, V2, V2); 4] = [
        (
            "wall-column",
            "Through the wall column",
            setup::at(7, 2),
            setup::at(11, 2),
        ),
        (
            "wall-pressed",
            "Pressed against the wall",
            (9 * TILE - 300, 2 * TILE + 512),
            (10 * TILE + 300, 2 * TILE + 512),
        ),
        (
            "wall-corner-a",
            "Across a diagonal corner",
            (5 * TILE - 600, 4 * TILE + 600),
            (5 * TILE + 600, 4 * TILE - 600),
        ),
        (
            "wall-corner-b",
            "Across the same corner, the other way",
            (5 * TILE + 700, 4 * TILE - 500),
            (5 * TILE - 500, 4 * TILE + 700),
        ),
    ];
    open_lane(cat, rifle);
    let note = "combat_ballistics::a_target_behind_a_wall_is_never_hit. The rifle is aimed straight at the target. A wall, or two walls that only touch at a corner, stops every pellet. The 600 random shots in no_bullet_passes_through_a_wall use this same map.";
    for (id, title, from, to) in cases {
        aimed(
            cat,
            rifle,
            Lane {
                map: setup::WALLED,
                from,
                to,
                target: 2,
                id,
                title,
                note,
            },
        );
    }
}

fn open_lane(cat: &mut Catalog, rifle: i32) {
    aimed(
        cat,
        rifle,
        Lane {
            map: setup::WALLED,
            from: setup::at(3, 2),
            to: setup::at(7, 2),
            target: 2,
            id: "open-lane",
            title: "Open lane on the same map",
            note: "The control next to the wall cases: same room, no wall between the rifle and the target, so the shot lands.",
        },
    );
}

struct Lane<'a> {
    map: &'a str,
    from: V2,
    to: V2,
    target: usize,
    id: &'a str,
    title: &'a str,
    note: &'a str,
}

fn aimed(cat: &mut Catalog, id: i32, lane: Lane) {
    let mut m = setup::start(lane.map, 1);
    if m.world.units.len() > 3 {
        setup::place(&mut m, 1, setup::at(2, 7), 0);
        setup::place(&mut m, 3, setup::at(17, 7), 0);
    }
    let angle = fixed::atan2(lane.to.1 - lane.from.1, lane.to.0 - lane.from.0);
    setup::place(&mut m, 0, lane.from, angle);
    setup::place(&mut m, lane.target, lane.to, 0);
    setup::arm(&mut m, 0, id);
    let full = m.world.units[lane.target].hp;
    let mut t = Take::begin(m);
    t.act(0, A_FIRE, angle, 0);
    fly(&mut t, 12);
    let dealt = full - t.m.world.units[lane.target].hp.max(0);
    t.finish(
        cat,
        take::meta(
            "flight",
            lane.id,
            lane.title,
            lane.note,
            &format!("dealt {dealt}"),
        ),
    );
}

fn dodge(cat: &mut Catalog, id: i32, name: &str, tiles: i32, kind: &str) {
    let mut m = setup::start(&arena(34, 25, 1), 1);
    setup::place(&mut m, 0, setup::at(4, 6), 0);
    setup::place(&mut m, 1, setup::at(4 + tiles, 6), 32768);
    setup::arm(&mut m, 0, id);
    let wr = m.world.rules.weapons[id as usize];
    let aim = if kind == "lead" {
        lead_aim(&m, wr.windup, wr.bullet_speed)
    } else {
        0
    };
    let mut t = Take::begin(m);
    t.act(0, A_FIRE, aim, 0);
    t.step();
    let run = t.m.world.rules.core.run_speed;
    for _ in 0..12 {
        if kind != "stand" {
            t.act(1, A_MOVE, 16384, run);
        }
        t.step();
    }
    let dealt = t.m.world.rules.core.max_hp - t.m.world.units[1].hp.max(0);
    let (title, note) = dodge_copy(name, tiles, kind);
    t.finish(
        cat,
        take::meta(
            "flight",
            &format!("dodge-{name}-{tiles}-{kind}"),
            &title,
            note,
            &format!("dealt {dealt}"),
        ),
    );
}

fn dodge_copy(name: &str, tiles: i32, kind: &str) -> (String, &'static str) {
    let title = match kind {
        "stand" => format!("{name} at {tiles} tiles, target stands"),
        "strafe" => format!("{name} at {tiles} tiles, aimed where it stood"),
        _ => format!("{name} at {tiles} tiles, aimed where it will be"),
    };
    let note = match kind {
        "strafe" => "combat_ballistics::a_strafing_target_at_range_dodges_a_shot_aimed_where_it_stood. The target starts moving the tick after the wind-up is visible. A single pellet aimed at the old spot arrives late.",
        "lead" => "The same strafe, but the shooter aims at where the target will be: wind-up ticks of movement, then the flight time. Nothing is random, so the led shot lands.",
        _ => "The same shot with the target standing still, so the pellet hits.",
    };
    (title, note)
}

fn lead_aim(m: &Match, windup: i32, bullet_speed: i32) -> i32 {
    let (me, foe) = (m.world.units[0].pos, m.world.units[1].pos);
    let run = i64::from(m.world.rules.core.run_speed);
    let radius = m.world.rules.core.unit_radius;
    let mut p = foe;
    for _ in 0..4 {
        let fly = i64::from((fixed::dist(me, p) - 2 * radius).max(0));
        let moved = run * (i64::from(windup) - 1) + run * fly / i64::from(bullet_speed);
        p = (foe.0, foe.1 + moved as i32);
    }
    fixed::atan2(p.1 - me.1, p.0 - me.0)
}

fn trade(cat: &mut Catalog, pistol: i32) {
    let mut m = setup::start(&arena(20, 9, 1), 1);
    setup::place(&mut m, 0, setup::at(4, 4), 0);
    setup::place(&mut m, 1, setup::at(10, 4), 32768);
    for u in 0..2 {
        setup::arm(&mut m, u, pistol);
        m.world.units[u].hp = 5;
    }
    let mut t = Take::begin(m);
    t.act(0, A_FIRE, 0, 0);
    t.act(1, A_FIRE, 32768, 0);
    fly(&mut t, 3);
    let both = !t.m.world.units[0].alive && !t.m.world.units[1].alive;
    let outcome = if both { "both dead" } else { "still standing" };
    t.finish(
        cat,
        take::meta(
            "flight",
            "trade",
            "Simultaneous lethal shots",
            "combat_ballistics::simultaneous_lethal_shots_trade. Both pistols are in the air in the same slice, so each kill is credited. Hits in one slice are applied together.",
            outcome,
        ),
    );
}

fn fly(t: &mut Take, n: i32) {
    for _ in 0..n {
        t.step();
    }
}

fn corrected(angle: i32, want: i32) -> V2 {
    let mut d = fixed::polar(angle, want);
    for _ in 0..3 {
        let len = fixed::dist((0, 0), d).max(1);
        d = fixed::polar(
            angle,
            (i64::from(want) * i64::from(want) / i64::from(len)) as i32,
        );
    }
    d
}
