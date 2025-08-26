# Org-Borg

Assimilate GitHub organization repositories. Resistance is futile.

A Rust CLI tool for managing GitHub repositories across organizations with support for bulk cloning, archiving, and organization management.

## Features

- **Show GitHub Account Info**: Display information about the currently authenticated GitHub user
- **List Organizations**: View all organizations you have access to
- **Clone Organization Repos**: Clone all repositories from a specific organization
- **Clone All Repos**: Clone repositories from all accessible organizations and personal repos
- **Archive Support**: Optionally archive repositories after cloning (make them read-only)
- **SSH/HTTPS Support**: Choose between SSH and HTTPS for cloning
- **Progress Indicators**: Visual feedback during clone operations
- **Concurrent Operations**: Clone multiple repositories in parallel for better performance

## Installation

```bash
cargo build --release
```

The binary will be available at `target/release/org-borg`

## Setup

### Authentication

The tool supports multiple authentication methods, tried in this order:

1. **GitHub CLI (recommended)** - If you have `gh` CLI installed and authenticated:
   ```bash
   gh auth login
   ```
   The tool will automatically use your `gh` authentication.

2. **Environment Variable** - Set a personal access token:
   ```bash
   export GITHUB_TOKEN="your_token_here"
   ```

3. **Command Line Flag** - Pass token directly:
   ```bash
   org-borg --token "your_token_here" <command>
   ```

### Creating a Personal Access Token

If you prefer using a personal access token instead of `gh` CLI:
1. Go to https://github.com/settings/tokens
2. Create a token with these scopes:
   - `repo` (full control of private repositories)
   - `read:org` (read organization membership)

## Usage

### Show Current User Information
```bash
org-borg whoami
```

### List All Organizations
```bash
org-borg list-orgs
```

### Clone All Repos from a Specific Organization
```bash
org-borg clone-org <ORG_NAME> [OPTIONS]

Options:
  -o, --output <PATH>   Output directory (default: ./repos)
  -s, --ssh            Use SSH URLs for cloning
  -a, --archive        Archive repositories after cloning
```

Example:
```bash
org-borg clone-org my-org -o ~/github-repos --ssh
```

### Clone All Repos from All Organizations
```bash
org-borg clone-all [OPTIONS]

Options:
  -o, --output <PATH>   Output directory (default: ./repos)
  -s, --ssh            Use SSH URLs for cloning
  -a, --archive        Archive repositories after cloning
```

Example:
```bash
org-borg clone-all -o ~/all-repos --archive
```

## Directory Structure

When cloning, repositories are organized as follows:
```
output_directory/
├── organization1/
│   ├── repo1/
│   ├── repo2/
│   └── ...
├── organization2/
│   ├── repo1/
│   └── ...
└── personal/
    ├── repo1/
    └── ...
```

## SSH Setup

If using SSH for cloning (`--ssh` flag), ensure your SSH agent is running and has your GitHub SSH key loaded:
```bash
ssh-add ~/.ssh/id_rsa
```

## Notes

- The tool will skip repositories that already exist locally and attempt to pull updates instead
- Archived repositories on GitHub will be skipped from the archive operation
- Maximum 5 concurrent clone operations to avoid overwhelming the system
- Rate limiting is handled automatically by the GitHub API client