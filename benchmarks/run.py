#!/usr/bin/env python3
"""TPT benchmark harness: tpt codecs vs. ffmpeg's reference encoders.

    python benchmarks/run.py fetch      # download + prepare the Xiph corpus
    python benchmarks/run.py run        # measure, write benchmarks/results/<stamp>.json
    python benchmarks/run.py report     # regenerate docs/benchmarks.md from all results
    python benchmarks/run.py all        # run + report

Video: Thrum and Vault (quality sweep) vs x264 / x265 / SVT-AV1 (CRF sweep),
all scored by the same ffmpeg PSNR/SSIM filters on decoded output, rate taken
from the raw payload (no container overhead on either side).
Audio: Aura (lossless) vs FLAC and an LPC-residual entropy estimate.
Cinder is deliberately excluded until the v2 overlap-add redesign lands.
"""
import datetime, json, math, os, re, subprocess, sys, tempfile, time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
BENCH = ROOT / "benchmarks"
CORPUS = BENCH / "corpus"
RESULTS = BENCH / "results"
REPORT = ROOT / "docs" / "benchmarks.md"
TPT = ROOT / "target" / "release" / ("tpt.exe" if os.name == "nt" else "tpt")

XIPH = "https://media.xiph.org"
VIDEO_CLIPS = ["akiyo_cif", "foreman_cif", "mobile_cif"]  # low / medium / high motion
FRAMES = 100
AUDIO_SRC = {  # name -> (url path, start seconds)
    "sintel_trailer": ("sintel/sintel_trailer-audio.flac", 0),
    "ed_music": ("ED/ED-CM-St-16bit.flac", 60),
}
AUDIO_SECONDS = 30

TPT_QUALITIES = [10, 30, 50, 70, 90]
REFERENCES = {  # name -> (ffmpeg encoder args, rate-control values, container ext)
    "x264-medium": (["-c:v", "libx264", "-preset", "medium", "-crf", "{v}"], [20, 26, 32, 38, 44], "h264"),
    "x264-veryslow": (["-c:v", "libx264", "-preset", "veryslow", "-crf", "{v}"], [20, 26, 32, 38, 44], "h264"),
    "x265-veryslow": (["-c:v", "libx265", "-preset", "veryslow", "-crf", "{v}", "-x265-params", "log-level=error"], [20, 26, 32, 38, 44], "hevc"),
    "svtav1-p4": (["-c:v", "libsvtav1", "-preset", "4", "-crf", "{v}"], [24, 32, 40, 48, 56], "ivf"),
}


def sh(cmd, **kw):
    r = subprocess.run([str(c) for c in cmd], capture_output=True, text=True, **kw)
    if r.returncode:
        raise RuntimeError(f"{' '.join(map(str, cmd))}\n{r.stderr[-800:]}")
    return r


def ff(*args):
    return sh(["ffmpeg", "-v", "error", "-y", *args])


# --------------------------------------------------------------------------
# corpus
# --------------------------------------------------------------------------
def fetch():
    import urllib.request
    (CORPUS / "video").mkdir(parents=True, exist_ok=True)
    (CORPUS / "video_c").mkdir(exist_ok=True)
    (CORPUS / "audio_src").mkdir(exist_ok=True)
    (CORPUS / "audio").mkdir(exist_ok=True)
    for clip in VIDEO_CLIPS:
        raw = CORPUS / "video" / f"{clip}.y4m"
        if not raw.exists():
            print("download", clip)
            urllib.request.urlretrieve(f"{XIPH}/video/derf/y4m/{clip}.y4m", raw)
        out = CORPUS / "video_c" / f"{clip}_{FRAMES}f.y4m"
        if not out.exists():  # tpt's y4m reader requires an explicit C tag
            ff("-f", "yuv4mpegpipe", "-i", raw, "-pix_fmt", "yuv420p", "-frames:v", FRAMES, out)
    for name, (path, start) in AUDIO_SRC.items():
        src = CORPUS / "audio_src" / Path(path).name
        if not src.exists():
            print("download", path)
            urllib.request.urlretrieve(f"{XIPH}/{path}", src)
        out = CORPUS / "audio" / f"{name}_{AUDIO_SECONDS}s.wav"
        if not out.exists():
            ff("-ss", start, "-i", src, "-t", AUDIO_SECONDS, "-ac", 1, "-ar", 48000, "-c:a", "pcm_s16le", out)


# --------------------------------------------------------------------------
# video
# --------------------------------------------------------------------------
def score(decoded, reference):
    g = "[0:v]split[a][b];[1:v]split[c][d];[a][c]psnr;[b][d]ssim"
    err = subprocess.run(["ffmpeg", "-hide_banner", "-i", str(decoded), "-i", str(reference),
                          "-lavfi", g, "-f", "null", "-"], capture_output=True, text=True).stderr
    psnr_y = float(re.search(r"PSNR y:([\d.]+)", err).group(1))
    psnr_avg = float(re.search(r"PSNR y:.*average:([\d.]+)", err).group(1))
    ssim = float(re.search(r"SSIM Y:[\d.]+ \([\d.]+\) U:[\d.]+ \([\d.]+\) V:[\d.]+ \([\d.]+\) All:([\d.]+)", err).group(1))
    return psnr_y, psnr_avg, ssim


def kbps(nbytes, seconds):
    return nbytes * 8 / seconds / 1000


def video_tpt(codec, ref, seconds, tmp):
    pts = []
    for q in TPT_QUALITIES:
        j = json.loads(sh([TPT, "benchmark", "--codec", codec, "--runs", 1, "--quality", q, "--json", ref]).stdout)
        tpt, dec = tmp / "o.tpt", tmp / "o.y4m"
        sh([TPT, "encode", "--codec", codec, "--quality", q, ref, tpt])
        sh([TPT, "decode", "--codec", codec, tpt, dec])
        py, pa, ss = score(dec, ref)
        pts.append(dict(setting=q, kbps=kbps(j["payload_bytes"], seconds), psnr_y=py, psnr_avg=pa, ssim=ss,
                        encode_x_realtime=j["encode_realtime_factor_min"], decode_x_realtime=j["decode_realtime_factor_min"]))
        print(f"   {codec} q{q}: {pts[-1]['kbps']:.0f} kbps  Y {py:.2f} dB  SSIM {ss:.4f}", flush=True)
    return pts


def video_ref(name, ref, seconds, tmp):
    args, values, ext = REFERENCES[name]
    pts = []
    for v in values:
        out, dec = tmp / f"o.{ext}", tmp / "o.y4m"
        a = [x.replace("{v}", str(v)) for x in args]
        t0 = time.perf_counter()
        ff("-i", ref, "-pix_fmt", "yuv420p", *a, "-an", "-f", {"h264": "h264", "hevc": "hevc", "ivf": "ivf"}[ext], out)
        wall = time.perf_counter() - t0
        ff("-i", out, "-pix_fmt", "yuv420p", dec)
        py, pa, ss = score(dec, ref)
        pts.append(dict(setting=v, kbps=kbps(out.stat().st_size, seconds), psnr_y=py, psnr_avg=pa, ssim=ss,
                        encode_x_realtime=seconds / wall))
        print(f"   {name} {v}: {pts[-1]['kbps']:.0f} kbps  Y {py:.2f} dB  SSIM {ss:.4f}", flush=True)
    return pts


def run_video():
    out = {}
    with tempfile.TemporaryDirectory() as t:
        tmp = Path(t)
        for clip in VIDEO_CLIPS:
            ref = CORPUS / "video_c" / f"{clip}_{FRAMES}f.y4m"
            seconds = FRAMES * 1001 / 30000
            print(f"video {clip}", flush=True)
            res = {}
            for codec in ("thrum", "vault"):
                res[codec] = video_tpt(codec, ref, seconds, tmp)
            for name in REFERENCES:
                res[name] = video_ref(name, ref, seconds, tmp)
            out[clip] = res
    return out


# --------------------------------------------------------------------------
# audio (lossless)
# --------------------------------------------------------------------------
def lpc_entropy_bits(x, order=32, block=4096):
    """Ideal-Laplace cost of the per-block LPC residual, in bits. An
    *estimate* of what a strong lossless predictor + ideal entropy coder
    could reach (ignores coefficient side-info, so slightly optimistic)."""
    x = x.astype(np.float64)
    total = 0.0
    for s in range(0, len(x) - block + 1, block):
        b = x[s:s + block]
        w = b * np.hanning(block)
        r = np.correlate(w, w, "full")[block - 1:block + order]
        if r[0] <= 0:
            continue
        r[0] *= 1 + 1e-9
        a = np.zeros(order + 1); a[0] = 1.0; e = r[0]
        for i in range(1, order + 1):  # Levinson-Durbin
            k = -(r[i] + a[1:i] @ r[i - 1:0:-1]) / e
            prev = a.copy()
            a[1:i] = prev[1:i] + k * prev[i - 1:0:-1]
            a[i] = k
            e *= 1 - k * k
        res = np.round(np.convolve(b, a)[order:block])
        scale = max(np.mean(np.abs(res)), 0.5)
        total += len(res) * (math.log2(2 * math.e * scale))
    return total


def run_audio():
    out = {}
    with tempfile.TemporaryDirectory() as t:
        tmp = Path(t)
        for name in AUDIO_SRC:
            wav = CORPUS / "audio" / f"{name}_{AUDIO_SECONDS}s.wav"
            raw = wav.stat().st_size - 44
            print(f"audio {name}", flush=True)
            j = json.loads(sh([TPT, "benchmark", "--codec", "aura", "--runs", 1, "--json", wav]).stdout)
            res = dict(pcm_bytes=raw, aura=dict(bytes=j["payload_bytes"], bit_exact=j["bit_exact"],
                                                encode_x_realtime=j["encode_realtime_factor_min"],
                                                decode_x_realtime=j["decode_realtime_factor_min"]))
            for lvl in (5, 8, 12):
                f = tmp / f"o{lvl}.flac"
                t0 = time.perf_counter()
                ff("-i", wav, "-c:a", "flac", "-compression_level", lvl, f)
                res[f"flac-{lvl}"] = dict(bytes=f.stat().st_size, encode_x_realtime=AUDIO_SECONDS / (time.perf_counter() - t0))
            pcm = np.frombuffer(wav.read_bytes()[44:], dtype="<i2")
            res["lpc_estimate"] = dict(bytes=int(lpc_entropy_bits(pcm) / 8))
            out[name] = res
            print("  ", {k: v.get("bytes", v) if isinstance(v, dict) else v for k, v in res.items()}, flush=True)
    return out


# --------------------------------------------------------------------------
# run / report
# --------------------------------------------------------------------------
def git(*a):
    return subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def run():
    sh(["cargo", "build", "--release", "-p", "tpt-cli"], cwd=ROOT)
    RESULTS.mkdir(parents=True, exist_ok=True)
    meta = dict(date=datetime.datetime.now().isoformat(timespec="seconds"), commit=git("rev-parse", "--short", "HEAD"),
                dirty=bool(git("status", "--porcelain", "--untracked-files=no")), frames=FRAMES,
                ffmpeg=sh(["ffmpeg", "-version"]).stdout.splitlines()[0])
    data = dict(meta=meta, video=run_video(), audio=run_audio())
    f = RESULTS / f"{meta['date'][:10]}-{meta['commit']}{'-dirty' if meta['dirty'] else ''}.json"
    f.write_text(json.dumps(data, indent=1))
    print("wrote", f)


def interp_kbps(points, psnr):
    """Rate (kbps) the reference needs for `psnr` luma dB, log-linear between
    its measured points; None when outside the measured range."""
    pts = sorted((p["psnr_y"], math.log(p["kbps"])) for p in points)
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        if x0 <= psnr <= x1:
            return math.exp(y0 + (y1 - y0) * (psnr - x0) / (x1 - x0))
    return None


def report():
    files = sorted(RESULTS.glob("*.json"))
    runs = [json.loads(f.read_text()) for f in files]
    if not runs:
        sys.exit("no results; run `run` first")
    cur = runs[-1]
    m = cur["meta"]
    L = ["# Benchmarks", "",
         "Generated by `benchmarks/run.py report` from `benchmarks/results/*.json`. Do not edit by hand; "
         "re-run the harness after a codec change and commit the new result file.", "",
         f"Latest run: **{m['date']}**, commit `{m['commit']}`{' (dirty tree)' if m['dirty'] else ''}, "
         f"{m['ffmpeg'].split(' Copyright')[0]}.", "",
         "## Method", "",
         "- **Corpus:** Xiph.org derf CIF sequences (akiyo = low motion, foreman = medium, mobile = high), first "
         f"{m['frames']} frames, 4:2:0; audio is 30 s mono 48 kHz excerpts of Sintel's trailer mix and Elephants Dream's music.",
         "- **Video metric:** every codec's output is decoded and scored by the *same* ffmpeg PSNR and SSIM filters "
         "against the source. Rate is the raw payload, with no container overhead on either side. Each codec is swept over "
         "its quality setting; a single point means little, the curve is the result.",
         "- **Reference ceiling:** no true rate-distortion bound exists for natural video, so the practical ceiling is the "
         "best of x264 `veryslow`, x265 `veryslow` and SVT-AV1 preset 4.",
         "- **Equal-quality column:** how many times *larger* the tpt stream is than the best reference at the same luma "
         "PSNR (log-linear interpolation). `>1` means tpt is worse; `n/a` means the PSNR is outside the reference's range.",
         "- **Audio:** Aura is lossless, so the comparison is size against FLAC and against an *estimate* of an ideal "
         "predictor (order-32 LPC per 4096-sample block, ideal Laplace entropy coder, side info ignored). It is an "
         "estimate, not a proven bound.",
         "- **Cinder (lossy audio) is not benchmarked yet.** It has a known ≈3 dB SNR ceiling until the v2 overlap-add "
         "redesign lands (see `docs/codec-design.md`); once it does, the plan is rate-quality vs Opus plus the Gaussian "
         "reverse-water-filling bound on the MDCT coefficients.", ""]

    L += ["## Video", ""]
    for clip, res in cur["video"].items():
        L += [f"### {clip}", "", "| codec | setting | kbps | PSNR-Y dB | SSIM | enc ×realtime | tpt size vs best ref at equal PSNR |",
              "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
        refs = [n for n in res if n in REFERENCES]
        for name, pts in res.items():
            for p in pts:
                eq = ""
                if name not in REFERENCES:
                    best = [interp_kbps(res[r], p["psnr_y"]) for r in refs]
                    best = [b for b in best if b]
                    eq = f"{p['kbps'] / min(best):.1f}×" if best else "n/a"
                L.append(f"| {name} | {p['setting']} | {p['kbps']:.0f} | {p['psnr_y']:.2f} | {p['ssim']:.4f} | "
                         f"{p['encode_x_realtime']:.2f} | {eq} |")
        L.append("")

    L += ["## Audio (lossless)", "", "| clip | PCM | Aura | FLAC-5 | FLAC-8 | FLAC-12 | LPC estimate | Aura vs FLAC-8 | Aura bit-exact |",
          "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: |"]
    for name, r in cur["audio"].items():
        k = lambda b: f"{b / 1000:.0f} kB"
        L.append(f"| {name} | {k(r['pcm_bytes'])} | {k(r['aura']['bytes'])} ({r['aura']['bytes'] / r['pcm_bytes']:.1%}) | "
                 f"{k(r['flac-5']['bytes'])} | {k(r['flac-8']['bytes'])} | {k(r['flac-12']['bytes'])} | "
                 f"{k(r['lpc_estimate']['bytes'])} | {r['aura']['bytes'] / r['flac-8']['bytes']:.2f}× | "
                 f"{'yes' if r['aura']['bit_exact'] else '**NO**'} |")
    L.append("")

    L += ["## Progress", "", "One row per recorded run: video rate at roughly equal quality is hard to summarise in one number, "
          "so this tracks each tpt codec's size relative to the best reference at the same luma PSNR (median over the "
          "points where it is defined), plus Aura vs FLAC-8.", "",
          "| date | commit | Thrum (×ref) | Vault (×ref) | Aura/FLAC-8 |", "| --- | --- | ---: | ---: | ---: |"]
    for r in runs:
        cells = []
        for codec in ("thrum", "vault"):
            vals = []
            for res in r["video"].values():
                refs = [n for n in res if n in REFERENCES]
                for p in res[codec]:
                    best = [b for b in (interp_kbps(res[x], p["psnr_y"]) for x in refs) if b]
                    if best:
                        vals.append(p["kbps"] / min(best))
            cells.append(f"{np.median(vals):.1f}×" if vals else "n/a")
        a = np.mean([v["aura"]["bytes"] / v["flac-8"]["bytes"] for v in r["audio"].values()])
        L.append(f"| {r['meta']['date'][:10]} | `{r['meta']['commit']}`{'*' if r['meta']['dirty'] else ''} | "
                 f"{cells[0]} | {cells[1]} | {a:.2f}× |")
    L += ["", "`*` = run on a tree with uncommitted changes.", ""]
    REPORT.write_text("\n".join(L), encoding="utf-8")
    print("wrote", REPORT)


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "all"
    if cmd == "fetch":
        fetch()
    elif cmd == "run":
        fetch(); run()
    elif cmd == "report":
        report()
    elif cmd == "all":
        fetch(); run(); report()
    else:
        sys.exit(__doc__)
