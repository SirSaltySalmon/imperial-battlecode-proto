// Page wiring: the session, the navigation bar, and rendering the route in the address bar.

import { Api } from "./api";
import { errorBox, h, link } from "./dom";
import { admin } from "./pages/admin";
import { device, signIn, signUp } from "./pages/auth";
import { home } from "./pages/home";
import { ladder, teamPage } from "./pages/ladder";
import { matches, series } from "./pages/matches";
import { scrims } from "./pages/scrims";
import { submissions } from "./pages/submissions";
import { team } from "./pages/team";
import { tournament, tournaments } from "./pages/tournaments";
import { href, parseHash, pathRoute, resolve } from "./router";
import { SIGNED_OUT, displayName, loadSession, tokenStore, type Ctx, type Page, type Session } from "./session";

interface Route {
  pattern: string;
  title: string;
  page: Page;
}

const ROUTES: Route[] = [
  { pattern: "/", title: "Home", page: home },
  { pattern: "/signin", title: "Sign in", page: signIn },
  { pattern: "/signup", title: "Sign up", page: signUp },
  { pattern: "/device", title: "CLI sign-in", page: device },
  { pattern: "/team", title: "Team", page: team },
  { pattern: "/teams/:id", title: "Team", page: teamPage },
  { pattern: "/submissions", title: "Submissions", page: submissions },
  { pattern: "/ladder", title: "Ladder", page: ladder },
  { pattern: "/matches", title: "Matches", page: matches },
  { pattern: "/series/:id", title: "Series", page: series },
  { pattern: "/scrims", title: "Scrims", page: scrims },
  { pattern: "/tournaments", title: "Tournaments", page: tournaments },
  { pattern: "/tournaments/:id", title: "Tournament", page: tournament },
  { pattern: "/admin", title: "Organisers", page: admin },
];

const NAV: [string, string, (s: Session) => boolean][] = [
  ["/ladder", "Ladder", () => true],
  ["/matches", "Matches", () => true],
  ["/tournaments", "Tournaments", () => true],
  ["/team", "Team", (s) => s.via !== "none"],
  ["/submissions", "Submissions", (s) => s.team !== null],
  ["/scrims", "Scrims", (s) => s.team !== null],
  ["/admin", "Organisers", (s) => s.isAdmin],
];

const api = new Api({ token: () => tokenStore.get() });
let session: Session = SIGNED_OUT;
let renderSeq = 0;

const $ = (id: string) => document.getElementById(id)!;

function go(hash: string): void {
  if (location.hash === hash) void render();
  else location.hash = hash;
}

async function refresh(): Promise<void> {
  session = await loadSession(api);
  renderChrome();
  await render();
}

async function signOut(): Promise<void> {
  if (session.via === "token") tokenStore.clear();
  else await api.logout();
  await refresh();
  go("#/");
}

function renderChrome(): void {
  const current = parseHash(location.hash).path;
  $("nav").replaceChildren(
    ...NAV.filter(([, , show]) => show(session)).map(([path, label]) =>
      link(href(path), label, { class: current === path || current.startsWith(`${path}/`) ? "active" : null }),
    ),
  );
  const account = $("account");
  if (session.via === "none") {
    account.replaceChildren(link(href("/signin"), "Sign in"), " ", link(href("/signup"), "Sign up"));
    return;
  }
  const out = h("button", { type: "button", class: "link" }, "Sign out");
  out.addEventListener("click", () => {
    signOut().catch((e) => $("page").prepend(errorBox(e)));
  });
  account.replaceChildren(
    h("span", { class: "who" }, displayName(session), session.via === "token" ? " (token)" : ""),
    " ",
    out,
  );
}

async function render(): Promise<void> {
  const seq = ++renderSeq;
  const loc = parseHash(location.hash);
  const page = $("page");
  renderChrome();
  const found = resolve(ROUTES, loc.path);
  if (!found) {
    page.replaceChildren(h("h1", {}, "Not found"), h("p", {}, "No such page. ", link("#/", "Home")));
    return;
  }
  document.title = `${found.route.title} · Imperial Battlecode`;
  const ctx: Ctx = { api, session, params: found.params, query: loc.query, refresh, go };
  try {
    const node = await found.route.page(ctx);
    if (seq === renderSeq) page.replaceChildren(node);
  } catch (e) {
    if (seq === renderSeq) page.replaceChildren(errorBox(e));
  }
}

async function start(): Promise<void> {
  // `/device?code=X` (the URL the CLI prints) -> `/#/device?code=X`.
  const fromPath = pathRoute(location.pathname, location.search, location.hash);
  if (fromPath) history.replaceState(null, "", `/${fromPath}`);
  window.addEventListener("hashchange", () => void render());
  try {
    session = await loadSession(api);
  } catch (e) {
    session = SIGNED_OUT;
    await render();
    $("page").prepend(errorBox(e));
    return;
  }
  await render();
}

void start();
