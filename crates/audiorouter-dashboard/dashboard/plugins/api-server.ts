import { spawn, type ChildProcess } from "node:child_process";
import { connect } from "node:net";
import { resolve } from "node:path";
import type { Plugin, ViteDevServer } from "vite";

/**
 * Dev-only plugin: spawns the audiorouter-dashboard-api (Rust) server and
 * configures the Vite dev-server proxy for /api requests.
 *
 * In build mode this plugin is a no-op — no proxy, no subprocess.
 */
export function apiServer(options?: { addr?: string }): Plugin {
  const apiAddr = options?.addr ?? process.env.AUDIOROUTER_DASHBOARD_ADDR ?? "127.0.0.1:7822";
  const apiTarget = process.env.AUDIOROUTER_API ?? `http://${apiAddr}`;
  const workspaceDir = resolve(process.cwd(), "../../..");

  return {
    name: "dev-api-server",
    apply: "serve",

    // Inject the /api proxy config so the user doesn't need server.proxy
    // in vite.config.ts.
    config: () => ({
      server: {
        proxy: {
          "/api": {
            target: apiTarget,
            changeOrigin: true,
          },
        },
      },
    }),

    // Spawn the Rust API server alongside the Vite dev server.
    async configureServer(server) {
      installTtyEioGuard();
      console.log(`Starting audiorouter-dashboard-api at http://${apiAddr}`);

      const child = spawn(
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
        {
          cwd: workspaceDir,
          // The API server is non-interactive. Do not share stdin with Vite's
          // readline handler; on some TTYs, Ctrl-C/close surfaces as read EIO.
          stdio: ["ignore", "inherit", "inherit"],
          // The dev API binary does not serve embedded frontend assets, so it
          // should not run dashboard/build.rs -> pnpm build. Skipping that
          // removes a long window where Vite can accept /api requests before
          // the proxy target is listening.
          env: {
            ...process.env,
            AUDIOROUTER_DASHBOARD_ADDR: apiAddr,
            SKIP_DASHBOARD_BUILD: "1",
          },
        },
      );

      manageApiProcess(server, child);
      await waitForTcp(apiAddr, 30_000);
      console.log(`audiorouter-dashboard-api ready at http://${apiAddr}`);
    },
  };
}

function manageApiProcess(server: ViteDevServer, child: ChildProcess): void {
  let shuttingDown = false;
  let childExited = false;

  const stopChild = () => {
    if (!childExited && !child.killed) {
      child.kill("SIGTERM");
    }
  };

  const shutdownFromSignal = () => {
    shuttingDown = true;
    stopChild();
  };

  const closeVite = (exitCode: number) => {
    if (shuttingDown) {
      return;
    }
    shuttingDown = true;
    process.exitCode = exitCode;
    server
      .close()
      .catch((error: unknown) => {
        console.error("Failed to close Vite dev server after API server exit:", error);
        process.exitCode = 1;
      })
      .finally(() => {
        // Vite's dev CLI otherwise keeps the Node process alive after close().
        process.exit(process.exitCode ?? exitCode);
      });
  };

  child.once("error", (error) => {
    console.error("Failed to start audiorouter-dashboard-api:", error.message);
    closeVite(1);
  });

  child.once("exit", (code, signal) => {
    childExited = true;
    if (shuttingDown) {
      return;
    }

    const exitCode = code ?? (signal ? 1 : 0);
    if (exitCode === 0) {
      console.error("audiorouter-dashboard-api exited unexpectedly; stopping Vite dev server.");
    } else {
      console.error(
        `audiorouter-dashboard-api exited with ${signal ?? `code ${exitCode}`}; stopping Vite dev server.`,
      );
    }
    closeVite(exitCode === 0 ? 1 : exitCode);
  });

  process.once("SIGINT", shutdownFromSignal);
  process.once("SIGTERM", shutdownFromSignal);
  process.once("exit", stopChild);
  server.httpServer?.once("close", stopChild);
}

function waitForTcp(addr: string, timeoutMs: number): Promise<void> {
  const parsed = parseHostPort(addr);
  const startedAt = Date.now();

  return new Promise((resolveReady, rejectReady) => {
    const tryConnect = () => {
      const socket = connect({ host: parsed.host, port: parsed.port });
      socket.once("connect", () => {
        socket.destroy();
        resolveReady();
      });
      socket.once("error", (error) => {
        socket.destroy();
        if (Date.now() - startedAt >= timeoutMs) {
          rejectReady(
            new Error(
              `timed out waiting for audiorouter-dashboard-api at ${addr}: ${error.message}`,
            ),
          );
          return;
        }
        setTimeout(tryConnect, 100);
      });
    };

    tryConnect();
  });
}

function parseHostPort(addr: string): { host: string; port: number } {
  const url = new URL(`tcp://${addr}`);
  const port = Number(url.port);
  if (!url.hostname || !Number.isInteger(port)) {
    throw new Error(`invalid API address: ${addr}`);
  }
  return { host: url.hostname, port };
}

let ttyEioGuardInstalled = false;

function installTtyEioGuard(): void {
  if (ttyEioGuardInstalled) {
    return;
  }
  ttyEioGuardInstalled = true;

  process.on("uncaughtException", (error: Error) => {
    if (isReadEio(error)) {
      // Node 26 can surface TTY shutdown during Ctrl-C as an unhandled
      // readline Interface "read EIO" error. The dev server is already being
      // interrupted, so treat this like a clean terminal close instead of
      // dumping a Node.js stack trace.
      process.exit(0);
    }
    throw error;
  });
}

function isReadEio(error: Error): boolean {
  const err = error as NodeJS.ErrnoException;
  return err.code === "EIO" && err.syscall === "read";
}
