/*
 * tpt_media.h — the stable C ABI of the tpt-media codec abstraction.
 *
 * Hand-written counterpart of `ffi/tpt-media-sys/src/lib.rs`; kept in
 * sync with it by `tests/round_trip.rs::header_matches_the_abi` (every
 * exported symbol must appear here, and vice versa). If you change the
 * ABI, change both files in the same commit — see the ABI stability
 * rule in lib.rs (adding functions or error codes is compatible;
 * changing signatures, struct layouts, or code meanings bumps
 * TPT_ABI_VERSION).
 *
 * Ownership, thread-safety, and panic rules are documented once in
 * lib.rs's crate docs and summarized per function below. In short:
 * handles are exclusively owned (never call into one handle from two
 * threads), every allocating call has a matching free, packet payloads
 * are copied in, and no Rust panic ever unwinds into C (a caught panic
 * surfaces as TPT_ERR_PANIC).
 */
#ifndef TPT_MEDIA_H
#define TPT_MEDIA_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI revision of this library; see the stability rule above. */
#define TPT_ABI_VERSION 1u

/* Error codes returned by every function. Zero is success; the values
 * and meanings are ABI and never change within a version. */
enum tpt_error {
    TPT_OK = 0,
    TPT_ERR_NULL_ARGUMENT = -1,   /* a required pointer was NULL            */
    TPT_ERR_INVALID_ARGUMENT = -2,/* nonsensical value (rate 0, ch != 1,    */
                                  /* quality > 100, wrong block size, ...)  */
    TPT_ERR_UNSUPPORTED = -3,     /* unknown codec id / bitstream version   */
    TPT_ERR_MALFORMED_PACKET = -4,/* packet malformed or truncated          */
    TPT_ERR_NEED_MORE_INPUT = -5, /* decoder needs more input               */
    TPT_ERR_RESOURCE_LIMIT = -6,  /* a configured resource limit was hit    */
    TPT_ERR_INVALID_CONFIG = -7,  /* config rejected by the codec           */
    TPT_ERR_CANCELLED = -8,       /* operation cancelled                    */
    TPT_ERR_INTERNAL = -9,        /* codec-layer invariant violated         */
    TPT_ERR_PANIC = -10           /* a Rust panic was caught at the ABI     */
};

/* Opaque handles. Each is exclusively owned: free with the matching
 * tpt_*_free, never use from two threads at once. */
typedef struct TptEncoder TptEncoder;
typedef struct TptDecoder TptDecoder;
typedef struct TptFrame TptFrame;
typedef struct TptPacket TptPacket;

/* The abstract per-codec configuration — the FFI counterpart of the
 * CLI's --quality, so callers never touch per-codec parameter structs.
 *   sample_rate: Hz (44.1/48k for Cinder/Aura, 8/16k for Reed)
 *   channels:    must be 1 (the native audio family is S16 mono)
 *   quality:     0..=100; maps to Cinder coefficient bits (4 + q*8/100)
 *                and Reed LSF bits (4 + q*4/100); ignored by Aura
 *                (lossless)
 */
typedef struct tpt_media_config {
    uint32_t sample_rate;
    uint16_t channels;
    uint8_t quality;
} tpt_media_config;

/* Returns the ABI revision of the loaded library. */
uint32_t tpt_abi_version(void);

/* Crate version, NUL-terminated static string (diagnostics). */
const char *tpt_version_string(void);

/* Pack four ASCII bytes into the codec id the constructors take
 * (e.g. tpt_codec_id('C','I','N','D') for Cinder). */
uint32_t tpt_codec_id(uint8_t a, uint8_t b, uint8_t c, uint8_t d);

/* Samples per channel one encode call consumes for this codec and
 * sample rate (Cinder 512, Aura 1024, Reed rate/50); 0 for an unknown
 * codec id. Size your tpt_frame_new with this. */
uint32_t tpt_encoder_block_size(uint32_t codec_id, uint32_t sample_rate);

/* Allocate an encoder. `out` receives the handle; caller frees with
 * tpt_encoder_free. */
int32_t tpt_encoder_new(uint32_t codec_id, const tpt_media_config *config,
                        TptEncoder **out);

/* Encode one frame into at most one packet. `out` receives a new
 * packet handle (tpt_packet_free) or NULL when nothing was produced.
 * The frame must carry exactly tpt_encoder_block_size samples per
 * channel; it is read, never stored. */
int32_t tpt_encoder_encode(TptEncoder *enc, const TptFrame *frame,
                           TptPacket **out);

/* Finish the stream: emit any buffered packet (NULL `out` when none).
 * Free the encoder afterwards; further frames are not supported. */
int32_t tpt_encoder_flush(TptEncoder *enc, TptPacket **out);

/* Release an encoder handle. NULL is allowed. */
void tpt_encoder_free(TptEncoder *enc);

/* Allocate a decoder built with the same quality→parameter mapping as
 * tpt_encoder_new, so a pair from one config round-trips. */
int32_t tpt_decoder_new(uint32_t codec_id, const tpt_media_config *config,
                        TptDecoder **out);

/* Decode one packet into at most one frame. `out` receives a new frame
 * handle (tpt_frame_free) or NULL when nothing was produced; a frame
 * from this call carries the packet's PTS. */
int32_t tpt_decoder_decode(TptDecoder *dec, const TptPacket *pkt,
                           TptFrame **out);

/* Finish the stream: emit any buffered frame (NULL `out` when none).
 * A flushed frame carries PTS 0 — there is no packet to associate. */
int32_t tpt_decoder_flush(TptDecoder *dec, TptFrame **out);

/* Release a decoder handle. NULL is allowed. */
void tpt_decoder_free(TptDecoder *dec);

/* Allocate an S16 mono frame of `sample_count` samples, zero-filled,
 * at `pts_ticks` on the tb_num/tb_den timebase. Fill it through
 * tpt_frame_samples. */
int32_t tpt_frame_new(uint32_t sample_rate, uint16_t channels,
                      uint32_t sample_count, int64_t pts_ticks,
                      uint32_t tb_num, uint32_t tb_den, TptFrame **out);

/* Writable pointer to the frame's S16 interleaved sample bytes
 * (sample_count * channels * 2 bytes; valid until free). */
uint8_t *tpt_frame_samples(TptFrame *frame);

/* Read-only pointer to the frame's sample bytes. */
const uint8_t *tpt_frame_samples_const(const TptFrame *frame);

/* Samples per channel. */
uint32_t tpt_frame_sample_count(const TptFrame *frame);

/* Sample rate in Hz. */
uint32_t tpt_frame_sample_rate(const TptFrame *frame);

/* PTS in timebase ticks. */
int64_t tpt_frame_pts(const TptFrame *frame);

/* Release a frame handle. NULL is allowed. */
void tpt_frame_free(TptFrame *frame);

/* Allocate a packet by COPYING len bytes from `data` (your buffer
 * stays yours), with pts/dts on the tb_num/tb_den timebase. */
int32_t tpt_packet_new(const uint8_t *data, uint32_t len, int64_t pts_ticks,
                       int64_t dts_ticks, uint32_t tb_num, uint32_t tb_den,
                       int32_t keyframe, TptPacket **out);

/* Read-only payload pointer (tpt_packet_len bytes; valid until free). */
const uint8_t *tpt_packet_data(const TptPacket *pkt);

/* Payload length in bytes. */
uint32_t tpt_packet_len(const TptPacket *pkt);

/* PTS / DTS in timebase ticks. */
int64_t tpt_packet_pts(const TptPacket *pkt);
int64_t tpt_packet_dts(const TptPacket *pkt);

/* 1 when the packet is a keyframe, 0 otherwise. */
int32_t tpt_packet_is_keyframe(const TptPacket *pkt);

/* Release a packet handle. NULL is allowed. */
void tpt_packet_free(TptPacket *pkt);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* TPT_MEDIA_H */
