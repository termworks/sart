use super::state::{Mode, TextError, validate_display_text};
use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};

pub const MAGIC: [u8; 4] = *b"BART";
pub const PROTOCOL_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 24;
pub const MAX_PAYLOAD_LEN: usize = 8 * 1024;
pub const MAX_FRAME_LEN: usize = HEADER_LEN + MAX_PAYLOAD_LEN;

pub const MAX_STATUS_LEN: usize = 2 * 1024;
pub const MAX_MESSAGE_LEN: usize = 4 * 1024;
pub const MAX_PATH_LEN: usize = 4 * 1024;
pub const MAX_ERROR_LEN: usize = 2 * 1024;

pub const FLAG_RETAIN_SPLASH: u32 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Opcode {
    Ping = 0x0001,
    Show = 0x0002,
    Hide = 0x0003,
    Status = 0x0004,
    Progress = 0x0005,
    Message = 0x0006,
    HideMessage = 0x0007,
    DetailsShow = 0x0008,
    DetailsHide = 0x0009,
    DetailsToggle = 0x000a,
    Deactivate = 0x000b,
    Reactivate = 0x000c,
    SetMode = 0x000d,
    UpdateRootFs = 0x000e,
    State = 0x000f,
    Quit = 0x0010,
    /// Ask whether the native password listener and coordinator are active.
    NativeReady = 0x0011,

    Ack = 0x8000,
    Error = 0x8001,
    Pong = 0x8002,
    StateResult = 0x8003,
}

impl Opcode {
    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

impl TryFrom<u16> for Opcode {
    type Error = ProtocolError;

    fn try_from(value: u16) -> Result<Self, ProtocolError> {
        match value {
            0x0001 => Ok(Self::Ping),
            0x0002 => Ok(Self::Show),
            0x0003 => Ok(Self::Hide),
            0x0004 => Ok(Self::Status),
            0x0005 => Ok(Self::Progress),
            0x0006 => Ok(Self::Message),
            0x0007 => Ok(Self::HideMessage),
            0x0008 => Ok(Self::DetailsShow),
            0x0009 => Ok(Self::DetailsHide),
            0x000a => Ok(Self::DetailsToggle),
            0x000b => Ok(Self::Deactivate),
            0x000c => Ok(Self::Reactivate),
            0x000d => Ok(Self::SetMode),
            0x000e => Ok(Self::UpdateRootFs),
            0x000f => Ok(Self::State),
            0x0010 => Ok(Self::Quit),
            0x0011 => Ok(Self::NativeReady),
            0x8000 => Ok(Self::Ack),
            0x8001 => Ok(Self::Error),
            0x8002 => Ok(Self::Pong),
            0x8003 => Ok(Self::StateResult),
            _ => Err(ProtocolError::UnknownOpcode(value)),
        }
    }
}

/// One complete control request or response.
///
/// Fields are private so every constructible `Frame` has already passed the
/// same length, flag, UTF-8, and terminal-text checks as a decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    version: u16,
    opcode: Opcode,
    flags: u32,
    request_id: u64,
    payload: Vec<u8>,
}

impl Frame {
    pub fn new(
        opcode: Opcode,
        flags: u32,
        request_id: u64,
        payload: impl Into<Vec<u8>>,
    ) -> Result<Self, ProtocolError> {
        let payload = payload.into();
        validate_frame_fields(opcode, flags, &payload)?;
        Ok(Self {
            version: PROTOCOL_VERSION,
            opcode,
            flags,
            request_id,
            payload,
        })
    }

    pub fn empty(opcode: Opcode, request_id: u64) -> Result<Self, ProtocolError> {
        Self::new(opcode, 0, request_id, Vec::new())
    }

    pub fn text(
        opcode: Opcode,
        request_id: u64,
        text: impl Into<String>,
    ) -> Result<Self, ProtocolError> {
        Self::new(opcode, 0, request_id, text.into().into_bytes())
    }

    pub fn progress(request_id: u64, percent: u8) -> Result<Self, ProtocolError> {
        Self::new(Opcode::Progress, 0, request_id, vec![percent])
    }

    pub fn mode(request_id: u64, mode: Mode) -> Result<Self, ProtocolError> {
        Self::new(Opcode::SetMode, 0, request_id, vec![encode_mode(mode)])
    }

    pub fn quit(request_id: u64, retain_splash: bool) -> Result<Self, ProtocolError> {
        let flags = if retain_splash { FLAG_RETAIN_SPLASH } else { 0 };
        Self::new(Opcode::Quit, flags, request_id, Vec::new())
    }

    pub fn ack(request_id: u64) -> Self {
        Self::empty(Opcode::Ack, request_id).expect("empty acknowledgement is valid")
    }

    pub fn error(request_id: u64, message: impl Into<String>) -> Result<Self, ProtocolError> {
        Self::text(Opcode::Error, request_id, message)
    }

    pub fn pong(request_id: u64) -> Self {
        Self::empty(Opcode::Pong, request_id).expect("empty pong is valid")
    }

    pub fn state_result(request_id: u64, json: impl Into<String>) -> Result<Self, ProtocolError> {
        Self::text(Opcode::StateResult, request_id, json)
    }

    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn opcode(&self) -> Opcode {
        self.opcode
    }

    pub fn flags(&self) -> u32 {
        self.flags
    }

    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn payload_text(&self) -> Result<&str, ProtocolError> {
        std::str::from_utf8(&self.payload).map_err(|error| ProtocolError::InvalidUtf8 {
            opcode: self.opcode,
            valid_up_to: error.valid_up_to(),
        })
    }

    pub fn progress_value(&self) -> Option<u8> {
        (self.opcode == Opcode::Progress).then(|| self.payload[0])
    }

    pub fn mode_value(&self) -> Option<Mode> {
        (self.opcode == Opcode::SetMode)
            .then(|| decode_mode(self.payload[0]).expect("validated mode payload always decodes"))
    }

    pub fn retains_splash(&self) -> bool {
        self.opcode == Opcode::Quit && self.flags & FLAG_RETAIN_SPLASH != 0
    }

    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(self.encoded_len());
        encoded.extend_from_slice(&MAGIC);
        encoded.extend_from_slice(&self.version.to_be_bytes());
        encoded.extend_from_slice(&self.opcode.as_u16().to_be_bytes());
        encoded.extend_from_slice(&self.flags.to_be_bytes());
        encoded.extend_from_slice(&self.request_id.to_be_bytes());
        encoded.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&self.payload);
        encoded
    }

    /// Decodes exactly one frame from an already bounded message buffer.
    ///
    /// This is the preferred entry point for datagram-like boundaries. It
    /// rejects both partial data and concatenated/trailing frames.
    pub fn decode_exact(encoded: &[u8]) -> Result<Self, ProtocolError> {
        if encoded.len() < HEADER_LEN {
            return Err(ProtocolError::Truncated {
                expected: HEADER_LEN,
                actual: encoded.len(),
            });
        }

        let header = parse_header(&encoded[..HEADER_LEN])?;
        let expected = HEADER_LEN + header.payload_len;
        if encoded.len() < expected {
            return Err(ProtocolError::Truncated {
                expected,
                actual: encoded.len(),
            });
        }
        if encoded.len() > expected {
            return Err(ProtocolError::TrailingBytes {
                expected,
                actual: encoded.len(),
            });
        }

        Self::from_header_and_payload(header, encoded[HEADER_LEN..].to_vec())
    }

    /// Reads one frame without reading beyond its declared payload.
    ///
    /// Persistent stream users can call this repeatedly. If a connection is
    /// defined to carry exactly one message, use [`Self::read_exact_message`]
    /// so trailing bytes are rejected too.
    pub fn read_from(reader: &mut impl Read) -> Result<Self, ProtocolError> {
        let mut encoded_header = [0_u8; HEADER_LEN];
        let read = read_fully(reader, &mut encoded_header)?;
        if read != HEADER_LEN {
            return Err(ProtocolError::Truncated {
                expected: HEADER_LEN,
                actual: read,
            });
        }

        let header = parse_header(&encoded_header)?;
        let mut payload = vec![0_u8; header.payload_len];
        let read = read_fully(reader, &mut payload)?;
        if read != header.payload_len {
            return Err(ProtocolError::Truncated {
                expected: HEADER_LEN + header.payload_len,
                actual: HEADER_LEN + read,
            });
        }

        Self::from_header_and_payload(header, payload)
    }

    pub fn read_exact_message(reader: &mut impl Read) -> Result<Self, ProtocolError> {
        let frame = Self::read_from(reader)?;
        let mut trailing = [0_u8; 1];
        let trailing_len = read_fully(reader, &mut trailing)?;
        if trailing_len != 0 {
            return Err(ProtocolError::TrailingBytes {
                expected: frame.encoded_len(),
                actual: frame.encoded_len() + trailing_len,
            });
        }
        Ok(frame)
    }

    pub fn write_to(&self, writer: &mut impl Write) -> Result<(), ProtocolError> {
        writer
            .write_all(&self.encode())
            .map_err(|error| ProtocolError::Io(error.kind()))
    }

    fn from_header_and_payload(
        header: DecodedHeader,
        payload: Vec<u8>,
    ) -> Result<Self, ProtocolError> {
        validate_frame_fields(header.opcode, header.flags, &payload)?;
        Ok(Self {
            version: PROTOCOL_VERSION,
            opcode: header.opcode,
            flags: header.flags,
            request_id: header.request_id,
            payload,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct DecodedHeader {
    opcode: Opcode,
    flags: u32,
    request_id: u64,
    payload_len: usize,
}

fn parse_header(encoded: &[u8]) -> Result<DecodedHeader, ProtocolError> {
    debug_assert_eq!(encoded.len(), HEADER_LEN);

    let magic: [u8; 4] = encoded[0..4]
        .try_into()
        .expect("the checked header contains a complete magic value");
    if magic != MAGIC {
        return Err(ProtocolError::InvalidMagic(magic));
    }

    let version = u16::from_be_bytes(
        encoded[4..6]
            .try_into()
            .expect("the checked header contains a complete version"),
    );
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(version));
    }

    let opcode = Opcode::try_from(u16::from_be_bytes(
        encoded[6..8]
            .try_into()
            .expect("the checked header contains a complete opcode"),
    ))?;
    let flags = u32::from_be_bytes(
        encoded[8..12]
            .try_into()
            .expect("the checked header contains complete flags"),
    );
    let request_id = u64::from_be_bytes(
        encoded[12..20]
            .try_into()
            .expect("the checked header contains a complete request id"),
    );
    let payload_len = u32::from_be_bytes(
        encoded[20..24]
            .try_into()
            .expect("the checked header contains a complete payload length"),
    ) as usize;

    if payload_len > MAX_PAYLOAD_LEN {
        return Err(ProtocolError::PayloadTooLarge {
            length: payload_len,
            maximum: MAX_PAYLOAD_LEN,
        });
    }

    Ok(DecodedHeader {
        opcode,
        flags,
        request_id,
        payload_len,
    })
}

fn validate_frame_fields(opcode: Opcode, flags: u32, payload: &[u8]) -> Result<(), ProtocolError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(ProtocolError::PayloadTooLarge {
            length: payload.len(),
            maximum: MAX_PAYLOAD_LEN,
        });
    }

    let permitted_flags = match opcode {
        Opcode::Quit => FLAG_RETAIN_SPLASH,
        _ => 0,
    };
    if flags & !FLAG_RETAIN_SPLASH != 0 {
        return Err(ProtocolError::UnknownFlags(flags & !FLAG_RETAIN_SPLASH));
    }
    if flags & !permitted_flags != 0 {
        return Err(ProtocolError::FlagsNotAllowed { opcode, flags });
    }

    match opcode {
        Opcode::Ping
        | Opcode::Show
        | Opcode::Hide
        | Opcode::DetailsShow
        | Opcode::DetailsHide
        | Opcode::DetailsToggle
        | Opcode::Deactivate
        | Opcode::Reactivate
        | Opcode::State
        | Opcode::Quit
        | Opcode::NativeReady
        | Opcode::Ack
        | Opcode::Pong => require_length(opcode, payload, 0),
        Opcode::Status => validate_text_payload(opcode, payload, MAX_STATUS_LEN, true).map(|_| ()),
        Opcode::Progress => {
            require_length(opcode, payload, 1)?;
            if payload[0] > 100 {
                Err(ProtocolError::InvalidProgress(payload[0]))
            } else {
                Ok(())
            }
        }
        Opcode::Message => {
            validate_text_payload(opcode, payload, MAX_MESSAGE_LEN, false).map(|_| ())
        }
        Opcode::HideMessage => {
            validate_text_payload(opcode, payload, MAX_MESSAGE_LEN, true).map(|_| ())
        }
        Opcode::SetMode => {
            require_length(opcode, payload, 1)?;
            decode_mode(payload[0]).map(|_| ())
        }
        Opcode::UpdateRootFs => {
            let path = validate_text_payload(opcode, payload, MAX_PATH_LEN, false)?;
            if !path.starts_with('/') {
                return Err(ProtocolError::InvalidRootPath);
            }
            Ok(())
        }
        Opcode::Error => validate_text_payload(opcode, payload, MAX_ERROR_LEN, false).map(|_| ()),
        Opcode::StateResult => {
            validate_text_payload(opcode, payload, MAX_PAYLOAD_LEN, false).map(|_| ())
        }
    }
}

fn require_length(opcode: Opcode, payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(ProtocolError::InvalidPayloadLength {
            opcode,
            expected,
            actual: payload.len(),
        })
    }
}

fn validate_text_payload(
    opcode: Opcode,
    payload: &[u8],
    maximum: usize,
    allow_empty: bool,
) -> Result<&str, ProtocolError> {
    if payload.len() > maximum {
        return Err(ProtocolError::TextTooLong {
            opcode,
            length: payload.len(),
            maximum,
        });
    }
    if payload.is_empty() && !allow_empty {
        return Err(ProtocolError::EmptyText(opcode));
    }

    let text = std::str::from_utf8(payload).map_err(|error| ProtocolError::InvalidUtf8 {
        opcode,
        valid_up_to: error.valid_up_to(),
    })?;
    validate_display_text(text, maximum)
        .map_err(|error| ProtocolError::InvalidText { opcode, error })?;
    Ok(text)
}

fn encode_mode(mode: Mode) -> u8 {
    match mode {
        Mode::Boot => 0,
        Mode::Shutdown => 1,
        Mode::Reboot => 2,
        Mode::Update => 3,
        Mode::Upgrade => 4,
    }
}

fn decode_mode(value: u8) -> Result<Mode, ProtocolError> {
    match value {
        0 => Ok(Mode::Boot),
        1 => Ok(Mode::Shutdown),
        2 => Ok(Mode::Reboot),
        3 => Ok(Mode::Update),
        4 => Ok(Mode::Upgrade),
        _ => Err(ProtocolError::InvalidMode(value)),
    }
}

fn read_fully(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, ProtocolError> {
    let mut offset = 0;
    while offset < buffer.len() {
        match reader.read(&mut buffer[offset..]) {
            Ok(0) => break,
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ProtocolError::Io(error.kind())),
        }
    }
    Ok(offset)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Truncated {
        expected: usize,
        actual: usize,
    },
    TrailingBytes {
        expected: usize,
        actual: usize,
    },
    InvalidMagic([u8; 4]),
    UnsupportedVersion(u16),
    UnknownOpcode(u16),
    UnknownFlags(u32),
    FlagsNotAllowed {
        opcode: Opcode,
        flags: u32,
    },
    PayloadTooLarge {
        length: usize,
        maximum: usize,
    },
    InvalidPayloadLength {
        opcode: Opcode,
        expected: usize,
        actual: usize,
    },
    TextTooLong {
        opcode: Opcode,
        length: usize,
        maximum: usize,
    },
    EmptyText(Opcode),
    InvalidUtf8 {
        opcode: Opcode,
        valid_up_to: usize,
    },
    InvalidText {
        opcode: Opcode,
        error: TextError,
    },
    InvalidProgress(u8),
    InvalidMode(u8),
    InvalidRootPath,
    Io(io::ErrorKind),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { expected, actual } => write!(
                formatter,
                "truncated frame: expected {expected} bytes, received {actual}"
            ),
            Self::TrailingBytes { expected, actual } => write!(
                formatter,
                "trailing frame bytes: expected {expected} bytes, received {actual}"
            ),
            Self::InvalidMagic(value) => write!(formatter, "invalid protocol magic {value:?}"),
            Self::UnsupportedVersion(value) => {
                write!(formatter, "unsupported protocol version {value}")
            }
            Self::UnknownOpcode(value) => write!(formatter, "unknown opcode {value:#06x}"),
            Self::UnknownFlags(value) => write!(formatter, "unknown protocol flags {value:#010x}"),
            Self::FlagsNotAllowed { opcode, flags } => {
                write!(
                    formatter,
                    "flags {flags:#010x} are not valid for {opcode:?}"
                )
            }
            Self::PayloadTooLarge { length, maximum } => write!(
                formatter,
                "payload is {length} bytes; protocol maximum is {maximum}"
            ),
            Self::InvalidPayloadLength {
                opcode,
                expected,
                actual,
            } => write!(
                formatter,
                "{opcode:?} payload is {actual} bytes; expected {expected}"
            ),
            Self::TextTooLong {
                opcode,
                length,
                maximum,
            } => write!(
                formatter,
                "{opcode:?} text is {length} bytes; maximum is {maximum}"
            ),
            Self::EmptyText(opcode) => write!(formatter, "{opcode:?} text must not be empty"),
            Self::InvalidUtf8 {
                opcode,
                valid_up_to,
            } => write!(
                formatter,
                "{opcode:?} payload is not UTF-8 at byte {valid_up_to}"
            ),
            Self::InvalidText { opcode, error } => {
                write!(formatter, "invalid {opcode:?} text: {error}")
            }
            Self::InvalidProgress(value) => {
                write!(formatter, "progress {value} is outside 0..=100")
            }
            Self::InvalidMode(value) => write!(formatter, "unknown presentation mode {value}"),
            Self::InvalidRootPath => write!(formatter, "root path must be absolute"),
            Self::Io(kind) => write!(formatter, "protocol I/O error: {kind:?}"),
        }
    }
}

impl Error for ProtocolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidText { error, .. } => Some(error),
            _ => None,
        }
    }
}
