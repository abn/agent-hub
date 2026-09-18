// The REST client. One wrapper carries the bearer token, the JSON content
// type, and the RFC 9457 problem body, so no caller reads a response twice.

import { prefs } from "./prefs.mjs";

export async function api(path, options = {}) {
  const headers = Object.assign({}, options.headers || {});
  if (prefs.token) headers.Authorization = "Bearer " + prefs.token;
  if (options.body) headers["Content-Type"] = "application/json";
  const response = await fetch(path, Object.assign({}, options, { headers }));
  if (response.status === 401) throw new Error("unauthorized: set a token in Settings");
  if (!response.ok) {
    let detail = response.statusText;
    try {
      const problem = await response.json();
      detail = problem.detail || problem.title || detail;
    } catch {}
    throw new Error(detail);
  }
  return response.status === 204 ? null : response.json();
}
