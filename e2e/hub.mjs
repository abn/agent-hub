// The seeded hub the browser checks run against, and the Python bridge that
// holds it.
//
// One hub per project. A screen that has been read has moved the hub's own read
// cursor, and Home draws its unread dots from what is still above that cursor,
// so a project running after another would start from data the first one
// changed. Separate hubs keep the two runs independent, which is what lets
// each project assert against the same fixture the harness seeds.
//
// The seeding itself is the Python harness's, reached through a bridge program
// rather than a second implementation of it in JavaScript. The bridge writes
// this descriptor once the hub is up, and the tests read it, so the fixture
// strings are the harness's constants and not literals repeated in the specs.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, watch } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const BRIDGE = path.join(ROOT, "e2e", "hub-bridge.py");
// The throwaway side of the run lives under the build tree, like every other
// check's: it is a disk, and `cargo clean` empties what a killed run left.
const SCRATCH = path.join(ROOT, "target", "tmp", "e2e");

// How long a hub may take to start and seed before the run says so. The
// harness starts the binary, waits for it to answer, and then makes about forty
// MCP calls, so this is a ceiling rather than a wait: the descriptor appears the
// moment the seed is done.
const READY_MS = 60_000;

const descriptorFor = (project) => path.join(SCRATCH, `${project}.json`);

// The interpreter the harness runs under. It needs no browser of its own: the
// bridge is the Python harness's seeding, which imports nothing outside the
// standard library.
const python = process.env.PYTHON || "python3";

// The descriptor is waited for as an event rather than polled for on a timer:
// the bridge writes it when the seed is done, so there is nothing to guess at.
async function waitForDescriptor(child, descriptor, said) {
  if (existsSync(descriptor)) return JSON.parse(readFileSync(descriptor, "utf8"));
  const named = path.basename(descriptor);
  return new Promise((resolve, reject) => {
    let watcher = null;
    let ceiling = null;
    const stop = () => {
      watcher?.close();
      clearTimeout(ceiling);
    };
    watcher = watch(path.dirname(descriptor), (_event, name) => {
      if (name !== named && name !== null) return;
      if (!existsSync(descriptor)) return;
      stop();
      resolve(JSON.parse(readFileSync(descriptor, "utf8")));
    });
    child.once("exit", () => {
      stop();
      // The harness reports a missing binary by exiting green, so a run that
      // cannot start says the harness's own reason rather than a timeout.
      reject(new Error(`the hub bridge exited before describing a hub\n${said()}`));
    });
    ceiling = setTimeout(() => {
      stop();
      reject(new Error(`the hub bridge described nothing within ${READY_MS / 1000}s\n${said()}`));
    }, READY_MS);
  });
}

// A hub is reaped when its bridge exits, but a bridge killed with SIGKILL (an
// aborted run) orphans its hub, which holds a port and memory and poisons the
// next run. Every test hub and its bridge carry HUB_CHECK_HUB, so the sweep
// kills only a test process, never a real hub. It runs once, before any hub of
// this run starts, so the only marked process it can reach is an orphan.
function sweepStaleHubs() {
  let pids;
  try {
    pids = readdirSync("/proc");
  } catch {
    return; // Not Linux; there is no /proc to sweep.
  }
  for (const entry of pids) {
    if (!/^\d+$/.test(entry)) continue;
    let env;
    try {
      env = readFileSync(`/proc/${entry}/environ`);
    } catch {
      continue; // Another user's process, or gone.
    }
    if (!env.includes("HUB_CHECK_HUB=1")) continue;
    try {
      process.kill(Number(entry), "SIGKILL");
    } catch {
      // Gone between the read and the kill.
    }
  }
}

export async function startHubs(projects) {
  mkdirSync(SCRATCH, { recursive: true });
  sweepStaleHubs();
  const bridges = [];
  for (const project of projects) {
    const descriptor = descriptorFor(project);
    rmSync(descriptor, { force: true });
    const child = spawn(python, [BRIDGE, descriptor], {
      cwd: ROOT,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, HUB_CHECK_HUB: "1" },
    });
    let said = "";
    child.stdout.on("data", (chunk) => {
      said += chunk;
    });
    child.stderr.on("data", (chunk) => {
      said += chunk;
    });
    const hub = await waitForDescriptor(child, descriptor, () => said.trim());
    console.log(`e2e: ${project} runs against ${hub.baseUrl}`);
    bridges.push(child);
  }
  return async () => {
    for (const child of bridges) {
      const stopped = new Promise((resolve) => child.once("exit", resolve));
      child.kill("SIGTERM");
      await stopped;
    }
    rmSync(SCRATCH, { recursive: true, force: true });
  };
}

// What a test needs of the hub it runs against, read by the project's name.
export function readHub(project) {
  const descriptor = descriptorFor(project);
  if (!existsSync(descriptor)) {
    throw new Error(`no seeded hub for the ${project} project: ${descriptor} is missing`);
  }
  return JSON.parse(readFileSync(descriptor, "utf8"));
}
