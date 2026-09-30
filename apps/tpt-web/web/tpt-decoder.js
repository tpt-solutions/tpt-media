// tpt-decoder.js: the thin JavaScript half of the tpt-web C ABI.
//
// This file is the hand-written counterpart of apps/tpt-web/src/abi.rs (there
// is no wasm-bindgen). It has no DOM dependency, so it runs unchanged in a
// browser, a Worker, and Node (see check.mjs / bench.mjs). All the playback
// policy (WebAudio scheduling, canvas drawing, fetching) lives in
// tpt-web.js.
//
// Memory rules, restated from abi.rs because getting them wrong corrupts
// data silently:
//   * The wasm memory can grow during ANY call that allocates, which detaches
//     the old `memory.buffer`. Never cache a typed-array view across calls;
//     this file builds views fresh and documents which ones are ephemeral.
//   * `audio.planar` and `video.rgba` returned by nextAudio()/nextVideo() are
//     views straight into wasm memory. They are valid only until the next
//     call into this decoder. Copy them (copyToChannel / putImageData /
//     .slice()) before doing anything else.

export const ABI_VERSION = 1;

/** Largest slice handed to the module in one go (matches STAGING_MAX_BYTES
 *  in src/limits.rs). Larger pushes are split transparently. */
export const MAX_SLICE = 4 * 1024 * 1024;

export const Status = Object.freeze({
  NEED_INPUT: 0,
  READY: 1,
  ENDED: 2,
  FAILED: 3,
});

// Keys for tpt_web_info (see the table in abi.rs).
const INFO = Object.freeze({
  hasAudio: 0,
  audioSampleRate: 1,
  audioChannels: 2,
  hasVideo: 3,
  videoWidth: 4,
  videoHeight: 5,
  fpsNum: 6,
  fpsDen: 7,
  audioQueuedMs: 8,
  videoQueued: 9,
  memoryInUse: 10,
  memoryCeiling: 11,
  bytesIn: 12,
  packets: 13,
  decodeErrors: 14,
  droppedClusters: 15,
  droppedOutputs: 16,
  audioSamples: 17,
  videoFrames: 18,
  truncatedInput: 19,
  ignoredTracks: 20,
});

/** An error reported by the wasm module. `code` is the negative ABI code:
 *  -2 input full, -3 container, -4 limit, -5 unsupported, -6 codec,
 *  -7 bad argument, -1 invalid handle. */
export class TptError extends Error {
  constructor(message, code) {
    super(message);
    this.name = "TptError";
    this.code = code;
  }
}

export class TptModule {
  /**
   * @param {string | URL | Response | ArrayBuffer | Uint8Array | WebAssembly.Module} source
   * @returns {Promise<TptModule>}
   */
  static async load(source) {
    let instance;
    if (source instanceof WebAssembly.Module) {
      instance = await WebAssembly.instantiate(source, {});
    } else if (source instanceof ArrayBuffer || ArrayBuffer.isView(source)) {
      ({ instance } = await WebAssembly.instantiate(source, {}));
    } else {
      const response =
        source instanceof Response ? source : await fetch(source);
      if (!response.ok) {
        throw new Error(`cannot fetch wasm: HTTP ${response.status}`);
      }
      const type = response.headers.get("Content-Type") || "";
      if (WebAssembly.instantiateStreaming && type.startsWith("application/wasm")) {
        ({ instance } = await WebAssembly.instantiateStreaming(response, {}));
      } else {
        // Servers that do not send application/wasm (or old browsers).
        ({ instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {}));
      }
    }
    const version = instance.exports.tpt_web_abi_version();
    if (version !== ABI_VERSION) {
      throw new Error(
        `tpt-web ABI mismatch: module is v${version}, tpt-decoder.js expects v${ABI_VERSION}`
      );
    }
    return new TptModule(instance);
  }

  constructor(instance) {
    this.x = instance.exports;
    this.memory = instance.exports.memory;
  }

  /** Current size of the module's linear memory in bytes. */
  get memoryBytes() {
    return this.memory.buffer.byteLength;
  }

  errorMessage() {
    const ptr = this.x.tpt_web_error_ptr();
    const len = this.x.tpt_web_error_len();
    return new TextDecoder().decode(new Uint8Array(this.memory.buffer, ptr, len));
  }

  /** Throw a TptError if `code` is negative, else return it. */
  check(code) {
    if (code < 0) throw new TptError(this.errorMessage(), code);
    return code;
  }

  /**
   * Open a decoder. Every limit is optional; omitted (0) keeps the module's
   * conservative default, and the module clamps whatever is passed to its
   * compiled-in ceiling.
   * @returns {TptDecoder}
   */
  open({ maxInputBytes = 0, maxFrameBytes = 0, maxAudioQueueMs = 0, maxVideoQueueFrames = 0 } = {}) {
    const h = this.x.tpt_web_open(maxInputBytes, maxFrameBytes, maxAudioQueueMs, maxVideoQueueFrames);
    if (h === 0) throw new TptError(this.errorMessage(), -4);
    return new TptDecoder(this, h);
  }
}

export class TptDecoder {
  constructor(module, handle) {
    this.m = module;
    this.x = module.x;
    this.h = handle;
  }

  /**
   * Offer bytes to the decoder. Returns how many bytes were consumed. If it
   * is less than `bytes.length` the input buffer is full: call pump(), drain
   * the output, and offer `bytes.subarray(consumed)` again. Throws TptError
   * on a fatal error (malformed or hostile input).
   * @param {Uint8Array} bytes
   * @returns {number}
   */
  push(bytes) {
    let done = 0;
    while (done < bytes.length) {
      const n = Math.min(MAX_SLICE, bytes.length - done);
      const ptr = this.x.tpt_web_input_ptr(this.h, n);
      if (ptr === 0) throw new TptError(this.m.errorMessage(), -4);
      // Fresh view: tpt_web_input_ptr may have grown memory.
      new Uint8Array(this.m.memory.buffer, ptr, n).set(bytes.subarray(done, done + n));
      const code = this.x.tpt_web_input_commit(this.h, n);
      if (code === -2) return done; // InputFull: nothing of this slice consumed
      this.m.check(code);
      done += n;
    }
    return done;
  }

  /** Declare end of input. */
  finish() {
    this.m.check(this.x.tpt_web_finish(this.h));
  }

  /** Decode until the output queues are full or input runs out.
   *  @returns {number} a Status value (never FAILED: that throws). */
  pump() {
    return this.m.check(this.x.tpt_web_pump(this.h));
  }

  get status() {
    return this.m.check(this.x.tpt_web_status(this.h));
  }

  info(name) {
    return this.x.tpt_web_info(this.h, INFO[name]);
  }

  /** Stream description; `audio` / `video` are null until the track is seen. */
  get streamInfo() {
    const audio = this.info("hasAudio")
      ? {
          sampleRate: this.info("audioSampleRate"),
          channels: this.info("audioChannels"),
        }
      : null;
    const video = this.info("hasVideo")
      ? {
          width: this.info("videoWidth"),
          height: this.info("videoHeight"),
          fps: this.info("fpsNum") / (this.info("fpsDen") || 1),
        }
      : null;
    return { audio, video };
  }

  /** All counters, for a stats overlay. */
  stats() {
    const out = {};
    for (const k of Object.keys(INFO)) out[k] = this.info(k);
    out.wasmMemoryBytes = this.m.memoryBytes;
    return out;
  }

  /**
   * Take the next decoded audio chunk, or null. `planar` is an EPHEMERAL view
   * (see the file header): channel c is planar.subarray(c*frames, (c+1)*frames).
   */
  nextAudio() {
    const frames = this.x.tpt_web_audio_next(this.h);
    if (frames === 0) return null;
    const channels = this.x.tpt_web_audio_info(this.h, 2);
    const ptr = this.x.tpt_web_audio_ptr(this.h);
    return {
      pts: this.x.tpt_web_audio_info(this.h, 0),
      sampleRate: this.x.tpt_web_audio_info(this.h, 1),
      channels,
      frames,
      planar: new Float32Array(this.m.memory.buffer, ptr, channels * frames),
    };
  }

  /** Presentation time (seconds) of the next picture, or NaN if none queued. */
  peekVideoPts() {
    return this.x.tpt_web_video_peek_pts(this.h);
  }

  /** Take the next picture, or null. `rgba` is an EPHEMERAL Uint8ClampedArray. */
  nextVideo() {
    if (this.x.tpt_web_video_next(this.h) === 0) return null;
    const width = this.x.tpt_web_video_info(this.h, 1);
    const height = this.x.tpt_web_video_info(this.h, 2);
    const ptr = this.x.tpt_web_video_ptr(this.h);
    return {
      pts: this.x.tpt_web_video_info(this.h, 0),
      width,
      height,
      rgba: new Uint8ClampedArray(this.m.memory.buffer, ptr, width * height * 4),
    };
  }

  /** Free everything the decoder owns. The object is unusable afterwards. */
  close() {
    if (this.h) this.x.tpt_web_close(this.h);
    this.h = 0;
  }
}
