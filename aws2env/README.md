# aws2env

A command-line tool that converts AWS credentials from `~/.aws/credentials` and `~/.aws/config` files into environment variable export commands.

## Features

- Reads AWS credentials from standard AWS configuration files
- Supports multiple AWS profiles
- Generates export commands for:
  - `AWS_ACCESS_KEY_ID`
  - `AWS_SECRET_ACCESS_KEY`
  - `AWS_SESSION_TOKEN` (if present)
  - `AWS_DEFAULT_REGION` and `AWS_REGION`
- Lists all available profiles

## Installation

```bash
cargo build --release
```

The binary will be available at `target/release/aws2env`.

## Usage

### Export credentials for the default profile

```bash
aws2env
```

### Export credentials for a specific profile

```bash
aws2env -p myprofile
# or
aws2env --profile myprofile
```

### List all available profiles

```bash
aws2env -l
# or
aws2env --list
```

### Apply the exports to your current shell

```bash
eval $(aws2env -p myprofile)
```

## How it Works

The tool reads from:
- `~/.aws/credentials` - Contains AWS access keys and secret keys
- `~/.aws/config` - Contains regions and can also contain credentials

It parses these INI-style files and generates the appropriate export commands that can be evaluated by your shell to set the environment variables.

## Error Handling

The tool will report errors for:
- Missing home directory
- Missing AWS configuration directory
- File read errors
- Profile not found

## License

MIT