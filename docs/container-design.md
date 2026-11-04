# Container Design Notes

The native `.tpt` container is a TLV (Type-Length-Value) chunk-based
format designed for:

- Forward compatibility (unknown chunk IDs are skipped; reserved header
  bytes are ignored).
- Fuzz resistance (every chunk length is bounds-checked; the parser never
  panics on corrupt or truncated input).
- Streaming (chunks can be parsed as they arrive; a partial trailing chunk
  merely pauses the parser).
- Seekability (keyframes are flags on `CLUS` chunks; a per-track index is
  optional and versioned).

The reference implementation lives in `containers/tpt-container`.

## Binary Layout

All integers are little-endian.

```text
FileHeader (20 bytes)
  magic        u8[4]   = "TPTM"        — 0x54 0x50 0x54 0x4D
  version      u16                       — major in high byte, minor in low
                                         — currently 0x0002 (major 0, minor 2);
                                         —   0x0001 files are still read
  header_size  u16                       — size of this header (currently 20;
                                         — 16-byte files version 0.1 still parse)
  timebase_num u32                       — numerator of global timebase
  timebase_den u32                       — denominator of global timebase
  reserved     u32                       — must be zero when writing; ignored on
                                         — read (reserved for future flags)

ChunkHeader (8 bytes)
  chunk_id     u8[4]                    — four bytes; defined IDs are ASCII ("TRAK",
                                         —   "CLUS", ...). First byte 0x00..=0x1F is
                                         —   reserved (see Forward Compatibility)
  chunk_size   u32                       — size of chunk body, LE

TrackHeader ("TRAK")
  track_id     u8
  media_type   u8                       — 0 audio, 1 video, 2 subtitle, 3 metadata;
                                         —   any other value fails the open (BadChunk)
  codec_id     u8[4]                    — codec identifier
  codec_version u16                     — codec bitstream version (major << 8 | minor)
  codec_priv_len u16
  codec_priv   u8[codec_priv_len]       — codec-specific configuration bytes
  track_timebase_num u32                — track timebase (zero num or den = use global)
  track_timebase_den u32
  sample_rate  u32                      — Hz, 0 = not applicable
  channel_layout u8                     — 0 none, 1 mono, 2 stereo, 3 5.1, 4 custom
  channel_mask u16                      — ChannelMask bits (meaningful when custom)
  width        u32                      — pixels, 0 = not applicable
  height       u32                      — pixels, 0 = not applicable
  pixel_format u8                       — 0 Gray8, 1 Rgb24, 2 Rgba32, 3 Yuv420, 4 Nv12,
                                         —   0xFF none; other values read as no format
  flags        u8                       — bit 0 = name present, bit 1 = language present
  name_len     u8 + name                — iff flags bit 0 (UTF-8)
  lang_len     u8 + lang                — iff flags bit 1 (UTF-8)

TrackMetadata ("TMTA")                   — typed key/value pairs for one track
  track_id     u8
  count        u16
  repeat:
    key_len    u8 + key                  — UTF-8 key
    value_type u8                        — 0 text, 1 int, 2 bool, 3 bytes
    value:
      text        u16 len + bytes
      int         i64 (LE)
      bool        1 byte (any nonzero value reads as true)
      bytes       u32 len + bytes

PacketRun ("PKTS")                      — since 0.2; written for every packet
  track_id     u8
  flags        u8                        — bit 0: checksum present
  count        u16                       — records, 1 to 32
  base_pts     u64                       — track timebase
  base_dts     u64                       — track timebase
  record × count:
    flags      u8                        — bit 0 keyframe, bit 1 end-of-frame,
                                         —   bit 2 discontinuity, bit 3 duration
                                         —   present, bit 4 dts present
    pts_delta  i32                       — pts − base_pts
    dts_delta  i32                       — iff bit 4: dts − base_dts (else dts = pts)
    duration   u32                       — iff bit 3 (ticks, track timebase)
    len        u32
    payload    u8[len]
  crc32        u32                       — iff flags bit 0: CRC-32 of every byte of
                                         —   the chunk body before this field

DataCluster ("CLUS")                     — version 0.1 layout; still read
  pts          u64                       — presentation timestamp, track timebase
  dts          u64                       — decode timestamp, track timebase
  flags        u8                        — bit 0 keyframe, bit 1 end-of-frame,
                                         —   bit 2 discontinuity, bit 3 duration
                                         —   present, bit 4 checksum present
  track_id     u8
  duration     u32                       — iff flags bit 3 (ticks, track timebase)
  payload      u8[...]                   — compressed frame
  checksum     u32                       — iff flags bit 4 (CRC-32 of the chunk body,
                                         —   excluding the 8-byte chunk header, up to
                                         —   but excluding this field)

Index ("IDX ")
  index_version u8                       — currently 2 (1 is still read)
  track_count  u8
  per track:
    track_id   u8
    entry_count u32                      — one entry per chunk (version 2) or per
                                         —   CLUS (version 1)
    per entry:
      pts      u64                       — pts of the chunk's first packet
      offset   u64                       — absolute byte offset of the chunk header
      keyframe u8                        — 1 if any packet in the chunk is a keyframe

Checksum footer ("CRC1")                 — written last by finalize
  crc          u32                       — CRC-32 of every byte up to but excluding
                                         —   the CRC1 chunk header; body must be 4 bytes
```

## Versioning (file header)

- `version` is `major << 8 | minor`. The current version is `0x0002`.
- The readable versions are 0x0001 (0.1) and 0x0002 (0.2), major 0. Version
  0x0000, a larger minor, and any other major are rejected with
  `UnsupportedVersion`. The set is listed rather than given as a range, because
  there is no version 0.0.
- Version 0.2 changed the packet framing (`PKTS` runs) and the index
  (one entry per chunk). A 0.2 reader reads 0.1 files; a 0.1 reader does not
  know `PKTS`, skips it, and so sees no packets in a 0.2 file. The version
  field is how it can tell.
- `header_size` is 16 (version 0.1, no reserved field) or at least 20. Sizes
  17 to 19 and anything below 16 are refused as `BadHeader`, since chunk
  parsing begins at `header_size` and would otherwise start inside the
  reserved field. Bytes between 20 and `header_size` are skipped, so larger
  headers can be added without breaking readers.
- The `reserved` bytes are ignored on read (forward compatibility: future
  flag bits can be added without a version bump).

## Forward Compatibility

- Reserved chunk IDs (`0x00..=0x1F` first byte) are skipped silently.
- Any unknown chunk ID is skipped; the parser advances by `chunk_size`.
- The `CLUS` flags byte is bitwise-mapped; unknown flag bits are ignored.
- Per-cluster and footer checksums are optional and disabled by default.
- A `PKTS` run's checksum covers the whole run and is checked once; a
  mismatch drops every packet in the run.

## Packet Runs (0.2)

A version 0.2 writer puts packets in `PKTS` runs rather than one `CLUS` per
packet. A run holds up to 32 packets of one track, and its timestamps are
deltas from the run's base, so the framing for an audio packet is about 9
bytes, against about 43 for a `CLUS`. The writer closes a run when it holds
32 packets, when the next packet is for another track or cannot be stored as
an `i32` delta, when the run would exceed 64 KiB (a run holding one packet may
be larger), before any `CLUS` chunk, and at `finalize`.

Measured on a 30 s Cinder file (`ed_music_30s.wav`, quality 80): 339,273 bytes
as version 0.1, 162,871 bytes as version 0.2 (the payload is 97,181 bytes in
both).

A reader that scans a run yields one packet per record, in order. The scan
index holds one entry per packet, so seeking works per packet, while the
on-disk `IDX ` holds one entry per chunk.

## Backward Compatibility

A reader of version 0.2 reads files written at 0.2 and at 0.1. Version 0.1
files have a 16-byte header and contain only `CLUS` chunks, so a 0.2 reader
reads them with the 0.1 layout and no `PKTS` runs. Major version bumps are
reserved for binary-incompatible header changes. Layouts older than 0.1
(untagged audio, epoch-timebase clusters) are not read; the reference
implementation has no compatibility shim for them. The container is
pre-release; no on-disk files exist outside this repository.

## Corruption Tolerance

- Truncated file (strict parse): returns a `Truncated` error; the partially
  decoded state is still available on the demuxer.
- Truncated file (streaming parse): parsing pauses at the incomplete chunk
  and resumes automatically once more bytes are fed.
- Invalid chunk length: the parser bounds-checks every body against the
  buffer; bodies that would run past the end are `Truncated`, and chunk sizes
  that overflow arithmetic are `BadChunk`.
- CRC/checksum mismatch: configurable; the default is to ignore per-cluster
  checksum mismatches and report them through
  `verify_cluster_checksums()`. The footer is verified on demand through
  `verify_checksum()`.
- Unknown chunk ID: skipped.

## Seeking

Linear seek: scan for `CLUS` chunks, ignore non-keyframes, return the first
keyframe at or after the requested timestamp.

Indexed seek: if an `IDX ` chunk is present *and versioned correctly*, it is
used for offset lookup. Per-track indexes live in memory on the demuxer
(`track_index(track_id)`); `seek_to_keyframe_on_track` and
`seek_to_offset` are the offset/lookup primitives. If the on-disk index is
missing, corrupt, or of a newer version, seeking falls back to the scan
index (recovery).

## Streaming

The container supports being written in a streaming mode:

- Chunks are flushed to the underlying writer as they are produced;
  no seek-back is required. A version 0.2 writer holds up to one `PKTS` run
  per track back until the run is full or the next packet starts another;
  `TptMuxer::flush` writes it early, which a live writer does before it sends
  bytes on.
- The parser (`new_streaming`) tolerates a partial trailing chunk; `feed()`
  resumes scanning once the buffer is extended (required: same prefix).
- An `IDX ` chunk can be appended at the end of the stream, or omitted
  entirely (the player falls back to linear scan).
- Graceful termination: `finalize()` writes the `IDX ` + `CRC1` trailer.

## Index Rebuilding / Footer Reconstruction

If the index is missing or corrupt, the container library offers a
`rebuild_index()` API that scans the file end-to-end and rebuilds the index
in memory. `reconstruct_trailer()` returns the `IDX ` + `CRC1` bytes for the
current buffer, which the caller can append to turn a live stream into a
fully finalized file. The result is not written back unless explicitly
requested.

## Checksums

- `CRC1` (footer): CRC-32 of the whole file up to the footer. Present in all
  finalized files. Verified with `verify_checksum()`.
- Per-cluster: optional (`CLUS` flags bit 4), enabled on the muxer with
  `enable_cluster_checksums()`. Verified with `verify_cluster_checksums()`.

## Reader Rules for Edge Cases

These rules fix behavior that the layout above leaves open. The reference
reader (`containers/tpt-container`) defines them, and they were settled when
the gaps were resolved (2026-11-05). A change to any of them is a format
change.

The stdlib reader (`tools/tpt-py`) follows the same rules, and its tests pin
them. It still reports the oddities it sees as warnings in its output.

- **Timebases.** A zero numerator or denominator in the file header fails the
  open (`BadHeader`). A track timebase with a zero term falls back to the
  global one.
- **Duplicate tracks.** The first `TRAK` for a track ID is used. Later `TRAK`s
  with the same ID are ignored.
- **Track references.** A `TMTA` applies only to a track whose `TRAK` has
  already been read, so `TRAK` must come before its `TMTA` (the writer always
  does this). A `TMTA` naming an undeclared track is ignored. `CLUS` and `PKTS`
  naming an undeclared track are still indexed and yielded.
- **Unknown values.** An unknown `media_type` fails the open (`BadChunk`). An
  unknown metadata `value_type` also fails the open, because the length of
  such a value is unknown and the entries after it cannot be found. This is
  the one unknown value that is not skipped. An unknown `pixel_format` reads
  as no format.
- **Bool values.** Any nonzero byte reads as `true`.
- **Strings.** `TRAK` names and languages, `TMTA` keys, and text values are
  UTF-8. Invalid UTF-8 reads as an empty string and does not fail the open.
- **Trailing bytes.** Bytes after the last field a reader decodes in a `TRAK`
  or `TMTA` body are ignored.
- **`CRC1`.** The body must be exactly 4 bytes; any other size is ignored as
  if absent. If several are present, the last wins. The footer covers the
  bytes before its own chunk header, so bytes after it are not covered. The
  writer always puts it last.
- **Checksum scope.** `CLUS` and `PKTS` checksums are CRC-32 of the chunk body,
  excluding the 8-byte chunk header.
- **Index offsets.** An `IDX ` offset is the byte offset of the first byte of
  the chunk's ID, i.e. the chunk header.

## Metadata Keys

Metadata keys are UTF-8 text. Keys beginning with `tpt.` are reserved for
names defined by this project. Other keys are free for writers to use.

- `tpt.io.sample_count` (Int, audio tracks): the original per-track sample
  count, before block-size padding. `tpt encode` writes it, and `tpt decode`
  trims the output to it. A caller-supplied value for the same key takes
  precedence over the one the encoder writes.
