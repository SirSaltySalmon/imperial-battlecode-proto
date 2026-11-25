import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// `base: "./"` keeps every asset URL relative, so the built `dist/` works from any path of any static host
// and from `ibc view`. The combat lab is a second page (`lab.html`) so it shares this build.
const root = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  base: "./",
  build: {
    target: "es2022",
    sourcemap: false,
    rollupOptions: {
      input: {
        main: resolve(root, "index.html"),
        lab: resolve(root, "lab.html"),
      },
    },
  },
  test: { environment: "node", include: ["test/**/*.test.ts"] },
});
