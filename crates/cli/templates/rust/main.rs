// The wasm entry point: runs one `{{type}}` per unit, one `turn` per tick. The bot itself is in lib.rs.
ibc::bot!({{crate}}::{{type}});
