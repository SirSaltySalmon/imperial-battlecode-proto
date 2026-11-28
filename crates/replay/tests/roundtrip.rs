use ibc_engine::{Input, Map, Match, Ruleset};
use ibc_replay::*;

const MAP: &str = include_str!("../../../maps/crossfire.ibcmap");

fn header() -> Header {
    let bot = |n: &str| BotInfo {
        name: n.into(),
        sha256: sha256_hex(n.as_bytes()),
        fuel_multiplier: 1.0,
        language: "rust".into(),
        memory_multiplier: 1.0,
    };
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
        team_size: Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 7).unit_count() / 2,
        redacted: [false, false],
        rules: HeaderRules::of(&Ruleset::builtin()),
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
            m.debug_draw(unit, ibc_engine::abi::DEBUG_LINE, 0, 0, t as i32, unit as i32, 0xff0000);
            let team = m.unit_team(unit) as usize;
            w.record(
                team,
                Record::Log {
                    unit,
                    text: format!(
                        "t{t} u{unit}
"
                    )
                    .into_bytes(),
                },
            );
            w.record(team, Record::Fuel { unit, used: 1000 + t as u64, out_of_fuel: t % 7 == 0 });
        }
        if t == 3 {
            w.inputs(m.take_inputs());
            w.fault(1);
            m.fault(1);
        }
        w.inputs(m.take_inputs());
        w.draws(m.take_debug());
        m.step();
        w.end_tick(m.state_hashes());
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
    r.ticks[30].hashes.world ^= 1;
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

#[test]
fn records_round_trip_and_are_split_by_team() {
    let r = record(12);
    let team_size = r.header.team_size;
    for t in &r.ticks {
        for (team, recs) in t.private.iter().enumerate() {
            assert!(!recs.is_empty());
            for rec in recs {
                let unit = match rec {
                    Record::Log { unit, .. } | Record::Fuel { unit, .. } => *unit,
                    Record::Draw(d) => d.0,
                };
                assert_eq!((unit >= team_size) as usize, team, "{rec:?} filed under the wrong team");
            }
        }
    }
    assert!(r.ticks[0].private[0].iter().any(|rec| matches!(rec, Record::Draw(_))));
    assert_eq!(Replay::decode(&r.encode()).unwrap(), r);
}

#[test]
fn header_rules_round_trip() {
    let r = record(3);
    let back = Replay::decode(&r.encode()).unwrap();
    let rules = back.header.rules.as_ref().expect("header carries the rules");
    assert_eq!(rules.parse().unwrap().hash(), Ruleset::builtin().hash());
    assert_eq!(back.rules().unwrap().hash(), Ruleset::builtin().hash());

    // Header rules are what re-simulation uses: a changed rule that the header hash agrees with is accepted
    // even though this build's built-in rules differ...
    let mut changed = back.clone();
    let h = changed.header.rules.as_mut().unwrap();
    h.core = h.core.replace("sense_radius = 1536", "sense_radius = 1600");
    let other = h.parse().unwrap();
    assert_ne!(other.hash(), Ruleset::builtin().hash());
    changed.header.ruleset_hash = hash_hex(other.hash());
    assert_eq!(changed.rules().unwrap().hash(), other.hash());
    // ...but rules that do not hash to the header's ruleset hash are not.
    changed.header.ruleset_hash = hash_hex(Ruleset::builtin().hash());
    assert!(changed.rules().unwrap_err().contains("ruleset mismatch"));

    // Without header rules the reader falls back to its built-in ruleset.
    let mut old = back;
    old.header.rules = None;
    verify(&old).unwrap();

    // A ruleset edited after parsing has no trustworthy source.
    let mut edited = Ruleset::builtin();
    edited.core.max_ticks += 1;
    assert!(HeaderRules::of(&edited).is_none());
}

#[test]
fn records_do_not_change_state_hashes() {
    let full = record(60);
    let mut bare = full.clone();
    for t in &mut bare.ticks {
        t.private = Default::default();
    }
    verify(&bare).unwrap();
    // Debug drawings in the engine are not state either.
    use ibc_engine::abi::Host;
    let mut a = Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 7);
    let mut b = Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 7);
    for _ in 0..30 {
        for unit in a.turn_order() {
            a.debug_draw(unit, ibc_engine::abi::DEBUG_DOT, 5, 6, 7, 8, 0x00ff00);
        }
        assert_eq!(a.state_hashes(), b.state_hashes());
        a.step();
        b.step();
    }
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn redacted_replays_verify() {
    let full = record(80);
    let mem_writes = |r: &Replay, team: usize| {
        r.ticks
            .iter()
            .flat_map(|t| &t.events)
            .filter(|e| matches!(e, Event::Input(Input::MemWrite { unit, .. }) if r.header.unit_team(*unit) == team))
            .count()
    };
    assert!(mem_writes(&full, 0) > 0 && mem_writes(&full, 1) > 0);
    for keep in [Keep::Team(0), Keep::Team(1), Keep::None, Keep::Both] {
        let view = full.view(keep);
        let back = Replay::decode(&view.encode()).unwrap();
        assert_eq!(back, view);
        verify(&back).unwrap_or_else(|e| panic!("{keep:?}: {e}"));
        for team in 0..2 {
            let kept = keep == Keep::Both || keep == Keep::Team(team);
            assert_eq!(back.header.redacted[team], !kept);
            assert_eq!(back.header.bots[team].sha256.is_empty(), !kept, "module identity");
            assert_eq!(mem_writes(&back, team) > 0, kept, "{keep:?} team {team}");
            assert_eq!(back.ticks.iter().any(|t| !t.private[team].is_empty()), kept);
            assert_eq!(back.ticks.iter().all(|t| t.hashes.memory[team] == 0), !kept);
        }
    }
    // A redacted file is smaller, and redacting twice changes nothing.
    let public = full.view(Keep::None);
    assert!(public.encode().len() < full.encode().len());
    assert_eq!(public.view(Keep::Team(0)), public);
}

#[test]
fn tampering_with_a_kept_memory_write_fails() {
    let full = record(40);
    let view = full.view(Keep::Team(1));
    let mut r = view.clone();
    let write = r
        .ticks
        .iter_mut()
        .flat_map(|t| &mut t.events)
        .filter_map(|e| match e {
            Event::Input(Input::MemWrite { data, .. }) => Some(data),
            _ => None,
        })
        // The last one: team memory is zeroed at round start, so a write before it may leave no trace.
        .last()
        .expect("team B memory write");
    write[0] ^= 0x55;
    assert!(verify(&r).unwrap_err().contains("team B memory hash"));

    // Dropping it is caught as well, and so is a forged memory hash of the kept team.
    let mut r = view.clone();
    for t in &mut r.ticks {
        t.events.retain(|e| !matches!(e, Event::Input(Input::MemWrite { .. })));
    }
    assert!(verify(&r).is_err());
    let mut r = view;
    r.ticks[10].hashes.memory[1] ^= 1;
    assert!(verify(&r).unwrap_err().contains("tick 10"));
}

#[test]
fn records_json_lists_only_kept_teams() {
    // The first accepted team-memory write is on tick 10: ticks 0 and 5 are the draft and the buy phase.
    let full = record(11);
    let all: serde_json::Value = serde_json::from_str(&records_json(&full, true)).unwrap();
    assert!(!all["logs"].as_array().unwrap().is_empty());
    assert!(!all["mem"].as_array().unwrap().is_empty());
    let none: serde_json::Value = serde_json::from_str(&records_json(&full.view(Keep::None), true)).unwrap();
    for key in ["logs", "fuel", "draws", "mem"] {
        assert!(none[key].as_array().unwrap().is_empty(), "{key}");
    }
    assert_eq!(none["redacted"], serde_json::json!([true, true]));
}

#[test]
fn refused_calls_leave_no_input_and_writes_are_kept_once_per_team_and_scope() {
    use ibc_engine::abi::*;
    let mut m = Match::new(Ruleset::builtin(), Map::parse(MAP).unwrap(), 7);
    let team_b = m.unit_count() / 2;
    // The draft tick: moves and team memory are refused, and so is a bad scope or range.
    assert_eq!(m.act(0, A_MOVE, 0, 100, 0), ERR_NOT_ALLOWED);
    assert_eq!(m.mem_write(0, MEM_TEAM, 0, &[1; 1 << 20]), ERR_NOT_ALLOWED);
    assert_eq!(m.mem_write(0, 9, 0, &[1]), ERR_BAD_ARG);
    assert_eq!(m.mem_write(0, MEM_DRAFT, 1020, &[1; 8]), ERR_OUT_OF_RANGE);
    assert!(m.take_inputs().is_empty());
    // Three writes of team A become one input over their span, under the unit that wrote last.
    assert_eq!(m.mem_write(0, MEM_DRAFT, 8, &[1, 2, 3]), OK);
    assert_eq!(m.mem_write(1, MEM_DRAFT, 4, &[9]), OK);
    assert_eq!(m.mem_write(0, MEM_DRAFT, 9, &[7]), OK);
    assert_eq!(m.mem_write(team_b, MEM_DRAFT, 0, &[5]), OK);
    assert_eq!(m.act(0, A_DRAFT, 1, 0, 0), OK);
    let draft = |unit, off, data: &[u8]| Input::MemWrite { unit, scope: MEM_DRAFT, off, data: data.to_vec() };
    assert_eq!(
        m.take_inputs(),
        [
            Input::Act { unit: 0, kind: A_DRAFT, a: 1, b: 0, c: 0 },
            draft(0, 4, &[9, 0, 0, 0, 1, 7, 3]),
            draft(team_b, 0, &[5])
        ]
    );
    assert!(m.take_inputs().is_empty());

    // In the action phase a unit gets MAX_ACTS accepted actions a tick; the rest are refused and not kept.
    while !m.world.in_action() {
        m.step();
    }
    for i in 0..ibc_engine::MAX_ACTS {
        assert_eq!(m.act(0, A_MOVE, i as i32, 100, 0), OK);
    }
    assert_eq!(m.act(0, A_FACE, 0, 0, 0), ERR_NOT_ALLOWED);
    assert_eq!(m.act(2, A_FACE, 0, 0, 0), OK, "the cap is per unit");
    assert_eq!(m.take_inputs().len(), ibc_engine::MAX_ACTS as usize + 1);
    m.step();
    assert_eq!(m.act(0, A_FACE, 0, 0, 0), OK, "and per tick");
}

#[test]
fn a_recorded_match_keeps_only_accepted_inputs_and_simulating_keeps_none() {
    let r = record(40);
    // Tick 0 is the draft: every move and team-memory write of `record` is refused there.
    assert!(r.ticks[0].events.is_empty(), "{:?}", r.ticks[0].events);
    for (i, t) in r.ticks.iter().enumerate() {
        let mut writes: Vec<(usize, u32)> = t
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Input(Input::MemWrite { unit, scope, .. }) => Some((r.header.unit_team(*unit), *scope)),
                _ => None,
            })
            .collect();
        let n = writes.len();
        writes.dedup();
        assert_eq!(writes.len(), n, "tick {i}: more than one write per team and scope");
        assert_eq!(n > 0, i % 5 == 0 && i >= 10, "tick {i}");
    }
    let mut m = r.simulate(|_| {}).unwrap();
    assert!(m.take_inputs().is_empty(), "re-simulation keeps no copy of the inputs");
}
