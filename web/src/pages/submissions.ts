// Upload a bot module, list the team's versions with their validation status, choose the live one.

import type { Language, Submission } from "../api";
import { actionButton, h, field, notice, onSubmit, section, table } from "../dom";
import { formatBytes, formatTime } from "../format";
import type { Ctx, Page } from "../session";
import { signInFirst } from "./auth";
import { liveStatus } from "./team";

function statusCell(s: Submission): HTMLElement {
  if (s.status === "validated") return h("span", { class: "ok" }, "validated");
  return h("span", { class: "bad", title: s.reason ?? "" }, `${s.status}${s.reason ? `: ${s.reason}` : ""}`);
}

function liveCell(ctx: Ctx, s: Submission): Node {
  if (s.live) return h("strong", {}, "live");
  if (s.status !== "validated") return h("span", { class: "muted" }, "—");
  return actionButton("Make live", () => ctx.api.setLive(s.submission_id), () => void ctx.refresh());
}

function versions(ctx: Ctx): HTMLElement {
  return section("Versions", async () => {
    const list = await ctx.api.submissions();
    return table(
      ["Uploaded", "Id", "Language", "Size", "Status", "Note", "Live"],
      list.map((s) => [
        formatTime(s.created_at),
        h("code", { title: s.sha256 }, s.submission_id),
        s.language,
        formatBytes(s.size),
        statusCell(s),
        s.note ?? "",
        liveCell(ctx, s),
      ]),
      "No versions yet. Build one with `ibc build` and upload it here, or run `ibc submit`.",
    );
  });
}

/** The upload form; `uploaded` runs after every upload the server recorded (validated or rejected). */
function uploadForm(ctx: Ctx, uploaded: () => void): HTMLElement {
  const file = h("input", { type: "file", name: "wasm", accept: ".wasm,application/wasm", required: true });
  const language = h("select", { name: "language" });
  for (const [v, label] of [
    ["rust", "Rust"],
    ["cpp", "C++"],
  ]) {
    language.add(new Option(label, v));
  }
  const note = h("input", { type: "text", name: "note", maxlength: "200", placeholder: "what changed" });
  const live = h("input", { type: "checkbox", name: "live", checked: true });
  return onSubmit(
    h(
      "form",
      { class: "card" },
      h("h2", {}, "Upload a version"),
      field("Bot module (.wasm from `ibc build`)", file),
      field("Language", language),
      field("Note", note, "Optional; shown in your version list."),
      h("label", { class: "check" }, live, " Make it live if it validates"),
      h("button", { type: "submit" }, "Upload"),
    ),
    async () => {
      const chosen = file.files?.[0];
      if (!chosen) throw new Error("Choose a .wasm file first.");
      try {
        const res = await ctx.api.upload(chosen, language.value as Language, live.checked, note.value);
        return notice(`Uploaded ${res.submission_id}: ${res.status}${res.live ? ", now live" : ""}.`, "ok");
      } finally {
        uploaded();
      }
    },
  );
}

/** `#/submissions` */
export const submissions: Page = (ctx) => {
  const head = h("h1", {}, "Submissions");
  if (ctx.session.via === "none") return h("div", {}, head, signInFirst("Sign in to upload your bot."));
  if (!ctx.session.team) return h("div", {}, head, notice("Join or create a team first (Team page).", "warn"));
  let list = versions(ctx);
  const status = () =>
    section("Status", async () => {
      const t = await ctx.api.team();
      return "admin" in t ? null : liveStatus(t);
    });
  let statusEl = status();
  const reload = () => {
    const [nextStatus, nextList] = [status(), versions(ctx)];
    statusEl.replaceWith(nextStatus);
    list.replaceWith(nextList);
    [statusEl, list] = [nextStatus, nextList];
  };
  return h("div", {}, head, statusEl, uploadForm(ctx, reload), list);
};
