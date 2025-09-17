# Fun tools written by Tim Mattison

I started this repo forever ago (2014!) to hold some tools I needed at the time. Now I'm converting the tools to ~~Golang~~ Rust
for fun.

## Shared Libraries

### repowalker
A shared Rust library for walking repository directories with intelligent filtering and gitignore support. Used by `goup`, `polish`, and `nodeup` to provide consistent repository traversal with support for:
- Git repository and worktree detection
- Respecting `.gitignore` files and other ignore patterns
- Skipping `node_modules` directories
- Configurable filtering options

See [src/repowalker/README.md](src/repowalker/README.md) for detailed documentation.

### filewalker
A shared Rust library for walking directories and files with filtering capabilities. Used by `sf` and `cf` to provide consistent file traversal with support for:
- Walking multiple directories with deduplication
- Filtering files by suffix, prefix, or substring
- Formatted output for counts and byte sizes
- Error handling for inaccessible files

### clipboardmon
A shared Rust library for monitoring and transforming clipboard content. Provides a framework for building clipboard monitoring tools with:
- Automatic clipboard polling and change detection
- Transformer trait for implementing content transformations
- Only processes relevant content based on custom rules
- Used as the foundation for clipboard transformation tools like `htmlboard`, `jsonboard`, and `unescapeboard`

## The tools

- dirhash
    - Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
      identical. This hash will only be the same if the directories have the same file names and the same file contents.
      However, we ignore the directory names and locations of files in the directories. Respects .gitignore and other
      ignore files by default. See below for an example.
    - To install: `cargo install --git https://github.com/timmattison/tools dirhash`
- prcp
    - Copies a file and shows the progress in the console with a beautiful progress bar using Unicode block characters.
      Useful for when you're copying large files and you don't want to keep opening a new terminal window to run `du -sh`
      to see how much has been copied. You can press the space bar to pause the copy and press it again to resume.
      Press Ctrl+C to cancel and cleanly exit.
    - To install: `cargo install --git https://github.com/timmattison/tools prcp`
- prgz
    - Similar to `prcp` but instead of copying a file it gzip compresses it. It shows the progress in the console.
    - To install: `go install github.com/timmattison/tools/cmd/prgz@latest`
- update-aws-credentials
    - Takes AWS credentials from your clipboard in the format provided by AWS SSO and writes it to
      your AWS config file. This is useful if you're using AWS SSO and you want to use the AWS CLI locally.
    - To install: `cargo install --git https://github.com/timmattison/tools update-aws-credentials`
- sf (size of files)
    - Shows you the total size of files in the specified directories (and subdirectories) in a human-readable format. 
      Supports optional filtering by suffix (e.g. `--suffix .mkv`), prefix (e.g. `--prefix IMG_`), or substring 
      (e.g. `--substring G_00`). Without filters, it shows the total size of all files. Doesn't assume suffixes have 
      a period in front of them so you need to include that if you want it.
    - To install: `cargo install --git https://github.com/timmattison/tools sf`
- cf (count files)
    - Recursively counts files in the specified directories. Without filters, counts all files. Supports optional 
      filtering by suffix (e.g. `--suffix .mkv`), prefix (e.g. `--prefix IMG_`), or substring (e.g. `--substring G_00`). 
      The same as doing `find . | wc -l` but shorter and faster.
    - To install: `cargo install --git https://github.com/timmattison/tools cf`
- htmlboard
    - Waits for HTML to be put on the clipboard and then pretty prints it and puts it back in the clipboard.
    - To install: `cargo install --git https://github.com/timmattison/tools htmlboard`
- jsonboard
    - Waits for JSON to be put on the clipboard and then pretty prints it and puts it back in the clipboard.
    - To install: `cargo install --git https://github.com/timmattison/tools jsonboard`
- bm
    - Bulk Move. Named "bm" because moving lots of files is shitty.
    - To install: `go install github.com/timmattison/tools/cmd/bm@latest`
- localnext
    - Runs statically compiled NextJS applications locally. You'll need to build your code and get the magic `out`
      directory by adding `output: 'export'` to your `next.config.mjs` file. This was written to work
      with [the templates I was testing at the time](https://github.com/timmattison/material-ui-react-templates)
    - To install: `go install github.com/timmattison/tools/cmd/localnext@latest`
- unescapeboard
    - Waits for text with `\\"` in it to be put on the clipboard and then unescapes one level of it.
    - To install: `cargo install --git https://github.com/timmattison/tools unescapeboard`
- prhash
    - Hashes files with the requested hashing algorithm (MD5, SHA1, SHA256, SHA512, Blake3) and shows the progress
      in the console with a beautiful progress bar using Unicode block characters. Outputs results in shasum-compatible
      format. Good for hashing very large files. You must specify the algorithm with `-a/--algorithm`. Press space
      to pause/resume, Ctrl+C to cancel.
    - To install: `cargo install --git https://github.com/timmattison/tools prhash`
- subito
    - Subscribes to a list of topics on AWS IoT Core and prints out the messages it receives. This is useful for
      debugging and testing. I was going to call it `subiot` but `subito` actually means "immediately" in Italian and
      I thought that was cooler. Just run `subito topic1 topic2 topic3 ...` and you'll see the messages.
    - To install: `go install github.com/timmattison/tools/cmd/subito@latest`
- portplz
    - Generates an unprivileged port number based on the name of the current directory and git branch. Nice for picking a port number
      for a service that needs to live behind a reverse proxy that also needs to be consistent across deployments and
      separate instances/VMs.
    - To install: `cargo install --git https://github.com/timmattison/tools portplz`
- tubeboard
    - Waits for text that looks like a YouTube video URL to be put on the clipboard and then extracts the video ID from
      it.
      I use this for deep linking videos to my Roku TVs through their APIs.
    - To install: `cargo install --git https://github.com/timmattison/tools tubeboard`
- safeboard
    - Monitors clipboard for dangerous Unicode characters that could be used in copy-paste attacks. Detects invisible 
      characters like zero-width spaces, directional overrides, and private use area characters that attackers use to 
      hide malicious code or commands. Options include `--audible` for sound alerts and `--modify` to prepend a warning 
      to dangerous content. Includes a test script to verify functionality.
    - To install: `cargo install --git https://github.com/timmattison/tools safeboard`
- gitrdun
    - Shows your recent git commits across multiple repositories. Useful for finding what you've been working on
      recently
      across different projects.
    - To install: `cargo install --git https://github.com/timmattison/tools gitrdun`
- procinfo
    - Shows detailed information about running processes matching a name. Displays process details, working directory,
      command line, open files, network connections, and optionally environment variables. Useful for debugging and
      investigating running applications.
    - To install: `go install github.com/timmattison/tools/cmd/procinfo@latest`
- hexfind
    - Searches for a hex string in a binary file and displays a hex dump with surrounding bytes. Shows the offset in
      both
      hex and decimal formats. Useful for analyzing binary files and finding specific patterns or signatures.
    - To install: `cargo install --git https://github.com/timmattison/tools hexfind`
- ic
    - A fast terminal image and video display utility, designed as a high-performance alternative to `imgcat`. Supports
      multiple image and video formats, resizing with aspect ratio preservation, and reading from files or stdin. Video support requires ffmpeg.
    - To install: `cargo install --git https://github.com/timmattison/tools ic`
- inscribe
    - Automatically generates clear and consistent git commit messages using Claude AI. Analyzes staged changes and creates
      conventional commit messages. Supports credential storage in system credential managers (Keychain on macOS, Credential
      Manager on Windows, Secret Service on Linux). **Note: Currently only tested on macOS.**
    - Usage: `inscribe` (requires staged changes), `inscribe -a` (stages all changes), `inscribe -d` (dry run),
      `inscribe --store-key` (save API key)
    - To install: `cargo install --git https://github.com/timmattison/tools inscribe`
- idear
    - IDEA Reaper. Cleans up orphaned .idea directories that remain when you delete a project directory before closing 
      JetBrains IDEs (IntelliJ IDEA, PyCharm, WebStorm, PhpStorm, RubyMine, CLion, DataGrip, GoLand, Rider, Android Studio). 
      These IDEs create .idea directories to store project metadata, but they can become orphaned and waste disk space if 
      you remove the project folder while the IDE is still open. This tool finds directories containing only a .idea 
      subdirectory and can safely remove them.
    - Usage examples:
      - `idear` - List directories containing only .idea
      - `idear --delete --dry-run` - Show what would be deleted
      - `idear --delete` - Delete directories after confirmation
      - `idear --delete --force` - Delete without confirmation
    - To install: `cargo install --git https://github.com/timmattison/tools idear`
- wifiqr
    - Generates QR codes for WiFi networks that, when scanned by a mobile device, allow the device to automatically
      connect to the WiFi network without manually entering credentials. Supports custom resolution, adding a logo
      in the center of the QR code, and adjusting the logo size.
    - To install: `cargo install --git https://github.com/timmattison/tools wifiqr`
- wu
    - Cross-platform tool to identify which processes have a file, directory, or device open. "Who's using" a file or
      path. Shows process name, PID, user, and access mode. Supports multiple paths and recursive directory scanning.
      Works on macOS (using lsof), Linux (using /proc), and Windows (using system APIs). Supports JSON output and verbose mode.
    - To install: `cargo install --git https://github.com/timmattison/tools wu`
- symfix
    - Recursively scans directories for broken symlinks and optionally fixes them. Can prepend a string to or remove
      a prefix from broken symlink targets to attempt to fix them. Useful for fixing broken symlinks after moving
      directories or restructuring projects.
    - To install: `go install github.com/timmattison/tools/cmd/symfix@latest`
- dirc
    - A versatile directory path tool that can both:
        - Copy the current working directory to the clipboard
        - Read a directory path from the clipboard and output a command to change to that directory (`paste` mode)
    - Works best with an alias like `dirp='eval $(dirc -paste)'` in your shell configuration.
    - To install: `go install github.com/timmattison/tools/cmd/dirc@latest`
- gitdiggin
    - Recursively searches Git repositories for commits containing a specific string. Can search in commit messages by
      default and optionally in commit contents (diffs). Useful for finding when and where specific changes were made
      across multiple repositories.
    - To install: `cargo install --git https://github.com/timmattison/tools gitdiggin`
- glo
    - Finds and displays large objects in Git repositories. Useful for identifying files that are bloating your
      repository
      and could be candidates for Git LFS or removal.
    - To install: `cargo install --git https://github.com/timmattison/tools glo`
- clipboard-random
    - Generates random data and copies it to the clipboard. Supports two modes: binary data (with hex, base64, or raw 
      output formats) and text with diacritics (Zalgo text). Features include customizable parameters, presets for 
      text generation (mild, scary, insane, zalgo, doom), and a dry run mode to preview without copying.
    - To install: `cargo install --git https://github.com/timmattison/tools clipboard-random`
- freeport
    - Finds a free TCP port on localhost (127.0.0.1) quickly and cross-platform. Supports random or sequential port 
      selection, custom port ranges, and can include privileged ports. Useful for development servers, testing 
      environments, and service configuration.
    - To install: `cargo install --git https://github.com/timmattison/tools freeport`
- wl
    - Shows which process is listening on a given port. Useful for identifying what program is using a specific port
      on your system. Supports verbose output to show detailed socket information.
    - To install: `cargo install --git https://github.com/timmattison/tools wl`
- repotidy
    - Runs `go mod tidy` in all directories containing go.mod files within a git repository. Automatically finds
      the repository root and cleans up Go module dependencies throughout the entire codebase.
    - To install: `cargo install --git https://github.com/timmattison/tools repotidy`
- reposize
    - Calculates and displays the total size of a git repository in human-readable format. Shows the total
      byte count with thousands separators based on your locale.
    - To install: `cargo install --git https://github.com/timmattison/tools reposize`
- goup
    - Updates Go dependencies in a git repository. Automatically finds all go.mod files and updates
      dependencies. Supports `--update` flag to use `go get -u all` for latest versions, otherwise
      uses `go mod tidy` for cleanup.
    - To install: `cargo install --git https://github.com/timmattison/tools goup`
- polish
    - Polishes Rust dependencies in a git repository. Automatically finds all Cargo.toml files and
      updates dependencies. Supports `--latest` flag to use cargo-edit's `cargo upgrade` for latest
      versions (requires cargo-edit installed), otherwise uses standard `cargo update`.
    - To install: `cargo install --git https://github.com/timmattison/tools polish`
- nodenuke
    - Removes node_modules directories and lock files (pnpm-lock.yaml, package-lock.json) throughout a
      repository. Supports `--no-root` flag to start from current directory instead of git root, and
      `--hidden` flag to include hidden directories in the search.
    - To install: `cargo install --git https://github.com/timmattison/tools nodenuke`
- cdknuke
    - Removes cdk.out directories from AWS CDK projects throughout a repository. Uses the same intelligent
      directory scanning as nodenuke. Supports `--no-root` flag to start from current directory instead of
      git root, and `--hidden` flag to include hidden directories in the search.
    - To install: `cargo install --git https://github.com/timmattison/tools cdknuke`
- nodeup
    - Updates npm/pnpm/yarn packages in all directories with package.json. Intelligently detects which
      package manager to use based on lock files. Supports `--latest` flag for major version updates,
      `--npm`/`--pnpm` to force a specific package manager, and `--no-root` to start from current directory.
    - To install: `cargo install --git https://github.com/timmattison/tools nodeup`
- runat
    - TUI tool to run commands at a specified time with a real-time countdown display. Supports various
      time formats including RFC3339, local time, and time-only (runs today or tomorrow). Shows
      current time, target time, and remaining time with styled output. Press Ctrl-C to cancel.
    - To install: `cargo install --git https://github.com/timmattison/tools runat`
- rr
    - Rust remover - runs `cargo clean` in all Rust projects to free disk space. Shows the size of each
      target directory before cleaning. Supports `--dry-run` to preview what would be cleaned and
      `--no-root` to start from current directory. Displays total space freed after completion.
    - To install: `cargo install --git https://github.com/timmattison/tools rr`
- rcc
    - Rust Cross Compiler helper - simplifies Rust cross-compilation by automatically determining target 
      architectures from uname output, managing Cross.toml configuration, and executing cross build commands. 
      Eliminates the complexity of setting up cross-compilation environments by handling target detection, 
      Docker image configuration, and build execution automatically.
    - To install: `cargo install --git https://github.com/timmattison/tools rcc`
- r2-bucket-cleaner
    - Lists and optionally clears all objects from a Cloudflare R2 bucket using the wrangler CLI. Features 
      parallel deletion with 10 concurrent operations, automatic pagination handling with the `--all` flag, 
      and progress tracking. Includes safety confirmation prompts and retry logic for reliability.
    - To install: `cargo install --git https://github.com/timmattison/tools r2-bucket-cleaner`
- org-borg
    - Assimilate GitHub organization repositories - resistance is futile. Clone and manage repositories from 
      GitHub organizations with bulk operations. Features automatic authentication via GitHub CLI (`gh`), 
      concurrent cloning, smart updates for existing repos, and optional archiving. Supports cloning from 
      specific organizations or all accessible organizations at once.
    - To install: `cargo install --git https://github.com/timmattison/tools org-borg`
- aws2env
    - Converts AWS credentials from `~/.aws/credentials` and `~/.aws/config` files into environment variable 
      export commands. Supports multiple profiles, lists available profiles, and generates exports for 
      AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN, and AWS_REGION. Use with `eval $(aws2env)` 
      to apply exports to current shell.
    - To install: `cargo install --git https://github.com/timmattison/tools aws2env`

## dirhash

Calculate a SHA256 hash of a directory tree that's deterministic based on file contents. Respects .gitignore and other ignore files by default.

### Usage

```
dirhash [OPTIONS] <DIRECTORY>
```

### Options

- `--no-ignore`: Don't respect ignore files (.gitignore, .ignore, etc.)
- `--no-ignore-vcs`: Don't respect .gitignore files specifically
- `--hidden`: Include hidden files and directories

### Features

- **Respects ignore files**: Automatically excludes files listed in .gitignore, .ignore, and other standard ignore files
- **Clean output**: Outputs only the final hash to stdout for easy scripting
- **Informative messages**: Shows count of ignored files on stderr with instructions on how to include them
- **Fast**: Uses parallel processing for hashing multiple files

### How it works

If you have two directories with the following contents:

```
dir1/
  file1.txt
  file2.txt
  subdir1/
    file3.txt
    file4.txt
```

```
dir2/
  subdir1/
    file1.txt
    file2.txt
  subdir2/
    file3.txt
    file4.txt
```

As long as the contents of `file1.txt`, `file2.txt`, `file3.txt`, and `file4.txt` are the same in both directories, the
hashes will be the same. The subdirectory names and locations are ignored.

### Examples

Basic usage (respects .gitignore):
```bash
dirhash /path/to/directory
```

Include all files (ignore .gitignore):
```bash
dirhash --no-ignore-vcs /path/to/directory
```

Include hidden files and directories:
```bash
dirhash --hidden /path/to/directory
```

Compare two directories:
```bash
if [ "$(dirhash dir1)" = "$(dirhash dir2)" ]; then
  echo "Directories have identical contents"
fi
```

## prcp

Simply run `prcp <source> <destination>` and you'll see the progress of the copy in the console.

**Features:**
- Beautiful progress bar with Unicode block characters (█▉▊▋▌▍▎▏)
- Real-time throughput display with human-readable byte formatting
- Elapsed time, ETA, and completion percentage
- Pause/resume with spacebar
- Ctrl+C to cancel cleanly with proper terminal cleanup
- 16MB buffer size for efficient copying
- Preserves file permissions

## prhash

Hash files with progress display: `prhash -a sha256 file1.txt file2.txt`

**Features:**
- Supports MD5, SHA1, SHA256, SHA512, and Blake3 algorithms
- Beautiful progress bar with Unicode block characters
- Outputs in shasum-compatible format
- Required algorithm selection (no default)
- Pause/resume with spacebar
- Ctrl+C to cancel cleanly with proper terminal cleanup
- Processes multiple files sequentially
- 16MB buffer size for efficient hashing

## update-aws-credentials

Just run `update-aws-credentials` and it will take the AWS credentials from your clipboard and write them to your AWS config file. If something goes wrong it'll let you know.

## sf (size of files)

Just run `sf --suffix .mkv` and you'll see the size of all of the `.mkv` files in the current directory and all
subdirectories. I use it to figure out how large my videos are in a certain directory before trying to move them around.

## wifiqr

Generate QR codes for WiFi networks that can be scanned by mobile devices to automatically connect to the network.

### Basic Usage

```
wifiqr -ssid MyWiFiNetwork -password MySecretPassword
```

This will generate a QR code image named `MyWiFiNetwork.png` in the current directory.

### Options

- `-ssid` (required): The WiFi network name (SSID)
- `-password` (required): The WiFi network password
- `-resolution` (optional): Resolution of the QR code image in pixels (default: 1024)
- `-logo` (optional): Path to an image file to use as a logo in the center of the QR code
- `-logo-size` (optional): Size of the logo as a percentage of the QR code (1-100, default: 10%)

### Examples

Generate a basic WiFi QR code:

```
wifiqr -ssid MyWiFiNetwork -password MySecretPassword
```

Generate a smaller QR code (512x512 pixels):

```
wifiqr -resolution 512 -ssid MyWiFiNetwork -password MySecretPassword
```

Generate a QR code with a logo in the center:

```
wifiqr -logo company_logo.png -ssid MyWiFiNetwork -password MySecretPassword
```

Generate a QR code with a larger logo (20% of QR code size):

```
wifiqr -logo company_logo.png -logo-size 20 -ssid MyWiFiNetwork -password MySecretPassword
```

When scanned with a smartphone camera, these QR codes will prompt the device to join the specified WiFi network
automatically.

## wu

Cross-platform tool to identify which processes have a file, directory, or device open. Shows process information including PID, name, user, and access mode. When given a directory, it recursively checks all files within that directory tree. Supports checking multiple paths in a single command.

### Basic Usage

```
wu /path/to/file
wu /path/to/directory      # Recursively checks all files in directory
wu /dev/disk0
wu file1.txt file2.txt     # Check multiple files
wu /dir1 /dir2 file.txt    # Mix of directories and files
```

### Options

- `--json` or `-j`: Output results in JSON format for scripting
- `--verbose` or `-v`: Show detailed information for each process

### Examples

Check which processes are using the current directory (recursively):

```
wu .
```

Check multiple paths at once:

```
wu /home/user/documents /var/log/myapp.log
```

Check a specific file with verbose output:

```
wu --verbose /Users/shared/document.txt
```

Get JSON output for scripting:

```
wu --json /tmp /var/tmp
```

### Platform Support

- **macOS**: Uses the `lsof` command with `+D` flag for recursive directory searches
- **Linux**: Directly reads from the `/proc` filesystem for optimal performance, recursively walking directories
- **Windows**: Uses system APIs and the sysinfo crate to enumerate process handles, with directory recursion

### Output Format

Default output shows a table with:
- **PID**: Process ID
- **NAME**: Process name
- **USER**: User running the process
- **ACCESS**: Type of access (read, write, directory, etc.)
- **FILE**: The specific file or directory being accessed

Verbose output groups processes by PID and shows all files each process has open, including file descriptors and detailed access modes.

## symfix

Recursively scans directories for broken symlinks and optionally fixes them by modifying the symlink targets.

### Basic Usage

```
symfix                                # Scan current directory for broken symlinks
symfix -dir /path/to/scan             # Scan a specific directory
symfix -prepend-to-fix ../            # Fix broken symlinks by prepending "../" to targets
symfix -remove-to-fix /old/path/      # Fix broken symlinks by removing "/old/path/" prefix
```

### Options

- `-dir`: Directory to scan for broken symlinks (default: current directory)
- `-prepend-to-fix`: String to prepend to broken symlink targets to attempt fixing them
- `-remove-to-fix`: String to remove from the beginning of broken symlink targets
- `-verbose`: Enable verbose output for debugging
- `-help`: Show help message with usage information

### Examples

Find all broken symlinks in the current directory:

```
symfix
```

Find all broken symlinks in a specific directory:

```
symfix -dir ~/projects/my-website
```

Fix broken symlinks by prepending a string to their targets:

```
symfix -prepend-to-fix ../
```

Fix broken symlinks by removing a prefix from their targets:

```
symfix -remove-to-fix /old/path/prefix/
```

Scan a specific directory and fix symlinks by prepending:

```
symfix -dir ~/projects/my-website -prepend-to-fix ..
```

When fixing symlinks, targets are resolved relative to the symlink's location. The tool will report all broken symlinks
found and indicate which ones were fixed.

## rcc

Rust Cross Compiler helper that eliminates the complexity of cross-compilation by automatically handling target detection, configuration management, and build execution. Perfect for developers who need to build Rust applications for different architectures without memorizing target triples or Docker image names.

### How it makes cross-compilation easier

**Before rcc:**
1. Install cross manually
2. Figure out the correct target triple (e.g., `aarch64-unknown-linux-gnu` vs `aarch64-unknown-linux-musl`)
3. Create Cross.toml with the right Docker image
4. Remember the exact cross build command syntax

**With rcc:**
1. Run `rcc --uname "$(ssh remote-host uname -a)"` 
2. rcc automatically detects the target, creates Cross.toml, and runs the build

### Basic Usage

```
rcc                                          # Use existing Cross.toml
rcc --target aarch64-unknown-linux-gnu      # Specify target directly
rcc --uname "Linux host 5.4.0 aarch64 GNU/Linux"  # Auto-detect from uname
rcc --release                                # Build in release mode
```

### Target Detection from uname

rcc can parse uname output to automatically determine the correct target triple:

```bash
# Get uname from remote host and let rcc figure out the target
ssh pi@raspberrypi.local uname -a
# "Linux raspberrypi 5.10.17-v8+ #1414 SMP PREEMPT Fri Apr 30 13:18:35 BST 2021 aarch64 GNU/Linux"

rcc --uname "Linux raspberrypi 5.10.17-v8+ #1414 SMP PREEMPT Fri Apr 30 13:18:35 BST 2021 aarch64 GNU/Linux"
# Automatically detects: aarch64-unknown-linux-gnu
```

**Supported architectures:**
- `aarch64` → `aarch64-unknown-linux-{gnu|musl}`
- `x86_64` → `x86_64-unknown-linux-{gnu|musl}`  
- `armv7l` → `armv7-unknown-linux-{gnu|musl}eabihf`
- `i686` → `i686-unknown-linux-{gnu|musl}`

**Libc detection:**
- Alpine Linux (contains "alpine") → `musl`
- All others → `gnu`

### Cross.toml Management

rcc automatically creates Cross.toml if it doesn't exist:

```toml
[target.aarch64-unknown-linux-gnu]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-gnu:edge"
```

If Cross.toml exists:
- **Single target**: Uses that target automatically
- **Multiple targets**: Lists available targets and prompts for selection with `--target`

### Examples

Cross-compile for a Raspberry Pi:
```bash
rcc --uname "Linux raspberrypi 5.15.84-v8+ aarch64 GNU/Linux"
```

Cross-compile for Alpine Linux server:
```bash
rcc --uname "Linux alpine 5.15.74-0-lts x86_64 Alpine Linux"
# Auto-detects: x86_64-unknown-linux-musl
```

Build release version for specific target:
```bash
rcc --target aarch64-unknown-linux-gnu --release
```

### Prerequisites

rcc automatically checks for and guides installation of the `cross` tool:

```bash
cargo install cross --git https://github.com/cross-rs/cross
```
