// The ladder, and a public team page with its standing and recent matches.

import type { LadderRow } from "../api";
import { h, link, section, table } from "../dom";
import { ratingText, recordText, tierLabel } from "../format";
import { href } from "../router";
import type { Page } from "../session";
import { matchTable } from "./matches";

export const ladder: Page = (ctx) =>
  h(
    "div",
    {},
    h("h1", {}, "Ladder"),
    h(
      "p",
      { class: "muted" },
      "Every team starts at 1500. A new bot plays its placement series right away and shows Provisional until they finish; tiers are labels only and do not affect matchmaking.",
    ),
    section("Standings", async () => {
      const rows = await ctx.api.ladder();
      const mine = ctx.session.team?.id;
      return table(
        ["Rank", "Team", "Tier", "Rating", "Games", "W-L-D"],
        rows.map((r) => [
          String(r.rank),
          h("span", { class: r.team_id === mine ? "mine" : "" }, link(href(`/teams/${r.team_id}`), r.team_name)),
          h("span", { class: r.provisional ? "tier provisional" : "tier" }, tierLabel(r)),
          ratingText(r),
          String(r.games),
          recordText(r),
        ]),
        "No teams on the ladder yet.",
      );
    }),
  );

function standing(row: LadderRow | undefined): HTMLElement {
  if (!row) return h("p", { class: "muted" }, "Not on the ladder (no live submission yet).");
  return h(
    "p",
    {},
    `Rank ${row.rank} · ${tierLabel(row)} · rating ${ratingText(row)} · ${row.games} games`,
    row.wins !== undefined ? ` · ${recordText(row)}` : "",
  );
}

/** `#/teams/:id` */
export const teamPage: Page = (ctx) => {
  const id = ctx.params.id!;
  const title = h("h1", {}, "Team");
  return h(
    "div",
    {},
    title,
    section("Standing", async () => {
      const row = (await ctx.api.ladder()).find((r) => r.team_id === id);
      if (row) title.textContent = row.team_name;
      return standing(row);
    }),
    section("Recent matches", async () => {
      const page = await ctx.api.matches({ team: id, limit: 20 });
      const first = page.matches[0];
      if (first) title.textContent = first.bot_a.team_id === id ? first.bot_a.name : first.bot_b.name;
      return [
        matchTable(page.matches, id, ctx.session.team?.id),
        ctx.session.team && ctx.session.team.id !== id
          ? h("p", {}, link(href("/scrims", { opponent: id }), "Request a scrim against this team"))
          : null,
      ];
    }),
  );
};
