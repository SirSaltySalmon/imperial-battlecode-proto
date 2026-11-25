import { defineConfig } from "vitest/config";

// `base: "./"` keeps every asset URL relative, so the built `dist/` works from any path of any static host
// and from `ibc view`.
export default defineConfig({
  base: "./",
  build: { target: "es2022", sourcemap: false },
  test: { environment: "node", include: ["test/**/*.test.ts"] },
});
