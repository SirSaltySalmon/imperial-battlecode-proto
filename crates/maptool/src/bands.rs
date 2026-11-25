//! Target bands for `analyze`: the ranges a competitive attack/defend map should land in, and the
//! parameters of the measurements. Defaults are tuned for the stock ruleset; override any of them
//! with `--bands file.toml` (same keys as `ibc-maptool bands` prints). `docs/maps.md` explains why.

use serde::{Deserialize, Serialize};

/// Inclusive `[low, high]`.
pub type Band = [i64; 2];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Bands {
    /// Ticks for the fastest attacker to run from spawn to the site centre.
    pub attacker_ticks: Band,
    /// Ticks for the fastest defender to run from spawn to the site centre.
    pub defender_ticks: Band,
    /// Attacker ticks minus defender ticks: how long defenders hold the site before first contact.
    pub advantage_ticks: Band,
    /// Ticks until the teams meet when both run at each other's spawn (half the spawn-to-spawn run).
    /// Above the band a unit cannot cross the map and come back within a round, and defenders
    /// pushing out are out of earshot of an attack that has already left.
    pub spawn_to_spawn_ticks: Band,
    /// Ticks to run between two site centres.
    pub rotation_ticks: Band,
    /// Distinct chokepoints between the attacker spawn zone and the site.
    pub routes: Band,
    /// Total width of those chokepoints, in tiles.
    pub cut_width: Band,
    /// Longest sightline on the map, in tiles.
    pub longest_sightline_tiles: Band,
    /// Share of the site seen from the best defender hold position, percent.
    pub best_hold_share_pct: Band,
    /// Mean share of the site seen over all hold positions, percent.
    pub mean_hold_share_pct: Band,
    /// Walls plus low cover among the tiles around the site, percent.
    pub cover_pct: Band,
    /// Walkable tiles that can see a charge planted at the site centre, percent of the map.
    pub postplant_visible_pct: Band,
    /// Fuse ticks left when a defender rotating from the nearest other site finishes defusing.
    pub retake_slack_ticks: Band,

    /// Hold positions are looked for between these distances (tiles) from the site centre.
    pub hold_radius_tiles: Band,
    /// A hold position must be reachable by a defender this many ticks before any attacker.
    pub hold_lead_ticks: i64,
    /// Cover density is measured within this many tiles of the site centre.
    pub cover_radius_tiles: i64,
    /// Chokes are looked for beyond this share (percent) of the attackers' way to the site: the
    /// first part of the approach, and anything within `spawn_zone_steps` of a spawn, cannot be cut.
    pub approach_start_pct: i64,
    pub spawn_zone_steps: i64,
    /// How many of the longest sightlines to list.
    pub sightlines_listed: i64,
}

impl Default for Bands {
    fn default() -> Bands {
        Bands {
            attacker_ticks: [22, 50],
            defender_ticks: [6, 36],
            advantage_ticks: [5, 18],
            spawn_to_spawn_ticks: [14, 26],
            rotation_ticks: [10, 30],
            routes: [2, 5],
            cut_width: [3, 14],
            longest_sightline_tiles: [10, 24],
            best_hold_share_pct: [50, 100],
            mean_hold_share_pct: [25, 85],
            cover_pct: [8, 40],
            postplant_visible_pct: [3, 35],
            retake_slack_ticks: [0, 25],
            hold_radius_tiles: [2, 9],
            hold_lead_ticks: 2,
            cover_radius_tiles: 6,
            approach_start_pct: 40,
            spawn_zone_steps: 4,
            sightlines_listed: 5,
        }
    }
}

impl Bands {
    pub fn from_toml(text: &str) -> Result<Bands, String> {
        let b: Bands = toml::from_str(text).map_err(|e| format!("bands: {e}"))?;
        for (name, band) in b.named() {
            if band[0] > band[1] {
                return Err(format!(
                    "bands: {name} = [{}, {}] has low above high",
                    band[0], band[1]
                ));
            }
        }
        Ok(b)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("bands serialise")
    }

    fn named(&self) -> [(&'static str, Band); 14] {
        [
            ("attacker_ticks", self.attacker_ticks),
            ("defender_ticks", self.defender_ticks),
            ("advantage_ticks", self.advantage_ticks),
            ("spawn_to_spawn_ticks", self.spawn_to_spawn_ticks),
            ("rotation_ticks", self.rotation_ticks),
            ("routes", self.routes),
            ("cut_width", self.cut_width),
            ("longest_sightline_tiles", self.longest_sightline_tiles),
            ("best_hold_share_pct", self.best_hold_share_pct),
            ("mean_hold_share_pct", self.mean_hold_share_pct),
            ("cover_pct", self.cover_pct),
            ("postplant_visible_pct", self.postplant_visible_pct),
            ("retake_slack_ticks", self.retake_slack_ticks),
            ("hold_radius_tiles", self.hold_radius_tiles),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_partial_files_override() {
        let d = Bands::default();
        assert_eq!(Bands::from_toml(&d.to_toml()).unwrap(), d);
        let b = Bands::from_toml("advantage_ticks = [3, 9]\ncover_radius_tiles = 4\n").unwrap();
        assert_eq!(
            (b.advantage_ticks, b.cover_radius_tiles, b.routes),
            ([3, 9], 4, d.routes)
        );
        assert!(Bands::from_toml("advantage_ticks = [9, 3]").is_err());
        assert!(Bands::from_toml("no_such_band = [1, 2]").is_err());
    }
}
