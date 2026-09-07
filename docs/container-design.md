# Container Design Notes

The native `.tpt` container is a TLV (Type-Length-Value) chunk-based
format designed for:

- Forward compatibility (unknown chunk IDs are skipped).
- Fuzz resistance (every chunk length is bounds-checked).
- Streaming (chunks can be parsed as they arrive).
- Seekability (keyframes are flags on `CLUS` chunks).

## Binary Layout

All integers are little-endian.

```text
FileHeader (16 bytes)
  magic        u8[4]   = "TPTM"        — 0x54 0x50 0x54 0x4D
  version      u16                       — major in low byte, minor in high
  header_size  u16                       — size of this header (currently 16)
  timebase_num u32                       — numerator of global timebase
  timebase_den u32                       — denominator of global timebase

ChunkHeader (8 bytes)
  chunk_id     u8[4]                    — 4 ASCII bytes ("TRAK", "CLUS", ...)
  chunk_size   u32                       — size of chunk body, LE

TrackHeader ("TRAK")
  track_id     u8                        — 0..N
  codec_id     u8[4]                     — codec identifier
  codec_priv   u8[...]                   — codec-specific configuration bytes

DataCluster ("CLUS")
  timestamp    u64                       — in global timebase units
  flags        u8                        — bit 0 = keyframe, bit 1 = EOS
  track_id     u8                        — 0..N
  payload      u8[...]                   — compressed frame
```

## Forward Compatibility

- Reserved chunk IDs (`0x00..=0x1F`) are skipped silently.
- The container parser must not fail on an unknown chunk; it should log
  and skip to the next 8-byte aligned position based on `chunk_size`.

## Backward Compatibility

A reader of version `X` must be able to read a file written by an
encoder of version `X` or any earlier minor version. Major version
bumps are reserved for binary-incompatible header changes.

## Corruption Tolerance

- Truncated file: parser returns a `Truncated` error and the partially
  decoded state.
- Invalid chunk length: parser logs the chunk ID and seeks to the next
  8-byte aligned position based on the current offset and the declared
  chunk size.
- CRC/checksum mismatch: configurable; the default is to log and
  continue, but a strict mode is available.
- Unknown chunk ID: skipped.

## Seeking

Linear seek: scan for `CLUS` chunks, ignore non-keyframes, return the
offset of the first keyframe at or after the requested timestamp.

Indexed seek (optional): if an `INDX` chunk is present, use it instead
of scanning. The index is built lazily on first use.

## Streaming

The container supports being written in a streaming mode:

- Chunks are flushed to the underlying writer as they are produced.
- No seek-back is required.
- An `INDX` chunk can be appended at the end of the stream, or omitted
  entirely (the player will fall back to linear scan).

## Recovery / Index Rebuilding

If the index is missing or corrupt, the container library offers a
`rebuild_index()` API that scans the file end-to-end and rebuilds the
index in memory. The result is not written back unless explicitly
requested.