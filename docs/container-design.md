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
  chunk_id     u8[4]                    — 4 ASCII bytes ("TRAK", "CLUS", ...)
  chunk_size   u32                       — size of chunk body, LE

TrackHeader ("TRAK")
  track_id     u8
  media_type   u8                       — 0 audio, 1 video, 2 subtitle, 3 metadata
  codec_id     u8[4]                    — codec identifier
  codec_version u16                     — codec bitstream version (major << 8 | minor)
  codec_priv_len u16
  codec_priv   u8[codec_priv_len]       — codec-specific configuration bytes
  track_timebase_num u32                — track timebase (0/0 = use global)
  track_timebase_den u32
  sample_rate  u32                      — Hz, 0 = not applicable
  channel_layout u8                     — 0 none, 1 mono, 2 stereo, 3 5.1, 4 custom
  channel_mask u16                      — ChannelMask bits (meaningful when custom)
  width        u32                      — pixels, 0 = not applicable
  height       u32                      — pixels, 0 = not applicable
  pixel_format u8                       — 0xFF = none, else PixelFormat value
  flags        u8                       — bit 0 = name present, bit 1 = language present
  name_len     u8 + name                — iff flags bit 0
  lang_len     u8 + lang                — iff flags bit 1

TrackMetadata ("TMTA")                   — typed key/value pairs for one track
  track_id     u8
  count        u16
  repeat:
    key_len    u8 + key                  — UTF-8 key
    value_type u8                        — 0 text, 1 int, 2 bool, 3 bytes
    value:
      text        u16 len + bytes
      int         i64 (LE)
      bool        1 byte
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
                                         —   the body before this field

DataCluster ("CLUS")                     — version 0.1 layout; still read
  pts          u64                       — presentation timestamp, track timebase
  dts          u64                       — decode timestamp, track timebase
  flags        u8                        — bit 0 keyframe, bit 1 end-of-frame,
                                         —   bit 2 discontinuity, bit 3 duration
                                         —   present, bit 4 checksum present
  track_id     u8
  duration     u32                       — iff flags bit 3 (ticks, track timebase)
  payload      u8[...]                   — compressed frame
  checksum     u32                       — iff flags bit 4 (CRC-32 of the body up
                                         —   to but excluding this field)

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

Checksum footer ("CRC1")
  crc          u32                       — CRC-32 of every byte up to but excluding
                                         —   the CRC1 chunk header
```

## Versioning (file header)

- `version` is `major << 8 | minor`. The current version is `0x0002`.
- A reader must accept any version equal to its own or an earlier minor in
  the same major. `version == 0`, a larger minor, or any higher major is
  rejected with `UnsupportedVersion`.
- Version 0.2 changed the packet framing (`PKTS` runs) and the index
  (one entry per chunk). A 0.2 reader reads 0.1 files; a 0.1 reader does not
  know `PKTS`, skips it, and so sees no packets in a 0.2 file. The version
  field is how it can tell.
- `header_size` must be at least 16. Chunk parsing begins at `header_size`,
  so larger headers can be added without breaking readers.
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

A reader of version `X` can read a file written by an encoder of version `X`
or any earlier minor version. Version 0.2 readers read version 0.1 files,
which contain only `CLUS` chunks. Major version bumps are reserved for
binary-incompatible header changes. Files written by version 0.1 (16-byte
header, `TRAK`/`CLUS` without the extended fields) are no longer recognized
as *valid* 0.1 files only if their magic matches; older 0.1 tracks parse as
untagged audio with epoch-timebase clusters only through a strict
compatibility shim, which the reference implementation does not provide.
The container is pre-release; no on-disk files exist outside this repository.

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