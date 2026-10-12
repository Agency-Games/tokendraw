#!/usr/bin/env node
// Writes the npm packages into OUT: tokendraw, which depends optionally on
// every platform package in platforms.json, and the platform package of
// each TARGET given with its binary.
//
//   node npm/build.js VERSION OUT TARGET=BINARY...
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const platforms = require("./platforms.json");

const root = path.join(__dirname, "..");

const common = (version) => ({
  version,
  license: "MIT AND CC-BY-4.0",
  homepage: "https://github.com/Agency-Games/tokendraw",
  repository: {
    type: "git",
    url: "git+https://github.com/Agency-Games/tokendraw.git",
  },
});

function writePackage(dir, manifest, files) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, "package.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  for (const [name, source, mode] of files) {
    const target = path.join(dir, name);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(source, target);
    fs.chmodSync(target, mode);
  }
}

function build(version, out, binaries) {
  const optionalDependencies = {};
  for (const platform of Object.values(platforms)) {
    optionalDependencies[platform.package] = version;
  }
  writePackage(
    path.join(out, "tokendraw"),
    {
      name: "tokendraw",
      ...common(version),
      description:
        "Estimate the energy usage and emissions of LLM coding agent use",
      keywords: [
        "command-line-utilities",
        "llm",
        "energy",
        "emissions",
        "carbon",
        "cli",
      ],
      bin: { tokendraw: "tokendraw.js" },
      engines: { node: ">=16" },
      optionalDependencies,
    },
    [
      ["tokendraw.js", path.join(__dirname, "tokendraw.js"), 0o755],
      ["platforms.json", path.join(__dirname, "platforms.json"), 0o644],
      ["README.md", path.join(root, "README.md"), 0o644],
      ["LICENSE", path.join(root, "LICENSE"), 0o644],
    ],
  );

  for (const [target, binary] of Object.entries(binaries)) {
    const platform = platforms[target];
    if (!platform) {
      throw new Error(`${target} is not in platforms.json`);
    }
    const executable = platform.os === "win32" ? "tokendraw.exe" : "tokendraw";
    writePackage(
      path.join(out, platform.package),
      {
        name: platform.package,
        ...common(version),
        description: `The tokendraw binary for ${target}, installed by the package tokendraw`,
        os: [platform.os],
        cpu: platform.cpu,
        preferUnplugged: true,
      },
      [
        [`bin/${executable}`, binary, 0o755],
        ["LICENSE", path.join(root, "LICENSE"), 0o644],
      ],
    );
  }
}

if (require.main === module) {
  const [version, out, ...pairs] = process.argv.slice(2);
  if (!version || !out) {
    console.error("usage: node npm/build.js VERSION OUT TARGET=BINARY...");
    process.exit(2);
  }
  const binaries = {};
  for (const pair of pairs) {
    const at = pair.indexOf("=");
    if (at < 1) {
      console.error(`build.js: ${pair} is not TARGET=BINARY`);
      process.exit(2);
    }
    binaries[pair.slice(0, at)] = pair.slice(at + 1);
  }
  build(version, out, binaries);
}

module.exports = { build };
