//! Cron validation, persisted schedules, and atomic occurrence claims.
use chrono::{DateTime, Utc};
use gitadel_db::entity::{action_schedule, action_schedule_scan};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, QuerySelect, Set, sea_query::Expr,
};
use std::{collections::BTreeSet, str::FromStr as _};
use uuid::Uuid;
use yaml_serde::Value;

const MAX_CRONS_PER_WORKFLOW: usize = 16;
const MAX_DUE_PER_TICK: u64 = 64;
const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Parses `on.schedule`, returning its cron expressions.
pub fn parse_crons(value: &Value) -> Result<Vec<String>, String> {
    let entries = value
        .as_sequence()
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| "schedule must be a non-empty list of `cron` entries".to_owned())?;
    if entries.len() > MAX_CRONS_PER_WORKFLOW {
        return Err(format!(
            "schedule supports at most {MAX_CRONS_PER_WORKFLOW} cron entries"
        ));
    }
    let mut crons = Vec::with_capacity(entries.len());
    for entry in entries {
        let cron = entry
            .as_mapping()
            .filter(|entry| entry.len() == 1)
            .and_then(|entry| entry.get("cron"))
            .and_then(Value::as_str)
            .ok_or_else(|| "schedule entries must be `- cron: \"...\"`".to_owned())?;
        let cron = cron.split_whitespace().collect::<Vec<_>>().join(" ");
        next_fire(&cron, Utc::now())?;
        if !crons.contains(&cron) {
            crons.push(cron);
        }
    }
    Ok(crons)
}

/// The next occurrence of a five-field POSIX cron expression in UTC.
pub fn next_fire(cron: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let translated = translate(cron).map_err(|error| format!("cron `{cron}` {error}"))?;
    cron::Schedule::from_str(&translated)
        .map_err(|_| format!("cron `{cron}` is not a valid cron expression"))?
        .after(&after)
        .next()
        .ok_or_else(|| format!("cron `{cron}` has no future occurrence"))
}

/// Converts POSIX cron (`min hour dom month dow`, Sunday = 0 or 7) to the
/// `cron` crate's seconds-first syntax with named weekdays.
fn translate(cron: &str) -> Result<String, String> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = fields[..] else {
        return Err("must have five fields".to_owned());
    };
    if cron
        .chars()
        .any(|character| matches!(character, '?' | 'L' | 'W' | '#'))
    {
        return Err("uses syntax that is not POSIX cron".to_owned());
    }
    if day != "*" && weekday != "*" {
        return Err("cannot restrict both day of month and day of week".to_owned());
    }
    let weekday = if weekday == "*" {
        "*".to_owned()
    } else {
        weekdays(weekday)?
    };
    Ok(format!("0 {minute} {hour} {day} {month} {weekday}"))
}

fn weekday_number(value: &str) -> Result<u32, String> {
    if let Some(index) = DAYS.iter().position(|day| day.eq_ignore_ascii_case(value)) {
        return Ok(index as u32);
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|number| *number <= 7)
        .ok_or_else(|| format!("has an invalid day of week `{value}`"))
}

fn weekdays(field: &str) -> Result<String, String> {
    let mut days = BTreeSet::new();
    for item in field.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (
                range,
                step.parse::<u32>()
                    .ok()
                    .filter(|step| *step > 0)
                    .ok_or_else(|| format!("has an invalid step in `{item}`"))?,
            ),
            None => (item, 1),
        };
        let (start, end) = match range {
            "*" => (0, 6),
            _ => match range.split_once('-') {
                Some((start, end)) => (weekday_number(start)?, weekday_number(end)?),
                None if item.contains('/') => (weekday_number(range)?, 6),
                None => {
                    let day = weekday_number(range)?;
                    (day, day)
                }
            },
        };
        if start > end {
            return Err(format!("has a descending day range `{item}`"));
        }
        days.extend((start..=end).step_by(step as usize).map(|day| day % 7));
    }
    Ok(days
        .into_iter()
        .map(|day| DAYS[day as usize])
        .collect::<Vec<_>>()
        .join(","))
}

/// Updates persisted cron entries after workflow discovery at a changed head.
pub async fn sync_schedules(
    database: &DatabaseConnection,
    repository_id: Uuid,
    branch: String,
    commit: String,
    wanted: BTreeSet<(String, String)>,
    scan: Option<action_schedule_scan::Model>,
) -> Result<(), sea_orm::DbErr> {
    let existing = action_schedule::Entity::find()
        .filter(action_schedule::Column::RepositoryId.eq(repository_id))
        .all(database)
        .await?;
    let now = Utc::now();
    for row in &existing {
        if !wanted.contains(&(row.workflow_path.clone(), row.cron.clone())) {
            action_schedule::Entity::delete_by_id((
                repository_id,
                row.workflow_path.clone(),
                row.cron.clone(),
            ))
            .exec(database)
            .await?;
        }
    }
    for (path, cron) in wanted {
        if existing
            .iter()
            .any(|row| row.workflow_path == path && row.cron == cron)
        {
            continue;
        }
        let Ok(next_fire_at) = next_fire(&cron, now) else {
            continue;
        };
        action_schedule::ActiveModel {
            repository_id: Set(repository_id),
            workflow_path: Set(path),
            cron: Set(cron),
            last_fired_at: Set(None),
            next_fire_at: Set(next_fire_at),
            created_at: Set(now),
        }
        .insert(database)
        .await?;
    }
    match scan {
        Some(scan) => {
            let mut active = scan.into_active_model();
            active.default_branch = Set(branch);
            active.head_sha = Set(commit);
            active.scanned_at = Set(now);
            active.update(database).await?;
        }
        None => {
            action_schedule_scan::ActiveModel {
                repository_id: Set(repository_id),
                default_branch: Set(branch),
                head_sha: Set(commit),
                scanned_at: Set(now),
            }
            .insert(database)
            .await?;
        }
    }
    Ok(())
}

/// Removes schedules when a repository has no eligible default branch.
pub async fn clear_schedules(
    database: &DatabaseConnection,
    repository_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    action_schedule::Entity::delete_many()
        .filter(action_schedule::Column::RepositoryId.eq(repository_id))
        .exec(database)
        .await?;
    action_schedule_scan::Entity::delete_by_id(repository_id)
        .exec(database)
        .await?;
    Ok(())
}
/// Lists overdue occurrences in dispatch order, bounded per tick.
pub async fn due_schedules(
    database: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<Vec<action_schedule::Model>, sea_orm::DbErr> {
    let due = action_schedule::Entity::find()
        .filter(action_schedule::Column::NextFireAt.lte(now))
        .order_by_asc(action_schedule::Column::NextFireAt)
        .limit(MAX_DUE_PER_TICK)
        .all(database)
        .await?;
    Ok(due)
}
/// Claims one occurrence using a compare-and-swap on its persisted deadline.
pub async fn claim_due(
    database: &DatabaseConnection,
    schedule: &action_schedule::Model,
    now: DateTime<Utc>,
) -> Result<bool, sea_orm::DbErr> {
    let Ok(next_fire_at) = next_fire(&schedule.cron, now) else {
        action_schedule::Entity::delete_by_id((
            schedule.repository_id,
            schedule.workflow_path.clone(),
            schedule.cron.clone(),
        ))
        .exec(database)
        .await?;
        return Ok(false);
    };
    // Claim this occurrence; a concurrent or repeated tick sees 0 rows.
    let claimed = action_schedule::Entity::update_many()
        .col_expr(
            action_schedule::Column::LastFiredAt,
            Expr::value(Some(schedule.next_fire_at)),
        )
        .col_expr(
            action_schedule::Column::NextFireAt,
            Expr::value(next_fire_at),
        )
        .filter(action_schedule::Column::RepositoryId.eq(schedule.repository_id))
        .filter(action_schedule::Column::WorkflowPath.eq(&schedule.workflow_path))
        .filter(action_schedule::Column::Cron.eq(&schedule.cron))
        .filter(action_schedule::Column::NextFireAt.eq(schedule.next_fire_at))
        .exec(database)
        .await?;
    Ok(claimed.rows_affected == 1)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    #[tokio::test]
    async fn occurrence_claim_is_atomic_and_head_rescan_preserves_deadline() {
        let database = crate::test_support::database().await;
        let owner = crate::test_support::owner(&database, "alice").await;
        let repository = crate::test_support::repository(&database, &owner, "project").await;
        let wanted = BTreeSet::from([("ci.yml".to_owned(), "* * * * *".to_owned())]);
        sync_schedules(
            &database,
            repository.id,
            "main".to_owned(),
            "first".to_owned(),
            wanted.clone(),
            None,
        )
        .await
        .unwrap();
        let row = action_schedule::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        let scan = action_schedule_scan::Entity::find_by_id(repository.id)
            .one(&database)
            .await
            .unwrap();
        sync_schedules(
            &database,
            repository.id,
            "main".to_owned(),
            "second".to_owned(),
            wanted,
            scan,
        )
        .await
        .unwrap();
        let rescanned = action_schedule::Entity::find()
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.next_fire_at, rescanned.next_fire_at);
        let now = row.next_fire_at + chrono::Duration::seconds(1);
        assert!(claim_due(&database, &row, now).await.unwrap());
        assert!(!claim_due(&database, &row, now).await.unwrap());
        assert!(due_schedules(&database, now).await.unwrap().is_empty());
        clear_schedules(&database, repository.id).await.unwrap();
        assert!(
            action_schedule::Entity::find()
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            action_schedule_scan::Entity::find_by_id(repository.id)
                .one(&database)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn posix_weekdays_translate_to_named_days() {
        assert_eq!(
            translate("0 9 * * 1-5").unwrap(),
            "0 0 9 * * MON,TUE,WED,THU,FRI"
        );
        assert_eq!(translate("*/15 * * * *").unwrap(), "0 */15 * * * *");
        assert_eq!(translate("0 0 * * 0").unwrap(), "0 0 0 * * SUN");
        assert_eq!(translate("0 0 * * 7").unwrap(), "0 0 0 * * SUN");
        assert_eq!(translate("0 0 * * 5-7").unwrap(), "0 0 0 * * SUN,FRI,SAT");
        assert_eq!(
            translate("0 0 * * */2").unwrap(),
            "0 0 0 * * SUN,TUE,THU,SAT"
        );
        assert_eq!(translate("0 0 * * sat,sun").unwrap(), "0 0 0 * * SUN,SAT");
        for invalid in [
            "* * * *",
            "0 0 1 * 1",
            "0 0 * * 8",
            "0 0 * * 5-1",
            "0 0 L * *",
            "0 0 * * 1#2",
        ] {
            assert!(translate(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn next_fire_uses_posix_weekday_numbers_in_utc() {
        // 2026-09-27 is a Sunday.
        let after = Utc.with_ymd_and_hms(2026, 9, 27, 12, 0, 0).unwrap();
        assert_eq!(
            next_fire("30 8 * * 1", after).unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 28, 8, 30, 0).unwrap()
        );
        assert_eq!(
            next_fire("0 13 * * 0", after).unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 27, 13, 0, 0).unwrap()
        );
    }

    #[test]
    fn schedule_entries_are_validated_and_deduplicated() {
        let value: Value = yaml_serde::from_str(
            "- cron: '0 0 * * *'\n- cron: '0  0 * * *'\n- cron: '5 * * * *'\n",
        )
        .unwrap();
        assert_eq!(parse_crons(&value).unwrap(), ["0 0 * * *", "5 * * * *"]);
        for invalid in [
            "[]",
            "- '0 0 * * *'\n",
            "- cron: nope\n",
            "- cron: '0 0 * * *'\n  extra: 1\n",
        ] {
            let value: Value = yaml_serde::from_str(invalid).unwrap();
            assert!(parse_crons(&value).is_err(), "{invalid}");
        }
    }
}
