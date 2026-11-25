// Copies the engine build into public/engine/<engine_version>.wasm and refreshes public/engine/index.json.
// Engine versions sit side by side: builds already in public/engine are kept, so a viewer deployment can
// carry one file per released engine and old replays stay viewable.
//
//   node scripts/copy-engine.mjs [path/to/ibc_engine_wasm.wasm]
// Without a path: $IBC_ENGINE_WASM, else the release build under $CARGO_TARGET_DIR (default ../target).
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const outDir = resolve(here, "../public/engine");
const source = resolve(
  process.argv[2] ??
    process.env.IBC_ENGINE_WASM ??
    join(process.env.CARGO_TARGET_DIR ?? join(here, "../../target"), "wasm32-unknown-unknown/release/ibc_engine_wasm.wasm"),
);

mkdirSync(outDir, { recursive: true });

if (existsSync(source)) {
  const bytes = readFileSync(source);
  // Ask the module itself which engine it is: the file name must match what replay headers say.
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const e = instance.exports;
  const version = new TextDecoder().decode(
    new Uint8Array(e.memory.buffer, e.ibc_version_ptr(), e.ibc_version_len()),
  );
  if (!/^[0-9A-Za-z.+-]+$/.test(version)) throw new Error(`unexpected engine version ${JSON.stringify(version)}`);
  copyFileSync(source, join(outDir, `${version}.wasm`));
  console.log(`engine ${version} -> public/engine/${version}.wasm (${bytes.length} bytes)`);
} else {
  console.warn(
    `warning: ${source} not found.\n` +
      "  Build it with: cargo build -p ibc-engine-wasm --release --target wasm32-unknown-unknown",
  );
}

const versions = readdirSync(outDir)
  .filter((f) => f.endsWith(".wasm"))
  .map((f) => f.slice(0, -5))
  .sort();
writeFileSync(join(outDir, "index.json"), JSON.stringify({ versions }) + "\n");
if (versions.length === 0) {
  console.warn("warning: no engine builds in public/engine; the viewer will not be able to open replays.");
}
