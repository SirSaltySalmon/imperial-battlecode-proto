// The typed client of the coordinator's `/v1` API, and the contract types the pages use.
//
// Auth is the session cookie (`credentials: "same-origin"`, `X-IBC-CSRF: 1` on every write) or, for
// organisers and development, a bearer token kept for the browser tab. Every failure becomes an `ApiError`
// carrying the server's `{error, message}`, the retry time of a 429, and whether the endpoint exists at all.

// ---- contract types ----

export interface User {
  id: string;
  email: string;
  display_name: string;
  is_admin: boolean;
}

export interface TeamRef {
  id: string;
  name: string;
  join_code?: string | null;
}

/** `GET me` */
export interface MeResponse {
  user: User | null;
  team: TeamRef | null;
}

export interface AuthResponse {
  user: User;
  session_token: string;
}

export interface Member {
  id?: string;
  display_name?: string;
  email?: string;
}

/** `GET team`. `join_code`, `members` and the live-status fields appear once the server sends them. */
export interface TeamInfo {
  team_id: string;
  name: string;
  live_submission: string | null;
  join_code?: string | null;
  members?: Member[];
  live_status?: string | null;
  grace_until?: string | null;
}

/** `GET team` with the organisers' token. */
export interface AdminWhoami {
  admin: true;
}

export interface CreateTeamResponse {
  team: TeamRef;
  join_code: string;
}

export type Language = "rust" | "cpp";

export interface Submission {
  submission_id: string;
  sha256: string;
  size: number;
  language: string;
  fuel_mult: number;
  status: string;
  reason: string | null;
  live: boolean;
  created_at: string;
  note?: string | null;
}

export interface UploadResponse {
  submission_id: string;
  team_id: string;
  sha256: string;
  status: string;
  live: boolean;
  language: string;
  fuel_mult: number;
}

export interface LadderRow {
  rank: number;
  team_id: string;
  team_name: string;
  rating: number | null;
  tier: string | null;
  provisional: boolean;
  games: number;
  wins?: number;
  losses?: number;
  draws?: number;
}

export interface MatchTeam {
  team_id: string;
  name: string;
}

export type MatchKind = "ranked" | "unranked" | "tournament";

export interface Match {
  match_id: string;
  series_id: string;
  index: number;
  kind: string;
  map: string;
  seed: number;
  bot_a: MatchTeam;
  bot_b: MatchTeam;
  state: string;
  winner: number | null;
  winner_team: string | null;
  outcome: "a" | "b" | "draw" | "unfinished" | null;
  score: [number, number] | null;
  ticks: number | null;
  replay_url: string | null;
  verified: boolean;
  created_at: string;
  finished_at: string | null;
}

export interface MatchPage {
  matches: Match[];
  /** The `before` of the next (older) page, if there may be one. */
  next: string | null;
}

export interface MatchQuery {
  team?: string;
  kind?: MatchKind;
  before?: string;
  limit?: number;
}

export interface SeriesTeam extends MatchTeam {
  submission_id?: string;
}

export interface Series {
  series_id: string;
  kind: string;
  team_a: SeriesTeam;
  team_b: SeriesTeam;
  games: number;
  state: string;
  wins_a: number;
  wins_b: number;
  draws: number;
  note: string | null;
  created_at: string;
  finished_at: string | null;
  matches: Match[];
}

export interface ScrimRequest {
  opponent_team_id: string;
  games: number;
  maps?: string[];
  my_submission_id?: string;
  against_series_id?: string;
}

export interface MapInfo {
  name: string;
  sha256: string;
  in_pool: boolean;
}

export type DecidedBy = "maps" | "rounds" | "seed";

export interface Pairing {
  a: string | null;
  b: string | null;
  a_name?: string | null;
  b_name?: string | null;
  seed_a: number | null;
  seed_b: number | null;
  maps_won: [number, number] | null;
  rounds_won: [number, number] | null;
  winner: string | null;
  decided_by: DecidedBy | null;
  /** `waiting`: a seat is still to be decided; `bye`: one seat is empty; `empty`: nobody plays it (an unneeded
   * bracket reset, or two byes). Absent from servers that predate it. */
  state?: "waiting" | "running" | "done" | "bye" | "empty";
}

export interface TournamentSummary {
  id: string;
  name: string;
  format: "single" | "double";
  status: string;
}

export interface Tournament extends TournamentSummary {
  /** `name` (e.g. "Grand final reset") is absent from servers that predate it. */
  rounds: { name?: string; pairings: Pairing[] }[];
  maps: string[] | null;
}

export type TournamentField =
  | { kind: "all" }
  | { kind: "top"; n: number }
  | { kind: "list"; team_ids: string[] };

export interface TournamentCreate {
  name: string;
  format: "single" | "double";
  field: TournamentField;
  maps: string[];
}

export type ModerationAction = "suspend" | "ban" | "unban";

export interface ModerationRequest {
  reason: string;
  until?: string;
}

export interface DeviceApproveResponse {
  approved?: boolean;
}

export interface Distribution {
  version: string;
  patch: string | null;
  artifacts: { target: string; url: string; sha256: string }[];
}

// ---- errors ----

export class ApiError extends Error {
  constructor(
    readonly status: number,
    /** The server's `error` code, or `""` when the answer had no API error body. */
    readonly code: string,
    message: string,
    /** Seconds to wait before retrying (429 only). */
    readonly retryAfter: number | null,
  ) {
    super(message);
    this.name = "ApiError";
  }

  /** The server has no such endpoint (not deployed or not merged yet), as opposed to "no such thing". */
  get unavailable(): boolean {
    return (
      (this.status === 404 && (this.code === "" || this.code === "no_route")) || this.status === 405 || this.status === 501
    );
  }
}

/** `Retry-After` as seconds: a number of seconds or an HTTP date. */
export function parseRetryAfter(header: string | null, now = Date.now()): number | null {
  if (header === null || header.trim() === "") return null;
  const secs = Number(header.trim());
  if (Number.isFinite(secs)) return Math.max(0, Math.ceil(secs));
  const at = Date.parse(header);
  return Number.isNaN(at) ? null : Math.max(0, Math.ceil((at - now) / 1000));
}

/** The `ApiError` for a failed response, from its status, `Retry-After` header and body text. */
export function toApiError(status: number, retryAfterHeader: string | null, bodyText: string): ApiError {
  let code = "";
  let message = "";
  let retry = parseRetryAfter(retryAfterHeader);
  try {
    const body: unknown = JSON.parse(bodyText);
    if (body && typeof body === "object") {
      const b = body as Record<string, unknown>;
      if (typeof b.error === "string") code = b.error;
      if (typeof b.message === "string") message = b.message;
      if (retry === null && typeof b.retry_after === "number") retry = Math.max(0, Math.ceil(b.retry_after));
    }
  } catch {
    // Not the API's JSON (a proxy page, an empty 404): keep the status line.
  }
  if (message === "") message = code !== "" ? code : `HTTP ${status}`;
  return new ApiError(status, code, message, status === 429 ? retry : null);
}

// ---- normalisers (tolerate the shapes the server has sent while the contract settled) ----

function asRecord(v: unknown): Record<string, unknown> {
  return v && typeof v === "object" ? (v as Record<string, unknown>) : {};
}

function numOrNull(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

/** `GET ladder`: the contract's array, or the older `{teams:[{name,...}]}` snapshot. */
export function normalizeLadder(raw: unknown): LadderRow[] {
  const list = Array.isArray(raw) ? raw : asRecord(raw).teams;
  if (!Array.isArray(list)) return [];
  return list.map((item, i) => {
    const r = asRecord(item);
    const name = r.team_name ?? r.name;
    return {
      rank: numOrNull(r.rank) ?? i + 1,
      team_id: String(r.team_id ?? ""),
      team_name: typeof name === "string" ? name : String(r.team_id ?? ""),
      rating: numOrNull(r.rating),
      tier: typeof r.tier === "string" ? r.tier : null,
      provisional: r.provisional === true,
      games: numOrNull(r.games) ?? 0,
      wins: numOrNull(r.wins) ?? undefined,
      losses: numOrNull(r.losses) ?? undefined,
      draws: numOrNull(r.draws) ?? undefined,
    };
  });
}

/** `GET matches`: `{matches, next}` or a bare array. */
export function normalizeMatches(raw: unknown): MatchPage {
  if (Array.isArray(raw)) return { matches: raw as Match[], next: null };
  const r = asRecord(raw);
  const matches = Array.isArray(r.matches) ? (r.matches as Match[]) : [];
  return { matches, next: typeof r.next === "string" ? r.next : null };
}

/** A list endpoint that answers `[...]`, `{<key>: [...]}`, or an object with one list in it. */
export function listOf<T>(raw: unknown, key: string): T[] {
  if (Array.isArray(raw)) return raw as T[];
  const r = asRecord(raw);
  const list = Array.isArray(r[key]) ? r[key] : Object.values(r).find(Array.isArray);
  return Array.isArray(list) ? (list as T[]) : [];
}

// ---- client ----

export type Method = "GET" | "POST" | "PUT" | "DELETE";

export interface RequestOptions {
  query?: Record<string, string | number | boolean | undefined | null>;
  json?: unknown;
  body?: Blob | ArrayBuffer | Uint8Array;
  contentType?: string;
}

export interface ClientOptions {
  /** Prefix of every path; `/v1` by default (same origin). */
  base?: string;
  fetch?: typeof fetch;
  /** The bearer token to send, if the tab signed in with one. */
  token?: () => string | null;
}

/** `/v1/<path>?<query>`, skipping empty query values. */
export function buildUrl(base: string, path: string, query?: RequestOptions["query"]): string {
  const url = `${base}/${path.replace(/^\/+/, "")}`;
  if (!query) return url;
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== null && v !== "") params.set(k, String(v));
  }
  const qs = params.toString();
  return qs ? `${url}?${qs}` : url;
}

export class Api {
  private readonly base: string;
  private readonly doFetch: typeof fetch;
  private readonly token: () => string | null;

  constructor(opts: ClientOptions = {}) {
    this.base = opts.base ?? "/v1";
    this.doFetch = opts.fetch ?? ((input, init) => fetch(input, init));
    this.token = opts.token ?? (() => null);
  }

  get hasToken(): boolean {
    return this.token() !== null;
  }

  async request<T>(method: Method, path: string, opts: RequestOptions = {}): Promise<T> {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (method !== "GET") headers["X-IBC-CSRF"] = "1";
    const token = this.token();
    if (token) headers.Authorization = `Bearer ${token}`;
    let body: BodyInit | undefined;
    if (opts.json !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(opts.json);
    } else if (opts.body !== undefined) {
      headers["Content-Type"] = opts.contentType ?? "application/octet-stream";
      body = opts.body as BodyInit;
    }
    const res = await this.doFetch(buildUrl(this.base, path, opts.query), {
      method,
      headers,
      body,
      credentials: "same-origin",
    });
    const text = await res.text();
    if (!res.ok) throw toApiError(res.status, res.headers.get("Retry-After"), text);
    if (text === "") return undefined as T;
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new ApiError(res.status, "bad_response", `the server answered ${path} with something that is not JSON`, null);
    }
  }

  // auth
  me(): Promise<MeResponse> {
    return this.request("GET", "me");
  }
  register(email: string, password: string, display_name: string): Promise<AuthResponse> {
    return this.request("POST", "auth/register", { json: { email, password, display_name } });
  }
  login(email: string, password: string): Promise<AuthResponse> {
    return this.request("POST", "auth/login", { json: { email, password } });
  }
  logout(): Promise<void> {
    return this.request("POST", "auth/logout");
  }
  approveDevice(user_code: string): Promise<DeviceApproveResponse> {
    return this.request("POST", "auth/device/approve", { json: { user_code } });
  }

  // teams
  team(): Promise<TeamInfo | AdminWhoami> {
    return this.request("GET", "team");
  }
  createTeam(name: string): Promise<CreateTeamResponse> {
    return this.request("POST", "teams", { json: { name } });
  }
  joinTeam(join_code: string): Promise<unknown> {
    return this.request("POST", "teams/join", { json: { join_code } });
  }
  leaveTeam(): Promise<unknown> {
    return this.request("POST", "teams/leave");
  }

  // submissions
  async submissions(): Promise<Submission[]> {
    return listOf<Submission>(await this.request("GET", "submissions"), "submissions");
  }
  upload(wasm: Blob, language: Language, live: boolean, note?: string): Promise<UploadResponse> {
    return this.request("POST", "submissions", {
      query: { language, live, note: note?.trim() || undefined },
      body: wasm,
      contentType: "application/wasm",
    });
  }
  setLive(submissionId: string): Promise<unknown> {
    return this.request("POST", `submissions/${encodeURIComponent(submissionId)}/live`);
  }

  // league
  async ladder(): Promise<LadderRow[]> {
    return normalizeLadder(await this.request("GET", "ladder"));
  }
  async matches(q: MatchQuery = {}): Promise<MatchPage> {
    return normalizeMatches(await this.request("GET", "matches", { query: { ...q } }));
  }
  series(id: string): Promise<Series> {
    return this.request("GET", `series/${encodeURIComponent(id)}`);
  }
  scrim(req: ScrimRequest): Promise<Partial<Series>> {
    return this.request("POST", "scrims", { json: req });
  }
  async maps(): Promise<MapInfo[]> {
    return listOf<MapInfo>(await this.request("GET", "maps"), "maps");
  }

  // tournaments
  async tournaments(): Promise<TournamentSummary[]> {
    return listOf<TournamentSummary>(await this.request("GET", "tournaments"), "tournaments");
  }
  tournament(id: string): Promise<Tournament> {
    return this.request("GET", `tournaments/${encodeURIComponent(id)}`);
  }
  createTournament(t: TournamentCreate): Promise<TournamentSummary> {
    return this.request("POST", "admin/tournaments", { json: t });
  }
  tournamentAction(id: string, action: "lock" | "start" | "reveal"): Promise<unknown> {
    return this.request("POST", `admin/tournaments/${encodeURIComponent(id)}/${action}`);
  }

  // moderation
  adminOverview(): Promise<Record<string, unknown>> {
    return this.request("GET", "admin/overview");
  }
  async adminFlags(): Promise<Record<string, unknown>[]> {
    return listOf<Record<string, unknown>>(await this.request("GET", "admin/flags"), "flags");
  }
  async adminAudit(): Promise<Record<string, unknown>[]> {
    return listOf<Record<string, unknown>>(await this.request("GET", "admin/audit"), "entries");
  }
  moderate(teamId: string, action: ModerationAction, req: ModerationRequest): Promise<unknown> {
    return this.request("POST", `admin/teams/${encodeURIComponent(teamId)}/${action}`, { json: req });
  }

  distribution(): Promise<Distribution> {
    return this.request("GET", "distribution");
  }
}
