// Tournaments: the list, and a bracket (one column per round). Organisers also see the lock / start /
// reveal controls and every pairing a seed decided, highlighted.

import type { Pairing, Tournament } from "../api";
import { actionButton, h, link, section, table } from "../dom";
import { decidedText, isFlagged, pairingSide } from "../format";
import { href } from "../router";
import type { Ctx, Page } from "../session";

export const tournaments: Page = (ctx) =>
  h(
    "div",
    {},
    h("h1", {}, "Tournaments"),
    section("All tournaments", async () => {
      const list = await ctx.api.tournaments();
      return table(
        ["Name", "Format", "Status"],
        list.map((t) => [link(href(`/tournaments/${t.id}`), t.name), `${t.format} elimination`, t.status]),
        "No tournaments yet.",
      );
    }),
  );

function side(p: Pairing, s: "a" | "b"): HTMLElement {
  const id = s === "a" ? p.a : p.b;
  const seed = s === "a" ? p.seed_a : p.seed_b;
  const won = p.winner !== null && p.winner === id;
  const maps = p.maps_won ? p.maps_won[s === "a" ? 0 : 1] : null;
  return h(
    "div",
    { class: won ? "side winner" : "side" },
    h("span", { class: "seed" }, seed !== null ? String(seed) : ""),
    id ? link(href(`/teams/${id}`), pairingSide(p, s)) : h("span", { class: "muted" }, pairingSide(p, s)),
    h("span", { class: "maps-won" }, maps !== null ? String(maps) : ""),
  );
}

export function pairingBox(p: Pairing, admin: boolean): HTMLElement {
  // Nobody plays it: the bracket reset when the winners-bracket champion won the grand final, or two byes.
  if (p.state === "empty") return h("div", { class: "pairing" }, h("span", { class: "muted" }, "Not needed"));
  const flagged = admin && isFlagged(p);
  return h(
    "div",
    { class: flagged ? "pairing flagged" : "pairing", title: flagged ? "Decided by seed: review" : "" },
    side(p, "a"),
    side(p, "b"),
    p.decided_by ? h("div", { class: "decided" }, decidedText(p), flagged ? h("strong", {}, " · flagged") : null) : null,
  );
}

function bracket(t: Tournament, admin: boolean, onlyFlagged: boolean): HTMLElement {
  if (t.rounds.length === 0) return h("p", { class: "muted" }, "The bracket is drawn when the tournament starts.");
  return h(
    "div",
    { class: "bracket" },
    t.rounds.map((round, i) =>
      h(
        "div",
        { class: "round" },
        h("h3", {}, round.name ?? `Round ${i + 1}`),
        round.pairings.filter((p) => !onlyFlagged || isFlagged(p)).map((p) => pairingBox(p, admin)),
      ),
    ),
  );
}

function adminControls(ctx: Ctx, t: Tournament): HTMLElement {
  const act = (a: "lock" | "start" | "reveal") => actionButton(a[0]!.toUpperCase() + a.slice(1), () => ctx.api.tournamentAction(t.id, a), () => void ctx.refresh());
  return h(
    "div",
    { class: "admin-bar" },
    h("strong", {}, "Organisers: "),
    act("lock"),
    act("start"),
    act("reveal"),
  );
}

/** `#/tournaments/:id` */
export const tournament: Page = (ctx) => {
  const id = ctx.params.id!;
  const admin = ctx.session.isAdmin;
  return h(
    "div",
    {},
    section(`Tournament`, async () => {
      const t = await ctx.api.tournament(id);
      const holder = h("div", {}, bracket(t, admin, false));
      const filter = h("input", { type: "checkbox" });
      filter.addEventListener("change", () => holder.replaceChildren(bracket(t, admin, filter.checked)));
      const flaggedCount = t.rounds.flatMap((r) => r.pairings).filter(isFlagged).length;
      return [
        h("h1", {}, t.name),
        h("p", {}, `${t.format} elimination · ${t.status}`, t.maps ? ` · maps: ${t.maps.join(", ")}` : ""),
        admin ? adminControls(ctx, t) : null,
        admin
          ? h("label", { class: "check" }, filter, ` Only pairings decided by seed (${flaggedCount})`)
          : null,
        holder,
      ];
    }),
  );
};
