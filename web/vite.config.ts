import { defineConfig } from "vitest/config";

// The coordinator serves `dist/` at `/` and answers `index.html` for client routes such as `/device?code=…`,
// so asset URLs are absolute (`base: "/"`): relative ones would break below a nested path.
// `npm run dev` proxies the API and the viewer to a running coordinator (IBC_COORDINATOR, default :8080).
const coordinator = process.env.IBC_COORDINATOR ?? "http://127.0.0.1:8080";

export default defineConfig({
  base: "/",
  build: { target: "es2022", sourcemap: false },
  server: {
    proxy: {
      "/v1": coordinator,
      "/viewer": coordinator,
    },
  },
  test: { environment: "node", include: ["test/**/*.test.ts"] },
});
