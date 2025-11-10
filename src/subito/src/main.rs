use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::provider::ProvideCredentials;
use aws_sdk_iot::Client as IotClient;
use chrono::Utc;
use clap::Parser;
use hmac::{Hmac, Mac};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS, Transport};
use sha2::Sha256;
use tracing::{error, info};

type HmacSha256 = Hmac<Sha256>;

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

    if args.topics.is_empty() {
        error!("You must provide at least one AWS IoT topic to subscribe to");
        std::process::exit(1);
    }

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
                info!("\nTopic: {}\nMessage: {}", topic, payload);
            }
            Ok(_) => {}
            Err(e) => {
                error!("MQTT error: {}", e);
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

    info!("Connecting to AWS IoT Core via WebSocket with presigned URL");
    
    // rumqttc expects the WebSocket URL to be passed as the broker address when using Transport::Ws
    // The presigned URL already contains wss://, path, and query parameters with authentication
    // We pass the full URL minus the "wss://" prefix as rumqttc adds the scheme based on Transport
    let broker_url = presigned_url.trim_start_matches("wss://");
    
    // Create MQTT options with the WebSocket URL (without wss:// prefix)
    let mut mqttoptions = MqttOptions::new(client_id, broker_url, 443);
    mqttoptions.set_keep_alive(std::time::Duration::from_secs(30));
    
    // Set transport to WebSocket - rumqttc will prepend wss:// to the broker URL
    mqttoptions.set_transport(Transport::Ws);

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
    let now = Utc::now();
    let date_stamp = now.format("%Y%m%d").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();

    let method = "GET";
    let canonical_uri = "/mqtt";
    let canonical_headers = format!("host:{}\n", host);
    let signed_headers = "host";
    
    let algorithm = "AWS4-HMAC-SHA256";
    let credential_scope = format!("{}/{}/iotdevicegateway/aws4_request", date_stamp, region);

    let mut canonical_querystring = format!(
        "X-Amz-Algorithm={}&X-Amz-Credential={}/{}&X-Amz-Date={}&X-Amz-SignedHeaders={}",
        algorithm,
        access_key,
        credential_scope,
        amz_date,
        signed_headers
    );

    if let Some(token) = session_token {
        canonical_querystring.push_str(&format!("&X-Amz-Security-Token={}", urlencoding::encode(token)));
    }

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        canonical_querystring,
        canonical_headers,
        signed_headers,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
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

    let presigned_url = format!(
        "wss://{}{}?{}&X-Amz-Signature={}",
        host, canonical_uri, canonical_querystring, signature
    );

    Ok(presigned_url)
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

mod urlencoding {
    pub fn encode(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }
}