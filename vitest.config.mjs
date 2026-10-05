import { defineConfig } from "vitest/config";

// The unit tests import the client modules the browser loads, from where they
// live. There is no build step, no alias and no copy, so a test and the page
// cannot drift: renaming a file breaks both at once.
export default defineConfig({
  test: {
    include: [".agents/js-tests/**/*.test.mjs"],
    // The screens read the DOM at import time (the element they paint into, the
    // event listeners the shell installs), so a module cannot be imported
    // without one.
    environment: "jsdom",
    setupFiles: [".agents/js-tests/setup.mjs"],
    reporters: ["default"],
  },
});
