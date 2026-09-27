//! Email commands: `gtd me email …`, `gtd me notifications …`, and
//! `gtd admin smtp …`.

use std::io::{self, IsTerminal, Read};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};

use super::ApiClient;

#[derive(Debug, Subcommand)]
pub(crate) enum EmailCommand {
    /// Show the account email address and whether it is verified.
    Show,
    /// Set the account email address and send a verification link.
    ///
    /// Token-authenticated requests must confirm the account password, because
    /// a verified address can reset it. The password is prompted for unless
    /// `--password-stdin` is given.
    Set {
        email: String,
        /// Read the current account password from standard input.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Send a new verification link for the unverified address.
    Resend,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NotificationsCommand {
    /// Show which notification emails are enabled.
    Show,
    /// Change notification emails; omitted switches keep their value.
    Set {
        /// Issues opened in owned repositories and issues assigned to you.
        #[arg(long)]
        issues: Option<bool>,
        /// Comments on issues you own, opened, or are assigned to.
        #[arg(long)]
        issue_comments: Option<bool>,
        /// Failed Actions runs triggered by your pushes.
        #[arg(long)]
        action_failures: Option<bool>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SmtpCommand {
    /// Show whether outgoing email is configured.
    Status,
    /// Send a test email and report the SMTP server's answer.
    Test {
        /// Recipient. Defaults to your verified account address.
        #[arg(long)]
        to: Option<String>,
    },
}

pub(crate) async fn run_email(api: &ApiClient, command: EmailCommand) -> Result<Value> {
    match command {
        EmailCommand::Show => api.request(Method::GET, "me/email", None).await,
        EmailCommand::Set {
            email,
            password_stdin,
        } => {
            if email.trim().is_empty() {
                bail!("email address is empty");
            }
            let password = read_password(password_stdin)?;
            api.request(
                Method::PUT,
                "me/email",
                Some(json!({"email": email, "current_password": password})),
            )
            .await
        }
        EmailCommand::Resend => {
            api.request(Method::POST, "me/email/verification", None)
                .await
        }
    }
}

pub(crate) async fn run_notifications(
    api: &ApiClient,
    command: NotificationsCommand,
) -> Result<Value> {
    match command {
        NotificationsCommand::Show => api.request(Method::GET, "me/notifications", None).await,
        NotificationsCommand::Set {
            issues,
            issue_comments,
            action_failures,
        } => {
            let current = api.request(Method::GET, "me/notifications", None).await?;
            let merged = |value: Option<bool>, key: &str| {
                value.unwrap_or_else(|| current.get(key).and_then(Value::as_bool).unwrap_or(true))
            };
            let body = json!({
                "issues": merged(issues, "issues"),
                "issue_comments": merged(issue_comments, "issue_comments"),
                "action_failures": merged(action_failures, "action_failures"),
            });
            api.request(Method::PUT, "me/notifications", Some(body))
                .await
        }
    }
}

pub(crate) async fn run_smtp(api: &ApiClient, command: SmtpCommand) -> Result<Value> {
    match command {
        SmtpCommand::Status => api.request(Method::GET, "admin/smtp", None).await,
        SmtpCommand::Test { to } => {
            api.request(Method::POST, "admin/smtp/test", Some(json!({"to": to})))
                .await
        }
    }
}

fn read_password(from_stdin: bool) -> Result<String> {
    let password = if from_stdin || !io::stdin().is_terminal() {
        let mut value = String::new();
        io::stdin()
            .read_to_string(&mut value)
            .context("could not read the password from stdin")?;
        value.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        rpassword::prompt_password("Current Gitadel password: ")
            .context("could not read the password")?
    };
    if password.is_empty() {
        bail!("password is empty");
    }
    Ok(password)
}
