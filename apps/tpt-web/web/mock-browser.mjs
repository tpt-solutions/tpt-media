// Logic test of tpt-web.js against FAKE browser objects, run under Node.
//
//   node apps/tpt-web/web/build.mjs && node apps/tpt-web/web/mock-browser.mjs
//
// This is NOT a browser test. It replaces AudioContext, canvas, ImageData,
// requestAnimationFrame and the wall clock with minimal fakes so the control
// flow of tpt-web.js (pushing, back-pressure, audio scheduling, underrun
// handling, picture pacing, end-of-stream, stop) actually executes against
// the real wasm decoder. What it cannot tell you is whether real WebAudio
// makes sound or a real canvas shows the picture.

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const SPEED = 8; // fake media clock runs 8x faster than the wall clock
const t00 = performance.now();
const fakeNow = () => ((performance.now() - t00) / 1000) * SPEED;

const scheduled = []; // {when, duration, channels, rate, peak}
const drawn = []; // {w, h}
let running = true;

class FakeAudioContext {
  constructor() { this.destination = {}; this.base = fakeNow(); this.frozenAt = null; }
  get currentTime() { return running ? fakeNow() : this.frozenAt; }
  resume() { running = true; return Promise.resolve(); }
  suspend() { this.frozenAt = fakeNow(); running = false; return Promise.resolve(); }
  close() { return Promise.resolve(); }
  createBuffer(channels, length, sampleRate) {
    const data = Array.from({ length: channels }, () => new Float32Array(length));
    return { channels, length, sampleRate, data, copyToChannel(src, c) { data[c].set(src); } };
  }
  createBufferSource() {
    return {
      buffer: null,
      connect() {},
      start(when) {
        const b = this.buffer;
        let peak = 0;
        for (const ch of b.data) for (const s of ch) peak = Math.max(peak, Math.abs(s));
        scheduled.push({ when, duration: b.length / b.sampleRate, channels: b.channels, rate: b.sampleRate, peak });
      },
    };
  }
}

globalThis.window = globalThis;
globalThis.AudioContext = FakeAudioContext;
globalThis.ImageData = class { constructor(data, w, h) { this.data = data; this.width = w; this.height = h; } };
let rafCb = null;
globalThis.requestAnimationFrame = (cb) => { rafCb = cb; return 1; };
globalThis.cancelAnimationFrame = () => { rafCb = null; };
const rafPump = setInterval(() => { const cb = rafCb; rafCb = null; cb?.(); }, 4);

const { TptPlayer } = await import("./tpt-web.js");
const wasm = new Uint8Array(await readFile(join(here, "tpt_web.wasm")));
const demo = new Uint8Array(await readFile(join(here, "demo.tpt")));

const canvas = {
  width: 0, height: 0,
  getContext: () => ({
    putImageData(img) {
      if (img.data.length !== img.width * img.height * 4) throw new Error("bad ImageData");
      drawn.push({ w: img.width, h: img.height, t: fakeNow() });
    },
  }),
};

let failures = 0;
const ok = (c, m) => { console.log(`${c ? "ok  " : "FAIL"} ${m}`); if (!c) failures++; };

async function playToEnd(source, limits) {
  scheduled.length = 0; drawn.length = 0;
  let info = null, stats = null, error = null;
  const ended = new Promise((resolve) => {
    const p = new TptPlayer({
      canvas, wasmUrl: wasm, limits,
      onInfo: (i) => (info = i),
      onStats: (s) => (stats = s),
      onEnded: () => resolve(p),
      onError: (e) => { error = e; resolve(p); },
    });
    p.play(source);
  });
  const p = await Promise.race([ended, new Promise((_, rej) => setTimeout(() => rej(new Error("timeout")), 20000))]);
  return { p, info, stats, error };
}

{
  const { info, stats, error } = await playToEnd(demo);
  ok(!error, `no error${error ? ": " + error.message : ""}`);
  ok(info?.audio?.sampleRate === 48000 && info?.video?.width === 160, `stream info announced: ${JSON.stringify(info)}`);
  const total = scheduled.reduce((a, s) => a + s.duration, 0);
  ok(Math.abs(total - 4.0) < 1e-6, `scheduled audio totals ${total.toFixed(4)} s (expect 4.0000)`);
  let contiguous = true;
  for (let i = 1; i < scheduled.length; i++)
    if (Math.abs(scheduled[i].when - (scheduled[i - 1].when + scheduled[i - 1].duration)) > 1e-6) contiguous = false;
  ok(contiguous, `${scheduled.length} audio buffers scheduled back to back`);
  ok(scheduled.every((s) => s.channels === 1 && s.rate === 48000 && s.peak > 0), "audio buffers non-silent, mono, 48 kHz");
  ok(drawn.length > 0 && drawn.every((d) => d.w === 160 && d.h === 96), `${drawn.length} pictures drawn at 160x96`);
  ok(canvas.width === 160 && canvas.height === 96, "canvas sized to the video");
  ok(stats && stats.shownFrames + stats.droppedFrames <= 60, `shown ${stats?.shownFrames} + dropped ${stats?.droppedFrames} <= 60 frames`);
  ok(stats && stats.decodeErrors === 0, "no decode errors");
}

{
  // A tiny video queue must not deadlock playback: back-pressure only.
  const { error } = await playToEnd(demo, { maxVideoQueueFrames: 1, maxAudioQueueMs: 100 });
  ok(!error && scheduled.reduce((a, s) => a + s.duration, 0) > 3.99, "playback completes with minimal queue limits");
}

{
  const { error } = await playToEnd(new TextEncoder().encode("not a tpt file at all, sorry"));
  ok(error && /container/i.test(error.message), `garbage input reported via onError: ${error?.message}`);
}

{
  // stop() mid-playback releases everything and silences callbacks.
  scheduled.length = 0;
  let endedCalled = false;
  const p = new TptPlayer({ canvas, wasmUrl: wasm, onEnded: () => (endedCalled = true) });
  await p.play(demo);
  await new Promise((r) => setTimeout(r, 40));
  p.stop();
  ok(p.decoder === null && p.ctx === null, "stop() released decoder and context");
  await new Promise((r) => setTimeout(r, 100));
  ok(!endedCalled, "no onEnded after stop()");
}

clearInterval(rafPump);
console.log(failures ? `\n${failures} check(s) FAILED` : "\nall checks passed");
process.exit(failures ? 1 : 0);
