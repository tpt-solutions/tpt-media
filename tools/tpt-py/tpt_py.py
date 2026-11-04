"""tpt_py: an independent, container-level reader for the TPT (.tpt) format.

Written from docs/container-design.md only. Parses the file header, TRAK,
TMTA, PKTS (0.2 packet runs), CLUS (0.1), IDX and CRC1 chunks, skips unknown
chunks, verifies CRCs, and
enumerates tracks and packets. It does not decode any codec payload.

Usage:
    python -I tools/tpt-py/tpt_py.py probe [--json] <file.tpt>

Exit codes: 0 ok, 1 malformed input, 2 checksum mismatch, 64 usage error.

JSON output (probe --json), all keys always present:
    reader, reader_version      "tpt_py", "0.1"
    file_size_bytes             int
    file_header                 {version: {major, minor}, header_size,
                                 timebase: {num, den}}
    tracks                      list of track objects:
        id, media_type          int id; media_type is "audio" | "video" |
                                "subtitle" | "metadata", or null if unknown
        media_type_code         raw u8
        codec_id                4-character string (non-printable bytes escaped)
        codec_version           {major, minor}
        codec_private_size      int (bytes; the content is not returned)
        timebase                effective {num, den} (global if track's is 0/0)
        timebase_is_global      bool
        sample_rate_hz, channel_layout (name or null), channel_layout_code,
        channel_mask, width, height, pixel_format (int or null for 0xFF)
        name, language          string or null
        metadata                list of {key, type, value}; type is "text" |
                                "int" | "bool" | "bytes"; bytes are hex text
        packet_count, keyframe_count, first_pts, last_pts (track ticks, or null)
        packets                 list of {pts, dts, size, offset, keyframe,
                                end_of_frame, discontinuity, duration (or null),
                                checksum (hex or null), checksum_ok (bool or null)}
                                offset is the byte offset of the CLUS chunk header
    skipped_chunks              list of {id, offset, size, reason}
                                reason is "reserved" (first byte < 0x20) or "unknown"
    index                       {present, valid, version, consistent}
                                consistent is null when there is no usable index
    checksums                   {crc1: {present, stored, computed, ok},
                                 clusters_with_checksum, cluster_failures: [offsets]}
    warnings                    list of strings (non-fatal oddities)

Errors (exit 1) are raised as TptError subclasses: BadMagic, UnsupportedVersion,
BadHeader, Truncated, BadChunk.
"""

import argparse
import json
import struct
import sys
import zlib

READER_NAME = "tpt_py"
READER_VERSION = "0.2"

FILE_MAGIC = b"TPTM"
FILE_HEADER_MIN = 16
CHUNK_HEADER_LEN = 8
CRC_LEN = 4
INDEX_ENTRY_LEN = 17
CRC_BLOCK = 1 << 20

CH_TRAK = b"TRAK"
CH_TMTA = b"TMTA"
CH_CLUS = b"CLUS"
CH_PKTS = b"PKTS"
CH_IDX = b"IDX "
CH_CRC1 = b"CRC1"

MEDIA_TYPES = {0: "audio", 1: "video", 2: "subtitle", 3: "metadata"}
CHANNEL_LAYOUTS = {0: "none", 1: "mono", 2: "stereo", 3: "5.1", 4: "custom"}
VALUE_TYPES = {0: "text", 1: "int", 2: "bool", 3: "bytes"}

CLUS_KEYFRAME = 1 << 0
CLUS_END_OF_FRAME = 1 << 1
CLUS_DISCONTINUITY = 1 << 2
CLUS_HAS_DURATION = 1 << 3
CLUS_HAS_CHECKSUM = 1 << 4


class TptError(Exception):
    """Base class for all malformed-input errors."""


class BadMagic(TptError):
    pass


class UnsupportedVersion(TptError):
    pass


class BadHeader(TptError):
    pass


class Truncated(TptError):
    """A length field runs past the end of the file."""


class BadChunk(TptError):
    """A chunk body is internally inconsistent."""


def _ascii_id(raw):
    return "".join(chr(b) if 0x20 <= b < 0x7F else "\\x%02x" % b for b in raw)


class _Body:
    """Bounds-checked little-endian reader over one chunk body."""

    def __init__(self, data, chunk_name):
        self.data = data
        self.pos = 0
        self.name = chunk_name

    def take(self, n, what="field"):
        if n > len(self.data) - self.pos:
            raise BadChunk("%s: %s needs %d bytes at body offset %d, body has %d"
                           % (self.name, what, n, self.pos, len(self.data)))
        out = self.data[self.pos:self.pos + n]
        self.pos += n
        return out

    def u8(self, what="u8"):
        return self.take(1, what)[0]

    def u16(self, what="u16"):
        return struct.unpack("<H", self.take(2, what))[0]

    def u32(self, what="u32"):
        return struct.unpack("<I", self.take(4, what))[0]

    def u64(self, what="u64"):
        return struct.unpack("<Q", self.take(8, what))[0]

    def i64(self, what="i64"):
        return struct.unpack("<q", self.take(8, what))[0]

    def remaining(self):
        return len(self.data) - self.pos


def _utf8(raw, what=None):
    """Decode UTF-8. Invalid text reads as "" rather than failing the open,
    as the reference reader does (containers/tpt-container, "Reader Rules")."""
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return ""


def _tb(num, den):
    return {"num": num, "den": den}


def _parse_header(fp, size):
    if size < FILE_HEADER_MIN:
        raise Truncated("file is %d bytes; the file header needs at least %d"
                        % (size, FILE_HEADER_MIN))
    fp.seek(0)
    raw = fp.read(FILE_HEADER_MIN)
    if raw[0:4] != FILE_MAGIC:
        raise BadMagic("bad magic %r, expected b'TPTM'" % raw[0:4])
    version, header_size = struct.unpack_from("<HH", raw, 4)
    tb_num, tb_den = struct.unpack_from("<II", raw, 8)
    major, minor = version >> 8, version & 0xFF
    # Reader is 0.2: accept major 0, minors 1 and 2 (version 0x0000 is rejected
    # explicitly by the spec, so an "earlier minor" can never be accepted).
    if version == 0 or major != 0 or minor > 2:
        raise UnsupportedVersion("file version %d.%d is not readable by %s %s"
                                 % (major, minor, READER_NAME, READER_VERSION))
    if header_size < FILE_HEADER_MIN:
        raise BadHeader("header_size %d is below the minimum of %d"
                        % (header_size, FILE_HEADER_MIN))
    # 16 is the version 0.1 header. 17 to 19 would start chunk parsing inside
    # the reserved field, so the spec refuses them.
    if header_size != FILE_HEADER_MIN and header_size < 20:
        raise BadHeader("header_size %d is inside the reserved field; use 16 or >= 20"
                        % header_size)
    if header_size > size:
        raise Truncated("header_size %d exceeds file size %d" % (header_size, size))
    if tb_num == 0 or tb_den == 0:
        raise BadHeader("global timebase %d/%d has a zero term" % (tb_num, tb_den))
    return {
        "version": {"major": major, "minor": minor},
        "header_size": header_size,
        "timebase": _tb(tb_num, tb_den),
    }


def _parse_trak(data, warnings):
    b = _Body(data, "TRAK")
    track_id = b.u8("track_id")
    media = b.u8("media_type")
    if media not in MEDIA_TYPES:
        raise BadChunk("TRAK track %d: unknown media_type %d" % (track_id, media))
    codec = b.take(4, "codec_id")
    cver = b.u16("codec_version")
    cpl = b.u16("codec_priv_len")
    b.take(cpl, "codec_priv")
    tb_num = b.u32("track_timebase_num")
    tb_den = b.u32("track_timebase_den")
    sample_rate = b.u32("sample_rate")
    layout = b.u8("channel_layout")
    mask = b.u16("channel_mask")
    width = b.u32("width")
    height = b.u32("height")
    pix = b.u8("pixel_format")
    flags = b.u8("flags")
    name = lang = None
    if flags & 1:
        n = b.u8("name_len")
        name = _utf8(b.take(n, "name"))
    if flags & 2:
        n = b.u8("lang_len")
        lang = _utf8(b.take(n, "language"))
    if b.remaining():
        warnings.append("TRAK for track %d has %d trailing bytes (ignored)"
                        % (track_id, b.remaining()))
    if (tb_num == 0) != (tb_den == 0):
        warnings.append("TRAK track %d timebase %d/%d has one zero term; "
                        "the global timebase is used" % (track_id, tb_num, tb_den))
        tb_num = tb_den = 0
    return {
        "id": track_id,
        "media_type": MEDIA_TYPES[media],
        "media_type_code": media,
        "codec_id": _ascii_id(codec),
        "codec_version": {"major": cver >> 8, "minor": cver & 0xFF},
        "codec_private_size": cpl,
        "track_timebase": None if (tb_num, tb_den) == (0, 0) else _tb(tb_num, tb_den),
        "sample_rate_hz": sample_rate,
        "channel_layout": CHANNEL_LAYOUTS.get(layout),
        "channel_layout_code": layout,
        "channel_mask": mask,
        "width": width,
        "height": height,
        "pixel_format": None if pix == 0xFF else pix,
        "name": name,
        "language": lang,
        "metadata": [],
        "packets": [],
    }


def _placeholder_track(track_id):
    """A track with no TRAK. Its packets are still listed, as the reference
    reader yields them; every descriptive field is unknown."""
    return {
        "id": track_id, "media_type": None, "media_type_code": None,
        "codec_id": None, "codec_version": None, "codec_private_size": None,
        "track_timebase": None, "sample_rate_hz": None, "channel_layout": None,
        "channel_layout_code": None, "channel_mask": None, "width": None,
        "height": None, "pixel_format": None, "name": None, "language": None,
        "metadata": [], "packets": [],
    }


def _parse_tmta(data, warnings):
    b = _Body(data, "TMTA")
    track_id = b.u8("track_id")
    count = b.u16("count")
    entries = []
    for _ in range(count):
        key = _utf8(b.take(b.u8("key_len"), "key"))
        vtype = b.u8("value_type")
        if vtype == 0:
            n = b.u16("text_len")
            value = _utf8(b.take(n, "text value"))
        elif vtype == 1:
            value = b.i64("int value")
        elif vtype == 2:
            # Spec: "1 byte". Any non-zero value is read as true.
            value = b.u8("bool value") != 0
        elif vtype == 3:
            n = b.u32("bytes_len")
            value = b.take(n, "bytes value").hex()
        else:
            raise BadChunk("TMTA track %d key %r: unknown value_type %d"
                           % (track_id, key, vtype))
        entries.append({"key": key, "type": VALUE_TYPES[vtype], "value": value})
    if b.remaining():
        warnings.append("TMTA for track %d has %d trailing bytes (ignored)"
                        % (track_id, b.remaining()))
    return track_id, entries


def _crc_of_range(fp, start, length):
    crc = 0
    fp.seek(start)
    left = length
    while left > 0:
        block = fp.read(min(CRC_BLOCK, left))
        if not block:
            raise Truncated("file ended while checksumming")
        crc = zlib.crc32(block, crc)
        left -= len(block)
    return crc & 0xFFFFFFFF


def _parse_clus(fp, pos, body, csize, warnings):
    """Parse a CLUS chunk. Payload is never stored; its CRC is streamed."""
    if csize < 18:
        raise BadChunk("CLUS at offset %d: body is %d bytes, needs at least 18"
                       % (pos, csize))
    fp.seek(body)
    head = fp.read(18)
    pts, dts = struct.unpack_from("<QQ", head, 0)
    flags = head[16]
    track_id = head[17]
    fixed = 18
    duration = None
    if flags & CLUS_HAS_DURATION:
        fixed += 4
    has_ck = bool(flags & CLUS_HAS_CHECKSUM)
    need = fixed + (CRC_LEN if has_ck else 0)
    if csize < need:
        raise BadChunk("CLUS at offset %d: flags 0x%02x need %d body bytes, have %d"
                       % (pos, flags, need, csize))
    if flags & CLUS_HAS_DURATION:
        duration = struct.unpack("<I", fp.read(4))[0]
    checksum = checksum_ok = None
    if has_ck:
        covered = csize - CRC_LEN
        computed = _crc_of_range(fp, body, covered)
        fp.seek(body + covered)
        checksum = struct.unpack("<I", fp.read(CRC_LEN))[0]
        checksum_ok = computed == checksum
    payload_len = csize - fixed - (CRC_LEN if has_ck else 0)
    return track_id, {
        "pts": pts,
        "dts": dts,
        "size": payload_len,
        "offset": pos,
        "slot": 0,
        "keyframe": bool(flags & CLUS_KEYFRAME),
        "end_of_frame": bool(flags & CLUS_END_OF_FRAME),
        "discontinuity": bool(flags & CLUS_DISCONTINUITY),
        "duration": duration,
        "checksum": None if checksum is None else "0x%08x" % checksum,
        "checksum_ok": checksum_ok,
    }


PKTS_MAX_COUNT = 32
PKTS_HEADER_LEN = 20
PKTS_HAS_CHECKSUM = 1 << 0
REC_HAS_DTS = 1 << 4
U64 = (1 << 64) - 1


def _parse_pkts(fp, pos, body, csize, warnings):
    """Parse a PKTS run. Returns (track_id, [packet, ...]), one packet per
    record. The run's CRC, when present, is reported on its first packet."""
    if csize < PKTS_HEADER_LEN:
        raise BadChunk("PKTS at offset %d: body is %d bytes, needs at least %d"
                       % (pos, csize, PKTS_HEADER_LEN))
    fp.seek(body)
    head = fp.read(PKTS_HEADER_LEN)
    track_id = head[0]
    rflags = head[1]
    count = struct.unpack_from("<H", head, 2)[0]
    base_pts, base_dts = struct.unpack_from("<QQ", head, 4)
    if not 1 <= count <= PKTS_MAX_COUNT:
        raise BadChunk("PKTS at offset %d declares %d records (1 to %d allowed)"
                       % (pos, count, PKTS_MAX_COUNT))
    has_ck = bool(rflags & PKTS_HAS_CHECKSUM)
    records_end = csize - (CRC_LEN if has_ck else 0)
    if records_end < PKTS_HEADER_LEN:
        raise BadChunk("PKTS at offset %d is too short for its checksum" % pos)
    checksum = checksum_ok = None
    if has_ck:
        computed = _crc_of_range(fp, body, records_end)
        fp.seek(body + records_end)
        checksum = struct.unpack("<I", fp.read(CRC_LEN))[0]
        checksum_ok = computed == checksum
    fp.seek(body + PKTS_HEADER_LEN)
    rb = fp.read(records_end - PKTS_HEADER_LEN)
    p = 0
    out = []
    for slot in range(count):
        def need(n, what):
            if p + n > len(rb):
                raise BadChunk("PKTS at offset %d: record %d runs past the body (%s)"
                               % (pos, slot, what))
        need(1, "flags")
        flags = rb[p]
        p += 1
        need(4, "pts delta")
        pts_delta = struct.unpack_from("<i", rb, p)[0]
        p += 4
        dts_delta = None
        if flags & REC_HAS_DTS:
            need(4, "dts delta")
            dts_delta = struct.unpack_from("<i", rb, p)[0]
            p += 4
        duration = None
        if flags & CLUS_HAS_DURATION:
            need(4, "duration")
            duration = struct.unpack_from("<I", rb, p)[0]
            p += 4
        need(4, "length")
        length = struct.unpack_from("<I", rb, p)[0]
        p += 4
        need(length, "payload")
        p += length
        pts = (base_pts + pts_delta) & U64
        dts = pts if dts_delta is None else (base_dts + dts_delta) & U64
        first = slot == 0
        out.append({
            "pts": pts,
            "dts": dts,
            "size": length,
            "offset": pos,
            "slot": slot,
            "keyframe": bool(flags & CLUS_KEYFRAME),
            "end_of_frame": bool(flags & CLUS_END_OF_FRAME),
            "discontinuity": bool(flags & CLUS_DISCONTINUITY),
            "duration": duration,
            "checksum": ("0x%08x" % checksum) if (has_ck and first) else None,
            "checksum_ok": checksum_ok if (has_ck and first) else None,
        })
    if p != len(rb):
        raise BadChunk("PKTS at offset %d has %d trailing bytes after its records"
                       % (pos, len(rb) - p))
    return track_id, out


def _parse_idx(data):
    """Returns (version, tracks) where tracks maps track_id -> entry list.

    Raises BadChunk on any structural problem; the caller treats that as a
    corrupt (ignored) index, not a fatal error, per the spec's recovery rule.
    """
    b = _Body(data, "IDX ")
    version = b.u8("index_version")
    count = b.u8("track_count")
    tracks = {}
    for _ in range(count):
        tid = b.u8("track_id")
        n = b.u32("entry_count")
        if n * INDEX_ENTRY_LEN > b.remaining():
            raise BadChunk("IDX track %d declares %d entries but only %d bytes remain"
                           % (tid, n, b.remaining()))
        entries = []
        for _ in range(n):
            pts = b.u64("entry pts")
            off = b.u64("entry offset")
            kf = b.u8("entry keyframe")
            if kf not in (0, 1):
                raise BadChunk("IDX track %d: keyframe byte %d is not 0 or 1" % (tid, kf))
            entries.append({"pts": pts, "offset": off, "keyframe": bool(kf)})
        if tid in tracks:
            raise BadChunk("IDX lists track %d twice" % tid)
        tracks[tid] = entries
    if b.remaining():
        raise BadChunk("IDX has %d trailing bytes" % b.remaining())
    return version, tracks


def parse_stream(fp):
    """Parse a TPT file from a seekable binary file object. Returns a dict
    matching the JSON schema in the module docstring. Raises TptError."""
    fp.seek(0, 2)
    size = fp.tell()
    header = _parse_header(fp, size)
    warnings = []

    tracks = {}           # track_id -> track dict (in TRAK order)
    pending_clus = []     # (track_id, packet)
    skipped = []
    crc1 = None           # {"offset", "stored"}
    index_raw = None      # bytes of the last IDX chunk
    index_count = 0
    index_error = None
    seen_index = False

    pos = header["header_size"]
    while pos < size:
        if size - pos < CHUNK_HEADER_LEN:
            raise Truncated("partial chunk header at offset %d (%d bytes left)"
                            % (pos, size - pos))
        fp.seek(pos)
        hdr = fp.read(CHUNK_HEADER_LEN)
        cid = hdr[0:4]
        csize = struct.unpack("<I", hdr[4:8])[0]
        body = pos + CHUNK_HEADER_LEN
        if csize > size - body:
            raise Truncated("chunk %r at offset %d declares %d body bytes but only %d remain"
                            % (cid, pos, csize, size - body))
        if crc1 is not None and cid != CH_CRC1:
            warnings.append("%r chunk at offset %d follows CRC1" % (cid, pos))

        if cid == CH_CRC1:
            if csize != CRC_LEN:
                warnings.append("CRC1 at offset %d has body size %d, expected 4; ignored"
                                % (pos, csize))
            else:
                if crc1 is not None:
                    warnings.append("second CRC1 at offset %d; the last one is used" % pos)
                fp.seek(body)
                crc1 = {"offset": pos, "stored": struct.unpack("<I", fp.read(4))[0]}
        elif cid in (CH_TRAK, CH_TMTA, CH_IDX):
            fp.seek(body)
            data = fp.read(csize)
            if cid == CH_TRAK:
                t = _parse_trak(data, warnings)
                if t["id"] in tracks:
                    warnings.append("duplicate TRAK for track %d ignored" % t["id"])
                else:
                    tracks[t["id"]] = t
            elif cid == CH_TMTA:
                tmta_id, entries = _parse_tmta(data, warnings)
                if tmta_id in tracks:
                    tracks[tmta_id]["metadata"].extend(entries)
                else:
                    warnings.append("TMTA for undeclared track %d ignored" % tmta_id)
            else:
                seen_index = True
                index_count += 1
                try:
                    index_raw = _parse_idx(data)
                    index_error = None
                except BadChunk as exc:
                    index_raw = None
                    index_error = str(exc)
                    warnings.append("index ignored: %s" % exc)
        elif cid == CH_CLUS:
            tid, pkt = _parse_clus(fp, pos, body, csize, warnings)
            pending_clus.append((tid, pkt))
        elif cid == CH_PKTS:
            tid, pkts = _parse_pkts(fp, pos, body, csize, warnings)
            pending_clus.extend((tid, p) for p in pkts)
        else:
            skipped.append({
                "id": _ascii_id(cid),
                "offset": pos,
                "size": csize,
                "reason": "reserved" if cid[0] < 0x20 else "unknown",
            })
        pos = body + csize

    if index_count > 1:
        warnings.append("%d IDX chunks found; the last one was used" % index_count)

    cluster_failures = []
    clusters_with_ck = 0
    for tid, pkt in pending_clus:
        if tid not in tracks:
            warnings.append("packets for undeclared track %d (no TRAK); listed without "
                            "track metadata" % tid)
            tracks[tid] = _placeholder_track(tid)
        tracks[tid]["packets"].append(pkt)
        if pkt["checksum_ok"] is not None:
            clusters_with_ck += 1
            if not pkt["checksum_ok"]:
                cluster_failures.append(pkt["offset"])

    track_list = []
    for tid in tracks:
        t = tracks[tid]
        pk = t.pop("packets")
        t["timebase_is_global"] = t["track_timebase"] is None
        t["timebase"] = t.pop("track_timebase") or header["timebase"]
        t["packet_count"] = len(pk)
        t["keyframe_count"] = sum(1 for p in pk if p["keyframe"])
        t["first_pts"] = pk[0]["pts"] if pk else None
        t["last_pts"] = pk[-1]["pts"] if pk else None
        t["packets"] = pk
        track_list.append(t)

    # CRC1 covers every byte before the CRC1 chunk header.
    checksums = {"crc1": {"present": False, "stored": None, "computed": None, "ok": None}}
    if crc1 is not None:
        computed = _crc_of_range(fp, 0, crc1["offset"])
        checksums["crc1"] = {
            "present": True,
            "stored": "0x%08x" % crc1["stored"],
            "computed": "0x%08x" % computed,
            "ok": computed == crc1["stored"],
        }
    checksums["clusters_with_checksum"] = clusters_with_ck
    checksums["cluster_failures"] = cluster_failures

    # Index: present/valid/consistent. A corrupt or unsupported index is not
    # fatal; seeking would fall back to a scan.
    index = {"present": seen_index, "valid": False, "version": None, "consistent": None}
    if index_raw is not None:
        version, idx_tracks = index_raw
        index["version"] = version
        if version not in (1, 2):
            warnings.append("index version %d is not 1 or 2; index ignored" % version)
        else:
            index["valid"] = True
            consistent = True
            # One index entry per chunk (a PKTS run or a CLUS): its first
            # packet's pts, and a keyframe flag set if any packet is one.
            chunks = {}   # offset -> [track_id, first_pts, any_keyframe]
            per_track = {}
            for t in track_list:
                for p in t["packets"]:
                    c = chunks.get(p["offset"])
                    if c is None:
                        c = chunks[p["offset"]] = [t["id"], p["pts"], False]
                        per_track[t["id"]] = per_track.get(t["id"], 0) + 1
                    c[2] = c[2] or p["keyframe"]
            for tid, entries in idx_tracks.items():
                if tid not in tracks:
                    warnings.append("index lists undeclared track %d" % tid)
                    consistent = False
                    continue
                if len(entries) != per_track.get(tid, 0):
                    consistent = False
                for e in entries:
                    hit = chunks.get(e["offset"])
                    if (hit is None or hit[0] != tid or hit[1] != e["pts"]
                            or hit[2] != e["keyframe"]):
                        consistent = False
            index["consistent"] = consistent
            if not consistent:
                warnings.append("index does not match the packets found by scanning")
    elif index_error is not None:
        index["valid"] = False

    return {
        "reader": READER_NAME,
        "reader_version": READER_VERSION,
        "file_size_bytes": size,
        "file_header": header,
        "tracks": track_list,
        "skipped_chunks": skipped,
        "index": index,
        "checksums": checksums,
        "warnings": warnings,
    }


def probe_file(path):
    with open(path, "rb") as fp:
        return parse_stream(fp)


def _exit_code(result):
    crc = result["checksums"]["crc1"]
    if (crc["present"] and not crc["ok"]) or result["checksums"]["cluster_failures"]:
        return 2
    return 0


def _format_text(result):
    lines = []
    fh = result["file_header"]
    lines.append("file: %d bytes, TPT %d.%d, header %d bytes, timebase %d/%d"
                 % (result["file_size_bytes"], fh["version"]["major"],
                    fh["version"]["minor"], fh["header_size"],
                    fh["timebase"]["num"], fh["timebase"]["den"]))
    lines.append("tracks: %d" % len(result["tracks"]))
    for t in result["tracks"]:
        lines.append("  track %d: %s %s, %d packets, pts %s..%s, codec-private %d bytes"
                     % (t["id"], t["media_type"] or "unknown(%d)" % t["media_type_code"],
                        t["codec_id"], t["packet_count"], t["first_pts"], t["last_pts"],
                        t["codec_private_size"]))
        for m in t["metadata"]:
            lines.append("    %s (%s) = %s" % (m["key"], m["type"], m["value"]))
    if result["skipped_chunks"]:
        lines.append("skipped chunks: %d" % len(result["skipped_chunks"]))
    crc = result["checksums"]["crc1"]
    if crc["present"]:
        lines.append("CRC1: %s (stored %s, computed %s)"
                     % ("ok" if crc["ok"] else "MISMATCH", crc["stored"], crc["computed"]))
    else:
        lines.append("CRC1: absent")
    lines.append("cluster checksums: %d present, %d failed"
                 % (result["checksums"]["clusters_with_checksum"],
                    len(result["checksums"]["cluster_failures"])))
    idx = result["index"]
    lines.append("index: %s" % ("absent" if not idx["present"] else
                                ("valid" if idx["valid"] else "invalid")
                                + ("" if idx["consistent"] is None else
                                   (", consistent" if idx["consistent"] else ", INCONSISTENT"))))
    for w in result["warnings"]:
        lines.append("warning: " + w)
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="tpt_py", description="TPT container reader")
    sub = parser.add_subparsers(dest="command", required=True)
    probe = sub.add_parser("probe", help="parse a .tpt file and report its structure")
    probe.add_argument("--json", action="store_true", help="emit JSON (see module docstring)")
    probe.add_argument("file", help="path to a .tpt file")
    try:
        args = parser.parse_args(argv)
    except SystemExit:
        return 64
    try:
        result = probe_file(args.file)
    except OSError as exc:
        print("error: cannot read %s: %s" % (args.file, exc), file=sys.stderr)
        return 1
    except TptError as exc:
        if args.json:
            print(json.dumps({"error": {"type": type(exc).__name__, "message": str(exc)}}))
        else:
            print("error: %s: %s" % (type(exc).__name__, exc), file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        print(_format_text(result))
    return _exit_code(result)


if __name__ == "__main__":
    sys.exit(main())
