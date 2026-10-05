// The jsdom environment the client modules are imported into. Two things are
// missing from jsdom and one is missing from a blank document, so they are
// supplied here rather than in the modules, which run unchanged in a browser.
//
// Nothing here decides a result: the pure functions under test read no DOM,
// and the frame below only has to exist because a module binds to it on import.

// jsdom implements no CSS media query, and the shell resolves the system theme
// through one at import time.
if (!window.matchMedia) {
  window.matchMedia = (query) => ({
    media: query,
    matches: false,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  });
}

// The element the screens paint into, and the one the keyboard map binds to at
// import. `web/index.html` is the real document; this is the part of it the
// modules reach for while they are being imported. It runs at setup, not in a
// hook, because the import that reads it is the test module's own import.
if (!document.getElementById("main")) {
  const main = document.createElement("main");
  main.id = "main";
  document.body.appendChild(main);
}
