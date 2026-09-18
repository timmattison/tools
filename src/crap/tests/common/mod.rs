//! Helpers that more than one integration test file of `crap` uses.

/// Whether `field` is a fork id that `crap` generated: a UUID v4 in the
/// lowercase, hyphenated `8-4-4-4-12` form that `claude --session-id` takes.
pub fn is_generated_fork_id(field: &str) -> bool {
    uuid::Uuid::try_parse(field).is_ok_and(|id| {
        id.get_version() == Some(uuid::Version::Random) && id.hyphenated().to_string() == field
    })
}
