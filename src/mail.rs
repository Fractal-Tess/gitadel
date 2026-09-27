//! Outgoing email over SMTP.
//!
//! Mail is optional: without an `[smtp]` section the [`Mailer`] is disabled
//! and every email feature is hidden. Request handlers only ever enqueue
//! messages; a single background worker delivers them, so a slow or failing
//! relay never delays a request.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::config::{SmtpSettings, SmtpTls};

/// Messages waiting beyond this are dropped with a warning rather than
/// letting a dead relay grow memory without bound.
const QUEUE_CAPACITY: usize = 512;
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(30)];

/// One plain-text message to a single recipient.
#[derive(Clone, Debug)]
pub struct Email {
    pub to: Mailbox,
    pub subject: String,
    pub body: String,
}

/// Non-secret view of the configuration, for the administration page.
#[derive(Clone, Debug, Serialize)]
pub struct SmtpSummary {
    pub host: String,
    pub port: u16,
    pub tls: &'static str,
    pub from: String,
    pub username: Option<String>,
}

#[derive(Clone, Default)]
pub struct Mailer {
    inner: Option<Arc<Inner>>,
}

struct Inner {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    queue: mpsc::Sender<Message>,
    summary: SmtpSummary,
}

impl Mailer {
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Build the transport and start the delivery worker.
    ///
    /// Must be called within a Tokio runtime. No connection is made here, so
    /// an unreachable relay does not prevent startup.
    pub fn start(settings: &SmtpSettings) -> Result<Self> {
        let from = settings
            .from
            .parse::<Mailbox>()
            .context("smtp.from is not a valid mailbox")?;
        let port = settings.port();
        let mut builder = match settings.tls {
            SmtpTls::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host)
                    .context("could not configure SMTP STARTTLS")?
            }
            SmtpTls::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.host)
                .context("could not configure SMTP TLS")?,
            SmtpTls::None => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host)
            }
        }
        .port(port)
        .timeout(Some(SEND_TIMEOUT));
        if let (Some(username), Some(password)) =
            (settings.username.clone(), settings.resolve_password()?)
        {
            builder = builder.credentials(Credentials::new(username, password));
        }
        let transport = builder.build();
        let (queue, receiver) = mpsc::channel(QUEUE_CAPACITY);
        tokio::spawn(deliver_queue(transport.clone(), receiver));
        Ok(Self {
            inner: Some(Arc::new(Inner {
                transport,
                from: from.clone(),
                queue,
                summary: SmtpSummary {
                    host: settings.host.clone(),
                    port,
                    tls: settings.tls.as_str(),
                    from: from.to_string(),
                    username: settings.username.clone(),
                },
            })),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn summary(&self) -> Option<&SmtpSummary> {
        self.inner.as_deref().map(|inner| &inner.summary)
    }

    /// Queue a message for background delivery. Returns whether it was
    /// accepted; failures are logged, never surfaced to the requester.
    pub fn enqueue(&self, email: Email) -> bool {
        let Some(inner) = self.inner.as_deref() else {
            return false;
        };
        let message = match build_message(&inner.from, email) {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!(%error, "could not build email");
                return false;
            }
        };
        match inner.queue.try_send(message) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "email queue is full or closed; message dropped");
                false
            }
        }
    }

    /// Deliver one message immediately and report the relay's answer.
    ///
    /// Only for the administrator's explicit connectivity test, where the
    /// result is the point of the request.
    pub async fn send_now(&self, email: Email) -> Result<(), String> {
        let inner = self
            .inner
            .as_deref()
            .ok_or_else(|| "SMTP is not configured.".to_owned())?;
        let message = build_message(&inner.from, email).map_err(|error| error.to_string())?;
        match tokio::time::timeout(SEND_TIMEOUT, inner.transport.send(message)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("The SMTP server did not answer in time.".to_owned()),
        }
    }
}

fn build_message(from: &Mailbox, email: Email) -> Result<Message> {
    Message::builder()
        .from(from.clone())
        .to(email.to)
        .subject(email.subject)
        .header(ContentType::TEXT_PLAIN)
        .body(email.body)
        .context("could not build email message")
}

async fn deliver_queue(
    transport: AsyncSmtpTransport<Tokio1Executor>,
    mut receiver: mpsc::Receiver<Message>,
) {
    while let Some(message) = receiver.recv().await {
        let mut attempt = 0;
        loop {
            match transport.send(message.clone()).await {
                Ok(_) => break,
                Err(error) if error.is_permanent() || attempt >= RETRY_DELAYS.len() => {
                    tracing::warn!(%error, "could not deliver email");
                    break;
                }
                Err(error) => {
                    tracing::info!(%error, attempt, "email delivery failed; retrying");
                    tokio::time::sleep(RETRY_DELAYS[attempt]).await;
                    attempt += 1;
                }
            }
        }
    }
}

/// Parse and normalise a user-supplied address.
pub fn parse_address(value: &str) -> Option<lettre::Address> {
    let value = value.trim();
    if value.is_empty() || value.len() > 254 || value.contains(['<', '>', ' ']) {
        return None;
    }
    value.to_ascii_lowercase().parse().ok()
}

/// Accept one SMTP session on a local port and return the first message's
/// DATA section.
#[cfg(test)]
pub(crate) async fn fake_smtp_server() -> (u16, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        let mut data = String::new();
        let mut in_data = false;
        while let Some(line) = lines.next_line().await.unwrap() {
            if in_data {
                if line == "." {
                    writer.write_all(b"250 queued\r\n").await.unwrap();
                    break;
                }
                data.push_str(&line);
                data.push('\n');
                continue;
            }
            let reply: &[u8] = if line.starts_with("DATA") {
                in_data = true;
                b"354 go ahead\r\n"
            } else {
                b"250 ok\r\n"
            };
            writer.write_all(reply).await.unwrap();
        }
        data
    });
    (port, handle)
}

/// A mailer that talks to [`fake_smtp_server`] without TLS.
#[cfg(test)]
pub(crate) fn test_mailer(port: u16) -> Mailer {
    Mailer::start(&SmtpSettings {
        host: "127.0.0.1".to_owned(),
        port: Some(port),
        tls: SmtpTls::None,
        username: None,
        password: None,
        password_file: None,
        from: "Gitadel <gitadel@example.com>".to_owned(),
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalised_and_bare() {
        assert_eq!(
            parse_address("  Alice@Example.COM ").map(|address| address.to_string()),
            Some("alice@example.com".to_owned())
        );
        assert!(parse_address("Alice <alice@example.com>").is_none());
        assert!(parse_address("not-an-address").is_none());
        assert!(parse_address("").is_none());
    }

    #[test]
    fn disabled_mailer_accepts_nothing() {
        let mailer = Mailer::disabled();
        assert!(!mailer.is_enabled());
        assert!(!mailer.enqueue(Email {
            to: "alice@example.com".parse().unwrap(),
            subject: "Hello".to_owned(),
            body: "Body".to_owned(),
        }));
    }

    #[tokio::test]
    async fn messages_are_delivered_over_smtp() {
        let (port, server) = fake_smtp_server().await;
        let mailer = test_mailer(port);
        mailer
            .send_now(Email {
                to: "alice@example.com".parse().unwrap(),
                subject: "Gitadel test email".to_owned(),
                body: "Outgoing email is working.".to_owned(),
            })
            .await
            .unwrap();
        let data = tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap();
        assert!(data.contains("Subject: Gitadel test email"));
        assert!(data.contains("To: alice@example.com"));
        assert!(data.contains("Outgoing email is working."));
    }

    #[tokio::test]
    async fn configured_mailer_reports_a_summary_without_secrets() {
        let mailer = Mailer::start(&SmtpSettings {
            host: "127.0.0.1".to_owned(),
            port: Some(2525),
            tls: SmtpTls::None,
            username: Some("mailer".to_owned()),
            password: Some("hunter2".to_owned()),
            password_file: None,
            from: "Gitadel <gitadel@example.com>".to_owned(),
        })
        .unwrap();
        let summary = mailer.summary().unwrap();
        assert_eq!(summary.port, 2525);
        assert_eq!(summary.tls, "none");
        assert!(!serde_json::to_string(summary).unwrap().contains("hunter2"));
    }
}
