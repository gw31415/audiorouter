import { readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Plugin } from "vite";
import { vitePrerenderPlugin } from "vite-prerender-plugin";

export type PrerenderBundleOptions = {
  prerenderScript: string;
  /** Selector where prerendered HTML is injected. */
  renderTarget?: string;
  /** Built asset filename prefix emitted by vite-prerender-plugin. */
  assetPrefix?: string;
  /** Work around libraries that keep Node's event loop alive after prerendering. */
  forceExit?: boolean;
};

/**
 * Build-time SSG plugin bundle.
 *
 * The prerender entry is a required positional argument. Relative entry paths
 * are resolved from the parent directory of the script that called
 * `prerenderBundle()` (normally vite.config.ts), not from the process cwd.
 * Cleanup discovers `outDir`/`assetsDir` from Vite's resolved config instead of
 * reading audiorouter-specific environment variables.
 */
export function prerenderBundle(options: PrerenderBundleOptions): Plugin[] {
  const root = callerDirectory();
  const assetPrefix = options.assetPrefix ?? "prerender-";
  const resolvedPrerenderScript = resolveFromRoot(root, options.prerenderScript);
  const forceExit = options.forceExit ?? true;

  let outDir = resolve(root, "dist");
  let assetsDirName = "assets";

  return [
    ...(vitePrerenderPlugin({
      renderTarget: options.renderTarget ?? "#root",
      prerenderScript: resolvedPrerenderScript,
    }) as Plugin[]),
    {
      name: "cleanup-prerender-chunks",
      apply: "build",
      enforce: "post",
      generateBundle(_, bundle) {
        const referencedByClient = new Set<string>();
        for (const [fileName, output] of Object.entries(bundle)) {
          if (fileName.startsWith(assetPrefix) || output.type !== "chunk") {
            continue;
          }
          for (const imported of output.imports) {
            referencedByClient.add(imported);
          }
          for (const imported of output.dynamicImports) {
            referencedByClient.add(imported);
          }
        }

        for (const [fileName, output] of Object.entries(bundle)) {
          if (
            fileName.startsWith(assetPrefix) &&
            output.type === "chunk" &&
            !referencedByClient.has(fileName)
          ) {
            delete bundle[fileName];
          }
        }
      },
    },
    {
      name: "cleanup-prerender-files",
      apply: "build",
      enforce: "post",
      configResolved(config) {
        outDir = resolveFromRoot(config.root, config.build.outDir);
        assetsDirName = config.build.assetsDir;
      },
      closeBundle() {
        const assetsDir = resolve(outDir, assetsDirName);
        const indexPath = resolve(outDir, "index.html");

        try {
          for (const fileName of readdirSync(assetsDir)) {
            // CSS emitted only for the prerender entry is never loaded by the
            // client. JS chunks may be shared with the client bundle, so keep
            // them; deleting those chunks causes /assets/prerender-*.js 404s.
            if (fileName.startsWith(assetPrefix) && !fileName.endsWith(".js")) {
              rmSync(resolve(assetsDir, fileName));
            }
          }
          const escapedPrefix = escapeRegExp(assetPrefix);
          const prerenderLink = new RegExp(
            `<link[^>]*href="[^"]*${escapedPrefix}[^"]*"[^>]*>\\s*`,
            "g",
          );
          const html = readFileSync(indexPath, "utf-8").replace(prerenderLink, "");
          writeFileSync(indexPath, html);
        } catch {
          // Cleanup is best-effort; the build artifact remains usable even if a
          // prerender-only file is already absent or the output layout changed.
        }

        // @xyflow/react leaves timers/observers in the event loop after
        // prerendering, preventing the process from exiting in this dashboard.
        if (forceExit) {
          process.exit(0);
        }
      },
    },
  ];
}

function resolveFromRoot(root: string, path: string): string {
  return isAbsolute(path) ? path : resolve(root, path);
}

function callerDirectory(): string {
  const stack = new Error().stack;
  if (!stack) {
    throw new Error("prerenderBundle() could not infer the caller script directory");
  }

  for (const line of stack.split("\n").slice(1)) {
    const filePath = filePathFromStackLine(line);
    if (!filePath || filePath === fileURLToPath(import.meta.url)) {
      continue;
    }
    return dirname(filePath);
  }

  throw new Error("prerenderBundle() could not infer the caller script directory");
}

function filePathFromStackLine(line: string): string | undefined {
  const match = line.match(
    /(?:\(|\s)(file:\/\/[^:)]+|\/?[^():]+\.[cm]?[jt]sx?)(?::\d+)?(?::\d+)?\)?$/,
  );
  if (!match) {
    return undefined;
  }

  const value = match[1];
  return value.startsWith("file://") ? fileURLToPath(value) : value;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
