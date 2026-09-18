// Shared frame runtime for artifact pages.
//
// Both frame paths reference this file instead of inlining scripts: the
// `/frame` route for stored HTML artifacts and the host-assembled srcdoc
// for rendered markdown. An external same-origin script runs under the
// inherited and frame policies alike, where an inline script would be
// blocked.
//
// The theme comes from the document this runs in, so both paths only
// stamp `data-theme`. The runtime bundle exposes either the bare API or
// an interop wrapper with the API under `default`; resolve both. Height
// reports let the host size the frame to its body: the frame is opaque,
// so the host cannot measure it directly.
document.addEventListener("DOMContentLoaded", function () {
  var dark = document.documentElement.getAttribute("data-theme") === "dark";
  var runtime = window.mermaid;
  var api =
    runtime && typeof runtime.initialize === "function"
      ? runtime
      : runtime && runtime.default;
  if (
    api &&
    typeof api.initialize === "function" &&
    typeof api.run === "function" &&
    document.querySelector(".mermaid")
  ) {
    api.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
    api.run({ querySelector: ".mermaid" });
  }
  postHeight();
});

window.addEventListener("load", postHeight);

if (typeof ResizeObserver !== "undefined") {
  new ResizeObserver(postHeight).observe(document.documentElement);
}

function postHeight() {
  var root = document.documentElement.scrollHeight;
  var body = document.body ? document.body.scrollHeight : 0;
  var height = root > body ? root : body;
  parent.postMessage({ hubFrameHeight: height }, "*");
}
