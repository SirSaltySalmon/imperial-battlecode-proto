//! Time-to-kill measured in the engine, and the generated tables of `docs/rules/combat.md`.
//!
//! The doc's tables are produced here from `rules/weapons.toml` and from played-out duels, and this test fails
//! when the doc is stale. To refresh it after a balance change:
//!
//!     IBC_BLESS=1 cargo test -p ibc-engine --test combat_ttk

mod combat_common;

use combat_common::*;
use ibc_engine::abi::*;
use ibc_engine::combat::{self, damage_at, startup_ticks};
use ibc_engine::fixed::{self, V2};
use ibc_engine::Ruleset;

/// Point blank (shotgun), fighting distance, mid range, rifle against marksman, long range.
/// `crates/combatviz/src/ttk.rs` plays the same duels and checks it keeps the same list.
const DISTANCES: [i32; 5] = [2, 5, 10, 15, 20];
/// A duel that has not ended by now is reported as no kill.
const CAP: i32 = 100;
/// Angle units per radian (65536 / 2 pi).
const UNITS_PER_RADIAN: i64 = 10430;
/// Before its first pull the running shooter strafes across the line to the target for this many ticks,
/// turning round half way, so it ends where it started.
const RUN_IN: i32 = 8;

fn strafe_angle(tick: i32) -> i32 {
    if tick < RUN_IN / 2 {
        16384
    } else {
        49152
    }
}

/// Ticks from the first trigger pull until a stationary target `tiles` away with `hp` and `armor` is dead.
///
/// The shooter stands, pulls whenever the weapon is ready and the pellets nearest the aim would still land
/// (an even fan straddles the aim by half its widened step; an odd fan always has one on it), and reloads
/// when empty. Pulling on every ready tick keeps it braced; holding off lets recoil settle but costs a fresh
/// startup.
/// `running`: it strafes at its weapon's top speed for `RUN_IN` ticks just before the first pull, so that
/// pull starts up unsteady.
fn ttk(id: i32, tiles: i32, running: bool, armor: i32) -> Option<i32> {
    let mut m = start(&arena(34, 15, 1), 1);
    let target: V2 = at(4 + tiles, 7);
    place(&mut m, 0, at(4, 7), 0);
    place(&mut m, 1, target, 32768);
    arm(&mut m, 0, id);
    m.world.units[1].armor = armor;
    if running {
        let speed = m.world.max_speed(0);
        for tick in 0..RUN_IN {
            m.act(0, A_MOVE, strafe_angle(tick), speed, 0);
            m.step();
        }
    }
    let wr = weapon(&m, id);
    let (radius, fov) = (
        m.world.rules.core.unit_radius as i64,
        m.world.rules.core.fov_half,
    );
    for tick in 0..CAP {
        let me = m.world.units[0].clone();
        let aim = fixed::atan2(target.1 - me.pos.1, target.0 - me.pos.0);
        m.act(0, A_FACE, aim, 0, 0);
        let covers = radius * UNITS_PER_RADIAN / fixed::dist(me.pos, target).max(1) as i64;
        // Heat when the bullets leave: braced at once, otherwise after the startup's recovery.
        let c = me.combat;
        let wait = if c.braced {
            0
        } else {
            startup_ticks(&wr, c.unsteady)
        };
        let heat = (c.recoil - wr.recoil_recovery * wait).max(0);
        let inner = if wr.pellets % 2 == 0 {
            combat::fan(&wr, 0, 0, fov, heat).step / 2
        } else {
            0
        };
        if me.ammo[me.slot] == 0 {
            m.act(0, A_RELOAD, 0, 0, 0);
        } else if combat::can_fire(&m.world, 0) == OK && inner as i64 <= covers {
            m.act(0, A_FIRE, aim, 0, 0);
        }
        m.step();
        if !m.world.units[1].alive {
            return Some(tick + 1);
        }
    }
    None
}

fn cell(t: Option<i32>) -> String {
    t.map_or("no kill".to_string(), |t| t.to_string())
}

fn weapons_table(r: &Ruleset) -> String {
    let mut out = String::new();
    let cols: Vec<(&str, Box<dyn Fn(&WeaponRules) -> String>)> = vec![
        ("id", Box::new(|w| w.id.to_string())),
        (
            "slot",
            Box::new(|w| ["primary", "sidearm"][w.slot as usize].to_string()),
        ),
        ("price", Box::new(|w| w.price.to_string())),
        ("damage", Box::new(|w| w.damage.to_string())),
        ("pellets", Box::new(|w| w.pellets.to_string())),
        ("spread", Box::new(|w| w.spread.to_string())),
        ("recoil_spread", Box::new(|w| w.recoil_spread.to_string())),
        (
            "recoil_per_shot",
            Box::new(|w| w.recoil_per_shot.to_string()),
        ),
        (
            "recoil_recovery",
            Box::new(|w| w.recoil_recovery.to_string()),
        ),
        ("windup", Box::new(|w| w.windup.to_string())),
        ("cooldown", Box::new(|w| w.cooldown.to_string())),
        ("magazine", Box::new(|w| w.magazine.to_string())),
        ("reload_ticks", Box::new(|w| w.reload_ticks.to_string())),
        ("bullet_speed", Box::new(|w| w.bullet_speed.to_string())),
        ("range", Box::new(|w| w.range.to_string())),
        ("falloff_start", Box::new(|w| w.falloff_start.to_string())),
        (
            "falloff_min_pct",
            Box::new(|w| w.falloff_min_pct.to_string()),
        ),
        ("move_pct", Box::new(|w| w.move_pct.to_string())),
        ("kill_reward", Box::new(|w| w.kill_reward.to_string())),
        ("unsteady_gain", Box::new(|w| w.unsteady_gain.to_string())),
        ("unsteady_decay", Box::new(|w| w.unsteady_decay.to_string())),
        ("unsteady_max", Box::new(|w| w.unsteady_max.to_string())),
        (
            "startup_per_point",
            Box::new(|w| w.startup_per_point.to_string()),
        ),
    ];
    out += &format!("| field | {} |\n", r.weapon_names.join(" | "));
    out += &format!("|---|{}\n", "---:|".repeat(r.weapons.len()));
    for (name, f) in &cols {
        let cells: Vec<String> = r.weapons.iter().map(f).collect();
        out += &format!("| `{name}` | {} |\n", cells.join(" | "));
    }
    out
}

/// Unsteadiness of a unit that has moved at `speed` long enough to settle (or reach the cap).
fn settled_heat(w: &WeaponRules, speed: i32) -> i32 {
    (0..64).fold(0, |h, _| combat::unsteadiness_after(w, h, speed))
}

fn derived_table(r: &Ruleset) -> String {
    let c = &r.core;
    let tiles = |v: i32| format!("{}.{}", v / TILE, (v % TILE) * 10 / TILE);
    let mut out = String::from(
        "| weapon | startup (steady / walked / ran) | ticks per braced shot | damage per pull (close / at range) | \
         full fan width | top speed | range (tiles) | bullet (tiles/tick) | ticks to empty + reload |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for (w, name) in r.weapons.iter().zip(&r.weapon_names) {
        let top = c.run_speed * w.move_pct / 100;
        let shot = 1 + w.cooldown;
        out += &format!(
            "| {name} | {} / {} / {} | {shot} | {} / {} | {} | {top} | {} | {} | {} + {} |\n",
            startup_ticks(w, 0),
            startup_ticks(w, settled_heat(w, c.walk_speed.min(top))),
            startup_ticks(w, settled_heat(w, top)),
            w.damage * w.pellets,
            damage_at(w, w.range) * w.pellets,
            (w.pellets - 1) * w.spread,
            tiles(w.range),
            tiles(w.bullet_speed),
            w.windup + 1 + (w.magazine - 1) * shot,
            w.reload_ticks,
        );
    }
    out
}

fn ttk_table(r: &Ruleset, armor: i32) -> String {
    let mut out = String::from("| weapon |");
    for d in DISTANCES {
        out += &format!(" {d} tiles standing | {d} tiles after running |");
    }
    out += &format!("\n|---|{}\n", "---:|".repeat(2 * DISTANCES.len()));
    for (w, name) in r.weapons.iter().zip(&r.weapon_names) {
        out += &format!("| {name} |");
        for d in DISTANCES {
            out += &format!(
                " {} | {} |",
                cell(ttk(w.id, d, false, armor)),
                cell(ttk(w.id, d, true, armor))
            );
        }
        out += "\n";
    }
    out
}

/// Replace the text between `<!-- generated:NAME -->` and `<!-- /generated:NAME -->`.
fn splice(doc: &str, name: &str, body: &str) -> String {
    let (open, close) = (
        format!("<!-- generated:{name} -->"),
        format!("<!-- /generated:{name} -->"),
    );
    let a = doc
        .find(&open)
        .unwrap_or_else(|| panic!("docs/rules/combat.md lacks {open}"))
        + open.len();
    let b = doc
        .find(&close)
        .unwrap_or_else(|| panic!("docs/rules/combat.md lacks {close}"));
    format!("{}\n{}{}", &doc[..a], body, &doc[b..])
}

/// The smallest and the largest armour value the shop sells (0 when it sells none).
fn armor_values(r: &Ruleset) -> (i32, i32) {
    let values = || {
        r.items
            .iter()
            .filter(|i| i.kind == ITEM_KIND_ARMOR)
            .map(|i| i.value)
    };
    (values().min().unwrap_or(0), values().max().unwrap_or(0))
}

#[test]
fn combat_doc_tables_match_the_ruleset() {
    let r = Ruleset::builtin();
    let (light, heavy) = armor_values(&r);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/rules/combat.md");
    let doc = std::fs::read_to_string(path)
        .expect("docs/rules/combat.md")
        .replace("\r\n", "\n");
    let mut fresh = splice(&doc, "weapons", &weapons_table(&r));
    fresh = splice(&fresh, "derived", &derived_table(&r));
    fresh = splice(&fresh, "ttk", &ttk_table(&r, 0));
    fresh = splice(&fresh, "ttk-light", &ttk_table(&r, light));
    fresh = splice(&fresh, "ttk-armor", &ttk_table(&r, heavy));
    if std::env::var_os("IBC_BLESS").is_some() {
        std::fs::write(path, &fresh).unwrap();
    } else {
        assert!(fresh == doc, "docs/rules/combat.md is stale: run `IBC_BLESS=1 cargo test -p ibc-engine --test combat_ttk`");
    }
}

#[test]
fn time_to_kill_gives_each_weapon_its_role() {
    let r = Ruleset::builtin();
    let (light, heavy) = armor_values(&r);
    let id = |name: &str| weapon_id(&r, name);
    let t = |name: &str, tiles: i32, running: bool| ttk(id(name), tiles, running, 0);
    for (w, name) in r.weapons.iter().zip(&r.weapon_names) {
        // Everything kills up close, and a steady shooter is never slower than one that just ran.
        assert!(
            ttk(w.id, 3, false, 0).is_some(),
            "{name} cannot kill at 3 tiles"
        );
        for d in std::iter::once(3).chain(DISTANCES) {
            let (stand, run) = (ttk(w.id, d, false, 0), ttk(w.id, d, true, 0));
            assert!(
                stand.unwrap_or(CAP) <= run.unwrap_or(CAP),
                "{name} at {d}: standing {stand:?}, running {run:?}"
            );
            // Armour never helps the shooter, and more of it never helps less.
            let (bare, lit) = (
                stand.unwrap_or(CAP),
                ttk(w.id, d, false, light).unwrap_or(CAP),
            );
            assert!(
                bare <= lit && lit <= ttk(w.id, d, false, heavy).unwrap_or(CAP),
                "{name} at {d} against armour"
            );
        }
        // The fastest possible kill is one pull.
        assert!(ttk(w.id, 3, false, 0).unwrap() >= w.windup + 1);
    }
    let others = |name: &str| {
        r.weapon_names
            .iter()
            .filter(move |n| *n != name)
            .cloned()
            .collect::<Vec<_>>()
    };

    // Shotgun: nothing kills faster point blank; nothing at all at long range.
    for o in others("shotgun") {
        assert!(
            t("shotgun", 2, false) <= t(&o, 2, false),
            "shotgun vs {o} at 2 tiles"
        );
    }
    assert_eq!(
        t("shotgun", 2, false),
        Some(r.weapons[id("shotgun") as usize].windup + 1),
        "one pull"
    );
    assert_eq!(t("shotgun", 20, false), None);

    // Marksman: the fastest kill at 20 tiles, and loses the most to a run.
    for o in others("marksman") {
        assert!(
            t("marksman", 20, false).unwrap() < t(&o, 20, false).unwrap_or(CAP),
            "marksman vs {o} at 20 tiles"
        );
    }
    let penalty = |name: &str| t(name, 10, true).unwrap_or(CAP) - t(name, 10, false).unwrap_or(CAP);
    for o in others("marksman") {
        assert!(
            penalty("marksman") >= penalty(&o),
            "marksman vs {o}: run penalty"
        );
    }
    assert!(penalty("marksman") > 0);

    // SMG: the best primary straight off a run at fighting distance, out of its depth at 20 tiles.
    for o in ["rifle", "marksman", "lmg"] {
        assert!(
            t("smg", 5, true).unwrap() < t(o, 5, true).unwrap_or(CAP),
            "smg vs {o} after running at 5 tiles"
        );
    }
    assert!(
        t("smg", 5, true).unwrap() <= t("smg", 5, false).unwrap() + 2,
        "the smg loses little by moving"
    );
    assert_eq!(t("smg", 20, false), None);

    // Rifle: the all-rounder. Kills at every distance standing, and beats the cheaper guns from 10 tiles out.
    for d in std::iter::once(3).chain(DISTANCES) {
        assert!(t("rifle", d, false).is_some(), "rifle at {d}");
    }
    for o in ["pistol", "smg", "shotgun"] {
        assert!(
            t("rifle", 10, false).unwrap() < t(o, 10, false).unwrap_or(CAP),
            "rifle vs {o} at 10 tiles"
        );
    }
    // At 15 tiles the rifle meets the marksman: only the marksman kills faster there.
    for o in ["pistol", "smg", "shotgun", "lmg"] {
        assert!(
            t("rifle", 15, false).unwrap() < t(o, 15, false).unwrap_or(CAP),
            "rifle vs {o} at 15 tiles"
        );
    }
    assert!(t("marksman", 15, false).unwrap() < t("rifle", 15, false).unwrap());

    // LMG: no faster than the rifle to the first kill, but it does not have to stop: one magazine
    // out-damages the rifle's by a wide margin.
    assert!(t("lmg", 5, false).unwrap() >= t("rifle", 5, false).unwrap());
    let (lmg, rifle) = (
        r.weapons[id("lmg") as usize],
        r.weapons[id("rifle") as usize],
    );
    assert!(
        lmg.magazine * lmg.pellets * lmg.damage
            >= 3 * rifle.magazine * rifle.pellets * rifle.damage
    );
    assert!(lmg.move_pct < rifle.move_pct && lmg.reload_ticks > rifle.reload_ticks);

    // The free sidearm is the slowest standing kill at 5 tiles: every purchase is an upgrade somewhere.
    for o in others("pistol") {
        assert!(
            t(&o, 5, false).unwrap() <= t("pistol", 5, false).unwrap(),
            "{o} vs pistol at 5 tiles"
        );
    }
}
