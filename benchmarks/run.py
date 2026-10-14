#!/usr/bin/env python3
"""TPT benchmark harness: tpt codecs vs. ffmpeg's reference encoders.

    python benchmarks/run.py fetch      # download + prepare the Xiph corpus
    python benchmarks/run.py run        # measure, write benchmarks/results/<stamp>.json
    python benchmarks/run.py report     # regenerate docs/benchmarks.md from all results
    python benchmarks/run.py audio      # re-measure audio only (keeps latest video) + report
    python benchmarks/run.py proxy      # add the entropy-coding proxy to the latest run + report
    python benchmarks/run.py all        # run + report

Video: Thrum and Vault (quality sweep) vs x264 / x265 / SVT-AV1 (CRF sweep),
all scored by the same ffmpeg PSNR/SSIM filters on decoded output, rate taken
from the raw payload (no container overhead on either side).
Audio: Aura (lossless) vs FLAC and an LPC-residual entropy estimate; Cinder
(lossy) and Opus against a Gaussian rate-distortion bound.
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


# Cinder's quality setting maps to bits via 4 + floor(8q/100). These are the
# quality values that land exactly on each bit depth 4..=12.
CINDER_BITS = list(range(4, 13))
CINDER_QUALITY = [0, 13, 25, 38, 50, 63, 75, 88, 100]
# Per-band step, half-octave grid from the wire's minimum (1/1024) to its maximum (64).
CINDER_STEPS = [2 ** (k / 2) for k in range(-20, 12)]
OPUS_KBPS = [12, 16, 24, 32, 48, 64, 96, 128]
SNR_TARGETS = [10, 15, 20, 25, 30]


def wav_samples(path):
    import wave
    with wave.open(str(path)) as w:
        return np.frombuffer(w.readframes(w.getnframes()), "<i2").astype(np.float64)


def snr_db(ref, test):
    n = min(len(ref), len(test))
    err = ((ref[:n] - test[:n]) ** 2).sum()
    return 10 * math.log10((ref[:n] ** 2).sum() / max(err, 1e-12))


def cinder_sweep(wav, tmp):
    """Every (bit depth, step) pair on the grid. Cinder v4 Rice-codes its levels
    per packet, so the rate depends on the step as well as on the bit depth
    (which bounds the levels). The envelope over the grid is the rate-quality
    curve a perfect rate control could reach."""
    ref = wav_samples(wav)
    pts = []
    seconds = len(ref) / 48000
    for bits, q in zip(CINDER_BITS, CINDER_QUALITY):
        for step in CINDER_STEPS:
            out, dec = tmp / "c.tpt", tmp / "c.wav"
            # Rate is the codec's payload (from `benchmark`), so the .tpt framing
            # (about 43 bytes per packet) is not charged to Cinder.
            j = json.loads(sh([TPT, "benchmark", "--codec", "cinder", "--runs", 1, "--quality", q,
                               "--band-step", f"{step:.6f}", "--json", wav]).stdout)
            sh([TPT, "encode", "--codec", "cinder", "--quality", q, "--band-step", f"{step:.6f}", wav, out])
            sh([TPT, "decode", "--codec", "cinder", "--container", "wav", out, dec])
            pts.append(dict(bits=bits, step=round(step, 6), kbps=j["bitrate_bits_per_second"] / 1000,
                            snr_db=snr_db(ref, wav_samples(dec))))
        best = max((p for p in pts if p["bits"] == bits), key=lambda p: p["snr_db"])
        print(f"   cinder {bits} bits: best step {best['step']} -> {best['kbps']:.0f} kbps, {best['snr_db']:.2f} dB", flush=True)
    return pts


def opus_points(wav, tmp):
    ref = wav_samples(wav)
    seconds = len(ref) / 48000
    pts = []
    for k in OPUS_KBPS:
        out, dec = tmp / "o.opus", tmp / "o.wav"
        ff("-i", wav, "-c:a", "libopus", "-b:a", f"{k}k", "-ac", 1, "-ar", 48000, out)
        ff("-i", out, "-ac", 1, "-ar", 48000, "-c:a", "pcm_s16le", dec)
        pts.append(dict(setting=k, kbps=kbps(out.stat().st_size, seconds), snr_db=snr_db(ref, wav_samples(dec))))
        print(f"   opus {k}k: {pts[-1]['kbps']:.1f} kbps, {pts[-1]['snr_db']:.2f} dB", flush=True)
    return pts


def dct_matrix(n):
    k = np.arange(n)[:, None]
    i = np.arange(n)[None, :]
    m = np.cos(np.pi / n * (i + 0.5) * k) * math.sqrt(2 / n)
    m[0] /= math.sqrt(2)
    return m


def _box(a, r0, r1):
    """Mean over a (2*r0+1) x (2*r1+1) neighbourhood, edge-clamped."""
    h, w = a.shape
    padded = np.pad(a, ((r0, r0), (r1, r1)), mode="edge")
    out = np.zeros_like(a)
    for di in range(2 * r0 + 1):
        for dj in range(2 * r1 + 1):
            out += padded[di:di + h, dj:dj + w]
    return out / ((2 * r0 + 1) * (2 * r1 + 1))


def rd_bound(wav, block=256, bands=16, side_bits=4):
    """Gaussian rate-distortion bound (reverse water-filling) with side information.

    The signal is split into orthonormal DCT-II blocks of `block` samples. The
    variance of every coefficient is the mean power of its band in that block
    (flat within the band), which a codec would have to transmit: `side_bits`
    per band per block is charged on top of the water-filling rate. This is
    still an estimate, not a proven bound: the envelope is fitted to the data
    it is scored on, and the cost of coding it is taken as a fixed budget."""
    x = wav_samples(wav)
    n = len(x) // block * block
    coefs = x[:n].reshape(-1, block) @ dct_matrix(block).T
    band_power = (coefs ** 2).reshape(-1, bands, block // bands).mean(axis=2) + 1e-12
    var = np.repeat(band_power, block // bands, axis=1)
    p_sig = (x[:n] ** 2).mean()
    seconds = n / 48000
    side_kbps = bands * side_bits * (48000 / block) / 1000
    out = []
    for snr in range(0, 61, 5):
        budget = p_sig / 10 ** (snr / 10) * n  # total squared error allowed
        lo, hi = math.log(1e-12), math.log(var.max())
        for _ in range(80):  # find the water level theta with D(theta) = budget
            mid = (lo + hi) / 2
            theta = math.exp(mid)
            if np.minimum(theta, var).sum() > budget:
                hi = mid
            else:
                lo = mid
        theta = math.exp(lo)
        bits = np.maximum(0.0, 0.5 * np.log2(var / theta)).sum()
        out.append(dict(snr_db=snr, kbps=bits / seconds / 1000 + side_kbps))
    return out


def entropy_proxy(wav, block=256, bands=16, side_bits=4):
    """What entropy-coding Cinder's quantised coefficients could reach, as a proxy.

    Uses the same DCT blocks and band layout as `rd_bound`, normalised input,
    and one uniform step per target SNR (uniform is the MSE-optimal choice at
    these rates). Rate is the empirical entropy of the levels, per band, over
    the whole clip, plus the same envelope side information as the bound. This
    is an oracle: the histograms are fitted to the clip and their transmission
    is not charged, and the transform is a DCT, not Cinder's MDCT. It shows the
    headroom entropy coding would have, not what Cinder achieves."""
    x = wav_samples(wav) / 32768.0
    n = len(x) // block * block
    flat = (x[:n].reshape(-1, block) @ dct_matrix(block).T).reshape(-1)
    band_idx = np.tile(np.repeat(np.arange(bands), block // bands), n // block)
    p_sig = (x[:n] ** 2).mean()
    seconds = n / 48000
    side_kbps = bands * side_bits * (48000 / block) / 1000
    out = []
    for target in SNR_TARGETS:
        lo, hi = math.log(1e-4), math.log(4.0)
        for _ in range(60):  # bisection: largest step that still reaches the target
            mid = (lo + hi) / 2
            lev = np.round(flat / math.exp(mid))
            snr = 10 * math.log10(p_sig / (((flat - lev * math.exp(mid)) ** 2).sum() / n))
            if snr > target:
                lo = mid
            else:
                hi = mid
        step = math.exp(lo)
        lev = np.round(flat / step).astype(np.int64)
        bits = 0.0
        for b in range(bands):
            _, c = np.unique(lev[band_idx == b], return_counts=True)
            bits += -(c * np.log2(c / c.sum())).sum()
        out.append(dict(snr_db=target, step=round(step, 6), kbps=bits / seconds / 1000 + side_kbps))
    return out


def rate_for_snr(points, target):
    """Lowest rate among points that reach `target` dB, or None."""
    ok = [p["kbps"] for p in points if p["snr_db"] >= target]
    return min(ok) if ok else None


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
            res["cinder"] = cinder_sweep(wav, tmp)
            res["opus"] = opus_points(wav, tmp)
            res["rd_bound"] = rd_bound(wav)
            out[name] = res
            print("  rd bound:", [(p["snr_db"], round(p["kbps"])) for p in res["rd_bound"][::2]], flush=True)
    return out


def git(*a):
    return subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def run_proxy_only():
    """Add the entropy-coding proxy to the latest run without re-measuring anything else."""
    f = sorted(RESULTS.glob("*.json"))[-1]
    data = json.loads(f.read_text())
    for name in data["audio"]:
        data["audio"][name]["entropy_proxy"] = entropy_proxy(CORPUS / "audio" / f"{name}_{AUDIO_SECONDS}s.wav")
    f.write_text(json.dumps(data, indent=1))
    print("updated", f)


def run_audio_only():
    """Re-measure only the audio section, keeping the latest run's video."""
    latest = json.loads(sorted(RESULTS.glob("*.json"))[-1].read_text())
    sh(["cargo", "build", "--release", "-p", "tpt-cli"], cwd=ROOT)
    meta = dict(latest["meta"], date=datetime.datetime.now().isoformat(timespec="seconds"),
                commit=git("rev-parse", "--short", "HEAD"),
                dirty=bool(git("status", "--porcelain", "--untracked-files=no")))
    data = dict(meta=meta, video=latest["video"], audio=run_audio())
    f = RESULTS / f"{meta['date'][:10]}-{meta['commit']}{'-dirty' if meta['dirty'] else ''}.json"
    f.write_text(json.dumps(data, indent=1))
    print("wrote", f)


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
         f"{m['frames']} frames, 4:2:0. Audio is 30 s mono 48 kHz excerpts of the Sintel trailer audio and Elephants Dream's "
         "music (ED-CM stereo, downmixed).",
         "- **Video metric:** every codec's output is decoded and scored by the *same* ffmpeg PSNR and SSIM filters "
         "against the source. Rate is the raw payload, with no container overhead on either side. Each codec is swept over "
         "its quality setting; a single point means little, the curve is the result.",
         "- **Video reference ceiling:** no true rate-distortion bound exists for natural video, so the practical ceiling is the "
         "best of x264 `veryslow`, x265 `veryslow` and SVT-AV1 preset 4, at default tuning.",
         "- **Video equal-quality column:** how many times *larger* the tpt stream is than the best reference at the same luma "
         "PSNR (log-linear interpolation). `>1` means tpt is worse; `n/a` means the PSNR is outside the reference's range.",
         "- **Audio, lossless:** Aura is compared by size against FLAC and against an *estimate* of an ideal predictor "
         "(order-32 LPC per 4096-sample block, ideal Laplace entropy coder, side info ignored). It is an estimate, not a proven bound.",
         "- **Audio, lossy:** Cinder is swept over bit depth (4–12, via `--quality`) and per-band step (`--band-step`, "
         "half-octave grid from 1/1024 to about 45). Rate is the codec's payload, not the file size. The Cinder rows use the "
         "lowest rate that reaches each SNR over the whole grid, which is an oracle choice: a shipped encoder would need rate "
         "control to find the step. Cinder v4 uses a per-packet Rice code, which spends at least one bit on every coefficient: "
         "about 48 kbps at 48 kHz, a floor it cannot go under however coarse the step. The proxy shows the headroom an "
         "arithmetic coder would have; closing that gap is the next codec step.",
         "- **Audio, lossy reference:** Opus (libopus via ffmpeg, mono 48 kHz) at 12–128 kbps. Opus is perceptually tuned and "
         "SNR is a waveform metric, so Opus is expected to look worse here than it sounds; read the SNR columns as "
         "a waveform-fidelity comparison, not a listening test.",
         "- **Audio, bound:** the Gaussian rate-distortion bound from reverse water-filling over 16 bands per 256-sample DCT block, "
         "each band's variance taken as its power in that block, plus 4 bits per band per block of side information (about 12 kbps). "
         "It is an estimate for that envelope model, not a proven bound on every source. Earlier runs used per-coefficient "
         "variances with no side information; those numbers were far too optimistic and are not comparable.",
         "- **Cinder's wire step:** version 3 carries steps in 1/1024 units. Version 2 wrote whole numbers only, which made every "
         "step below 1.0 decode to near silence; the rows below post-date that fix.",
         "- **Cinder's loudest peaks clip at fine steps.** At 12 bits the range is about ±2048 × step, so a step of 0.001 clips "
         "coefficients above about 2. The sweep includes those points; the SNR column shows the cost.", ""]

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

    L += ["## Audio, lossless", "", "| clip | PCM | Aura | FLAC-5 | FLAC-8 | FLAC-12 | LPC estimate | Aura vs FLAC-8 | Aura bit-exact |",
          "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: |"]
    for name, r in cur["audio"].items():
        k = lambda b: f"{b / 1000:.0f} kB"
        L.append(f"| {name} | {k(r['pcm_bytes'])} | {k(r['aura']['bytes'])} ({r['aura']['bytes'] / r['pcm_bytes']:.1%}) | "
                 f"{k(r['flac-5']['bytes'])} | {k(r['flac-8']['bytes'])} | {k(r['flac-12']['bytes'])} | "
                 f"{k(r['lpc_estimate']['bytes'])} | {r['aura']['bytes'] / r['flac-8']['bytes']:.2f}× | "
                 f"{'yes' if r['aura']['bit_exact'] else '**NO**'} |")
    L.append("")

    L += ["## Audio, lossy: rate needed for a given SNR", "",
          "Lowest kbps that reaches each SNR (oracle choice over each codec's own sweep). Ratios are against the Gaussian bound. "
          "The entropy-coding proxy is not a codec: it is what entropy coding Cinder's levels could reach, with oracle histograms "
          "and a DCT in place of the MDCT (see Method).", ""]
    for name, r in cur["audio"].items():
        if "cinder" not in r:
            continue
        L += [f"### {name}", "", "| SNR (dB) | Cinder kbps | Opus kbps | Gaussian bound kbps | Entropy-coding proxy kbps | Cinder ÷ bound | Opus ÷ bound |",
              "| ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
        proxy = {p["snr_db"]: p["kbps"] for p in r.get("entropy_proxy", [])}
        for t in SNR_TARGETS:
            c, o = rate_for_snr(r["cinder"], t), rate_for_snr(r["opus"], t)
            b = next((p["kbps"] for p in r["rd_bound"] if p["snr_db"] >= t), None)
            f = lambda v: "n/a" if v is None else f"{v:.0f}"
            ratio = lambda v: "n/a" if (v is None or b is None) else f"{v / b:.2f}×"
            L.append(f"| {t} | {f(c)} | {f(o)} | {f(b)} | {f(proxy.get(t))} | {ratio(c)} | {ratio(o)} |")
        L.append("")
        L += ["Cinder points (best step per bit depth):", "", "| bits | step | kbps | SNR dB |", "| ---: | ---: | ---: | ---: |"]
        for bits in CINDER_BITS:
            best = max((p for p in r["cinder"] if p["bits"] == bits), key=lambda p: p["snr_db"])
            L.append(f"| {bits} | {best['step']} | {best['kbps']:.0f} | {best['snr_db']:.2f} |")
        L.append("")
        L += ["Opus points:", "", "| bitrate | kbps | SNR dB |", "| ---: | ---: | ---: |"]
        for p in r["opus"]:
            L.append(f"| {p['setting']} | {p['kbps']:.1f} | {p['snr_db']:.2f} |")
        L.append("")

    L += ["## Progress", "", "One row per recorded run. Video: each tpt codec's size relative to the best reference at the same "
          "luma PSNR (median over the points where it is defined). Audio: Aura vs FLAC-8 (lossless); Cinder and Opus vs the "
          "Gaussian bound at 20 dB (lossy; n/a for runs made before that codec was measured).", "",
          "| date | commit | Thrum (×ref) | Vault (×ref) | Aura/FLAC-8 | Cinder @20 dB ÷ bound | Opus @20 dB ÷ bound |",
          "| --- | --- | ---: | ---: | ---: | ---: | ---: |"]
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
        lossy = []
        for key in ("cinder", "opus"):
            vals = []
            for v in r["audio"].values():
                if key not in v:
                    continue
                c = rate_for_snr(v[key], 20)
                b = next((p["kbps"] for p in v["rd_bound"] if p["snr_db"] >= 20), None)
                if c is not None and b:
                    vals.append(c / b)
            lossy.append(f"{np.mean(vals):.2f}×" if vals else "n/a")
        L.append(f"| {r['meta']['date'][:10]} | `{r['meta']['commit']}`{'*' if r['meta']['dirty'] else ''} | "
                 f"{cells[0]} | {cells[1]} | {a:.2f}× | {lossy[0]} | {lossy[1]} |")
    L += ["", "`*` = run on a tree with uncommitted changes.", ""]
    REPORT.write_text("\n".join(L), encoding="utf-8")
    print("wrote", REPORT)


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "all"
    if cmd == "fetch":
        fetch()
    elif cmd == "run":
        fetch(); run()
    elif cmd == "proxy":
        run_proxy_only(); report()
    elif cmd == "audio":
        fetch(); run_audio_only(); report()
    elif cmd == "report":
        report()
    elif cmd == "all":
        fetch(); run(); report()
    else:
        sys.exit(__doc__)
