//! Plain HTTP/1.x listener tolerating raw (not percent-encoded) UTF-8 in request targets.
//!
//! Clients like the OGC TEAM Engine send request lines such as
//! `GET /wfs?typename=sf:EntitéGénérique HTTP/1.1` with raw UTF-8 bytes. The HTTP parser of
//! actix-http rejects them with 400. Servers like Jetty (GeoServer) accept them.
//! [`LenientIo`] percent-encodes non-ASCII bytes in request targets on the fly. It follows
//! the HTTP/1.1 message framing (Content-Length and chunked bodies), so request bodies are
//! never modified. Anything unexpected switches it to pass-through mode.

use actix_http::{body::MessageBody, HttpService, Protocol, Request, Response};
use actix_service::{
    fn_service, map_config, IntoServiceFactory, ServiceFactory, ServiceFactoryExt,
};
use actix_web::dev::{AppConfig, Server};
use actix_web::Error;
use bytes::BytesMut;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

const MAX_HEADER_LINE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// Start of a request (leading empty lines are allowed)
    Method {
        len: usize,
    },
    Target,
    Version,
    /// Header lines. `line_len` counts bytes of the current line
    Headers {
        line_len: usize,
    },
    Body {
        remaining: u64,
    },
    ChunkSize {
        size: u64,
        digits: usize,
        ext: bool,
        cr: bool,
    },
    ChunkData {
        remaining: u64,
    },
    ChunkDataEnd {
        remaining: u8,
    },
    Trailers {
        line_len: usize,
    },
    Passthrough,
}

/// Incremental request stream transformation (sans IO)
#[derive(Debug)]
pub struct RequestTargetEncoder {
    state: State,
    /// Current header line (prefix up to MAX_HEADER_LINE)
    line: Vec<u8>,
    content_length: Option<u64>,
    chunked: bool,
    upgrade: bool,
}

impl Default for RequestTargetEncoder {
    fn default() -> Self {
        RequestTargetEncoder {
            state: State::Method { len: 0 },
            line: Vec::new(),
            content_length: None,
            chunked: false,
            upgrade: false,
        }
    }
}

impl RequestTargetEncoder {
    /// Transform `input` bytes received from the client, appending the result to `out`
    pub fn feed(&mut self, input: &[u8], out: &mut BytesMut) {
        out.reserve(input.len());
        let mut i = 0;
        while i < input.len() {
            match self.state {
                State::Passthrough => {
                    out.extend_from_slice(&input[i..]);
                    return;
                }
                State::Body { remaining } => {
                    let n = (remaining.min((input.len() - i) as u64)) as usize;
                    out.extend_from_slice(&input[i..i + n]);
                    i += n;
                    self.state = if remaining - n as u64 == 0 {
                        State::Method { len: 0 }
                    } else {
                        State::Body {
                            remaining: remaining - n as u64,
                        }
                    };
                }
                State::ChunkData { remaining } => {
                    let n = (remaining.min((input.len() - i) as u64)) as usize;
                    out.extend_from_slice(&input[i..i + n]);
                    i += n;
                    self.state = if remaining - n as u64 == 0 {
                        State::ChunkDataEnd { remaining: 2 }
                    } else {
                        State::ChunkData {
                            remaining: remaining - n as u64,
                        }
                    };
                }
                State::Target => {
                    // fast path: copy ASCII target bytes up to the next space or non-ASCII byte
                    let rest = &input[i..];
                    let n = rest
                        .iter()
                        .position(|&b| b == b' ' || b >= 0x80 || b == b'\r' || b == b'\n')
                        .unwrap_or(rest.len());
                    out.extend_from_slice(&rest[..n]);
                    i += n;
                    if i < input.len() {
                        let b = input[i];
                        i += 1;
                        self.target_byte(b, out);
                    }
                }
                _ => {
                    let b = input[i];
                    i += 1;
                    self.byte(b, out);
                }
            }
        }
    }

    fn target_byte(&mut self, b: u8, out: &mut BytesMut) {
        match b {
            b' ' => {
                out.extend_from_slice(b" ");
                self.state = State::Version;
            }
            0x80..=0xFF => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.extend_from_slice(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]]);
            }
            _ => {
                // HTTP/0.9 style or malformed request line: leave it to the HTTP parser
                out.extend_from_slice(&[b]);
                self.state = State::Passthrough;
            }
        }
    }

    fn byte(&mut self, b: u8, out: &mut BytesMut) {
        out.extend_from_slice(&[b]);
        match self.state {
            State::Method { len } => match b {
                b'\r' | b'\n' if len == 0 => {}
                b'A'..=b'Z' | b'-' | b'_' if len < 32 => {
                    self.state = State::Method { len: len + 1 };
                }
                b' ' if len > 0 => self.state = State::Target,
                _ => self.state = State::Passthrough,
            },
            State::Version => {
                if b == b'\n' {
                    self.start_headers();
                }
            }
            State::Headers { line_len } => {
                if b == b'\n' {
                    if line_len <= 1 {
                        // empty line (CRLF or LF): end of header section
                        self.end_headers();
                    } else {
                        self.header_line();
                        self.line.clear();
                        self.state = State::Headers { line_len: 0 };
                    }
                } else {
                    if self.line.len() < MAX_HEADER_LINE {
                        self.line.push(b);
                    }
                    self.state = State::Headers {
                        line_len: line_len + 1,
                    };
                }
            }
            State::ChunkSize {
                size,
                digits,
                ext,
                cr,
            } => {
                let hex = (b as char).to_digit(16);
                self.state = match (b, hex) {
                    (b'\n', _) if digits > 0 => {
                        if size == 0 {
                            State::Trailers { line_len: 0 }
                        } else {
                            State::ChunkData { remaining: size }
                        }
                    }
                    (_, _) if cr => State::Passthrough,
                    (b'\r', _) if digits > 0 => State::ChunkSize {
                        size,
                        digits,
                        ext,
                        cr: true,
                    },
                    (_, Some(h)) if !ext && digits < 16 => State::ChunkSize {
                        size: size * 16 + h as u64,
                        digits: digits + 1,
                        ext,
                        cr,
                    },
                    (b';' | b' ' | b'\t', _) if digits > 0 => State::ChunkSize {
                        size,
                        digits,
                        ext: true,
                        cr,
                    },
                    (_, _) if ext => State::ChunkSize {
                        size,
                        digits,
                        ext,
                        cr,
                    },
                    _ => State::Passthrough,
                };
            }
            State::ChunkDataEnd { remaining } => {
                let expected = if remaining == 2 { b'\r' } else { b'\n' };
                self.state = if b != expected {
                    State::Passthrough
                } else if remaining == 1 {
                    State::ChunkSize {
                        size: 0,
                        digits: 0,
                        ext: false,
                        cr: false,
                    }
                } else {
                    State::ChunkDataEnd { remaining: 1 }
                };
            }
            State::Trailers { line_len } => {
                if b == b'\n' {
                    if line_len <= 1 {
                        self.reset();
                    } else {
                        self.state = State::Trailers { line_len: 0 };
                    }
                } else {
                    self.state = State::Trailers {
                        line_len: line_len + 1,
                    };
                }
            }
            State::Target | State::Body { .. } | State::ChunkData { .. } | State::Passthrough => {
                unreachable!("handled in feed")
            }
        }
    }

    fn start_headers(&mut self) {
        self.line.clear();
        self.content_length = None;
        self.chunked = false;
        self.upgrade = false;
        self.state = State::Headers { line_len: 0 };
    }

    fn header_line(&mut self) {
        let line = String::from_utf8_lossy(&self.line);
        let Some((name, value)) = line.split_once(':') else {
            return;
        };
        let value = value.trim().to_ascii_lowercase();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => match value.parse::<u64>() {
                Ok(n) if !matches!(self.content_length, Some(c) if c != n) => {
                    self.content_length = Some(n)
                }
                _ => self.upgrade = true, // invalid or conflicting: stop interpreting the stream
            },
            "transfer-encoding" => {
                if value.split(',').any(|t| t.trim() == "chunked") {
                    self.chunked = true;
                }
            }
            "upgrade" => self.upgrade = true,
            "connection" if value.contains("upgrade") => self.upgrade = true,
            _ => {}
        }
    }

    fn end_headers(&mut self) {
        self.line.clear();
        self.state = if self.upgrade {
            State::Passthrough
        } else if self.chunked {
            State::ChunkSize {
                size: 0,
                digits: 0,
                ext: false,
                cr: false,
            }
        } else {
            match self.content_length {
                Some(n) if n > 0 => State::Body { remaining: n },
                _ => State::Method { len: 0 },
            }
        };
    }

    fn reset(&mut self) {
        self.line.clear();
        self.state = State::Method { len: 0 };
    }
}

/// Connection wrapper applying [`RequestTargetEncoder`] to the incoming byte stream
pub struct LenientIo<T> {
    inner: T,
    encoder: RequestTargetEncoder,
    pending: BytesMut,
    raw: Vec<u8>,
}

impl<T> LenientIo<T> {
    pub fn new(inner: T) -> Self {
        LenientIo {
            inner,
            encoder: RequestTargetEncoder::default(),
            pending: BytesMut::new(),
            raw: Vec::new(),
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for LenientIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pending.is_empty() {
            if this.encoder.state == State::Passthrough {
                return Pin::new(&mut this.inner).poll_read(cx, buf);
            }
            let cap = buf.remaining().max(1);
            this.raw.resize(cap, 0);
            let mut raw_buf = ReadBuf::new(&mut this.raw);
            match Pin::new(&mut this.inner).poll_read(cx, &mut raw_buf) {
                Poll::Ready(Ok(())) => {
                    let filled = raw_buf.filled().len();
                    if filled == 0 {
                        return Poll::Ready(Ok(())); // EOF
                    }
                    let (encoder, raw, pending) = (&mut this.encoder, &this.raw, &mut this.pending);
                    encoder.feed(&raw[..filled], pending);
                }
                other => return other,
            }
        }
        let n = this.pending.len().min(buf.remaining());
        buf.put_slice(&this.pending.split_to(n));
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for LenientIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// Plain HTTP/1.x server for an actix-web application factory (like `HttpServer::new(..).bind(..)`),
/// accepting raw UTF-8 in request targets.
pub fn http_server<F, I, S, B>(
    factory: F,
    addr: &str,
    workers: usize,
    shutdown_timeout: u64,
) -> io::Result<Server>
where
    F: Fn() -> I + Send + Clone + 'static,
    I: IntoServiceFactory<S, Request>,
    S: ServiceFactory<Request, Config = AppConfig> + 'static,
    S::Error: Into<Error> + 'static,
    S::InitError: fmt::Debug,
    S::Response: Into<Response<B>> + 'static,
    <S::Service as actix_service::Service<Request>>::Future: 'static,
    S::Service: 'static,
    B: MessageBody + 'static,
{
    Ok(Server::build()
        .workers(workers)
        .shutdown_timeout(shutdown_timeout)
        .bind("bbox-http", addr, move || {
            let app = factory().into_factory();
            fn_service(|io: TcpStream| async {
                let peer_addr = io.peer_addr().ok();
                Ok::<_, actix_http::error::DispatchError>((
                    LenientIo::new(io),
                    Protocol::Http1,
                    peer_addr,
                ))
            })
            .and_then(HttpService::build().finish(map_config(
                app.map_err(|e| e.into().error_response()),
                |_| AppConfig::default(),
            )))
        })?
        .run())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_chunks(chunks: &[&[u8]]) -> Vec<u8> {
        let mut enc = RequestTargetEncoder::default();
        let mut out = BytesMut::new();
        for c in chunks {
            enc.feed(c, &mut out);
        }
        out.to_vec()
    }

    fn encode(input: &[u8]) -> Vec<u8> {
        let all = encode_chunks(&[input]);
        // byte-wise feeding gives the same result
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(encode_chunks(&bytes), all, "byte-wise");
        all
    }

    #[test]
    fn encodes_utf8_in_target() {
        let req =
            "GET /wfs?typename=sf:EntitéGénérique&x=1 HTTP/1.1\r\nHost: a\r\nX-Name: é\r\n\r\n";
        assert_eq!(
            String::from_utf8(encode(req.as_bytes())).unwrap(),
            "GET /wfs?typename=sf:Entit%C3%A9G%C3%A9n%C3%A9rique&x=1 HTTP/1.1\r\nHost: a\r\nX-Name: é\r\n\r\n"
        );
    }

    #[test]
    fn ascii_requests_unchanged() {
        let req = b"GET /a?b=%C3%A9 HTTP/1.1\r\nHost: a\r\n\r\nGET /b HTTP/1.1\r\n\r\n";
        assert_eq!(encode(req), req.to_vec());
    }

    #[test]
    fn body_with_content_length_untouched() {
        let body = "<a>\r\n\r\nGET /é HTTP/1.1\r\n</a>";
        let req = format!(
            "POST /wfs HTTP/1.1\r\nHost: a\r\ncontent-LENGTH: {}\r\n\r\n{body}GET /é HTTP/1.1\r\n\r\n",
            body.len()
        );
        let out = String::from_utf8(encode(req.as_bytes())).unwrap();
        assert!(out.contains(body), "{out}");
        assert!(out.ends_with("GET /%C3%A9 HTTP/1.1\r\n\r\n"), "{out}");
    }

    #[test]
    fn chunked_body_untouched() {
        let req = "POST /wfs HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5;x=y\r\nGET /\r\na\r\né HTTP/1.\r\n0\r\nT: 1\r\n\r\nGET /é HTTP/1.1\r\n\r\n";
        let out = String::from_utf8(encode(req.as_bytes())).unwrap();
        assert!(
            out.contains("5;x=y\r\nGET /\r\na\r\né HTTP/1.\r\n0\r\nT: 1\r\n\r\n"),
            "{out}"
        );
        assert!(out.ends_with("GET /%C3%A9 HTTP/1.1\r\n\r\n"), "{out}");
    }

    #[test]
    fn upgrade_and_garbage_pass_through() {
        let req = "GET /ws HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\nGET /é HTTP/1.1\r\n";
        assert_eq!(encode(req.as_bytes()), req.as_bytes());
        let req = "\x16\x03\x01GET /é HTTP/1.1\r\n";
        assert_eq!(encode(req.as_bytes()), req.as_bytes());
        let req = "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00é";
        assert_eq!(encode(req.as_bytes()), req.as_bytes());
        // invalid content-length: stop interpreting
        let req = "POST / HTTP/1.1\r\nContent-Length: x\r\n\r\nGET /é HTTP/1.1\r\n";
        assert_eq!(encode(req.as_bytes()), req.as_bytes());
    }
}
