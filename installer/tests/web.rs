// SPDX-License-Identifier: MIT
//! The local guided setup, test for test as `tests/test_web.py`.
#![allow(clippy::unwrap_used)]
use fenix_patch::{
    Error, Result,
    json::{self, Value, json},
    system::{Host, Progress, System},
    targets::Target,
    web::{Server, Session},
};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Default)]
struct Fixture {
    installed: AtomicBool,
    release: Mutex<bool>,
    released: Condvar,
}

impl Fixture {
    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.released.notify_all();
    }
}

impl Target for Fixture {
    fn kind(&self) -> &str {
        "steam"
    }

    fn label(&self) -> String {
        "Steam · Fixture".to_string()
    }

    fn title(&self) -> String {
        "Fixture on Steam".to_string()
    }

    fn prefix(&self) -> PathBuf {
        PathBuf::from("/nonexistent/absent profile")
    }

    fn status(&self) -> Result<Value> {
        let installed = self.installed.load(Ordering::SeqCst);
        Ok(json!({
            "state": if installed { "installed" } else { "available" }, "installed": installed,
            "can_restore": installed, "idle": true, "message": "",
        }))
    }

    fn install(
        &self,
        _bundle: &OsStr,
        progress: Progress<'_>,
        _proton: Option<&OsStr>,
    ) -> Result<()> {
        progress("working")?;
        let released = self.release.lock().unwrap();
        let _ = self
            .released
            .wait_timeout_while(released, Duration::from_secs(5), |released| !*released)
            .unwrap();
        self.installed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn select(&self, _progress: Progress<'_>) -> Result<()> {
        Err(Error::patch("Quit Steam completely"))
    }

    fn configure(&self, _progress: Progress<'_>) -> Result<()> {
        Err(Error::other("not part of this fixture"))
    }

    fn restore(&self, _progress: Progress<'_>) -> Result<()> {
        Err(Error::other("not part of this fixture"))
    }

    fn app(
        &self,
        _progress: Progress<'_>,
        _executable: Option<&OsStr>,
        _manager: bool,
    ) -> Result<()> {
        Err(Error::other("not part of this fixture"))
    }
}

/// A machine whose home directory is empty, so that nothing of the real one is listed.
struct EmptyHome {
    host: Host,
    home: PathBuf,
}

impl System for EmptyHome {
    fn root(&self) -> &std::path::Path {
        self.host.root()
    }

    fn var(&self, name: &str) -> Option<std::ffi::OsString> {
        match name {
            "HOME" => Some(self.home.clone().into_os_string()),
            // No file dialog may ever be offered or started from a test.
            "PATH" => Some("/nonexistent".into()),
            _ => None,
        }
    }
}

struct Web {
    _temp: tempfile::TempDir,
    target: Arc<Fixture>,
    session: Arc<Session>,
    server: Server,
}

impl Web {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let system = Arc::new(EmptyHome {
            host: Host::new(temp.path().to_path_buf()),
            home: temp.path().to_path_buf(),
        });
        let target = Arc::new(Fixture::default());
        let session = Session::new(system, vec![target.clone()], OsStr::new("bundle"), None);
        let server = Server::start(session.clone(), "secret", 0).unwrap();
        Self {
            _temp: temp,
            target,
            session,
            server,
        }
    }

    /// One request on its own connection: the status and the body of the answer.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        host: Option<&str>,
        kind: &str,
    ) -> (u16, Vec<u8>) {
        let port = self.server.port();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAccept-Encoding: identity\r\n",
            host.map_or(format!("127.0.0.1:{port}"), str::to_string)
        );
        let body = body.map(|body| json::dumps(body, None)).unwrap_or_default();
        if !body.is_empty() {
            head.push_str(&format!(
                "Content-Length: {}\r\nContent-Type: {kind}\r\n",
                body.len()
            ));
        }
        self.raw(format!("{head}\r\n{body}").as_bytes())
    }

    fn raw(&self, request: &[u8]) -> (u16, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.server.port())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        let text = String::from_utf8_lossy(&answer);
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map_or(Vec::new(), |index| answer[index + 4..].to_vec());
        (status, body)
    }

    fn get(&self, path: &str) -> u16 {
        self.request("GET", path, None, None, "application/json").0
    }

    fn post(&self, path: &str, body: Value) -> u16 {
        self.request("POST", path, Some(&body), None, "application/json")
            .0
    }

    fn state(&self) -> Value {
        json::loads(
            &String::from_utf8(
                self.request("GET", "/secret/api/state", None, None, "application/json")
                    .1,
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn wait_idle(&self) {
        for _ in 0..100 {
            if !json::truthy(&self.state()["busy"]) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("operation did not finish");
    }
}

impl Drop for Web {
    fn drop(&mut self) {
        self.target.release();
    }
}

#[test]
fn only_the_secret_local_address_is_served() {
    let web = Web::new();
    assert_eq!(web.get("/secret/"), 200);
    assert_eq!(web.get("/"), 404);
    assert_eq!(web.get("/wrong/api/state"), 404);
    let attacker = format!("attacker.example:{}", web.server.port());
    assert_eq!(
        web.request(
            "GET",
            "/secret/api/state",
            None,
            Some(&attacker),
            "application/json"
        )
        .0,
        404
    );
    // A cross-site form cannot send JSON, and nothing may start that way.
    assert_eq!(
        web.request(
            "POST",
            "/secret/api/run",
            Some(&json!({"action": "install"})),
            None,
            "text/plain"
        )
        .0,
        404
    );
    assert!(!web.target.installed.load(Ordering::SeqCst) && !json::truthy(&web.state()["busy"]));
}

#[test]
fn one_step_runs_at_a_time_and_errors_are_shown() {
    let web = Web::new();
    assert_eq!(web.state()["view"]["current"], "install");
    assert_eq!(
        web.post("/secret/api/run", json!({"action": "install"})),
        200
    );
    let state = web.state();
    assert_eq!(
        (state["busy"].as_str(), state["view"]["current"].as_str()),
        (Some("install"), Some("install"))
    );
    assert_eq!(
        web.post("/secret/api/run", json!({"action": "install"})),
        400
    );
    assert_eq!(web.post("/secret/api/quit", json!({})), 400);
    web.target.release();
    web.wait_idle();
    let state = web.state();
    assert_eq!(
        (
            state["view"]["current"].as_str(),
            &state["log"],
            &state["error"]
        ),
        (Some("select"), &json!(["working"]), &Value::Null)
    );
    web.post("/secret/api/run", json!({"action": "select"}));
    web.wait_idle();
    assert!(
        web.state()["error"]
            .as_str()
            .unwrap()
            .contains("Quit Steam")
    );
    assert_eq!(
        web.post("/secret/api/run", json!({"action": "format-disk"})),
        200
    );
    web.wait_idle();
    assert_eq!(web.state()["error"], "Unknown action");
    assert_eq!(web.post("/secret/api/quit", json!({})), 200);
    assert!(web.session.is_closed());
}

#[test]
fn requests_are_bounded_and_answered_like_http_server() {
    // Beyond the Python tests: the limits and refusals of `http.server` that the port keeps.
    let web = Web::new();
    let port = web.server.port();
    let host = format!("Host: 127.0.0.1:{port}\r\n");
    let status = |request: String| web.raw(request.as_bytes()).0;
    // The page itself, byte for byte, and only for GET.
    let (code, page) = web.raw(format!("GET /secret/ HTTP/1.1\r\n{host}\r\n").as_bytes());
    assert_eq!(
        (code, page.as_slice()),
        (200, fenix_patch::web::PAGE.as_bytes())
    );
    assert_eq!(
        status(format!("GET /secret/?x=1 HTTP/1.1\r\n{host}\r\n")),
        404
    );
    assert_eq!(
        status(format!(
            "GET /secret/api/state HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n"
        )),
        200
    );
    assert_eq!(
        status(format!(
            "GET /secret/api/state HTTP/1.1\r\nHost: 127.0.0.1:{port} \r\n\r\n"
        )),
        404
    );
    assert_eq!(
        status(format!(
            "GET /secret/api/state HTTP/1.1\r\nHost : 127.0.0.1:{port}\r\n\r\n"
        )),
        404
    );
    assert_eq!(
        status("GET /secret/api/state HTTP/1.1\r\n\r\n".to_string()),
        404
    );
    for method in ["PUT", "DELETE", "HEAD", "OPTIONS", "get"] {
        assert_eq!(
            status(format!("{method} /secret/api/state HTTP/1.1\r\n{host}\r\n")),
            501,
            "{method}"
        );
    }
    assert_eq!(status(format!("GET /secret/ HTTP/2.0\r\n{host}\r\n")), 505);
    assert_eq!(status(format!("GET /secret/ FTP/1.0\r\n{host}\r\n")), 400);
    assert_eq!(
        status(format!("GET /secret/ extra HTTP/1.1\r\n{host}\r\n")),
        400
    );
    assert_eq!(
        status(format!(
            "GET /{} HTTP/1.1\r\n{host}\r\n",
            "a".repeat(70_000)
        )),
        414
    );
    assert_eq!(
        status(format!(
            "GET /secret/ HTTP/1.1\r\n{host}X: {}\r\n\r\n",
            "a".repeat(70_000)
        )),
        431
    );
    assert_eq!(
        status(format!(
            "GET /secret/ HTTP/1.1\r\n{host}{}\r\n",
            "X: 1\r\n".repeat(100)
        )),
        431
    );
    let post = |path: &str, headers: &str, body: &str| {
        status(format!("POST {path} HTTP/1.1\r\n{host}{headers}\r\n{body}"))
    };
    let kind = "Content-Type: application/json\r\n";
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: 12\r\n"),
            "{\"index\": 0}"
        ),
        200
    );
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: 12\r\n"),
            "{\"index\": 5}"
        ),
        400
    );
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: 2\r\n"),
            "{}"
        ),
        400
    );
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: 15\r\n"),
            "{\"index\": null}"
        ),
        400
    );
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: 1\r\n"),
            "x"
        ),
        400
    );
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: x\r\n"),
            ""
        ),
        400
    );
    assert_eq!(
        post(
            "/secret/api/select",
            &format!("{kind}Content-Length: 70000\r\n"),
            ""
        ),
        404
    );
    assert_eq!(
        post(
            "/secret/api/select",
            "Content-Type: application/json; charset=utf-8\r\nContent-Length: 2\r\n",
            "{}"
        ),
        404
    );
    assert_eq!(
        post(
            "/secret/api/unknown",
            &format!("{kind}Content-Length: 2\r\n"),
            "{}"
        ),
        404
    );
    assert_eq!(
        post(
            "/wrong/api/quit",
            &format!("{kind}Content-Length: 2\r\n"),
            "{}"
        ),
        404
    );
    assert!(!web.session.is_closed());
    // Without a body the request counts as an empty object.
    assert_eq!(post("/secret/api/quit", kind, ""), 200);
    assert!(web.session.is_closed());
}
