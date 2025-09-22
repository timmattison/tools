# Subito - AWS IoT Core MQTT Subscriber (Rust Version)

Subscribe to AWS IoT Core topics via WebSocket in Rust.

## Usage

```bash
# Subscribe to a single topic
./target/release/subito "my/topic"

# Subscribe to multiple topics
./target/release/subito "topic1" "topic2" "topic3"

# Specify a custom region
./target/release/subito --region us-west-2 "my/topic"

# Specify a custom IoT endpoint
./target/release/subito --endpoint "xxxxx-ats.iot.us-east-1.amazonaws.com" "my/topic"
```

## Building

```bash
cargo build --release
```

The binary will be available at `target/release/subito`.

## Features

- Connects to AWS IoT Core using WebSocket with presigned URLs
- Automatically discovers IoT endpoint if not provided
- Uses AWS credentials from standard credential chain
- Supports subscribing to multiple topics
- Real-time message display with topic and payload

## Requirements

- AWS credentials configured (via environment variables, AWS CLI, or IAM role)
- Appropriate IAM permissions for IoT operations