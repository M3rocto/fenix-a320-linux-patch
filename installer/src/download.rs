// SPDX-License-Identifier: MIT
//! Pinned downloads: nothing is kept unless its SHA-256 is the expected one.
use crate::{
    error::{Error, Result},
    fsx,
    system::System,
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, RecvTimeoutError, sync_channel},
    thread,
    time::{Duration, Instant},
};
use ureq::ResponseExt;

/// Python's `urlopen(request, timeout=45)`: no single network step may take longer.
const TIMEOUT: Duration = Duration::from_secs(45);
const USER_AGENT: &str = "Fenix-Linux-Patch/0.1";
pub const DEFAULT_LIMIT: u64 = 256 * 1024 * 1024;

pub enum Message {
    Data(Vec<u8>),
    End,
    Failed(Error),
}

/// A response body arriving from another thread, so that a stalled server
/// and Ctrl+C are both noticed.
pub struct Stream(pub Receiver<Message>);

impl Stream {
    fn next(&self, system: &dyn System) -> Result<Option<Vec<u8>>> {
        let waiting = Instant::now();
        loop {
            system.interrupted()?;
            match self.0.recv_timeout(Duration::from_millis(100)) {
                Ok(Message::Data(data)) => return Ok(Some(data)),
                Ok(Message::End) => return Ok(None),
                Ok(Message::Failed(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) if waiting.elapsed() < TIMEOUT => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Error::os_text("The read operation timed out"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::os_text("<urlopen error connection closed>"));
                }
            }
        }
    }
}

fn failure(error: ureq::Error) -> Error {
    match error {
        ureq::Error::RequireHttpsOnly(_) => Error::patch("Download redirected away from HTTPS"),
        ureq::Error::StatusCode(code) => {
            let reason = ureq::http::StatusCode::from_u16(code)
                .ok()
                .and_then(|status| status.canonical_reason())
                .unwrap_or("");
            Error::os_text(format!("HTTP Error {code}: {reason}"))
        }
        other => Error::os_text(format!("<urlopen error {other}>")),
    }
}

/// Open an HTTPS address. Redirects are followed, but never to plain HTTP.
pub fn https(url: &str) -> Result<Stream> {
    let (sender, receiver) = sync_channel(1);
    let url = url.to_string();
    thread::spawn(move || {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .user_agent(USER_AGENT)
            .timeout_resolve(Some(TIMEOUT))
            .timeout_connect(Some(TIMEOUT))
            .timeout_send_request(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = match agent.get(&url).call() {
            Ok(response) => response,
            Err(error) => {
                let _ = sender.send(Message::Failed(failure(error)));
                return;
            }
        };
        if response.get_uri().scheme_str() != Some("https") {
            let _ = sender.send(Message::Failed(Error::patch(
                "Download redirected away from HTTPS",
            )));
            return;
        }
        let mut reader = response.into_body().into_reader();
        loop {
            let mut buffer = vec![0u8; 1024 * 1024];
            let message = match reader.read(&mut buffer) {
                Ok(0) => Message::End,
                Ok(count) => {
                    buffer.truncate(count);
                    Message::Data(buffer)
                }
                Err(error) => Message::Failed(Error::os_text(format!("<urlopen error {error}>"))),
            };
            let last = !matches!(message, Message::Data(_));
            if sender.send(message).is_err() || last {
                return;
            }
        }
    });
    Ok(Stream(receiver))
}

/// Download `url` to `destination` unless a file with the expected digest is already there.
pub fn download(
    system: &dyn System,
    url: &str,
    destination: &Path,
    expected: &str,
    max_size: u64,
) -> Result<PathBuf> {
    store(system, destination, expected, max_size, &mut || https(url))
}

/// Keep what `open` delivers as `destination`, provided size and digest are right.
pub fn store(
    system: &dyn System,
    destination: &Path,
    expected: &str,
    max_size: u64,
    open: &mut dyn FnMut() -> Result<Stream>,
) -> Result<PathBuf> {
    if destination.is_file() && !destination.is_symlink() && fsx::digest(destination)? == expected {
        return Ok(destination.to_path_buf());
    }
    let directory = fsx::parent(destination);
    fsx::mkdir_parents(directory, 0o777)?;
    let (mut output, temp) = fsx::mkstemp(directory, ".download-")?;
    let result = (|| {
        let stream = open()?;
        let mut size = 0u64;
        while let Some(chunk) = stream.next(system)? {
            size += chunk.len() as u64;
            if size > max_size {
                return Err(Error::patch("Download exceeds the allowed size"));
            }
            output
                .write_all(&chunk)
                .map_err(|error| Error::io_path(&error, &temp))?;
        }
        output
            .sync_all()
            .map_err(|error| Error::io_path(&error, &temp))?;
        if fsx::digest(&temp)? != expected {
            return Err(Error::patch(format!(
                "Download checksum mismatch: {}",
                fsx::name(destination)
            )));
        }
        fsx::rename(&temp, destination)
    })();
    let cleaned = fsx::unlink_missing_ok(&temp);
    result.and(cleaned).map(|()| destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::system::Host;
    use std::{fs, net::TcpListener};

    fn stream(parts: Vec<Message>) -> Result<Stream> {
        let (sender, receiver) = sync_channel(parts.len());
        for part in parts {
            sender.send(part).unwrap();
        }
        Ok(Stream(receiver))
    }

    fn entries(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fsx::list_dir(directory)
            .unwrap()
            .iter()
            .map(|path| fsx::name(path))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn only_a_complete_file_with_the_right_digest_is_kept() {
        let temp = tempfile::tempdir().unwrap();
        let host = Host::new(temp.path().to_path_buf());
        let target = temp.path().join("cache/package.bin");
        let digest = fsx::digest_bytes(b"hello world");
        let good = || {
            stream(vec![
                Message::Data(b"hello ".to_vec()),
                Message::Data(b"world".to_vec()),
                Message::End,
            ])
        };
        assert_eq!(
            store(&host, &target, &digest, 64, &mut || good()).unwrap(),
            target
        );
        assert_eq!(fs::read(&target).unwrap(), b"hello world");
        // An existing correct file is reused without opening the connection.
        store(&host, &target, &digest, 64, &mut || {
            panic!("must not download again")
        })
        .unwrap();
        let wrong = fsx::digest_bytes(b"something else");
        let error = store(&host, &target, &wrong, 64, &mut || good()).unwrap_err();
        assert_eq!(error.to_string(), "Download checksum mismatch: package.bin");
        assert_eq!(fs::read(&target).unwrap(), b"hello world");
        let error = store(&host, &target, &wrong, 10, &mut || good()).unwrap_err();
        assert_eq!(error.to_string(), "Download exceeds the allowed size");
        let error = store(&host, &target, &wrong, 64, &mut || {
            stream(vec![
                Message::Data(b"x".to_vec()),
                Message::Failed(Error::os_text("lost")),
            ])
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "lost");
        // A link in place of the file is not trusted, and no temporary file stays behind.
        let link = temp.path().join("cache/link.bin");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        store(&host, &link, &digest, 64, &mut || good()).unwrap();
        assert!(!link.is_symlink());
        assert_eq!(
            entries(&temp.path().join("cache")),
            ["link.bin", "package.bin"]
        );
    }

    #[test]
    fn plain_http_is_refused_before_anything_is_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let host = Host::new(temp.path().to_path_buf());
        let url = format!(
            "http://127.0.0.1:{}/file",
            listener.local_addr().unwrap().port()
        );
        let error = download(&host, &url, &temp.path().join("file"), "0", 64).unwrap_err();
        assert_eq!(error, Error::patch("Download redirected away from HTTPS"));
        assert!(listener.accept().is_err());
        assert_eq!(entries(temp.path()), Vec::<String>::new());
    }

    #[test]
    #[ignore = "needs the network: fetches a pinned file of 0.5 MiB from Microsoft"]
    fn pinned_file_arrives_over_https() {
        let temp = tempfile::tempdir().unwrap();
        let host = Host::new(temp.path().to_path_buf());
        let (name, url, expected) = crate::core::GEOMETRY_DOWNLOADS[1];
        let target = temp.path().join(name);
        assert_eq!(
            download(&host, url, &target, expected, 16 * 1024 * 1024).unwrap(),
            target
        );
        assert_eq!(fsx::digest(&target).unwrap(), expected);
        assert_eq!(entries(temp.path()), [name]);
        // A wrong digest keeps nothing, and a missing page is an error like Python's HTTPError.
        let error = download(
            &host,
            url,
            &temp.path().join("other"),
            "0",
            16 * 1024 * 1024,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Download checksum mismatch: other");
        let error = download(
            &host,
            &format!("{url}.missing"),
            &temp.path().join("other"),
            "0",
            1024,
        )
        .unwrap_err();
        assert!(
            error.is_os() && error.to_string().starts_with("HTTP Error 404"),
            "{error}"
        );
        assert_eq!(entries(temp.path()), [name]);
    }

    #[test]
    #[ignore = "needs the network: follows the redirect of a GitHub release and stops after 2 MiB"]
    fn redirects_are_followed_and_the_size_is_limited() {
        let temp = tempfile::tempdir().unwrap();
        let host = Host::new(temp.path().to_path_buf());
        let pinned = crate::steam::proton_downloads().remove(0);
        let error = download(
            &host,
            &pinned.url,
            &temp.path().join("proton.tar.xz"),
            &pinned.sha256,
            2 * 1024 * 1024,
        )
        .unwrap_err();
        assert_eq!(error, Error::patch("Download exceeds the allowed size"));
        assert_eq!(entries(temp.path()), Vec::<String>::new());
    }
}
