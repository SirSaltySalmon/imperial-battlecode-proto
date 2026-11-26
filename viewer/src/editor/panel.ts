// The side panel: the engine's verdict, what it reads from the map, and the problem lists.

import { siteName } from "../map";
import { TILE } from "../types";
import { el } from "../ui";
import type { LintResult } from "./api";
import type { CheckResult, Finding } from "./check";

function findingItem(f: Finding): HTMLLIElement {
  const li = el("li", f.severity);
  li.append(el("code", undefined, f.code), document.createTextNode(f.message));
  return li;
}

export function renderFindings(list: HTMLElement, findings: Finding[], none: string): void {
  list.replaceChildren(...(findings.length ? findings.map(findingItem) : [el("li", "none", none)]));
}

export function renderCheck(verdict: HTMLElement, summaryBox: HTMLElement, list: HTMLElement, result: CheckResult): void {
  const s = result.summary;
  const errors = result.findings.filter((f) => f.severity === "error").length;
  verdict.className = `verdict ${s && errors === 0 ? "ok" : "bad"}`;
  verdict.textContent = !s
    ? "The engine rejects this map: ibc run --map would fail."
    : errors
      ? "The engine loads this map, but it has problems."
      : "The engine loads this map.";
  renderFindings(list, result.findings, "none found by the engine (Lint runs the full checks)");
  if (!s) {
    summaryBox.replaceChildren();
    return;
  }
  const rows: [string, string][] = [
    ["Size", `${s.width} × ${s.height}`],
    ["Team size", `${s.team_size}`],
    ["Spawn tiles", `a ${s.spawns[0]}, d ${s.spawns[1]}`],
    ...s.sites.map((site): [string, string] => [
      `Site ${siteName(site.id)}`,
      `${site.max_tx - site.min_tx} × ${site.max_ty - site.min_ty} box, plant at (${(site.centre_x / TILE).toFixed(1)}, ${(site.centre_y / TILE).toFixed(1)})`,
    ]),
    ["Map hash", s.hash],
  ];
  const table = el("table", "summary-table");
  for (const [k, v] of rows) {
    const tr = el("tr");
    tr.append(el("td", undefined, k), el("td", undefined, v));
    table.append(tr);
  }
  summaryBox.replaceChildren(el("div", "section-title", "What the engine reads"), table);
}

export function renderLint(box: HTMLElement, state: HTMLElement, list: HTMLElement, lint: LintResult | null, stale: boolean): void {
  box.hidden = lint === null;
  if (!lint) return;
  state.textContent = `${lint.errors} error(s), ${lint.warnings} warning(s)${stale ? " (map changed since)" : ""}`;
  renderFindings(list, lint.issues, "ok: no errors or warnings");
}
