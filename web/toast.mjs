// A transient line at the foot of the screen, with an optional undo for the
// actions that are reversible for a short window.

export function toast(message, undo) {
  const el = document.createElement("div");
  el.className = "card";
  el.style.position = "fixed";
  el.style.left = "var(--s-4)";
  el.style.right = "var(--s-4)";
  el.style.bottom = "72px";
  el.style.zIndex = "20";
  const row = document.createElement("div");
  row.className = "row";
  row.style.border = "0";
  const text = document.createElement("span");
  text.className = "grow";
  text.textContent = message;
  row.appendChild(text);
  if (undo) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "primary";
    button.textContent = "Undo";
    button.addEventListener("click", () => {
      undo().finally(() => el.remove());
    });
    row.appendChild(button);
  }
  el.appendChild(row);
  document.body.appendChild(el);
  setTimeout(() => el.remove(), undo ? 30000 : 5000);
}
