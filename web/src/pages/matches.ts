// Match lists (with filters and paging), the match table other pages reuse, and a series page.

import type { Match, MatchKind, MatchQuery } from "../api";
import { h, errorBox, link, notice, section, table } from "../dom";
import { formatTime, replayPath, resultFor, scoreText, viewerHref } from "../format";
import { href } from "../router";
import type { Page } from "../session";

const KINDS: MatchKind[] = ["ranked", "unranked", "tournament"];

function teamLink(t: { team_id: string; name: string }): HTMLElement {
  return link(href(`/teams/${t.team_id}`), t.name);
}

/** Watch and download links; a match the signed-in team played opens in that team's view. */
function matchLinks(m: Match, ownTeam?: string): HTMLElement {
  if (m.state !== "done") return h("span", { class: "muted" }, "—");
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

/**
 * One row per match. With `perspective` (a team id) a result column says won / lost / draw. `ownTeam` is the
 * signed-in team: its matches link to the team view of the replay.
 */
export function matchTable(matches: Match[], perspective?: string, ownTeam?: string): HTMLElement {
  const headers = ["When", "Kind", "Map", "Side A", "Score", "Side B"];
  if (perspective) headers.push("Result");
  headers.push("Series", "Replay");
  const rows = matches.map((m) => {
    const row: (Node | string)[] = [
      formatTime(m.finished_at ?? m.created_at),
      m.kind,
      m.map,
      teamLink(m.bot_a),
      scoreText(m),
      teamLink(m.bot_b),
    ];
    if (perspective) row.push(resultFor(m, perspective));
    row.push(link(href(`/series/${m.series_id}`), `#${m.index + 1}`), matchLinks(m, ownTeam));
    return row;
  });
  return table(headers, rows, "No matches yet.");
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
      list.replaceChildren(matchTable(shown, scope === "mine" ? myTeam?.id : undefined, myTeam?.id));
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
        matchTable(s.matches, me, me),
      ];
    }),
  );
};
