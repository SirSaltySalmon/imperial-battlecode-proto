// Who is signed in, and what every page is handed.

import { Api, ApiError, type TeamRef, type User } from "./api";

export interface Session {
  user: User | null;
  team: TeamRef | null;
  isAdmin: boolean;
  /** `session`: the cookie; `token`: a bearer token pasted into this tab; `none`: signed out. */
  via: "session" | "token" | "none";
}

export const SIGNED_OUT: Session = { user: null, team: null, isAdmin: false, via: "none" };

export interface Ctx {
  api: Api;
  session: Session;
  params: Record<string, string>;
  query: URLSearchParams;
  /** Reload the session (after signing in or out, or a team change) and re-render. */
  refresh(): Promise<void>;
  /** Go to a hash route (`#/team`). */
  go(hash: string): void;
}

export type Page = (ctx: Ctx) => Node | Promise<Node>;

/**
 * The session: `GET me` (cookie or user token). A team or organiser token has no user, so when `me` does not
 * answer for a token, `GET team` says which team (or that it is the organisers).
 */
export async function loadSession(api: Api): Promise<Session> {
  try {
    const me = await api.me();
    if (me.user || me.team) {
      return { user: me.user, team: me.team, isAdmin: me.user?.is_admin === true, via: api.hasToken ? "token" : "session" };
    }
  } catch (e) {
    const expected = e instanceof ApiError && (e.status === 401 || e.status === 403 || e.unavailable);
    if (!expected) throw e;
  }
  if (!api.hasToken) return SIGNED_OUT;
  const t = await api.team();
  if ("admin" in t) return { user: null, team: null, isAdmin: true, via: "token" };
  return {
    user: null,
    team: { id: t.team_id, name: t.name, join_code: t.join_code ?? null },
    isAdmin: false,
    via: "token",
  };
}

const TOKEN_KEY = "ibc-token";
let inMemory: string | null = null;

/**
 * The bearer token of this tab (organisers and development). Kept per tab, never across browser restarts;
 * with storage blocked it lasts until the page reloads.
 */
export const tokenStore = {
  get(): string | null {
    try {
      return sessionStorage.getItem(TOKEN_KEY) ?? inMemory;
    } catch {
      return inMemory;
    }
  },
  set(token: string): void {
    inMemory = token;
    try {
      sessionStorage.setItem(TOKEN_KEY, token);
    } catch {
      // Kept in memory only.
    }
  },
  clear(): void {
    inMemory = null;
    try {
      sessionStorage.removeItem(TOKEN_KEY);
    } catch {
      // Nothing stored.
    }
  },
};

export function displayName(s: Session): string {
  if (s.user) return s.user.display_name || s.user.email;
  if (s.team) return `team ${s.team.name}`;
  if (s.isAdmin) return "organisers";
  return "";
}
