// The `.ibcr` container, read just far enough to learn which engine build a replay needs:
//
//   "IBCR" | u32 LE format version | raw DEFLATE( header JSON line '\n' body )
//
// The body (inputs and state hashes) is only understood by the engine module.

import type { Header } from "./types";

export const MAGIC = "IBCR";
export const FORMAT_VERSION = 2;
/** Oldest format whose header line this reader understands; the engine build for the replay decides the rest. */
export const OLDEST_FORMAT_VERSION = 1;

export interface Container {
  formatVersion: number;
  /** The raw deflate stream after the 8-byte prefix. */
  deflated: Uint8Array;
}

export function parseContainer(bytes: Uint8Array): Container {
  if (bytes.length < 8 || String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]) !== MAGIC) {
    throw new Error("not an .ibcr replay (bad magic)");
  }
  const formatVersion = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(4, true);
  if (formatVersion < OLDEST_FORMAT_VERSION || formatVersion > FORMAT_VERSION) {
    throw new Error(
      `unsupported replay format version ${formatVersion} (this viewer reads ${OLDEST_FORMAT_VERSION} to ${FORMAT_VERSION})`,
    );
  }
  return { formatVersion, deflated: bytes.subarray(8) };
}

/**
 * Parse header JSON (the first payload line, or the engine's `ibc_header_*` string). `seed` is a u64, which
 * `JSON.parse` would round, so it is kept as its decimal text.
 */
export function parseHeaderJson(text: string): Header {
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new Error(`replay header is invalid: ${(e as Error).message}`);
  }
  if (typeof raw !== "object" || raw === null || typeof raw.engine_version !== "string") {
    throw new Error("replay header is invalid: no engine_version");
  }
  if (typeof raw.map_text !== "string") throw new Error("replay header is invalid: no map_text");
  // A quote inside a JSON string is written `\"`, so this pattern cannot match inside `map_text` or a bot
  // name: it only finds the real key.
  const seed = /(?:^\{|,)"seed":(\d+)/.exec(text);
  return { ...(raw as unknown as Header), seed: seed ? seed[1] : String(raw.seed ?? "0") };
}

/**
 * Inflate only until the header line is complete. Replays of long matches carry a large body the viewer
 * does not need to decode twice (the engine does it again when it loads the file).
 */
export async function readHeader(bytes: Uint8Array): Promise<Header> {
  const { deflated } = parseContainer(bytes);
  const stream = new Blob([deflated as BlobPart]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let line: Uint8Array | null = null;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      const nl = value.indexOf(10);
      if (nl >= 0) {
        chunks.push(value.subarray(0, nl));
        line = concat(chunks);
        break;
      }
      chunks.push(value);
    }
  } catch (e) {
    throw new Error(`replay is corrupt (inflate: ${(e as Error).message})`);
  } finally {
    reader.cancel().catch(() => {});
  }
  if (!line) throw new Error("replay is corrupt (no header line)");
  return parseHeaderJson(new TextDecoder().decode(line));
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}
