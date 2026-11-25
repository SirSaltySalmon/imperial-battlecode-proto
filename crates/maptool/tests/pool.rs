//! The committed map pool must stay lint-clean, and the maps designed against the bands must stay
//! inside them. (`crossfire` and `tunnels` predate the bands and are only linted.)

use ibc_maptool::analyze::analyze;
use ibc_maptool::bands::Bands;
use ibc_maptool::lint::lint;
use ibc_maptool::render::{ascii, svg, Overlay};
use ibc_maptool::sight::los;
use ibc_maptool::{report, tile_centre, Map, Ruleset};

const PREDATE_BANDS: [&str; 2] = ["crossfire", "tunnels"];

fn pool() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../maps");
    let mut maps: Vec<(String, String)> = std::fs::read_dir(dir)
        .expect("maps directory")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ibcmap"))
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    maps.sort();
    assert!(
        maps.len() >= 10,
        "expected the full pool, found {}",
        maps.len()
    );
    maps
}

#[test]
fn every_map_is_lint_clean() {
    let rules = Ruleset::builtin();
    for (name, text) in pool() {
        let issues = lint(&text, Some(&name), &rules);
        assert!(issues.is_empty(), "{name}: {issues:?}");
    }
}

#[test]
fn designed_maps_are_inside_every_band() {
    let (rules, bands) = (Ruleset::builtin(), Bands::default());
    for (name, text) in pool() {
        let map = Map::parse(&text).unwrap();
        let a = analyze(&map, &rules, &bands);
        assert_eq!(a.sites.len(), map.sites.len());
        if !PREDATE_BANDS.contains(&name.as_str()) {
            assert!(a.flags.is_empty(), "{name}: {:?}", a.flags);
        }
        // Reports and pictures render for every map.
        let md = report::markdown(&a);
        assert!(md.starts_with(&format!("# {name} ")) && md.contains("## Flags"));
        let json: serde_json::Value = serde_json::from_str(&report::json(&a)).unwrap();
        assert_eq!(json["sites"].as_array().unwrap().len(), map.sites.len());
        let overlays = Overlay::parse_list("routes,chokes,sightlines,advantage").unwrap();
        assert!(svg(&map, Some(&a), &overlays).ends_with("</svg>\n"));
        assert_eq!(
            ascii(&map, Some(&a), &overlays).lines().count() as i32,
            map.height + 2
        );
    }
}

#[test]
fn reported_sightlines_and_paths_hold_up_in_the_engine() {
    let (rules, bands) = (Ruleset::builtin(), Bands::default());
    for (name, text) in pool() {
        let map = Map::parse(&text).unwrap();
        let a = analyze(&map, &rules, &bands);
        for s in &a.sightlines {
            let (p, q) = (tile_centre(s.from.0, s.from.1), tile_centre(s.to.0, s.to.1));
            assert!(
                map.segment_wall(p, q).is_none() && los(&map, p, q),
                "{name}: {s:?}"
            );
        }
        for site in &a.sites {
            for path in [&site.attacker_path, &site.defender_path] {
                assert!(path.len() >= 2, "{name}: site {} has no path", site.site);
                // Walk each leg in small steps: the engine's circle test must never block.
                for leg in path.windows(2) {
                    for k in 0..=32 {
                        let p = (
                            leg[0].0 + (leg[1].0 - leg[0].0) * k / 32,
                            leg[0].1 + (leg[1].1 - leg[0].1) * k / 32,
                        );
                        assert!(
                            !map.circle_blocked(p, rules.core.unit_radius),
                            "{name}: path blocked at {p:?}"
                        );
                    }
                }
            }
        }
    }
}
