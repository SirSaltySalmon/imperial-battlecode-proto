use ibc_sim::{make_bot, run, run_custom, SimResult, BOTS, DEFAULT_MAX_TICKS};

/// Bots may hit the odd legitimate error (a race for a pickup, a weapon cooldown); more than this
/// share of failing `act` calls means a bot is spamming something it should have checked first.
const MAX_ACT_ERROR_PCT: u64 = 5;

fn maps() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../maps");
    let mut maps: Vec<(String, String)> = std::fs::read_dir(dir)
        .expect("maps directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ibcmap"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap()))
        .collect();
    maps.sort();
    assert!(!maps.is_empty(), "no maps in {dir}");
    maps
}

fn play(map: &str, seed: u64, a: &str, b: &str) -> SimResult {
    let r = run(map, seed, a, b, DEFAULT_MAX_TICKS).expect("match runs");
    assert!(r.finished, "{a} vs {b} did not finish in {} ticks", r.ticks);
    r
}

#[test]
fn same_inputs_same_hash() {
    for (name, map) in maps() {
        for (a, b) in [("rusher", "holder"), ("holder", "rusher"), ("rusher", "rusher")] {
            let (x, y) = (play(&map, 7, a, b), play(&map, 7, a, b));
            assert_eq!(x.final_hash, y.final_hash, "{a} vs {b} on {name}");
            assert_eq!((x.winner, x.score, x.ticks), (y.winner, y.score, y.ticks), "{a} vs {b} on {name}");
            assert_eq!(x.summary_json, y.summary_json, "{a} vs {b} on {name}");
        }
    }
}

#[test]
fn rusher_beats_idle_on_every_map() {
    for (name, map) in maps() {
        for seed in [1, 2] {
            let r = play(&map, seed, "rusher", "idle");
            assert_eq!(r.winner, Some(0), "rusher as team A on {name} seed {seed}: score {:?}", r.score);
            let r = play(&map, seed, "idle", "rusher");
            assert_eq!(r.winner, Some(1), "rusher as team B on {name} seed {seed}: score {:?}", r.score);
        }
    }
}

#[test]
fn holder_beats_idle_on_every_map() {
    for (name, map) in maps() {
        let r = play(&map, 1, "holder", "idle");
        assert_eq!(r.winner, Some(0), "holder as team A on {name}: score {:?}", r.score);
        let r = play(&map, 1, "idle", "holder");
        assert_eq!(r.winner, Some(1), "holder as team B on {name}: score {:?}", r.score);
    }
}

#[test]
fn bots_rarely_get_act_errors() {
    for (name, map) in maps() {
        for a in BOTS {
            for b in BOTS {
                let r = play(&map, 3, a, b);
                for (team, bot) in [(0, a), (1, b)] {
                    let s = r.acts[team];
                    assert!(
                        s.errors * 100 <= s.calls * MAX_ACT_ERROR_PCT,
                        "{bot} (team {team}, {a} vs {b} on {name}): {} of {} act calls failed, by kind {:?}",
                        s.errors,
                        s.calls,
                        s.errors_by_kind
                    );
                }
            }
        }
    }
}

#[test]
fn unknown_bot_and_bad_map_are_errors() {
    let (_, map) = maps().remove(0);
    assert!(run(&map, 1, "nope", "idle", 10).is_err());
    assert!(run("not a map", 1, "idle", "idle", 10).is_err());
}

#[test]
fn idle_mirror_finishes() {
    let (_, map) = maps().remove(0);
    let r = play(&map, 1, "idle", "idle");
    assert!(r.winner.is_some());
}

/// Test-only bot: never shoots. Attacking, the carrier runs to the nearest site and plants;
/// defending, it does nothing. It exists to force rounds through the plant, fuse and defuse paths.
struct Planter(ibc::Navigator);

impl ibc::Bot for Planter {
    fn new() -> Self {
        Planter(ibc::Navigator::new())
    }

    fn turn(&mut self) {
        use ibc::UnitExt;
        let (me, info) = (ibc::me(), ibc::match_info());
        if !me.alive() || info.phase != ibc::PHASE_ACTION || !me.has(ibc::S_HAS_CHARGE) {
            return;
        }
        let nav = &mut self.0;
        let site = ibc::sites()
            .map(|s| (s.centre_x, s.centre_y))
            .min_by_key(|&c| nav.path_len(me.pos(), c).unwrap_or(i32::MAX))
            .expect("map has a site");
        if ibc::dist(me.pos(), site) <= ibc::TILE / 4 {
            ibc::plant().expect("plant on the site centre");
        } else {
            nav.go(&me, site.0, site.1, 100_000).expect("move");
        }
    }
}

/// (attacker wins, defender wins) per `ROUND_WIN_*` reason, index 0 unused.
fn round_reasons(summary_json: &str) -> ([u32; 5], [u32; 5]) {
    let v: serde_json::Value = serde_json::from_str(summary_json).unwrap();
    let (mut atk, mut def) = ([0; 5], [0; 5]);
    for r in v["rounds"].as_array().unwrap() {
        let reason = r["reason"].as_u64().unwrap() as usize;
        if r["winner"] == r["attackers"] {
            atk[reason] += 1;
        } else {
            def[reason] += 1;
        }
    }
    (atk, def)
}

#[test]
fn planted_charge_detonates_against_idle() {
    for (name, map) in maps() {
        let make = |team| if team == 0 { Box::new(<Planter as ibc::Bot>::new()) as Box<dyn ibc::Bot> } else { make_bot("idle").unwrap() };
        let r = run_custom(&map, 1, DEFAULT_MAX_TICKS, make, |_| {}).unwrap();
        let (atk, _) = round_reasons(&r.summary_json);
        // Planter attacks in the first half: every one of those rounds must end in a detonation.
        assert!(atk[ibc::ROUND_WIN_DETONATION as usize] >= 3, "{name}: {}", r.summary_json);
        assert_eq!(r.acts[0].errors, 0, "{name}: planter act errors {:?}", r.acts[0].errors_by_kind);
    }
}

#[test]
fn reference_defenders_stop_a_pacifist_planter() {
    for (name, map) in maps() {
        for bot in ["rusher", "holder"] {
            let make = |team| if team == 0 { Box::new(<Planter as ibc::Bot>::new()) as Box<dyn ibc::Bot> } else { make_bot(bot).unwrap() };
            let r = run_custom(&map, 1, DEFAULT_MAX_TICKS, make, |_| {}).unwrap();
            let (atk, def) = round_reasons(&r.summary_json);
            eprintln!("{bot} on {name}: attackers {atk:?} defenders {def:?}");
            assert_eq!(r.winner, Some(1), "{bot} on {name}: {}", r.summary_json);
            assert_eq!(atk[ibc::ROUND_WIN_DETONATION as usize], 0, "{bot} let a charge detonate on {name}");
        }
    }
}

thread_local! {
    /// (walkers created, walkers that reached every stop) for `navigator_reaches_every_stop`.
    static WALKERS: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// How many stops each walker must reach within round 1.
const WALKER_STOPS: usize = 2;

/// Test-only bot: runs between the sites and the two spawns, starting at a stop chosen by its id.
struct Walker {
    nav: ibc::Navigator,
    stops: Vec<(i32, i32)>,
    visited: usize,
}

impl ibc::Bot for Walker {
    fn new() -> Self {
        let map = ibc::map_info();
        let mut stops: Vec<(i32, i32)> = ibc::sites().map(|s| (s.centre_x, s.centre_y)).collect();
        stops.push((map.enemy_spawn_x, map.enemy_spawn_y));
        stops.push((map.spawn_x, map.spawn_y));
        WALKERS.with(|w| w.set((w.get().0 + 1, w.get().1)));
        Walker { nav: ibc::Navigator::new(), stops, visited: 0 }
    }

    fn turn(&mut self) {
        use ibc::UnitExt;
        let (me, info) = (ibc::me(), ibc::match_info());
        if !me.alive() || info.round != 1 || info.phase != ibc::PHASE_ACTION || self.visited >= WALKER_STOPS {
            return;
        }
        let stop = self.stops[(me.id as usize + self.visited) % self.stops.len()];
        if self.nav.go(&me, stop.0, stop.1, 100_000).expect("move") {
            self.visited += 1;
            if self.visited == WALKER_STOPS {
                WALKERS.with(|w| w.set((w.get().0, w.get().1 + 1)));
            }
        }
    }
}

#[test]
fn navigator_reaches_every_stop() {
    for (name, map) in maps() {
        WALKERS.with(|w| w.set((0, 0)));
        // Idle defenders never shoot, so every walker lives through round 1 (which ends on the clock).
        let make = |team| if team == 0 { Box::new(<Walker as ibc::Bot>::new()) as Box<dyn ibc::Bot> } else { make_bot("idle").unwrap() };
        let r = run_custom(&map, 1, DEFAULT_MAX_TICKS, make, |_| {}).unwrap();
        let (created, done) = WALKERS.with(|w| w.get());
        assert!(created > 0, "{name}: no walkers ran");
        assert_eq!(done, created, "{name}: only {done} of {created} walkers reached their stops in one round");
        assert_eq!(r.acts[0].errors, 0, "{name}: walker act errors {:?}", r.acts[0].errors_by_kind);
    }
}

/// Attackers spawn around the only site, so a planter plants within a few ticks, long before the
/// defenders (who must walk around the wall) can see it happen.
const PLANT_FIRST_MAP: &str = "name: plant_first
team_size: 2
---
####################
#a1a.....#.......dd#
#........#.........#
#..................#
####################
";

#[test]
fn reference_defenders_defuse_a_planted_charge() {
    for bot in ["rusher", "holder"] {
        let make = |team| if team == 0 { Box::new(<Planter as ibc::Bot>::new()) as Box<dyn ibc::Bot> } else { make_bot(bot).unwrap() };
        let r = run_custom(PLANT_FIRST_MAP, 1, DEFAULT_MAX_TICKS, make, |_| {}).unwrap();
        let (atk, def) = round_reasons(&r.summary_json);
        eprintln!("{bot} on plant_first: attackers {atk:?} defenders {def:?}");
        // The planter attacks for the six rounds of the first half; each must be planted and then defused.
        assert_eq!(def[ibc::ROUND_WIN_DEFUSE as usize], 6, "{bot}: {}", r.summary_json);
        assert_eq!(r.winner, Some(1), "{bot}");
        let s = r.acts[1];
        assert!(s.errors * 100 <= s.calls * MAX_ACT_ERROR_PCT, "{bot}: act errors {:?} of {}", s.errors_by_kind, s.calls);
    }
}
