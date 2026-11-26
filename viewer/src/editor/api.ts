// The local server's editor routes (`ibc view --editor`, crates/cli/src/view/editor.rs). On any other host
// `detect` returns null and the page hides Lint and Save; opening and exporting files still work.

import type { Finding } from "./check";

export interface LintResult {
  errors: number;
  warnings: number;
  issues: Finding[];
}

export type SaveResult = { ok: true; file: string } | { ok: false; exists: boolean; message: string };

/** Every POST carries this header; the server refuses editor POSTs without it (see the Rust module). */
const HEADERS = { "Content-Type": "text/plain; charset=utf-8", "X-IBC-Editor": "1" };

export class EditorApi {
  constructor(
    readonly maps: string[],
    private readonly base: string,
  ) {}

  /** The server's map list, or null when the page is not served by `ibc view --editor`. */
  static async detect(base: string = document.baseURI): Promise<EditorApi | null> {
    try {
      const res = await fetch(new URL("api/editor", base));
      if (!res.ok || !(res.headers.get("content-type") ?? "").includes("json")) return null;
      const body = (await res.json()) as { maps?: unknown };
      if (!Array.isArray(body.maps)) return null;
      return new EditorApi(body.maps.map(String), base);
    } catch {
      return null;
    }
  }

  mapUrl(name: string): string {
    return new URL(`maps/${encodeURIComponent(name)}.ibcmap`, this.base).href;
  }

  async lint(text: string, name: string): Promise<LintResult> {
    const res = await fetch(new URL(`api/lint?name=${encodeURIComponent(name)}`, this.base), {
      method: "POST",
      headers: HEADERS,
      body: text,
    });
    const body = (await res.json()) as LintResult & { message?: string };
    if (!res.ok) throw new Error(body.message ?? `lint failed (${res.status})`);
    return body;
  }

  async save(text: string, name: string, overwrite: boolean): Promise<SaveResult> {
    const query = `name=${encodeURIComponent(name)}${overwrite ? "&overwrite=1" : ""}`;
    const res = await fetch(new URL(`api/save?${query}`, this.base), { method: "POST", headers: HEADERS, body: text });
    const body = (await res.json().catch(() => ({}))) as { file?: string; error?: string; message?: string };
    if (res.ok) return { ok: true, file: body.file ?? `${name}.ibcmap` };
    return { ok: false, exists: body.error === "exists", message: body.message ?? `save failed (${res.status})` };
  }
}

/** A map name the server accepts and the `name:` header should carry: letters, digits, `_`, `-`. */
export function validMapName(name: string): boolean {
  return /^[A-Za-z0-9_-]{1,64}$/.test(name) && !/^(con|prn|aux|nul|com[0-9]|lpt[0-9])$/i.test(name);
}
