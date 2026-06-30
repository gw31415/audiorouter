#!/usr/bin/env node
import { spawn, type ChildProcess, type SpawnOptions } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dashboardDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const workspaceDir = resolve(dashboardDir, "../../..");
const apiAddr = process.env.AUDIOROUTER_DASHBOARD_ADDR ?? "127.0.0.1:7822";
const apiUrl = `http://${apiAddr}`;

const cargoEnv: NodeJS.ProcessEnv = {
  ...process.env,
  AUDIOROUTER_DASHBOARD_ADDR: apiAddr,
};
const viteEnv: NodeJS.ProcessEnv = {
  ...process.env,
  AUDIOROUTER_API: apiUrl,
};

const children = new Set<ChildProcess>();
let shuttingDown = false;

function run(name: string, command: string, args: string[], options: SpawnOptions): ChildProcess {
  const child = spawn(command, args, {
    stdio: "inherit",
    ...options,
  });
  children.add(child);
  child.on("exit", (code, signal) => {
    children.delete(child);
    if (!shuttingDown) {
      shuttingDown = true;
      for (const other of children) other.kill("SIGTERM");
      if (code === 0 || signal) process.exit(0);
      console.error(`${name} exited with code ${code}`);
      process.exit(code ?? 1);
    }
  });
  return child;
}

function shutdown(): void {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const child of children) child.kill("SIGTERM");
}

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
process.on("exit", shutdown);

console.log(`Starting audiorouter-dashboard-api at ${apiUrl}`);
run(
  "audiorouter-dashboard-api",
  "cargo",
  [
    "run",
    "-p",
    "audiorouter-dashboard",
    "--bin",
    "audiorouter-dashboard-api",
    "--",
    "--addr",
    apiAddr,
  ],
  { cwd: workspaceDir, env: cargoEnv },
);

console.log(`Starting Vite dev server with /api proxy -> ${apiUrl}`);
run("vite", "pnpm", ["exec", "vp", "dev"], { cwd: dashboardDir, env: viteEnv });
