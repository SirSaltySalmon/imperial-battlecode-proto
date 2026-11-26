# Replays (`.ibcr`, format 2)

A replay holds everything needed to re-simulate a match without the bots: the map, the rules, the seed and
every accepted input per tick, with the state hashes after every step. Next to that it carries what each
team's bots produced that is not a simulation input: logs, fuel per turn and debug drawings. That part is
private to the team, and a replay can be redacted so it only has one team's private data, or none.

Code: `crates/replay` (format, verification, redaction), `crates/runner` (recording), `crates/engine-wasm`
(the viewer's loader).

## What is public and what is private

| In every replay | Private to the team (removed by redaction) |
|---|---|
| map, rules, seed, result | memory writes (`MemWrite` inputs, team, draft and buy scope) |
| every action (`Act` input) of both teams, in order | the team's memory hash per tick |
| bot crashes (`Fault`) | logs: what each unit wrote to `ibc.log`, stdout or stderr, per turn |
| the world hash per tick | fuel used per turn, and whether the turn ran out of fuel |
| | debug drawings (`debug_draw`) per tick |
| | the bot's name and module hash in the header (they would let others count the team's versions); a redacted side reads `A` or `B` and an empty hash |

Every action stays public, so behaviour can be watched and imitated; what is protected is intent (memory and
logs). Workers see both modules, so this assumes trusted workers.

## Split state hash

The per-tick hash is three numbers (`ibc_engine::StateHashes`):

- `world`: every state field that affects simulation, except team memory;
- `memory[0]`, `memory[1]`: one team's team memory and draft memory.

Memory is read by bots and never by the simulation, so a replay without one team's memory writes still
re-simulates to the same world. Verification checks the world hash on every tick and the memory hash of
every team that is not redacted. Tampering with a kept team's memory write fails verification; removing a
team's writes only verifies if the file says that team is redacted, and then nothing about its memory is
claimed. `Match::state_hash()` (summaries, goldens) is the three combined with FNV-1a.

Logs, fuel and debug drawings never feed any hash. `World::step` discards debug drawings, so the runner takes
them with `Match::take_debug()` before stepping.

## Header

One JSON line (`ibc_replay::Header`):

| Field | |
|---|---|
| `engine_version`, `ruleset_hash`, `map_hash`, `map_text`, `seed`, `wasmtime_version`, `abi_version`, `bots`, `result` | as in format 1 |
| `team_size` | units per team: unit ids `0..team_size` are team A, the next `team_size` team B. Lets anyone redact without the engine |
| `redacted` | `[bool, bool]`: teams whose private data was removed |
| `rules` | `{core, weapons, abilities, economy}`: the `rules/*.toml` texts the match was played under. Omitted if the recorder could not vouch for them (a ruleset changed in code after parsing) |

Re-simulation uses the header rules when present (they must hash to `ruleset_hash`), else the reader's
built-in rules (which must). So a replay stays verifiable and drawable after the built-in rules change, as
long as the engine version is the same. The viewer reads sizes and names from these rules too.

## Bytes

```text
"IBCR" | u32 LE format version (2) | raw DEFLATE(header JSON '\n' body)

body = varint tick_count, then per tick:
  varint event_count, events:
    0x00 Act       varint unit, varint kind, zigzag a, b, c
    0x01 MemWrite  varint unit, varint scope, varint off, varint len, bytes
    0x02 Fault     varint unit
  u64 LE world hash
  for team A, then team B, unless the header marks it redacted:
    u64 LE memory hash
    varint record_count, records:
      0x00 Log   varint unit, varint len, bytes      (the unit's log output during this tick's turn)
      0x01 Fuel  varint unit, varint used, u8 0|1    (1: the turn ran out of fuel)
      0x02 Draw  varint unit, varint kind, zigzag x0, y0, x1, y1, varint color
```

Records of tick `t` were made by bots looking at frame `t` (the state before the step), so the viewer shows
them with that frame. Format 1 (one combined hash per tick, no records) is not read: its hashes cannot be
split, so this engine could not verify it. Re-run the match.

## Commands

```
ibc replay verify FILE                  # full or redacted; says which teams' memory was not checked
ibc replay redact IN OUT --keep A|B|none
ibc replay records FILE                 # memory writes, logs, fuel and debug drawings as JSON
ibc replay info FILE                    # header
```

## Platform

The coordinator stores the full replay as the record and redacts on request:
`GET /v1/matches/{id}/replay?view=public|team|full`.

| View | Who | Private data kept |
|---|---|---|
| `public` (default) | anyone, no token | none |
| `team` | a team token of a team that played the match | that team's |
| `full` | the admin token | both (the stored bytes) |

A team token on `view=full`, a team that did not play, or the admin token on `view=team` get 403; no token on
`team` or `full` gets 401. A team that played both sides gets both sides' data on `view=team`. A stored file
that is not a format-2 replay cannot be redacted: `public` and `team` answer 410, `full` still serves it. The
last 32 redacted views are kept in memory, so a popular match is re-encoded once.

Workers never name bots, so the stored header names each side by a module hash prefix (docs/JOBS.md). The
`public` and `team` views replace both names with the current team names; `full` is the stored bytes, so it
keeps the prefixes and still hashes to `replay_sha256`.

Workers can read any blob, including full replays, by hash. That fits the existing trust model (workers run
both modules anyway) but means a worker token is as private as the admin token for replay privacy.
