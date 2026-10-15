#!/usr/bin/env python3
"""TPT benchmark harness: tpt codecs vs. ffmpeg's reference encoders.

    python benchmarks/run.py fetch      # download + prepare the Xiph corpus
    python benchmarks/run.py run        # measure, write benchmarks/results/<stamp>.json
    python benchmarks/run.py report     # regenerate docs/benchmarks.md from all results
    python benchmarks/run.py proxy      # add the entropy-coding proxy to the latest run + report
    python benchmarks/run.py speech     # re-measure narrowband PESQ on speech (Cinder, Opus, Codec 2, Reed) + report
    python benchmarks/run.py video      # re-measure video only, keeping the latest audio/speech + report
    python benchmarks/run.py audio      # re-measure audio only, keeping the latest video/speech + report
    python benchmarks/run.py all        # run + report

Video: Thrum and Vault (quality sweep) vs x264 / x265 / SVT-AV1 (CRF sweep)
at offline "ceiling" settings, and vs x264 zerolatency and VP9 realtime as the
low-latency references. All scored by the same ffmpeg PSNR/SSIM filters on
decoded output, rate taken from the raw payload (no container overhead).
Audio: Aura (lossless) vs FLAC, WavPack, ALAC and an LPC-residual entropy
estimate; Cinder (lossy) vs Opus and AAC-LC against a Gaussian rate-distortion
bound. Speech: Cinder and Opus vs Codec 2 and Reed on narrowband PESQ.
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
    "x264-zerolatency": (["-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-crf", "{v}"], [20, 26, 32, 38, 44], "h264"),
    "vp9-realtime": (["-c:v", "libvpx-vp9", "-deadline", "realtime", "-cpu-used", "8", "-b:v", "0", "-crf", "{v}"], [20, 26, 32, 38, 44], "ivf"),
}
# Offline references set the quality ceiling; real-time references are the
# low-latency settings a real-time codec like Thrum competes with.
CEILING_REFS = ["x264-veryslow", "x265-veryslow", "svtav1-p4"]
REALTIME_REFS = ["x264-zerolatency", "vp9-realtime"]
LOSSLESS_REFS = {  # name -> (ffmpeg encoder args, file ext). ALAC is muxed to m4a; its container is a few kB.
    "flac-8": (["-c:a", "flac", "-compression_level", "8"], "flac"),
    "wavpack": (["-c:a", "wavpack", "-compression_level", "3"], "wv"),
    "alac": (["-c:a", "alac", "-f", "ipod"], "m4a"),
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


MAX_LAG = 2048  # codec delay search, +/- samples (AAC's priming delay is 1024)


def best_lag(ref, test):
    """Integer offset d with test[i + d] ~ ref[i], by FFT cross-correlation of a
    ~3 s excerpt from the middle of the clip. Codecs add a fixed delay that
    decoders do not always trim (AAC in ADTS does not), which would otherwise
    read as noise and understate SNR."""
    n = min(len(ref), len(test))
    seg_len = min(131072, n // 2)
    start = (n - seg_len) // 2
    seg = ref[start:start + seg_len]
    lo = max(0, start - MAX_LAG)
    hi = min(n, start + seg_len + MAX_LAG)
    t = test[lo:hi]
    size = 1 << (len(t) + len(seg)).bit_length()
    corr = np.fft.irfft(np.fft.rfft(t, size) * np.conj(np.fft.rfft(seg, size)), size)
    k = int(np.argmax(corr[:len(t) - len(seg) + 1]))
    return (lo + k) - start


def snr_db(ref, test):
    """SNR after aligning `test` to `ref` by the best integer lag."""
    d = best_lag(ref, test)
    n = min(len(ref), len(test))
    if d >= 0:
        r, t = ref[:n - d], test[d:n]
    else:
        r, t = ref[-d:n], test[:n + d]
    err = ((r - t) ** 2).sum()
    return 10 * math.log10((r ** 2).sum() / max(err, 1e-12))


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


AAC_KBPS = [16, 24, 32, 48, 64, 96, 128]


def aac_points(wav, tmp):
    """AAC-LC (ffmpeg's native encoder) as the lossy reference beside Opus.
    Mono 48 kHz, raw ADTS so the rate is the AAC stream itself."""
    ref = wav_samples(wav)
    seconds = len(ref) / 48000
    pts = []
    for k in AAC_KBPS:
        out, dec = tmp / "a.aac", tmp / "a.wav"
        ff("-i", wav, "-c:a", "aac", "-b:a", f"{k}k", "-ac", 1, "-ar", 48000, "-f", "adts", out)
        ff("-i", out, "-ac", 1, "-ar", 48000, "-c:a", "pcm_s16le", dec)
        pts.append(dict(setting=k, kbps=kbps(out.stat().st_size, seconds), snr_db=snr_db(ref, wav_samples(dec))))
        print(f"   aac {k}k: {pts[-1]['kbps']:.1f} kbps, {pts[-1]['snr_db']:.2f} dB", flush=True)
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
            for lname, (args, ext) in LOSSLESS_REFS.items():
                f = tmp / f"o.{ext}"
                t0 = time.perf_counter()
                ff("-i", wav, *args, f)
                res[lname] = dict(bytes=f.stat().st_size, encode_x_realtime=AUDIO_SECONDS / (time.perf_counter() - t0))
            pcm = np.frombuffer(wav.read_bytes()[44:], dtype="<i2")
            res["lpc_estimate"] = dict(bytes=int(lpc_entropy_bits(pcm) / 8))
            res["cinder"] = cinder_sweep(wav, tmp)
            res["opus"] = opus_points(wav, tmp)
            res["aac"] = aac_points(wav, tmp)
            res["rd_bound"] = rd_bound(wav)
            out[name] = res
            print("  rd bound:", [(p["snr_db"], round(p["kbps"])) for p in res["rd_bound"][::2]], flush=True)
    return out


# --------------------------------------------------------------------------
# speech (perceptual): narrowband PESQ (ITU-T P.862) on Open Speech Repository clips
# --------------------------------------------------------------------------
SPEECH_CLIPS = ["OSR_us_000_0010_8k", "OSR_us_000_0011_8k"]
SPEECH_SECONDS = 20
PESQ_TARGETS = [2.0, 2.5, 3.0, 3.5, 4.0]


def fetch_speech():
    import urllib.request
    (CORPUS / "speech").mkdir(parents=True, exist_ok=True)
    (CORPUS / "speech48").mkdir(exist_ok=True)
    for clip in SPEECH_CLIPS:
        src = CORPUS / "speech" / f"{clip}.wav"
        if not src.exists():
            print("download", clip)
            urllib.request.urlretrieve(f"https://www.voiptroubleshooter.com/open_speech/american/{clip}.wav", src)
        dst = CORPUS / "speech48" / f"{clip}_48k.wav"
        if not dst.exists():
            ff("-i", src, "-ar", 48000, "-ac", 1, "-c:a", "pcm_s16le", "-t", SPEECH_SECONDS, dst)


def to_8k(path, tmp):
    """Narrowband view of a clip, through the same resampler for reference and decoded output."""
    out = tmp / "n8.wav"
    ff("-i", path, "-ar", 8000, "-ac", 1, "-c:a", "pcm_s16le", out)
    return wav_samples(out) / 32768.0


def pesq_nb(ref8, deg8):
    from pesq import pesq
    return float(pesq(8000, ref8.astype(np.float32), deg8.astype(np.float32), "nb"))


CODEC2_MODES = ["3200", "2400", "1600", "1200", "700C"]  # libcodec2 modes, 8 kHz only
REED_QUALITY = [0, 25, 50, 75, 100]  # Reed's quality maps to LSF bits; its rate moves with it


def codec2_points(ref8_wav, ref8, seconds, tmp):
    pts = []
    for mode in CODEC2_MODES:
        raw, dec = tmp / "c2.c2", tmp / "c2.wav"
        ff("-i", ref8_wav, "-c:a", "libcodec2", "-mode", mode, "-f", "codec2raw", raw)
        ff("-f", "codec2raw", "-mode", mode, "-i", raw, "-ar", 8000, "-ac", 1, "-c:a", "pcm_s16le", dec)
        pts.append(dict(codec="codec2", setting=f"{mode} bit/s", kbps=kbps(raw.stat().st_size, seconds),
                        pesq=pesq_nb(ref8, to_8k(dec, tmp))))
        print(f"   codec2 {mode}: {pts[-1]['kbps']:.2f} kbps, PESQ {pts[-1]['pesq']:.2f}", flush=True)
    return pts


def reed_points(ref8_wav, ref8, tmp):
    pts = []
    dec, enc = tmp / "r.wav", tmp / "r.tpt"
    for q in REED_QUALITY:
        j = json.loads(sh([TPT, "benchmark", "--codec", "reed", "--runs", 1, "--quality", q, "--json", ref8_wav]).stdout)
        sh([TPT, "encode", "--codec", "reed", "--quality", q, ref8_wav, enc])
        sh([TPT, "decode", "--codec", "reed", "--container", "wav", enc, dec])
        pts.append(dict(codec="reed", setting=f"quality {q}", kbps=j["bitrate_bits_per_second"] / 1000,
                        pesq=pesq_nb(ref8, to_8k(dec, tmp))))
        print(f"   reed q{q}: {pts[-1]['kbps']:.2f} kbps, PESQ {pts[-1]['pesq']:.2f}", flush=True)
    return pts


def speech_points(ref48, tmp):
    from pesq import pesq  # noqa: F401  (fail early if missing)
    ref8 = to_8k(ref48, tmp)
    ref8_wav = tmp / "ref8.wav"
    ff("-i", ref48, "-ar", 8000, "-ac", 1, "-c:a", "pcm_s16le", ref8_wav)
    seconds = len(wav_samples(ref48)) / 48000
    pts = codec2_points(ref8_wav, ref8, seconds, tmp) + reed_points(ref8_wav, ref8, tmp)
    dec = tmp / "d48.wav"
    for bits, q in zip(CINDER_BITS, CINDER_QUALITY):
        for step in CINDER_STEPS[::2]:  # octave grid keeps the sweep affordable
            j = json.loads(sh([TPT, "benchmark", "--codec", "cinder", "--runs", 1, "--quality", q,
                               "--band-step", f"{step:.6f}", "--json", ref48]).stdout)
            out = tmp / "c.tpt"
            sh([TPT, "encode", "--codec", "cinder", "--quality", q, "--band-step", f"{step:.6f}", ref48, out])
            sh([TPT, "decode", "--codec", "cinder", "--container", "wav", out, dec])
            pts.append(dict(codec="cinder", setting=f"{bits} bits, step {step:.4f}",
                            kbps=j["bitrate_bits_per_second"] / 1000, pesq=pesq_nb(ref8, to_8k(dec, tmp))))
    for k in OPUS_KBPS:
        opus, dec2 = tmp / "o.opus", tmp / "o48.wav"
        ff("-i", ref48, "-c:a", "libopus", "-b:a", f"{k}k", "-ac", 1, "-ar", 48000, opus)
        ff("-i", opus, "-ac", 1, "-ar", 48000, "-c:a", "pcm_s16le", dec2)
        pts.append(dict(codec="opus", setting=f"{k} kbps", kbps=kbps(opus.stat().st_size, seconds),
                        pesq=pesq_nb(ref8, to_8k(dec2, tmp))))
    print("   ceiling check (reference vs itself):", round(pesq_nb(ref8, ref8), 2), flush=True)
    return pts


def measure_speech():
    fetch_speech()
    speech = {}
    with tempfile.TemporaryDirectory() as t:
        tmp = Path(t)
        for clip in SPEECH_CLIPS:
            print("speech", clip, flush=True)
            speech[clip] = speech_points(CORPUS / "speech48" / f"{clip}_48k.wav", tmp)
    return speech


def result_files():
    """Result files oldest first, by the date recorded in each (filenames sort by commit, not time)."""
    return sorted(RESULTS.glob("*.json"), key=lambda f: json.loads(f.read_text())["meta"]["date"])


def git(*a):
    return subprocess.run(["git", *a], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def run_proxy_only():
    """Add the entropy-coding proxy to the latest run without re-measuring anything else."""
    f = result_files()[-1]
    data = json.loads(f.read_text())
    for name in data["audio"]:
        data["audio"][name]["entropy_proxy"] = entropy_proxy(CORPUS / "audio" / f"{name}_{AUDIO_SECONDS}s.wav")
    f.write_text(json.dumps(data, indent=1))
    print("updated", f)


def remeasure(sections):
    """Re-measure the named sections ("video", "audio", "speech") and keep the
    rest of the latest run unchanged. The result is a new file, dated now."""
    latest = json.loads(result_files()[-1].read_text())
    sh(["cargo", "build", "--release", "-p", "tpt-cli"], cwd=ROOT)
    meta = dict(latest["meta"], date=datetime.datetime.now().isoformat(timespec="seconds"),
                commit=git("rev-parse", "--short", "HEAD"),
                dirty=bool(git("status", "--porcelain", "--untracked-files=no")))
    measure = dict(video=run_video, audio=run_audio, speech=measure_speech)
    data = dict(meta=meta)
    for key in ("video", "audio", "speech"):
        if key in sections:
            data[key] = measure[key]()
        elif key in latest:
            data[key] = latest[key]
    f = RESULTS / f"{meta['date'][:10]}-{meta['commit']}{'-dirty' if meta['dirty'] else ''}.json"
    f.write_text(json.dumps(data, indent=1))
    print("wrote", f)


def run():
    sh(["cargo", "build", "--release", "-p", "tpt-cli"], cwd=ROOT)
    RESULTS.mkdir(parents=True, exist_ok=True)
    meta = dict(date=datetime.datetime.now().isoformat(timespec="seconds"), commit=git("rev-parse", "--short", "HEAD"),
                dirty=bool(git("status", "--porcelain", "--untracked-files=no")), frames=FRAMES,
                ffmpeg=sh(["ffmpeg", "-version"]).stdout.splitlines()[0])
    data = dict(meta=meta, video=run_video(), audio=run_audio(), speech=measure_speech())
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


def fmt_x(v):
    return "n/a" if v is None else f"{v:.2f}×"


def fmt_kbps(v, digits=0):
    return "n/a" if v is None else f"{v:.{digits}f}"


def ratio_of(v, b):
    return fmt_x(None if v is None or not b else v / b)


def bound_at(r, t):
    return next((p["kbps"] for p in r["rd_bound"] if p["snr_db"] >= t), None)


def equal_quality(res, refs, p):
    """tpt's rate over the cheapest reference in `refs` at the same luma PSNR, or None."""
    best = [b for b in (interp_kbps(res[x], p["psnr_y"]) for x in refs if x in res) if b]
    return p["kbps"] / min(best) if best else None


def video_ratios(cur, codec, refs):
    return [v for res in cur["video"].values() for p in res[codec]
            for v in [equal_quality(res, refs, p)] if v is not None]


def lossless_ratio(r):
    best = min((r[k]["bytes"] for k in ("flac-8", "wavpack", "alac") if k in r), default=None)
    return r["aura"]["bytes"] / best if best else None


def lossy_ratios(cur, key, t=20):
    vals = []
    for r in cur["audio"].values():
        if key in r:
            c, b = rate_for_snr(r[key], t), bound_at(r, t)
            if c is not None and b:
                vals.append(c / b)
    return vals


def speech_rate(speech, codec, target):
    """Lowest kbps reaching `target` PESQ on each clip, averaged over the clips;
    None if any clip never reaches it."""
    per_clip = []
    for clip in speech.values():
        vals = [p["kbps"] for p in clip if p["codec"] == codec and p["pesq"] >= target]
        if not vals:
            return None
        per_clip.append(min(vals))
    return sum(per_clip) / len(per_clip)


def summary_rows(cur):
    rows = []
    for codec in ("thrum", "vault"):
        for label, refs in (("real-time references", REALTIME_REFS), ("offline ceiling", CEILING_REFS)):
            vals = video_ratios(cur, codec, refs)
            value = fmt_x(float(np.median(vals))) if vals else "n/a"
            rows.append((f"{codec.title()}, size at equal PSNR vs {label} (median)", value))
    vals = [v for v in (lossless_ratio(r) for r in cur["audio"].values()) if v is not None]
    rows.append(("Aura, size vs best lossless reference (mean over clips)", fmt_x(float(np.mean(vals))) if vals else "n/a"))
    for key, label in (("cinder", "Cinder"), ("opus", "Opus"), ("aac", "AAC-LC")):
        vals = lossy_ratios(cur, key)
        rows.append((f"{label}, rate ÷ Gaussian bound at 20 dB (mean over clips)",
                     fmt_x(float(np.mean(vals))) if vals else "n/a"))
    if cur.get("speech"):
        for codec, label in (("opus", "Opus"), ("cinder", "Cinder"), ("codec2", "Codec 2"), ("reed", "Reed")):
            rows.append((f"{label}, lowest kbps at PESQ 3.0 (mean over clips)",
                         fmt_kbps(speech_rate(cur["speech"], codec, 3.0))))
    return rows


def report():
    files = result_files()
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
         "## Summary", "",
         "Each row is one comparison from the tables below. Ratios are how many times larger the codec's stream is "
         "than the reference at the same quality, so `1.00×` is parity and lower is better.", "",
         "| measure | result |", "| --- | ---: |"]
    L += [f"| {label} | {value} |" for label, value in summary_rows(cur)]
    L += ["",
         "## Method", "",
         "- **Corpus:** Xiph.org derf CIF sequences (akiyo = low motion, foreman = medium, mobile = high), first "
         f"{m['frames']} frames, 4:2:0. Audio is 30 s mono 48 kHz excerpts of the Sintel trailer audio and Elephants Dream's "
         "music (ED-CM stereo, downmixed).",
         "- **Video metric:** every codec's output is decoded and scored by the *same* ffmpeg PSNR and SSIM filters "
         "against the source. Rate is the raw payload, with no container overhead on either side. Each codec is swept over "
         "its quality setting; a single point means little, the curve is the result.",
         "- **Video references:** the *offline ceiling* is the best of x264 `veryslow`, x265 `veryslow` and SVT-AV1 preset 4 "
         "at default tuning. The *real-time references* are x264 `veryfast` with `zerolatency` and libvpx VP9 `realtime` at "
         "`cpu-used 8`, the settings low-latency encoders run with. Thrum is a low-latency codec, so the real-time group is the "
         "fair comparison; the offline ceiling shows the distance to the best encoders.",
         "- **Video equal-quality columns:** how many times *larger* the tpt stream is than the cheapest reference in that group "
         "at the same luma PSNR (log-linear interpolation). `>1` means tpt is worse; `n/a` means the PSNR is outside the "
         "reference's range.",
         "- **Audio, lossless:** Aura is compared by size against FLAC (compression level 8), WavPack (level 3) and ALAC "
         "(muxed into m4a, so a few kB of container is included), and against an *estimate* of an ideal predictor "
         "(order-32 LPC per 4096-sample block, ideal Laplace entropy coder, side info ignored). The estimate is not a proven bound.",
         "- **Audio, lossy:** Cinder is swept over bit depth (4–12, via `--quality`) and per-band step (`--band-step`, "
         "half-octave grid from 1/1024 to about 45). Rate is the codec's payload, not the file size. The Cinder rows use the "
         "lowest rate that reaches each SNR over the whole grid, which is an oracle choice: a shipped encoder would need rate "
         "control to find the step. Cinder v5 range-codes its levels under one of 16 fixed geometric models chosen per packet "
         "(a 4-bit class), so it no longer pays a per-coefficient floor. The entropy proxy is the remaining headroom: "
         "it uses ideal per-band histograms fitted to each clip.",
         "- **Audio, lossy references:** Opus (libopus via ffmpeg, mono 48 kHz, 12–128 kbps) and AAC-LC (ffmpeg's native "
         "encoder, mono 48 kHz, raw ADTS, 16–128 kbps). Both are perceptually tuned and SNR is a waveform metric, so they are "
         "expected to look worse here than they sound; read the SNR columns as a waveform-fidelity comparison, not a listening test.",
         "- **SNR alignment:** every decoded signal is aligned to its source by the best integer lag within ±2048 samples "
         "before SNR is taken, so a codec's fixed delay does not count as error.",
         "- **Audio, bound:** the Gaussian rate-distortion bound from reverse water-filling over 16 bands per 256-sample DCT block, "
         "each band's variance taken as its power in that block, plus 4 bits per band per block of side information (about 12 kbps). "
         "It is an estimate for that envelope model, not a proven bound on every source. Earlier runs used per-coefficient "
         "variances with no side information; those numbers were far too optimistic and are not comparable.",
         "- **Speech:** ITU-T P.862 narrowband PESQ on two 20 s Open Speech Repository clips, both sides resampled to 8 kHz. "
         "Cinder and Opus run at 48 kHz on the original clips. Codec 2 (libcodec2, five modes) and Reed (a quality sweep) run "
         "at 8 kHz, their native rate. Each cell is the lowest rate that reaches that PESQ on each clip, averaged over the clips. "
         "PESQ is a speech metric, so it says nothing about music.",
         "- **Cinder's wire step:** version 3 carries steps in 1/1024 units. Version 2 wrote whole numbers only, which made every "
         "step below 1.0 decode to near silence; the rows below post-date that fix.",
         "- **Cinder's loudest peaks clip at fine steps.** At 12 bits the range is about ±2048 × step, so a step of 0.001 clips "
         "coefficients above about 2. The sweep includes those points; the SNR column shows the cost.", ""]

    L += ["## Video", ""]
    for clip, res in cur["video"].items():
        L += [f"### {clip}", "", "| codec | setting | kbps | PSNR-Y dB | SSIM | enc ×realtime | vs offline ceiling | vs real-time reference |",
              "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
        for name, pts in res.items():
            for p in pts:
                ceil = real = ""
                if name not in REFERENCES:
                    ceil = fmt_x(equal_quality(res, CEILING_REFS, p))
                    real = fmt_x(equal_quality(res, REALTIME_REFS, p))
                L.append(f"| {name} | {p['setting']} | {p['kbps']:.0f} | {p['psnr_y']:.2f} | {p['ssim']:.4f} | "
                         f"{p['encode_x_realtime']:.2f} | {ceil} | {real} |")
        L.append("")

    k = lambda b: "n/a" if b is None else f"{b / 1000:.0f} kB"
    L += ["## Audio, lossless", "", "| clip | PCM | Aura | FLAC-8 | WavPack | ALAC | LPC estimate | Aura ÷ best reference | Aura bit-exact |",
          "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: |"]
    for name, r in cur["audio"].items():
        L.append(f"| {name} | {k(r['pcm_bytes'])} | {k(r['aura']['bytes'])} ({r['aura']['bytes'] / r['pcm_bytes']:.1%}) | "
                 f"{k(r.get('flac-8', {}).get('bytes'))} | {k(r.get('wavpack', {}).get('bytes'))} | "
                 f"{k(r.get('alac', {}).get('bytes'))} | {k(r['lpc_estimate']['bytes'])} | "
                 f"{fmt_x(lossless_ratio(r))} | {'yes' if r['aura']['bit_exact'] else '**NO**'} |")
    L.append("")

    L += ["## Audio, lossy: rate needed for a given SNR", "",
          "Lowest kbps that reaches each SNR (oracle choice over each codec's own sweep). Ratios are against the Gaussian bound. "
          "The entropy-coding proxy is not a codec: it is what entropy coding Cinder's levels could reach, with oracle histograms "
          "and a DCT in place of the MDCT (see Method).", ""]
    for name, r in cur["audio"].items():
        if "cinder" not in r:
            continue
        L += [f"### {name}", "", "| SNR (dB) | Cinder kbps | Opus kbps | AAC-LC kbps | Gaussian bound kbps | Entropy-coding proxy kbps | Cinder ÷ bound | Opus ÷ bound | AAC-LC ÷ bound |",
              "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
        proxy = {p["snr_db"]: p["kbps"] for p in r.get("entropy_proxy", [])}
        for t in SNR_TARGETS:
            c, o, a = (rate_for_snr(r.get(key, []), t) for key in ("cinder", "opus", "aac"))
            b = bound_at(r, t)
            L.append(f"| {t} | {fmt_kbps(c)} | {fmt_kbps(o)} | {fmt_kbps(a)} | {fmt_kbps(b)} | {fmt_kbps(proxy.get(t))} | "
                     f"{ratio_of(c, b)} | {ratio_of(o, b)} | {ratio_of(a, b)} |")
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
        if r.get("aac"):
            L += ["AAC-LC points:", "", "| bitrate | kbps | SNR dB |", "| ---: | ---: | ---: |"]
            for p in r["aac"]:
                L.append(f"| {p['setting']} | {p['kbps']:.1f} | {p['snr_db']:.2f} |")
            L.append("")

    if cur.get("speech"):
        sp = cur["speech"]
        L += ["## Speech, perceptual: narrowband PESQ", "",
              "ITU-T P.862 PESQ (MOS-LQO, 1 to 4.5) on two Open Speech Repository clips, both sides at 8 kHz. Each cell is the "
              "lowest rate that reaches that PESQ on each clip, averaged over the clips (n/a if a clip never reaches it). "
              "Codec 2 and Reed are narrowband codecs and run only at 8 kHz. The reference scores 4.5 against itself.", "",
              "| PESQ | Opus kbps | Cinder kbps | Codec 2 kbps | Reed kbps | Cinder ÷ Opus |", "| ---: | ---: | ---: | ---: | ---: | ---: |"]
        for t in PESQ_TARGETS:
            rates = {c: speech_rate(sp, c, t) for c in ("opus", "cinder", "codec2", "reed")}
            L.append(f"| {t} | {fmt_kbps(rates['opus'])} | {fmt_kbps(rates['cinder'])} | {fmt_kbps(rates['codec2'], 1)} | "
                     f"{fmt_kbps(rates['reed'], 1)} | {ratio_of(rates['cinder'], rates['opus'])} |")
        L.append("")
        L += ["Codec 2 and Reed points (mean over the clips):", "", "| codec | setting | kbps | PESQ |", "| --- | --- | ---: | ---: |"]
        for codec in ("codec2", "reed"):
            settings = [p["setting"] for p in next(iter(sp.values())) if p["codec"] == codec]
            for s in settings:
                pts = [p for clip in sp.values() for p in clip if p["codec"] == codec and p["setting"] == s]
                L.append(f"| {codec} | {s} | {np.mean([p['kbps'] for p in pts]):.2f} | {np.mean([p['pesq'] for p in pts]):.2f} |")
        L.append("")

    L += ["## Progress", "", "One row per recorded run, all recomputed with the current definitions. Video: each tpt codec's size "
          "relative to the offline ceiling at the same luma PSNR (median over the points where it is defined). Audio: Aura vs "
          "FLAC-8 (lossless); Cinder, Opus and AAC-LC vs the Gaussian bound at 20 dB (lossy; n/a where that codec was not measured).", "",
          "| date | commit | Thrum (×ceiling) | Vault (×ceiling) | Aura/FLAC-8 | Cinder @20 dB ÷ bound | Opus @20 dB ÷ bound | AAC-LC @20 dB ÷ bound |",
          "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for r in runs:
        cells = []
        for codec in ("thrum", "vault"):
            vals = [v for res in r["video"].values() for p in res[codec]
                    for v in [equal_quality(res, CEILING_REFS, p)] if v is not None]
            cells.append(f"{np.median(vals):.1f}×" if vals else "n/a")
        a = np.mean([v["aura"]["bytes"] / v["flac-8"]["bytes"] for v in r["audio"].values()])
        lossy = []
        for key in ("cinder", "opus", "aac"):
            vals = lossy_ratios({"audio": r["audio"]}, key)
            lossy.append(f"{np.mean(vals):.2f}×" if vals else "n/a")
        L.append(f"| {r['meta']['date'][:10]} | `{r['meta']['commit']}`{'*' if r['meta']['dirty'] else ''} | "
                 f"{cells[0]} | {cells[1]} | {a:.2f}× | {lossy[0]} | {lossy[1]} | {lossy[2]} |")
    L += ["", "`*` = run on a tree with uncommitted changes.", ""]
    REPORT.write_text("\n".join(L), encoding="utf-8")
    print("wrote", REPORT)


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "all"
    if cmd == "fetch":
        fetch()
    elif cmd == "run":
        fetch(); run()
    elif cmd == "speech":
        remeasure(("speech",)); report()
    elif cmd == "proxy":
        run_proxy_only(); report()
    elif cmd == "audio":
        fetch(); remeasure(("audio",)); report()
    elif cmd == "video":
        fetch(); remeasure(("video",)); report()
    elif cmd == "report":
        report()
    elif cmd == "all":
        fetch(); run(); report()
    else:
        sys.exit(__doc__)
