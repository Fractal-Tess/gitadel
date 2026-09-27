//! Administrator account and invitation commands, and organization deletion.

use anyhow::{Result, bail};
use clap::Subcommand;
use reqwest::Method;
use serde_json::Value;

use crate::{ApiClient, route_path};

#[derive(Debug, Subcommand)]
pub(crate) enum UserCommand {
    /// List accounts, optionally filtered by a username substring.
    List {
        #[arg(long, default_value = "")]
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: u64,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Disable an account, signing it out and revoking its API tokens.
    Disable { username: String },
    /// Re-enable a disabled account. Revoked tokens stay revoked.
    Enable { username: String },
    /// Permanently delete an account that owns no repositories or authored history.
    Delete {
        username: String,
        /// Must repeat the username.
        #[arg(long)]
        confirmation: String,
    },
    /// Remove an account's authenticator and recovery codes.
    ResetTwoFactor { username: String },
}

#[derive(Debug, Subcommand)]
pub(crate) enum InvitationCommand {
    /// List invitations that are neither used nor expired.
    List,
    /// Revoke a pending invitation by the id shown in `list`.
    Revoke { id: String },
}

pub(crate) async fn run_user(api: &ApiClient, command: UserCommand) -> Result<Value> {
    match command {
        UserCommand::List {
            query,
            limit,
            offset,
        } => {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("q", &query)
                .append_pair("limit", &limit.clamp(1, 100).to_string())
                .append_pair("offset", &offset.to_string())
                .finish();
            api.request(Method::GET, &format!("admin/users?{query}"), None)
                .await
        }
        UserCommand::Disable { username } => {
            let path = route_path("admin/users", &[&username, "disable"])?;
            api.request(Method::POST, &path, None).await
        }
        UserCommand::Enable { username } => {
            let path = route_path("admin/users", &[&username, "enable"])?;
            api.request(Method::POST, &path, None).await
        }
        UserCommand::Delete {
            username,
            confirmation,
        } => {
            if confirmation != username {
                bail!("--confirmation must repeat the username");
            }
            let path = route_path("admin/users", &[&username])?;
            api.request(Method::DELETE, &path, None).await
        }
        UserCommand::ResetTwoFactor { username } => {
            let path = route_path("admin/users", &[&username, "two-factor"])?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

pub(crate) async fn run_invitation(api: &ApiClient, command: InvitationCommand) -> Result<Value> {
    match command {
        InvitationCommand::List => api.request(Method::GET, "invitations", None).await,
        InvitationCommand::Revoke { id } => {
            let path = route_path("invitations", &[&id])?;
            api.request(Method::DELETE, &path, None).await
        }
    }
}

pub(crate) async fn delete_organization(
    api: &ApiClient,
    slug: &str,
    confirmation: &str,
) -> Result<Value> {
    if confirmation != slug {
        bail!("--confirmation must repeat the organization name");
    }
    api.request(Method::DELETE, &route_path("organizations", &[slug])?, None)
        .await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::{AdminCommand, Cli, Command, OrgCommand};

    use super::*;

    #[test]
    fn admin_user_delete_parses_confirmation() {
        let cli = Cli::try_parse_from([
            "gtd",
            "admin",
            "user",
            "delete",
            "bob",
            "--confirmation",
            "bob",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Admin {
                command: AdminCommand::User {
                    command: UserCommand::Delete { .. }
                }
            }
        ));
    }

    #[test]
    fn org_delete_requires_confirmation() {
        assert!(Cli::try_parse_from(["gtd", "org", "delete", "team"]).is_err());
        let cli = Cli::try_parse_from(["gtd", "org", "delete", "team", "--confirmation", "team"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Command::Org {
                command: OrgCommand::Delete { .. }
            }
        ));
    }

    #[tokio::test]
    async fn mismatched_user_confirmation_is_rejected_before_any_request() {
        let api = ApiClient::new("http://127.0.0.1:9".parse().unwrap(), None).unwrap();
        let error = run_user(
            &api,
            UserCommand::Delete {
                username: "bob".to_owned(),
                confirmation: "alice".to_owned(),
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("--confirmation"));
    }
}
