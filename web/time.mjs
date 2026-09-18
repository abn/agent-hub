// Timestamps as the screens read them. The hub writes RFC 3339, and the
// surface shows the minute, never the second.

export function stamp(ts) {
  return String(ts).slice(0, 16).replace("T", " ");
}

// A day bucket for grouping: Today, Yesterday, or the date.
export function dayOf(ts, now) {
  const date = new Date(ts);
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  const diff = Math.round((start - day) / 86400000);
  if (diff <= 0) return "Today";
  if (diff === 1) return "Yesterday";
  return date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

// Group events into day buckets, newest day first, preserving order inside a
// day. The feed and Home both use this, so their shape stays identical.
export function byDay(events) {
  const now = new Date();
  const groups = [];
  for (const event of events) {
    const label = dayOf(event.created_at, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.events.push(event);
    else groups.push({ label, events: [event] });
  }
  return groups;
}
