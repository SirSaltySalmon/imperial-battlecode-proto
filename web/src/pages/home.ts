// The landing page: what to do next, and the CLI downloads the server lists.

import { h, link, section, table } from "../dom";
import { safeUrl } from "../format";
import { href } from "../router";
import { displayName, type Page } from "../session";

function nextSteps(signedIn: boolean, hasTeam: boolean): HTMLElement {
  const steps: (string | Node)[][] = [
    signedIn ? ["Signed in."] : [link(href("/signup"), "Create an account"), " or ", link(href("/signin"), "sign in"), "."],
    hasTeam ? ["You have a team."] : [link(href("/team"), "Create or join a team"), " (1–4 people, joined by code)."],
    ["Install the CLI: ", h("code", {}, "pip install imperial-battlecode"), ", then ", h("code", {}, "ibc login"), "."],
    ["Write a bot, ", h("code", {}, "ibc build"), " it, and ", link(href("/submissions"), "upload"), " it (or ", h("code", {}, "ibc submit"), ")."],
    ["Watch it climb the ", link(href("/ladder"), "ladder"), " and ", link(href("/matches"), "replay its matches"), "."],
  ];
  return h("ol", { class: "steps" }, steps.map((s) => h("li", {}, s)));
}

export const home: Page = (ctx) => {
  const s = ctx.session;
  return h(
    "div",
    {},
    h("h1", {}, "Imperial Battlecode"),
    h("p", {}, "Write a bot for a team tactical shooter; it plays ranked matches against everyone else's around the clock."),
    s.via !== "none" ? h("p", {}, `Signed in as ${displayName(s)}.`) : null,
    nextSteps(s.via !== "none", s.team !== null),
    section("Downloads", async () => {
      const d = await ctx.api.distribution();
      return [
        h("p", {}, `CLI ${d.version}${d.patch ? ` · patch ${d.patch}` : ""}`),
        table(
          ["Platform", "File", "SHA-256"],
          d.artifacts.map((a) => {
            const url = safeUrl(a.url);
            const name = a.url.split("/").pop() || a.url;
            return [a.target, url ? link(url, name) : name, h("code", {}, a.sha256)];
          }),
          "No downloads published.",
        ),
      ];
    }),
  );
};
