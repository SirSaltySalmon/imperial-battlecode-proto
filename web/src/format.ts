// Pure text helpers behind the pages: error messages, ladder labels, match results, links.

import { ApiError, type LadderRow, type Match, type Pairing } from "./api";

/** "45 s", "3 min", "2 h 5 min". */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.ceil(seconds));
  if (s < 60) return `${s} s`;
  const minutes = Math.ceil(s / 60);
  if (minutes < 60) return `${minutes} min`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/** What to tell a person about a failed call. */
export function errorText(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.unavailable) return "Not available yet: this server does not offer that feature.";
    if (e.status === 429) {
      const wait = e.retryAfter !== null ? `Try again in ${formatDuration(e.retryAfter)}.` : "Try again later.";
      return `${e.message.replace(/\.$/, "")}. ${wait}`;
    }
    if (e.status === 401) return `Not signed in (${e.message}).`;
    return e.message;
  }
  if (e instanceof Error) return e.message;
  return String(e);
}

/** The tier column: provisional teams show that instead of a tier. */
export function tierLabel(row: Pick<LadderRow, "tier" | "provisional">): string {
  if (row.provisional) return "Provisional";
  return row.tier ?? "—";
}

export function ratingText(row: Pick<LadderRow, "rating">): string {
  return row.rating === null ? "—" : String(Math.round(row.rating));
}

export function recordText(row: Pick<LadderRow, "wins" | "losses" | "draws">): string {
  if (row.wins === undefined) return "";
  return `${row.wins}-${row.losses ?? 0}-${row.draws ?? 0}`;
}

/** "2026-11-25 14:03" in local time; the input as is if it is not a date. */
export function formatTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
}

/**
 * The replay of a match as the API serves it (same origin, so the viewer can fetch it). `view: "team"` keeps
 * the signed-in team's own memory and logs; leave it out for the public view.
 */
export function replayPath(matchId: string, view?: "team"): string {
  const base = `/v1/matches/${encodeURIComponent(matchId)}/replay`;
  return view ? `${base}?view=${view}` : base;
}

/** The embedded viewer opened on a match. */
export function viewerHref(matchId: string, view?: "team"): string {
  const path = replayPath(matchId, view);
  return `/viewer/?replay=${view ? encodeURIComponent(path) : path}`;
}

/** "7–4", or the state while the match is not done. */
export function scoreText(m: Pick<Match, "state" | "score">): string {
  if (m.state !== "done" || !m.score) return m.state;
  return `${m.score[0]}–${m.score[1]}`;
}

/** The result from one team's side: "won", "lost", "draw", or "" when it is not that team's match or not done. */
export function resultFor(m: Pick<Match, "state" | "outcome" | "bot_a" | "bot_b">, teamId: string): string {
  if (m.state !== "done") return "";
  const side = m.bot_a.team_id === teamId ? "a" : m.bot_b.team_id === teamId ? "b" : null;
  if (side === null) return "";
  if (m.outcome === "draw") return "draw";
  if (m.outcome === "a" || m.outcome === "b") return m.outcome === side ? "won" : "lost";
  return "";
}

/** A pairing side's display name: the name if the server sent one, else the id, else "to be decided" while the
 * seat waits on an earlier match, else "bye". */
export function pairingSide(p: Pairing, side: "a" | "b"): string {
  const name = side === "a" ? p.a_name : p.b_name;
  const id = side === "a" ? p.a : p.b;
  return name ?? id ?? (p.state === "waiting" ? "to be decided" : "bye");
}

/** "2–1 maps", "rounds 25–24", "seed": how the winner of a pairing was decided. */
export function decidedText(p: Pairing): string {
  switch (p.decided_by) {
    case "maps":
      return p.maps_won ? `maps ${p.maps_won[0]}–${p.maps_won[1]}` : "maps";
    case "rounds":
      return p.rounds_won ? `rounds ${p.rounds_won[0]}–${p.rounds_won[1]}` : "rounds";
    case "seed":
      return "higher seed (level on maps and rounds)";
    default:
      return "";
  }
}

/** The organisers review every pairing a seed decided. */
export function isFlagged(p: Pairing): boolean {
  return p.decided_by === "seed";
}

/** Columns for a table of objects whose shape is not fixed: every key, in first-seen order. */
export function objectColumns(rows: readonly Record<string, unknown>[]): string[] {
  const seen = new Set<string>();
  for (const row of rows) for (const k of Object.keys(row)) seen.add(k);
  return [...seen];
}

/** One cell of such a table. */
export function cellText(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  return JSON.stringify(v);
}

/** Odd and non-empty: a tournament pairing must not end level on maps. */
export function checkTournamentMaps(maps: readonly string[]): string | null {
  if (maps.length === 0) return "Pick at least one map.";
  if (maps.length % 2 === 0) return "Use an odd number of maps, so a pairing cannot end level on maps.";
  return null;
}

/** A link target from the server, if it is http(s) or same-origin; `null` for anything else (`javascript:`). */
export function safeUrl(url: string): string | null {
  if (url.startsWith("/") && !url.startsWith("//")) return url;
  return /^https?:\/\//i.test(url) ? url : null;
}

/** "a, b ,, c" -> ["a", "b", "c"] */
export function splitList(s: string): string[] {
  return s
    .split(/[,\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);
}
