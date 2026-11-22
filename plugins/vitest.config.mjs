import { defineConfig } from "vitest/config";

// The harness plugins are plain Node ES modules with no DOM, so they run in the
// node environment, apart from the PWA's jsdom setup in the root config.
export default defineConfig({
  test: {
    include: ["plugins/*/test/**/*.test.mjs"],
    environment: "node",
    reporters: ["default"],
  },
});
