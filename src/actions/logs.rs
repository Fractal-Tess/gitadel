use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, DatabaseConnection, EntityTrait, IntoActiveModel, Set, TransactionTrait,
};

use crate::entity::{action_job, action_job_log};

pub(crate) struct IncomingLogRow {
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) content: String,
}

pub(crate) async fn append(
    database: &DatabaseConnection,
    runner_id: i64,
    task_id: i64,
    start_index: i64,
    rows: Vec<IncomingLogRow>,
    max_row_bytes: usize,
    max_job_bytes: i64,
) -> Result<i64, LogError> {
    if start_index < 0 {
        return Err(LogError::Invalid(
            "log index must be nonnegative".to_owned(),
        ));
    }
    let transaction = database.begin().await?;
    let job = action_job::Entity::find_by_id(task_id)
        .one(&transaction)
        .await?
        .filter(|job| job.runner_id == Some(runner_id))
        .ok_or(LogError::ForeignTask)?;
    if start_index > job.expected_log_index {
        return Err(LogError::Invalid("log rows must be contiguous".to_owned()));
    }

    let mut expected = job.expected_log_index;
    let mut bytes = job.log_bytes;
    let mut truncated = job.log_truncated;
    for (offset, row) in rows.into_iter().enumerate() {
        let index = start_index
            + i64::try_from(offset)
                .map_err(|_| LogError::Invalid("log upload contains too many rows".to_owned()))?;
        let content = normalize(&row.content);
        if content.len() > max_row_bytes {
            return Err(LogError::Invalid(
                "log row exceeds the configured limit".to_owned(),
            ));
        }
        if index < expected {
            let stored = action_job_log::Entity::find_by_id((task_id, index))
                .one(&transaction)
                .await?;
            if stored
                .as_ref()
                .is_some_and(|stored| stored.content == content)
                || truncated
            {
                continue;
            }
            return Err(LogError::Invalid(
                "log retry conflicts with persisted content".to_owned(),
            ));
        }
        if index != expected {
            return Err(LogError::Invalid("log rows must be contiguous".to_owned()));
        }
        if bytes + content.len() as i64 <= max_job_bytes {
            let content_bytes = content.len() as i64;
            action_job_log::ActiveModel {
                job_id: Set(task_id),
                row_index: Set(index),
                timestamp: Set(row.timestamp),
                byte_count: Set(content_bytes),
                content: Set(content),
            }
            .insert(&transaction)
            .await?;
            bytes += content_bytes;
        } else if !truncated {
            let marker = "[Gitadel Actions log truncated at the configured limit]".to_owned();
            action_job_log::ActiveModel {
                job_id: Set(task_id),
                row_index: Set(index),
                timestamp: Set(Utc::now()),
                byte_count: Set(marker.len() as i64),
                content: Set(marker),
            }
            .insert(&transaction)
            .await?;
            truncated = true;
        }
        expected += 1;
    }
    let mut active = job.into_active_model();
    active.expected_log_index = Set(expected);
    active.log_bytes = Set(bytes);
    active.log_truncated = Set(truncated);
    active.update(&transaction).await?;
    transaction.commit().await?;
    Ok(expected)
}

fn normalize(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.next_if_eq(&'[').is_some() {
                while chars
                    .next()
                    .is_some_and(|next| !(('@'..='~').contains(&next)))
                {}
            }
            continue;
        }
        if character == '\t' || !character.is_control() {
            output.push(character);
        }
    }
    redact_tokens(output)
}

fn redact_tokens(mut content: String) -> String {
    for prefix in ["gta_reg_", "gta_runner_", "gta_job_"] {
        while let Some(start) = content.find(prefix) {
            let end = content[start..]
                .find(char::is_whitespace)
                .map_or(content.len(), |offset| start + offset);
            content.replace_range(start..end, "***");
        }
    }
    content
}

#[derive(Debug)]
pub(crate) enum LogError {
    ForeignTask,
    Invalid(String),
    Database(sea_orm::DbErr),
}

impl From<sea_orm::DbErr> for LogError {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_strips_terminal_controls_and_credentials() {
        assert_eq!(
            normalize("\u{1b}[31mred\u{1b}[0m gta_job_secret\n"),
            "red ***"
        );
    }
}
