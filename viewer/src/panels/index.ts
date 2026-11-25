// The overlays and strips around the map that read the whole match: buy menu (B), team tab (Tab), kill
// feed, round history. `main.ts` creates one `Panels`, loads it with each replay and updates it per frame.

import type { FrameStore } from "../engine";
import type { Timeline } from "../timeline";
import type { Frame, Header } from "../types";
import type { ViewRules } from "../vision";
import { BuyMenu } from "./buy";
import { roundResults, RoundStrip, type RoundResult } from "./history";
import { killCauses, KillFeed } from "./killfeed";
import { parseShopRules, type ShopRules } from "./shared";
import { TeamTab } from "./tab";

export { parseShopRules, type ShopRules };

export interface PanelRoots {
  buy: HTMLElement;
  tab: HTMLElement;
  feed: HTMLElement;
}

export interface PanelMatch {
  header: Header;
  timeline: Timeline;
  store: Pick<FrameStore, "parse">;
  view: ViewRules;
  /** `Engine.rawRules()` of the engine build that plays the replay. */
  rawRules: unknown;
  teamSize: number;
}

const NON_TEXT_INPUTS = ["range", "checkbox", "radio", "button", "submit", "reset", "file", "color"];

/** An element that types the key pressed into itself. */
function takesText(e: Element | null): boolean {
  if (e instanceof HTMLTextAreaElement) return true;
  return e instanceof HTMLInputElement && !NON_TEXT_INPUTS.includes(e.type);
}

/** A Tab press held longer than this (ms) is a peek that closes on release; shorter pins the tab open. */
const HOLD_MS = 250;

export class Panels {
  private readonly buy: BuyMenu;
  private readonly tab: TeamTab;
  private readonly feed: KillFeed;
  private strip: RoundStrip | null = null;
  private match: { timeline: Timeline; teamSize: number; causes: Map<string, string>; results: RoundResult[] } | null =
    null;
  private last: { index: number; frame: Frame } | null = null;
  private tabDownAt = -1;
  private tabPinned = false;

  constructor(roots: PanelRoots) {
    this.buy = new BuyMenu(roots.buy);
    this.tab = new TeamTab(roots.tab);
    this.feed = new KillFeed(roots.feed);
    this.buy.setRules(null, false);
  }

  load(m: PanelMatch, stripRoot: HTMLElement): void {
    const shop: ShopRules | null = parseShopRules(m.rawRules);
    const results = roundResults(m.timeline);
    this.buy.setRules(shop, true);
    this.tab.load(m.header, shop, m.view, results, m.teamSize);
    this.feed.clear();
    this.strip = new RoundStrip(stripRoot);
    this.match = {
      timeline: m.timeline,
      teamSize: m.teamSize,
      causes: killCauses(m.timeline, m.store, m.view, shop),
      results,
    };
    this.last = null;
  }

  clear(): void {
    this.match = null;
    this.strip = null;
    this.last = null;
    this.feed.clear();
    this.tab.clear();
    this.buy.setRules(null, false);
  }

  update(index: number, frame: Frame): void {
    this.last = { index, frame };
    if (!this.match) return;
    const { timeline, teamSize, causes, results } = this.match;
    this.feed.update(timeline, index, teamSize, causes);
    this.strip?.update(results, index);
    this.tab.update(index, frame);
  }

  private setTab(open: boolean): void {
    this.tab.setOpen(open);
    if (!open) return;
    if (this.last) this.tab.update(this.last.index, this.last.frame);
    else this.tab.showEmpty();
  }

  /** Close both overlays (Esc). */
  closeAll(): void {
    this.buy.close();
    this.tabPinned = false;
    this.tabDownAt = -1;
    this.tab.setOpen(false);
  }

  /**
   * B toggles the buy menu; Tab held peeks at the team tab, Tab tapped pins it (tap again to close); Esc
   * closes both. While a control has keyboard focus, Tab keeps moving focus as usual (clicked buttons give
   * focus back, so this only applies to keyboard navigation).
   */
  bindKeys(target: Window = window): void {
    target.addEventListener("keydown", (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const focused = e.target instanceof Element ? e.target : null;
      if (e.key === "Escape") {
        this.closeAll();
      } else if (e.key === "b" || e.key === "B") {
        if (takesText(focused)) return;
        e.preventDefault();
        this.buy.toggle();
      } else if (e.key === "Tab") {
        if (focused?.closest("button, select, input, textarea, a[href]")) return;
        e.preventDefault();
        if (e.repeat) return;
        if (this.tab.open && this.tabPinned) {
          this.tabPinned = false;
          this.setTab(false);
          return;
        }
        this.tabDownAt = performance.now();
        this.setTab(true);
      }
    });
    target.addEventListener("keyup", (e) => {
      if (e.key !== "Tab" || this.tabDownAt < 0) return;
      const held = performance.now() - this.tabDownAt;
      this.tabDownAt = -1;
      if (held > HOLD_MS) this.setTab(false);
      else this.tabPinned = true;
    });
    // A Tab released while the window was not focused never sends keyup: drop the peek with the focus.
    target.addEventListener("blur", () => {
      if (this.tabDownAt >= 0 && !this.tabPinned) this.setTab(false);
      this.tabDownAt = -1;
    });
  }
}
