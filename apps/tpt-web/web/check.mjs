// Smoke test of the built wasm module, run under Node (no browser needed).
//
//   node apps/tpt-web/web/build.mjs && node apps/tpt-web/web/check.mjs
//
// This exercises the *WebAssembly build* and the JavaScript glue
// (tpt-decoder.js) end to end: the same code path a browser uses, minus
// WebAudio and canvas. It does NOT test browser audio/video output; nothing
// in this repository has been run in a real browser.

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Status, TptError, TptModule } from "./tpt-decoder.js";

const here = dirname(fileURLToPath(import.meta.url));
const wasm = await readFile(join(here, "tpt_web.wasm"));
const demo = new Uint8Array(await readFile(join(here, "demo.tpt")));

let failures = 0;
function ok(cond, what) {
  console.log(`${cond ? "ok  " : "FAIL"} ${what}`);
  if (!cond) failures++;
}

// Parse the module's memory section to report the declared maximum.
function memoryLimits(bytes) {
  const u8 = new Uint8Array(bytes);
  let p = 8;
  const leb = () => {
    let r = 0, s = 0, b;
    do { b = u8[p++]; r += (b & 0x7f) * 2 ** s; s += 7; } while (b & 0x80);
    return r;
  };
  while (p < u8.length) {
    const id = u8[p++];
    const size = leb();
    if (id === 5) {
      leb(); // count
      const flags = u8[p++];
      const min = leb();
      const max = flags & 1 ? leb() : null;
      return { minPages: min, maxPages: max };
    }
    p += size;
  }
  return null;
}

const mod = await TptModule.load(wasm);
const lim = memoryLimits(wasm);
console.log(
  `wasm: ${wasm.length} bytes, initial memory ${(lim.minPages * 64) / 1024} MiB, ` +
    `declared max ${lim.maxPages === null ? "none" : (lim.maxPages * 64) / 1024 + " MiB"}`
);

// ---- 1. play demo.tpt in 64 KiB pushes with back-pressure ---------------
{
  const d = mod.open();
  let samples = 0, frames = 0, lastAudioEnd = 0, lastVideoPts = -1;
  let monotonic = true, finite = true, peakMem = 0, sawFull = false;
  const drain = () => {
    for (let a; (a = d.nextAudio()); ) {
      samples += a.frames;
      if (Math.abs(a.pts - lastAudioEnd) > 1e-6) monotonic = false;
      lastAudioEnd = a.pts + a.frames / a.sampleRate;
      for (const s of a.planar) if (!Number.isFinite(s) || Math.abs(s) > 1) finite = false;
    }
    for (let v; (v = d.nextVideo()); ) {
      if (v.pts <= lastVideoPts) monotonic = false;
      lastVideoPts = v.pts;
      if (v.rgba.length !== v.width * v.height * 4) finite = false;
      frames++;
    }
    peakMem = Math.max(peakMem, d.info("memoryInUse"));
  };
  for (let off = 0; off < demo.length; off += 65536) {
    let chunk = demo.subarray(off, off + 65536);
    while (chunk.length) {
      const n = d.push(chunk);
      chunk = chunk.subarray(n);
      if (chunk.length) { sawFull = true; d.pump(); drain(); }
    }
    d.pump();
    drain();
  }
  d.finish();
  let st;
  do { st = d.pump(); drain(); } while (st !== Status.ENDED);
  const info = d.streamInfo;
  ok(samples === 192000, `demo.tpt decodes exactly 4.000 s of audio (${samples} samples)`);
  ok(frames === 60, `demo.tpt decodes 60 video frames (${frames})`);
  ok(info.audio?.sampleRate === 48000 && info.video?.width === 160 && info.video?.height === 96,
     `stream info: ${JSON.stringify(info)}`);
  ok(monotonic, "audio contiguous, video timestamps strictly increasing");
  ok(finite, "all samples finite and within [-1, 1]; all pictures the right size");
  ok(d.info("decodeErrors") === 0, "no decode errors");
  ok(peakMem <= d.info("memoryCeiling"), `peak player memory ${peakMem} B <= ceiling ${d.info("memoryCeiling")} B`);
  console.log(`     wasm linear memory after playback: ${(mod.memoryBytes / 1048576).toFixed(1)} MiB, back-pressure exercised: ${sawFull}`);
  d.close();
}

// ---- 2. hostile input: errors are TptError, never a wasm trap ----------
{
  let handled = 0, trapped = 0, threw = 0;
  let seed = 0x9e3779b9;
  const rnd = () => ((seed ^= seed << 13), (seed ^= seed >>> 17), (seed ^= seed << 5), seed >>> 0);
  for (let round = 0; round < 300; round++) {
    const data = demo.slice(0, round % 4 === 3 ? rnd() % demo.length : demo.length);
    if (round % 4 === 2) for (let i = 20; i < data.length; i++) data[i] = rnd() & 255;
    else for (let k = 0; k < 1 + (rnd() % 10); k++) data[rnd() % data.length] = rnd() & 255;
    const d = mod.open();
    try {
      let chunk = data;
      while (chunk.length) {
        const n = d.push(chunk);
        chunk = chunk.subarray(n);
        if (chunk.length) { d.pump(); while (d.nextAudio()); while (d.nextVideo()); }
      }
      d.finish();
      for (let i = 0; i < 100000 && d.pump() !== Status.ENDED; i++) {
        while (d.nextAudio()); while (d.nextVideo());
      }
      handled++;
    } catch (e) {
      if (e instanceof TptError) threw++;
      else { trapped++; console.log("   unexpected:", e); }
    } finally {
      try { d.close(); } catch { /* instance may be dead after a trap */ }
    }
    if (trapped) break;
  }
  ok(trapped === 0, `300 mutated/truncated/noise files: ${handled} played out, ${threw} reported a TptError, ${trapped} wasm traps`);
}

// ---- 3. limits and handles ---------------------------------------------
{
  const d = mod.open({ maxInputBytes: 1, maxVideoQueueFrames: 2 });
  ok(d.info("memoryCeiling") > 0, "open() with tiny/odd limits is clamped, not rejected");
  d.close();
  let threw = false;
  try { d.pump(); } catch (e) { threw = e instanceof TptError && e.code === -1; }
  ok(threw, "a closed handle throws TptError code -1");
  const many = [];
  let refused = false;
  try { for (let i = 0; i < 10; i++) many.push(mod.open()); } catch (e) { refused = e instanceof TptError; }
  ok(refused && many.length === 4, `player count capped at 4 (opened ${many.length})`);
  many.forEach((x) => x.close());
}

console.log(failures ? `\n${failures} check(s) FAILED` : "\nall checks passed");
process.exit(failures ? 1 : 0);
