// A line diff between two texts, for reading a page's earlier version against
// the page as it is now. It is Myers' shortest edit script over lines, after
// the common head and tail are set aside, so a small edit to a long page costs
// little. An edit script longer than MAX_EDITS is not looked for: the diff is
// then `null`, and the reader is told there are too many changes to show line
// by line rather than shown counts that do not describe the edit.

const MAX_EDITS = 2000;

export function splitLines(text) {
  const value = String(text ?? "");
  if (!value) return [];
  const lines = value.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

// Every line of `before` and `after` in order, each marked `same`, `del` (only
// in before) or `add` (only in after), or `null` when the two differ by more
// than MAX_EDITS lines.
export function lineDiff(before, after, maxEdits = MAX_EDITS) {
  const a = splitLines(before);
  const b = splitLines(after);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) {
    tail += 1;
  }
  const changed = middle(a.slice(head, a.length - tail), b.slice(head, b.length - tail), maxEdits);
  if (!changed) return null;
  const out = a.slice(0, head).map((text) => ({ kind: "same", text }));
  out.push(...changed);
  out.push(...a.slice(a.length - tail).map((text) => ({ kind: "same", text })));
  return out;
}

function middle(a, b, maxEdits) {
  const n = a.length;
  const m = b.length;
  if (!n) return b.map((text) => ({ kind: "add", text }));
  if (!m) return a.map((text) => ({ kind: "del", text }));
  const max = Math.min(n + m, maxEdits);
  const offset = max + 1;
  const v = new Array(2 * max + 3).fill(0);
  // Depth d reads only the diagonals -d..d of the depth before it, so each
  // step keeps that slice rather than the whole array.
  const trace = [];
  for (let d = 0; d <= max; d += 1) {
    trace.push(v.slice(offset - d, offset + d + 1));
    for (let k = -d; k <= d; k += 2) {
      let x =
        k === -d || (k !== d && v[offset + k - 1] < v[offset + k + 1]) ? v[offset + k + 1] : v[offset + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x += 1;
        y += 1;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) return backtrack(trace, a, b, d);
    }
  }
  return null;
}

// trace[d] holds diagonals -d..d, so diagonal k of it is at index k + d.
function backtrack(trace, a, b, depth) {
  const out = [];
  let x = a.length;
  let y = b.length;
  for (let d = depth; d > 0; d -= 1) {
    const v = trace[d];
    const at = (diagonal) => v[diagonal + d];
    const k = x - y;
    const down = k === -d || (k !== d && at(k - 1) < at(k + 1));
    const prevK = down ? k + 1 : k - 1;
    const prevX = at(prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      x -= 1;
      y -= 1;
      out.push({ kind: "same", text: a[x] });
    }
    if (down) {
      y -= 1;
      out.push({ kind: "add", text: b[y] });
    } else {
      x -= 1;
      out.push({ kind: "del", text: a[x] });
    }
  }
  while (x > 0 && y > 0) {
    x -= 1;
    y -= 1;
    out.push({ kind: "same", text: a[x] });
  }
  return out.reverse();
}

// Whether two texts differ only in whether the last line ends with a newline.
// The line diff reads both as the same lines, so a reader is told this apart
// rather than shown no change.
export function onlyFinalNewline(before, after) {
  const a = String(before ?? "");
  const b = String(after ?? "");
  return a !== b && splitLines(a).join("\n") === splitLines(b).join("\n");
}

// How many lines a diff adds and removes.
export function diffStat(lines) {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === "add") added += 1;
    if (line.kind === "del") removed += 1;
  }
  return { added, removed };
}

// The diff as a reader sees it: every changed line, `context` unchanged lines
// either side of a change, and each longer unchanged run folded to one `skip`
// entry carrying how many lines it stands for.
export function hunks(lines, context = 3) {
  const near = new Array(lines.length).fill(false);
  lines.forEach((line, index) => {
    if (line.kind === "same") return;
    for (let at = Math.max(0, index - context); at <= Math.min(lines.length - 1, index + context); at += 1) {
      near[at] = true;
    }
  });
  const out = [];
  let skipped = 0;
  lines.forEach((line, index) => {
    if (near[index]) {
      if (skipped) out.push({ kind: "skip", count: skipped });
      skipped = 0;
      out.push(line);
    } else {
      skipped += 1;
    }
  });
  if (skipped) out.push({ kind: "skip", count: skipped });
  return out;
}
