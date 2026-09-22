// Design round 4 glyph set. 24x24 viewBox, stroke 1.7 currentColor.
// Glyphs are inline SVG strings with currentColor, aria-hidden, space after 'M'.

export const GLYPH_PATHS = {
  anchorPin:
    '<path d="M 12 21s6-6.2 6-10.2a6 6 0 0 0-12 0C6 14.8 12 21 12 21z"/><circle cx="12" cy="10.5" r="2"/>',
  bell: '<path d="M 12 4a5 5 0 0 0-5 5v4l-2 3h14l-2-3V9a5 5 0 0 0-5-5z"/><path d="M 10 19a2 2 0 0 0 4 0"/>',
  bellOff:
    '<path d="M 12 4a5 5 0 0 0-5 5v4l-2 3h14l-2-3V9a5 5 0 0 0-5-5z"/><path d="M 4 4l16 16"/>',
  check: '<path d="M 5 13l4 4 10-10"/>',
  chevronBack: '<path d="M 15 5l-7 7 7 7"/>',
  chevronDown: '<path d="M 6 9l6 6 6-6"/>',
  chevronRight: '<path d="M 9 5l7 7-7 7"/>',
  // Round 8: corners to 2, tail 3 deep on a 12 body with its base pulled
  // inboard to x=8. A hard rectangle with a tail at the corner read as a flag
  // at 20px on a phone, which is where it had to work and had never been seen.
  comments:
    '<path d="M 5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-7l-4 3v-3H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"/>',
  copyRaw: '<path d="M 9 7l-4 5 4 5"/><path d="M 15 7l4 5-4 5"/>',
  idCard:
    '<rect x="3" y="5.5" width="18" height="13" rx="2.2"/><circle cx="9" cy="11" r="1.9"/><path d="M 6.3 15.7a3 3 0 0 1 5.4 0M 14.5 10.5h3.5M 14.5 14h3.5"/>',
  key: '<circle cx="8" cy="12" r="4"/><path d="M 12 12h9M 18 12v4"/>',
  link: '<path d="M 10 13a4 4 0 0 0 6 .5l2-2a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M 14 11a4 4 0 0 0-6-.5l-2 2A4 4 0 0 0 11.7 18l1-1"/>',
  lock: '<rect x="5" y="10" width="14" height="10" rx="2"/><path d="M 8 10V7a4 4 0 0 1 8 0v3"/>',
  overflow:
    '<g fill="currentColor"><circle cx="12" cy="5" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="12" cy="19" r="1.7"/></g>',
  resolve: '<path d="M 5 12l5 5 9-9"/>',
  signOut: '<path d="M 14 5H6v14h8"/><path d="M 13 12h8M 18 9l3 3-3 3"/>',
  trash: '<path d="M 5 7h14M 9 7V5h6v2M 7 7l1 13h8l1-13"/>',
};

export function glyphSvg(name, { size = 20, strokeWidth = 1.7, className = "" } = {}) {
  const path = GLYPH_PATHS[name];
  if (!path) return "";
  const cls = className ? ` class="${className}"` : "";
  if (name === "overflow") {
    return `<svg${cls} width="${size}" height="${size}" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">${path}</svg>`;
  }
  return `<svg${cls} width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="${strokeWidth}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;
}
