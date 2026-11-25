//! Time-to-kill measured in the engine, and the generated tables of `docs/rules/combat.md`.
//!
//! The doc's tables are produced here from `rules/weapons.toml` and from played-out duels, and this test fails
//! when the doc is stale. To refresh it after a balance change:
//!
//!     IBC_BLESS=1 cargo test -p ibc-engine --test combat_ttk

mod combat_common;

use combat_common::*;
use ibc_engine::abi::*;
use ibc_engine::combat::{self, damage_at, extra_spread};
use ibc_engine::fixed::{self, V2};
use ibc_engine::Ruleset;

const DISTANCES: [i32; 3] = [5, 10, 20];
/// A duel that has not ended by now is reported as no kill.
const CAP: i32 = 100;
/// Angle units per radian (65536 / 2 pi).
const UNITS_PER_RADIAN: i64 = 10430;
/// The running shooter strafes across the line to the target and turns round every this many ticks.
const STRAFE_LEG: i32 = 4;

fn strafe_angle(tick: i32) -> i32 {
    if (tick / STRAFE_LEG) % 2 == 0 {
        16384
    } else {
        49152
    }
}

/// Ticks from the first trigger pull until a stationary target `tiles` away with `hp` and `armor` is dead.
///
/// The shooter fires whenever the weapon is ready and its recoil shift (after the wind-up's recovery) is no
/// larger than the angle the target covers, and reloads when empty. `running`: it also strafes at its
/// weapon's top speed, aiming from where it will be when the bullets leave.
fn ttk(id: i32, tiles: i32, running: bool, armor: i32) -> Option<i32> {
    let mut m = start(&arena(34, 15, 1), 1);
    let target: V2 = at(4 + tiles, 7);
    place(&mut m, 0, at(4, 7), 0);
    place(&mut m, 1, target, 32768);
    arm(&mut m, 0, id);
    m.world.units[1].armor = armor;
    let wr = weapon(&m, id);
    let radius = m.world.rules.core.unit_radius as i64;
    let speed = if running { m.world.max_speed(0) } else { 0 };
    for tick in 0..CAP {
        let me = m.world.units[0].clone();
        if running {
            m.act(0, A_MOVE, strafe_angle(tick), speed, 0);
        }
        // Where the bullets will leave from: `windup` more ticks of strafing.
        let mut from = me.pos;
        for k in 0..wr.windup {
            let step = fixed::polar(strafe_angle(tick + k), speed);
            from = (from.0 + step.0, from.1 + step.1);
        }
        let aim = fixed::atan2(target.1 - from.1, target.0 - from.0);
        m.act(0, A_FACE, aim, 0, 0);
        let covers = radius * UNITS_PER_RADIAN / fixed::dist(from, target).max(1) as i64;
        let recoil = (me.combat.recoil - wr.recoil_recovery * wr.windup).max(0);
        if me.ammo[me.slot] == 0 {
            m.act(0, A_RELOAD, 0, 0, 0);
        } else if combat::can_fire(&m.world, 0) == OK
            && extra_spread(&wr, 0, recoil) as i64 / 2 <= covers
        {
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
        ("move_spread", Box::new(|w| w.move_spread.to_string())),
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
    ];
    out += &format!("| field | {} |\n", r.weapon_names.join(" | "));
    out += &format!("|---|{}\n", "---:|".repeat(r.weapons.len()));
    for (name, f) in &cols {
        let cells: Vec<String> = r.weapons.iter().map(f).collect();
        out += &format!("| `{name}` | {} |\n", cells.join(" | "));
    }
    out
}

fn derived_table(r: &Ruleset) -> String {
    let c = &r.core;
    let tiles = |v: i32| format!("{}.{}", v / TILE, (v % TILE) * 10 / TILE);
    let mut out = String::from(
        "| weapon | ticks per pull | damage per pull (close / at range) | full fan width | \
         top speed | range (tiles) | bullet (tiles/tick) | ticks to empty + reload |\n|---|---:|---:|---:|---:|---:|---:|---:|\n",
    );
    for (w, name) in r.weapons.iter().zip(&r.weapon_names) {
        let pull = w.windup + 1 + w.cooldown;
        out += &format!(
            "| {name} | {pull} | {} / {} | {} | {} | {} | {} | {} + {} |\n",
            w.damage * w.pellets,
            damage_at(w, w.range) * w.pellets,
            (w.pellets - 1) * w.spread,
            c.run_speed * w.move_pct / 100,
            tiles(w.range),
            tiles(w.bullet_speed),
            w.magazine * pull,
            w.reload_ticks,
        );
    }
    out
}

fn ttk_table(r: &Ruleset, armor: i32) -> String {
    let mut out = String::from("| weapon |");
    for d in DISTANCES {
        out += &format!(" {d} tiles standing | {d} tiles running |");
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

#[test]
fn combat_doc_tables_match_the_ruleset() {
    let r = Ruleset::builtin();
    let heavy = r
        .items
        .iter()
        .filter(|i| i.kind == ITEM_KIND_ARMOR)
        .map(|i| i.value)
        .max()
        .unwrap_or(0);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/rules/combat.md");
    let doc = std::fs::read_to_string(path)
        .expect("docs/rules/combat.md")
        .replace("\r\n", "\n");
    let mut fresh = splice(&doc, "weapons", &weapons_table(&r));
    fresh = splice(&fresh, "derived", &derived_table(&r));
    fresh = splice(&fresh, "ttk", &ttk_table(&r, 0));
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
    let id = |name: &str| weapon_id(&r, name);
    let t = |name: &str, tiles: i32, running: bool| ttk(id(name), tiles, running, 0);
    for (w, name) in r.weapons.iter().zip(&r.weapon_names) {
        // Everything kills up close, and standing still is never slower than strafing.
        assert!(
            ttk(w.id, 3, false, 0).is_some(),
            "{name} cannot kill at 3 tiles"
        );
        for d in [3, 5, 10, 20] {
            let (stand, run) = (ttk(w.id, d, false, 0), ttk(w.id, d, true, 0));
            assert!(
                stand.unwrap_or(CAP) <= run.unwrap_or(CAP),
                "{name} at {d}: standing {stand:?}, running {run:?}"
            );
            // Armour never helps the shooter.
            assert!(
                stand.unwrap_or(CAP) <= ttk(w.id, d, false, 50).unwrap_or(CAP),
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

    // Marksman: the fastest kill at 20 tiles, the worst on the move.
    for o in others("marksman") {
        assert!(
            t("marksman", 20, false).unwrap() < t(&o, 20, false).unwrap_or(CAP),
            "marksman vs {o} at 20 tiles"
        );
    }
    assert_eq!(t("marksman", 10, true), None);

    // SMG: the best primary on the move at fighting distance, out of its depth at 20 tiles.
    for o in ["rifle", "marksman", "lmg"] {
        assert!(
            t("smg", 5, true).unwrap() < t(o, 5, true).unwrap_or(CAP),
            "smg vs {o} running at 5 tiles"
        );
    }
    assert!(
        t("smg", 5, true).unwrap() <= t("smg", 5, false).unwrap() + 2,
        "the smg loses little by moving"
    );
    assert_eq!(t("smg", 20, false), None);

    // Rifle: the all-rounder. Kills at every distance standing, and beats the cheaper guns from 10 tiles out.
    for d in [3, 5, 10, 20] {
        assert!(t("rifle", d, false).is_some(), "rifle at {d}");
    }
    for o in ["pistol", "smg", "shotgun"] {
        assert!(
            t("rifle", 10, false).unwrap() < t(o, 10, false).unwrap_or(CAP),
            "rifle vs {o} at 10 tiles"
        );
    }

    // LMG: slower than the rifle to the first kill, but it does not have to stop: one magazine out-damages
    // the rifle's by a wide margin.
    assert!(t("lmg", 5, false).unwrap() > t("rifle", 5, false).unwrap());
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
