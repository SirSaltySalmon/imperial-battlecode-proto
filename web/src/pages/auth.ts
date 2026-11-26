// Sign up, sign in (password or a pasted token), and approving a CLI device login.

import { h, field, link, notice, onSubmit } from "../dom";
import { href } from "../router";
import { tokenStore, type Ctx, type Page } from "../session";

/** "Sign in first" with a way back to this page afterwards. */
export function signInFirst(why: string): HTMLElement {
  const next = location.hash || "#/";
  return h(
    "div",
    { class: "notice warn" },
    `${why} `,
    link(href("/signin", { next }), "Sign in"),
    " or ",
    link(href("/signup", { next }), "create an account"),
    ".",
  );
}

/** Where to go after signing in: the `next` route if it is one of ours, else the team page. */
function nextRoute(ctx: Ctx, fallback: string): string {
  const next = ctx.query.get("next");
  return next && next.startsWith("#/") ? next : fallback;
}

function input(name: string, type: string, attrs: Record<string, string | boolean> = {}): HTMLInputElement {
  return h("input", { name, type, required: true, ...attrs });
}

export const signIn: Page = (ctx) => {
  const email = input("email", "email", { autocomplete: "email" });
  const password = input("password", "password", { autocomplete: "current-password" });
  const form = onSubmit(
    h("form", { class: "card" }, field("Email", email), field("Password", password), h("button", { type: "submit" }, "Sign in")),
    async () => {
      tokenStore.clear();
      await ctx.api.login(email.value.trim(), password.value);
      await ctx.refresh();
      ctx.go(nextRoute(ctx, "#/team"));
    },
  );
  const token = input("token", "password", { autocomplete: "off", placeholder: "ibt… / ibu… / organiser token" });
  const tokenForm = onSubmit(
    h(
      "form",
      { class: "card" },
      field("Token", token, "Kept for this browser tab only. For organisers and development."),
      h("button", { type: "submit" }, "Use token"),
    ),
    async () => {
      tokenStore.set(token.value.trim());
      try {
        await ctx.refresh();
      } catch (e) {
        tokenStore.clear();
        await ctx.refresh();
        throw e;
      }
      ctx.go(nextRoute(ctx, "#/"));
    },
  );
  return h(
    "div",
    { class: "narrow" },
    h("h1", {}, "Sign in"),
    form,
    h("p", {}, "No account yet? ", link(href("/signup", { next: ctx.query.get("next") }), "Sign up"), "."),
    h("details", {}, h("summary", {}, "Sign in with a token instead"), tokenForm),
  );
};

export const signUp: Page = (ctx) => {
  const email = input("email", "email", { autocomplete: "email" });
  const name = input("display_name", "text", { autocomplete: "nickname", maxlength: "64" });
  const password = input("password", "password", { autocomplete: "new-password" });
  const form = onSubmit(
    h(
      "form",
      { class: "card" },
      field("Email", email),
      field("Display name", name),
      field("Password", password),
      h("button", { type: "submit" }, "Create account"),
    ),
    async () => {
      tokenStore.clear();
      await ctx.api.register(email.value.trim(), password.value, name.value.trim());
      await ctx.refresh();
      ctx.go(nextRoute(ctx, "#/team"));
    },
  );
  return h(
    "div",
    { class: "narrow" },
    h("h1", {}, "Create an account"),
    form,
    h("p", {}, "Have one? ", link(href("/signin", { next: ctx.query.get("next") }), "Sign in"), "."),
  );
};

/** `/device?code=…`: the page `ibc login` sends you to. Approving signs the CLI in as you. */
export const device: Page = (ctx) => {
  const code = ctx.query.get("code") ?? "";
  const head = h("h1", {}, "Sign in the CLI");
  if (ctx.session.user === null) {
    return h("div", { class: "narrow" }, head, signInFirst("Sign in with your account to approve the CLI."));
  }
  const userCode = input("user_code", "text", { value: code, autocomplete: "off", spellcheck: false });
  const form = onSubmit(
    h(
      "form",
      { class: "card" },
      field("Code shown by the CLI", userCode),
      h("button", { type: "submit" }, "Approve"),
    ),
    async () => {
      await ctx.api.approveDevice(userCode.value.trim());
      return notice("Approved. The CLI is signed in as you; go back to the terminal.", "ok");
    },
  );
  return h(
    "div",
    { class: "narrow" },
    head,
    h("p", {}, `Only approve a code you just saw in your own terminal. The CLI will act as ${ctx.session.user.email}.`),
    form,
  );
};
