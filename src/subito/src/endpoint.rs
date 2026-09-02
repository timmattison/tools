//! The endpoint lookup of `subito`.
//!
//! An AWS account holds one AWS IoT Core data endpoint for each region, and the
//! name of that endpoint holds an identifier of the account. A user knows the
//! region and does not know the identifier, so the tool asks AWS for the name
//! with the `DescribeEndpoint` call of the AWS IoT control plane.

/// The endpoint type that names the data endpoint of the account.
///
/// AWS IoT Core gives two data endpoints. `iot:Data-ATS` carries a certificate
/// that Amazon Trust Services signed, and it is the endpoint AWS tells every
/// new connection to use. The older `iot:Data` endpoint carries a certificate
/// of a different root, and AWS gives no new one. The Go client this tool
/// replaces asks for `iot:Data-ATS`, and this tool asks for the same endpoint.
const DATA_ENDPOINT_TYPE: &str = "iot:Data-ATS";

/// The error the AWS SDK gives for a `DescribeEndpoint` call.
///
/// The type is long, and [`EndpointError`] holds it in a box, so this name
/// keeps both the box and the failure path short.
pub type DescribeEndpointSdkError =
    aws_sdk_iot::error::SdkError<aws_sdk_iot::operation::describe_endpoint::DescribeEndpointError>;

/// A failure of the lookup of the AWS IoT data endpoint.
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    /// The call to `DescribeEndpoint` did not complete.
    ///
    /// The tool did not reach AWS, or AWS refused the call. The SDK error
    /// stays as the source of this failure, because it names the error code
    /// and the message AWS sent, and a reader of the chain gets both. The box
    /// keeps the variant small: the SDK error holds the whole HTTP answer.
    #[error("the call to DescribeEndpoint failed")]
    Call(#[source] Box<DescribeEndpointSdkError>),

    /// The call completed, and the answer named no endpoint address.
    ///
    /// The field is optional in the AWS model, so the SDK gives an answer with
    /// no address. An empty host makes a URL that connects to nothing, so this
    /// is a failure and not an empty string.
    #[error("DescribeEndpoint gave an answer that names no endpoint address")]
    NoAddress,
}

/// Asks an AWS IoT client for the data endpoint of the account.
///
/// The client carries the region and the credentials, so this function takes
/// no other parameter. A test gives a client that points at a local HTTP
/// server, which is why the client is a parameter and not a value this
/// function builds.
///
/// # Errors
///
/// Gives [`EndpointError::Call`] when the call does not complete, and
/// [`EndpointError::NoAddress`] when the answer names no endpoint address.
pub async fn describe_data_endpoint_with(
    client: &aws_sdk_iot::Client,
) -> Result<String, EndpointError> {
    let answer = client
        .describe_endpoint()
        .endpoint_type(DATA_ENDPOINT_TYPE)
        .send()
        .await
        .map_err(|error| EndpointError::Call(Box::new(error)))?;

    answer
        .endpoint_address
        .filter(|address| !address.is_empty())
        .ok_or(EndpointError::NoAddress)
}

/// Builds a client from the ambient AWS configuration and asks it.
///
/// This is the entrance the binary takes. It adds nothing to
/// [`describe_data_endpoint_with`] but the client, because a client is the one
/// thing a test must give itself.
///
/// # Errors
///
/// Gives [`EndpointError::Call`] when the call does not complete, and
/// [`EndpointError::NoAddress`] when the answer names no endpoint address.
pub async fn describe_data_endpoint(
    config: &aws_config::SdkConfig,
) -> Result<String, EndpointError> {
    describe_data_endpoint_with(&aws_sdk_iot::Client::new(config)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address the test server names.
    const TEST_ADDRESS: &str = "a1b2c3d4e5f6g7-ats.iot.us-east-1.amazonaws.com";

    /// The region the test client names.
    const TEST_REGION: &str = "us-east-1";

    /// The access key identifier of the fixed credentials of a test.
    ///
    /// This is the identifier of the published AWS Signature Version 4 test
    /// vectors. It names no account.
    const TEST_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

    /// The secret access key of the fixed credentials of a test.
    ///
    /// This is the key of the published AWS Signature Version 4 test vectors.
    /// It opens no account.
    const TEST_SECRET_ACCESS_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    /// The name a test gives to the provider of its fixed credentials.
    const TEST_PROVIDER_NAME: &str = "subito-test";

    /// The path the AWS IoT control plane gives to `DescribeEndpoint`.
    const DESCRIBE_ENDPOINT_PATH: &str = "/endpoint";

    /// The query the SDK sends, as the wire carries it.
    ///
    /// The SDK encodes a query value with the set AWS states, which keeps the
    /// letters, the digits and `-._~` and encodes every other character. The
    /// colon of `iot:Data-ATS` therefore becomes `%3A` and the hyphen stays.
    const DESCRIBE_ENDPOINT_QUERY: &str = "endpointType=iot%3AData-ATS";

    /// The answer of a server that names the endpoint.
    const ANSWER_WITH_ADDRESS: &str =
        r#"{"endpointAddress":"a1b2c3d4e5f6g7-ats.iot.us-east-1.amazonaws.com"}"#;

    /// The answer of a server that names no endpoint address.
    const ANSWER_WITHOUT_ADDRESS: &str = "{}";

    /// The answer of a server that names an empty endpoint address.
    const ANSWER_WITH_EMPTY_ADDRESS: &str = r#"{"endpointAddress":""}"#;

    /// The media type of every answer of the AWS IoT control plane.
    const JSON_MEDIA_TYPE: &str = "application/json";

    /// The status of an answer that refuses the call.
    const FORBIDDEN: usize = 403;

    /// The status of an answer that reports a failure of the service.
    const SERVER_ERROR: usize = 500;

    /// Takes every variable out of the environment that redirects the SDK.
    ///
    /// The service client reads no `AWS_` variable of its own, and two parts
    /// under it do read the environment: the endpoint rules of the SDK read
    /// `SMITHY_CLIENT_SDK_CUSTOM_PARTITION` and load a file from the path it
    /// names, and the HTTP client reads the proxy variables when a caller asks
    /// it to. A test must reach the local server and nothing else, so this
    /// function removes each of them.
    ///
    /// The rule is a shape and not a list. A list of names goes stale, and a
    /// stale list reports a clean environment for the variable it never learned
    /// about.
    ///
    /// [`std::sync::Once`] holds every other caller until the first one
    /// finishes, and every test of this module calls this function before it
    /// builds a client, so no read of the environment inside the SDK overlaps
    /// the removal.
    fn scrub_environment() {
        static SCRUB: std::sync::Once = std::sync::Once::new();

        SCRUB.call_once(|| {
            let doomed: Vec<String> = std::env::vars_os()
                .filter_map(|(name, _)| name.into_string().ok())
                .filter(|name| {
                    let upper = name.to_ascii_uppercase();
                    upper.starts_with("AWS_")
                        || upper.starts_with("SMITHY_")
                        || upper.ends_with("_PROXY")
                })
                .collect();

            for name in doomed {
                std::env::remove_var(name);
            }
        });
    }

    /// Builds the fixed credentials of a test.
    fn test_credentials() -> aws_credential_types::Credentials {
        aws_credential_types::Credentials::new(
            TEST_ACCESS_KEY_ID,
            TEST_SECRET_ACCESS_KEY,
            None,
            None,
            TEST_PROVIDER_NAME,
        )
    }

    /// Builds a client that talks to the HTTP server at `url` and to nothing
    /// else.
    ///
    /// Every input of the client is a value of this file: the endpoint, the
    /// region and the credentials. The client therefore reads no configuration
    /// file and no environment variable, and it reaches no AWS account.
    ///
    /// The retries are off. A test that expects a failure must see the one
    /// answer the server gives, and not three of them a second apart.
    fn client_for(url: &str) -> aws_sdk_iot::Client {
        scrub_environment();

        let configuration = aws_sdk_iot::Config::builder()
            .behavior_version(aws_sdk_iot::config::BehaviorVersion::latest())
            .region(aws_sdk_iot::config::Region::new(TEST_REGION))
            .credentials_provider(test_credentials())
            .endpoint_url(url)
            .retry_config(aws_sdk_iot::config::retry::RetryConfig::disabled())
            .build();

        aws_sdk_iot::Client::from_conf(configuration)
    }

    /// Builds the ambient configuration of a test, pointed at `url`.
    ///
    /// This is the input of [`describe_data_endpoint`], and it holds the same
    /// fixed values as [`client_for`].
    fn config_for(url: &str) -> aws_config::SdkConfig {
        scrub_environment();

        aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_iot::config::Region::new(TEST_REGION))
            .credentials_provider(
                aws_credential_types::provider::SharedCredentialsProvider::new(test_credentials()),
            )
            .endpoint_url(url)
            .retry_config(aws_sdk_iot::config::retry::RetryConfig::disabled())
            .build()
    }

    /// Starts a server that answers one `DescribeEndpoint` call with `status`
    /// and `body`.
    ///
    /// The server binds a port the operating system chooses, so two runs of
    /// this test at one time do not collide.
    async fn server_that_answers(
        status: usize,
        body: &str,
    ) -> (mockito::ServerGuard, mockito::Mock) {
        let mut server = mockito::Server::new_async().await;

        let mock = server
            .mock("GET", DESCRIBE_ENDPOINT_PATH)
            .match_query(mockito::Matcher::Any)
            .with_status(status)
            .with_header("content-type", JSON_MEDIA_TYPE)
            .with_body(body)
            .create_async()
            .await;

        (server, mock)
    }

    #[tokio::test]
    async fn an_answer_that_names_an_address_gives_the_address_back() {
        let (server, mock) = server_that_answers(200, ANSWER_WITH_ADDRESS).await;

        let address = describe_data_endpoint_with(&client_for(&server.url())).await;

        mock.assert_async().await;
        assert_eq!(address.unwrap(), TEST_ADDRESS);
    }

    #[tokio::test]
    async fn the_request_names_the_data_endpoint_type() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", DESCRIBE_ENDPOINT_PATH)
            .match_query(mockito::Matcher::Exact(DESCRIBE_ENDPOINT_QUERY.to_string()))
            .with_status(200)
            .with_header("content-type", JSON_MEDIA_TYPE)
            .with_body(ANSWER_WITH_ADDRESS)
            .create_async()
            .await;

        let address = describe_data_endpoint_with(&client_for(&server.url())).await;

        // The mock matches the path and the whole query string, so this
        // assertion states the request the SDK sent, character for character.
        mock.assert_async().await;
        assert_eq!(address.unwrap(), TEST_ADDRESS);
    }

    #[tokio::test]
    async fn an_answer_that_names_no_address_is_a_failure() {
        let (server, mock) = server_that_answers(200, ANSWER_WITHOUT_ADDRESS).await;

        let address = describe_data_endpoint_with(&client_for(&server.url())).await;

        mock.assert_async().await;
        assert!(
            matches!(address, Err(EndpointError::NoAddress)),
            "an answer with no address is a failure, and never an empty string: {address:?}"
        );
    }

    #[tokio::test]
    async fn an_answer_that_names_an_empty_address_is_a_failure() {
        let (server, mock) = server_that_answers(200, ANSWER_WITH_EMPTY_ADDRESS).await;

        let address = describe_data_endpoint_with(&client_for(&server.url())).await;

        mock.assert_async().await;
        assert!(
            matches!(address, Err(EndpointError::NoAddress)),
            "an empty address names no host: {address:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_call_is_a_call_failure() {
        let (server, mock) = server_that_answers(FORBIDDEN, "{}").await;

        let address = describe_data_endpoint_with(&client_for(&server.url())).await;

        mock.assert_async().await;
        assert!(
            matches!(address, Err(EndpointError::Call(_))),
            "a refused call is a failure of the call: {address:?}"
        );
    }

    #[tokio::test]
    async fn a_failure_of_the_service_is_a_call_failure() {
        let (server, mock) = server_that_answers(SERVER_ERROR, "{}").await;

        let address = describe_data_endpoint_with(&client_for(&server.url())).await;

        mock.assert_async().await;
        assert!(
            matches!(address, Err(EndpointError::Call(_))),
            "a failure of the service is a failure of the call: {address:?}"
        );
    }

    #[tokio::test]
    async fn a_configuration_gives_the_address_back() {
        let (server, mock) = server_that_answers(200, ANSWER_WITH_ADDRESS).await;

        let address = describe_data_endpoint(&config_for(&server.url())).await;

        mock.assert_async().await;
        assert_eq!(address.unwrap(), TEST_ADDRESS);
    }
}
