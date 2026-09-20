// The REST client. One wrapper carries the bearer token, the JSON content
// type, and the RFC 9457 problem body, so no caller reads a response twice.

import { prefs } from "./prefs.mjs";

export async function api(path, options = {}) {
  // A token passed here is one being checked before it is kept, so it wins
  // over the stored one without replacing it.
  const { token, ...init } = options;
  const headers = Object.assign({}, init.headers || {});
  const bearer = token || prefs.token;
  if (bearer) headers.Authorization = "Bearer " + bearer;
  if (init.body) headers["Content-Type"] = "application/json";
  const response = await fetch(path, Object.assign({}, init, { headers }));
  if (!response.ok) {
    let detail = response.statusText;
    let code = "";
    try {
      const problem = await response.json();
      detail = problem.detail || problem.title || detail;
      code = problem.code || "";
    } catch {}
    // The words are for the reader; the status and the hub's own code are for
    // the screen, so no caller has to recognise a refusal by its wording. A
    // refusal to authenticate comes through here too: the screen that asks
    // for a token needs the hub's reason, and a sentence of our own in its
    // place would say the same thing whatever went wrong.
    const refused = new Error(detail);
    refused.status = response.status;
    refused.code = code;
    throw refused;
  }
  return response.status === 204 ? null : response.json();
}
