//! `gtd repo protection`: branch and tag protection rules.

use anyhow::{Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{ApiClient, route_path, route_repo};

#[derive(Debug, Subcommand)]
pub(crate) enum ProtectionCommand {
    /// List the repository's branch and tag protection rules.
    List { repository: String },
    /// Protect branches or tags matching an exact name or glob (`release/*`, `v*`).
    Add(AddRule),
    /// Remove a rule by ID or by pattern.
    Remove {
        repository: String,
        /// Rule ID, or the rule's pattern.
        rule: String,
        /// Which kind of rule a pattern refers to.
        #[arg(long, value_enum, default_value = "branch")]
        kind: RuleKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum RuleKind {
    Branch,
    Tag,
}

impl RuleKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Tag => "tag",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct AddRule {
    repository: String,
    pattern: String,
    #[arg(long, value_enum, default_value = "branch")]
    kind: RuleKind,
    /// Allow non-fast-forward pushes to matching branches.
    #[arg(long)]
    allow_force_push: bool,
    /// Allow deleting matching branches or tags.
    #[arg(long)]
    allow_deletion: bool,
    /// Allow moving existing matching tags.
    #[arg(long)]
    allow_tag_updates: bool,
    /// Only these users may push to matching refs (repeatable).
    #[arg(long = "push-user", value_name = "USERNAME")]
    push_users: Vec<String>,
    /// Do not exempt repository admins from the push user list.
    #[arg(long)]
    no_admin_bypass: bool,
}

impl AddRule {
    fn body(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "pattern": self.pattern,
            "block_force_push": self.kind == RuleKind::Branch && !self.allow_force_push,
            "block_deletion": !self.allow_deletion,
            "block_update": self.kind == RuleKind::Tag && !self.allow_tag_updates,
            "restrict_pushes": !self.push_users.is_empty(),
            "admins_bypass": !self.no_admin_bypass,
            "allowed_users": self.push_users,
        })
    }
}

pub(crate) async fn run(api: &ApiClient, command: ProtectionCommand) -> Result<Value> {
    match command {
        ProtectionCommand::List { repository } => {
            api.request(Method::GET, &rules_path(&repository)?, None)
                .await
        }
        ProtectionCommand::Add(rule) => {
            api.request(
                Method::POST,
                &rules_path(&rule.repository)?,
                Some(rule.body()),
            )
            .await
        }
        ProtectionCommand::Remove {
            repository,
            rule,
            kind,
        } => {
            let collection = rules_path(&repository)?;
            let id = match Uuid::parse_str(&rule) {
                Ok(id) => id,
                Err(_) => {
                    let rules = api.request(Method::GET, &collection, None).await?;
                    find_rule(&rules, &rule, kind)?
                }
            };
            api.request(
                Method::DELETE,
                &route_path(&collection, &[&id.to_string()])?,
                None,
            )
            .await
        }
    }
}

fn rules_path(repository: &str) -> Result<String> {
    Ok(route_repo("repositories", repository)? + "/protection-rules")
}

fn find_rule(rules: &Value, pattern: &str, kind: RuleKind) -> Result<Uuid> {
    let found = rules.as_array().into_iter().flatten().find(|rule| {
        rule.get("pattern").and_then(Value::as_str) == Some(pattern)
            && rule.get("kind").and_then(Value::as_str) == Some(kind.as_str())
    });
    let Some(id) = found
        .and_then(|rule| rule.get("id"))
        .and_then(Value::as_str)
    else {
        bail!("no {} protection rule matches `{pattern}`", kind.as_str());
    };
    Ok(Uuid::parse_str(id)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(kind: RuleKind) -> AddRule {
        AddRule {
            repository: "owner/name".to_owned(),
            pattern: "release/*".to_owned(),
            kind,
            allow_force_push: false,
            allow_deletion: false,
            allow_tag_updates: false,
            push_users: Vec::new(),
            no_admin_bypass: false,
        }
    }

    #[test]
    fn branch_rules_block_force_push_and_deletion_by_default() {
        let body = rule(RuleKind::Branch).body();
        assert_eq!(body["block_force_push"], true);
        assert_eq!(body["block_deletion"], true);
        assert_eq!(body["block_update"], false);
        assert_eq!(body["restrict_pushes"], false);
    }

    #[test]
    fn push_users_turn_on_restrictions() {
        let mut tag = rule(RuleKind::Tag);
        tag.push_users = vec!["alice".to_owned()];
        tag.no_admin_bypass = true;
        let body = tag.body();
        assert_eq!(body["block_update"], true);
        assert_eq!(body["block_force_push"], false);
        assert_eq!(body["restrict_pushes"], true);
        assert_eq!(body["admins_bypass"], false);
        assert_eq!(body["allowed_users"], json!(["alice"]));
    }

    #[test]
    fn rules_are_found_by_pattern_and_kind() {
        let id = Uuid::new_v4();
        let rules = json!([
            {"id": Uuid::new_v4(), "kind": "tag", "pattern": "main"},
            {"id": id, "kind": "branch", "pattern": "main"},
        ]);
        assert_eq!(find_rule(&rules, "main", RuleKind::Branch).unwrap(), id);
        assert!(find_rule(&rules, "develop", RuleKind::Branch).is_err());
    }
}
