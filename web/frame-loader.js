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

var isProtected = false;
var currentCallout = null;

function injectStyles() {
  if (document.getElementById("hub-comment-styles")) return;
  var style = document.createElement("style");
  style.id = "hub-comment-styles";
  style.textContent =
    '.hub-comment-highlight{background:#E6ECF7;box-shadow:inset 0 -2px 0 #2F5FA8;border-radius:2px;color:#1D1C19;cursor:pointer}' +
    '.hub-comment-highlight:focus,.hub-comment-highlight:hover,.hub-comment-highlight.active{box-shadow:inset 0 -2px 0 #2F5FA8,0 0 0 3px #E6ECF7;outline:none}' +
    '.hub-point-pin{position:absolute;left:4px;width:16px;height:16px;color:#2F5FA8;cursor:pointer}' +
    '.hub-selection-callout{position:absolute;min-height:44px;min-width:88px;padding:0 16px;background:#FFFFFF;color:#1D1C19;border:1px solid #CFC9BE;border-radius:8px;box-shadow:0 4px 12px rgba(0,0,0,0.15);font:600 13px/1 system-ui,sans-serif;display:flex;align-items:center;justify-content:center;z-index:1000;cursor:pointer}' +
    'html[data-theme="dark"] .hub-comment-highlight{background:#223052;box-shadow:inset 0 -2px 0 #8AAAE8;color:#ECE8E0}' +
    'html[data-theme="dark"] .hub-comment-highlight:focus,html[data-theme="dark"] .hub-comment-highlight:hover,html[data-theme="dark"] .hub-comment-highlight.active{box-shadow:inset 0 -2px 0 #8AAAE8,0 0 0 3px #223052}' +
    'html[data-theme="dark"] .hub-point-pin{color:#8AAAE8}' +
    'html[data-theme="dark"] .hub-selection-callout{background:#1D1C19;color:#ECE8E0;border-color:#44403A}';
  (document.head || document.documentElement).appendChild(style);
}

document.addEventListener("DOMContentLoaded", function () {
  injectStyles();
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
  notifyParent({ type: "hub:frame-ready" });
});

window.addEventListener("load", function () {
  postHeight();
  notifyParent({ type: "hub:frame-ready" });
});

if (typeof ResizeObserver !== "undefined") {
  new ResizeObserver(postHeight).observe(document.documentElement);
}

function postHeight() {
  var root = document.documentElement.scrollHeight;
  var body = document.body ? document.body.scrollHeight : 0;
  var height = root > body ? root : body;
  notifyParent({ hubFrameHeight: height });
}

function notifyParent(data) {
  try {
    if (window.parent && window.parent !== window) {
      window.parent.postMessage(data, "*");
    }
  } catch (_) {}
}

function normalizeText(s) {
  return String(s || "").replace(/\s+/g, " ").trim().toLowerCase();
}

// Anchor context keeps its boundary spaces: the space next to the quote is
// part of where it sits, and trimming it is what left a prefix and the text
// before an occurrence a character out of step. Collapsed and lowercased, but
// not trimmed.
function normalizeContext(s) {
  return String(s || "").replace(/\s+/g, " ").toLowerCase();
}

function getTextNodes(root) {
  var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: function (node) {
      var parent = node.parentElement;
      if (!parent) return NodeFilter.FILTER_REJECT;
      var tag = parent.tagName.toLowerCase();
      if (
        tag === "script" ||
        tag === "style" ||
        tag === "noscript" ||
        parent.classList.contains("hub-selection-callout")
      ) {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  var nodes = [];
  var n;
  while ((n = walker.nextNode())) {
    nodes.push(n);
  }
  return nodes;
}

// The document as one normalised string, with the maps a range needs: where
// each text node starts, and which normalised position each raw character
// holds. Selection-time context and match-time resolution both read this, so
// the two agree on whitespace and case.
function flattenText(root) {
  var textNodes = getTextNodes(root);
  var chars = [];
  var starts = new Map();
  var lengths = new Map();
  for (var i = 0; i < textNodes.length; i++) {
    var node = textNodes[i];
    starts.set(node, chars.length);
    var text = node.nodeValue;
    lengths.set(node, text.length);
    for (var j = 0; j < text.length; j++) {
      chars.push({ char: text[j], node: node, offset: j });
    }
    if (
      i < textNodes.length - 1 &&
      textNodes[i].parentElement !== textNodes[i + 1].parentElement
    ) {
      chars.push({ char: " ", node: null, offset: -1 });
    }
  }

  var normStr = "";
  var normMap = [];
  var normAt = new Array(chars.length);
  var inWhitespace = false;
  for (var cIdx = 0; cIdx < chars.length; cIdx++) {
    var c = chars[cIdx].char;
    if (/\s/.test(c)) {
      if (!inWhitespace) {
        normAt[cIdx] = normStr.length;
        normStr += " ";
        normMap.push(cIdx);
        inWhitespace = true;
      } else {
        normAt[cIdx] = normStr.length - 1;
      }
    } else {
      normAt[cIdx] = normStr.length;
      normStr += c.toLowerCase();
      normMap.push(cIdx);
      inWhitespace = false;
    }
  }
  return { chars: chars, starts: starts, lengths: lengths, normStr: normStr, normMap: normMap, normAt: normAt };
}

// The document's flattened form, kept until the highlights are repainted. A
// selection drag fires selectionchange many times and none of them change the
// document, so the drag reads this rather than reflattening each time. The
// only content mutation a frame sees is the highlight spans applyComments
// inserts; the callout beside a selection is excluded from the flatten by
// getTextNodes, so it does not retire this.
var documentText = null;

function cachedText() {
  if (!documentText) documentText = flattenText(document.body);
  return documentText;
}

// The normalised text just before and just after a selection: the context a
// quote carries so a repeated word resolves to the occurrence the reader
// chose. Capped, since it is a pointer rather than the words themselves.
function selectionContext(range) {
  var max = 32;
  var startNode = range.startContainer;
  var endNode = range.endContainer;
  if (startNode.nodeType !== 3 || endNode.nodeType !== 3) {
    return { prefix: "", suffix: "" };
  }
  var flat = cachedText();
  // An artifact's own inline script can change a text node after the flatten
  // was kept. The mapping is good only while the node still holds the text it
  // was built from, so a changed node rebuilds it once.
  if (
    flat.lengths.get(startNode) !== startNode.nodeValue.length ||
    flat.lengths.get(endNode) !== endNode.nodeValue.length
  ) {
    documentText = null;
    flat = cachedText();
  }
  var startBase = flat.starts.get(startNode);
  var endBase = flat.starts.get(endNode);
  var startLen = flat.lengths.get(startNode);
  var endLen = flat.lengths.get(endNode);
  if (startBase === undefined || endBase === undefined) {
    return { prefix: "", suffix: "" };
  }
  // The quote is trimmed, so the context is measured from the first and last
  // non-space characters the reader selected. A drag that caught the space
  // beside the word still lines up with where the quote sits, instead of
  // ending one character short of it.
  var chars = flat.chars;
  var first = startBase + range.startOffset;
  var last = endBase + range.endOffset - 1;
  if (first < startBase || first >= startBase + startLen || last < endBase || last >= endBase + endLen) {
    return { prefix: "", suffix: "" };
  }
  while (first <= last && first < chars.length && /\s/.test(chars[first].char)) first++;
  while (last >= first && last >= 0 && /\s/.test(chars[last].char)) last--;
  if (first > last || first >= chars.length || last < 0) {
    return { prefix: "", suffix: "" };
  }
  var startNorm = flat.normAt[first];
  var endNorm = flat.normAt[last];
  if (!Number.isFinite(startNorm) || !Number.isFinite(endNorm)) {
    return { prefix: "", suffix: "" };
  }
  return {
    prefix: flat.normStr.slice(Math.max(0, startNorm - max), startNorm),
    suffix: flat.normStr.slice(endNorm + 1, endNorm + 1 + max),
  };
}

function sharedPrefixLen(a, b) {
  var n = Math.min(a.length, b.length);
  var i = 0;
  while (i < n && a[i] === b[i]) i++;
  return i;
}

function sharedSuffixLen(a, b) {
  var n = Math.min(a.length, b.length);
  var i = 0;
  while (i < n && a[a.length - 1 - i] === b[b.length - 1 - i]) i++;
  return i;
}

// The quote's range in the document. A quote that repeats is resolved by the
// context it was selected from: each occurrence is scored by how much of the
// prefix and suffix sits around it, and the best one wins. With no context, or
// none that matches, the first occurrence is taken, as it always was.
function findQuoteRange(root, normQuote, normPrefix, normSuffix) {
  if (!normQuote) return null;
  var flat = flattenText(root);
  if (!flat.normStr) return null;

  var candidates = [];
  var from = 0;
  while (from <= flat.normStr.length) {
    var at = flat.normStr.indexOf(normQuote, from);
    if (at === -1) break;
    candidates.push(at);
    from = at + 1;
  }
  if (!candidates.length) return null;

  var chosen = candidates[0];
  if (normPrefix || normSuffix) {
    var best = 0;
    var bestScore = -1;
    for (var k = 0; k < candidates.length; k++) {
      var score = 0;
      var hit = candidates[k];
      if (normPrefix) {
        var before = flat.normStr.slice(Math.max(0, hit - normPrefix.length), hit);
        score += sharedSuffixLen(before, normPrefix);
      }
      if (normSuffix) {
        var after = flat.normStr.slice(hit + normQuote.length, hit + normQuote.length + normSuffix.length);
        score += sharedPrefixLen(after, normSuffix);
      }
      if (score > bestScore) {
        bestScore = score;
        best = k;
      }
    }
    chosen = candidates[best];
  }

  var startCharIdx = flat.normMap[chosen];
  var endCharIdx = flat.normMap[chosen + normQuote.length - 1];

  while (startCharIdx < flat.chars.length && !flat.chars[startCharIdx].node) {
    startCharIdx++;
  }
  var startItem = flat.chars[startCharIdx];

  while (endCharIdx >= 0 && !flat.chars[endCharIdx].node) {
    endCharIdx--;
  }
  var endItem = flat.chars[endCharIdx];

  if (!startItem || !endItem || !startItem.node || !endItem.node) return null;

  var range = document.createRange();
  range.setStart(startItem.node, startItem.offset);
  range.setEnd(endItem.node, endItem.offset + 1);
  return range;
}

function clearHighlights() {
  var highlights = document.querySelectorAll(".hub-comment-highlight");
  highlights.forEach(function (span) {
    var parent = span.parentNode;
    if (!parent) return;
    while (span.firstChild) {
      parent.insertBefore(span.firstChild, span);
    }
    span.remove();
  });
  var pins = document.querySelectorAll(".hub-point-pin");
  pins.forEach(function (pin) {
    pin.remove();
  });
}

function applyComments(comments, shownVersion) {
  // The highlight spans clearHighlights and this pass insert or remove change
  // the text nodes the flatten points at, so the cached one is retired here.
  clearHighlights();
  documentText = null;
  if (!Array.isArray(comments) || !comments.length) {
    if (document.body) document.body.classList.remove("has-comments");
    return;
  }

  var activeComments = comments.filter(function (c) {
    return !c.done && c.anchor_version === shownVersion;
  });

  if (document.body) {
    if (comments.length > 0) {
      document.body.classList.add("has-comments");
    } else {
      document.body.classList.remove("has-comments");
    }
  }

  for (var i = 0; i < activeComments.length; i++) {
    (function (comment) {
      var anchor = comment.anchor;
      if (!anchor || typeof anchor !== "object") return;

      if (anchor.mode === "text" && typeof anchor.quote === "string" && anchor.quote) {
        var normQuote = normalizeText(anchor.quote);
        if (!normQuote) return;
        var normPrefix = normalizeContext(anchor.prefix || "");
        var normSuffix = normalizeContext(anchor.suffix || "");
        var range = findQuoteRange(document.body, normQuote, normPrefix, normSuffix);
        if (range) {
          var span = document.createElement("span");
          span.className = "hub-comment-highlight";
          span.tabIndex = 0;
          span.setAttribute("role", "button");
          span.setAttribute("aria-label", "comment on this text");
          span.dataset.commentId = String(comment.id);
          try {
            range.surroundContents(span);
          } catch (_) {
            var fragment = range.extractContents();
            span.appendChild(fragment);
            range.insertNode(span);
          }
          span.addEventListener("click", function (e) {
            e.stopPropagation();
            span.classList.add("active");
            notifyParent({ type: "hub:open-comment", commentId: comment.id });
          });
          span.addEventListener("keydown", function (e) {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              span.classList.add("active");
              notifyParent({ type: "hub:open-comment", commentId: comment.id });
            }
          });
          span.addEventListener("mouseenter", function () {
            notifyParent({ type: "hub:hover-comment", commentId: comment.id, active: true });
          });
          span.addEventListener("mouseleave", function () {
            notifyParent({ type: "hub:hover-comment", commentId: comment.id, active: false });
          });
          span.addEventListener("focus", function () {
            notifyParent({ type: "hub:hover-comment", commentId: comment.id, active: true });
          });
          span.addEventListener("blur", function () {
            notifyParent({ type: "hub:hover-comment", commentId: comment.id, active: false });
          });
        }
      } else if (anchor.mode === "point") {
        var pin = document.createElement("div");
        pin.className = "hub-point-pin";
        pin.tabIndex = 0;
        pin.setAttribute("role", "button");
        pin.setAttribute("aria-label", "pinned comment");
        pin.dataset.commentId = String(comment.id);
        pin.style.top = Math.max(10, Math.round(anchor.y || 80)) + "px";
        pin.innerHTML =
          '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 12 21s6-6.2 6-10.2a6 6 0 0 0-12 0C6 14.8 12 21 12 21z"/><circle cx="12" cy="10.5" r="2"/></svg>';
        pin.addEventListener("click", function (e) {
          e.stopPropagation();
          notifyParent({ type: "hub:open-comment", commentId: comment.id });
        });
        pin.addEventListener("keydown", function (e) {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            notifyParent({ type: "hub:open-comment", commentId: comment.id });
          }
        });
        document.body.appendChild(pin);
      }
    })(activeComments[i]);
  }
}

function removeCallout() {
  if (currentCallout) {
    currentCallout.remove();
    currentCallout = null;
  }
}

function handleSelection() {
  var sel = window.getSelection();
  if (!sel || sel.isCollapsed || !sel.rangeCount) {
    removeCallout();
    notifyParent({ type: "hub:selection-change", quote: "" });
    return;
  }
  var text = sel.toString().trim();
  if (!text) {
    removeCallout();
    notifyParent({ type: "hub:selection-change", quote: "" });
    return;
  }
  var range = sel.getRangeAt(0);
  var context = selectionContext(range);
  notifyParent({
    type: "hub:selection-change",
    quote: text,
    prefix: context.prefix,
    suffix: context.suffix,
  });
  if (isProtected) {
    return;
  }
  // Under a coarse pointer the shell raises a fixed button instead. Drawing a
  // callout beside the selection here would put it exactly where Android
  // renders Copy, Select all, Share and Read aloud, which is browser chrome
  // and always paints on top, so the control was there and unreachable.
  if (window.matchMedia && window.matchMedia("(pointer: coarse)").matches) {
    return;
  }

  var rect = range.getBoundingClientRect();
  if (!rect || (rect.width === 0 && rect.height === 0)) return;

  removeCallout();
  var callout = document.createElement("button");
  callout.type = "button";
  callout.className = "hub-selection-callout";
  callout.textContent = "Comment";
  callout.setAttribute("aria-label", "Comment on selection");

  var top = Math.max(10, rect.top + window.scrollY - 50);
  var left = Math.max(10, rect.left + window.scrollX + rect.width / 2 - 44);
  callout.style.top = top + "px";
  callout.style.left = left + "px";

  var create = function () {
    notifyParent({
      type: "hub:create-comment",
      quote: text,
      prefix: context.prefix,
      suffix: context.suffix,
    });
  };
  callout.addEventListener("pointerdown", function (e) {
    e.preventDefault();
    e.stopPropagation();
    create();
    removeCallout();
  });
  callout.addEventListener("click", function (e) {
    e.preventDefault();
    e.stopPropagation();
    create();
    removeCallout();
  });
  document.body.appendChild(callout);
  currentCallout = callout;
}

document.addEventListener("selectionchange", handleSelection);
document.addEventListener("mouseup", handleSelection);
document.addEventListener("touchend", handleSelection);
document.addEventListener("mousedown", function (e) {
  if (currentCallout && !currentCallout.contains(e.target)) {
    removeCallout();
  }
});
document.addEventListener("keydown", function (e) {
  if (e.key === "Escape" && currentCallout) {
    removeCallout();
  }
});

window.addEventListener("message", function (event) {
  var data = event.data;
  if (!data || typeof data !== "object") return;
  if (data.type === "hub:set-comments") {
    isProtected = !!data.protected;
    applyComments(data.comments, data.shownVersion);
  } else if (data.type === "hub:highlight-comment") {
    var highlights = document.querySelectorAll(
      '.hub-comment-highlight[data-comment-id="' + data.commentId + '"]'
    );
    highlights.forEach(function (el) {
      if (data.active) el.classList.add("active");
      else el.classList.remove("active");
    });
  } else if (data.type === "hub:scroll-to-comment") {
    var target = document.querySelector(
      '.hub-comment-highlight[data-comment-id="' +
        data.commentId +
        '"], .hub-point-pin[data-comment-id="' +
        data.commentId +
        '"]'
    );
    if (target) {
      target.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }
});
