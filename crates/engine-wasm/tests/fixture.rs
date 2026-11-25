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
