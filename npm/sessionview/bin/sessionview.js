#!/usr/bin/env node
"use strict";

/**
 * Launcher for the SessionView headless server binary (`npx sessionview`).
 *
 * Resolution order:
 *   1. Newest released version (npm registry check, fail-soft when offline):
 *      if newer than this package, its binary is downloaded once into the
 *      per-user cache — `npx sessionview` therefore tracks releases even
 *      from a stale install.
 *   2. The platform package installed alongside this one (optional deps).
 *   3. Per-user cache, then sha256-verified download from GitHub releases.
 *
 * All CLI arguments pass through to the binary (default port: 9921).
 */

const { spawn } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { version } = require("../package.json");

const REPO = "tyql688/sessionview";
const SUPPORTED = new Set(["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "win32-x64"]);

function platformKey() {
  const key = `${process.platform}-${process.arch}`;
  if (!SUPPORTED.has(key)) {
    console.error(`sessionview: unsupported platform ${key}`);
    console.error(`supported: ${[...SUPPORTED].join(", ")}`);
    process.exit(1);
  }
  return key;
}

function binaryName(key) {
  return key.startsWith("win32") ? "sessionview-headless.exe" : "sessionview-headless";
}

function fromPlatformPackage(key) {
  try {
    return require.resolve(`sessionview-${key}/bin/${binaryName(key)}`);
  } catch {
    return null;
  }
}

function cacheDir(forVersion) {
  if (process.platform === "win32" && process.env.LOCALAPPDATA) {
    return path.join(process.env.LOCALAPPDATA, "sessionview", "bin", forVersion);
  }
  const base = process.env.XDG_CACHE_HOME || path.join(os.homedir(), ".cache");
  return path.join(base, "sessionview", "bin", forVersion);
}

function newerThan(a, b) {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  if (pa.some(Number.isNaN) || pb.some(Number.isNaN)) return false;
  for (let i = 0; i < 3; i++) {
    if ((pa[i] ?? 0) !== (pb[i] ?? 0)) return (pa[i] ?? 0) > (pb[i] ?? 0);
  }
  return false;
}

/** Latest published version, or null when offline/unreachable — availability
 * beats freshness here: the locally installed version must always launch. */
async function latestVersion() {
  try {
    const response = await fetch("https://registry.npmjs.org/sessionview/latest", {
      signal: AbortSignal.timeout(3000),
    });
    if (!response.ok) return null;
    const data = await response.json();
    return typeof data.version === "string" ? data.version : null;
  } catch {
    return null;
  }
}

async function fetchBuffer(url) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) {
    throw new Error(`download failed (${response.status} ${response.statusText}): ${url}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

async function downloadBinary(key, forVersion) {
  const asset = `sessionview-headless-${key}${key.startsWith("win32") ? ".exe" : ""}`;
  const base = `https://github.com/${REPO}/releases/download/v${forVersion}`;

  console.error(`sessionview: downloading v${forVersion} (${key})…`);
  const [binary, checksumFile] = await Promise.all([
    fetchBuffer(`${base}/${asset}`),
    fetchBuffer(`${base}/${asset}.sha256`),
  ]);

  const expected = checksumFile.toString("utf8").trim().split(/\s+/)[0];
  const actual = crypto.createHash("sha256").update(binary).digest("hex");
  if (!expected || expected !== actual) {
    throw new Error(`checksum mismatch for ${asset}: expected ${expected}, got ${actual}`);
  }

  const dir = cacheDir(forVersion);
  fs.mkdirSync(dir, { recursive: true });
  const target = path.join(dir, binaryName(key));
  // Temp name + rename so a concurrent launcher never sees a half-written
  // executable.
  const temp = `${target}.${process.pid}.tmp`;
  fs.writeFileSync(temp, binary, { mode: 0o755 });
  fs.renameSync(temp, target);
  return target;
}

async function resolveBinary() {
  const key = platformKey();

  const latest = await latestVersion();
  const desired = latest && newerThan(latest, version) ? latest : version;

  if (desired === version) {
    const installed = fromPlatformPackage(key);
    if (installed) return installed;
  } else {
    console.error(`sessionview: update available (v${version} → v${desired})`);
  }

  const cached = path.join(cacheDir(desired), binaryName(key));
  if (fs.existsSync(cached)) return cached;

  try {
    return await downloadBinary(key, desired);
  } catch (error) {
    // A failed update download must not break launching: fall back to the
    // installed version when one exists.
    const installed = fromPlatformPackage(key);
    if (desired !== version && installed) {
      console.error(`sessionview: update failed (${error.message}); running v${version}`);
      return installed;
    }
    throw error;
  }
}

async function main() {
  const binary = await resolveBinary();
  const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });
  child.on("error", (error) => {
    console.error(`sessionview: failed to start ${binary}: ${error.message}`);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 1);
  });
  // Forward Ctrl-C etc. to the server for a graceful shutdown.
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => child.kill(signal));
  }
}

main().catch((error) => {
  console.error(`sessionview: ${error.message}`);
  process.exit(1);
});
