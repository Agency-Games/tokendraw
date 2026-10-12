#!/usr/bin/env node
// Installs the npm packages the way a user does and runs tokendraw: builds
// them with BINARY for this machine's platform and a placeholder for the
// others, serves them from a registry on localhost, installs only
// tokendraw globally into a new prefix, checks that npm chose this
// platform's package and blocked no install script, then runs tokendraw
// from the prefix made read-only.
//
//   node npm/test-install.js BINARY
"use strict";

const assert = require("node:assert/strict");
const { spawn, spawnSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { build } = require("./build.js");
const platforms = require("./platforms.json");

const windows = process.platform === "win32";

function run(command, args, options) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { shell: windows, ...options });
    let stdout = "";
    let output = "";
    child.stdout.on("data", (data) => {
      stdout += data;
      output += data;
    });
    child.stderr.on("data", (data) => (output += data));
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout, output }));
  });
}

function serve(packages) {
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://localhost");
    const name = decodeURIComponent(url.pathname.slice(1));
    const pkg = packages[name];
    if (pkg) {
      response.setHeader("content-type", "application/json");
      const version = { ...pkg.manifest, dist: pkg.dist(server) };
      response.end(
        JSON.stringify({
          name,
          "dist-tags": { latest: pkg.manifest.version },
          versions: { [pkg.manifest.version]: version },
        }),
      );
      return;
    }
    const tarball = Object.values(packages).find(
      (p) => `/-/${path.basename(p.tarball)}` === url.pathname,
    );
    if (tarball) {
      response.end(fs.readFileSync(tarball.tarball));
      return;
    }
    response.statusCode = 404;
    response.end();
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => resolve(server)),
  );
}

function setWritable(dir, writable) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const entryPath = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      setWritable(entryPath, writable);
    }
    if (!entry.isSymbolicLink()) {
      const mode = fs.statSync(entryPath).mode;
      fs.chmodSync(entryPath, writable ? mode | 0o200 : mode & ~0o222);
    }
  }
  const mode = fs.statSync(dir).mode;
  fs.chmodSync(dir, writable ? mode | 0o200 : mode & ~0o222);
}

async function main() {
  const binary = process.argv[2];
  if (!binary) {
    console.error("usage: node npm/test-install.js BINARY");
    process.exit(2);
  }
  const version = spawnSync(binary, ["--version"], { encoding: "utf8" })
    .stdout.trim()
    .replace(/^tokendraw /, "");
  const host = Object.entries(platforms).find(
    ([, p]) => p.os === process.platform && p.cpu.includes(process.arch),
  );
  assert.ok(host, `no package for ${process.platform}-${process.arch}`);

  const work = fs.mkdtempSync(path.join(os.tmpdir(), "tokendraw-npm-"));
  const placeholder = path.join(work, "placeholder");
  fs.writeFileSync(placeholder, "");
  const binaries = {};
  for (const target of Object.keys(platforms)) {
    binaries[target] = target === host[0] ? binary : placeholder;
  }
  const out = path.join(work, "packages");
  build(version, out, binaries);

  const npmrc = path.join(work, "npmrc");
  fs.writeFileSync(npmrc, "");
  const npmEnv = { ...process.env, npm_config_userconfig: npmrc };
  const packages = {};
  for (const dir of fs.readdirSync(out)) {
    const manifest = JSON.parse(
      fs.readFileSync(path.join(out, dir, "package.json"), "utf8"),
    );
    const packed = await run(
      "npm",
      ["pack", "--pack-destination", work, path.join(out, dir)],
      { env: npmEnv },
    );
    assert.equal(packed.status, 0, packed.output);
    const tarball = path.join(work, `${manifest.name}-${manifest.version}.tgz`);
    const bytes = fs.readFileSync(tarball);
    packages[manifest.name] = {
      manifest,
      tarball,
      dist: (server) => ({
        tarball: `http://127.0.0.1:${server.address().port}/-/${path.basename(tarball)}`,
        integrity: `sha512-${crypto.createHash("sha512").update(bytes).digest("base64")}`,
        shasum: crypto.createHash("sha1").update(bytes).digest("hex"),
      }),
    };
  }

  const server = await serve(packages);
  const prefix = path.join(work, "prefix");
  try {
    const install = await run(
      "npm",
      [
        "install",
        "--global",
        "--prefix",
        prefix,
        "--registry",
        `http://127.0.0.1:${server.address().port}/`,
        "--cache",
        path.join(work, "cache"),
        "--no-audit",
        "--no-fund",
        "tokendraw",
      ],
      { env: npmEnv },
    );
    console.log(install.output);
    assert.equal(install.status, 0, "npm install failed");
    assert.ok(
      !/install-scripts|install scripts/.test(install.output),
      "npm reported install scripts",
    );
  } finally {
    server.close();
  }

  const modules = path.join(
    prefix,
    windows ? "node_modules" : path.join("lib", "node_modules"),
    "tokendraw",
    "node_modules",
  );
  const installed = fs.existsSync(modules)
    ? fs.readdirSync(modules).filter((name) => name.startsWith("tokendraw-"))
    : [];
  assert.deepEqual(installed, [host[1].package]);

  const tokendraw = windows
    ? path.join(prefix, "tokendraw.cmd")
    : path.join(prefix, "bin", "tokendraw");
  if (!windows) {
    setWritable(prefix, false);
  }
  try {
    const versionRun = await run(tokendraw, ["--version"]);
    assert.equal(versionRun.status, 0, versionRun.output);
    assert.equal(versionRun.stdout.trim(), `tokendraw ${version}`);

    const direct = spawnSync(binary, ["no-such-command"]);
    const launched = await run(tokendraw, ["no-such-command"]);
    assert.notEqual(direct.status, 0);
    assert.equal(launched.status, direct.status, launched.output);
  } finally {
    if (!windows) {
      setWritable(prefix, true);
    }
  }
  fs.rmSync(work, { recursive: true, force: true });
  console.log(`npm installed ${host[1].package} and tokendraw ran from it`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
