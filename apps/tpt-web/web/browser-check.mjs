// Real-browser check for tpt-web (todo.md section 45).
//
//   node apps/tpt-web/web/build.mjs && node apps/tpt-web/web/browser-check.mjs
//
// This is the counterpart to `mock-browser.mjs`. That one replaces
// AudioContext, canvas and rAF with fakes, so it can test the player's control
// flow in Node but cannot say whether real WebAudio or a real canvas work.
// This one runs the *unmodified* player in a real headless Chrome: a genuine
// AudioContext whose clock advances against the machine's audio device, a
// genuine 2D canvas whose pixels are read back with getImageData, genuine
// requestAnimationFrame, and the real wasm module. The only thing it cannot
// do is prove a human heard it.
//
// Zero dependencies, like the rest of `web/`. A browser is required; if none
// is found the script says so and exits non-zero rather than skipping, so a
// missing browser can never read as a pass.
//
// What it does NOT do: verify the *content* of the picture beyond "it is a
// real, non-uniform, opaque rasterisation", and it cannot verify audibility.

import { spawn } from "node:child_process";
import { access } from "node:fs/promises";
import { constants } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { serve } from "./serve.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const TIMEOUT_MS = 120_000;

// Common install locations, tried in order. Chrome first: it is the engine
// the player is developed against.
const BROWSERS = [
  process.env.TPT_BROWSER,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean);

const readable = (p) => access(p, constants.R_OK).then(() => true, () => false);

const browser = (await Promise.all(BROWSERS.map(async (p) => [p, await readable(p)])))
  .find(([, ok]) => ok)?.[0];

if (!browser) {
  console.error("No Chrome/Edge found. Set TPT_BROWSER to a browser binary.");
  process.exit(1);
}

if (!(await readable(join(here, "tpt_web.wasm")))) {
  console.error("tpt_web.wasm is missing - run `node apps/tpt-web/web/build.mjs` first.");
  process.exit(1);
}

const { port, close, done } = await serve(here);

// `--autoplay-policy` lets the AudioContext start without a click; without it
// Chrome suspends the context and the player's master clock never advances.
// No virtual-time budget: it fast-forwards renderer timers past the audio
// thread and the clock reads as frozen.
const child = spawn(browser, [
  "--headless=new",
  "--disable-gpu",
  "--no-sandbox",
  "--autoplay-policy=no-user-gesture-required",
  `http://127.0.0.1:${port}/browser-check.html`,
], { stdio: "ignore" });

const shutdown = () => { try { child.kill(); } catch { /* already gone */ } close(); };
process.on("exit", shutdown);

let report;
try {
  report = await Promise.race([
    done,
    new Promise((_, rej) => setTimeout(() => rej(new Error("timed out")), TIMEOUT_MS)),
  ]);
} catch (e) {
  console.error(`browser check did not report in: ${e.message}`);
  shutdown();
  process.exit(1);
}
shutdown();

console.log(`browser: ${browser}\n`);
console.log(report);
process.exit(/^FAIL/m.test(report) ? 1 : 0);
