import { describe, expect, it } from "vitest";
import { href, matchPattern, parseHash, pathRoute, resolve } from "../src/router";

describe("router", () => {
  it("parses hash routes", () => {
    expect(parseHash("").path).toBe("/");
    expect(parseHash("#/").path).toBe("/");
    const r = parseHash("#/matches/?scope=all&kind=ranked");
    expect(r.path).toBe("/matches");
    expect(r.query.get("kind")).toBe("ranked");
  });

  it("matches patterns and decodes parameters", () => {
    expect(matchPattern("/teams/:id", "/teams/a%20b")).toEqual({ id: "a b" });
    expect(matchPattern("/teams/:id", "/teams")).toBeNull();
    expect(matchPattern("/team", "/teams")).toBeNull();
    expect(matchPattern("/", "/")).toEqual({});
    expect(matchPattern("/teams/:id", "/teams/%E0%A4%A")).toBeNull();
  });

  it("resolves the first matching route", () => {
    const routes = [{ pattern: "/tournaments" }, { pattern: "/tournaments/:id" }];
    expect(resolve(routes, "/tournaments/t9")?.params).toEqual({ id: "t9" });
    expect(resolve(routes, "/nope")).toBeNull();
  });

  it("builds links that round-trip", () => {
    const link = href("/series/s 1", { against: "x&y", empty: "", none: undefined });
    expect(link).toBe("#/series/s%201?against=x%26y");
    const back = parseHash(link);
    expect(resolve([{ pattern: "/series/:id" }], back.path)?.params.id).toBe("s 1");
    expect(back.query.get("against")).toBe("x&y");
  });

  it("turns the CLI's /device?code= URL into a hash route", () => {
    expect(pathRoute("/device", "?code=AB-12", "")).toBe("#/device?code=AB-12");
    expect(pathRoute("/", "", "")).toBeNull();
    expect(pathRoute("/index.html", "", "")).toBeNull();
    expect(pathRoute("/device", "", "#/ladder")).toBeNull();
  });
});
