// The organisers' page: overview numbers, flagged accounts, suspend / ban / unban, tournaments, audit log.
// The moderation endpoints' row shapes are not fixed, so their tables show whatever columns come back.

import type { ModerationAction, TournamentField } from "../api";
import { h, field, link, notice, onSubmit, section, table } from "../dom";
import { cellText, checkTournamentMaps, objectColumns, splitList } from "../format";
import { href } from "../router";
import type { Ctx, Page } from "../session";

function objectTable(rows: Record<string, unknown>[], empty: string): HTMLElement {
  const cols = objectColumns(rows);
  return table(cols, rows.map((r) => cols.map((c) => cellText(r[c]))), empty);
}

/** Top-level numbers and strings as a definition list; nested values as JSON. */
function overview(o: Record<string, unknown>): HTMLElement {
  return h(
    "dl",
    { class: "overview" },
    Object.entries(o).map(([k, v]) => [h("dt", {}, k.replace(/_/g, " ")), h("dd", {}, cellText(v))]),
  );
}

function moderationForm(ctx: Ctx): HTMLElement {
  const team = h("input", { name: "team", required: true, placeholder: "team id", spellcheck: false });
  const action = h("select", { name: "action" });
  for (const a of ["suspend", "ban", "unban"]) action.add(new Option(a, a));
  const reason = h("input", { name: "reason", required: true });
  const until = h("input", { name: "until", type: "datetime-local" });
  return onSubmit(
    h(
      "form",
      { class: "card" },
      field("Team id", team),
      field("Action", action),
      field("Reason", reason, "Recorded in the audit log and shown to the team."),
      field("Until", until, "Suspensions only; empty means until lifted."),
      h("button", { type: "submit" }, "Apply"),
    ),
    async () => {
      const req = { reason: reason.value.trim(), until: until.value ? new Date(until.value).toISOString() : undefined };
      await ctx.api.moderate(team.value.trim(), action.value as ModerationAction, req);
      return `${action.value} applied to ${team.value.trim()}.`;
    },
  );
}

function fieldFrom(kind: string, n: string, list: string): TournamentField {
  if (kind === "top") return { kind: "top", n: Number(n) };
  if (kind === "list") return { kind: "list", team_ids: splitList(list) };
  return { kind: "all" };
}

function tournamentForm(ctx: Ctx): HTMLElement {
  const name = h("input", { name: "name", required: true });
  const format = h("select", { name: "format" });
  format.add(new Option("Single elimination", "single"));
  format.add(new Option("Double elimination", "double"));
  const kind = h("select", { name: "field" });
  kind.add(new Option("Every team with a live submission", "all"));
  kind.add(new Option("Top N of the ladder", "top"));
  kind.add(new Option("A list of teams", "list"));
  const n = h("input", { name: "n", type: "number", min: "2", value: "8" });
  const list = h("input", { name: "team_ids", placeholder: "team ids, comma separated" });
  const maps = h("input", { name: "maps", required: true, placeholder: "map names, comma separated (odd count)" });
  return onSubmit(
    h(
      "form",
      { class: "card" },
      field("Name", name),
      field("Format", format),
      field("Field", kind),
      field("N (top N)", n),
      field("Teams (list)", list),
      field("Maps", maps, "Unseen maps; an odd number so no pairing ends level on maps."),
      h("button", { type: "submit" }, "Create tournament"),
    ),
    async () => {
      const mapList = splitList(maps.value);
      const problem = checkTournamentMaps(mapList);
      if (problem) throw new Error(problem);
      const t = await ctx.api.createTournament({
        name: name.value.trim(),
        format: format.value as "single" | "double",
        field: fieldFrom(kind.value, n.value, list.value),
        maps: mapList,
      });
      return h("div", { class: "notice ok" }, "Created ", link(href(`/tournaments/${t.id}`), t.name), ". Lock it to snapshot the entries.");
    },
  );
}

/** `#/admin` */
export const admin: Page = (ctx) => {
  if (!ctx.session.isAdmin) return h("div", {}, h("h1", {}, "Organisers"), notice("Organisers only.", "warn"));
  return h(
    "div",
    {},
    h("h1", {}, "Organisers"),
    section("Overview", async () => overview(await ctx.api.adminOverview())),
    section("Flags", async () => objectTable(await ctx.api.adminFlags(), "No flags.")),
    h("section", {}, h("h2", {}, "Suspend, ban, unban"), moderationForm(ctx)),
    h("section", {}, h("h2", {}, "New tournament"), tournamentForm(ctx), h("p", {}, link(href("/tournaments"), "All tournaments"))),
    section("Audit log", async () => objectTable(await ctx.api.adminAudit(), "Nothing logged yet.")),
  );
};
