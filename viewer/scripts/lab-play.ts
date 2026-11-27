// Dev-server routes behind the combat lab's Play section. `tools/lab.sh` sets IBC_LAB_IBC (an `ibc` binary),
// IBC_LAB_PLAY_PORT and optionally IBC_LAB_PLAY_ARGS (extra `ibc play` options, one per line). The page picks
// the bots, the map and a kit; this starts `ibc play TEAM OPPONENT --map M [--unit 0 --kit K] --port P
// -o out/lab-play` with them, replacing the match before. Only the command line of `ibc play` is assumed.
//
//   GET  /lab-play/options   { bots, maps, running }
//   POST /lab-play/start     { team, opponent, map, kit } -> { url } or { error }

import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import type { IncomingMessage, ServerResponse } from "node:http";
import { join, resolve } from "node:path";
import type { Plugin } from "vite";

interface Setup {
  team: string;
  opponent: string;
  map: string;
  /** Kit name for unit 0; "" leaves the draft to the team's bot. `ibc play` checks the name. */
  kit: string;
}

const READY_MS = 180_000;

/** Bot directories `ibc build` can build: a Rust crate or C++ sources. */
function listBots(repo: string): string[] {
  const dir = join(repo, "bots");
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .filter((e) => {
      const d = join(dir, e.name);
      const src = join(d, "src");
      return existsSync(join(d, "Cargo.toml")) || (existsSync(src) && readdirSync(src).some((f) => f.endsWith(".cpp")));
    })
    .map((e) => `bots/${e.name}`)
    .sort();
}

function listMaps(repo: string): string[] {
  const pool = join(repo, "maps");
  const maps = existsSync(pool) ? readdirSync(pool).filter((f) => f.endsWith(".ibcmap")).sort().map((f) => `maps/${f}`) : [];
  return ["tools/lab/range.ibcmap", ...maps];
}

function send(res: ServerResponse, status: number, body: unknown): void {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(body));
}

function readJson(req: IncomingMessage): Promise<unknown> {
  return new Promise((ok, fail) => {
    let text = "";
    req.on("data", (c) => (text += c));
    req.on("end", () => {
      try {
        ok(JSON.parse(text));
      } catch (e) {
        fail(e);
      }
    });
  });
}

export function labPlay(): Plugin {
  const ibc = process.env.IBC_LAB_IBC;
  const port = Number(process.env.IBC_LAB_PLAY_PORT || 0);
  const extra = (process.env.IBC_LAB_PLAY_ARGS ?? "").split("\n").filter(Boolean);
  let repo = "";
  const url = `http://127.0.0.1:${port}/`;
  let child: ChildProcess | null = null;
  let running: Setup | null = null;

  async function stop(): Promise<void> {
    const c = child;
    child = null;
    running = null;
    if (!c || c.exitCode !== null) return;
    await new Promise<void>((done) => {
      c.once("exit", () => done());
      c.kill();
    });
  }

  async function start(setup: Setup): Promise<string> {
    await stop();
    const kit = setup.kit ? ["--unit", "0", "--kit", setup.kit] : [];
    const args = ["play", setup.team, setup.opponent, "--map", setup.map, ...kit, "--port", String(port), "-o", "out/lab-play", ...extra];
    const c = spawn(ibc!, args, { cwd: repo, stdio: ["ignore", "pipe", "pipe"] });
    child = c;
    let log = "";
    const keep = (b: Buffer) => (log = (log + b.toString()).slice(-2000));
    c.stdout!.on("data", keep);
    c.stderr!.on("data", keep);
    const until = Date.now() + READY_MS;
    while (Date.now() < until) {
      if (c.exitCode !== null) {
        const errors = log.split("\n").filter((l) => l.startsWith("error")).join("\n");
        throw new Error(errors || log.trim() || `ibc play exited with ${c.exitCode}`);
      }
      if (await fetch(url).then((r) => r.ok, () => false)) {
        running = setup;
        return url;
      }
      await new Promise((r) => setTimeout(r, 300));
    }
    await stop();
    throw new Error("ibc play did not start serving in time");
  }

  return {
    name: "lab-play",
    apply: "serve",
    configureServer(server) {
      if (!ibc || !port) return;
      repo = resolve(server.config.root, "..");
      // Vite restarts the server (and this plugin) when the config changes; the match must not outlive it.
      server.httpServer?.on("close", () => child?.kill());
      process.on("exit", () => child?.kill());
      server.middlewares.use("/lab-play/options", (_req, res) => {
        send(res, 200, { bots: listBots(repo), maps: listMaps(repo), running: running && { ...running, url } });
      });
      server.middlewares.use("/lab-play/start", async (req, res) => {
        if (req.method !== "POST") return send(res, 405, { error: "POST only" });
        try {
          const body = (await readJson(req)) as Partial<Setup>;
          const bots = listBots(repo);
          const setup = { team: String(body.team), opponent: String(body.opponent), map: String(body.map), kit: String(body.kit ?? "") };
          if (!bots.includes(setup.team) || !bots.includes(setup.opponent) || !listMaps(repo).includes(setup.map)) {
            return send(res, 400, { error: "unknown bot or map" });
          }
          if (!/^[\w-]*$/.test(setup.kit)) return send(res, 400, { error: "bad kit name" });
          send(res, 200, { url: await start(setup) });
        } catch (e) {
          send(res, 500, { error: e instanceof Error ? e.message : String(e) });
        }
      });
    },
  };
}
