//! The `.ibcr` byte format (version 2):
//!
//! ```text
//! "IBCR"                      4 bytes magic
//! format version              u32 little-endian (2)
//! DEFLATE(payload)            raw deflate stream (no zlib header)
//!
//! payload = header JSON (one line, serde_json of `Header`) '\n' body
//! body    = varint tick_count, then per tick:
//!             varint event_count, then per event one of
//!               0x00 Act      varint unit, varint kind, zigzag a, zigzag b, zigzag c
//!               0x01 MemWrite varint unit, varint scope, varint off, varint len, bytes
//!               0x02 Fault    varint unit          (the bot of this unit crashed at this point of the tick)
//!             world hash after the step, u64 little-endian
//!             then for team A and team B, unless the header marks the team redacted:
//!               memory hash after the step, u64 little-endian
//!               varint record_count, then per record one of
//!                 0x00 Log    varint unit, varint len, bytes
//!                 0x01 Fuel   varint unit, varint used, u8 out_of_fuel (0 or 1)
//!                 0x02 Draw   varint unit, varint kind, zigzag x0, y0, x1, y1, varint color
//! ```
//!
//! Version 1 (one combined state hash per tick, no records) is not read: its hashes cannot be split, so this
//! build could not verify it anyway.

use crate::{Event, Header, Record, Replay, TickRecord, FORMAT_VERSION, MAGIC};
use ibc_engine::Input;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_zigzag(out: &mut Vec<u8>, v: i32) {
    put_varint(out, ((v << 1) ^ (v >> 31)) as u32 as u64);
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.pos).ok_or("replay body truncated")?;
        self.pos += 1;
        Ok(v)
    }
    fn varint(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift >= 64 {
                return Err("replay body: varint too long".into());
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    fn u32(&mut self) -> Result<u32, String> {
        u32::try_from(self.varint()?).map_err(|_| "replay body: value out of range".to_string())
    }
    fn zigzag(&mut self) -> Result<i32, String> {
        let v = self.u32()?;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("replay body truncated")?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let len = self.varint()? as usize;
        Ok(self.take(len)?.to_vec())
    }
}

fn put_event(out: &mut Vec<u8>, e: &Event) {
    match e {
        Event::Input(Input::Act { unit, kind, a, b, c }) => {
            out.push(0);
            put_varint(out, *unit as u64);
            put_varint(out, *kind as u64);
            put_zigzag(out, *a);
            put_zigzag(out, *b);
            put_zigzag(out, *c);
        }
        Event::Input(Input::MemWrite { unit, scope, off, data }) => {
            out.push(1);
            put_varint(out, *unit as u64);
            put_varint(out, *scope as u64);
            put_varint(out, *off as u64);
            put_varint(out, data.len() as u64);
            out.extend_from_slice(data);
        }
        Event::Fault(unit) => {
            out.push(2);
            put_varint(out, *unit as u64);
        }
    }
}

fn get_event(r: &mut Reader) -> Result<Event, String> {
    Ok(match r.byte()? {
        0 => {
            Event::Input(Input::Act { unit: r.u32()?, kind: r.u32()?, a: r.zigzag()?, b: r.zigzag()?, c: r.zigzag()? })
        }
        1 => Event::Input(Input::MemWrite { unit: r.u32()?, scope: r.u32()?, off: r.u32()?, data: r.bytes()? }),
        2 => Event::Fault(r.u32()?),
        t => return Err(format!("replay body: unknown event tag {t}")),
    })
}

fn put_record(out: &mut Vec<u8>, rec: &Record) {
    match rec {
        Record::Log { unit, text } => {
            out.push(0);
            put_varint(out, *unit as u64);
            put_varint(out, text.len() as u64);
            out.extend_from_slice(text);
        }
        Record::Fuel { unit, used, out_of_fuel } => {
            out.push(1);
            put_varint(out, *unit as u64);
            put_varint(out, *used);
            out.push(*out_of_fuel as u8);
        }
        Record::Draw((unit, kind, x0, y0, x1, y1, color)) => {
            out.push(2);
            put_varint(out, *unit as u64);
            put_varint(out, *kind as u64);
            for v in [x0, y0, x1, y1] {
                put_zigzag(out, *v);
            }
            put_varint(out, *color as u64);
        }
    }
}

fn get_record(r: &mut Reader) -> Result<Record, String> {
    Ok(match r.byte()? {
        0 => Record::Log { unit: r.u32()?, text: r.bytes()? },
        1 => {
            let (unit, used) = (r.u32()?, r.varint()?);
            let out_of_fuel = match r.byte()? {
                0 => false,
                1 => true,
                b => return Err(format!("replay body: bad out_of_fuel flag {b}")),
            };
            Record::Fuel { unit, used, out_of_fuel }
        }
        2 => Record::Draw((r.u32()?, r.u32()?, r.zigzag()?, r.zigzag()?, r.zigzag()?, r.zigzag()?, r.u32()?)),
        t => return Err(format!("replay body: unknown record tag {t}")),
    })
}

/// Decoded lengths are attacker-controlled: never pre-allocate more than this many entries.
const PREALLOC_CAP: usize = 1 << 16;

fn get_tick(r: &mut Reader, redacted: [bool; 2]) -> Result<TickRecord, String> {
    let ne = r.varint()? as usize;
    let mut t = TickRecord { events: Vec::with_capacity(ne.min(PREALLOC_CAP)), ..Default::default() };
    for _ in 0..ne {
        t.events.push(get_event(r)?);
    }
    t.hashes.world = r.u64()?;
    for team in 0..2 {
        if redacted[team] {
            continue;
        }
        t.hashes.memory[team] = r.u64()?;
        let nr = r.varint()? as usize;
        t.private[team].reserve(nr.min(PREALLOC_CAP));
        for _ in 0..nr {
            t.private[team].push(get_record(r)?);
        }
    }
    Ok(t)
}

impl Replay {
    pub fn encode(&self) -> Vec<u8> {
        let redacted = self.header.redacted;
        let mut payload = serde_json::to_vec(&self.header).expect("header serializes");
        payload.push(b'\n');
        put_varint(&mut payload, self.ticks.len() as u64);
        for t in &self.ticks {
            put_varint(&mut payload, t.events.len() as u64);
            for e in &t.events {
                put_event(&mut payload, e);
            }
            payload.extend_from_slice(&t.hashes.world.to_le_bytes());
            for team in (0..2).filter(|&team| !redacted[team]) {
                payload.extend_from_slice(&t.hashes.memory[team].to_le_bytes());
                put_varint(&mut payload, t.private[team].len() as u64);
                for rec in &t.private[team] {
                    put_record(&mut payload, rec);
                }
            }
        }
        let mut out = Vec::with_capacity(payload.len() / 3 + 16);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&payload, 8));
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Replay, String> {
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err("not an .ibcr replay (bad magic)".into());
        }
        let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        if version != FORMAT_VERSION {
            let hint = if version < FORMAT_VERSION { "; re-run the match to record a new one" } else { "" };
            return Err(format!(
                "unsupported replay format version {version} (this build reads {FORMAT_VERSION}){hint}"
            ));
        }
        let payload = miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes[8..], 1 << 30)
            .map_err(|e| format!("replay is corrupt (inflate: {e})"))?;
        let nl = payload.iter().position(|&b| b == b'\n').ok_or("replay is corrupt (no header line)")?;
        let header: Header =
            serde_json::from_slice(&payload[..nl]).map_err(|e| format!("replay header is invalid: {e}"))?;
        if header.team_size == 0 {
            return Err("replay header is invalid: team_size is 0".into());
        }
        let mut r = Reader { b: &payload[nl + 1..], pos: 0 };
        let n = r.varint()? as usize;
        let mut ticks = Vec::with_capacity(n.min(PREALLOC_CAP));
        for _ in 0..n {
            ticks.push(get_tick(&mut r, header.redacted)?);
        }
        if r.pos != r.b.len() {
            return Err("replay body: trailing bytes".into());
        }
        Ok(Replay { header, ticks })
    }
}
