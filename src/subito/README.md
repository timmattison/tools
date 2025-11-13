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
- Required IAM permissions for IoT operations:
  - `iot:Connect` - Connect to AWS IoT Core
  - `iot:Subscribe` - Subscribe to topics
  - `iot:Receive` - Receive messages from subscribed topics
  - `iot:DescribeEndpoint` - Auto-discover the IoT endpoint (required if `--endpoint` is not specified)

### Example IAM Policy

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "iot:Connect",
        "iot:Subscribe",
        "iot:Receive"
      ],
      "Resource": [
        "arn:aws:iot:REGION:ACCOUNT_ID:client/${iot:Connection.Thing.ThingName}",
        "arn:aws:iot:REGION:ACCOUNT_ID:topicfilter/*"
      ]
    },
    {
      "Effect": "Allow",
      "Action": "iot:DescribeEndpoint",
      "Resource": "*"
    }
  ]
}
```

Replace `REGION` and `ACCOUNT_ID` with your AWS region and account ID. Adjust the topic filter resources to match your specific topic patterns.