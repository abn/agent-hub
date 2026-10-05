// Start a seeded hub for each project before the run, and stop them after it.
import { startHubs } from "./hub.mjs";

export default async function globalSetup(config) {
  return startHubs(config.projects.map((project) => project.name));
}
