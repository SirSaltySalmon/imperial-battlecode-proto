use ibc_engine::{Input, Map, Match, Ruleset};
use ibc_replay::*;

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");

fn header() -> Header {
    let bot = |n: &str| BotInfo { name: n.into(), sha256: sha256_hex(n.as_bytes()), fuel_multiplier: 1.0 };
    Header {
        engine_version: ibc_engine::ENGINE_VERSION.into(),
        ruleset_hash: hash_hex(Ruleset::builtin().hash()),
        map_hash: hash_hex(Map::parse(MAP).unwrap().hash()),
        map_text: MAP.into(),
        seed: 7,
        wasmtime_version: String::new(),
        abi_version: ibc_engine::abi::ABI_VERSION,
        bots: [bot("a"), bot("b")],
        result: MatchResult { winner: None, score: [0, 0], ticks: 0, finished: false },
    }
}

/// Drive a match with scripted inputs (no bots) and record it.
fn record(ticks: u32) -> Replay {
    use ibc_engine::abi::Host;
    let mut m = Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 7);
    let mut w = ReplayWriter::new(header());
    for t in 0..ticks {
        for unit in m.turn_order() {
            m.act(unit, ibc_engine::abi::A_MOVE, ((t * 997 + unit * 13) % 65536) as i32, 200, 0);
            if t % 5 == 0 {
                m.mem_write(unit, ibc_engine::abi::MEM_TEAM, unit, &[t as u8, unit as u8]);
            }
        }
        if t == 3 {
            w.inputs(m.take_inputs());
            w.fault(1);
            m.fault(1);
        }
        w.inputs(m.take_inputs());
        m.step();
        w.end_tick(m.state_hash());
    }
    w.finish(MatchResult { winner: m.winner(), score: m.score(), ticks, finished: m.finished() })
}

#[test]
fn sha256_known_vector() {
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn encode_decode_roundtrip() {
    let mut r = record(40);
    // every event kind and extreme values survive
    r.ticks[0].events.push(Event::Input(Input::Act { unit: u32::MAX, kind: 9, a: i32::MIN, b: i32::MAX, c: -1 }));
    r.ticks[0].events.push(Event::Input(Input::MemWrite { unit: 3, scope: 1, off: 70000, data: vec![0, 255, 7] }));
    let bytes = r.encode();
    assert_eq!(&bytes[..4], b"IBCR");
    let back = Replay::decode(&bytes).unwrap();
    assert_eq!(back, r);
    assert_eq!(back.encode(), bytes);
}

#[test]
fn verify_accepts_a_recorded_match() {
    let r = record(120);
    assert!(r.ticks.iter().any(|t| !t.events.is_empty()));
    verify(&r).unwrap();
    verify(&Replay::decode(&r.encode()).unwrap()).unwrap();
}

#[test]
fn verify_rejects_tampering() {
    let good = record(60);

    let mut r = good.clone();
    r.ticks[30].hash ^= 1;
    assert!(verify(&r).unwrap_err().contains("tick 30"));

    let mut r = good.clone();
    r.header.seed += 1;
    assert!(verify(&r).is_err());

    let mut r = good.clone();
    r.header.result.score[0] += 1;
    assert!(verify(&r).unwrap_err().contains("score"));

    let mut r = good.clone();
    r.header.ruleset_hash = hash_hex(1);
    assert!(verify(&r).unwrap_err().contains("ruleset"));
}

#[test]
fn decode_rejects_garbage() {
    assert!(Replay::decode(b"nope").is_err());
    assert!(Replay::decode(b"IBCR\x09\0\0\0").is_err());
    let mut bytes = record(5).encode();
    bytes.truncate(bytes.len() - 3);
    assert!(Replay::decode(&bytes).is_err());
}
