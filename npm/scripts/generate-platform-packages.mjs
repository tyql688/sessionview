#!/usr/bin/env node
/**
 * Assemble publishable npm packages for the headless server.
 *
 * Usage: node npm/scripts/generate-platform-packages.mjs <version> <binaries-dir> <out-dir>
 *
 * `binaries-dir` must contain release binaries named
 * `sessionview-headless-<os>-<arch>[.exe]`. For every binary present this
 * emits a platform package (`sessionview-<os>-<arch>`), then emits
 * the main `sessionview` package with its version and
 * optionalDependencies pinned to <version>. Missing platforms are dropped
 * from optionalDependencies (npm skips non-matching ones anyway; absent
 * packages must not be referenced at all).
 */

import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const [version, binariesDir, outDir] = process.argv.slice(2);
if (!version || !binariesDir || !outDir) {
  console.error("usage: generate-platform-packages.mjs <version> <binaries-dir> <out-dir>");
  process.exit(1);
}

const PLATFORMS = [
  { key: "darwin-arm64", os: "darwin", cpu: "arm64" },
  { key: "darwin-x64", os: "darwin", cpu: "x64" },
  { key: "linux-x64", os: "linux", cpu: "x64" },
  { key: "linux-arm64", os: "linux", cpu: "arm64" },
  { key: "win32-x64", os: "win32", cpu: "x64" },
];

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const templateDir = path.resolve(scriptDir, "..", "sessionview");

const present = [];
for (const platform of PLATFORMS) {
  const ext = platform.os === "win32" ? ".exe" : "";
  const source = path.join(binariesDir, `sessionview-headless-${platform.key}${ext}`);
  if (!existsSync(source)) {
    console.warn(`skipping ${platform.key}: ${source} not found`);
    continue;
  }
  present.push(platform);

  const packageDir = path.join(outDir, `sessionview-${platform.key}`);
  const binDir = path.join(packageDir, "bin");
  mkdirSync(binDir, { recursive: true });

  const binaryName = `sessionview-headless${ext}`;
  cpSync(source, path.join(binDir, binaryName));
  if (!ext) chmodSync(path.join(binDir, binaryName), 0o755);

  writeFileSync(
    path.join(packageDir, "package.json"),
    `${JSON.stringify(
      {
        name: `sessionview-${platform.key}`,
        version,
        description: `SessionView headless server binary (${platform.key})`,
        license: "MIT",
        repository: { type: "git", url: "git+https://github.com/tyql688/sessionview.git" },
        os: [platform.os],
        cpu: [platform.cpu],
        files: ["bin/"],
      },
      null,
      2,
    )}\n`,
  );
  console.log(`generated sessionview-${platform.key}`);
}

if (present.length === 0) {
  console.error("no platform binaries found — refusing to generate an empty release");
  process.exit(1);
}

const mainDir = path.join(outDir, "sessionview");
mkdirSync(mainDir, { recursive: true });
cpSync(path.join(templateDir, "bin"), path.join(mainDir, "bin"), { recursive: true });
cpSync(path.join(templateDir, "README.md"), path.join(mainDir, "README.md"));

const manifest = JSON.parse(readFileSync(path.join(templateDir, "package.json"), "utf8"));
manifest.version = version;
manifest.optionalDependencies = Object.fromEntries(
  present.map((platform) => [`sessionview-${platform.key}`, version]),
);
writeFileSync(path.join(mainDir, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`generated sessionview@${version} (${present.length} platform packages)`);
