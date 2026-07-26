use bootart::splash::protocol::{
    FLAG_RETAIN_SPLASH, Frame, HEADER_LEN, MAGIC, MAX_MESSAGE_LEN, MAX_PAYLOAD_LEN, Opcode,
    PROTOCOL_VERSION, ProtocolError,
};
use bootart::splash::state::{Mode, TextError};
use std::io::Cursor;

fn raw_frame(version: u16, opcode: u16, flags: u32, request_id: u64, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&version.to_be_bytes());
    bytes.extend_from_slice(&opcode.to_be_bytes());
    bytes.extend_from_slice(&flags.to_be_bytes());
    bytes.extend_from_slice(&request_id.to_be_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn frame_header_and_payload_use_network_byte_order() {
    let frame = Frame::text(Opcode::Status, 0x0102_0304_0506_0708, "ready").unwrap();
    let encoded = frame.encode();

    assert_eq!(&encoded[0..4], &MAGIC);
    assert_eq!(&encoded[4..6], &PROTOCOL_VERSION.to_be_bytes());
    assert_eq!(&encoded[6..8], &Opcode::Status.as_u16().to_be_bytes());
    assert_eq!(&encoded[8..12], &0_u32.to_be_bytes());
    assert_eq!(&encoded[12..20], &0x0102_0304_0506_0708_u64.to_be_bytes());
    assert_eq!(&encoded[20..24], &5_u32.to_be_bytes());
    assert_eq!(&encoded[24..], b"ready");
    assert_eq!(Frame::decode_exact(&encoded).unwrap(), frame);
}

#[test]
fn typed_payloads_round_trip() {
    let progress = Frame::progress(8, 73).unwrap();
    assert_eq!(progress.progress_value(), Some(73));
    assert_eq!(Frame::decode_exact(&progress.encode()).unwrap(), progress);

    let mode = Frame::mode(9, Mode::Upgrade).unwrap();
    assert_eq!(mode.mode_value(), Some(Mode::Upgrade));

    let quit = Frame::quit(10, true).unwrap();
    assert_eq!(quit.flags(), FLAG_RETAIN_SPLASH);
    assert!(quit.retains_splash());
}

#[test]
fn truncated_header_and_payload_are_rejected() {
    assert_eq!(
        Frame::decode_exact(b"BAR"),
        Err(ProtocolError::Truncated {
            expected: HEADER_LEN,
            actual: 3,
        })
    );

    let mut encoded = Frame::text(Opcode::Status, 3, "working").unwrap().encode();
    encoded.pop();
    assert_eq!(
        Frame::decode_exact(&encoded),
        Err(ProtocolError::Truncated {
            expected: HEADER_LEN + 7,
            actual: HEADER_LEN + 6,
        })
    );
}

#[test]
fn trailing_or_concatenated_data_is_rejected() {
    let mut encoded = Frame::empty(Opcode::Ping, 11).unwrap().encode();
    encoded.push(0);
    assert_eq!(
        Frame::decode_exact(&encoded),
        Err(ProtocolError::TrailingBytes {
            expected: HEADER_LEN,
            actual: HEADER_LEN + 1,
        })
    );

    let mut cursor = Cursor::new(encoded);
    assert_eq!(
        Frame::read_exact_message(&mut cursor),
        Err(ProtocolError::TrailingBytes {
            expected: HEADER_LEN,
            actual: HEADER_LEN + 1,
        })
    );
}

#[test]
fn unknown_version_and_opcode_are_rejected() {
    let unsupported = raw_frame(PROTOCOL_VERSION + 1, Opcode::Ping.as_u16(), 0, 1, b"");
    assert_eq!(
        Frame::decode_exact(&unsupported),
        Err(ProtocolError::UnsupportedVersion(PROTOCOL_VERSION + 1))
    );

    let unknown = raw_frame(PROTOCOL_VERSION, 0x7777, 0, 1, b"");
    assert_eq!(
        Frame::decode_exact(&unknown),
        Err(ProtocolError::UnknownOpcode(0x7777))
    );
}

#[test]
fn oversize_is_rejected_before_allocation_or_payload_read() {
    let mut header = raw_frame(PROTOCOL_VERSION, Opcode::Status.as_u16(), 0, 1, b"");
    header[20..24].copy_from_slice(&((MAX_PAYLOAD_LEN + 1) as u32).to_be_bytes());

    assert_eq!(
        Frame::decode_exact(&header),
        Err(ProtocolError::PayloadTooLarge {
            length: MAX_PAYLOAD_LEN + 1,
            maximum: MAX_PAYLOAD_LEN,
        })
    );
    assert_eq!(
        Frame::new(Opcode::StateResult, 0, 1, vec![b'x'; MAX_PAYLOAD_LEN + 1]),
        Err(ProtocolError::PayloadTooLarge {
            length: MAX_PAYLOAD_LEN + 1,
            maximum: MAX_PAYLOAD_LEN,
        })
    );
}

#[test]
fn invalid_utf8_and_terminal_controls_are_rejected() {
    let invalid_utf8 = raw_frame(PROTOCOL_VERSION, Opcode::Status.as_u16(), 0, 1, &[0xff]);
    assert!(matches!(
        Frame::decode_exact(&invalid_utf8),
        Err(ProtocolError::InvalidUtf8 {
            opcode: Opcode::Status,
            valid_up_to: 0
        })
    ));

    assert!(matches!(
        Frame::text(Opcode::Message, 1, "hello\u{1b}[2J"),
        Err(ProtocolError::InvalidText {
            opcode: Opcode::Message,
            error: TextError::UnsafeCharacter {
                codepoint: 0x1b,
                ..
            }
        })
    ));
    assert!(matches!(
        Frame::text(Opcode::Status, 1, "line one\nline two"),
        Err(ProtocolError::InvalidText {
            opcode: Opcode::Status,
            ..
        })
    ));
}

#[test]
fn each_text_field_has_an_explicit_limit() {
    assert!(Frame::text(Opcode::Message, 1, "x".repeat(MAX_MESSAGE_LEN)).is_ok());
    assert_eq!(
        Frame::text(Opcode::Message, 1, "x".repeat(MAX_MESSAGE_LEN + 1)),
        Err(ProtocolError::TextTooLong {
            opcode: Opcode::Message,
            length: MAX_MESSAGE_LEN + 1,
            maximum: MAX_MESSAGE_LEN,
        })
    );
}

#[test]
fn opcode_payload_shapes_and_flags_are_validated() {
    assert_eq!(
        Frame::progress(1, 101),
        Err(ProtocolError::InvalidProgress(101))
    );
    assert_eq!(
        Frame::new(Opcode::Ping, 0, 1, b"not empty".to_vec()),
        Err(ProtocolError::InvalidPayloadLength {
            opcode: Opcode::Ping,
            expected: 0,
            actual: 9,
        })
    );
    assert_eq!(
        Frame::new(Opcode::Show, FLAG_RETAIN_SPLASH, 1, Vec::new()),
        Err(ProtocolError::FlagsNotAllowed {
            opcode: Opcode::Show,
            flags: FLAG_RETAIN_SPLASH,
        })
    );
    assert_eq!(
        Frame::text(Opcode::UpdateRootFs, 1, "relative/root"),
        Err(ProtocolError::InvalidRootPath)
    );
}

#[test]
fn bounded_stream_reader_handles_fragmented_reads() {
    struct OneByteReader(Cursor<Vec<u8>>);

    impl std::io::Read for OneByteReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let mut byte = [0_u8; 1];
            let read = std::io::Read::read(&mut self.0, &mut byte)?;
            if read == 1 && !buffer.is_empty() {
                buffer[0] = byte[0];
                Ok(1)
            } else {
                Ok(0)
            }
        }
    }

    let expected = Frame::text(Opcode::Status, 0x1234, "fragmented").unwrap();
    let mut reader = OneByteReader(Cursor::new(expected.encode()));
    assert_eq!(Frame::read_exact_message(&mut reader).unwrap(), expected);
}
