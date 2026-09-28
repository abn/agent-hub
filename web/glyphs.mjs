// Design round 4 glyph set. 24x24 viewBox, stroke 1.7 currentColor.
// Glyphs are inline SVG strings with currentColor, aria-hidden, space after 'M'.

export const GLYPH_PATHS = {
  // The comment anchor, added in round 14 with the knowledge base's comment
  // threads: a pin, because the anchor is a place in the document.
  anchorPin:
    '<path d="M12 21s6-6.2 6-10.2a6 6 0 0 0-12 0C6 14.8 12 21 12 21z"/><circle cx="12" cy="10.5" r="2"/>',
  chevronBack: '<path d="M 15 5l-7 7 7 7"/>',
  // Two crossing strokes. Close was a word in one place and a down chevron
  // in two others, and a chevron says the sheet goes somewhere rather than
  // away.
  close: '<path d="M 6 6l12 12"/><path d="M 18 6L6 18"/>',
  chevronDown: '<path d="M 6 9l6 6 6-6"/>',
  chevronRight: '<path d="M 9 5l7 7-7 7"/>',
  // Round 8: corners to 2, tail 3 deep on a 12 body with its base pulled
  // inboard to x=8. A hard rectangle with a tail at the corner read as a flag
  // at 20px on a phone, which is where it had to work and had never been seen.
  comments:
    '<path d="M 5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-7l-4 3v-3H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z"/>',
  // A sheet and its duplicate. Added in round 11, drawn in round 14 wherever a
  // value is lifted out of the page.
  copy: '<rect x="9" y="9" width="10" height="10" rx="2"/><path d="M 15 9V6.5A1.5 1.5 0 0 0 13.5 5h-7A1.5 1.5 0 0 0 5 6.5v7A1.5 1.5 0 0 0 6.5 15H9"/>',
  copyRaw: '<path d="M 9 7l-4 5 4 5"/><path d="M 15 7l4 5-4 5"/>',
  lock: '<rect x="5" y="10" width="14" height="10" rx="2"/><path d="M 8 10V7a4 4 0 0 1 8 0v3"/>',
  overflow:
    '<g fill="currentColor"><circle cx="12" cy="5" r="1.7"/><circle cx="12" cy="12" r="1.7"/><circle cx="12" cy="19" r="1.7"/></g>',
  resolve: '<path d="M 5 12l5 5 9-9"/>',
  // The owner's call, against the designer's: a 40px circle cannot hold a word
  // legibly, and every keyboard on a phone puts an arrow where this button is.
  // Three strokes, one motif, nothing inside anything.
  send: '<path d="M 12 20V5"/><path d="M 6 11l6-6 6 6"/>',
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
