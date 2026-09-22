// Design round 4 glyph set. 24x24 viewBox, stroke 1.7 currentColor.
// Glyphs are inline SVG strings with currentColor, aria-hidden, space after 'M'.

export const GLYPH_PATHS = {
  anchorPin:
    '<path d="M 12 21s6-6.2 6-10.2a6 6 0 0 0-12 0C6 14.8 12 21 12 21z"/><circle cx="12" cy="10.5" r="2"/>',
  chevronBack: '<path d="M 15 5l-7 7 7 7"/>',
  chevronDown: '<path d="M 6 9l6 6 6-6"/>',
  chevronRight: '<path d="M 9 5l7 7-7 7"/>',
  comments: '<path d="M 4 5h16v11H9l-5 4z"/>',
  copyRaw: '<path d="M 9 7l-4 5 4 5"/><path d="M 15 7l4 5-4 5"/>',
  key: '<circle cx="8" cy="12" r="4"/><path d="M 12 12h9M 18 12v4"/>',
  link: '<path d="M 10 13a4 4 0 0 0 6 .5l2-2a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M 14 11a4 4 0 0 0-6-.5l-2 2A4 4 0 0 0 11.7 18l1-1"/>',
  lock: '<rect x="5" y="10" width="14" height="10" rx="2"/><path d="M 8 10V7a4 4 0 0 1 8 0v3"/>',
  overflow:
    '<g fill="currentColor"><circle cx="12" cy="5" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="12" cy="19" r="1.7"/></g>',
  resolve: '<path d="M 5 12l5 5 9-9"/>',
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
