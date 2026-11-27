//! Plant points: whatever a site's shape, `SiteInfo.plant_x`/`plant_y` is on the site and the carrier can
//! plant there, even where the bounding-box centre is floor off the site or inside a wall.

use ibc_engine::abi::*;
use ibc_engine::{Map, Match, Ruleset};

/// Each fixture's site: (map text, tile of the plant point, tile of the bounding-box centre).
const FIXTURES: [(&str, (i32, i32), (i32, i32)); 2] = [
    (include_str!("fixtures/site_l.ibcmap"), (2, 3), (2, 4)),
    (
        include_str!("fixtures/site_hollow.ibcmap"),
        (10, 3),
        (10, 5),
    ),
];

fn tile_centre(t: (i32, i32)) -> (i32, i32) {
    (t.0 * TILE + TILE / 2, t.1 * TILE + TILE / 2)
}

fn site_row(m: &Match, unit: u32) -> SiteInfo {
    let mut buf = Vec::new();
    let n = m.get(unit, T_SITES, 0, &mut buf);
    assert_eq!(n as usize, core::mem::size_of::<SiteInfo>());
    from_bytes(&buf).unwrap()
}

#[test]
fn the_plant_point_is_on_the_site_where_the_box_centre_is_not() {
    for (text, plant, centre) in FIXTURES {
        let map = Map::parse(text).unwrap();
        let s = map.sites[0];
        let name = &map.name;
        assert_eq!((s.plant_x, s.plant_y), tile_centre(plant), "{name}");
        assert_eq!(map.site_at((s.plant_x, s.plant_y)), Some(0), "{name}");
        // The fixtures are the shapes the plant point exists for: the box centre is off the site.
        assert_eq!((s.centre_x, s.centre_y), tile_centre(centre), "{name}");
        assert_eq!(map.site_at((s.centre_x, s.centre_y)), None, "{name}");
    }
    let hollow = Map::parse(FIXTURES[1].0).unwrap();
    let c = (hollow.sites[0].centre_x, hollow.sites[0].centre_y);
    assert_eq!(
        hollow.tile(c.0 / TILE, c.1 / TILE),
        TILE_WALL,
        "the hollow site's box centre is in its pillar"
    );
}

#[test]
fn a_carrier_on_the_plant_point_plants() {
    for (text, _, _) in FIXTURES {
        let rules = Ruleset::builtin();
        let plant_ticks = rules.core.plant_ticks;
        let mut m = Match::new(rules, Map::parse(text).unwrap(), 1);
        while m.world.phase != PHASE_ACTION {
            m.step();
        }
        let carrier = m
            .world
            .units
            .iter()
            .position(|u| u.has_charge)
            .expect("a carrier") as u32;
        let site = site_row(&m, carrier);
        let plant = (site.plant_x, site.plant_y);
        let name = m.world.map.name.clone();

        // From the box centre of the L-shaped site (floor) the plant is refused: it is not on the site.
        if m.world.map.tile(site.centre_x / TILE, site.centre_y / TILE) == TILE_FLOOR {
            m.world.units[carrier as usize].pos = (site.centre_x, site.centre_y);
            assert_eq!(m.act(carrier, A_PLANT, 0, 0, 0), ERR_OUT_OF_RANGE, "{name}");
        }

        m.world.units[carrier as usize].pos = plant;
        for _ in 0..plant_ticks {
            assert_eq!(m.act(carrier, A_PLANT, 0, 0, 0), OK, "{name}");
            m.step();
        }
        assert_eq!(
            m.world.phase, PHASE_PLANTED,
            "{name}: planted after plant_ticks"
        );
        assert_eq!(m.world.charge_pos, plant, "{name}");
        let event = m
            .world
            .events
            .iter()
            .find(|e| e.kind == EVENT_PLANT)
            .expect("plant event");
        assert_eq!(
            (event.x, event.y, event.value),
            (plant.0, plant.1, site.id),
            "{name}"
        );
    }
}
