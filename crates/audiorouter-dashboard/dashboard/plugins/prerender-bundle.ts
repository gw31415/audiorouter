import { cpSync, existsSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";
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
 * are left relative so Vite resolves them against the configured project root.
 * Cleanup discovers `root`/`outDir`/`assetsDir` from Vite's resolved config
 * instead of reading audiorouter-specific environment variables.
 */
export function prerenderBundle(options: PrerenderBundleOptions): Plugin[] {
  const assetPrefix = options.assetPrefix ?? "prerender-";
  const forceExit = options.forceExit ?? true;

  let root = process.cwd();
  let outDir = resolve(root, "dist");
  let assetsDirName = "assets";
  let publicDir = resolve(root, "public");

  return [
    ...(vitePrerenderPlugin({
      renderTarget: options.renderTarget ?? "#root",
      prerenderScript: options.prerenderScript,
    }) as Plugin[]),
    {
      name: "cleanup-prerender-chunks",
      apply: "build",
      enforce: "post",
      configResolved(config) {
        root = config.root;
      },
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

        const indexHtml = bundle["index.html"];
        if (indexHtml?.type === "asset" && typeof indexHtml.source === "string") {
          indexHtml.source = restoreStaticHeadTags(root, indexHtml.source);
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
        publicDir = resolveFromRoot(config.root, config.publicDir);
      },
      writeBundle() {
        postprocessPrerenderOutput({
          root,
          outDir,
          assetsDirName,
          assetPrefix,
          publicDir,
        });
      },
      closeBundle() {
        const postprocess = () =>
          postprocessPrerenderOutput({
            root,
            outDir,
            assetsDirName,
            assetPrefix,
            publicDir,
          });

        postprocess();

        // @xyflow/react leaves timers/observers in the event loop after
        // prerendering, preventing the process from exiting in this dashboard.
        if (forceExit) {
          process.once("exit", postprocess);
          process.exit(0);
        }
      },
    },
  ];
}

function resolveFromRoot(root: string, path: string): string {
  return isAbsolute(path) ? path : resolve(root, path);
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function postprocessPrerenderOutput({
  root,
  outDir,
  assetsDirName,
  assetPrefix,
  publicDir,
}: {
  root: string;
  outDir: string;
  assetsDirName: string;
  assetPrefix: string;
  publicDir: string;
}): void {
  const assetsDir = resolve(outDir, assetsDirName);
  const indexPath = resolve(outDir, "index.html");

  try {
    for (const fileName of readdirSync(assetsDir)) {
      // CSS emitted only for the prerender entry is never loaded by the
      // client. JS chunks may be shared with the client bundle, so keep them;
      // deleting those chunks causes /assets/prerender-*.js 404s.
      if (fileName.startsWith(assetPrefix) && !fileName.endsWith(".js")) {
        rmSync(resolve(assetsDir, fileName));
      }
    }
    const escapedPrefix = escapeRegExp(assetPrefix);
    const prerenderLink = new RegExp(`<link[^>]*href="[^"]*${escapedPrefix}[^"]*"[^>]*>\\s*`, "g");
    const html = readFileSync(indexPath, "utf-8").replace(prerenderLink, "");
    writeFileSync(indexPath, restoreStaticHeadTags(root, html));
  } catch {
    // Cleanup is best-effort; the build artifact remains usable even if a
    // prerender-only file is already absent or the output layout changed.
  }

  // vite-prerender-plugin can leave @xyflow/react timers alive; this plugin
  // intentionally force-exits. Copy public assets before exiting so favicon
  // and touch icon files are present in dist even if the build pipeline would
  // otherwise be cut short.
  if (existsSync(publicDir)) {
    cpSync(publicDir, outDir, { recursive: true });
  }
}

function restoreStaticHeadTags(root: string, builtHtml: string): string {
  const sourceIndexPath = resolve(root, "index.html");
  if (!existsSync(sourceIndexPath)) {
    return builtHtml;
  }

  const sourceHtml = readFileSync(sourceIndexPath, "utf-8");
  const sourceHead = sourceHtml.match(/<head>([\s\S]*?)<\/head>/)?.[1] ?? "";
  const tags = sourceHead
    .split("\n")
    .map((line) => line.trim())
    .filter(
      (line) =>
        /<meta\s+name="theme-color"(?:\s|>)/.test(line) ||
        /<link\s+[^>]*rel="(?:alternate icon|apple-touch-icon|icon)"(?:\s|>)/.test(line),
    );

  let html = builtHtml;
  for (const tag of tags) {
    const href = tag.match(/href="([^"]+)"/)?.[1];
    const name = tag.match(/name="([^"]+)"/)?.[1];
    const rel = tag.match(/rel="([^"]+)"/)?.[1];
    const alreadyPresent = href
      ? html.includes(`href="${href}"`)
      : name
        ? html.includes(`name="${name}"`)
        : rel
          ? html.includes(`rel="${rel}"`)
          : html.includes(tag);
    if (!alreadyPresent) {
      html = html.replace(/\n\s*<title>/, `\n    ${tag}\n    <title>`);
    }
  }

  return html;
}
