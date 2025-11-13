use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::provider::ProvideCredentials;
use aws_sdk_iot::Client as IotClient;
use chrono::Utc;
use clap::Parser;
use hmac::{Hmac, Mac};
use percent_encoding::{percent_encode, AsciiSet, CONTROLS};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use rustls::{ClientConfig, RootCertStore};
use sha2::Sha256;
use tracing::{error, info};

type HmacSha256 = Hmac<Sha256>;

/// AWS SigV4 URI encoding set: encode everything except A-Z, a-z, 0-9, -, _, ., ~
const SIGV4_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Subscribe to AWS IoT Core topics via WebSocket
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// AWS IoT topics to subscribe to
    #[arg(required = true)]
    topics: Vec<String>,

    /// AWS region (defaults to environment or config)
    #[arg(short, long)]
    region: Option<String>,

    /// AWS IoT endpoint (if not provided, will be fetched from AWS IoT)
    #[arg(short, long)]
    endpoint: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let config = if let Some(region) = args.region {
        aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(region))
            .load()
            .await
    } else {
        aws_config::load_defaults(BehaviorVersion::latest()).await
    };

    let iot_endpoint = match args.endpoint {
        Some(endpoint) => endpoint,
        None => {
            let iot_client = IotClient::new(&config);
            get_iot_endpoint(&iot_client).await?
        }
    };

    info!("Connecting to AWS IoT endpoint: {}", iot_endpoint);

    let (client, mut eventloop) = create_mqtt_client(&config, &iot_endpoint).await?;

    for topic in &args.topics {
        client
            .subscribe(topic, QoS::AtMostOnce)
            .await
            .context(format!("Failed to subscribe to topic: {}", topic))?;
        info!("Subscribed to topic: {}", topic);
    }

    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                let topic = publish.topic;
                let payload = String::from_utf8_lossy(&publish.payload);
                info!(topic = %topic, payload = %payload, "Received message");
            }
            Ok(_) => {}
            Err(e) => {
                error!(error = %e, "MQTT error");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn get_iot_endpoint(iot_client: &IotClient) -> Result<String> {
    let response = iot_client
        .describe_endpoint()
        .endpoint_type("iot:Data-ATS")
        .send()
        .await
        .context("Failed to describe IoT endpoint")?;

    response
        .endpoint_address()
        .ok_or_else(|| anyhow::anyhow!("No endpoint address returned"))
        .map(|s| s.to_string())
}

async fn create_mqtt_client(
    config: &aws_config::SdkConfig,
    iot_endpoint: &str,
) -> Result<(AsyncClient, EventLoop)> {
    let credentials_provider = config
        .credentials_provider()
        .ok_or_else(|| anyhow::anyhow!("No credentials provider available"))?;

    let credentials = credentials_provider
        .provide_credentials()
        .await
        .context("Failed to get AWS credentials")?;

    let region = config
        .region()
        .ok_or_else(|| anyhow::anyhow!("No region configured"))?
        .as_ref();

    // Generate the presigned URL for AWS IoT WebSocket authentication
    let presigned_url = create_presigned_url(
        iot_endpoint,
        region,
        credentials.access_key_id(),
        credentials.secret_access_key(),
        credentials.session_token(),
    )?;

    let client_id = format!("subito-{}", Utc::now().timestamp_millis());

    info!("Connecting to AWS IoT Core via secure WebSocket with presigned URL");

    // Create MQTT options with the full presigned URL (wss://...) as the host
    // rumqttc accepts the full WebSocket URL as the host parameter
    let mut mqttoptions = MqttOptions::new(client_id, presigned_url, 443);
    mqttoptions.set_keep_alive(std::time::Duration::from_secs(30));

    // Configure TLS for secure WebSocket connection using system root certificates
    let mut root_cert_store = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs()
        .context("Failed to load native certificates")?
    {
        root_cert_store.add(cert).ok();
    }

    let client_config = ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();

    let tls_config = TlsConfiguration::Rustls(std::sync::Arc::new(client_config));

    // Set transport to secure WebSocket (Wss) with TLS configuration
    mqttoptions.set_transport(Transport::Wss(tls_config));

    let (client, eventloop) = AsyncClient::new(mqttoptions, 10);
    Ok((client, eventloop))
}

fn create_presigned_url(
    host: &str,
    region: &str,
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
) -> Result<String> {
    use std::cmp::Ordering;

    let now = Utc::now();
    let date_stamp = now.format("%Y%m%d").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

    let method = "GET";
    let canonical_uri = "/mqtt";
    let canonical_headers = format!("host:{}\n", host);
    let signed_headers = "host";
    let algorithm = "AWS4-HMAC-SHA256";
    let credential_scope = format!("{}/{}/iotdevicegateway/aws4_request", date_stamp, region);

    // Build and sort canonical query parameters per SigV4 (encode values!)
    let mut params: Vec<(String, String)> = vec![
        ("X-Amz-Algorithm".into(), algorithm.into()),
        (
            "X-Amz-Credential".into(),
            percent_encode(format!("{}/{}", access_key, credential_scope).as_bytes(), SIGV4_ENCODE_SET)
                .to_string(),
        ),
        ("X-Amz-Date".into(), amz_date.clone()),
        // Common practice for IoT Core is long-lived presigns (e.g., 86400s). Adjust if needed.
        ("X-Amz-Expires".into(), "86400".into()),
        ("X-Amz-SignedHeaders".into(), signed_headers.into()),
    ];
    if let Some(token) = session_token {
        params.push((
            "X-Amz-Security-Token".into(),
            percent_encode(token.as_bytes(), SIGV4_ENCODE_SET).to_string(),
        ));
    }
    params.sort_by(|a, b| match a.0.cmp(&b.0) {
        Ordering::Equal => a.1.cmp(&b.1),
        other => other,
    });
    let canonical_querystring = params
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("&");

    // GET has empty payload; use SHA256("") hash in canonical request
    let empty_sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        &canonical_querystring,
        canonical_headers,
        signed_headers,
        empty_sha256
    );

    let string_to_sign = format!(
        "{}\n{}\n{}\n{}",
        algorithm,
        amz_date,
        credential_scope,
        hex::encode(sha256_hash(canonical_request.as_bytes()))
    );
    let signing_key = get_signature_key(secret_key, &date_stamp, region, "iotdevicegateway");
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    Ok(format!(
        "wss://{}{}?{}&X-Amz-Signature={}",
        host, canonical_uri, canonical_querystring, signature
    ))
}

fn sha256_hash(data: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn get_signature_key(key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{}", key);
    let k_date = hmac_sha256(k_secret.as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}