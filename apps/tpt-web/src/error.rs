//! The error type shared by the parser, the player and the C ABI.

use core::fmt;
use tpt_container::TptError;

/// Why the container bytes were rejected: a copyable, comparable mirror of
/// [`TptError`] (which is neither `Clone` nor `PartialEq`, and a terminal
/// error has to be both, so the player can keep returning it).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContainerFault {
    /// The stream ended inside the header or a chunk.
    Truncated,
    /// The file header is missing or malformed (wrong magic, bad size).
    BadHeader,
    /// A chunk is malformed.
    BadChunk,
    /// The container version is newer than this reader.
    UnsupportedVersion,
    /// Any other container-level fault.
    Other(&'static str),
}

impl fmt::Display for ContainerFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContainerFault::Truncated => f.write_str("truncated"),
            ContainerFault::BadHeader => f.write_str("bad header"),
            ContainerFault::BadChunk => f.write_str("bad chunk"),
            ContainerFault::UnsupportedVersion => f.write_str("unsupported container version"),
            ContainerFault::Other(m) => f.write_str(m),
        }
    }
}

/// Everything that can go wrong while feeding or pulling from a
/// [`crate::Player`].
///
/// Only [`WebError::InputFull`] is *retryable*: it means "pump, drain the
/// output queues, then push again". Every other variant is terminal for the
/// stream: the player is in [`crate::Status::Failed`] and returns the same
/// error until it is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebError {
    /// The container is malformed (bad header, bad chunk, unsupported
    /// version).
    Container(ContainerFault),
    /// A hard memory / size limit (todo.md section 19) was exceeded. The
    /// message names it.
    Limit(&'static str),
    /// The input buffer is at its limit; nothing was consumed. Retryable.
    InputFull,
    /// A codec refused the stream (bad private data, unsupported geometry).
    Codec(String),
    /// The stream is well formed but nothing in it can be played, or it uses
    /// a feature this player does not implement.
    Unsupported(String),
    /// More than `max_decode_errors` packets failed to decode.
    TooManyDecodeErrors,
    /// Input was pushed after [`crate::Player::finish_input`].
    InputClosed,
    /// A caller-supplied argument was invalid (used by the C ABI).
    BadArgument(&'static str),
}

impl WebError {
    /// Stable numeric code used by the C ABI (`0` and up are statuses, so
    /// every error is negative).
    pub fn code(&self) -> i32 {
        match self {
            WebError::BadArgument(_) => -7,
            WebError::InputFull => -2,
            WebError::Container(_) => -3,
            WebError::Limit(_) => -4,
            WebError::Unsupported(_) => -5,
            WebError::Codec(_) | WebError::TooManyDecodeErrors | WebError::InputClosed => -6,
        }
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WebError::Container(e) => write!(f, "malformed .tpt container: {e}"),
            WebError::Limit(what) => write!(f, "resource limit exceeded: {what}"),
            WebError::InputFull => f.write_str("input buffer full; pump and retry"),
            WebError::Codec(m) => write!(f, "codec error: {m}"),
            WebError::Unsupported(m) => write!(f, "unsupported: {m}"),
            WebError::TooManyDecodeErrors => f.write_str("too many undecodable packets"),
            WebError::InputClosed => f.write_str("input already finished"),
            WebError::BadArgument(m) => write!(f, "bad argument: {m}"),
        }
    }
}

impl std::error::Error for WebError {}

impl From<TptError> for WebError {
    fn from(e: TptError) -> Self {
        WebError::Container(match e {
            TptError::ResourceExceeded => return WebError::Limit("container chunk"),
            TptError::Truncated => ContainerFault::Truncated,
            TptError::BadHeader => ContainerFault::BadHeader,
            TptError::BadChunk => ContainerFault::BadChunk,
            TptError::UnsupportedVersion => ContainerFault::UnsupportedVersion,
            TptError::Other(m) => ContainerFault::Other(m),
        })
    }
}
