//! The private part of a replay as JSON, for the viewer and `ibc replay records`.

use crate::{Event, Record, Replay};
use ibc_engine::Input;
use serde_json::{json, Value};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One JSON object with every private record by tick, flat lists so a long match stays one cheap parse:
///
/// ```text
/// {"redacted": [false, true],
///  "logs":  [[tick, unit, "text"], ...],
///  "fuel":  [[tick, unit, used, out_of_fuel], ...],
///  "draws": [[tick, unit, kind, x0, y0, x1, y1, color], ...],
///  "mem":   [[tick, unit, scope, off, "hex bytes"], ...]}      (only with `with_memory`)
/// ```
///
/// `tick` is the tick the bots were playing, i.e. records of tick `t` belong with frame `t` (the state the
/// bots saw), not with the frame after the step.
pub fn records_json(replay: &Replay, with_memory: bool) -> String {
    let (mut logs, mut fuel, mut draws, mut mem) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (tick, t) in replay.ticks.iter().enumerate() {
        for rec in t.private.iter().flatten() {
            match rec {
                Record::Log { unit, text } => logs.push(json!([tick, unit, String::from_utf8_lossy(text)])),
                Record::Fuel { unit, used, out_of_fuel } => fuel.push(json!([tick, unit, used, out_of_fuel])),
                Record::Draw((unit, kind, x0, y0, x1, y1, color)) => {
                    draws.push(json!([tick, unit, kind, x0, y0, x1, y1, color]))
                }
            }
        }
        if with_memory {
            for e in &t.events {
                if let Event::Input(Input::MemWrite { unit, scope, off, data }) = e {
                    mem.push(json!([tick, unit, scope, off, hex(data)]));
                }
            }
        }
    }
    let mut out = json!({
        "redacted": replay.header.redacted,
        "logs": logs,
        "fuel": fuel,
        "draws": draws,
    });
    if with_memory {
        out["mem"] = Value::Array(mem);
    }
    out.to_string()
}
