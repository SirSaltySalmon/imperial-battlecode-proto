// Hash routing: `#/teams/abc?x=1`. The coordinator also answers `index.html` for plain paths such as
// `/device?code=…` (the URL the CLI prints), which `pathRoute` turns into the matching hash route.

export interface Location {
  path: string;
  query: URLSearchParams;
}

/** `#/a/b?x=1` -> `{path: "/a/b", query: x=1}`. An empty hash is `/`. */
export function parseHash(hash: string): Location {
  const raw = hash.replace(/^#/, "");
  const q = raw.indexOf("?");
  const pathPart = q < 0 ? raw : raw.slice(0, q);
  const query = new URLSearchParams(q < 0 ? "" : raw.slice(q + 1));
  let path = "/" + pathPart.replace(/^\/+/, "").replace(/\/+$/, "");
  if (path === "") path = "/";
  return { path, query };
}

/** Match `/teams/:id` against `/teams/abc` -> `{id: "abc"}`; `null` if it does not match. */
export function matchPattern(pattern: string, path: string): Record<string, string> | null {
  const want = pattern.split("/").filter(Boolean);
  const got = path.split("/").filter(Boolean);
  if (want.length !== got.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < want.length; i++) {
    const w = want[i]!;
    const g = got[i]!;
    if (w.startsWith(":")) {
      try {
        params[w.slice(1)] = decodeURIComponent(g);
      } catch {
        return null;
      }
    } else if (w !== g) {
      return null;
    }
  }
  return params;
}

export interface RouteMatch<T> {
  route: T;
  params: Record<string, string>;
}

/** The first route whose pattern matches. */
export function resolve<T extends { pattern: string }>(routes: readonly T[], path: string): RouteMatch<T> | null {
  for (const route of routes) {
    const params = matchPattern(route.pattern, path);
    if (params) return { route, params };
  }
  return null;
}

/** A link to a route: `href("/matches", {kind: "ranked"})` -> `#/matches?kind=ranked`. */
export function href(path: string, query: Record<string, string | undefined | null> = {}): string {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== null && v !== "") params.set(k, v);
  }
  const segments = path.split("/").filter(Boolean).map(encodeURIComponent);
  const qs = params.toString();
  return `#/${segments.join("/")}${qs ? `?${qs}` : ""}`;
}

/**
 * The hash route for a page opened at a plain path (`/device?code=X` -> `#/device?code=X`), or `null` when
 * the page was opened at `/` or already carries a hash route.
 */
export function pathRoute(pathname: string, search: string, hash: string): string | null {
  if (hash.startsWith("#/")) return null;
  const path = pathname.replace(/\/+$/, "");
  if (path === "" || path === "/index.html") return null;
  return `#${path}${search}`;
}
