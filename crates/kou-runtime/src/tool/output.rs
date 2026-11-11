//! The output bound a call may ask for (Go `harness/tool/output.go`).
use super::*;
use crate::operation::shell::MAX_OUTPUT_LENGTH;
use serde_json::value::RawValue;

/// Reads `max_output_length`, which models write as 100 or "100" alike. A call
/// that does not give it gets the default.
pub fn parse_max_output_length(encoded: Option<&RawValue>) -> Result<usize> {
    let Some(encoded) = encoded else {
        return Ok(DEFAULT_MAX_OUTPUT_LENGTH);
    };
    let number = number::number(encoded)
        .map_err(|error| Error(format!("decode max_output_length: {error}")))?;
    // A whole number is one float64 holds exactly: at most 2^53 in magnitude.
    let Some(value) = number.filter(|n| *n > 0.0 && *n <= (1u64 << 53) as f64 && n.fract() == 0.0)
    else {
        return Err(Error("max_output_length must be a positive integer".into()));
    };
    if value > MAX_OUTPUT_LENGTH as f64 {
        return Err(Error(format!(
            "max_output_length must not exceed {MAX_OUTPUT_LENGTH}"
        )));
    }
    Ok(value as usize)
}

#[cfg(test)]
mod tests;
