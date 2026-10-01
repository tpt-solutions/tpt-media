// tpt-web.js: browser playback on top of the tpt-web wasm decoder.
//
// tpt-decoder.js moves bytes across the wasm boundary; this file decides
// WHEN things happen: fetching, WebAudio scheduling, canvas drawing, pause,
// underrun recovery. No bundler, no dependencies: an ES module that the
// browser loads directly (see index.html).
//
// STATUS: run against the real browser APIs by web/browser-check.mjs — a real
// AudioContext on the machine's audio device, a real canvas read back with
// getImageData, real requestAnimationFrame, the real wasm module. 29 checks,
// all passing, repeated (todo.md §45): scheduling, pacing, presentation and
// the end-of-stream path are exercised for real. What that does NOT cover:
// nobody has *heard* it, and on a machine with no audio output device the
// playback-dependent checks report "not verifiable here" rather than passing,
// so a pass means this file works, not that the output is pleasing.
// check.mjs (Node) and mock-browser.mjs (fakes) cover the rest.
//
// Clock model: the AudioContext clock is the master clock for everything,
// audio-only, video-only or both. `t0` maps media time to context time:
//   context time of a frame = t0 + frame.pts
// It is anchored when the first audio chunk or picture arrives. Audio is
// scheduled ahead of time with sample-accurate `start(when)`; video is
// presented from requestAnimationFrame when its pts is due. If audio is
// late (an underrun) `t0` is shifted so playback resumes cleanly instead of
// trying to catch up.

import { TptModule, Status } from "./tpt-decoder.js";

const AUDIO_AHEAD_S = 0.5; // schedule at most this much audio into the future
const START_DELAY_S = 0.15; // lead time before the first frame plays
const TICK_MS = 8; // pump interval
const INBOX_CHUNKS = 4; // fetched-but-unpushed chunks held at most
const STATS_MS = 250;

function makeReader(source) {
  if (typeof source === "string" || source instanceof URL) {
    return fetch(source).then((r) => {
      if (!r.ok) throw new Error(`cannot fetch ${source}: HTTP ${r.status}`);
      return r.body.getReader();
    });
  }
  if (typeof Blob !== "undefined" && source instanceof Blob) {
    return Promise.resolve(source.stream().getReader());
  }
  const bytes = source instanceof ArrayBuffer ? new Uint8Array(source) : source;
  let off = 0;
  return Promise.resolve({
    async read() {
      if (off >= bytes.length) return { done: true, value: undefined };
      const value = bytes.subarray(off, off + 65536);
      off += 65536;
      return { done: false, value };
    },
    async cancel() {},
  });
}

export class TptPlayer {
  /**
   * @param {object} opts
   * @param {HTMLCanvasElement} [opts.canvas]   where video is drawn
   * @param {string|URL|ArrayBuffer|Uint8Array} [opts.wasmUrl]  defaults to ./tpt_web.wasm next to this file
   * @param {object} [opts.limits]              see TptModule.open (all optional)
   * @param {(stats: object) => void} [opts.onStats]
   * @param {(info: object) => void} [opts.onInfo]   stream info once tracks are known
   * @param {() => void} [opts.onEnded]
   * @param {(err: Error) => void} [opts.onError]
   */
  constructor({ canvas, wasmUrl, limits = {}, onStats, onInfo, onEnded, onError } = {}) {
    this.canvas = canvas ?? null;
    this.g2d = canvas ? canvas.getContext("2d") : null;
    this.wasmUrl = wasmUrl ?? new URL("./tpt_web.wasm", import.meta.url);
    // Packets are decoded in file order, so a full video queue also holds
    // back the audio packets behind it. 8 pictures keeps roughly half a
    // second of look-ahead at 15 fps (about a quarter second at 30 fps); the
    // trade-off is memory: each queued picture is width*height*4 bytes.
    this.limits = { maxVideoQueueFrames: 8, ...limits };
    this.onStats = onStats;
    this.onInfo = onInfo;
    this.onEnded = onEnded;
    this.onError = onError;
    this.module = null;
    this.session = 0;
    this.#reset();
  }

  #reset() {
    this.decoder = null;
    this.ctx = null;
    this.reader = null;
    this.inbox = [];
    this.inputDone = false;
    this.finished = false;
    this.t0 = null;
    this.audioNext = 0;
    this.underruns = 0;
    this.droppedFrames = 0;
    this.shownFrames = 0;
    this.announced = false;
    this.tickTimer = null;
    this.statsTimer = null;
    this.raf = 0;
  }

  /**
   * Start playing `source`: a URL (streamed with fetch), a File/Blob, or an
   * ArrayBuffer/Uint8Array. Call from a user gesture (click) so the browser
   * lets the AudioContext start. Resolves once playback has started; the
   * `onEnded` / `onError` callbacks report the rest.
   */
  async play(source) {
    this.stop();
    const session = ++this.session;
    try {
      const AC = window.AudioContext || window.webkitAudioContext;
      // Created synchronously inside the gesture, before any await.
      this.ctx = new AC();
      const ctxReady = this.ctx.resume();
      this.module ??= await TptModule.load(this.wasmUrl);
      await ctxReady;
      if (session !== this.session) return;
      this.decoder = this.module.open(this.limits);
      this.reader = await makeReader(source);
      if (session !== this.session) return;
      this.#feedLoop(session);
      this.tickTimer = setInterval(() => this.#tick(session), TICK_MS);
      this.statsTimer = setInterval(() => this.#emitStats(), STATS_MS);
      this.raf = requestAnimationFrame(() => this.#frame(session));
    } catch (e) {
      this.#fail(e, session);
    }
  }

  /** Pause / resume. Suspending the AudioContext stops the master clock, so
   *  video pauses with the audio. */
  pause() {
    return this.ctx?.suspend();
  }
  resume() {
    return this.ctx?.resume();
  }

  /** Stop playback and free the decoder, the context and the network reader. */
  stop() {
    this.session++;
    clearInterval(this.tickTimer);
    clearInterval(this.statsTimer);
    if (this.raf) cancelAnimationFrame(this.raf);
    this.reader?.cancel?.().catch?.(() => {});
    this.decoder?.close();
    this.ctx?.close().catch(() => {});
    this.#reset();
  }

  #fail(e, session) {
    if (session !== this.session) return;
    this.stop();
    if (this.onError) this.onError(e);
    else console.error(e);
  }

  // ---- network -> inbox -----------------------------------------------
  async #feedLoop(session) {
    try {
      while (session === this.session && !this.inputDone) {
        if (this.inbox.length >= INBOX_CHUNKS) {
          await new Promise((r) => setTimeout(r, 10)); // back-pressure
          continue;
        }
        const { done, value } = await this.reader.read();
        if (session !== this.session) return;
        if (done) this.inputDone = true;
        else this.inbox.push(value);
      }
    } catch (e) {
      this.#fail(e, session);
    }
  }

  // ---- pump + audio ---------------------------------------------------
  #tick(session) {
    if (session !== this.session) return;
    try {
      const d = this.decoder;
      while (this.inbox.length) {
        const chunk = this.inbox[0];
        const n = d.push(chunk);
        if (n === chunk.length) this.inbox.shift();
        else {
          this.inbox[0] = chunk.subarray(n); // decoder full: retry after pumping
          break;
        }
      }
      if (this.inputDone && !this.inbox.length && !this.finished) {
        d.finish();
        this.finished = true;
      }
      d.pump();
      this.#scheduleAudio();
      if (!this.announced) {
        const info = d.streamInfo;
        if (info.audio || info.video) {
          this.announced = true;
          if (this.canvas && info.video) {
            this.canvas.width = info.video.width;
            this.canvas.height = info.video.height;
          }
          this.onInfo?.(info);
        }
      }
      if (this.finished && d.status === Status.ENDED && this.ctx.currentTime >= this.audioNext - 0.01) {
        this.#emitStats();
        const cb = this.onEnded;
        this.stop();
        cb?.();
      }
    } catch (e) {
      this.#fail(e, session);
    }
  }

  #anchor(pts) {
    if (this.t0 === null) this.t0 = this.ctx.currentTime + START_DELAY_S - pts;
  }

  #scheduleAudio() {
    const d = this.decoder;
    const ctx = this.ctx;
    while (this.audioNext - ctx.currentTime < AUDIO_AHEAD_S) {
      const a = d.nextAudio();
      if (!a) break;
      this.#anchor(a.pts);
      let when = this.t0 + a.pts;
      const now = ctx.currentTime;
      if (when < now) {
        // Underrun: shift the clock so this chunk starts just ahead of now.
        this.t0 += now + 0.02 - when;
        when = now + 0.02;
        this.underruns++;
      }
      const buffer = ctx.createBuffer(a.channels, a.frames, a.sampleRate);
      for (let c = 0; c < a.channels; c++) {
        // `a.planar` is a view into wasm memory: copy it out right now.
        buffer.copyToChannel(a.planar.subarray(c * a.frames, (c + 1) * a.frames), c);
      }
      const node = ctx.createBufferSource();
      node.buffer = buffer;
      node.connect(ctx.destination);
      node.start(when);
      this.audioNext = when + a.frames / a.sampleRate;
    }
  }

  // ---- video ------------------------------------------------------------
  #frame(session) {
    if (session !== this.session) return;
    try {
      const d = this.decoder;
      if (d && this.g2d) {
        if (this.t0 === null) {
          // Video-only stream: anchor on the first picture.
          const first = d.peekVideoPts();
          if (!Number.isNaN(first)) this.#anchor(first);
        }
        if (this.t0 !== null) {
          const media = this.ctx.currentTime - this.t0;
          while (d.peekVideoPts() <= media) {
            const v = d.nextVideo();
            if (d.peekVideoPts() <= media) {
              this.droppedFrames++; // a newer picture is also due: skip this one
              continue;
            }
            // `v.rgba` is a view into wasm memory: draw it right now.
            this.g2d.putImageData(new ImageData(v.rgba, v.width, v.height), 0, 0);
            this.shownFrames++;
            break;
          }
        }
      }
      this.raf = requestAnimationFrame(() => this.#frame(session));
    } catch (e) {
      this.#fail(e, session);
    }
  }

  #emitStats() {
    if (!this.decoder || !this.onStats) return;
    this.onStats({
      ...this.decoder.stats(),
      underruns: this.underruns,
      shownFrames: this.shownFrames,
      droppedFrames: this.droppedFrames,
      mediaTime: this.t0 === null ? 0 : Math.max(0, this.ctx.currentTime - this.t0),
    });
  }
}
