#!/usr/bin/env node
// Runs the tokendraw binary from the platform package npm installed as an
// optional dependency.
"use strict";

const { spawnSync } = require("node:child_process");
const platforms = require("./platforms.json");

const platform = Object.values(platforms).find(
  (p) => p.os === process.platform && p.cpu.includes(process.arch),
);
if (!platform) {
  const supported = Object.values(platforms).flatMap((p) =>
    p.cpu.map((cpu) => `${p.os}-${cpu}`),
  );
  console.error(
    `tokendraw: no binary for ${process.platform}-${process.arch}; the npm package has binaries for ${supported.join(", ")}`,
  );
  process.exit(1);
}

const executable = platform.os === "win32" ? "tokendraw.exe" : "tokendraw";
let binary;
try {
  binary = require.resolve(`${platform.package}/bin/${executable}`);
} catch {
  console.error(
    `tokendraw: the package ${platform.package} is not installed; install tokendraw again without --omit=optional`,
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`tokendraw: ${binary}: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status === null ? 1 : result.status);
