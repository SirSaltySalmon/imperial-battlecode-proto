import { rmSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";
import { labPlay } from "./scripts/lab-play";

// `base: "./"` keeps every asset URL relative, so the built `dist/` works from any path of any static host
// and from `ibc view`. The combat lab (`lab.html`) and the map editor (`editor.html`) are more pages of this build.
const root = fileURLToPath(new URL(".", import.meta.url));

// `public/lab.json` is recorded from the rules of the working tree (`tools/lab.sh`). A copy in `dist/` would keep
// that ruleset's numbers after the rules change, so builds never ship it.
const noLabRecording: Plugin = {
  name: "no-lab-recording",
  apply: "build",
  closeBundle() {
    rmSync(resolve(root, "dist/lab.json"), { force: true });
  },
};

export default defineConfig({
  base: "./",
  plugins: [noLabRecording, labPlay()],
  build: {
    target: "es2022",
    sourcemap: false,
    rollupOptions: {
      input: {
        main: resolve(root, "index.html"),
        lab: resolve(root, "lab.html"),
        play: resolve(root, "play.html"),
        editor: resolve(root, "editor.html"),
      },
    },
  },
  test: { environment: "node", include: ["test/**/*.test.ts"] },
});
