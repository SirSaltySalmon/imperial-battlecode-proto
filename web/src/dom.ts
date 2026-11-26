// A few DOM helpers. Text always goes in through `textContent` (team names and notes are user input).

import { errorText } from "./format";

type Child = Node | string | number | null | undefined | false | Child[];
type Attrs = Record<string, string | number | boolean | null | undefined | ((ev: Event) => void)>;

/** `h("a", {href: "#/"}, "Home")`. `on<event>` attributes become listeners; `false`/`null` attributes are left out. */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (typeof v === "function") el.addEventListener(k.slice(2).toLowerCase(), v);
    else if (v === true) el.setAttribute(k, "");
    else el.setAttribute(k, String(v));
  }
  append(el, children);
  return el;
}

function append(el: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(typeof c === "object" ? c : document.createTextNode(String(c)));
  }
}

export function table(headers: string[], rows: Child[][], empty = "Nothing here yet."): HTMLElement {
  if (rows.length === 0) return h("p", { class: "muted" }, empty);
  return h(
    "table",
    {},
    h("thead", {}, h("tr", {}, headers.map((t) => h("th", {}, t)))),
    h(
      "tbody",
      {},
      rows.map((r) => h("tr", {}, r.map((c) => h("td", {}, c)))),
    ),
  );
}

export function errorBox(e: unknown): HTMLElement {
  return h("div", { class: "error", role: "alert" }, errorText(e));
}

export function notice(text: string, kind: "ok" | "warn" | "info" = "info"): HTMLElement {
  return h("div", { class: `notice ${kind}` }, text);
}

/** A section that fills itself from `load` and shows its own error, so one failing call spares the page. */
export function section(title: string, load: () => Promise<Child>): HTMLElement {
  const body = h("div", { class: "muted" }, "Loading…");
  const el = h("section", {}, h("h2", {}, title), body);
  load().then(
    (content) => body.replaceWith(h("div", {}, content)),
    (e) => body.replaceWith(errorBox(e)),
  );
  return el;
}

export function field(label: string, input: HTMLElement, hint?: string): HTMLElement {
  return h("label", { class: "field" }, h("span", {}, label), input, hint ? h("small", { class: "muted" }, hint) : null);
}

/**
 * Run `submit` when the form is submitted: the buttons are disabled meanwhile, and its message (or the
 * error) is shown in the form's status line.
 */
export function onSubmit(form: HTMLFormElement, submit: () => Promise<string | Node | void>): HTMLFormElement {
  const status = h("div", { class: "status", "aria-live": "polite" });
  form.append(status);
  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const buttons = [...form.querySelectorAll("button")];
    buttons.forEach((b) => (b.disabled = true));
    status.replaceChildren(h("span", { class: "muted" }, "Working…"));
    submit().then(
      (msg) => status.replaceChildren(msg ? (typeof msg === "string" ? notice(msg, "ok") : msg) : ""),
      (e) => status.replaceChildren(errorBox(e)),
    ).finally(() => buttons.forEach((b) => (b.disabled = false)));
  });
  return form;
}

/** A button that runs `action`, shows its error next to itself, and calls `done` after a success. */
export function actionButton(label: string, action: () => Promise<unknown>, done?: () => void): HTMLElement {
  const status = h("span", { class: "status" });
  const button = h("button", { type: "button" }, label);
  button.addEventListener("click", () => {
    button.disabled = true;
    status.replaceChildren();
    action().then(
      () => done?.(),
      (e) => status.replaceChildren(errorBox(e)),
    ).finally(() => (button.disabled = false));
  });
  return h("span", { class: "action" }, button, status);
}

export function link(href: string, text: string, attrs: Attrs = {}): HTMLAnchorElement {
  return h("a", { href, ...attrs }, text);
}
