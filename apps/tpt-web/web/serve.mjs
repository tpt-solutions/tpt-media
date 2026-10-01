// Zero-dependency static server for the browser checks.
//
// Two things it does that `python -m http.server` cannot:
//   * serves the player folder over http (ES modules and `WebAssembly`
//     cannot be loaded from file://);
//   * collects the result a test page POSTs to `/__done`, so the driver waits
//     for the page to say it finished rather than guessing when it has.
//
// The second is the important one. Driving an async page with Chrome's
// `--dump-dom` means racing the `load` event, and a module with top-level
// `await` makes that race lose. A page that reports in is unambiguous.
//
// `/__slow?ms=N` also exists: it responds after N *real* milliseconds, which a
// page can use to hold a resource open. Real milliseconds matter, because
// Chrome's `--virtual-time-budget` fast-forwards renderer timers while the
// WebAudio clock runs on the audio thread in real time, so a virtual-time run
// races ahead of the audio clock and reports a frozen `currentTime`.

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".wasm": "application/wasm",
  ".tpt": "application/octet-stream",
  ".css": "text/css; charset=utf-8",
};

/// Serve `root` on an ephemeral port.
/// Resolves to `{ server, port, close, done }`, where `done` is a promise that
/// resolves with the body a test page POSTed to `/__done`.
export function serve(root) {
  const base = resolve(root);
  let resolveDone;
  const done = new Promise((ok) => { resolveDone = ok; });

  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://127.0.0.1");

    if (url.pathname === "/__done" && req.method === "POST") {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      res.writeHead(204).end();
      resolveDone(Buffer.concat(chunks).toString("utf8"));
      return;
    }

    if (url.pathname.startsWith("/__slow")) {
      const ms = Number(url.searchParams.get("ms") ?? 500);
      setTimeout(() => {
        res.writeHead(200, { "content-type": "text/javascript" });
        res.end("// slow resource\n");
      }, Number.isFinite(ms) ? ms : 500);
      return;
    }

    // `normalize` collapses any `..` before the join, so a request cannot
    // escape the served folder.
    const rel = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "");
    const file = join(base, rel === "" ? "index.html" : rel);
    if (!file.startsWith(base)) {
      res.writeHead(403).end("forbidden");
      return;
    }
    try {
      const body = await readFile(file);
      res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
      res.end(body);
    } catch {
      res.writeHead(404).end("not found");
    }
  });

  return new Promise((ok) => {
    server.listen(0, "127.0.0.1", () => {
      ok({ server, port: server.address().port, close: () => server.close(), done });
    });
  });
}