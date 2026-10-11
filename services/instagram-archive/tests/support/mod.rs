//! A private `nats-server` with authorization enabled, for tests that need the broker to refuse
//! a publish the way the deployed ACL does (XR-021 CONTRACTS.md S03, R2-03).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test harness: a broker that cannot start or reload is the failure under test"
)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The user with every permission, standing in for the privileged Edge provisioning identity.
pub(crate) const FIXTURE_USER: (&str, &str) = ("fixture", "fixture-secret");
/// The user standing in for the Instagram identity, restricted to a publish allowlist.
pub(crate) const INSTAGRAM_USER: (&str, &str) = ("instagram", "instagram-secret");

/// A running authorization-enabled broker; the process and its files are removed on drop.
pub(crate) struct AuthBroker {
    child: Child,
    directory: PathBuf,
    port: u16,
}

#[expect(
    clippy::disallowed_methods,
    reason = "the integration binary may confine its private broker to an operator-chosen port range"
)]
fn free_port() -> u16 {
    let base = std::env::var("INSTAGRAM_ARCHIVE_TEST_PORT_BASE")
        .ok()
        .and_then(|value| value.parse::<u16>().ok());
    let candidates: Box<dyn Iterator<Item = u16>> = match base {
        Some(base) => Box::new(base..base.saturating_add(20)),
        None => Box::new(std::iter::once(0)),
    };
    for candidate in candidates {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", candidate)) {
            return listener.local_addr().expect("a bound address").port();
        }
    }
    panic!("no free loopback port for the private broker");
}

impl AuthBroker {
    /// Starts a broker whose Instagram user may publish only to `allowed_subjects`.
    pub(crate) fn start(allowed_subjects: &[&str]) -> Self {
        let port = free_port();
        let directory = std::env::temp_dir().join(format!(
            "instagram-auth-broker-{}-{port}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("the broker directory is created");
        write_config(&directory, port, allowed_subjects);
        let child = Command::new("nats-server")
            .arg("-c")
            .arg(directory.join(CONFIG_FILE))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect(
                "nats-server is installed and starts (the gate extracts it from the pinned image)",
            );
        let broker = Self {
            child,
            directory,
            port,
        };
        broker.wait_until_listening();
        broker
    }

    /// The client URL of this broker.
    pub(crate) fn url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.port)
    }

    /// Replaces the Instagram allowlist and makes the running server apply it (SIGHUP).
    pub(crate) fn reload(&self, allowed_subjects: &[&str]) {
        write_config(&self.directory, self.port, allowed_subjects);
        let status = Command::new("kill")
            .args(["-HUP", &self.child.id().to_string()])
            .status()
            .expect("SIGHUP is deliverable");
        assert!(status.success(), "the reload signal was delivered");
    }

    fn wait_until_listening(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if std::net::TcpStream::connect(("127.0.0.1", self.port)).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the private broker did not start listening");
    }
}

const CONFIG_FILE: &str = "nats.conf";

fn write_config(directory: &Path, port: u16, allowed_subjects: &[&str]) {
    let allowed = allowed_subjects
        .iter()
        .map(|subject| format!("\"{subject}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let config = format!(
        "listen: 127.0.0.1:{port}\n\
         jetstream {{ store_dir: \"{store}\" }}\n\
         authorization {{\n\
           users = [\n\
             {{ user: \"{fixture}\", password: \"{fixture_secret}\", \
                permissions: {{ publish: \">\", subscribe: \">\" }} }}\n\
             {{ user: \"{instagram}\", password: \"{instagram_secret}\", \
                permissions: {{ publish: {{ allow: [{allowed}] }}, \
                                subscribe: {{ allow: [\"_INBOX.>\"] }} }} }}\n\
           ]\n\
         }}\n",
        store = directory.join("js").display(),
        fixture = FIXTURE_USER.0,
        fixture_secret = FIXTURE_USER.1,
        instagram = INSTAGRAM_USER.0,
        instagram_secret = INSTAGRAM_USER.1,
    );
    std::fs::write(directory.join(CONFIG_FILE), config).expect("the broker config is written");
}

impl Drop for AuthBroker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
