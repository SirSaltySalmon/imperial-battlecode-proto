// Match lists (with filters and paging), the series table other pages reuse, and a series page.

import type { Match, MatchKind, MatchQuery } from "../api";
import { h, errorBox, link, notice, section } from "../dom";
import {
  formatTime,
  groupSeries,
  replayPath,
  resultFor,
  scoreText,
  seriesResultFor,
  seriesScoreText,
  viewerHref,
  type SeriesSummary,
} from "../format";
import { href } from "../router";
import type { Page } from "../session";

const KINDS: MatchKind[] = ["ranked", "unranked", "tournament"];

function teamLink(t: { team_id: string; name: string }): HTMLElement {
  return link(href(`/teams/${t.team_id}`), t.name);
}

/** Watch and download links; a match the signed-in team played opens in that team's view. */
function matchLinks(m: Match, ownTeam?: string): HTMLElement {
  const played = ownTeam !== undefined && [m.bot_a.team_id, m.bot_b.team_id].includes(ownTeam);
  const view = played ? "team" : undefined;
  return h(
    "span",
    { class: "links" },
    link(viewerHref(m.match_id, view), "Watch"),
    " · ",
    link(replayPath(m.match_id, view), "Download", { download: `${m.match_id}.ibcr` }),
  );
}

/** One line per game: "Game 2 · crossfire · Alpha 7–4 Beta · won · Watch · Download". */
export function gameList(matches: Match[], perspective?: string, ownTeam?: string): HTMLElement {
  if (matches.length === 0) return h("p", { class: "muted" }, "No games yet.");
  return h(
    "ul",
    { class: "games" },
    matches.map((m) => {
      const result = perspective ? resultFor(m, perspective) : "";
      return h(
        "li",
        {},
        h("strong", {}, `Game ${m.index + 1}`),
        ` · ${m.map} · `,
        teamLink(m.bot_a),
        ` ${scoreText(m)} `,
        teamLink(m.bot_b),
        result ? ` · ${result}` : "",
        m.state === "done" ? [" · ", matchLinks(m, ownTeam)] : null,
        h("span", { class: "muted" }, ` · ${formatTime(m.finished_at ?? m.created_at)}`),
      );
    }),
  );
}

/** A series row, and the hidden row of its games that clicking it shows. */
function seriesRows(s: SeriesSummary, columns: number, perspective?: string, ownTeam?: string): HTMLElement[] {
  const toggle = h("button", { type: "button", class: "link toggle", "aria-expanded": "false" }, "▸");
  const cells: (Node | string)[] = [
    toggle,
    formatTime(s.when),
    s.kind,
    teamLink(s.team_a),
    seriesScoreText(s),
    teamLink(s.team_b),
  ];
  if (perspective) cells.push(seriesResultFor(s, perspective));
  cells.push(String(s.matches.length));
  const row = h("tr", { class: "series-row" }, cells.map((c) => h("td", {}, c)));
  const games = h(
    "tr",
    { class: "series-games", hidden: true },
    h(
      "td",
      { colspan: columns },
      gameList(s.matches, perspective, ownTeam),
      link(href(`/series/${s.series_id}`), "Series page"),
    ),
  );
  row.addEventListener("click", (ev) => {
    if ((ev.target as Element).closest("a")) return;
    const open = games.hidden;
    games.hidden = !open;
    toggle.setAttribute("aria-expanded", String(open));
    toggle.textContent = open ? "▾" : "▸";
  });
  return [row, games];
}

/**
 * One row per series; clicking a row lists its games. With `perspective` (a team id) a result column says
 * won / lost / draw. `ownTeam` is the signed-in team: its matches link to the team view of the replay.
 */
export function seriesTable(matches: Match[], perspective?: string, ownTeam?: string): HTMLElement {
  const all = groupSeries(matches);
  if (all.length === 0) return h("p", { class: "muted" }, "No matches yet.");
  const headers = ["", "When", "Kind", "Side A", "Series", "Side B"];
  if (perspective) headers.push("Result");
  headers.push("Games");
  return h(
    "table",
    { class: "series" },
    h("thead", {}, h("tr", {}, headers.map((t) => h("th", {}, t)))),
    h("tbody", {}, all.map((s) => seriesRows(s, headers.length, perspective, ownTeam))),
  );
}

function select(name: string, options: [string, string][], value: string): HTMLSelectElement {
  const el = h("select", { name });
  for (const [v, label] of options) el.add(new Option(label, v, false, v === value));
  return el;
}

/** `#/matches?scope=mine|all&kind=…` */
export const matches: Page = (ctx) => {
  const myTeam = ctx.session.team;
  const scope = myTeam && ctx.query.get("scope") !== "all" ? "mine" : "all";
  const kindParam = ctx.query.get("kind") ?? "";
  const kind = (KINDS as string[]).includes(kindParam) ? (kindParam as MatchKind) : undefined;
  const go = (q: Record<string, string>) => ctx.go(href("/matches", { scope, kind, ...q }));

  const scopeSel = select(
    "scope",
    myTeam ? [["mine", "My team"], ["all", "All teams"]] : [["all", "All teams"]],
    scope,
  );
  scopeSel.addEventListener("change", () => go({ scope: scopeSel.value }));
  const kindSel = select("kind", [["", "All kinds"], ...KINDS.map((k): [string, string] => [k, k])], kind ?? "");
  kindSel.addEventListener("change", () => go({ kind: kindSel.value }));

  const list = h("div", {});
  const more = h("button", { type: "button", hidden: true }, "Older matches");
  const query: MatchQuery = { team: scope === "mine" ? myTeam?.id : undefined, kind, limit: 50 };
  const shown: Match[] = [];
  const load = async () => {
    more.disabled = true;
    try {
      const page = await ctx.api.matches(query);
      // A server that ignores `kind` still gets the filter.
      shown.push(...page.matches.filter((m) => !kind || m.kind === kind));
      list.replaceChildren(seriesTable(shown, scope === "mine" ? myTeam?.id : undefined, myTeam?.id));
      query.before = page.next ?? undefined;
      more.hidden = page.next === null;
    } catch (e) {
      list.append(errorBox(e));
    } finally {
      more.disabled = false;
    }
  };
  more.addEventListener("click", () => void load());
  list.append(h("p", { class: "muted" }, "Loading…"));
  void load();
  return h(
    "div",
    {},
    h("h1", {}, "Matches"),
    h("div", { class: "filters" }, scopeSel, kindSel, myTeam ? link(href("/scrims"), "Request a scrim") : null),
    list,
    more,
  );
};

/** `#/series/:id`: the games of one series, and a rematch against the same opponent version. */
export const series: Page = (ctx) => {
  const id = ctx.params.id!;
  return h(
    "div",
    {},
    section(`Series ${id}`, async () => {
      const s = await ctx.api.series(id);
      const me = ctx.session.team?.id;
      const opponent = me === s.team_a.team_id ? s.team_b : me === s.team_b.team_id ? s.team_a : null;
      return [
        h(
          "p",
          {},
          teamLink(s.team_a),
          ` ${s.wins_a}–${s.wins_b} `,
          teamLink(s.team_b),
          s.draws ? ` (${s.draws} drawn)` : "",
          ` · ${s.kind} · ${s.games} games · ${s.state}`,
        ),
        s.note ? notice(s.note) : null,
        opponent
          ? h(
              "p",
              {},
              link(
                href("/scrims", { opponent: opponent.team_id, against: s.series_id }),
                `Rematch ${opponent.name}'s version from this series`,
              ),
            )
          : null,
        gameList(s.matches, me, me),
      ];
    }),
  );
};
