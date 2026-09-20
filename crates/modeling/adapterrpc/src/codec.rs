//! Newline-delimited JSON framing, used by both the daemon (requests) and the adapter
//! (responses). Mirrors `codec.go`.

use std::io::{BufRead, Write};

use serde::Serialize;

use crate::error::CodecError;
use crate::types::{Request, Response};

/// Encode `v` as a single newline-delimited JSON frame.
pub fn write_message<T: Serialize>(mut w: impl Write, v: &T) -> Result<(), CodecError> {
    let mut b = serde_json::to_vec(v)?;
    b.push(b'\n');
    w.write_all(&b)?;
    Ok(())
}

/// Decode one request frame. Used by the adapter side.
///
/// Mirrors Go `ReadRequest`: a final frame without a trailing newline still decodes; only
/// an empty read (clean EOF before any bytes) surfaces the io error.
pub fn read_request(r: &mut impl BufRead) -> Result<Request, CodecError> {
    let line = read_frame(r)?;
    Ok(serde_json::from_slice(&line)?)
}

/// Decode one response frame. Used by the daemon side.
pub fn read_response(r: &mut impl BufRead) -> Result<Response, CodecError> {
    let line = read_frame(r)?;
    Ok(serde_json::from_slice(&line)?)
}

fn read_frame(r: &mut impl BufRead) -> Result<Vec<u8>, CodecError> {
    let mut line = Vec::new();
    let n = r.read_until(b'\n', &mut line)?;
    if n == 0 {
        // Go: ReadBytes returns io.EOF with no data.
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof").into());
    }
    trim_newline(&mut line);
    Ok(line)
}

fn trim_newline(b: &mut Vec<u8>) {
    while matches!(b.last(), Some(b'\n') | Some(b'\r')) {
        b.pop();
    }
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;

    use super::*;
    use crate::types::{CONTRACT_VERSION, Status};

    fn sample_request() -> Request {
        Request {
            request_id: "req-1".to_owned(),
            contract_version: CONTRACT_VERSION.to_owned(),
            domain: "calendar".to_owned(),
            operation: "CreateEvent".to_owned(),
            deadline_ms: 30_000,
            resource: Some(
                serde_json::value::to_raw_value(&serde_json::json!({"integrationId":"int-1"}))
                    .unwrap(),
            ),
            credential: None,
            payload: Some(serde_json::value::to_raw_value(&serde_json::json!({"x":1})).unwrap()),
        }
    }

    #[test]
    fn write_then_read_request_round_trip() {
        let req = sample_request();
        let mut buf = Vec::new();
        write_message(&mut buf, &req).unwrap();
        assert!(buf.ends_with(b"\n"));
        let mut r = BufReader::new(&buf[..]);
        let got = read_request(&mut r).unwrap();
        assert_eq!(got.request_id, "req-1");
        assert_eq!(got.deadline_ms, 30_000);
        assert!(got.credential.is_none());
        // omitempty: credential absent from the frame entirely.
        let frame = std::str::from_utf8(&buf).unwrap();
        assert!(!frame.contains("credential"), "frame: {frame}");
        assert!(frame.contains("\"requestId\":\"req-1\""));
        assert!(frame.contains("\"deadlineMs\":30000"));
    }

    #[test]
    fn write_then_read_response_round_trip() {
        let resp = Response {
            request_id: "req-2".to_owned(),
            contract_version: CONTRACT_VERSION.to_owned(),
            status: Status::Ok,
            failure_kind: None,
            payload: Some(serde_json::value::to_raw_value(&serde_json::json!({"a":true})).unwrap()),
            diagnostic: None,
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &resp).unwrap();
        let mut r = BufReader::new(&buf[..]);
        let got = read_response(&mut r).unwrap();
        assert_eq!(got.status, Status::Ok);
        let frame = std::str::from_utf8(&buf).unwrap();
        assert!(!frame.contains("failureKind"), "frame: {frame}");
        assert!(!frame.contains("diagnostic"), "frame: {frame}");
    }

    #[test]
    fn read_trims_crlf() {
        let frame = b"{\"requestId\":\"req-3\",\"contractVersion\":\"1\",\"domain\":\"d\",\"operation\":\"o\",\"deadlineMs\":1}\r\n";
        let mut r = BufReader::new(&frame[..]);
        let got = read_request(&mut r).unwrap();
        assert_eq!(got.request_id, "req-3");
    }

    #[test]
    fn read_final_frame_without_newline_still_decodes() {
        // Go: ReadBytes returns data plus io.EOF; the frame is still unmarshaled.
        let frame = b"{\"requestId\":\"req-4\",\"contractVersion\":\"1\",\"status\":\"ok\"}";
        let mut r = BufReader::new(&frame[..]);
        let got = read_response(&mut r).unwrap();
        assert_eq!(got.request_id, "req-4");
    }

    #[test]
    fn read_clean_eof_errors() {
        let mut r = BufReader::new(&[][..]);
        let err = read_response(&mut r).unwrap_err();
        match err {
            CodecError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("want io EOF, got {other}"),
        }
    }

    #[test]
    fn read_garbage_frame_is_decode_error() {
        let frame = b"this-is-not-json\n";
        let mut r = BufReader::new(&frame[..]);
        assert!(matches!(read_response(&mut r), Err(CodecError::Encode(_))));
    }
}
