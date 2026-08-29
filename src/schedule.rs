use std::{fmt, str::FromStr as _};

use chrono::{DateTime, Utc};
use cron::Schedule;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronScheduleError {
    FieldCount,
    Invalid,
    NoFutureOccurrence,
}

impl fmt::Display for CronScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldCount => formatter.write_str("must use five, six, or seven fields"),
            Self::Invalid => formatter.write_str("is not a valid cron expression"),
            Self::NoFutureOccurrence => formatter.write_str("has no future occurrence"),
        }
    }
}

impl std::error::Error for CronScheduleError {}

pub fn next_occurrence(
    expression: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, CronScheduleError> {
    let fields = expression.split_whitespace().count();
    let normalized = match fields {
        5 => format!("0 {expression}"),
        6 | 7 => expression.to_owned(),
        _ => return Err(CronScheduleError::FieldCount),
    };
    Schedule::from_str(&normalized)
        .map_err(|_| CronScheduleError::Invalid)?
        .after(&after)
        .next()
        .ok_or(CronScheduleError::NoFutureOccurrence)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};

    use super::*;

    #[test]
    fn next_occurrence_accepts_standard_five_field_cron() {
        let after = Utc.with_ymd_and_hms(2026, 8, 26, 10, 14, 0).unwrap();

        let next = next_occurrence("*/15 * * * *", after).unwrap();

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 8, 26, 10, 15, 0).unwrap());
    }

    #[test]
    fn next_occurrence_rejects_nonstandard_field_counts() {
        let error = next_occurrence("0 * * *", Utc::now()).unwrap_err();

        assert_eq!(error, CronScheduleError::FieldCount);
    }
}
