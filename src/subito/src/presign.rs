//! The presigner of `subito`.
//!
//! AWS IoT Core accepts an MQTT connection over a WebSocket. The handshake
//! carries no header of its own, so the credentials travel in the query string
//! of the URL. This module builds that URL and signs it with AWS Signature
//! Version 4.

use std::time::SystemTime;

/// A failure of the presigner.
#[derive(Debug, thiserror::Error)]
pub enum PresignError {
    /// The clock gives a moment that no calendar date names.
    #[error("the clock gives a moment that no calendar date names: {0:?}")]
    ClockOutOfRange(SystemTime),
}

/// Builds the signed WebSocket URL of an AWS IoT Core MQTT connection.
///
/// The URL names the host `endpoint`, the path `/mqtt`, and a query string
/// that holds the algorithm, the credential, the date, the signed headers and
/// the signature. When `credentials` carry a session token, the URL takes one
/// more parameter for it. The token stays outside the signature, because AWS
/// documents the handshake that way.
///
/// `now` is a parameter, and not a call to [`SystemTime::now`], so a test can
/// fix the clock and compare the whole URL against a known vector.
///
/// # Errors
///
/// Gives [`PresignError::ClockOutOfRange`] when `now` names a moment that no
/// calendar date can state.
pub fn presign_websocket_url(
    endpoint: &str,
    region: &str,
    credentials: &aws_credential_types::Credentials,
    now: SystemTime,
) -> Result<String, PresignError> {
    let _ = (endpoint, region, credentials, now);
    unimplemented!("presign_websocket_url")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The access key identifier of the published AWS SigV4 test vectors.
    const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

    /// The secret access key of the published AWS SigV4 test vectors.
    const SECRET_ACCESS_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    /// The region of the published AWS SigV4 test vectors.
    const REGION: &str = "us-east-1";

    /// A second region, to prove the region reaches the signature.
    const OTHER_REGION: &str = "eu-west-1";

    /// An AWS IoT data endpoint of the shape `DescribeEndpoint` gives.
    const ENDPOINT: &str = "a1b2c3d4e5f6g7-ats.iot.us-east-1.amazonaws.com";

    /// `2015-08-30T12:36:00Z`, the moment of the published AWS SigV4 vectors,
    /// as seconds after the Unix epoch.
    const VECTOR_SECONDS: u64 = 1_440_938_160;

    /// One day after [`VECTOR_SECONDS`], to prove the clock reaches the
    /// signature.
    const OTHER_SECONDS: u64 = VECTOR_SECONDS + 86_400;

    /// The name a test gives to the provider of its fixed credentials.
    const PROVIDER_NAME: &str = "subito-test";

    /// A session token that holds three characters a query string must encode:
    /// the solidus, the plus sign and the equals sign.
    const SESSION_TOKEN: &str = "FQoGZXIvYXdzE//abc+def=";

    /// [`SESSION_TOKEN`] after the encoding a query value takes.
    const ENCODED_SESSION_TOKEN: &str = "FQoGZXIvYXdzE%2F%2Fabc%2Bdef%3D";

    /// The signature the vector gives.
    ///
    /// `openssl` computed this number. The pipeline is the one AWS states:
    /// four rounds of HMAC-SHA256 make the signing key from the secret, the
    /// short date, the region, the service and the words `aws4_request`; one
    /// more round of HMAC-SHA256 signs the string to sign with that key.
    ///
    /// The same `openssl` pipeline, given the canonical request of the
    /// published AWS `get-vanilla` test, gives
    /// `5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31`,
    /// which is the signature AWS publishes for that test. The pipeline
    /// therefore reproduces a known-good vector, and no code of this crate
    /// takes part in it. A test that computed this number again with its own
    /// HMAC code would compare the implementation against itself and prove
    /// nothing.
    const EXPECTED_SIGNATURE: &str =
        "abaa19ec946afb81af70a8d779058f5587de5b38d677c12d9f47702a8744126e";

    /// The whole URL the vector gives for permanent credentials.
    ///
    /// The URL holds no `X-Amz-Expires` parameter. The working Go client sends
    /// none, so this URL sends none.
    const EXPECTED_URL: &str = "wss://a1b2c3d4e5f6g7-ats.iot.us-east-1.amazonaws.com/mqtt?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fiotdevicegateway%2Faws4_request&X-Amz-Date=20150830T123600Z&X-Amz-SignedHeaders=host&X-Amz-Signature=abaa19ec946afb81af70a8d779058f5587de5b38d677c12d9f47702a8744126e";

    /// The credential scope the vector gives, after the encoding a query value
    /// takes.
    const EXPECTED_CREDENTIAL: &str =
        "AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fiotdevicegateway%2Faws4_request";

    /// The long date the vector gives.
    const EXPECTED_DATE: &str = "20150830T123600Z";

    /// The moment of the published AWS SigV4 test vectors.
    fn vector_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(VECTOR_SECONDS)
    }

    /// Builds the credentials of the vector, with no session token.
    ///
    /// The values are fixed in this file, so no test reads the environment and
    /// no test reaches an AWS account.
    fn permanent_credentials() -> aws_credential_types::Credentials {
        aws_credential_types::Credentials::new(
            ACCESS_KEY_ID,
            SECRET_ACCESS_KEY,
            None,
            None,
            PROVIDER_NAME,
        )
    }

    /// Builds the credentials of the vector, with a session token.
    fn temporary_credentials() -> aws_credential_types::Credentials {
        aws_credential_types::Credentials::new(
            ACCESS_KEY_ID,
            SECRET_ACCESS_KEY,
            Some(SESSION_TOKEN.to_string()),
            None,
            PROVIDER_NAME,
        )
    }

    /// Signs the vector with permanent credentials.
    fn vector_url() -> String {
        presign_websocket_url(ENDPOINT, REGION, &permanent_credentials(), vector_time())
            .expect("the vector names a valid moment")
    }

    /// Gives the query string of a URL.
    fn query(url: &str) -> &str {
        url.split_once('?')
            .expect("a presigned URL holds a query string")
            .1
    }

    /// Gives the names of the query parameters of a URL, in the order they
    /// appear.
    fn query_names(url: &str) -> Vec<&str> {
        query(url)
            .split('&')
            .map(|pair| pair.split_once('=').map_or(pair, |(name, _)| name))
            .collect()
    }

    /// Gives the value of one query parameter of a URL, as the URL holds it.
    fn query_value<'a>(url: &'a str, name: &str) -> Option<&'a str> {
        query(url).split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
    }

    #[test]
    fn permanent_credentials_give_the_url_of_the_vector() {
        assert_eq!(vector_url(), EXPECTED_URL);
    }

    #[test]
    fn the_url_holds_no_expires_parameter() {
        assert!(
            !vector_url().contains("X-Amz-Expires"),
            "the handshake of the working Go client sends no X-Amz-Expires parameter"
        );
    }

    #[test]
    fn the_query_parameters_come_in_the_order_the_handshake_fixes() {
        assert_eq!(
            query_names(&vector_url()),
            [
                "X-Amz-Algorithm",
                "X-Amz-Credential",
                "X-Amz-Date",
                "X-Amz-SignedHeaders",
                "X-Amz-Signature",
            ]
        );
    }

    #[test]
    fn the_algorithm_parameter_names_sigv4_with_sha256() {
        assert_eq!(
            query_value(&vector_url(), "X-Amz-Algorithm"),
            Some("AWS4-HMAC-SHA256")
        );
    }

    #[test]
    fn the_credential_parameter_names_the_scope_of_the_vector() {
        assert_eq!(
            query_value(&vector_url(), "X-Amz-Credential"),
            Some(EXPECTED_CREDENTIAL)
        );
    }

    #[test]
    fn the_date_parameter_names_the_moment_of_the_signature() {
        assert_eq!(
            query_value(&vector_url(), "X-Amz-Date"),
            Some(EXPECTED_DATE)
        );
    }

    #[test]
    fn the_signed_headers_parameter_names_the_host_header_only() {
        assert_eq!(
            query_value(&vector_url(), "X-Amz-SignedHeaders"),
            Some("host")
        );
    }

    #[test]
    fn the_signature_parameter_carries_the_signature_of_the_vector() {
        assert_eq!(
            query_value(&vector_url(), "X-Amz-Signature"),
            Some(EXPECTED_SIGNATURE)
        );
    }

    #[test]
    fn temporary_credentials_give_the_same_signature() {
        let url = presign_websocket_url(ENDPOINT, REGION, &temporary_credentials(), vector_time())
            .expect("the vector names a valid moment");

        assert_eq!(
            query_value(&url, "X-Amz-Signature"),
            Some(EXPECTED_SIGNATURE),
            "the session token stays outside the signature"
        );
    }

    #[test]
    fn temporary_credentials_add_the_encoded_token_at_the_end() {
        let url = presign_websocket_url(ENDPOINT, REGION, &temporary_credentials(), vector_time())
            .expect("the vector names a valid moment");

        assert_eq!(
            url,
            format!("{EXPECTED_URL}&X-Amz-Security-Token={ENCODED_SESSION_TOKEN}")
        );
    }

    #[test]
    fn the_token_parameter_comes_after_the_signature() {
        let url = presign_websocket_url(ENDPOINT, REGION, &temporary_credentials(), vector_time())
            .expect("the vector names a valid moment");

        assert_eq!(
            query_names(&url),
            [
                "X-Amz-Algorithm",
                "X-Amz-Credential",
                "X-Amz-Date",
                "X-Amz-SignedHeaders",
                "X-Amz-Signature",
                "X-Amz-Security-Token",
            ]
        );
    }

    #[test]
    fn another_clock_gives_another_signature() {
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(OTHER_SECONDS);
        let url = presign_websocket_url(ENDPOINT, REGION, &permanent_credentials(), later)
            .expect("the later moment is a valid date");

        assert_eq!(query_value(&url, "X-Amz-Date"), Some("20150831T123600Z"));
        assert_ne!(
            query_value(&url, "X-Amz-Signature"),
            Some(EXPECTED_SIGNATURE),
            "the clock reaches the signature"
        );
    }

    #[test]
    fn another_region_gives_another_signature_and_another_scope() {
        let url = presign_websocket_url(
            ENDPOINT,
            OTHER_REGION,
            &permanent_credentials(),
            vector_time(),
        )
        .expect("the vector names a valid moment");

        assert_eq!(
            query_value(&url, "X-Amz-Credential"),
            Some("AKIDEXAMPLE%2F20150830%2Feu-west-1%2Fiotdevicegateway%2Faws4_request")
        );
        assert_ne!(
            query_value(&url, "X-Amz-Signature"),
            Some(EXPECTED_SIGNATURE),
            "the region reaches the signature"
        );
    }
}
