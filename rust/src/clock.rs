//! Clock and entropy sources. Production reads the system clock; tests inject
//! fixed values through the same environment variables the Python reference
//! uses. No public debug subcommand exists.

use std::sync::OnceLock;

use time::{OffsetDateTime, PrimitiveDateTime};

use crate::json;

#[derive(Clone, Debug)]
pub struct Clock {
    fake_now: Option<PrimitiveDateTime>,
    fake_uuid: Option<String>,
    fake_token: Option<String>,
}

pub fn clock() -> &'static Clock {
    static CLOCK: OnceLock<Clock> = OnceLock::new();
    CLOCK.get_or_init(|| Clock {
        fake_now: std::env::var("ENGRAMARK_TEST_NOW")
            .ok()
            .and_then(|value| parse_local_datetime(&value)),
        fake_uuid: std::env::var("ENGRAMARK_TEST_UUID")
            .ok()
            .filter(|value| !value.is_empty()),
        fake_token: std::env::var("ENGRAMARK_TEST_TOKEN")
            .ok()
            .filter(|value| !value.is_empty()),
    })
}

fn parse_local_datetime(value: &str) -> Option<PrimitiveDateTime> {
    let format = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    PrimitiveDateTime::parse(value, &format).ok()
}

fn system_local() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

impl Clock {
    fn local(&self) -> OffsetDateTime {
        if let Some(fake) = self.fake_now {
            let offset = system_local().offset();
            return fake.assume_offset(offset);
        }
        system_local()
    }

    /// `date.today().isoformat()`
    pub fn today(&self) -> String {
        let date = self.local().date();
        format!(
            "{:04}-{:02}-{:02}",
            date.year(),
            date.month() as u8,
            date.day()
        )
    }

    /// `datetime.now().isoformat(timespec='seconds')` — local, no offset.
    pub fn isoformat_seconds(&self) -> String {
        let now = self.local();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        )
    }

    /// `time.time()`
    pub fn unix_seconds(&self) -> f64 {
        if let Some(fake) = self.fake_now {
            let offset = system_local().offset();
            return fake.assume_offset(offset).unix_timestamp() as f64;
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as f64 + f64::from(d.subsec_nanos()) / 1e9)
            .unwrap_or(0.0)
    }

    /// `time.time_ns()`
    pub fn unix_nanos(&self) -> i64 {
        if self.fake_now.is_some() {
            return (self.unix_seconds() * 1_000_000_000.0) as i64;
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    }

    pub fn uuid4(&self) -> String {
        if let Some(fake) = &self.fake_uuid {
            return fake.clone();
        }
        uuid::Uuid::new_v4().to_string()
    }

    /// `secrets.token_urlsafe(24)` — 24 random bytes, base64url, no padding.
    pub fn urlsafe_token(&self) -> String {
        if let Some(fake) = &self.fake_token {
            return fake.clone();
        }
        let mut bytes = [0u8; 24];
        if getrandom::fill(&mut bytes).is_err() {
            return self.uuid4().replace('-', "");
        }
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
    }
}

/// Crash-point injection for failure-matrix tests (`ENGRAMARK_CRASH_STAGE`).
pub fn crash_point(stage: &str) {
    if std::env::var("ENGRAMARK_CRASH_STAGE").as_deref() == Ok(stage) {
        std::process::exit(97);
    }
}

pub fn json_time_pair() -> json::Json {
    json::Json::from(clock().isoformat_seconds())
}
