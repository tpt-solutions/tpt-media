/*
 * C integration test for the tpt-media ABI (todo.md §46).
 *
 * Compiled by the build script against include/tpt_media.h — this file
 * deliberately knows nothing about Rust: it sees only the header, the
 * way a real C consumer would. tests/c_abi.rs calls the entry point
 * below and asserts it succeeded.
 *
 * Returns 0 on success, a nonzero stage code on the first failure.
 */
#include <stdint.h>
#include <string.h>

#include "tpt_media.h"

/* Same deterministic sample the Rust tests use: small enough that a
 * lossless codec round-trips it exactly. */
static int16_t sample_at(int i) {
    return (int16_t)(((i % 64) - 32) * 64);
}

int32_t tpt_c_integration_round_trip(void) {
    const uint32_t codec = tpt_codec_id('A', 'U', 'R', 'A');
    const uint32_t rate = 48000;
    const uint32_t blocks = 2;
    uint32_t b, i;

    if (tpt_abi_version() != TPT_ABI_VERSION) return 1;

    tpt_media_config cfg;
    cfg.sample_rate = rate;
    cfg.channels = 1;
    cfg.quality = 80;

    TptEncoder *enc = 0;
    if (tpt_encoder_new(codec, &cfg, &enc) != TPT_OK) return 2;

    const uint32_t block = tpt_encoder_block_size(codec, rate);
    if (block == 0) return 3;

    TptDecoder *dec = 0;
    if (tpt_decoder_new(codec, &cfg, &dec) != TPT_OK) return 4;

    for (b = 0; b < blocks; ++b) {
        TptFrame *f = 0;
        if (tpt_frame_new(rate, 1, block, (int64_t)b, 1, rate, &f) != TPT_OK) return 5;
        uint8_t *samples = tpt_frame_samples(f);
        if (samples == 0) return 6;
        for (i = 0; i < block; ++i) {
            int16_t v = sample_at((int)i + (int)(b * block));
            memcpy(samples + i * 2, &v, 2);
        }

        TptPacket *p = 0;
        if (tpt_encoder_encode(enc, f, &p) != TPT_OK) return 7;
        if (p == 0) return 8;
        if (!tpt_packet_is_keyframe(p)) return 9;
        if (tpt_packet_len(p) == 0) return 10;
        tpt_frame_free(f);
        f = 0;

        TptFrame *out = 0;
        if (tpt_decoder_decode(dec, p, &out) != TPT_OK) return 11;
        if (out == 0) return 12;
        if (tpt_frame_sample_count(out) != block) return 13;
        if (tpt_frame_sample_rate(out) != rate) return 14;
        if (tpt_frame_pts(out) != (int64_t)b) return 15;

        /* Aura lossless: the decoded samples must be bit-exact. */
        {
            const uint8_t *got = tpt_frame_samples_const(out);
            for (i = 0; i < block; ++i) {
                int16_t expect = sample_at((int)i + (int)(b * block));
                int16_t actual;
                memcpy(&actual, got + i * 2, 2);
                if (actual != expect) return 16;
            }
        }

        tpt_frame_free(out);
        tpt_packet_free(p);
    }

    {
        TptPacket *p = 0;
        if (tpt_encoder_flush(enc, &p) != TPT_OK) return 17;
        if (p != 0) return 18; /* audio encoders buffer nothing */
        TptFrame *f = 0;
        if (tpt_decoder_flush(dec, &f) != TPT_OK) return 19;
        if (f != 0) return 20;
    }

    tpt_encoder_free(enc);
    tpt_decoder_free(dec);
    return 0;
}

/* NULL-argument rejection from C: every pointer-validated entry point
 * must answer with TPT_ERR_NULL_ARGUMENT, never dereference. */
int32_t tpt_c_integration_null_arguments(void) {
    if (tpt_encoder_new(tpt_codec_id('A', 'U', 'R', 'A'), 0, 0) !=
        TPT_ERR_NULL_ARGUMENT)
        return 1;
    if (tpt_decoder_decode(0, 0, 0) != TPT_ERR_NULL_ARGUMENT) return 2;
    if (tpt_frame_new(48000, 1, 1024, 0, 1, 48000, 0) != TPT_ERR_NULL_ARGUMENT)
        return 3;
    tpt_frame_free(0);
    tpt_packet_free(0);
    tpt_encoder_free(0);
    tpt_decoder_free(0);
    return 0;
}
