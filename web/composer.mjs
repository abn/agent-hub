// The control a reply is written in, in place of the browser's prompt box.
//
// It is multi-line, it keeps what was typed when the send fails, and it says
// why in the component rather than in a modal the reader has to dismiss before
// they can see their own words again. Screens build one, put it where the
// reply belongs, and hand it the call that sends.

let sequence = 0;

// A phone keyboard offers Return as the line break and has no Shift beside it,
// so Enter sends only where a keyboard does. The send button is what sends on
// a touch screen, and it is the only path that is always there.
const SENDS_ON_ENTER = !window.matchMedia("(pointer: coarse)").matches;
// Spaced after each command letter: the compact form spells a shape the
// repository's identifier check rejects.
const SEND_PATH = "M 12 19V5 M 6 11l6-6 6 6";

function sendGlyph() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "16");
  svg.setAttribute("height", "16");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", SEND_PATH);
  svg.appendChild(path);
  return svg;
}

// label: what the field is, for a reader who cannot see what it sits under.
// placeholder: the resting line. send: what to call with the typed body; it
// rejects with the message the reader needs to see.
export function composer({ label, placeholder = "Or type a reply", send }) {
  sequence += 1;
  const errorId = `composer-error-${sequence}`;

  const form = document.createElement("form");
  form.className = "composer";

  const field = document.createElement("textarea");
  field.className = "composer-field";
  field.rows = 1;
  field.placeholder = placeholder;
  field.setAttribute("aria-label", label);
  field.setAttribute("aria-describedby", errorId);

  const button = document.createElement("button");
  button.type = "submit";
  button.className = "composer-send";
  button.setAttribute("aria-label", "Send");
  button.appendChild(sendGlyph());

  const error = document.createElement("p");
  error.className = "composer-error";
  error.id = errorId;
  error.setAttribute("role", "alert");
  error.hidden = true;

  const row = document.createElement("div");
  row.className = "composer-row";
  row.append(field, button);
  form.append(row, error);

  function fail(message) {
    error.hidden = false;
    error.textContent = message;
    field.focus();
  }

  function busy(state) {
    field.disabled = state;
    button.disabled = state;
    form.setAttribute("aria-busy", String(state));
  }

  // The field rests at one line and grows with what is written in it, so a
  // long answer is visible while it is being written.
  field.addEventListener("input", () => {
    field.style.height = "auto";
    field.style.height = `${field.scrollHeight}px`;
  });

  field.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey || !SENDS_ON_ENTER) return;
    event.preventDefault();
    form.requestSubmit();
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const body = field.value.trim();
    if (!body) {
      fail("Nothing was sent: the reply is empty.");
      return;
    }
    error.hidden = true;
    error.textContent = "";
    busy(true);
    try {
      await send(body);
    } catch (problem) {
      // What was typed stays in the field: it is the reader's, and the send is
      // the only thing that failed.
      busy(false);
      fail(`Nothing was sent: ${problem.message}`);
    }
  });

  return { element: form, focus: () => field.focus() };
}
