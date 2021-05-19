use chrono::{serde::ts_seconds, DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use std::time::Duration as StdDuration;
#[derive(Serialize, Deserialize, Clone)]
pub struct Job {
    pub label: String,
    #[serde(with = "ts_seconds")]
    pub begin_date: DateTime<Utc>,
    pub timebox: Option<StdDuration>,
    pub last_notifiaction: Option<DateTime<Utc>>,
}

impl Job {
    fn timebox_remaining(&self) -> Option<StdDuration> {
        match self.timebox {
            Some(timebox) => {
                let dur_result = (self.begin_date
                    + Duration::from_std(timebox).expect("Duration out of range.")
                    - Utc::now())
                .to_std();
                match dur_result {
                    Ok(dur) => Some(dur),
                    Err(_) => Some(StdDuration::new(0, 0)),
                }
            }
            None => None,
        }
    }
    pub fn timebox_expired(&self) -> bool {
        self.timebox_remaining() == Some(StdDuration::new(0, 0))
    }
}
