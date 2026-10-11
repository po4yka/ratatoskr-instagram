//! The service reaches its Edge-provisioned durable and publishes through its real connection
//! function under the Instagram stanza of the deployed ACL (XR-021 CONTRACTS.md R3-01). That
//! stanza lets the identity subscribe only to `_INBOX_instagram.>`, so a connection without the
//! inbox prefix never receives a `JetStream` reply.

#![allow(
    clippy::expect_used,
    reason = "the isolated broker assertions are the integration contract"
)]

#[path = "support/stanza.rs"]
mod stanza;

use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use ratatoskr_instagram_archive::BusConfig;
use ratatoskr_instagram_archive_service::nats_transport::connect_bus;
use stanza::StanzaBroker;

const COMMANDS_STREAM: &str = "ratatoskr_commands";
const EVENTS_STREAM: &str = "ratatoskr_events";
const COMMAND_DURABLE: &str = "ratatoskr_instagram_browser_capture";
const COMMAND_SUBJECT: &str = "cmd.instagram.capture.requested.v1";
const REPORT_SUBJECT: &str = "evt.platform.operation.reported.v1";

/// Creates what Edge creates: both streams and the durable the Instagram identity may use, and
/// leaves one command in the stream.
async fn provision(broker: &StanzaBroker) {
    let administrator =
        async_nats::ConnectOptions::with_nkey(broker.administrator_seed().to_owned())
            .connect(broker.url())
            .await
            .expect("the provisioning identity connects");
    let context = jetstream::new(administrator);
    let commands = context
        .get_or_create_stream(jetstream::stream::Config {
            name: COMMANDS_STREAM.to_owned(),
            subjects: vec!["cmd.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the command stream exists");
    commands
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(COMMAND_DURABLE.to_owned()),
            filter_subject: COMMAND_SUBJECT.to_owned(),
            ack_policy: jetstream::consumer::AckPolicy::Explicit,
            ..jetstream::consumer::pull::Config::default()
        })
        .await
        .expect("the command durable exists");
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: EVENTS_STREAM.to_owned(),
            subjects: vec!["evt.>".to_owned()],
            ..jetstream::stream::Config::default()
        })
        .await
        .expect("the event stream exists");
    context
        .publish(COMMAND_SUBJECT, "{}".into())
        .await
        .expect("the command is accepted")
        .await
        .expect("the broker stores the command");
}

#[tokio::test]
async fn the_lane_completes_jetstream_requests_under_its_inbox_prefix() {
    let broker = StanzaBroker::start();
    provision(&broker).await;
    let bus = BusConfig {
        url: broker.url(),
        nkey_seed_path: Some(broker.instagram_seed_path().to_path_buf()),
    };

    let client = connect_bus(&bus)
        .await
        .expect("the service connects through its own connection function");
    let context = jetstream::new(client);
    let consumer: jetstream::consumer::PullConsumer = tokio::time::timeout(
        Duration::from_secs(10),
        context.get_consumer_from_stream(COMMAND_DURABLE, COMMANDS_STREAM),
    )
    .await
    .expect("the durable verification gets its reply under the Instagram stanza")
    .expect("the durable verifies under the Instagram stanza");
    let mut batch = tokio::time::timeout(
        Duration::from_secs(10),
        consumer.fetch().max_messages(1).messages(),
    )
    .await
    .expect("the pull request gets its reply under the Instagram stanza")
    .expect("the pull request is accepted");
    let message = tokio::time::timeout(Duration::from_secs(10), batch.next())
        .await
        .expect("the delivery arrives under the Instagram stanza")
        .expect("the durable holds the command")
        .expect("the delivery is valid");
    message
        .ack()
        .await
        .expect("the acknowledgement travels through the narrow $JS.ACK grant");

    let acknowledgement = tokio::time::timeout(Duration::from_secs(10), async {
        context
            .publish(REPORT_SUBJECT, "{}".into())
            .await
            .expect("the report is published")
            .await
    })
    .await
    .expect("the publish acknowledgement arrives under the Instagram stanza")
    .expect("the broker acknowledges the report");
    assert_eq!(
        acknowledgement.stream, EVENTS_STREAM,
        "the report is stored by Edge's stream"
    );
}
