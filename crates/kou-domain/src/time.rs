use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(Option<DateTime<FixedOffset>>);

impl Timestamp {
    pub fn parse(text: &str) -> Result<Self, chrono::ParseError> {
        let time = DateTime::parse_from_rfc3339(text)?;
        if time.timestamp() == -62_135_596_800 && time.timestamp_subsec_nanos() == 0 {
            Ok(Self(None))
        } else {
            Ok(Self(Some(time)))
        }
    }

    pub fn is_zero(&self) -> bool {
        self.0.is_none()
    }

    pub fn unix_millis(&self) -> Option<i64> {
        self.0.map(|time| time.timestamp_millis())
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(time) => {
                write!(f, "{}", time.format("%Y-%m-%dT%H:%M:%S"))?;
                let nanos = time.timestamp_subsec_nanos();
                if nanos != 0 {
                    write!(f, ".{}", format!("{nanos:09}").trim_end_matches('0'))?;
                }
                if time.offset().local_minus_utc() == 0 {
                    f.write_str("Z")
                } else {
                    write!(f, "{}", time.format("%:z"))
                }
            }
            None => f.write_str("0001-01-01T00:00:00Z"),
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        match Option::<String>::deserialize(de)? {
            Some(text) => Self::parse(&text).map_err(de::Error::custom),
            None => Ok(Self::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_offsets_and_go_nanosecond_format() {
        let at = Timestamp::parse("2026-08-27T12:00:00.123456789+02:00").unwrap();
        assert_eq!(at.to_string(), "2026-08-27T12:00:00.123456789+02:00");
        assert_eq!(
            Timestamp::parse("2026-08-27T12:00:00.123400Z")
                .unwrap()
                .to_string(),
            "2026-08-27T12:00:00.1234Z"
        );
        assert!(Timestamp::default().is_zero());
        assert!(Timestamp::parse("0001-01-01T00:00:00Z").unwrap().is_zero());
    }
}
