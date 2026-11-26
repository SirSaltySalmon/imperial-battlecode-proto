// My team: create or join one, see members and the join code, leave.

import type { TeamInfo } from "../api";
import { actionButton, h, field, link, notice, onSubmit, section, table } from "../dom";
import { formatTime } from "../format";
import { href } from "../router";
import type { Ctx, Page } from "../session";
import { signInFirst } from "./auth";

function noTeam(ctx: Ctx): HTMLElement {
  const name = h("input", { name: "name", required: true, maxlength: "64" });
  const create = onSubmit(
    h("form", { class: "card" }, h("h2", {}, "Create a team"), field("Team name", name), h("button", { type: "submit" }, "Create")),
    async () => {
      const res = await ctx.api.createTeam(name.value.trim());
      await ctx.refresh();
      return notice(`Team ${res.team.name} created. Join code: ${res.join_code}`, "ok");
    },
  );
  const code = h("input", { name: "join_code", required: true, autocomplete: "off", spellcheck: false });
  const join = onSubmit(
    h("form", { class: "card" }, h("h2", {}, "Join a team"), field("Join code", code, "Ask a teammate for it."), h("button", { type: "submit" }, "Join")),
    async () => {
      await ctx.api.joinTeam(code.value.trim());
      await ctx.refresh();
    },
  );
  return h("div", { class: "columns" }, create, join);
}

/** The live status line: what the server says, or the missing-live warning (that team is not matched). */
export function liveStatus(t: TeamInfo): HTMLElement {
  if (t.live_status || t.grace_until) {
    const grace = t.grace_until ? ` (grace until ${formatTime(t.grace_until)})` : "";
    return notice(`Live submission: ${t.live_status ?? "set"}${grace}`, t.live_submission ? "info" : "warn");
  }
  if (!t.live_submission) {
    return notice("No live submission: your team is not matched in ranked games until you upload a bot that validates.", "warn");
  }
  return notice(`Live submission: ${t.live_submission}`);
}

function teamView(ctx: Ctx, t: TeamInfo): Node[] {
  const joinCode = t.join_code ?? ctx.session.team?.join_code ?? null;
  const members = t.members ?? [];
  return [
    h("p", {}, "Team id: ", h("code", {}, t.team_id), " · ", link(href(`/teams/${t.team_id}`), "public page")),
    joinCode ? h("p", {}, "Join code: ", h("code", { class: "join-code" }, joinCode), h("span", { class: "muted" }, " (share it with teammates)")) : null,
    liveStatus(t),
    h("h3", {}, "Members"),
    members.length > 0
      ? table(["Name", "Email"], members.map((m) => [m.display_name ?? "—", m.email ?? "—"]))
      : h("p", { class: "muted" }, "The server does not list members yet."),
    ctx.session.user
      ? actionButton(
          "Leave team",
          async () => {
            if (!confirm(`Leave ${t.name}?`)) return;
            await ctx.api.leaveTeam();
            await ctx.refresh();
          },
        )
      : null,
  ].filter((n): n is HTMLElement => n !== null);
}

/** `#/team` */
export const team: Page = (ctx) => {
  if (ctx.session.via === "none") return h("div", {}, h("h1", {}, "Team"), signInFirst("Sign in to create or join a team."));
  if (ctx.session.isAdmin && !ctx.session.team && !ctx.session.user) {
    return h("div", {}, h("h1", {}, "Team"), notice("Signed in with the organisers' token, which belongs to no team."));
  }
  if (!ctx.session.team) return h("div", {}, h("h1", {}, "Team"), noTeam(ctx));
  return h(
    "div",
    {},
    h("h1", {}, ctx.session.team.name),
    section("Your team", async () => {
      const t = await ctx.api.team();
      if ("admin" in t) return notice("The organisers' token belongs to no team.");
      return teamView(ctx, t);
    }),
  );
};
