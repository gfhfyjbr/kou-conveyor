use super::*;

impl Config {
    pub(super) fn validate(&self) -> Result<()> {
        let now = Instant::now();
        for duration in [
            self.tool_heartbeat.saturating_mul(4),
            self.tool_grace,
            self.tool_grace_limit,
            self.tool_wait_limit,
            self.max_duration,
            self.report_every,
        ] {
            if duration.as_nanos() > i64::MAX as u128 || now.checked_add(duration).is_none() {
                return Err(Error(
                    "coordinator duration exceeds the supported clock range".into(),
                ));
            }
        }
        if self.max_verifications > i64::MAX as usize {
            return Err(Error(
                "coordinator verification limit exceeds the supported range".into(),
            ));
        }
        Ok(())
    }
}
pub(super) fn duration_text(duration: Duration) -> String {
    let nanos = duration.subsec_nanos();
    let seconds = duration.as_secs();
    if duration.is_zero() {
        return "0s".into();
    }
    if seconds == 0 && nanos < 1000 {
        return format!("{nanos}ns");
    }
    if seconds == 0 {
        let (scale, digits, suffix) = if nanos < 1_000_000 {
            (1000, 3, "µs")
        } else {
            (1_000_000, 6, "ms")
        };
        let whole = nanos / scale;
        let fraction = nanos % scale;
        return if fraction == 0 {
            format!("{whole}{suffix}")
        } else {
            format!(
                "{whole}.{}{suffix}",
                format!("{fraction:0digits$}").trim_end_matches('0')
            )
        };
    }
    let fraction = if nanos == 0 {
        String::new()
    } else {
        format!(".{nanos:09}").trim_end_matches('0').to_owned()
    };
    if seconds >= 3600 {
        format!(
            "{}h{}m{}{fraction}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m{}{fraction}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}{fraction}s")
    }
}
