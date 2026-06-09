//! Top-level error type for `echobot-core`.
//!
//! Sub-errors from individual modules are wrapped via dedicated variants so
//! callers can match on the broad category while still preserving the original
//! cause via `source()` (powered by `thiserror`).

use thiserror::Error;

/// Result alias for `echobot-core` operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type aggregating every failure mode the core crate can
/// surface. Sub-errors live alongside the modules that produce them; the
/// `Error` enum simply re-exports them through their respective variants.
#[derive(Debug, Error)]
pub enum Error {
    /// Configuration / environment loading failed.
    #[error(transparent)]
    Config(#[from] ConfigError),

    /// Image normalization (decode/resize/encode) failed.
    #[error(transparent)]
    Image(#[from] ImageError),

    /// Attachment store I/O or metadata failure.
    #[error(transparent)]
    Attachment(#[from] AttachmentError),

    /// Model / content construction failure (e.g. invalid message content).
    #[error(transparent)]
    Model(#[from] ModelError),

    /// Tool-level error (not-found, bad args, execution failure,
    /// safety-policy block, etc.). Used by `echobot_tools` and the
    /// runtime.
    #[error(transparent)]
    Tool(#[from] ToolError),

    /// I/O error (file or network-adjacent).
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Configuration / env loading failures.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A `.env` line could not be parsed.
    #[error("invalid env line {line_number} in {path}: {message}")]
    InvalidEnvLine {
        path: String,
        line_number: usize,
        message: String,
    },

    /// A required environment variable was missing.
    #[error("missing required environment variable: {0}")]
    MissingEnvVar(String),

    /// An environment variable carried an invalid value.
    #[error("invalid value for {name}: {message}")]
    InvalidEnvValue { name: String, message: String },

    /// A log level was set to an unknown value.
    #[error("invalid log level for {name}: {value} (expected one of: {valid})")]
    InvalidLogLevel {
        name: String,
        value: String,
        valid: String,
    },
}

/// Image normalization failures (decode, resize, encode, budget violations).
#[derive(Debug, Error)]
pub enum ImageError {
    /// Bytes were empty.
    #[error("chat image must not be empty")]
    Empty,

    /// Input bytes exceeded `max_input_bytes`.
    #[error(
        "chat image exceeds the upload size limit ({actual} bytes > {limit} bytes)"
    )]
    InputTooLarge { actual: usize, limit: usize },

    /// Image has an invalid (zero) size.
    #[error("chat image must have a valid size")]
    InvalidSize,

    /// Pixels exceed `max_pixels`.
    #[error("chat image exceeds the pixel budget ({width}x{height} > {max} pixels)")]
    PixelBudgetExceeded {
        width: u32,
        height: u32,
        max: u64,
    },

    /// Compressed output still exceeds `max_output_bytes`.
    #[error("chat image exceeds the compressed size limit ({0} bytes)")]
    OutputTooLarge(usize),

    /// The image format could not be decoded.
    #[error("unsupported chat image format: {0}")]
    UnsupportedFormat(String),
}

/// Attachment store failures.
#[derive(Debug, Error)]
pub enum AttachmentError {
    /// The supplied attachment id is empty or contains forbidden characters.
    #[error("attachment id is invalid: {0}")]
    InvalidAttachmentId(String),

    /// No attachment with the requested id was found.
    #[error("attachment not found: {0}")]
    NotFound(String),

    /// The requested id referred to the wrong kind of attachment.
    #[error("attachment is not {expected}: {actual}")]
    WrongKind { expected: String, actual: String },

    /// Metadata on disk was malformed.
    #[error("attachment metadata is invalid: {0}")]
    InvalidMetadata(String),

    /// Metadata is missing required fields.
    #[error("attachment metadata is incomplete: {0}")]
    IncompleteMetadata(String),

    /// The underlying file is missing on disk.
    #[error("attachment file is missing: {0}")]
    FileMissing(String),

    /// The supplied content type was wrong for the requested operation.
    #[error("invalid content type: {0}")]
    InvalidContentType(String),

    /// The supplied file bytes were empty.
    #[error("attachment file must not be empty")]
    EmptyFile,

    /// The supplied file exceeds `FileBudget::max_input_bytes`.
    #[error(
        "attachment file exceeds the upload size limit ({actual} bytes > {limit} bytes)"
    )]
    FileTooLarge { actual: usize, limit: usize },
}

/// Failure building / validating LLM model content.
#[derive(Debug, Error)]
pub enum ModelError {
    /// A content block had an unknown / unsupported `type`.
    #[error("unknown message content block type: {0}")]
    UnknownContentBlockType(String),

    /// A content block was missing a required field (e.g. `text`).
    #[error("message content block missing required field: {0}")]
    MissingField(String),
}

/// Tool-level failures. Re-exported from `echobot-tools` via the
/// `Error::Tool` variant, but defined here so the core `Error` enum can
/// `#[from]`-convert without forcing a hard dependency on the tools
/// crate.
#[derive(Debug, Error)]
pub enum ToolError {
    /// The requested tool name was not registered.
    #[error("tool not found: {0}")]
    NotFound(String),

    /// The supplied tool arguments were not valid JSON or not a JSON
    /// object.
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),

    /// The tool raised an exception during execution.
    #[error("tool '{name}' failed: {message}")]
    Execution { name: String, message: String },

    /// Required argument was missing or empty.
    #[error("missing required argument: {0}")]
    MissingArgument(String),

    /// An argument carried an invalid value.
    #[error("invalid value for {name}: {message}")]
    InvalidValue { name: String, message: String },

    /// Operation not permitted in the current safety mode.
    #[error("{0}")]
    Blocked(String),
}
