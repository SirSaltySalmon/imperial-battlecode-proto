//! The committed fixture replay must load with this engine build. If you changed rules or mechanics on
//! purpose, regenerate it: `ibc run bots/rusher bots/holder --map maps/tunnels.ibcmap --seed 1 -o out/fx`
//! then copy out/fx/replay.ibcr to fixtures/sample.ibcr.
#[test]
fn fixture_replay_loads_and_verifies() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/sample.ibcr")).unwrap();
    let (frames, header) = ibc_engine_wasm::load(&bytes).expect("fixture replay must verify");
    assert!(frames.len() > 100);
    assert!(header.contains("engine_version"));
}

/// The viewer names weapons from this export, so every weapon in `rules/weapons.toml` must be in it, in id
/// order.
#[test]
fn rules_export_names_every_weapon() {
    let rules: serde_json::Value = serde_json::from_str(&ibc_engine_wasm::rules()).unwrap();
    let names: Vec<&str> = rules["weapons"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    let builtin = ibc_engine::Ruleset::builtin();
    assert_eq!(names, builtin.weapon_names.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(names.len() >= 6 && names[0] == "pistol" && names.contains(&"marksman"), "{names:?}");
    assert_eq!(rules["fuse_ticks"], builtin.core.fuse_ticks);
}
