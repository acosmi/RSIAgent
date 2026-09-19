//! Fixed pure-function target for the offline curriculum profile.

pub const TARGET_ID: &str = "reference_host.clamp_i64.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClampInput {
    pub value: i64,
    pub min: i64,
    pub max: i64,
}

pub fn clamp_i64(input: ClampInput) -> Result<i64, &'static str> {
    if input.min < -1_000_000
        || input.max > 1_000_000
        || input.value < -1_000_000
        || input.value > 1_000_000
        || input.min > input.max
    {
        return Err("outside_registered_domain");
    }
    Ok(input.value.clamp(input.min, input.max))
}
