//! JSON-RPC 2.0 framing for LSP: build requests/notifications and split framed
//! messages off a byte stream. Transport-agnostic — the caller owns the pipe.
//!
//! LSP frames are `Content-Length: N\r\n\r\n` followed by `N` bytes of JSON.

use crate::json::{self, Json};

/// Wrap a JSON body in an LSP frame.
fn frame(body: &str) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn message(fields: Vec<(String, Json)>) -> Vec<u8> {
    frame(&Json::Object(fields).to_string())
}

/// A request (expects a response with the same `id`).
pub fn request(id: i64, method: &str, params: Json) -> Vec<u8> {
    message(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("id".into(), Json::Num(id as f64)),
        ("method".into(), Json::Str(method.into())),
        ("params".into(), params),
    ])
}

/// A notification (no response).
pub fn notification(method: &str, params: Json) -> Vec<u8> {
    message(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("method".into(), Json::Str(method.into())),
        ("params".into(), params),
    ])
}

/// A response to a server→client request.
pub fn response(id: Json, result: Json) -> Vec<u8> {
    message(vec![
        ("jsonrpc".into(), Json::Str("2.0".into())),
        ("id".into(), id),
        ("result".into(), result),
    ])
}

/// Try to split one complete frame off the front of `buf`, removing its bytes
/// and returning the parsed JSON body. `Ok(None)` means the frame is not yet
/// complete (wait for more bytes); `Err` is a malformed header/body.
pub fn take_message(buf: &mut Vec<u8>) -> Result<Option<Json>, String> {
    // Find the header/body separator.
    let Some(sep) = find_subslice(buf, b"\r\n\r\n") else {
        return Ok(None);
    };
    let header = std::str::from_utf8(&buf[..sep]).map_err(|_| "non-UTF-8 header".to_string())?;

    let mut content_len: Option<usize> = None;
    for line in header.split("\r\n") {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_len = value.trim().parse::<usize>().ok();
        }
    }
    let len = content_len.ok_or("missing Content-Length")?;

    let body_start = sep + 4;
    if buf.len() < body_start + len {
        return Ok(None); // body not fully arrived yet
    }

    let body = std::str::from_utf8(&buf[body_start..body_start + len])
        .map_err(|_| "non-UTF-8 body".to_string())?;
    let value = json::parse(body)?;
    buf.drain(..body_start + len);
    Ok(Some(value))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(frame: &[u8]) -> String {
        let s = std::str::from_utf8(frame).unwrap();
        let (_, body) = s.split_once("\r\n\r\n").unwrap();
        body.to_string()
    }

    #[test]
    fn request_has_jsonrpc_id_method_params() {
        let f = request(1, "initialize", Json::Object(vec![]));
        let body = body_of(&f);
        let j = json::parse(&body).unwrap();
        assert_eq!(j.get("jsonrpc").unwrap().as_str(), Some("2.0"));
        assert_eq!(j.get("id").unwrap().as_i64(), Some(1));
        assert_eq!(j.get("method").unwrap().as_str(), Some("initialize"));
        assert!(j.get("params").is_some());
    }

    #[test]
    fn notification_has_no_id() {
        let f = notification("initialized", Json::Object(vec![]));
        let j = json::parse(&body_of(&f)).unwrap();
        assert!(j.get("id").is_none());
        assert_eq!(j.get("method").unwrap().as_str(), Some("initialized"));
    }

    #[test]
    fn frame_roundtrips_through_take_message() {
        let mut stream = request(7, "x", Json::Num(1.0));
        let msg = take_message(&mut stream).unwrap().unwrap();
        assert_eq!(msg.get("id").unwrap().as_i64(), Some(7));
        assert!(stream.is_empty()); // fully consumed
    }

    #[test]
    fn partial_frame_waits() {
        let full = notification("m", Json::Null);
        let mut buf = full[..full.len() - 3].to_vec(); // truncated body
        assert_eq!(take_message(&mut buf).unwrap(), None);
        // header without body separator yet
        let mut headerless = b"Content-Length: 10".to_vec();
        assert_eq!(take_message(&mut headerless).unwrap(), None);
    }

    #[test]
    fn two_messages_in_one_buffer() {
        let mut buf = request(1, "a", Json::Null);
        buf.extend(notification("b", Json::Null));
        let first = take_message(&mut buf).unwrap().unwrap();
        assert_eq!(first.get("id").unwrap().as_i64(), Some(1));
        let second = take_message(&mut buf).unwrap().unwrap();
        assert_eq!(second.get("method").unwrap().as_str(), Some("b"));
        assert!(buf.is_empty());
        assert_eq!(take_message(&mut buf).unwrap(), None);
    }

    #[test]
    fn content_length_counts_utf8_bytes() {
        // "é" is 2 bytes; the framing must use byte length, not char count.
        let f = notification("m", Json::Str("é".into()));
        let mut buf = f.clone();
        let msg = take_message(&mut buf).unwrap().unwrap();
        assert_eq!(msg.get("params").unwrap().as_str(), Some("é"));
    }
}
