// Request an unranked scrim: opponent, 1–5 games, maps, which of my versions, or a rematch against the
// opponent version of an earlier series.

import type { ScrimRequest } from "../api";
import { h, errorBox, field, link, notice, onSubmit } from "../dom";
import { href } from "../router";
import type { Ctx, Page } from "../session";
import { signInFirst } from "./auth";

/** A team list from the ladder, or a plain id field when the ladder has no other team. */
async function opponentPicker(ctx: Ctx, chosen: string): Promise<HTMLSelectElement | HTMLInputElement> {
  const rows = (await ctx.api.ladder().catch(() => [])).filter((r) => r.team_id !== ctx.session.team?.id);
  if (rows.length === 0) {
    return h("input", { name: "opponent", required: true, value: chosen, placeholder: "team id", spellcheck: false });
  }
  const el = h("select", { name: "opponent", required: true });
  el.add(new Option("Choose a team", "", true, chosen === ""));
  for (const r of rows) el.add(new Option(`${r.team_name} (#${r.rank})`, r.team_id, false, r.team_id === chosen));
  if (chosen && !rows.some((r) => r.team_id === chosen)) el.add(new Option(chosen, chosen, false, true));
  return el;
}

async function mapPicker(ctx: Ctx): Promise<HTMLElement & { chosen(): string[] }> {
  const maps = (await ctx.api.maps().catch(() => [])).filter((m) => m.in_pool);
  const boxes = maps.map((m) => h("input", { type: "checkbox", value: m.name }));
  const el = h(
    "fieldset",
    { class: "maps" },
    h("legend", {}, "Maps (none ticked: the server picks from the pool)"),
    maps.length === 0 ? h("span", { class: "muted" }, "No maps in the pool.") : null,
    maps.map((m, i) => h("label", { class: "check" }, boxes[i]!, ` ${m.name}`)),
  );
  return Object.assign(el, { chosen: () => boxes.filter((b) => b.checked).map((b) => b.value) });
}

async function versionPicker(ctx: Ctx): Promise<HTMLSelectElement> {
  const el = h("select", { name: "my_submission_id" });
  el.add(new Option("My live version", ""));
  for (const s of await ctx.api.submissions().catch(() => [])) {
    if (s.status !== "validated" || s.live) continue;
    el.add(new Option(`${s.submission_id}${s.note ? ` — ${s.note}` : ""}`, s.submission_id));
  }
  return el;
}

async function form(ctx: Ctx): Promise<HTMLElement> {
  const [opponent, maps, version] = await Promise.all([
    opponentPicker(ctx, ctx.query.get("opponent") ?? ""),
    mapPicker(ctx),
    versionPicker(ctx),
  ]);
  const games = h("input", { type: "number", name: "games", min: "1", max: "5", value: "1", required: true });
  const against = h("input", {
    type: "text",
    name: "against_series_id",
    value: ctx.query.get("against") ?? "",
    placeholder: "series id",
    spellcheck: false,
  });
  return onSubmit(
    h(
      "form",
      { class: "card" },
      field("Opponent", opponent),
      field("Games", games, "1 to 5; sides alternate."),
      maps,
      field("My version", version),
      field("Rematch the opponent version from series", against, "Optional: replay their bot from an earlier series."),
      h("button", { type: "submit" }, "Request scrim"),
    ),
    async () => {
      const req: ScrimRequest = { opponent_team_id: opponent.value, games: Number(games.value) };
      const chosenMaps = maps.chosen();
      if (chosenMaps.length > 0) req.maps = chosenMaps;
      if (version.value) req.my_submission_id = version.value;
      if (against.value.trim()) req.against_series_id = against.value.trim();
      const res = await ctx.api.scrim(req);
      return h(
        "div",
        { class: "notice ok" },
        "Scrim queued. ",
        res.series_id ? link(href(`/series/${res.series_id}`), "Follow the series") : link(href("/matches"), "See your matches"),
        ".",
      );
    },
  );
}

/** `#/scrims?opponent=<team id>&against=<series id>` */
export const scrims: Page = (ctx) => {
  const head = h("h1", {}, "Request a scrim");
  if (ctx.session.via === "none") return h("div", {}, head, signInFirst("Sign in to request scrims."));
  if (!ctx.session.team) return h("div", {}, head, notice("Join or create a team first (Team page).", "warn"));
  const body = h("div", { class: "muted" }, "Loading…");
  form(ctx).then(
    (f) => body.replaceWith(f),
    (e) => body.replaceWith(errorBox(e)),
  );
  return h(
    "div",
    { class: "narrow" },
    head,
    h("p", { class: "muted" }, "Scrims are unranked and count against your team's daily game quota."),
    body,
  );
};
