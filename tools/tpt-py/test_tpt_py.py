"""Tests for tpt_py. Run from the repo root with:

    python -m unittest discover -s tools/tpt-py -t tools/tpt-py -v

Files are built byte-by-byte from docs/container-design.md, not with the
Rust writer, so these tests check the reader against the spec text.
"""

import io
import os
import struct
import sys
import tempfile
import unittest
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import tpt_py  # noqa: E402


def chunk(cid, body):
    return cid + struct.pack("<I", len(body)) + body


def file_header(version=0x0001, header_size=20, tb=(1, 1000), reserved=0):
    raw = b"TPTM" + struct.pack("<HHII", version, header_size, tb[0], tb[1])
    if header_size >= 20:
        raw += struct.pack("<I", reserved)
    return raw


def trak(track_id=0, media=0, codec=b"TEST", priv=b"", tb=(0, 0), flags=0, name=b"", lang=b""):
    body = struct.pack("<BB4sHH", track_id, media, codec, 0x0102, len(priv)) + priv
    body += struct.pack("<II", tb[0], tb[1])
    body += struct.pack("<I", 48000)            # sample_rate
    body += struct.pack("<B", 1)                # channel_layout mono
    body += struct.pack("<H", 0)                # channel_mask
    body += struct.pack("<II", 0, 0)            # width, height
    body += struct.pack("<B", 0xFF)             # pixel_format none
    body += struct.pack("<B", flags)
    if flags & 1:
        body += struct.pack("<B", len(name)) + name
    if flags & 2:
        body += struct.pack("<B", len(lang)) + lang
    return chunk(b"TRAK", body)


def clus(pts, track_id=0, payload=b"\x01\x02", keyframe=True, duration=None, checksum=False,
         dts=None):
    flags = (1 if keyframe else 0) | 2
    if duration is not None:
        flags |= 8
    if checksum:
        flags |= 16
    body = struct.pack("<QQBB", pts, pts if dts is None else dts, flags, track_id)
    if duration is not None:
        body += struct.pack("<I", duration)
    body += payload
    if checksum:
        body += struct.pack("<I", zlib.crc32(body) & 0xFFFFFFFF)
    return body


def clus_chunk(*args, **kwargs):
    return chunk(b"CLUS", clus(*args, **kwargs))


def tmta(track_id, entries):
    body = struct.pack("<BH", track_id, len(entries))
    for key, vtype, value in entries:
        kb = key.encode("utf-8")
        body += struct.pack("<B", len(kb)) + kb + struct.pack("<B", vtype)
        if vtype == 0:
            body += struct.pack("<H", len(value)) + value
        elif vtype == 1:
            body += struct.pack("<q", value)
        elif vtype == 2:
            body += struct.pack("<B", 1 if value else 0)
        else:
            body += struct.pack("<I", len(value)) + value
    return chunk(b"TMTA", body)


def crc1_chunk(prefix):
    return chunk(b"CRC1", struct.pack("<I", zlib.crc32(prefix) & 0xFFFFFFFF))


def build(chunks, crc=False, **hdr):
    data = file_header(**hdr) + b"".join(chunks)
    if crc:
        data += crc1_chunk(data)
    return data


def parse_bytes(data):
    return tpt_py.parse_stream(io.BytesIO(data))


def simple_file(crc=False):
    return build([trak(0), clus_chunk(0), clus_chunk(20), clus_chunk(40)], crc=crc)


def pkts_chunk(track_id, records, base_pts=0, base_dts=0, checksum=False):
    """A PKTS run. records: (flags, pts_delta, payload) tuples; the dts is
    stored when it is given as a fourth element (its delta from base_dts)."""
    body = struct.pack("<BBHQQ", track_id, 1 if checksum else 0, len(records),
                       base_pts, base_dts)
    for rec in records:
        flags, pts_delta, payload = rec[0], rec[1], rec[2]
        dts_delta = rec[3] if len(rec) > 3 else None
        if dts_delta is not None:
            flags |= 0x10
        body += struct.pack("<B", flags) + struct.pack("<i", pts_delta)
        if dts_delta is not None:
            body += struct.pack("<i", dts_delta)
        body += struct.pack("<I", len(payload)) + payload
    if checksum:
        body += struct.pack("<I", zlib.crc32(body) & 0xFFFFFFFF)
    return chunk(b"PKTS", body)


class Runs(unittest.TestCase):
    """Version 0.2 PKTS runs, written from the byte layout in container-design.md."""

    def test_run_yields_one_packet_per_record(self):
        data = build([trak(0), pkts_chunk(0, [(1, 0, b"aa"), (0, 5, b"bbb", 7), (0, 9, b"c")],
                                          base_pts=100, base_dts=100)])
        r = parse_bytes(data)
        pk = r["tracks"][0]["packets"]
        self.assertEqual([p["pts"] for p in pk], [100, 105, 109])
        self.assertEqual([p["dts"] for p in pk], [100, 107, 109])
        self.assertEqual([p["size"] for p in pk], [2, 3, 1])
        self.assertEqual([p["slot"] for p in pk], [0, 1, 2])
        self.assertTrue(pk[0]["keyframe"])
        self.assertFalse(pk[1]["keyframe"])

    def test_run_checksum_is_checked_once_and_reported_on_its_first_packet(self):
        good = build([trak(0), pkts_chunk(0, [(1, 0, b"x"), (0, 1, b"y")], checksum=True)])
        r = parse_bytes(good)
        self.assertEqual(r["checksums"]["clusters_with_checksum"], 1)
        self.assertTrue(r["tracks"][0]["packets"][0]["checksum_ok"])
        self.assertIsNone(r["tracks"][0]["packets"][1]["checksum_ok"])
        bad = bytearray(good)
        last_payload = len(good) - 4 - 1          # the "y", just before the CRC
        self.assertEqual(bytes(bad[last_payload:last_payload + 1]), b"y")
        bad[last_payload] ^= 0xFF
        r = parse_bytes(bytes(bad))
        self.assertEqual(len(r["checksums"]["cluster_failures"]), 1)

    def test_record_running_past_the_body_is_rejected(self):
        body = struct.pack("<BBHQQ", 0, 0, 1, 0, 0) + struct.pack("<BiI", 1, 0, 500) + b"short"
        with self.assertRaises(tpt_py.BadChunk):
            parse_bytes(build([trak(0), chunk(b"PKTS", body)]))

    def test_run_with_more_than_32_records_is_rejected(self):
        with self.assertRaises(tpt_py.BadChunk):
            parse_bytes(build([trak(0), pkts_chunk(0, [(1, i, b"") for i in range(33)])]))

    def test_index_version_2_matches_one_entry_per_run(self):
        run = pkts_chunk(0, [(1, 0, b"a"), (1, 1, b"b")], base_pts=0)
        run2 = pkts_chunk(0, [(0, 0, b"c")], base_pts=10)
        base = build([trak(0), run, run2])
        r = parse_bytes(base)
        offsets = sorted({p["offset"] for p in r["tracks"][0]["packets"]})
        self.assertEqual(len(offsets), 2)
        idx_body = struct.pack("<BB", 2, 1) + struct.pack("<BI", 0, 2)
        idx_body += struct.pack("<QQB", 0, offsets[0], 1)
        idx_body += struct.pack("<QQB", 10, offsets[1], 0)
        r = parse_bytes(base + chunk(b"IDX ", idx_body))
        self.assertTrue(r["index"]["valid"])
        self.assertTrue(r["index"]["consistent"], r["warnings"])


class ParseBasics(unittest.TestCase):
    def test_minimal_file_round_trip(self):
        r = parse_bytes(simple_file())
        self.assertEqual(r["file_header"]["timebase"], {"num": 1, "den": 1000})
        self.assertEqual(len(r["tracks"]), 1)
        t = r["tracks"][0]
        self.assertEqual(t["codec_id"], "TEST")
        self.assertEqual(t["media_type"], "audio")
        self.assertEqual(t["packet_count"], 3)
        self.assertEqual([p["pts"] for p in t["packets"]], [0, 20, 40])
        self.assertEqual(t["packets"][0]["offset"], 20 + len(trak(0)))
        self.assertEqual(t["packets"][0]["size"], 2)
        self.assertTrue(t["timebase_is_global"])
        self.assertEqual(r["warnings"], [])

    def test_header_size_16_is_accepted(self):
        data = file_header(header_size=16) + trak(0) + clus_chunk(5)
        r = parse_bytes(data)
        self.assertEqual(r["tracks"][0]["packets"][0]["offset"], 16 + len(trak(0)))

    def test_track_timebase_and_metadata(self):
        data = build([trak(0, tb=(1, 48000)),
                      tmta(0, [("title", 0, b"Tone"), ("n", 1, -7),
                               ("flag", 2, True), ("blob", 3, b"\xde\xad")]),
                      clus_chunk(0)])
        t = parse_bytes(data)["tracks"][0]
        self.assertEqual(t["timebase"], {"num": 1, "den": 48000})
        self.assertFalse(t["timebase_is_global"])
        self.assertEqual(t["metadata"], [
            {"key": "title", "type": "text", "value": "Tone"},
            {"key": "n", "type": "int", "value": -7},
            {"key": "flag", "type": "bool", "value": True},
            {"key": "blob", "type": "bytes", "value": "dead"},
        ])

    def test_empty_file_after_header_has_no_tracks(self):
        r = parse_bytes(build([]))
        self.assertEqual(r["tracks"], [])
        self.assertFalse(r["checksums"]["crc1"]["present"])


class Errors(unittest.TestCase):
    def test_truncated_file_below_header_size(self):
        with self.assertRaises(tpt_py.Truncated):
            parse_bytes(b"TPTM\x01\x00\x14")

    def test_truncated_mid_chunk_body(self):
        data = simple_file()
        with self.assertRaises(tpt_py.Truncated):
            parse_bytes(data[:-3])

    def test_truncated_mid_chunk_header(self):
        data = simple_file()
        # Cut 4 bytes into the header of the last CLUS chunk.
        last_clus = len(data) - (8 + len(clus(40)))
        with self.assertRaises(tpt_py.Truncated):
            parse_bytes(data[:last_clus + 4])

    def test_cut_on_chunk_boundary_parses(self):
        data = simple_file()
        last_clus = len(data) - (8 + len(clus(40)))
        r = parse_bytes(data[:last_clus])
        self.assertEqual(r["tracks"][0]["packet_count"], 2)

    def test_bad_magic(self):
        data = b"TPTX" + simple_file()[4:]
        with self.assertRaises(tpt_py.BadMagic):
            parse_bytes(data)

    def test_unsupported_versions(self):
        for version in (0x0000, 0x0003, 0x0100, 0x0201):
            with self.subTest(version=hex(version)):
                with self.assertRaises(tpt_py.UnsupportedVersion):
                    parse_bytes(build([trak(0)], version=version))

    def test_header_size_17_to_19_rejected(self):
        for size in (17, 18, 19):
            with self.subTest(header_size=size):
                with self.assertRaises(tpt_py.BadHeader):
                    parse_bytes(file_header(header_size=size) + trak(0) + clus_chunk(5))

    def test_header_size_below_16_rejected(self):
        with self.assertRaises(tpt_py.BadHeader):
            parse_bytes(b"TPTM" + struct.pack("<HHII", 1, 12, 1, 1) + b"\x00" * 4)

    def test_chunk_length_past_eof(self):
        data = file_header() + b"CLUS" + struct.pack("<I", 0xFFFFFFFF) + b"\x00" * 10
        with self.assertRaises(tpt_py.Truncated):
            parse_bytes(data)

    def test_track_field_past_chunk_body(self):
        # codec_priv_len claims 200 bytes inside a 20-byte TRAK body.
        body = struct.pack("<BB4sHH", 0, 0, b"TEST", 1, 200) + b"\x00" * 10
        with self.assertRaises(tpt_py.BadChunk):
            parse_bytes(build([chunk(b"TRAK", body)]))

    def test_unknown_media_type_fails_the_open(self):
        # Reference reader rule: an unknown media_type is a BadChunk.
        with self.assertRaises(tpt_py.BadChunk):
            parse_bytes(build([trak(0, media=7)]))

    def test_clus_for_undeclared_track_is_listed_with_a_warning(self):
        # Reference reader rule: packets for a track with no TRAK are still
        # yielded; the track carries no descriptive metadata.
        result = parse_bytes(build([trak(0), clus_chunk(0, track_id=9)]))
        ids = {t["id"]: t for t in result["tracks"]}
        self.assertEqual(ids[9]["packet_count"], 1)
        self.assertIsNone(ids[9]["codec_id"])
        self.assertTrue(any("undeclared track 9" in w for w in result["warnings"]))

    def test_duplicate_trak_first_one_wins(self):
        result = parse_bytes(build([trak(0, codec=b"AAAA"), trak(0, codec=b"BBBB")]))
        self.assertEqual(len(result["tracks"]), 1)
        self.assertEqual(result["tracks"][0]["codec_id"], "AAAA")
        self.assertTrue(any("duplicate TRAK" in w for w in result["warnings"]))

    def test_tmta_before_its_trak_is_ignored(self):
        result = parse_bytes(build([tmta(0, [("k", 1, 5)]), trak(0)]))
        self.assertEqual(result["tracks"][0]["metadata"], [])
        self.assertTrue(any("undeclared track 0" in w for w in result["warnings"]))

    def test_tmta_after_its_trak_is_applied(self):
        result = parse_bytes(build([trak(0), tmta(0, [("k", 1, 5)])]))
        self.assertEqual(result["tracks"][0]["metadata"], [{"key": "k", "type": "int", "value": 5}])

    def test_one_zero_timebase_term_falls_back_to_global(self):
        result = parse_bytes(build([trak(0, tb=(0, 90))]))
        self.assertTrue(result["tracks"][0]["timebase_is_global"])
        self.assertTrue(any("one zero term" in w for w in result["warnings"]))

    def test_invalid_utf8_name_reads_as_empty(self):
        result = parse_bytes(build([trak(0, name=b"\xff\xfe", flags=1)]))
        self.assertEqual(result["tracks"][0]["name"], "")

    def test_crc1_with_wrong_body_size_is_ignored(self):
        data = build([trak(0), clus_chunk(0)]) + chunk(b"CRC1", b"\x00" * 8)
        result = parse_bytes(data)
        self.assertFalse(result["checksums"]["crc1"]["present"])
        self.assertTrue(any("expected 4" in w for w in result["warnings"]))

    def test_second_crc1_replaces_the_first(self):
        base = build([trak(0), clus_chunk(0)], crc=True)
        data = base + chunk(b"CRC1", struct.pack("<I", 0x12345678))
        result = parse_bytes(data)
        self.assertEqual(result["checksums"]["crc1"]["stored"], "0x12345678")
        self.assertTrue(any("second CRC1" in w for w in result["warnings"]))

    def test_huge_index_count_is_corrupt_not_allocated(self):
        idx = struct.pack("<BB", 1, 1) + struct.pack("<BI", 0, 0xFFFFFFFF)
        r = parse_bytes(build([trak(0), clus_chunk(0), chunk(b"IDX ", idx)]))
        self.assertTrue(r["index"]["present"])
        self.assertFalse(r["index"]["valid"])
        self.assertTrue(any("index ignored" in w for w in r["warnings"]))
        self.assertEqual(r["tracks"][0]["packet_count"], 1)

    def test_cli_truncated_exit_code_and_json_error(self):
        import json
        import subprocess
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "cut.tpt")
            with open(path, "wb") as f:
                f.write(simple_file()[:-3])
            script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tpt_py.py")
            p = subprocess.run([sys.executable, "-I", script, "probe", "--json", path],
                               capture_output=True, text=True)
            self.assertEqual(p.returncode, 1)
            self.assertEqual(json.loads(p.stdout)["error"]["type"], "Truncated")


class UnknownChunks(unittest.TestCase):
    def test_unknown_chunks_are_skipped(self):
        extra = build([trak(0), chunk(b"ZZZZ", b"opaque\x00\xff" * 5),
                       clus_chunk(0),
                       chunk(b"\x01\x02\x03\x04", b"reserved"),
                       clus_chunk(20)])
        r = parse_bytes(extra)
        self.assertEqual(r["tracks"][0]["packet_count"], 2)
        self.assertEqual([p["pts"] for p in r["tracks"][0]["packets"]], [0, 20])
        reasons = {c["id"]: c["reason"] for c in r["skipped_chunks"]}
        self.assertEqual(reasons, {"ZZZZ": "unknown", "\\x01\\x02\\x03\\x04": "reserved"})
        self.assertEqual(r["skipped_chunks"][0]["size"], 40)
        # Second CLUS offset: file header + TRAK + ZZZZ + CLUS + reserved chunk.
        expected = (20 + len(trak(0)) + len(chunk(b"ZZZZ", b"opaque\x00\xff" * 5))
                    + len(clus_chunk(0)) + len(chunk(b"\x01\x02\x03\x04", b"reserved")))
        self.assertEqual(r["tracks"][0]["packets"][1]["offset"], expected)
        self.assertEqual(extra[expected:expected + 4], b"CLUS")

    def test_unknown_chunk_after_crc1_is_warned_not_fatal(self):
        data = build([trak(0), clus_chunk(0)], crc=True) + chunk(b"ZZZZ", b"")
        r = parse_bytes(data)
        self.assertTrue(any("follows CRC1" in w for w in r["warnings"]))
        self.assertTrue(r["checksums"]["crc1"]["ok"])


class Checksums(unittest.TestCase):
    def test_crc1_ok_for_untouched_file(self):
        r = parse_bytes(simple_file(crc=True))
        self.assertTrue(r["checksums"]["crc1"]["ok"])

    def test_crc1_mismatch_detected_on_payload_flip(self):
        data = bytearray(simple_file(crc=True))
        # Flip a payload byte of the first CLUS (payload starts 18 bytes into its body).
        first_payload = 20 + len(trak(0)) + 8 + 18
        data[first_payload] ^= 0x40
        r = parse_bytes(bytes(data))
        crc = r["checksums"]["crc1"]
        self.assertTrue(crc["present"])
        self.assertFalse(crc["ok"])
        self.assertNotEqual(crc["stored"], crc["computed"])

    def test_crc1_mismatch_detected_on_stored_value_flip(self):
        data = bytearray(simple_file(crc=True))
        data[-1] ^= 0x01
        r = parse_bytes(bytes(data))
        self.assertFalse(r["checksums"]["crc1"]["ok"])

    def test_cluster_checksum_mismatch_detected(self):
        good = build([trak(0), clus_chunk(0, checksum=True, payload=b"abcdef")])
        r = parse_bytes(good)
        self.assertEqual(r["checksums"]["clusters_with_checksum"], 1)
        self.assertEqual(r["checksums"]["cluster_failures"], [])
        self.assertTrue(r["tracks"][0]["packets"][0]["checksum_ok"])

        bad = bytearray(good)
        bad[20 + len(trak(0)) + 8 + 18 + 2] ^= 0xFF          # corrupt a payload byte
        r = parse_bytes(bytes(bad))
        self.assertEqual(r["checksums"]["cluster_failures"], [20 + len(trak(0))])
        self.assertFalse(r["tracks"][0]["packets"][0]["checksum_ok"])

    def test_cli_exit_code_2_on_checksum_mismatch(self):
        import subprocess
        data = bytearray(simple_file(crc=True))
        data[20 + len(trak(0)) + 8 + 18] ^= 0x01
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "bad.tpt")
            with open(path, "wb") as f:
                f.write(bytes(data))
            script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tpt_py.py")
            p = subprocess.run([sys.executable, "-I", script, "probe", path],
                               capture_output=True, text=True)
            self.assertEqual(p.returncode, 2)
            self.assertIn("MISMATCH", p.stdout)


class Index(unittest.TestCase):
    def test_consistent_index(self):
        base = build([trak(0), clus_chunk(0), clus_chunk(20)])
        r = parse_bytes(base)
        off = [p["offset"] for p in r["tracks"][0]["packets"]]
        idx_body = struct.pack("<BB", 1, 1) + struct.pack("<BI", 0, 2)
        for pts, o, kf in ((0, off[0], 1), (20, off[1], 1)):
            idx_body += struct.pack("<QQB", pts, o, kf)
        r = parse_bytes(base + chunk(b"IDX ", idx_body))
        self.assertTrue(r["index"]["valid"])
        self.assertTrue(r["index"]["consistent"])

    def test_inconsistent_index_warns(self):
        base = build([trak(0), clus_chunk(0)])
        idx_body = struct.pack("<BB", 1, 1) + struct.pack("<BI", 0, 1) + struct.pack("<QQB", 0, 999, 1)
        r = parse_bytes(base + chunk(b"IDX ", idx_body))
        self.assertFalse(r["index"]["consistent"])
        self.assertTrue(any("does not match" in w for w in r["warnings"]))

    def test_unsupported_index_version_ignored(self):
        base = build([trak(0), clus_chunk(0)])
        idx_body = struct.pack("<BB", 3, 0)
        r = parse_bytes(base + chunk(b"IDX ", idx_body))
        self.assertFalse(r["index"]["valid"])
        self.assertIsNone(r["index"]["consistent"])


if __name__ == "__main__":
    unittest.main()

