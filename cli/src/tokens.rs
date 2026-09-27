//! `gtd me token`: list, create, and revoke personal API tokens.
//!
//! The server currently manages API tokens only from a browser session, so
//! these commands receive 403 when authenticated with an API token.

use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};

use crate::{ApiClient, id, route_path};

#[derive(Debug, Subcommand)]
pub(crate) enum TokenCommand {
    /// List active API tokens.
    List,
    /// Create an API token. The secret is printed once and cannot be shown again.
    Create {
        /// Descriptive token name.
        name: String,
        /// Comma-separated scopes to grant.
        #[arg(long, value_enum, value_delimiter = ',', required = true)]
        scopes: Vec<TokenScope>,
        /// Expire the token after this many days (1-3650); omit for no expiry.
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..=3650))]
        expires_in_days: Option<i64>,
    },
    /// Revoke an API token by ID.
    Revoke { id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TokenScope {
    Read,
    Write,
    #[value(name = "ssh_keys", alias = "ssh-keys")]
    SshKeys,
}

impl TokenScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::SshKeys => "ssh_keys",
        }
    }
}

fn create_body(name: &str, scopes: &[TokenScope], expires_in_days: Option<i64>) -> Value {
    let mut names: Vec<&str> = scopes.iter().map(|scope| scope.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    json!({
        "name": name,
        "scopes": names,
        "expires_in_days": expires_in_days,
    })
}

pub(crate) async fn run(api: &ApiClient, command: TokenCommand) -> Result<Value> {
    match command {
        TokenCommand::List => api.request(Method::GET, "me/tokens", None).await,
        TokenCommand::Create {
            name,
            scopes,
            expires_in_days,
        } => {
            let created = api
                .request(
                    Method::POST,
                    "me/tokens",
                    Some(create_body(&name, &scopes, expires_in_days)),
                )
                .await?;
            eprintln!("Store the token now; Gitadel will not show it again.");
            Ok(created)
        }
        TokenCommand::Revoke { id: value } => {
            let path = route_path("me/tokens", &[&id(&value)?.to_string()])?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::{Cli, Command, MeCommand};

    fn parse(args: &[&str]) -> Result<TokenCommand, clap::Error> {
        let mut argv = vec!["gtd", "me", "token"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Command::Me {
                command: MeCommand::Token { command },
            } => Ok(command),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn create_parses_comma_separated_scopes() {
        let command = parse(&["create", "ci", "--scopes", "read,ssh-keys"]).unwrap();
        let TokenCommand::Create { scopes, .. } = command else {
            panic!("expected create");
        };
        assert_eq!(scopes, [TokenScope::Read, TokenScope::SshKeys]);
    }

    #[test]
    fn create_requires_scopes() {
        assert!(parse(&["create", "ci"]).is_err());
    }

    #[test]
    fn create_rejects_out_of_range_expiry() {
        assert!(parse(&["create", "ci", "--scopes", "read", "--expires-in-days", "0"]).is_err());
    }

    #[test]
    fn create_body_uses_server_scope_names() {
        let body = create_body(
            "ci",
            &[TokenScope::Write, TokenScope::SshKeys, TokenScope::Write],
            Some(30),
        );
        assert_eq!(
            body,
            json!({"name": "ci", "scopes": ["ssh_keys", "write"], "expires_in_days": 30})
        );
    }
}
