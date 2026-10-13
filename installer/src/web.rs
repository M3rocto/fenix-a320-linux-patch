// SPDX-License-Identifier: MIT
//! Guided setup in the user's browser, served only to this computer.
//!
//! The address contains a random secret, and requests for another host name are
//! refused, so other web pages cannot reach the installer. The server follows
//! Python's `http.server`, which the original installer used: one request per
//! connection, the same limits and the same answers.
use crate::{
    error::{Error, Result, repr},
    guide,
    json::{self, Value, json},
    py,
    system::System,
    targets::Target,
};
use std::{
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const IDLE_SECONDS: u64 = 15 * 60;
pub const PAGE: &str = include_str!("page.html");
const MAX_LINE: usize = 65536;
const MAX_HEADERS: usize = 100;
const MAX_BODY: i64 = 65536;

struct Shared {
    index: usize,
    busy: Option<String>,
    /// When the running step started and where its lines begin in the log.
    since: Option<(Instant, usize)>,
    log: Vec<String>,
    error: Option<String>,
    view: Option<Value>,
    seen: Instant,
}

pub struct Session {
    system: Arc<dyn System>,
    found: Vec<Arc<dyn Target>>,
    bundle: OsString,
    proton: Option<OsString>,
    shared: Mutex<Shared>,
    closed: Mutex<bool>,
    wake: Condvar,
}

impl Session {
    pub fn new(
        system: Arc<dyn System>,
        found: Vec<Arc<dyn Target>>,
        bundle: &OsStr,
        proton: Option<&OsStr>,
    ) -> Arc<Self> {
        Arc::new(Self {
            system,
            found,
            bundle: bundle.to_os_string(),
            proton: proton.map(OsStr::to_os_string),
            shared: Mutex::new(Shared {
                index: 0,
                busy: None,
                since: None,
                log: Vec::new(),
                error: None,
                view: None,
                seen: Instant::now(),
            }),
            closed: Mutex::new(false),
            wake: Condvar::new(),
        })
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether a step is running. An empty action name never counted as one.
    pub fn busy(&self) -> bool {
        self.shared()
            .busy
            .as_deref()
            .is_some_and(|action| !action.is_empty())
    }

    pub fn is_closed(&self) -> bool {
        *self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn close(&self) {
        *self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.wake.notify_all();
    }

    /// Wait until the session is closed, at most for `timeout`.
    fn wait_closed(&self, timeout: Duration) -> bool {
        let closed = self
            .closed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (closed, _) = self
            .wake
            .wait_timeout_while(closed, timeout, |closed| !*closed)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *closed
    }

    pub fn state(&self) -> Result<Value> {
        let (busy, index, refresh) = {
            let mut shared = self.shared();
            shared.seen = Instant::now();
            (
                shared.busy.clone(),
                shared.index,
                shared.busy.is_none() || shared.view.is_none(),
            )
        };
        let Some(target) = self.found.get(index) else {
            return Ok(
                json!({"targets": [], "busy": null, "log": [], "error": null, "view": null}),
            );
        };
        if refresh {
            // While an operation runs, its intermediate state is not a problem to report.
            let view = guide::overview(target.as_ref())?;
            self.shared().view = Some(view);
        }
        let shared = self.shared();
        let labels: Vec<String> = self.found.iter().map(|item| item.label()).collect();
        let mut state = json::Map::new();
        state.insert("targets".into(), json!(labels));
        state.insert("index".into(), json!(shared.index));
        state.insert("busy".into(), json!(busy));
        state.insert(
            "log".into(),
            json!(shared.log[shared.log.len().saturating_sub(200)..]),
        );
        // What the running step has reported so far, and for how long it runs.
        state.insert(
            "activity".into(),
            match shared.since.filter(|_| shared.busy.is_some()) {
                Some((started, first)) => json!({
                    "seconds": started.elapsed().as_secs(),
                    "steps": shared.log[first.min(shared.log.len())..],
                }),
                None => Value::Null,
            },
        );
        state.insert("error".into(), json!(shared.error));
        state.insert("view".into(), shared.view.clone().unwrap_or(Value::Null));
        drop(shared);
        state.insert(
            "installers".into(),
            json!(guide::installers(self.system.as_ref())),
        );
        state.insert(
            "can_pick".into(),
            json!(guide::can_pick_file(self.system.as_ref())),
        );
        Ok(Value::Object(state))
    }

    pub fn run(self: &Arc<Self>, action: &str, executable: Option<String>) -> Result<()> {
        let target = {
            let mut shared = self.shared();
            if shared
                .busy
                .as_deref()
                .is_some_and(|action| !action.is_empty())
            {
                return Err(Error::patch("Another step is still running."));
            }
            shared.busy = Some(action.to_string());
            shared.since = Some((Instant::now(), shared.log.len()));
            shared.error = None;
            self.found.get(shared.index).cloned()
        };
        let (session, action) = (self.clone(), action.to_string());
        let started = thread::Builder::new().spawn(move || {
            let log = |line: &str| {
                session.shared().log.push(line.to_string());
                Ok(())
            };
            let outcome = match &target {
                Some(target) => guide::perform(
                    session.system.as_ref(),
                    target.as_ref(),
                    &action,
                    &session.bundle,
                    &log,
                    executable.as_deref(),
                    session.proton.as_deref(),
                ),
                None => Err(Error::patch("No such simulator.")),
            };
            let mut shared = session.shared();
            if let Err(error) = outcome {
                shared.error = Some(error.to_string());
                shared.log.push(format!("Error: {error}"));
            }
            shared.busy = None;
        });
        if let Err(error) = started {
            self.shared().busy = None;
            return Err(Error::io(&error));
        }
        Ok(())
    }

    pub fn select(&self, index: i64) -> Result<()> {
        let mut shared = self.shared();
        if shared
            .busy
            .as_deref()
            .is_some_and(|action| !action.is_empty())
        {
            return Err(Error::patch("Another step is still running."));
        }
        match usize::try_from(index)
            .ok()
            .filter(|index| *index < self.found.len())
        {
            Some(index) => {
                shared.index = index;
                shared.view = None;
                shared.error = None;
                Ok(())
            }
            None => Err(Error::patch("No such simulator.")),
        }
    }
}

struct Request {
    command: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    /// The first header of this name; only leading blanks of its value are dropped.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// `rfile.readline(limit)`: at most `limit` bytes, up to and including a line feed.
fn read_line(reader: &mut impl BufRead, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut line = Vec::new();
    while line.len() < limit {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let wanted = &available[..available.len().min(limit - line.len())];
        let (taken, done) = match wanted.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (wanted.len(), false),
        };
        line.extend_from_slice(&wanted[..taken]);
        reader.consume(taken);
        if done {
            break;
        }
    }
    Ok(line)
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct Connection<'a> {
    stream: &'a TcpStream,
    /// HTTP/0.9 has neither a status line nor headers in the answer.
    simple: bool,
    /// A HEAD request is refused without the page that explains why.
    head: bool,
}

impl Connection<'_> {
    fn send(&mut self, code: u16, reason: &str, headers: &[(&str, String)], body: &[u8]) {
        let mut out = Vec::new();
        if !self.simple {
            out.extend(format!("HTTP/1.0 {code} {reason}\r\n").into_bytes());
            out.extend(
                format!(
                    "Server: fenix-patch/{}\r\nDate: {}\r\n",
                    crate::VERSION,
                    py::Utc::now().http_date()
                )
                .into_bytes(),
            );
            for (name, value) in headers {
                out.extend(format!("{name}: {value}\r\n").into_bytes());
            }
            out.extend(b"\r\n");
        }
        out.extend(body);
        let mut stream = self.stream;
        let _ = stream.write_all(&out);
        let _ = stream.flush();
    }

    fn reply(&mut self, code: u16, body: &[u8], kind: &str) {
        let reason = match code {
            200 => "OK",
            400 => "Bad Request",
            _ => "Not Found",
        };
        let headers = [
            ("Content-Type", kind.to_string()),
            ("Content-Length", body.len().to_string()),
            ("Cache-Control", "no-store".to_string()),
            ("X-Content-Type-Options", "nosniff".to_string()),
            ("Referrer-Policy", "no-referrer".to_string()),
        ];
        self.send(code, reason, &headers, body);
    }

    fn reply_json(&mut self, code: u16, body: &Value) {
        self.reply(code, json::dumps(body, None).as_bytes(), "application/json");
    }

    fn not_found(&mut self) {
        self.reply_json(404, &json!({"error": "Not found"}));
    }

    /// `BaseHTTPRequestHandler.send_error`.
    fn error(&mut self, code: u16, message: Option<&str>, explain: Option<&str>) {
        let (short, long) = match code {
            400 => ("Bad Request", "Bad request syntax or unsupported method"),
            414 => ("URI Too Long", "URI is too long"),
            431 => (
                "Request Header Fields Too Large",
                "The server is unwilling to process the request because its header fields are too large",
            ),
            501 => ("Not Implemented", "Server does not support this operation"),
            505 => ("HTTP Version Not Supported", "Cannot fulfill request"),
            _ => ("???", "???"),
        };
        let message = message.unwrap_or(short);
        let body = format!(
            "<!DOCTYPE HTML>\n<html lang=\"en\">\n    <head>\n        <meta charset=\"utf-8\">\n        <style type=\"text/css\">\n            \
             :root {{\n                color-scheme: light dark;\n            }}\n        </style>\n        <title>Error response</title>\n    \
             </head>\n    <body>\n        <h1>Error response</h1>\n        <p>Error code: {code}</p>\n        <p>Message: {}.</p>\n        \
             <p>Error code explanation: {code} - {}.</p>\n    </body>\n</html>\n",
            html_escape(message),
            html_escape(explain.unwrap_or(long))
        );
        let headers = [
            ("Connection", "close".to_string()),
            ("Content-Type", "text/html;charset=utf-8".to_string()),
            ("Content-Length", body.len().to_string()),
        ];
        self.send(
            code,
            message,
            &headers,
            if self.head { b"" } else { body.as_bytes() },
        );
    }
}

/// `BaseHTTPRequestHandler.parse_request`; `None` once an error has been answered.
fn parse_request(reader: &mut impl BufRead, connection: &mut Connection<'_>) -> Option<Request> {
    let raw = read_line(reader, MAX_LINE + 1).ok()?;
    if raw.len() > MAX_LINE {
        connection.error(414, None, None);
        return None;
    }
    if raw.is_empty() {
        return None;
    }
    let line = latin1(&raw);
    let line = line.trim_end_matches(['\r', '\n']);
    let words = py::split(line);
    if words.is_empty() {
        return None;
    }
    if words.len() >= 3 {
        let version = words[words.len() - 1];
        let number = version.strip_prefix("HTTP/").and_then(|number| {
            let parts: Vec<&str> = number.split('.').collect();
            let valid = parts.len() == 2
                && parts.iter().all(|part| {
                    !part.is_empty()
                        && part.len() <= 10
                        && part.bytes().all(|byte| byte.is_ascii_digit())
                });
            valid.then(|| (parts[0].parse::<u64>().unwrap_or(0), number))
        });
        match number {
            None => {
                connection.error(
                    400,
                    Some(&format!("Bad request version ({})", repr(version))),
                    None,
                );
                return None;
            }
            Some((major, number)) if major >= 2 => {
                connection.error(505, Some(&format!("Invalid HTTP version ({number})")), None);
                return None;
            }
            Some(_) => {}
        }
    }
    if !(2..=3).contains(&words.len()) {
        connection.error(
            400,
            Some(&format!("Bad request syntax ({})", repr(line))),
            None,
        );
        return None;
    }
    let (command, mut path) = (words[0].to_string(), words[1].to_string());
    if words.len() == 2 {
        if command != "GET" {
            connection.error(
                400,
                Some(&format!("Bad HTTP/0.9 request type ({})", repr(&command))),
                None,
            );
            return None;
        }
        connection.simple = true;
    }
    connection.head = command == "HEAD";
    if path.starts_with("//") {
        path = format!("/{}", path.trim_start_matches('/'));
    }
    let mut lines = Vec::new();
    loop {
        let raw = read_line(reader, MAX_LINE + 1).ok()?;
        if raw.len() > MAX_LINE {
            connection.error(
                431,
                Some("Line too long"),
                Some(&format!(
                    "got more than {MAX_LINE} bytes when reading header line"
                )),
            );
            return None;
        }
        let end = matches!(raw.as_slice(), b"\r\n" | b"\n" | b"");
        lines.push(latin1(&raw));
        if lines.len() > MAX_HEADERS {
            connection.error(
                431,
                Some("Too many headers"),
                Some(&format!("got more than {MAX_HEADERS} headers")),
            );
            return None;
        }
        if end {
            break;
        }
    }
    // The header syntax of Python's email parser, which `http.server` relies on.
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut open = false;
    for line in &lines {
        if line.starts_with([' ', '\t']) {
            if open && let Some((_, value)) = headers.last_mut() {
                value.push_str(line);
            }
            continue;
        }
        open = false;
        let name_end = line.find(|c: char| !matches!(c, '!'..='9' | ';'..='~'));
        match name_end {
            Some(index) if line[index..].starts_with(':') => {
                if index > 0 {
                    headers.push((
                        line[..index].to_string(),
                        line[index + 1..]
                            .trim_start_matches([' ', '\t'])
                            .to_string(),
                    ));
                    open = true;
                }
            }
            _ if line.starts_with("From ") => {}
            _ => break,
        }
    }
    for (_, value) in &mut headers {
        let trimmed = value.trim_end_matches(['\r', '\n']).len();
        value.truncate(trimmed);
    }
    Some(Request {
        command,
        path,
        headers,
    })
}

/// `int(value)` for a decoded JSON value, with Python's refusals.
fn python_int(value: &Value) -> Result<i64> {
    match value {
        Value::Bool(flag) => Ok(i64::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => Ok(integer),
            None => match number.to_string().parse::<f64>() {
                Ok(float) if float.is_finite() => Ok(float.trunc() as i64),
                _ => Err(Error::Value(
                    "cannot convert float NaN to integer".to_string(),
                )),
            },
        },
        Value::String(text) => py::int(text),
        other => Err(Error::Value(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            json::type_name(other)
        ))),
    }
}

fn handle(stream: TcpStream, session: &Arc<Session>, secret: &str, port: u16) {
    let Ok(input) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(input);
    let mut connection = Connection {
        stream: &stream,
        simple: false,
        head: false,
    };
    let Some(request) = parse_request(&mut reader, &mut connection) else {
        return;
    };
    let route = {
        let host = request.header("Host");
        let prefix = format!("/{secret}/");
        let local = host == Some(&format!("127.0.0.1:{port}"))
            || host == Some(&format!("localhost:{port}"));
        if local {
            request.path.strip_prefix(&prefix)
        } else {
            None
        }
    };
    match request.command.as_str() {
        "GET" => match route {
            Some("") => connection.reply(200, PAGE.as_bytes(), "text/html; charset=utf-8"),
            Some("api/state") => {
                // A failure here ended the connection without an answer in Python, too.
                if let Ok(state) = session.state() {
                    connection.reply_json(200, &state);
                }
            }
            _ => connection.not_found(),
        },
        "POST" => {
            let answer = (|| -> Result<Option<(u16, Value)>> {
                let length = py::int(request.header("Content-Length").unwrap_or("0"))?;
                let Some(route) = route.filter(|_| {
                    request.header("Content-Type") == Some("application/json") && length <= MAX_BODY
                }) else {
                    return Ok(Some((404, json!({"error": "Not found"}))));
                };
                let mut raw = Vec::new();
                let read = match u64::try_from(length) {
                    Ok(length) => reader.by_ref().take(length).read_to_end(&mut raw),
                    // Python reads up to the end of the connection for a negative length.
                    Err(_) => reader.read_to_end(&mut raw),
                };
                if read.is_err() {
                    return Ok(None);
                }
                let text = py::decode(raw)?;
                let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
                let body = json::loads(if text.is_empty() { "{}" } else { text })?;
                let Some(fields) = body.as_object() else {
                    return Ok(None);
                };
                match route {
                    "api/run" => {
                        let action = json::text(fields.get("action").unwrap_or(&Value::Null));
                        let executable = fields
                            .get("executable")
                            .filter(|value| json::truthy(value))
                            .map(json::text);
                        session.run(&action, executable)?;
                    }
                    "api/select" => {
                        session.select(python_int(fields.get("index").unwrap_or(&json!(-1)))?)?
                    }
                    "api/pick" => {
                        return Ok(Some((
                            200,
                            json!({"path": guide::pick_file(session.system.as_ref())?}),
                        )));
                    }
                    "api/quit" => {
                        if session.busy() {
                            return Err(Error::patch("Finish the running step first."));
                        }
                        session.close();
                    }
                    _ => return Ok(Some((404, json!({"error": "Not found"})))),
                }
                Ok(Some((200, json!({"ok": true}))))
            })();
            match answer {
                Ok(Some((code, body))) => connection.reply_json(code, &body),
                Ok(None) => {}
                Err(error @ (Error::Patch(_) | Error::Value(_))) => {
                    connection.reply_json(400, &json!({"error": error.to_string()}))
                }
                Err(_) => {}
            }
        }
        other => connection.error(
            501,
            Some(&format!("Unsupported method ({})", repr(other))),
            None,
        ),
    }
}

/// The local web server; it stops when dropped.
pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// Listen on the loopback interface only and answer in the background.
    pub fn start(session: Arc<Session>, secret: &str, port: u16) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (stopping, secret) = (stop.clone(), secret.to_string());
        let worker = thread::Builder::new().spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (session, secret) = (session.clone(), secret.clone());
                        // One thread per connection, as Python's ThreadingHTTPServer.
                        let _ = thread::Builder::new().spawn(move || {
                            if stream.set_nonblocking(false).is_ok() {
                                handle(stream, &session, &secret, port);
                            }
                        });
                    }
                    Err(_) => {
                        let mut waiting = [rustix::event::PollFd::new(
                            &listener,
                            rustix::event::PollFlags::IN,
                        )];
                        let timeout = rustix::event::Timespec {
                            tv_sec: 0,
                            tv_nsec: 100_000_000,
                        };
                        let _ = rustix::event::poll(&mut waiting, Some(&timeout));
                    }
                }
            }
        })?;
        Ok(Self {
            port,
            stop,
            worker: Some(worker),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Show an address in the user's browser. Failing to is not an error: the
/// address is printed as well.
fn open_in_browser(url: &str) {
    let spawned = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = spawned {
        // Collect its exit status in the background, so that no zombie remains.
        let _ = thread::Builder::new().spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Serve the guided setup until it is closed, idle, or interrupted with Ctrl+C.
///
/// `interrupted` reports each Ctrl+C once. A running step is never cut short
/// by the first one.
pub fn serve(
    system: Arc<dyn System>,
    found: Vec<Arc<dyn Target>>,
    bundle: &OsStr,
    proton: Option<&OsStr>,
    open_browser: bool,
    interrupted: &dyn Fn() -> bool,
    out: &mut dyn Write,
) -> Result<String> {
    let session = Session::new(system, found, bundle, proton);
    let secret = py::token_urlsafe()?;
    let server = Server::start(session.clone(), &secret, 0)?;
    let url = format!("http://127.0.0.1:{}/{secret}/", server.port());
    writeln!(
        out,
        "Fenix A320 Linux Patch — setup is open in your browser.\nIf no window appeared, open: {url}\n\
         Keep this running until you are done; Ctrl+C or “Close installer” ends it."
    )?;
    out.flush()?;
    if open_browser {
        open_in_browser(&url);
    }
    let mut checked = Instant::now();
    loop {
        if session.wait_closed(Duration::from_millis(100)) {
            break;
        }
        if interrupted() {
            if session.busy() {
                writeln!(out, "\nA step is still running; wait for it to finish.")?;
                out.flush()?;
                while session.busy() {
                    if interrupted() {
                        return Err(Error::Interrupted);
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            break;
        }
        if checked.elapsed() >= Duration::from_secs(5) {
            checked = Instant::now();
            let idle = {
                let shared = session.shared();
                shared.busy.is_none() && shared.seen.elapsed() > Duration::from_secs(IDLE_SECONDS)
            };
            if idle {
                writeln!(out, "No browser window is connected any more; closing.")?;
                out.flush()?;
                break;
            }
        }
    }
    drop(server);
    Ok(url)
}
