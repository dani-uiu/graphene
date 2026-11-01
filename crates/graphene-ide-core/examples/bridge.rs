//! Dev bridge: serves the built IDE (`ide/dist`) over HTTP with a stand-in
//! for Tauri's `invoke` that calls `shell::dispatch`. This lets the exact
//! same UI run in an ordinary (or headless) browser for automated checks.
//!
//! Usage: `cargo run -p graphene-ide-core --example bridge -- [port] [dist]`
//!
//! Invariants: development only; binds to 127.0.0.1; std only (no async
//! runtime); never panics on a bad request (it answers 400/404 instead).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use graphene_ide_core::shell::dispatch;
use serde_json::{json, Value as Json};

const SHIM: &str = r#"<script>
window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    if (cmd.startsWith("plugin:")) return null;
    const r = await fetch("/invoke/" + encodeURIComponent(cmd), { method: "POST", body: JSON.stringify(args ?? {}) });
    const j = await r.json();
    if ("err" in j) throw j.err;
    return j.ok;
  },
  transformCallback: (cb) => { const id = Math.floor(Math.random() * 1e9); window["_" + id] = cb; return id; },
  unregisterCallback: () => {},
  convertFileSrc: (p) => p,
  metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
};
</script>"#;

fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(1421);
    let dist = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ide/dist"));
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {port}: {e}");
            return;
        }
    };
    println!(
        "bridge: http://127.0.0.1:{port}/ serving {}",
        dist.display()
    );
    for stream in listener.incoming().flatten() {
        let dist = dist.clone();
        std::thread::spawn(move || {
            if let Err(e) = handle(stream, &dist) {
                eprintln!("bridge: {e}");
            }
        });
    }
}

fn handle(stream: TcpStream, dist: &Path) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let path = target.split('?').next().unwrap_or("/");
    let mut out = stream;
    if method == "POST" && path.starts_with("/invoke/") {
        let cmd = decode(&path["/invoke/".len()..]);
        let args: Json = serde_json::from_slice(&body).unwrap_or(Json::Null);
        let reply = match dispatch(&cmd, &args) {
            Ok(v) => json!({ "ok": v }),
            Err(e) => json!({ "err": e }),
        };
        return respond(
            &mut out,
            200,
            "application/json",
            reply.to_string().as_bytes(),
        );
    }
    serve_file(&mut out, dist, path)
}

fn serve_file(out: &mut TcpStream, dist: &Path, path: &str) -> std::io::Result<()> {
    let rel = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    if rel.contains("..") {
        return respond(out, 400, "text/plain", b"bad path");
    }
    let file = dist.join(rel);
    let Ok(mut bytes) = std::fs::read(&file) else {
        return respond(out, 404, "text/plain", b"not found");
    };
    if rel == "index.html" {
        let html = String::from_utf8_lossy(&bytes).replacen("<head>", &format!("<head>{SHIM}"), 1);
        bytes = html.into_bytes();
    }
    let kind = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    respond(out, 200, kind, &bytes)
}

fn respond(out: &mut TcpStream, code: u16, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let status = match code {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    write!(
        out,
        "HTTP/1.1 {code} {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    out.write_all(body)
}

/// Percent-decoding for command names such as `plugin%3Adialog%7Copen`.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
