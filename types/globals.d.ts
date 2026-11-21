// Globals the client reads that the DOM typings do not know: scripts loaded on
// demand, and the hooks the pages expose for tests to read.
interface Window {
  marked?: { parse(source: string, options?: object): string };
  mermaid?: any;
  __router?: { screens(): string[] };
  __sync?: object;
}
