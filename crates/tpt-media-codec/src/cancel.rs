//! Cooperative cancellation for codecs.
//!
//! A [`CancelToken`] is a shared one-way flag. A caller holds a clone and
//! calls [`CancelToken::cancel`] from any thread; a codec wrapped in one of
//! the `Cancellable*` adapters below checks the flag before every
//! `decode`/`encode`/`flush`/`drain` call and returns
//! [`CodecError::Cancelled`] once it is set.
//!
//! The granularity is one call. Each call is already bounded by the
//! structural limits in [`ResourceLimits`](crate::ResourceLimits) (see
//! `docs/codec-design.md`), so a cancelled wrapper stops at the next packet
//! or frame boundary and never interrupts a call midway. There is no
//! wall-clock deadline here: a time fuse would make decoding depend on the
//! machine, which the decoder contract (§39) rules out.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use tpt_media_core::Packet;

use crate::{
    CodecError, CodecInfo, CodecResult, Decoder, DynDecoder, DynEncoder, DynFrame, Encoder,
};

/// A shared, one-way cancellation flag.
///
/// Clones observe the same flag. Once cancelled it stays cancelled; there
/// is no reset, so a fresh job needs a fresh token.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// A token that is not yet cancelled.
    pub fn new() -> Self {
        CancelToken::default()
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Whether [`CancelToken::cancel`] has been called on any clone.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    fn check(&self) -> CodecResult<()> {
        if self.is_cancelled() {
            Err(CodecError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// A [`Decoder`] that refuses to run once its [`CancelToken`] is set.
pub struct CancellableDecoder<D> {
    inner: D,
    token: CancelToken,
}

impl<D> CancellableDecoder<D> {
    /// Wrap `inner` so it observes `token`.
    pub fn new(inner: D, token: CancelToken) -> Self {
        CancellableDecoder { inner, token }
    }

    /// The wrapped decoder.
    pub fn get_ref(&self) -> &D {
        &self.inner
    }

    /// Unwrap, discarding the token.
    pub fn into_inner(self) -> D {
        self.inner
    }
}

impl<D: Decoder> Decoder for CancellableDecoder<D> {
    type Frame = D::Frame;

    fn info(&self) -> &CodecInfo {
        self.inner.info()
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<Self::Frame>> {
        self.token.check()?;
        self.inner.decode(packet)
    }

    fn flush(&mut self) -> CodecResult<Vec<Self::Frame>> {
        self.token.check()?;
        self.inner.flush()
    }

    fn drain(&mut self) -> CodecResult<Vec<Self::Frame>> {
        self.token.check()?;
        self.inner.drain()
    }
}

/// An [`Encoder`] that refuses to run once its [`CancelToken`] is set.
pub struct CancellableEncoder<E> {
    inner: E,
    token: CancelToken,
}

impl<E> CancellableEncoder<E> {
    /// Wrap `inner` so it observes `token`.
    pub fn new(inner: E, token: CancelToken) -> Self {
        CancellableEncoder { inner, token }
    }

    /// The wrapped encoder.
    pub fn get_ref(&self) -> &E {
        &self.inner
    }

    /// Unwrap, discarding the token.
    pub fn into_inner(self) -> E {
        self.inner
    }
}

impl<E: Encoder> Encoder for CancellableEncoder<E> {
    type Frame = E::Frame;

    fn info(&self) -> &CodecInfo {
        self.inner.info()
    }

    fn encode(&mut self, frame: &Self::Frame) -> CodecResult<Vec<Packet>> {
        self.token.check()?;
        self.inner.encode(frame)
    }

    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        self.token.check()?;
        self.inner.flush()
    }

    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        self.token.check()?;
        self.inner.drain()
    }
}

/// A type-erased decoder that refuses to run once its [`CancelToken`] is set.
///
/// Use this to cancel a decoder obtained from a [`Registry`](crate::Registry).
pub struct CancellableDynDecoder {
    inner: Box<dyn DynDecoder>,
    token: CancelToken,
}

impl CancellableDynDecoder {
    /// Wrap `inner` so it observes `token`.
    pub fn new(inner: Box<dyn DynDecoder>, token: CancelToken) -> Self {
        CancellableDynDecoder { inner, token }
    }
}

impl DynDecoder for CancellableDynDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<DynFrame>> {
        self.token.check()?;
        self.inner.decode(packet)
    }

    fn flush(&mut self) -> CodecResult<Vec<DynFrame>> {
        self.token.check()?;
        self.inner.flush()
    }

    fn drain(&mut self) -> CodecResult<Vec<DynFrame>> {
        self.token.check()?;
        self.inner.drain()
    }

    fn info(&self) -> CodecInfo {
        self.inner.info()
    }
}

/// A type-erased encoder that refuses to run once its [`CancelToken`] is set.
pub struct CancellableDynEncoder {
    inner: Box<dyn DynEncoder>,
    token: CancelToken,
}

impl CancellableDynEncoder {
    /// Wrap `inner` so it observes `token`.
    pub fn new(inner: Box<dyn DynEncoder>, token: CancelToken) -> Self {
        CancellableDynEncoder { inner, token }
    }
}

impl DynEncoder for CancellableDynEncoder {
    fn encode(&mut self, frame: &DynFrame) -> CodecResult<Vec<Packet>> {
        self.token.check()?;
        self.inner.encode(frame)
    }

    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        self.token.check()?;
        self.inner.flush()
    }

    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        self.token.check()?;
        self.inner.drain()
    }

    fn info(&self) -> CodecInfo {
        self.inner.info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Capabilities;
    use tpt_media_core::{Timebase, Timestamp};

    fn info() -> CodecInfo {
        CodecInfo {
            name: "counting",
            codec_id: *b"TEST",
            version: 1,
            capabilities: Capabilities::default(),
            supported_formats: Vec::new(),
        }
    }

    fn assert_cancelled<T: core::fmt::Debug>(result: CodecResult<T>) {
        assert!(matches!(result, Err(CodecError::Cancelled)), "{result:?}");
    }

    fn packet() -> Packet {
        Packet::empty(0, Timestamp::zero(Timebase::MILLISECONDS))
    }

    /// Counts calls so a test can prove a refused call never reached it.
    struct Counting {
        info: CodecInfo,
        calls: u32,
    }

    impl Counting {
        fn new() -> Self {
            Counting {
                info: info(),
                calls: 0,
            }
        }
    }

    impl Decoder for Counting {
        type Frame = u8;
        fn info(&self) -> &CodecInfo {
            &self.info
        }
        fn decode(&mut self, _packet: &Packet) -> CodecResult<Vec<u8>> {
            self.calls += 1;
            Ok(alloc::vec![1])
        }
        fn flush(&mut self) -> CodecResult<Vec<u8>> {
            self.calls += 1;
            Ok(Vec::new())
        }
        fn drain(&mut self) -> CodecResult<Vec<u8>> {
            self.calls += 1;
            Ok(Vec::new())
        }
    }

    impl Encoder for Counting {
        type Frame = u8;
        fn info(&self) -> &CodecInfo {
            &self.info
        }
        fn encode(&mut self, _frame: &u8) -> CodecResult<Vec<Packet>> {
            self.calls += 1;
            Ok(alloc::vec![packet()])
        }
        fn flush(&mut self) -> CodecResult<Vec<Packet>> {
            self.calls += 1;
            Ok(Vec::new())
        }
        fn drain(&mut self) -> CodecResult<Vec<Packet>> {
            self.calls += 1;
            Ok(Vec::new())
        }
    }

    #[test]
    fn new_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn clones_share_one_flag_and_cancel_is_idempotent() {
        let token = CancelToken::new();
        let other = token.clone();
        other.cancel();
        other.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn uncancelled_decoder_passes_every_call_through() {
        let mut d = CancellableDecoder::new(Counting::new(), CancelToken::new());
        assert_eq!(d.decode(&packet()).unwrap(), alloc::vec![1]);
        assert!(d.flush().unwrap().is_empty());
        assert!(d.drain().unwrap().is_empty());
        assert_eq!(d.get_ref().calls, 3);
    }

    #[test]
    fn cancelled_decoder_refuses_every_call_without_reaching_the_codec() {
        let token = CancelToken::new();
        let mut d = CancellableDecoder::new(Counting::new(), token.clone());
        d.decode(&packet()).unwrap();
        token.cancel();
        assert_eq!(d.decode(&packet()), Err(CodecError::Cancelled));
        assert_eq!(d.flush(), Err(CodecError::Cancelled));
        assert_eq!(d.drain(), Err(CodecError::Cancelled));
        assert_eq!(d.into_inner().calls, 1);
    }

    #[test]
    fn cancelled_encoder_refuses_every_call_without_reaching_the_codec() {
        let token = CancelToken::new();
        let mut e = CancellableEncoder::new(Counting::new(), token.clone());
        assert_eq!(e.encode(&0).unwrap().len(), 1);
        token.cancel();
        assert_eq!(e.encode(&0), Err(CodecError::Cancelled));
        assert_eq!(e.flush(), Err(CodecError::Cancelled));
        assert_eq!(e.drain(), Err(CodecError::Cancelled));
        assert_eq!(e.get_ref().calls, 1);
    }

    #[test]
    fn info_is_forwarded_even_after_cancellation() {
        let token = CancelToken::new();
        let d = CancellableDecoder::new(Counting::new(), token.clone());
        token.cancel();
        assert_eq!(Decoder::info(&d).name, "counting");
    }

    struct DynCounting(Counting);

    impl DynDecoder for DynCounting {
        fn decode(&mut self, _packet: &Packet) -> CodecResult<Vec<DynFrame>> {
            self.0.calls += 1;
            Ok(Vec::new())
        }
        fn flush(&mut self) -> CodecResult<Vec<DynFrame>> {
            self.0.calls += 1;
            Ok(Vec::new())
        }
        fn drain(&mut self) -> CodecResult<Vec<DynFrame>> {
            self.0.calls += 1;
            Ok(Vec::new())
        }
        fn info(&self) -> CodecInfo {
            self.0.info.clone()
        }
    }

    impl DynEncoder for DynCounting {
        fn encode(&mut self, _frame: &DynFrame) -> CodecResult<Vec<Packet>> {
            self.0.calls += 1;
            Ok(Vec::new())
        }
        fn flush(&mut self) -> CodecResult<Vec<Packet>> {
            self.0.calls += 1;
            Ok(Vec::new())
        }
        fn drain(&mut self) -> CodecResult<Vec<Packet>> {
            self.0.calls += 1;
            Ok(Vec::new())
        }
        fn info(&self) -> CodecInfo {
            self.0.info.clone()
        }
    }

    #[test]
    fn dyn_decoder_refuses_once_cancelled() {
        let token = CancelToken::new();
        let mut d =
            CancellableDynDecoder::new(Box::new(DynCounting(Counting::new())), token.clone());
        assert!(d.decode(&packet()).is_ok());
        token.cancel();
        assert_cancelled(d.decode(&packet()));
        assert_cancelled(d.flush());
        assert_cancelled(d.drain());
        assert_eq!(d.info().codec_id, *b"TEST");
    }

    #[test]
    fn dyn_encoder_refuses_once_cancelled() {
        let token = CancelToken::new();
        let mut e =
            CancellableDynEncoder::new(Box::new(DynCounting(Counting::new())), token.clone());
        let frame = DynFrame::Video {
            pixel_format: tpt_media_core::PixelFormat::Gray8,
            width: 0,
            height: 0,
            stride: 0,
            pixels: Vec::new(),
        };
        assert!(e.encode(&frame).is_ok());
        token.cancel();
        assert_cancelled(e.encode(&frame));
        assert_cancelled(e.flush());
        assert_cancelled(e.drain());
    }
}
