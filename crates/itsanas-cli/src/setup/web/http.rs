//! Just enough HTTP/1.1 for one page on the loopback interface.
//!
//! No crate: `cargo deny` refuses most of the async stacks, and a page that
//! answers one person on 127.0.0.1 needs a request line, a few headers and a
//! short body -- not a framework. What it does need is limits that hold
//! against a hostile local page or process, so every one is a constant here
//! and every refusal happens *before* the oversized part is read: a head that
//! is not over after [`MAX_HEAD`] bytes is answered 431 at that byte, a body
//! announced over [`MAX_BODY`] is answered 413 without a byte of it read.

use std::io::{Read, Write};

/// The request line and headers together, at most.
pub(super) const MAX_HEAD: usize = 16 * 1024;
/// A request body, at most: a form of a few answers is well under 1 KiB.
pub(super) const MAX_BODY: usize = 64 * 1024;

/// One request, read whole and within the limits.
#[derive(Debug)]
pub(super) struct Request {
    pub(super) method: String,
    /// Without its query string: no route here takes one.
    pub(super) path: String,
    headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}

impl Request {
    /// The value of the header `name` (any case), when it is there once.
    /// Twice is treated as absent-and-suspicious by the callers that matter
    /// (Host), through [`Request::count`].
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub(super) fn count(&self, name: &str) -> usize {
        self.headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .count()
    }
}

/// Read one request, or the status to refuse it with.
pub(super) fn read_request(stream: &mut impl Read) -> Result<Request, u16> {
    let (head, mut rest) = read_head(stream)?;
    let mut request = parse_head(&head)?;
    if request.count("transfer-encoding") > 0 {
        // Chunked bodies are a parser of their own, and the page never sends one.
        return Err(411);
    }
    let length = match request.count("content-length") {
        0 => 0,
        1 => request
            .header("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or(400_u16)?,
        _ => return Err(400),
    };
    if length > MAX_BODY {
        return Err(413);
    }
    rest.truncate(length);
    if rest.len() < length {
        let mut more = Vec::with_capacity(length - rest.len());
        stream
            .take((length - rest.len()) as u64)
            .read_to_end(&mut more)
            .map_err(|error| status_of(&error))?;
        rest.extend_from_slice(&more);
        if rest.len() < length {
            return Err(400);
        }
    }
    request.body = rest;
    Ok(request)
}

fn status_of(error: &std::io::Error) -> u16 {
    match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => 408,
        _ => 400,
    }
}

/// The head, and whatever of the body came with it.
fn read_head(stream: &mut impl Read) -> Result<(Vec<u8>, Vec<u8>), u16> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    loop {
        // Never more than one byte past the limit: that byte is the proof
        // the head is too long, and nothing after it is read.
        let room = (MAX_HEAD + 1 - buffer.len()).min(chunk.len());
        let read = stream
            .read(&mut chunk[..room])
            .map_err(|error| status_of(&error))?;
        if read == 0 {
            return Err(400);
        }
        let searched_from = buffer.len().saturating_sub(3);
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer[searched_from..]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            let end = searched_from + at;
            let rest = buffer.split_off(end + 4);
            buffer.truncate(end);
            return Ok((buffer, rest));
        }
        if buffer.len() > MAX_HEAD {
            return Err(431);
        }
    }
}

fn parse_head(head: &[u8]) -> Result<Request, u16> {
    let text = std::str::from_utf8(head).map_err(|_| 400_u16)?;
    let mut lines = text.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(400);
    };
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(505);
    }
    let path = target.split(['?', '#']).next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(400_u16)?;
        if name.is_empty() || name.contains([' ', '\t']) {
            return Err(400);
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }
    Ok(Request {
        method: method.to_owned(),
        path,
        headers,
        body: Vec::new(),
    })
}

/// Sent with every response, refusals included. `no-store` because the
/// state this page shows is a person's machine; the rest so no other page can
/// frame it, sniff a type into it, learn its address from a Referer, or run a
/// script it did not serve itself. No CORS header is ever sent: another
/// origin is not to read anything here, so the browser's default refusal is
/// the policy.
pub(super) const SECURITY_HEADERS: [(&str, &str); 5] = [
    ("Cache-Control", "no-store"),
    ("X-Frame-Options", "DENY"),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'",
    ),
];

/// What goes back.
#[derive(Debug)]
pub(super) struct Response {
    pub(super) status: u16,
    pub(super) content_type: &'static str,
    pub(super) body: Vec<u8>,
}

impl Response {
    pub(super) fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
        }
    }

    /// A refusal, said in a sentence the page can show.
    pub(super) fn refuse(status: u16, why: &str) -> Self {
        Self::new(
            status,
            "application/json",
            format!("{{\"error\":{}}}", super::json::quote(why)),
        )
    }
}

pub(super) const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

pub(super) fn write_response(stream: &mut impl Write, response: &Response) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason(response.status),
        response.content_type,
        response.body.len()
    );
    for (name, value) in SECURITY_HEADERS {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// Whether two secrets are equal, in a time that does not depend on where
/// they first differ: the token is the only thing between a local process
/// and this machine's settings, and a compare that returns at the first
/// wrong byte tells it, byte by byte, how much it has right.
pub(super) fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() {
        // The length is not secret: every token is 32 hex digits.
        return false;
    }
    given
        .iter()
        .zip(expected)
        .fold(0_u8, |differ, (a, b)| differ | (a ^ b))
        == 0
}

/// Whether `Host` names this server. Anything else is a page that resolved
/// its own name to 127.0.0.1 (DNS rebinding) to read this one as same-origin.
pub(super) fn host_allowed(request: &Request, port: u16) -> bool {
    if request.count("host") != 1 {
        return false;
    }
    let host = request.header("host").unwrap_or("");
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

/// Whether a request may come from where it says: no `Origin` (a browser
/// navigating, or a tool), or this server's own. And not a fetch the
/// browser itself marks cross-site.
pub(super) fn origin_allowed(request: &Request, port: u16) -> bool {
    let origin_ok = match request.header("origin") {
        None => request.count("origin") == 0,
        Some(origin) => {
            request.count("origin") == 1
                && (origin == format!("http://127.0.0.1:{port}")
                    || origin == format!("http://localhost:{port}"))
        }
    };
    let site_ok = !matches!(
        request.header("sec-fetch-site"),
        Some("cross-site" | "same-site")
    );
    origin_ok && site_ok
}
