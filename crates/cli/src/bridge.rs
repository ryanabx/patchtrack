//! `patchtrack bridge` and `patchtrack serve`: this machine's store, for
//! the web app.
//!
//! The app keeps nothing of its own: it asks the bridge, which reads and
//! changes the store, syncs it with the archive on its own schedule, and
//! does what this machine was allowed to (send mail, run b4). What it
//! answers is in [`crate::api`]; this is who it answers:
//!
//! - it listens on 127.0.0.1 (`serve`: where it is told, behind a proxy),
//!   and refuses a Host header that is not that address, localhost or a
//!   name it was given (a DNS-rebinding page would send its own);
//! - it answers CORS for the app's origins only, never `*`, and refuses a
//!   request from any other origin;
//! - every API request carries the pairing token (`X-Patchtrack-Token`),
//!   which the pairing link hands to the app; a request without it is
//!   refused before its body is read.
//!
//! `serve` also serves the built web app (`app_dir`), so the app and its
//! API share an origin.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::api::{Answer, Api};

/// A request's body, at most: an mbox to import is the largest.
const MAX_BODY: usize = 256 * 1024 * 1024;

pub struct Bridge {
    pub port: u16,
    pub token: String,
    /// Origins allowed to call it, e.g. https://ryanabx.github.io.
    pub origins: Vec<String>,
    /// Host headers it answers to besides 127.0.0.1 and localhost on its
    /// port: a server's public name ("pt.example.org").
    pub hosts: Vec<String>,
    /// The built web app, served at its root, same-origin with the API.
    pub app_dir: Option<PathBuf>,
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    /// Content-Length was more than [`MAX_BODY`]; the body was not read.
    too_large: bool,
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn error(status: u16, why: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: json!({ "error": why }).to_string().into_bytes(),
        }
    }
}

/// Constant time, so the token cannot be guessed a byte at a time.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A request; its body only when it carries `token` and is not too large.
fn read_request(stream: &mut TcpStream, token: &str) -> std::io::Result<Option<HttpRequest>> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut words = line.split_whitespace();
    let (Some(method), Some(path)) = (words.next(), words.next()) else { return Ok(None) };
    let (method, path) = (method.to_string(), path.to_string());
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let paired = headers.get("x-patchtrack-token").is_some_and(|t| same(t, token));
    let too_large = len > MAX_BODY;
    let mut body = vec![];
    if paired && !too_large {
        body = vec![0; len];
        reader.read_exact(&mut body)?;
    }
    Ok(Some(HttpRequest { method, path, headers, body, too_large }))
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "",
    }
}

fn write_response(stream: &mut TcpStream, r: &HttpResponse) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} {}\r\n", r.status, reason(r.status));
    for (k, v) in &r.headers {
        head += &format!("{k}: {v}\r\n");
    }
    head += &format!("Content-Length: {}\r\nConnection: close\r\n\r\n", r.body.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

impl Bridge {
    /// Answer one request.
    fn handle(&self, req: &HttpRequest, api: &Api) -> HttpResponse {
        let host_ok = req.headers.get("host").is_some_and(|h| {
            [format!("127.0.0.1:{}", self.port), format!("localhost:{}", self.port)].iter().any(|ok| ok == h)
                || self.hosts.iter().any(|ok| ok.eq_ignore_ascii_case(h))
        });
        if !host_ok {
            return HttpResponse::error(
                403,
                "this server answers only at 127.0.0.1, localhost or the names it was given",
            );
        }
        // The web app itself, where it is served: public, no token.
        if let Some(dir) = &self.app_dir
            && req.method == "GET"
            && !req.path.starts_with("/v1/")
        {
            return serve_file(dir, &req.path);
        }
        let origin = req.headers.get("origin");
        if let Some(o) = origin.filter(|o| !self.origins.contains(o)) {
            // No CORS headers, so the page only sees a network error: say it here.
            return HttpResponse::error(
                403,
                &format!("this bridge does not serve {o}: start it with `--app {o}/`, or add `--origin {o}`"),
            );
        }
        let mut r = self.route(req, api);
        if let Some(o) = origin {
            r.headers.extend([
                ("Access-Control-Allow-Origin".into(), o.clone()),
                ("Vary".into(), "Origin".into()),
                ("Access-Control-Expose-Headers".into(), "Content-Disposition".into()),
            ]);
        }
        r
    }

    fn route(&self, req: &HttpRequest, api: &Api) -> HttpResponse {
        if req.method == "OPTIONS" {
            let mut r = HttpResponse { status: 204, headers: vec![], body: vec![] };
            r.headers.extend([
                ("Access-Control-Allow-Methods".into(), "GET, POST".into()),
                ("Access-Control-Allow-Headers".into(), "X-Patchtrack-Token, Content-Type".into()),
                ("Access-Control-Max-Age".into(), "600".into()),
            ]);
            return r;
        }
        if !req.headers.get("x-patchtrack-token").is_some_and(|t| same(t, &self.token)) {
            return HttpResponse::error(
                403,
                "wrong or missing pairing token: open the link `patchtrack bridge` prints",
            );
        }
        if req.too_large {
            return HttpResponse::error(413, "request too large");
        }
        match api.handle(&req.method, &req.path, &req.body) {
            Answer::Json(status, v) => HttpResponse {
                status,
                headers: vec![
                    ("Content-Type".into(), "application/json".into()),
                    ("Cache-Control".into(), "no-store".into()),
                ],
                body: v.to_string().into_bytes(),
            },
            Answer::File(kind, name, body) => HttpResponse {
                status: 200,
                headers: vec![
                    ("Content-Type".into(), kind.into()),
                    ("Content-Disposition".into(), format!("attachment; filename=\"{name}\"")),
                    ("Cache-Control".into(), "no-store".into()),
                ],
                body,
            },
        }
    }

    /// Serve until the process ends, each connection on its own thread: a
    /// browser may open a connection and leave it idle, which must not hold
    /// up the next one.
    pub fn serve(&self, listener: TcpListener, api: &Api) {
        std::thread::scope(|scope| {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                scope.spawn(move || self.answer(stream, api));
            }
        });
    }

    fn answer(&self, mut stream: TcpStream, api: &Api) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let Ok(Some(req)) = read_request(&mut stream, &self.token) else { return };
        let resp = self.handle(&req, api);
        if resp.status >= 400 {
            let why = serde_json::from_slice::<serde_json::Value>(&resp.body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string));
            let from = req.headers.get("origin").map_or(String::new(), |o| format!(" from {o}"));
            eprintln!("refused {} {}{from}: {}", req.method, req.path, why.unwrap_or_default());
        } else if req.method == "POST" {
            eprintln!("{} {} {}", resp.status, req.method, req.path);
        }
        let _ = write_response(&mut stream, &resp);
    }
}

/// A file of the web app, by URL path; "/" is index.html. Nothing outside
/// the directory, and no hidden files.
fn serve_file(dir: &Path, path: &str) -> HttpResponse {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let rel = PathBuf::from(path.trim_start_matches('/'));
    let safe = rel.components().all(|c| matches!(c, Component::Normal(n) if !n.to_string_lossy().starts_with('.')));
    if !safe {
        return HttpResponse::error(404, "no such file");
    }
    let file = if path.ends_with('/') { dir.join(&rel).join("index.html") } else { dir.join(&rel) };
    let Ok(body) = std::fs::read(&file) else { return HttpResponse::error(404, "no such file") };
    let kind = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    };
    HttpResponse {
        status: 200,
        headers: vec![("Content-Type".into(), kind.into()), ("X-Content-Type-Options".into(), "nosniff".into())],
        body,
    }
}

/// A fresh pairing token: 128 random bits, in hex.
pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("the system's random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://app.example";

    /// A server on a free port over a fresh store, its app in `app`.
    fn start(name: &str, app: Option<PathBuf>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = std::env::temp_dir().join(format!("patchtrack-bridge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::thread::spawn(move || {
            let api = Api::new(&dir.join("s.sqlite3"), vec![], false).unwrap();
            let b = Bridge {
                port,
                token: "t0k".into(),
                origins: vec![ORIGIN.into()],
                hosts: vec!["pt.example.org".into()],
                app_dir: app,
            };
            b.serve(listener, &api);
        });
        port
    }

    /// (status, headers, body) of one raw request.
    fn call(port: u16, raw: &str) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = vec![];
        s.read_to_end(&mut out).unwrap();
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&out[..split]).to_string();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, head, out[split + 4..].to_vec())
    }

    fn get(port: u16, path: &str, origin: &str, token: &str, host: &str) -> (u16, String, Vec<u8>) {
        let host = if host.is_empty() { format!("127.0.0.1:{port}") } else { host.to_string() };
        call(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nX-Patchtrack-Token: {token}\r\n\r\n"),
        )
    }

    fn post(port: u16, path: &str, token: &str, body: &str) -> (u16, String, Vec<u8>) {
        call(
            port,
            &format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {ORIGIN}\r\nX-Patchtrack-Token: {token}\r\n\
                 Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    #[test]
    fn answers_the_app_from_the_store() {
        let port = start("answers", None);
        let (status, head, body) = get(port, "/v1/hello", ORIGIN, "t0k", "");
        assert_eq!(status, 200, "{head}");
        assert!(head.contains(&format!("Access-Control-Allow-Origin: {ORIGIN}")), "{head}");
        assert!(String::from_utf8_lossy(&body).contains(r#""can":{"b4":false,"send":false}"#));
        // What the app changes is changed in the store, and read back.
        let (status, ..) = post(port, "/v1/settings", "t0k", r#"{"key": "name", "value": "Ada"}"#);
        assert_eq!(status, 200);
        let (_, _, body) = get(port, "/v1/settings", ORIGIN, "t0k", "");
        assert!(String::from_utf8_lossy(&body).contains(r#""value":"\"Ada\"""#));
        let (status, _, body) = post(port, "/v1/settings", "t0k", r#"{"key": "nope", "value": "1"}"#);
        assert!(status == 400 && String::from_utf8_lossy(&body).contains("nope"));
        let (status, ..) = get(port, "/v1/summaries", ORIGIN, "t0k", "");
        assert_eq!(status, 200);
        assert_eq!(get(port, "/v1/nothing", ORIGIN, "t0k", "").0, 404);
        assert_eq!(get(port, "/v1/close", ORIGIN, "t0k", "").0, 405);
        // A store file to download.
        let (status, head, body) = get(port, "/v1/export", ORIGIN, "t0k", "");
        assert!(status == 200 && head.contains("application/vnd.sqlite3") && body.starts_with(b"SQLite"), "{head}");
    }

    #[test]
    fn refuses_what_it_should() {
        let port = start("refuses", None);
        // Another origin, a wrong token, another host name (DNS rebinding).
        assert_eq!(get(port, "/v1/hello", "https://evil.example", "t0k", "").0, 403);
        assert_eq!(get(port, "/v1/hello", ORIGIN, "t0j", "").0, 403);
        assert_eq!(get(port, "/v1/hello", ORIGIN, "t0k", &format!("evil.example:{port}")).0, 403);
        // A change without the token is refused, and changes nothing.
        assert_eq!(post(port, "/v1/settings", "t0j", r#"{"key": "name", "value": "Eve"}"#).0, 403);
        let (_, _, body) = get(port, "/v1/settings", ORIGIN, "t0k", "");
        assert!(!String::from_utf8_lossy(&body).contains("Eve"));
        // This machine's actions, only where allowed.
        assert_eq!(post(port, "/v1/send", "t0k", r#"{"msgid": "a@x"}"#).0, 403);
        assert_eq!(post(port, "/v1/b4", "t0k", r#"{"repo": "r", "branch": "b", "action": "send"}"#).0, 403);
        // A preflight from the app is answered; from elsewhere it is not.
        let pre = |origin: &str| {
            call(
                port,
                &format!(
                    "OPTIONS /v1/follow HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\n\
                     Access-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: x-patchtrack-token\r\n\r\n"
                ),
            )
        };
        let (status, head, _) = pre(ORIGIN);
        assert!(status == 204 && head.contains("Access-Control-Allow-Headers: X-Patchtrack-Token"), "{head}");
        let (status, head, body) = pre("https://evil.example");
        assert!(status == 403 && !head.contains("Access-Control-Allow-Origin"), "{head}");
        // The page cannot read why; the bridge's own output says, with the way out.
        assert!(String::from_utf8_lossy(&body).contains("`--origin https://evil.example`"));
    }

    #[test]
    fn an_idle_connection_holds_up_nothing() {
        let port = start("idle", None);
        // A browser may open a connection ahead of need and leave it idle.
        let _idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let t = std::time::Instant::now();
        assert_eq!(get(port, "/v1/hello", ORIGIN, "t0k", "").0, 200);
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn the_app_is_served_with_its_api_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("patchtrack-app-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<!doctype html>app").unwrap();
        std::fs::write(dir.join("assets/a.js"), b"0").unwrap();
        std::fs::write(dir.join(".secret"), "no").unwrap();
        let port = start("app", Some(dir.clone()));
        let get = |path: &str| call(port, &format!("GET {path} HTTP/1.1\r\nHost: pt.example.org\r\n\r\n"));
        // No token for the app itself.
        let (status, head, body) = get("/");
        assert!(status == 200 && head.contains("text/html") && body == b"<!doctype html>app", "{head}");
        let (status, head, _) = get("/assets/a.js?v=1");
        assert!(status == 200 && head.contains("text/javascript"), "{head}");
        for bad in ["/.secret", "/../etc/passwd", "/assets/../../x", "/nope.js"] {
            assert_eq!(get(bad).0, 404, "{bad}");
        }
        // The API still needs it.
        assert_eq!(get("/v1/hello").0, 403);
        // Another host name is refused, app or not.
        let (status, ..) = call(port, "GET / HTTP/1.1\r\nHost: evil.example\r\n\r\n");
        assert_eq!(status, 403);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tokens_are_fresh() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
