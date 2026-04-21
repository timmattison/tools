#[allow(
    dead_code,
    reason = "scaffolding for the new S3-backed R2 client; fully wired in during the green step"
)]
pub fn endpoint_url(_account_id: &str) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_url_builds_r2_host_for_account_id() {
        assert_eq!(
            endpoint_url("abc123def456"),
            "https://abc123def456.r2.cloudflarestorage.com"
        );
    }

    #[test]
    fn endpoint_url_interpolates_different_account_ids() {
        assert_eq!(endpoint_url("acme"), "https://acme.r2.cloudflarestorage.com");
    }
}
