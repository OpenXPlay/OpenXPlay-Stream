// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
}
/// Reads only the header, so a pipelined WebSocket frame stays in the transport.
pub fn read(input: &mut impl Read) -> Result<Request, u16> {
    fn line(input: &mut impl Read, total: &mut usize, limit: usize) -> Result<String, u16> {
        let mut b = Vec::new();
        loop {
            let mut c = [0];
            input.read_exact(&mut c).map_err(|_| 400u16)?;
            *total += 1;
            if *total > 32768 || b.len() >= limit {
                return Err(431);
            }
            if c[0] == 13 {
                input.read_exact(&mut c).map_err(|_| 400u16)?;
                *total += 1;
                if c[0] != 10 {
                    return Err(400);
                }
                return String::from_utf8(b).map_err(|_| 400);
            }
            if !(32..=126).contains(&c[0]) && c[0] != 9 {
                return Err(400);
            }
            b.push(c[0]);
        }
    }
    let mut total = 0;
    let l = line(input, &mut total, 4096)?;
    let parts: Vec<_> = l.split(' ').collect();
    if parts.len() != 3
        || parts[0].is_empty()
        || !parts[0].bytes().all(|b| b.is_ascii_uppercase())
        || !parts[1].starts_with('/')
        || parts[2] != "HTTP/1.1"
    {
        return Err(400);
    }
    let mut headers = BTreeMap::new();
    loop {
        let l = line(input, &mut total, 8192)?;
        if l.is_empty() {
            break;
        }
        let (k, v) = l.split_once(':').ok_or(400u16)?;
        if k.is_empty()
            || !k
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&b))
            || headers
                .insert(k.to_ascii_lowercase(), v.trim().to_owned())
                .is_some()
            || headers.len() > 100
        {
            return Err(400);
        }
    }
    if headers.get("host").is_none_or(|s| s.is_empty())
        || headers.contains_key("transfer-encoding")
        || headers.get("content-length").is_some_and(|s| s != "0")
    {
        return Err(400);
    }
    Ok(Request {
        method: parts[0].into(),
        target: parts[1].into(),
        headers,
    })
}
pub fn upgrade(r: &Request, out: &mut impl Write) -> Result<(), u16> {
    let tokens = |name: &str, token: &str| {
        r.headers
            .get(name)
            .is_some_and(|s| s.split(',').any(|v| v.trim().eq_ignore_ascii_case(token)))
    };
    let key = r.headers.get("sec-websocket-key").ok_or(400u16)?;
    if !tokens("upgrade", "websocket")
        || !tokens("connection", "upgrade")
        || r.headers
            .get("sec-websocket-version")
            .is_none_or(|s| s != "13")
        || !STANDARD.decode(key).is_ok_and(|v| v.len() == 16)
    {
        return Err(400);
    }
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    write!(out,"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").map_err(|_|400u16)?;
    out.flush().map_err(|_| 400)
}
fn resolve(target: &str, root: &Path) -> Result<PathBuf, u16> {
    if target.starts_with("//")
        || target.contains('#')
        || target.bytes().any(|b| b"<>\"{}|\\^`".contains(&b))
    {
        return Err(404);
    }
    for (index, byte) in target.bytes().enumerate() {
        if byte == b'%'
            && !target
                .as_bytes()
                .get(index + 1..index + 3)
                .is_some_and(|v| v.iter().all(u8::is_ascii_hexdigit))
        {
            return Err(404);
        }
    }
    let path = target.split('?').next().ok_or(404u16)?;
    let mut decoded = Vec::new();
    let mut i = 0;
    let b = path.as_bytes();
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3).ok_or(404u16)?;
            let s = std::str::from_utf8(hex).map_err(|_| 404u16)?;
            decoded.push(u8::from_str_radix(s, 16).map_err(|_| 404u16)?);
            i += 3;
        } else {
            decoded.push(b[i]);
            i += 1;
        }
    }
    let path = std::str::from_utf8(&decoded).map_err(|_| 404u16)?;
    if path.contains(['\\', '\0']) || path.split('/').any(|c| c == "." || c == "..") {
        return Err(404);
    }
    let path = match path {
        "/" => "/receiver/index.html",
        "/probe/" => "/probe/index.html",
        p => p,
    };
    let base = root.canonicalize().map_err(|_| 404u16)?;
    let file = base
        .join(path.strip_prefix('/').unwrap_or(path))
        .canonicalize()
        .map_err(|_| 404u16)?;
    if !file.starts_with(&base) || !file.is_file() {
        return Err(404);
    }
    Ok(file)
}
pub fn serve(r: &Request, root: &Path, out: &mut impl Write) -> Result<(), u16> {
    let p = resolve(&r.target, root)?;
    let ty = match p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    let mut f = std::fs::File::open(p).map_err(|_| 404u16)?;
    let size = f.metadata().map_err(|_| 404u16)?.len();
    write!(out,"HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {size}\r\nConnection: close\r\n{}\r\n",if ty.starts_with("text/html") {"Cache-Control: no-store\r\n"} else {""}).map_err(|_|400u16)?;
    std::io::copy(&mut f, out).map_err(|_| 400u16)?;
    out.flush().map_err(|_| 400)
}
pub fn error(out: &mut impl Write, status: u16) {
    let reason = match status {
        400 => "Bad Request",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Not Found",
    };
    let _ = write!(
        out,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n{}\r\n",
        if status == 405 { "Allow: GET\r\n" } else { "" }
    );
    let _ = out.flush();
}
