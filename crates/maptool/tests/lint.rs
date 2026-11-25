//! One tiny map per lint failure mode, plus the clean case.

use ibc_maptool::lint::{fails, lint, Issue, Severity};
use ibc_maptool::Ruleset;

fn run(grid: &[&str]) -> Vec<Issue> {
    let text = format!("name: t\nteam_size: 3\n---\n{}\n", grid.join("\n"));
    lint(&text, Some("t"), &Ruleset::builtin())
}

fn codes(issues: &[Issue]) -> Vec<&'static str> {
    issues.iter().map(|i| i.code).collect()
}

/// The issue with `code` must be present, be an error and make lint fail.
fn expect_error(grid: &[&str], code: &str) {
    let issues = run(grid);
    let hit = issues.iter().find(|i| i.code == code);
    assert!(
        hit.is_some_and(|i| i.severity == Severity::Error),
        "expected error `{code}`, got {issues:?}"
    );
    assert!(fails(&issues, false));
    assert!(!hit.unwrap().message.is_empty());
}

const CLEAN: [&str; 7] = [
    "###########",
    "#aaa.#....#",
    "#....#.11.#",
    "#....#.11.#",
    "#.........#",
    "#....#.ddd#",
    "###########",
];

#[test]
fn clean_map_has_no_issues() {
    let issues = run(&CLEAN);
    assert!(issues.is_empty(), "{issues:?}");
    assert!(!fails(&issues, true));
}

#[test]
fn open_border() {
    let mut g = CLEAN;
    g[0] = "####.######";
    expect_error(&g, "open-border");
    g = CLEAN;
    g[3] = "#....#.11..";
    expect_error(&g, "open-border");
}

#[test]
fn ragged_rows() {
    let mut g = CLEAN;
    g[2] = "#....#.11.";
    expect_error(&g, "ragged-row");
}

#[test]
fn unknown_tile() {
    let mut g = CLEAN;
    g[4] = "#....x....#";
    expect_error(&g, "bad-tile");
}

#[test]
fn missing_grid_separator() {
    let issues = lint("name: t\n#####\n#a1d#\n#####\n", None, &Ruleset::builtin());
    assert_eq!(codes(&issues), ["no-grid"]);
}

#[test]
fn too_large() {
    let wide = "#".repeat(129);
    let issues = lint(
        &format!("name: t\n---\n{wide}\n{wide}\n{wide}\n"),
        None,
        &Ruleset::builtin(),
    );
    assert!(codes(&issues).contains(&"too-large"), "{issues:?}");
}

#[test]
fn too_few_spawns_is_reported_through_the_engine() {
    let mut g = CLEAN;
    g[1] = "#aa..#....#";
    expect_error(&g, "engine-parse");
}

#[test]
fn spawn_cut_off_from_the_other_spawns() {
    let mut g = CLEAN;
    // The third defender tile sits in a sealed cell.
    g[4] = "#.......###";
    g[5] = "#....#.d#d#";
    g[3] = "#....#.11d#";
    expect_error(&g, "spawn-unreachable");
}

#[test]
fn unreachable_pocket() {
    let mut g = CLEAN;
    g[1] = "#aaa.#..#.#";
    g[2] = "#....#.1###";
    g[3] = "#....#.111#";
    expect_error(&g, "pocket");
}

#[test]
fn site_unreachable() {
    let g = [
        "###########",
        "#aaa.#ddd##",
        "#....#...##",
        "#........##",
        "########..#",
        "#11.....#.#",
        "#11.....#.#",
        "###########",
    ];
    let issues = run(&g);
    assert!(codes(&issues).contains(&"site-unreachable"), "{issues:?}");
    assert!(fails(&issues, false));
}

#[test]
fn site_too_small() {
    let mut g = CLEAN;
    g[2] = "#....#.1..#";
    g[3] = "#....#.1..#";
    expect_error(&g, "site-small");
}

#[test]
fn site_centre_not_on_the_site() {
    // An L-shaped site: the bounding-box centre (tile 8,3) is plain floor.
    let mut g = CLEAN;
    g[2] = "#....#.111#";
    g[3] = "#....#.1..#";
    g[4] = "#......1..#";
    expect_error(&g, "site-centre");
}

#[test]
fn site_centre_inside_cover() {
    let mut g = CLEAN;
    g[2] = "#....#.111#";
    g[3] = "#....#.1o1#";
    g[4] = "#......111#";
    expect_error(&g, "site-centre");
}

#[test]
fn even_sized_site_centre_must_be_surrounded_by_site_tiles() {
    // 2x2 is fine (CLEAN); a 2x2 bounding box with one corner missing is not.
    let mut g = CLEAN;
    g[2] = "#....#.11.#";
    g[3] = "#....#.1..#";
    let issues = run(&g);
    assert!(codes(&issues).contains(&"site-centre"), "{issues:?}");
}

#[test]
fn site_numbers_must_not_skip() {
    let mut g = CLEAN;
    g[2] = "#....#.33.#";
    g[3] = "#....#.33.#";
    expect_error(&g, "site-numbering");
}

#[test]
fn spawns_in_sight_of_each_other() {
    let mut g = CLEAN;
    g[1] = "#aaa......#";
    g[5] = "#....#....#";
    g[4] = "#......ddd#";
    g[2] = "#....#.11.#";
    // Row 1 is now open from the attackers to the east wall, and the defenders moved up into view.
    g[1] = "#aaa...ddd#";
    g[4] = "#.........#";
    expect_error(&g, "spawn-sightline");
}

#[test]
fn diagonal_pinch_is_a_warning() {
    let mut g = CLEAN;
    g[3] = "#...#..11.#";
    g[4] = "#..#......#";
    let issues = run(&g);
    let pinch = issues
        .iter()
        .find(|i| i.code == "diagonal-pinch")
        .expect("pinch reported");
    assert_eq!(pinch.severity, Severity::Warning);
    assert!(!fails(&issues, false), "{issues:?}");
    assert!(
        fails(&issues, true),
        "--strict turns warnings into failures"
    );
}

#[test]
fn gaps_too_narrow_for_a_fat_unit() {
    // With unit_radius above half a tile no unit fits a 1-tile corridor.
    let rules = Ruleset::from_toml(
        &include_str!("../../../rules/core.toml").replace("unit_radius = 300", "unit_radius = 600"),
        include_str!("../../../rules/weapons.toml"),
        include_str!("../../../rules/abilities.toml"),
        include_str!("../../../rules/economy.toml"),
    )
    .unwrap();
    let text = "name: t\nteam_size: 1\n---\n#######\n#a....#\n#.....#\n###.###\n#.11..#\n#.11.d#\n#######\n";
    let issues = lint(text, Some("t"), &rules);
    assert!(codes(&issues).contains(&"narrow-gap"), "{issues:?}");
    assert!(lint(text, Some("t"), &Ruleset::builtin())
        .iter()
        .all(|i| i.code != "narrow-gap"));
}

#[test]
fn header_warnings() {
    let text = "team_size: 7\n---\n#######\n#a.1.d#\n#..1..#\n#######\n";
    let issues = lint(text, Some("x"), &Ruleset::builtin());
    assert!(codes(&issues).contains(&"no-name"), "{issues:?}");
    let text = "name: other\n---\n#######\n#a#11d#\n#..11.#\n#######\n";
    let c = lint(text, Some("x"), &Ruleset::builtin());
    assert!(
        codes(&c).contains(&"name-mismatch") && codes(&c).contains(&"no-team-size"),
        "{c:?}"
    );
}
