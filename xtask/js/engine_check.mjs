// Loads replays with the browser engine (wasm32-unknown-unknown build of ibc-engine-wasm) in Node and
// prints one JSON line per replay. `cargo xtask engine-wasm` compares the lines with the native engine.
//   node engine_check.mjs <ibc_engine_wasm.wasm> <replay.ibcr>...
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";

const [wasmPath, ...replays] = process.argv.slice(2);
if (!wasmPath || replays.length === 0) {
  console.error("usage: node engine_check.mjs <engine.wasm> <replay.ibcr>...");
  process.exit(2);
}

const module = new WebAssembly.Module(readFileSync(wasmPath));
const imports = WebAssembly.Module.imports(module);
if (imports.length !== 0) {
  console.error(`the engine wasm must have no imports, found: ${imports.map((i) => `${i.module}.${i.name}`).join(", ")}`);
  process.exit(1);
}
const ex = new WebAssembly.Instance(module, {}).exports;
// The memory can grow during a call, so take a fresh view every time.
const bytes = (ptr, len) => new Uint8Array(ex.memory.buffer, ptr, len);
const text = (ptr, len) => Buffer.from(bytes(ptr, len)).toString("utf8");
const sha256 = (s) => createHash("sha256").update(s, "utf8").digest("hex");

const engine = text(ex.ibc_version_ptr(), ex.ibc_version_len());
for (const file of replays) {
  const data = readFileSync(file);
  const ptr = ex.ibc_alloc(data.length);
  bytes(ptr, data.length).set(data);
  const rc = ex.ibc_load(ptr, data.length);
  ex.ibc_free(ptr, data.length);
  if (rc !== 0) {
    console.log(JSON.stringify({ file, engine, error: text(ex.ibc_error_ptr(), ex.ibc_error_len()) }));
    continue;
  }
  const count = ex.ibc_frame_count();
  const all = createHash("sha256");
  let last = "";
  for (let i = 0; i < count; i++) {
    last = text(ex.ibc_frame_ptr(i), ex.ibc_frame_len(i));
    all.update(last, "utf8");
    all.update("\n", "utf8");
  }
  const header = text(ex.ibc_header_ptr(), ex.ibc_header_len());
  console.log(
    JSON.stringify({
      file,
      engine,
      frames: count,
      last_frame_sha256: sha256(last),
      all_frames_sha256: all.digest("hex"),
      header_sha256: sha256(header),
    }),
  );
}
