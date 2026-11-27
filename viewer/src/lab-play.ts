// The lab's Play section: pick the bots, the map and your kit, and the dev server of `tools/lab.sh`
// (`scripts/lab-play.ts`) starts `ibc play` with them. Absent when the page is served any other way.

import type { Kit } from "./lab-math";

interface Setup {
  team: string;
  opponent: string;
  map: string;
  /** Kit name for the unit you start in; "" leaves the draft to your team's bot. */
  kit: string;
}

interface Options {
  bots: string[];
  maps: string[];
  running: (Setup & { url: string }) | null;
}

const DEFAULTS: Setup = { team: "bots/idle", opponent: "bots/idle", map: "tools/lab/range.ibcmap", kit: "" };

function mapLabel(path: string): string {
  return path === DEFAULTS.map ? "lab range (1v1)" : path.replace(/^.*\//, "").replace(/\.ibcmap$/, "");
}

function picker(label: string, title: string, values: string[], value: string, name: (v: string) => string): [HTMLElement, HTMLSelectElement] {
  const l = document.createElement("label");
  l.textContent = label;
  l.title = title;
  const s = document.createElement("select");
  for (const v of values) s.append(new Option(name(v), v, false, v === value));
  l.append(s);
  return [l, s];
}

async function options(): Promise<Options | null> {
  try {
    const res = await fetch("lab-play/options", { headers: { accept: "application/json" } });
    return res.ok ? ((await res.json()) as Options) : null;
  } catch {
    return null;
  }
}

/** The section, built once so its choices survive the sidebar being redrawn; null without the launcher.
 *  `kits` come from the lab recording's rules. */
export async function playSection(kits: Kit[]): Promise<HTMLElement | null> {
  const opts = await options();
  if (!opts || opts.bots.length === 0) return null;
  const now = opts.running ?? DEFAULTS;
  const pick = (list: string[], v: string) => (list.includes(v) ? v : list[0]);
  const bot = (v: string) => v.replace(/^bots\//, "");
  const [oppL, opp] = picker("Opponent", "The bot you play against", opts.bots, pick(opts.bots, now.opponent), bot);
  const [teamL, team] = picker("Your team", "The bot that plays your teammates (none on the 1v1 range)", opts.bots, pick(opts.bots, now.team), bot);
  const [mapL, map] = picker("Map", "", opts.maps, pick(opts.maps, now.map), mapLabel);
  const kitNames = ["", ...kits.map((k) => k.name)];
  const kitLabel = (v: string) => {
    const k = kits.find((k) => k.name === v);
    return k ? `${k.name} · ${k.hp} hp · ${k.speed_pct}% speed` : "your team's bot picks";
  };
  const [kitL, kit] = picker("Your kit", "Drafted for the unit you start in (unit 0)", kitNames, pick(kitNames, now.kit), kitLabel);

  const box = document.createElement("div");
  box.className = "play";
  const go = document.createElement("button");
  go.type = "button";
  go.textContent = "Start match";
  go.title = "Starts ibc play with these choices in a new tab, replacing the match before";
  const status = document.createElement("p");
  status.className = "status";
  const show = (text: string, url?: string) => {
    status.replaceChildren(text);
    if (url) {
      const a = document.createElement("a");
      a.href = url;
      a.target = "_blank";
      a.textContent = "open";
      status.append(" ", a);
    }
  };
  const describe = (s: Setup) => `running: ${s.kit || "bot's kit"} vs ${bot(s.opponent)} on ${mapLabel(s.map)}`;
  if (opts.running) show(describe(opts.running), opts.running.url);

  go.addEventListener("click", async () => {
    const setup: Setup = { team: team.value, opponent: opp.value, map: map.value, kit: kit.value };
    // Opened now, inside the click, so the browser does not block it as a popup.
    const tab = window.open("about:blank", "_blank");
    go.disabled = true;
    show("starting (the first match of a bot builds it)");
    try {
      const res = await fetch("lab-play/start", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(setup) });
      const out = (await res.json()) as { url?: string; error?: string };
      if (!out.url) throw new Error(out.error ?? `HTTP ${res.status}`);
      if (tab) tab.location.href = out.url;
      show(describe(setup), out.url);
    } catch (e) {
      tab?.close();
      show(e instanceof Error ? e.message : String(e));
    } finally {
      go.disabled = false;
    }
  });
  box.append(oppL, teamL, mapL, kitL, go, status);
  return box;
}
