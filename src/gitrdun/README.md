# gitrdun (Rust Port)

A high-performance Rust port of the `gitrdun` tool for finding and summarizing recent Git commits across multiple repositories.

## Overview

This is a complete Rust reimplementation of the original Go-based `gitrdun` tool. It recursively searches for Git repositories and finds recent commits within a specified time range, with optional AI-powered summaries using Ollama.

## Features

- **Fast Repository Scanning**: Multi-threaded directory traversal with skip lists for common non-repo directories
- **Flexible Date Parsing**: Natural language support ("7d", "last monday", "2 weeks", etc.)  
- **Interactive Terminal UI**: Real-time progress display with statistics
- **Git Integration**: Full support for branch filtering, author filtering, and commit analysis
- **Ollama AI Summaries**: Generate intelligent summaries of repository work using local LLMs
- **Cross-platform**: Works on Windows, macOS, and Linux

## Installation

### From Source

```bash
cargo install --git https://github.com/timmattison/tools gitrdun
```

### Local Development

```bash
git clone https://github.com/timmattison/tools
cd tools/src/gitrdun
cargo build --release
```

## Usage

### Basic Examples

```bash
# Find commits from the last 24 hours (default)
gitrdun

# Find commits from the last 7 days
gitrdun --start 7d

# Find commits between specific dates
gitrdun --start monday --end friday

# Search all branches instead of just current
gitrdun --all

# Generate AI summaries with Ollama
gitrdun --ollama --ollama-model llama2:7b

# Search from a specific directory
gitrdun --root /path/to/projects

# Include nested repositories
gitrdun --find-nested
```

### Advanced Usage

```bash
# Generate meta-summary across all repositories
gitrdun --meta-ollama --ollama-model gpt-oss

# Save results to file
gitrdun --output results.txt

# Show only summary without individual commits
gitrdun --summary-only

# Show Git operation statistics
gitrdun --stats

# Search specific paths
gitrdun path1 path2 path3
```

## Command Line Options

- `--start <DURATION>`: How far back to search (default: "24h")
- `--end <TIME>`: End time for search range
- `--all`: Search all branches instead of just current
- `--find-nested`: Include nested Git repositories
- `--ollama`: Generate AI summaries using Ollama
- `--meta-ollama`: Generate cross-repository meta-summary
- `--ollama-model <MODEL>`: Ollama model to use (default: "gpt-oss")
- `--ollama-url <URL>`: Ollama API URL (default: "http://localhost:11434")
- `--output <FILE>`: Write results to file
- `--summary-only`: Show only repository names and commit counts
- `--stats`: Show Git operation performance statistics
- `--ignore-failures`: Suppress inaccessible directory warnings
- `--filter-user`: Only show commits from current Git user (default: true)
- `--keep-thinking`: Keep `<think>` tags in LLM output

## Date Format Support

The `--start` and `--end` options support various formats:

- **Duration**: `24h`, `7d`, `2w`, `30m`
- **Natural Language**: `yesterday`, `last week`, `monday`, `2 days ago`
- **ISO Dates**: `2023-12-31`, `2023-12-31T12:00:00`

## Dependencies

- **Git**: Git must be installed and configured
- **Ollama** (optional): For AI summary generation

## Architecture

### Core Modules

- **CLI**: Command-line interface using `clap`
- **Git**: Repository operations using `git2`
- **UI**: Terminal interface using `ratatui` and `crossterm`
- **Date**: Flexible date parsing with `chrono` and `chrono-english`
- **Ollama**: AI integration using `reqwest` and `tokio`
- **Stats**: Performance monitoring and statistics

### Performance Features

- **Parallel Processing**: Multi-threaded repository scanning
- **Smart Filtering**: Skip common non-repository directories
- **Efficient Git Operations**: Direct libgit2 integration
- **Memory Management**: Streaming and chunked processing for large datasets

## Testing

```bash
# Run all tests
cargo test

# Run with verbose output
cargo test -- --nocapture

# Run specific test module
cargo test cli_tests
```

## Contributing

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass: `cargo test`
5. Submit a pull request

## License

Same as the parent project.

## Comparison with Go Version

### Advantages of Rust Port

- **Performance**: Faster execution due to zero-cost abstractions
- **Memory Safety**: Eliminates memory leaks and data races
- **Better Error Handling**: Comprehensive `Result<T, E>` error propagation
- **Modern Dependencies**: Latest crate ecosystem for Git, HTTP, and UI
- **Type Safety**: Compile-time guarantees for correctness

### Feature Parity

- ✅ All command-line options supported
- ✅ Full Git repository scanning functionality
- ✅ Date parsing with natural language support
- ✅ Ollama AI integration
- ✅ Interactive terminal UI
- ✅ Statistics tracking
- ✅ Cross-platform support

## Performance Notes

The Rust version is optimized for performance:

- Repository scanning is parallelized across CPU cores
- Git operations use efficient libgit2 bindings
- Terminal UI updates are throttled to prevent excessive redraws
- Memory usage is minimized through streaming and early filtering