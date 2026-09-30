// Build the wasm module and put it next to index.html.
//
//   node apps/tpt-web/web/build.mjs
//
// Needs the wasm32 target once:  rustup target add wasm32-unknown-unknown
// Optional first argument: a wasm memory cap in MiB (default 512), passed to
// the linker as --max-memory so an instance can never grow past it whatever
// the input does. Growth beyond the cap fails inside the module instead of
// taking the tab down.

import { spawnSync } from "node:child_process";
import { copyFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..", "..", "..");
const capMiB = Number(process.argv[2] ?? 512);
if (!Number.isInteger(capMiB) || capMiB < 16 || capMiB > 4096) {
  console.error("memory cap must be an integer number of MiB in 16..=4096");
  process.exit(2);
}
const flags = `-C link-arg=--max-memory=${capMiB * 1024 * 1024}`;
const cargo = spawnSync(
  "cargo",
  ["build", "-p", "tpt-web", "--target", "wasm32-unknown-unknown", "--release"],
  {
    cwd: root,
    stdio: "inherit",
    // Appended to any RUSTFLAGS the caller already set.
    env: { ...process.env, RUSTFLAGS: `${process.env.RUSTFLAGS ?? ""} ${flags}`.trim() },
    shell: process.platform === "win32",
  }
);
if (cargo.status !== 0) process.exit(cargo.status ?? 1);
const from = join(root, "target", "wasm32-unknown-unknown", "release", "tpt_web.wasm");
const to = join(here, "tpt_web.wasm");
copyFileSync(from, to);
console.log(`wrote ${to} (${statSync(to).size} bytes, memory cap ${capMiB} MiB)`);
