//! A private `nats-server` that runs the stanza shipped in `deploy/nats/identity.conf`, the way
//! the deployed ACL does (XR-021 CONTRACTS.md S03 and R3-01).
//!
//! The broker's users are an nkey administrator with every permission, standing in for Edge, and
//! the Instagram identity exactly as the shipped file spells it, with only its public-key
//! placeholder replaced by a generated key.

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "test harness: a broker that cannot start is the failure under test, and each test binary uses a part of it"
)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The prefix of the placeholder the stanza carries in place of the public nkey.
const PLACEHOLDER: &str = "UREPLACE_ME_WITH_THE_PUBLIC_NKEY_OF_RATATOSKR_INSTAGRAM_";

/// A running broker with the shipped Instagram stanza; the process and its files are removed on
/// drop.
pub(crate) struct StanzaBroker {
    child: Child,
    directory: PathBuf,
    port: u16,
    administrator_seed: String,
    instagram_seed_path: PathBuf,
    knowledge_seed: String,
}

/// The only subject the test Knowledge identity may publish.
pub(crate) const ACK_SUBJECT: &str = "evt.knowledge.deletion.acknowledged.v1";

/// The inbox prefix the test Knowledge identity connects under; its stanza lets it subscribe there
/// and nowhere else.
pub(crate) const KNOWLEDGE_INBOX_PREFIX: &str = "_INBOX_knowledge";

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

/// The shipped stanza with its placeholder replaced by `public_key`.
fn shipped_stanza(public_key: &str) -> String {
    let stanza = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/nats/identity.conf"
    ))
    .expect("deploy/nats/identity.conf is shipped");
    let start = stanza
        .find(PLACEHOLDER)
        .expect("the shipped stanza carries the Instagram public-key placeholder");
    let length = stanza.get(start..).map_or(0, |rest| {
        rest.chars()
            .take_while(|character| character.is_ascii_uppercase() || *character == '_')
            .count()
    });
    format!(
        "{}{public_key}{}",
        stanza.get(..start).unwrap_or_default(),
        stanza.get(start + length..).unwrap_or_default()
    )
}

impl StanzaBroker {
    /// Starts the broker with `JetStream`, the administrator and the shipped Instagram stanza.
    pub(crate) fn start() -> Self {
        let port = free_port();
        let directory = std::env::temp_dir().join(format!(
            "instagram-stanza-broker-{}-{port}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("the broker directory is created");
        let administrator = nkeys::KeyPair::new_user();
        let instagram = nkeys::KeyPair::new_user();
        let knowledge = nkeys::KeyPair::new_user();
        let config = format!(
            "listen: 127.0.0.1:{port}\n\
             jetstream {{ store_dir: \"{store}\" }}\n\
             authorization {{\n  users = [\n{stanza}\n    {{ nkey: {admin} }}\n    \
             {{ nkey: {knowledge}, permissions: {{ publish: {{ allow: [\"{ACK_SUBJECT}\"] }}, \
             subscribe: {{ allow: [\"{KNOWLEDGE_INBOX_PREFIX}.>\"] }} }} }}\n  ]\n}}\n",
            store = directory.join("js").display(),
            stanza = shipped_stanza(&instagram.public_key()),
            admin = administrator.public_key(),
            knowledge = knowledge.public_key(),
        );
        let config_path = directory.join("nats.conf");
        std::fs::write(&config_path, config).expect("the broker config is written");
        let instagram_seed_path = directory.join("instagram.nkey");
        std::fs::write(
            &instagram_seed_path,
            instagram.seed().expect("an Instagram seed"),
        )
        .expect("the seed is written");
        let child = Command::new("nats-server")
            .arg("-c")
            .arg(&config_path)
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
            administrator_seed: administrator.seed().expect("an administrator seed"),
            instagram_seed_path,
            knowledge_seed: knowledge.seed().expect("a Knowledge seed"),
        };
        broker.wait_until_listening();
        broker
    }

    /// The client URL of this broker.
    pub(crate) fn url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.port)
    }

    /// The seed of the administrator that provisions topology.
    pub(crate) fn administrator_seed(&self) -> &str {
        &self.administrator_seed
    }

    /// The seed of the test Knowledge identity, which may publish only the acknowledgement subject.
    pub(crate) fn knowledge_seed(&self) -> &str {
        &self.knowledge_seed
    }

    /// The file holding the Instagram identity's seed.
    pub(crate) fn instagram_seed_path(&self) -> &Path {
        &self.instagram_seed_path
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

impl Drop for StanzaBroker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
