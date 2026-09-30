// WebAssembly decode-throughput benchmark, run under Node's V8.
//
//   cargo run -p tpt-web --example make_demo -- --bench-dir bench-inputs
//   node apps/tpt-web/web/build.mjs
//   node apps/tpt-web/web/bench.mjs bench-inputs        # a directory of .tpt files
//   node apps/tpt-web/web/bench.mjs demo.tpt            # or a single file
//   node apps/tpt-web/web/bench.mjs                     # defaults to web/demo.tpt
//
// It times the same operation as the native benchmark
// (benches/decode_throughput.rs): push, parse, decode, convert, pop, for a
// whole file, median of RUNS after WARMUP, on the same byte-identical inputs.
// It measures the wasm module and tpt-decoder.js under V8, NOT a browser tab:
// no WebAudio, no canvas, no main-thread contention. Treat the browser as
// this number plus whatever the page spends on presenting the output.

import { readFile, readdir, stat } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Status, TptModule } from "./tpt-decoder.js";

const WARMUP = 2;
const RUNS = 15;
const SLICE = 16 * 1024;

const here = dirname(fileURLToPath(import.meta.url));
const mod = await TptModule.load(await readFile(join(here, "tpt_web.wasm")));

function decodeAll(file) {
  const d = mod.open();
  let samples = 0, frames = 0, rate = 0, fps = 0;
  const drain = () => {
    for (let a; (a = d.nextAudio()); ) { samples += a.frames; rate = a.sampleRate; }
    for (let v; (v = d.nextVideo()); ) frames++;
  };
  for (let off = 0; off < file.length; off += SLICE) {
    let chunk = file.subarray(off, off + SLICE);
    while (chunk.length) {
      const n = d.push(chunk);
      chunk = chunk.subarray(n);
      if (chunk.length) { d.pump(); drain(); }
    }
    d.pump();
    drain();
  }
  d.finish();
  while (d.pump() !== Status.ENDED) drain();
  drain();
  fps = d.streamInfo.video?.fps ?? 0;
  d.close();
  return { samples, frames, rate, fps };
}

const arg = process.argv[2] ?? join(here, "demo.tpt");
const files = (await stat(arg)).isDirectory()
  ? (await readdir(arg)).filter((f) => f.endsWith(".tpt")).sort().map((f) => join(arg, f))
  : [arg];

console.log(
  `tpt-web wasm decode throughput under Node ${process.version} / V8 ${process.versions.v8} ` +
    `(${RUNS} runs each after ${WARMUP} warm-up, median; push slices of ${SLICE / 1024} KiB)\n`
);
console.log("| file | size | median | min | realtime factor | input MB/s | video fps |");
console.log("| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
for (const path of files) {
  const bytes = new Uint8Array(await readFile(path));
  let last;
  for (let i = 0; i < WARMUP; i++) last = decodeAll(bytes);
  const ms = [];
  for (let i = 0; i < RUNS; i++) {
    const t = performance.now();
    last = decodeAll(bytes);
    ms.push(performance.now() - t);
  }
  ms.sort((a, b) => a - b);
  const med = ms[Math.floor(ms.length / 2)];
  const media = Math.max(
    last.rate ? last.samples / last.rate : 0,
    last.fps ? last.frames / last.fps : 0
  );
  const secs = med / 1000;
  console.log(
    `| ${basename(path)} | ${(bytes.length / 1024).toFixed(1)} KiB | ${med.toFixed(2)} ms | ` +
      `${ms[0].toFixed(2)} ms | ${(media / secs).toFixed(0)}x | ${(bytes.length / secs / 1e6).toFixed(0)} | ` +
      `${last.frames ? (last.frames / secs).toFixed(0) : "-"} |`
  );
}
console.log(`\nwasm linear memory at exit: ${(mod.memoryBytes / 1048576).toFixed(1)} MiB`);
