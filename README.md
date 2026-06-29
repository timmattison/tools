# Fun tools written by Tim Mattison

I started this repo forever ago (2014!) to hold some tools I needed at the time. Now I'm converting the tools to ~~Golang~~ Rust
for fun.

> **In a hurry?** See [TLDR.md](./TLDR.md) for a one-line description of every tool.

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

### portplz-core
A shared Rust library that derives a deterministic, unprivileged TCP port from a git repository's root name and
current branch (or, with no git, a directory name). It hides SHA-256 hashing and `gix` repository discovery behind
a single `derive()` entry point. Used by `portplz` (which prints the port) and `sirn` (which serves on it), so both
agree on the same port for a given project without `portplz` needing to be installed.

## The tools

- dirhash
    - Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
      identical. This hash will only be the same if the directories have the same file names and the same file contents.
      However, we ignore the directory names and locations of files in the directories. Respects .gitignore and other
      ignore files by default. See below for an example.
    - To install: `cargo install --git https://github.com/timmattison/tools dirhash`
- prcp
    - Copies files with a beautiful progress bar using Unicode block characters. Supports wildcards, multi-file copy,
      and move mode (`--rm`) that verifies SHA256 before removing source. Press space to pause/resume, Ctrl+C to cancel.
      Run `prcp --shell-setup` to add a `prmv` command for convenient moves.
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
- tc (token count)
    - Counts estimated tokens in files, similar to how `wc` counts words/lines/characters. Useful for estimating
      LLM API costs and checking if content fits within context windows. Supports multiple OpenAI tokenizer models
      (GPT-3.5-turbo, GPT-4, GPT-4o) and can read from stdin or multiple files. Shows counts with
      thousands separators for easy reading.
    - To install: `cargo install --git https://github.com/timmattison/tools tc`
- htmlboard
    - Waits for HTML to be put on the clipboard and then pretty prints it and puts it back in the clipboard.
    - To install: `cargo install --git https://github.com/timmattison/tools htmlboard`
- jsonboard
    - Waits for JSON to be put on the clipboard and then pretty prints it and puts it back in the clipboard.
    - To install: `cargo install --git https://github.com/timmattison/tools jsonboard`
- bm
    - Bulk Move - recursively find and move files matching a pattern to a destination directory. Named "bm" because
      moving lots of files is shitty. Much simpler than `find ... -exec mv`, especially for common tasks like moving
      all files of a certain type. Collision-safe by default and handles moves across volumes (where `rename` fails).
    - To install: `cargo install --git https://github.com/timmattison/tools bm`
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
- sirn
    - Serve It Right Now — a tiny, zero-config HTTP file server. Run `sirn <file>...` to serve each file at
      `/<basename>`, or `sirn` with no arguments to serve the current directory as a browsable tree. The listening
      port is derived automatically from the git repo root and branch (the same algorithm as `portplz`), so a given
      project always serves on a stable port; override it with `-p/--port`. Binds `127.0.0.1` by default — use
      `--bind 0.0.0.0` to expose it on the LAN.
    - To install: `cargo install --git https://github.com/timmattison/tools sirn`
- uuidplz
    - Generates UUIDs. With no input it prints a random v4 UUID. Given a string or a file it seeds a name-based
      v5 (SHA-1) UUID, so the same input always produces the same UUID — handy for stable, reproducible IDs. The
      argument is auto-detected as a file when it names one (override with `--string`/`--file`), and piped stdin
      is hashed too (empty stdin falls back to random). The namespace defaults to the RFC 4122 URL namespace and
      can be overridden with `--namespace <uuid>`. The bare UUID goes to stdout (pipe-friendly); `-v/--verbose`
      explains the derivation on stderr. Examples: `uuidplz`, `uuidplz "my-key"`, `uuidplz config.json`,
      `cat data.bin | uuidplz`, `uuidplz --namespace 6ba7b810-9dad-11d1-80b4-00c04fd430c8 example.com`.
    - To install: `cargo install --git https://github.com/timmattison/tools uuidplz`
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
- spv (smart process viewer)
    - Smart process viewer with enhanced filtering and display. Find processes by PID (single or comma-separated),
      name pattern (case-insensitive substring), or regex. Displays process info in a formatted table or raw output.
      Optionally shows working directories (`--cwd`) and open files (`--lsof`). Examples: `spv 77763`, `spv node`,
      `spv --regex 'node.*'`, `spv --cwd --lsof zsh`.
    - To install: `cargo install --git https://github.com/timmattison/tools spv`
- pk (process killer)
    - Process killer with dry-run mode and detailed feedback. Uses macOS's libproc API (same as Activity Monitor)
      to find processes that `ps` and `pkill` cannot see (like version-named XPC services). Shows what was killed,
      what failed with error messages, and warns if nothing matched. Supports dry-run (`-n`), regex matching (`-r`),
      exact name matching (`-e`), and signal selection (`-s` or `-9` for SIGKILL). Examples: `pk --dry-run 2.1.29`,
      `pk -9 zombie`, `pk --regex '2\.1\.\d+'`.
    - To install: `cargo install --git https://github.com/timmattison/tools pk`
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
- diskhog
    - Shows per-process disk I/O usage on macOS in a continuously updating terminal UI. Displays disk bandwidth
      (read/write bytes per second) for all processes. When run with sudo, also shows IOPS (operations per second)
      using fs_usage. Features include configurable refresh rate, process count limits, and keyboard controls (q/Esc to quit).
    - To install: `cargo install --git https://github.com/timmattison/tools diskhog`
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
- gr8
    - Displays GitHub API rate limit information in a user-friendly format. Fetches rate limits using the GitHub CLI
      (`gh api rate_limit`), converts epoch timestamps to local time in ISO 8601 format, and color-codes the output
      (green for healthy, yellow for under 20% remaining, red for exceeded). Shows limits for all API resource types
      including core, GraphQL, search, code scanning, and more. Requires GitHub CLI to be installed and authenticated.
    - To install: `cargo install --git https://github.com/timmattison/tools gr8`
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
- wolly
    - Wake-on-LAN tool to remotely wake computers by sending magic packets. Features automatic subnet broadcast
      detection, sends multiple packets for reliability (default: 3), supports both WoL ports (7 and 9), and
      includes comprehensive troubleshooting hints. Supports multiple MAC address formats (colon-separated,
      dash-separated, or no separators). Perfect for reliably waking computers on your local network.
    - To install: `cargo install --git https://github.com/timmattison/tools wolly`
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
      repository. Supports `--no-root` flag to start from current directory instead of git root,
      `--hidden` flag to include hidden directories in the search, and `--worktrees` flag to include
      git worktrees in the search.
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
- aa
  - AWS Account - quickly get AWS account information without a pager. Runs the equivalent of
    `aws sts get-caller-identity` but as a simple Rust binary that outputs JSON directly to stdout.
    Perfect for when you need to check which AWS account you're using frequently and don't want to
    type the full AWS CLI command or deal with pager output.
  - To install: `cargo install --git https://github.com/timmattison/tools aa`
- nwt
  - New Worktree - Creates a new git worktree with a randomly generated Docker-style name
    (e.g., "absurd-rock", "zesty-penguin"). Supports config files (~/.nwt.toml), custom branch
    names, checking out existing refs, running commands after creation, and opening worktrees
    in new tmux windows. Worktrees are created in a `{repo-name}-worktrees` directory alongside
    the repository.
  - To install: `cargo install --git https://github.com/timmattison/tools nwt`
- cwt
  - Change Worktree - Navigate between git worktrees in a repository. Shows a list of all
    worktrees with the current one highlighted, or cycle through them with `-f` (forward) and
    `-p` (previous). Can also jump directly to a worktree by directory name or branch name.
    Use `--shell-setup` to automatically add shell integration to your config.
  - To install: `cargo install --git https://github.com/timmattison/tools cwt`
- crap (Claude, Resume Anywhere Please)
  - Resume a Claude Code session from wherever you are. Given a session id, `crap` looks the
    session up under `~/.claude/projects`, recovers the directory it originally ran in, `cd`s
    there, and re-launches Claude with `--resume <id>` — preferring your `clauded` alias if you
    have one, otherwise plain `claude`. If the original directory no longer exists it tells you
    and stops, and it refuses to resume a session that's already open in another running process
    (pass `--force` to override) so two processes can't corrupt the same session log. With
    `--here` it brings the session into the *current* directory instead, resuming it as a forked
    (new-id) session so you can carry its context into a different working tree. `--status <id>`
    reports where a session left off (`waiting-for-user`, `busy`, `awaiting-assistant`, …) without
    resuming; `--status` with no id lists every session for the current directory (as a table, or
    JSON with `--json`) showing each one's state and start/last times. Run `crap --shell-setup`
    once to install the shell function.
  - To install: `cargo install --git https://github.com/timmattison/tools crap`
- ng (navel-gaze)
  - Watches JS/TS source files in the current directory and re-runs `pnpm lint` on change. Pass
    `-t` / `--typecheck` to run `pnpm typecheck` instead. Events are debounced (300ms), and common
    build/dependency directories (`node_modules`, `dist`, `.output`, `.git`, `.next`, `target`,
    `build`, `.turbo`, `.cache`) and `*.test.ts(x)` files are ignored. The screen is cleared and
    pass/fail status is printed in color before each run.
  - To install: `cargo install --git https://github.com/timmattison/tools ng`
- gsw (git status watch)
  - Compact one-shot pretty output of branch state, designed to be wrapped by `viddy` (or `watch`)
    for a continuously refreshing dashboard. Shows branch, ahead/behind, working-tree changes, and
    a `git log --oneline` tail. Respects `COLUMNS` and preserves colors under watch wrappers.
  - To install: `cargo install --git https://github.com/timmattison/tools gsw`
- seescc (sccache stats viewer)
  - Self-refreshing terminal viewer for [sccache](https://github.com/mozilla/sccache) statistics —
    no `viddy`/`watch` wrapper needed. Polls `sccache --show-stats --stats-format=json` on a timer
    (default 1s) and draws a compact, Rust-focused table with Unicode sparklines (`▁▂▃▄▅▆▇` —
    capped below the full block so adjacent rows never visually merge) showing recent activity per
    metric over a configurable history window (default 15m). Counters spark per-bucket deltas, hit
    rate sparks the windowed rate with each bar colored green or red by whether the rate rose or
    fell in that slice, and a mid-run `sccache --zero-stats` never draws a spurious spike. Quit
    with `q`, `Esc`, or Ctrl-C.
    `--one-shot` renders a single frame for scripting (implied when stdout is not a TTY);
    `--one-shot --format json` emits the selected metrics as a JSON object for `jq`. Configure via
    `~/.config/seescc/config.toml` (`--write-default-config` scaffolds an annotated one):
    `poll_interval`/`window` (durations like `500ms`, `1s`, `15m`, `1h`), `languages` (per-language
    metrics filtered to these; `[]` sums all), and `metrics` rows in display order with optional
    `label` and `spark`. Metric keys: per-language `cache_hits`, `cache_misses`, `cache_errors`,
    `hit_rate`; global `compile_requests`, `requests_executed`, `requests_not_cacheable`,
    `requests_not_compile`, `requests_unsupported_compiler`, `cache_writes`, `compilations`,
    `compile_fails`, `forced_recaches`, `cache_size`, `max_cache_size` (an unknown key errors with
    the full catalog). A transient poll failure shows an error banner and keeps the last good
    numbers on screen.
  - To install: `cargo install --git https://github.com/timmattison/tools seescc`
- tsm (terminal session manager)
  - Records every shell command you run via a precmd hook, writing JSONL session logs you can later
    search and replay. `tsm shell-init <shell>` emits the hook snippet to eval; `tsm record` is the
    per-command recorder invoked by the hook.
  - To install: `cargo install --git https://github.com/timmattison/tools tsm`
- beta
  - Terminal session recorder and player — because Betamax was always better than VHS. Captures
    terminal I/O with microsecond timestamps, replays with speed control / pause / rewind, and can
    export recordings to self-contained HTML players or MP4/GIF videos with multiple themes.
  - To install: `cargo install --git https://github.com/timmattison/tools beta`
- vpn-tunnel
  - Generates Docker-based VPN tunnels using gluetun + ProtonVPN + WireGuard. Produces a ready-to-run
    `docker-compose.yml` plus helper scripts; pulls the WireGuard credential from 1Password via
    op-cache. Supports per-city pinning or US-wide IP diversity and configurable container prefixes.
  - To install: `cargo install --git https://github.com/timmattison/tools vpn-tunnel`
- op-cache
  - 1Password credential cache with retry logic, atomic writes, and worktree support. Wraps `op read`
    so repeated calls don't re-hit 1Password (or trigger Touch ID) for every secret. Supports text and
    binary secrets, env-var overrides, cache invalidation, and includes worktree hooks for automatic
    setup. Required by other tools in this repo (e.g. `vpn-tunnel`).
  - To install: `cargo install --git https://github.com/timmattison/tools op-cache`
- kitchen-sync
  - Installs every Rust binary from a git repository with a single command. Clones the repo, parses
    the workspace, finds every member that produces a binary, and runs `cargo install` for each.
    Useful for installing this entire toolbox at once.
  - To install: `cargo install --git https://github.com/timmattison/tools kitchen-sync`
- claude-usage
  - Parses an Anthropic API usage CSV export and computes per-model costs using built-in pricing for
    each Claude model. Useful for reconciling spend or estimating burn across date ranges.
  - To install: `cargo install --git https://github.com/timmattison/tools claude-usage`
- swt (subagent worktree)
  - Subagent worktree helper for parallel TDD. `swt create <name>` verifies HEAD is green and spins
    up an isolated worktree on a new branch; `swt merge <path>` verifies the subagent is green,
    rebases if the parent advanced, fast-forward-merges, and cleans up. Concurrent merges are
    serialized via `.git/swt.lock`. Drop a `./.swt-check` script to override the default green check.
  - To install: symlink `swt/swt.ts` from a clone of this repo into your `PATH`
    (e.g. `ln -s "$PWD/swt/swt.ts" ~/.local/bin/swt`). Requires `npx`/`tsx`.

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

Copy files with a beautiful progress bar: `prcp <source>... <destination>`

**Features:**
- Beautiful progress bar with Unicode block characters (█▉▊▋▌▍▎▏)
- Real-time throughput display with human-readable byte formatting
- Elapsed time, ETA, and completion percentage
- Pause/resume with spacebar
- Ctrl+C to cancel cleanly with proper terminal cleanup
- 16MB buffer size for efficient copying
- Preserves file permissions
- Wildcard/glob support (e.g., `prcp *.txt backup/`)
- Multi-file copy with overall progress tracking
- Move mode with `--rm` flag (verifies SHA256 hash before removing source)
- `--continue-on-error` to keep going if some files fail
- `-y` to skip confirmation prompts

**Shell Integration:**

Run `prcp --shell-setup` to add a `prmv` function to your shell config. This provides a convenient move command:

```bash
prmv file.txt destination/   # Same as: prcp --rm file.txt destination/
```

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

## tc (token count)

Count estimated tokens in files, similar to how `wc` counts words/lines/characters. Useful for estimating LLM API costs and checking if content fits within context windows.

### Basic Usage

```bash
tc file.txt                    # Count tokens in a single file
tc *.md                        # Count tokens in all markdown files
tc file1.txt file2.txt         # Count tokens across multiple files
echo "Hello world" | tc        # Count tokens from stdin
```

### Options

- `--model <MODEL>`: Tokenizer model to use (default: gpt-4)
  - Supported models: `gpt-3.5-turbo`, `gpt-4`, `gpt-4o`
- `--per-file`: Show token count for each file individually (useful with multiple files)
- `-h, --help`: Print help information
- `-V, --version`: Print version information

### Features

- **Multiple tokenizer models**: Support for GPT-3.5-turbo, GPT-4, and GPT-4o tokenizers
- **Stdin support**: Read from pipes or use `-` to read from stdin
- **Human-readable output**: Numbers formatted with thousands separators (e.g., `8,748 tokens`)
- **Per-file breakdown**: Optional detailed output showing token count for each file
- **Fast and efficient**: Built in Rust for performance

### Output Formats

**Single file:**
```bash
$ tc README.md
8,748 tokens  README.md
```

**Multiple files (total only):**
```bash
$ tc file1.txt file2.txt
12,345 tokens  total
```

**Multiple files with per-file breakdown:**
```bash
$ tc --per-file file1.txt file2.txt file3.txt
1,234 tokens  file1.txt
2,345 tokens  file2.txt
3,456 tokens  file3.txt
-------
7,035 tokens  total
```

**From stdin:**
```bash
$ echo "Hello world!" | tc
3 tokens

$ cat large-document.txt | tc --model gpt-3.5-turbo
45,678 tokens
```

### Examples

Count tokens in a single file with default model (GPT-4):
```bash
tc README.md
```

Count tokens using GPT-4o tokenizer:
```bash
tc --model gpt-4o documentation.md
```

Count tokens across multiple files and show breakdown:
```bash
tc --per-file src/*.rs
```

Estimate tokens before sending to an API:
```bash
cat prompt.txt context.txt | tc --model gpt-4o
```

Check if content fits in a context window:
```bash
tokens=$(tc --model gpt-4 large-file.txt | awk '{print $1}' | tr -d ',')
if [ $tokens -lt 8000 ]; then
  echo "Fits in 8K context window"
fi
```

### Use Cases

- **API Cost Estimation**: Calculate approximate costs before sending content to LLM APIs
- **Context Window Validation**: Verify content fits within model context limits
- **Content Planning**: Plan document chunking for RAG systems
- **Token Budgeting**: Track token usage across multiple files in a project
- **Development**: Quick token counts during prompt engineering

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

## wolly

Wake-on-LAN (WoL) tool to remotely wake computers by sending magic packets over the network. Supports various MAC address formats and custom network configurations.

### Basic Usage

```bash
wolly AA:BB:CC:DD:EE:FF
```

This sends 3 magic packets (for reliability) to wake the computer with the specified MAC address. wolly automatically detects your subnet broadcast address and uses it instead of the global broadcast for better results on local networks. Includes helpful troubleshooting hints if the device doesn't wake.

### Options

- `<MAC_ADDRESS>` (required unless using `--list-interfaces`): MAC address of the target computer. Supports multiple formats:
  - Colon-separated: `AA:BB:CC:DD:EE:FF`
  - Dash-separated: `AA-BB-CC-DD-EE-FF`
  - No separators: `AABBCCDDEEFF`
  - Case-insensitive: `aa:bb:cc:dd:ee:ff` or `Aa:Bb:Cc:Dd:Ee:Ff`
- `-p, --port <PORT>`: UDP port to send the magic packet to (default: 9)
- `-b, --broadcast <BROADCAST>`: Broadcast address to send the packet to. Default is `255.255.255.255`, but wolly automatically detects and uses your subnet broadcast address for better reliability
- `-i, --interface <INTERFACE>`: Network interface to use for sending the packet (e.g., en0, eth0). If not specified, uses the first available non-loopback interface
- `-c, --count <COUNT>`: Number of packets to send (default: 3). Sending multiple packets improves reliability
- `-d, --delay <DELAY>`: Delay between packets in milliseconds (default: 100ms)
- `--try-both-ports`: Try sending on both port 7 and port 9 for maximum compatibility (some devices use port 7)
- `--list-interfaces`: List all available network interfaces with their IP addresses and broadcast addresses, then exit
- `-v, --verbose`: Show detailed output including packet details and sending progress

### How it works

Wake-on-LAN works by sending a "magic packet" containing:
- 6 bytes of `0xFF` (255 in decimal)
- 16 repetitions of the target computer's MAC address (6 bytes each)
- Total packet size: 102 bytes

The packet is sent as a UDP broadcast, which allows it to reach computers even when their IP address is unknown.

### Examples

List available network interfaces with broadcast addresses:
```bash
wolly --list-interfaces
# Output: en8 - 192.168.0.118 (broadcast: 192.168.0.255)
```

Basic usage (sends 3 packets to auto-detected subnet broadcast):
```bash
wolly AA:BB:CC:DD:EE:FF
```

Wake a computer using a specific network interface:
```bash
wolly --interface en0 AA:BB:CC:DD:EE:FF
```

Try both standard WoL ports for maximum compatibility:
```bash
wolly --try-both-ports AA:BB:CC:DD:EE:FF
```

Send a single packet with custom port:
```bash
wolly --count 1 --port 7 AA:BB:CC:DD:EE:FF
```

Send 5 packets with 200ms delay between each:
```bash
wolly --count 5 --delay 200 AA:BB:CC:DD:EE:FF
```

Use global broadcast instead of subnet broadcast:
```bash
wolly --broadcast 255.255.255.255 AA:BB:CC:DD:EE:FF
```

Show verbose output with detailed packet information:
```bash
wolly --verbose AA:BB:CC:DD:EE:FF
```

Combine multiple options for maximum reliability:
```bash
wolly -v --try-both-ports --count 5 -i eth0 AA:BB:CC:DD:EE:FF
```

### Prerequisites

The target computer must:
- Have Wake-on-LAN enabled in BIOS/UEFI settings
- Have Wake-on-LAN enabled in the network adapter settings
- Be connected to power (even if turned off)
- Be connected to the network via Ethernet (most WiFi adapters don't support WoL)

### Finding your computer's MAC address

**macOS:**
```bash
ifconfig en0 | grep ether
```

**Linux:**
```bash
ip link show eth0
```

**Windows:**
```bash
ipconfig /all
```

### Troubleshooting

If your device doesn't wake up, try these steps in order:

#### 1. Verify Device Configuration
- **BIOS/UEFI Settings**: Ensure Wake-on-LAN is enabled
  - Look for options like "Wake on LAN", "Power on by PCI-E", or "PME Event Wake Up"
  - On some systems, this is under Power Management settings
- **Network Adapter Settings**:
  - Windows: Device Manager → Network Adapter → Properties → Advanced → Wake on Magic Packet (Enabled)
  - Linux: Check with `ethtool eth0` and look for "Wake-on" (should show 'g' for magic packet)
  - macOS: System Preferences → Energy Saver → Wake for network access

#### 2. Try Different Broadcast Addresses
wolly automatically uses your subnet broadcast (e.g., 192.168.0.255), but some networks require the global broadcast:

```bash
# Try global broadcast
wolly --broadcast 255.255.255.255 B0:4F:13:10:4A:FC

# Or try your specific subnet broadcast
wolly --broadcast 192.168.1.255 B0:4F:13:10:4A:FC
```

#### 3. Try Both Ports
Some devices listen on port 7 instead of the standard port 9:

```bash
wolly --try-both-ports B0:4F:13:10:4A:FC
```

#### 4. Send More Packets
Increase reliability by sending more packets:

```bash
wolly --count 10 B0:4F:13:10:4A:FC
```

#### 5. Check Network Configuration
- **Same Subnet**: Ensure both devices are on the same subnet/VLAN
- **Switch/Router Settings**: Some switches block broadcast packets or have port security
- **Firewall**: Check if firewall rules are blocking UDP broadcasts
- **Network Segmentation**: VLANs or network segmentation may block broadcasts

#### 6. Verify the MAC Address
Double-check you're using the correct MAC address:

```bash
# On the target computer (when it's on)
# macOS:
ifconfig | grep ether

# Linux:
ip link show

# Windows:
ipconfig /all
```

#### 7. Test Different Power States
- Try waking from **sleep/suspend** instead of full shutdown first
- Some motherboards only support WoL from certain power states (S3/S4/S5)
- Check if your device has an LED indicator for WoL (some network cards light up when receiving magic packets)

#### 8. Use Verbose Mode
See exactly what's being sent:

```bash
wolly -v --try-both-ports B0:4F:13:10:4A:FC
```

#### Common Issues

**Issue**: Device wakes from sleep but not from shutdown
- **Solution**: Check BIOS power management settings. Some systems need "Deep Sleep" or "ErP Ready" disabled

**Issue**: WoL works sometimes but not always
- **Solution**: Use `--count 5` to send multiple packets. Network congestion can drop packets

**Issue**: WoL doesn't work across subnets
- **Solution**: You need directed broadcasts or WoL forwarding configured on your router. For cross-subnet WoL, specify the broadcast address of the target subnet

**Issue**: WiFi device won't wake
- **Solution**: Most WiFi adapters don't support WoL. Connect via Ethernet

**Issue**: Device won't wake after long shutdown period
- **Solution**: Some systems lose WoL capability if unplugged. Ensure continuous power supply

## nwt (new worktree)

Creates a new git worktree with a randomly generated Docker-style name (e.g., "absurd-rock", "zesty-penguin").

### Basic Usage

```bash
nwt                           # Create worktree with random name
nwt -b feature-branch         # Create with specific branch name
nwt -c main                   # Check out existing ref
nwt --run "pnpm install"      # Run command after creation
nwt --tmux                    # Open in new tmux window
```

### Options

- `-b, --branch <NAME>`: Create worktree with specific branch name instead of random name
- `--random-directory`: Use a random directory name even when `--branch` is given (by default the branch name doubles as the directory name)
- `-c, --checkout <REF>`: Check out an existing branch/tag/commit instead of creating a new branch
- `--run <COMMAND>`: Run a command in the new worktree after creation
- `--tmux`: Open the new worktree in a new tmux window (Unix only)
- `--no-copy-env`: Skip copying untracked `.env` files from the main worktree into the new one
- `--no-bootstrap-hooks`: Skip the package-manager install that regenerates git hooks (see Hook Bootstrap below)
- `--shell-setup`: Install shell integration for auto-cd into new worktrees (conflicts with all other flags)
- `-q, --quiet`: Suppress non-error messages

### Config File

Create `~/.nwt.toml` to set defaults:

```toml
# Default branch name (optional)
branch = "feature"

# Or default ref to checkout (optional, conflicts with branch)
checkout = "main"

# Default command to run after creation
run = "pnpm install"

# Open in tmux by default
tmux = true

# Suppress output by default
quiet = false

# Copy untracked .env files into new worktrees (default true)
copy_env = true

# Run the package manager's install to regenerate git hooks (default true)
bootstrap_hooks = true
```

### Hook Bootstrap

After creating the worktree, if `package.json` at the worktree root declares a `prepare` script (the husky convention), nwt runs the project's package manager install so git-hook managers regenerate their hooks directory. This matters because `core.hooksPath` often points at a gitignored, generated directory (e.g. `.husky/_`) that a freshly created worktree doesn't have — without the install, git finds no hooks directory and silently runs nothing, so every commit bypasses lint/typecheck/test gates. The package manager is chosen by the `packageManager` field, then a lockfile, then pnpm. Repos without a `prepare` script are unaffected — no install is run.

Disable the install for a single invocation with `--no-bootstrap-hooks`, or set `bootstrap_hooks = false` in `~/.nwt.toml` to disable it by default. When a synchronous `--run` command (without `--tmux`) already invokes a package manager install (e.g. `--run "pnpm install"`), nwt skips its own bootstrap install so dependencies are installed once, not twice. As a safety net, nwt verifies the effective `core.hooksPath` directory actually exists and prints a loud warning if it doesn't — whether bootstrap was skipped, failed, or didn't apply — since that missing directory is the only signal that commits in the new worktree would otherwise be ungated. When you pass a synchronous `--run` command (without `--tmux`), this check runs *after* that command finishes, so a `--run` that installs hooks (e.g. `pnpm install`) can create the directory before the check looks — no false alarm. With `--tmux`, the `--run` command runs asynchronously inside the new window, so the check necessarily runs before tmux is spawned.

### Examples

Create a new worktree and install dependencies:
```bash
nwt --run "pnpm install"
```

Create a worktree from an existing branch:
```bash
nwt -c feature-branch
```

Create worktree and open in tmux:
```bash
nwt --tmux --run "code ."
```

## cwt (change worktree)

Navigate between git worktrees in a repository. Lists all worktrees, cycles through them, or jumps to a specific one by name.

### Basic Usage

```bash
cwt                           # Show list of worktrees with current highlighted
cwt -f                        # Go to next worktree (wraps around)
cwt -p                        # Go to previous worktree (wraps around)
cwt main                      # Go to worktree by branch name
cwt absurd-rock               # Go to worktree by directory name
```

### Options

- `-f, --forward`: Go to the next worktree in the sorted list (wraps around)
- `-p, --prev`: Go to the previous worktree (wraps around)
- `[TARGET]`: Worktree to switch to (directory name or branch name)
- `--shell-setup`: Automatically add shell integration to your ~/.zshrc or ~/.bashrc
- `-q, --quiet`: Suppress error messages

### Shell Integration

The easiest way to set up shell integration is:

```bash
cwt --shell-setup
```

This automatically adds the `wt` function and aliases to your shell config. Run `source ~/.zshrc` (or `~/.bashrc`) to activate, or open a new terminal.

> **Note:** `--shell-setup` currently supports bash and zsh only. Fish users should use the manual setup below.

#### Manual Setup

If you prefer to add it manually, since a program can't change the parent shell's directory, cwt outputs the target path to stdout. Add these shell functions to enable directory changing:

#### Bash / Zsh (~/.bashrc or ~/.zshrc)

```bash
# Change to a git worktree
function wt() {
    if [ $# -eq 0 ]; then
        # No args: show list interactively
        cwt
    else
        local target=$(cwt "$@")
        if [ $? -eq 0 ] && [ -n "$target" ]; then
            cd "$target"
        fi
    fi
}

# Quick navigation aliases
alias wtf='wt -f'  # Next worktree
alias wtb='wt -p'  # Previous worktree (back)
alias wtm='wt main'  # Main worktree
```

#### Fish (~/.config/fish/config.fish)

```fish
function wt
    if test (count $argv) -eq 0
        cwt
    else
        set -l target (cwt $argv)
        if test $status -eq 0 -a -n "$target"
            cd $target
        end
    end
end

# Quick navigation aliases
alias wtf 'wt -f'  # Next worktree
alias wtb 'wt -p'  # Previous worktree (back)
alias wtm 'wt main'  # Main worktree
```

### Examples

Show all worktrees with current highlighted:
```bash
cwt
#   /path/to/repo                    [main]
# > /path/to/repo-worktrees/absurd   [feature-branch]
#   /path/to/repo-worktrees/zen      [fix-bug]
```

Cycle through worktrees:
```bash
wt -f    # Move to next worktree
wt -p    # Move to previous worktree
```

Jump to specific worktree:
```bash
wt main           # By branch name
wt absurd-rock    # By directory name
wtm               # Quick alias for main worktree
```

### Exit Codes

- `0`: Success
- `1`: Not in a git repository
- `2`: Git command error
- `3`: Worktree not found
- `4`: Could not determine current worktree (for -f/-p)
- `5`: Shell setup failed

## crap (Claude, Resume Anywhere Please)

Resume a Claude Code session from any directory. You give `crap` a session id; it finds that session under `~/.claude/projects`, recovers the directory the session originally ran in, changes into it, and re-launches Claude with `--resume <id>` from there.

### Basic Usage

```bash
crap 57570685-2d64-4431-8ab6-c021a12fa1af   # cd into that session's dir and resume it
```

The session id is the name of the `.jsonl` file under `~/.claude/projects/<project>/`. `crap` reads the directory from the session log itself (the sanitized project folder name is lossy), so it always lands in the real original path.

If you have a `clauded` alias or command (e.g. `claude --dangerously-skip-permissions`), `crap` uses it; otherwise it falls back to plain `claude`. If the session's original directory no longer exists, `crap` prints an error and stops without launching anything.

### Resume in the current directory: `--here`

Sometimes you don't want to go back to where a session started — you want to bring its context to where you *are* now (a different worktree, a fresh checkout, a scratch dir):

```bash
crap --here 57570685-2d64-4431-8ab6-c021a12fa1af   # resume it right here
```

Claude resolves `--resume <id>` only against the project folder that matches your current directory, so a plain `claude --resume <id>` from anywhere else fails with *"No conversation found with session ID"*. `crap --here` gets around that: it symlinks the session's transcript into the current directory's project folder so Claude can find it, then resumes it with `--fork-session`. Forking means Claude continues with the **full prior context under a brand-new session id**, so the original transcript is never modified.

The symlink is only needed while Claude reads the transcript at startup. A background watcher removes it the moment the forked session file appears — typically within a second — so it doesn't linger for the whole session, and a final `rm` after the session ends serves as a safety net.

A couple of things to know:

- The replayed history still references the *original* directory's paths. Claude works in your current directory from here on, but the conversation it inherits talks about the old one.
- It still won't resume a session that's open elsewhere unless you pass `--force` (forking reads the live transcript, which can be mid-write).

#### Choosing the forked session's id

By default the fork gets a random new id, which you only learn after Claude starts. Pass a second argument to choose it yourself:

```bash
crap --here 57570685-2d64-4431-8ab6-c021a12fa1af 9f8e7d6c-5b4a-3210-fedc-ba9876543210
```

The new id must be a valid UUID, and `crap` refuses it if it already names a session (so the fork can never overwrite an unrelated transcript). This is handy when a script needs to know the resumed session's id in advance — generate a UUID, hand it to `crap --here`, and you already know where the new transcript will live. Omit it to keep the random-id behavior.

### Don't attach twice

Claude Code records every live CLI session under `~/.claude/sessions/<pid>.json` and removes it on clean exit. Before resuming, `crap` checks that registry: if the session you asked for is already open in another running `claude` process, it refuses and tells you where:

```text
Error: session '4d1637ec-…' is already running (pid 62043, idle)
       in /Volumes/code/muxiavelli
       resuming it again can corrupt the session log.
       re-run with --force to resume anyway.
```

This prevents two processes from appending to the same session log at once. The check verifies the recorded pid is still a live `claude` process (so a stale file left by a crash — or a pid since reused by something else — won't trigger a false alarm). Pass `--force` to resume anyway.

### Check a session's state: `--status`

Before resuming — or when scripting over many sessions — you can ask where a session left off without launching anything:

```bash
crap --status 57570685-2d64-4431-8ab6-c021a12fa1af
```

It prints exactly one of these tokens on stdout:

- `waiting-for-user` — Claude finished its turn and is waiting for your input.
- `busy` — work is in flight: the assistant has a pending tool call, or a tool result was just delivered and the reply hasn't landed yet.
- `awaiting-assistant` — you sent the last message and Claude hasn't replied (an active turn, or a session abandoned mid-reply).
- `empty` — the transcript has no conversational turns yet.

Claude Code never writes an explicit "waiting for input" marker, so `crap` infers the state from the last real turn in the transcript — skipping subagent (`isSidechain`) turns, injected (`isMeta`) entries, and trailing bookkeeping lines, and trusting each turn's `stop_reason` over the per-line content shape.

If the session is **currently open** in a live `claude` process, that process's own status is more authoritative than transcript inference, so it's reported instead:

```text
busy (live, pid 17041)
```

The tokens are stable and newline-terminated, so they script cleanly:

```bash
[ "$(crap --status "$id")" = waiting-for-user ] && echo "ready for you"
```

`--status` exits non-zero for a malformed id or a session that is neither live nor on disk.

#### List every session for the current directory

Give `--status` **no id** and it lists every session recorded for the directory you're in — handy when a single project has several conversations going. Each row shows the state plus when the transcript was *started* and *last written*, read from the transcript's own timestamps (not file mtimes), so they reflect real activity:

```text
2 sessions for /Volumes/code/crap

┌──────────────────────────────────────┬────────────────────────┬─────────────────────┬─────────────────────┐
│ SESSION                              ┆ STATE                  ┆ STARTED             ┆ LAST                │
╞══════════════════════════════════════╪════════════════════════╪═════════════════════╪═════════════════════╡
│ c43eb4df-1ba3-4c42-84f2-ab76319a860c ┆ waiting-for-user       ┆ 2026-05-25 20:02:29 ┆ 2026-05-25 20:11:21 │
│ 1c8aad51-26aa-416d-8da9-a0b586fd0632 ┆ busy (live, pid 98519) ┆ 2026-05-25 18:43:05 ┆ 2026-05-25 20:29:44 │
└──────────────────────────────────────┴────────────────────────┴─────────────────────┴─────────────────────┘
```

Rows are ordered oldest-activity first, so the most recently used session sits at the bottom. Live sessions show their own status and pid; the rest show the inferred state. A session with no recorded activity shows `—` for its times.

#### JSON output: `--json`

Add `--json` (only valid with `--status`) for machine-readable output instead of text — a single object for one id, an array for the directory listing. Keys are camelCase and timestamps are the raw ISO 8601 values, so it pipes straight into `jq`:

```bash
# Which sessions here are waiting on me?
crap --status --json | jq -r '.[] | select(.state == "waiting-for-user") | .sessionId'
```

```json
[
  {
    "sessionId": "c43eb4df-1ba3-4c42-84f2-ab76319a860c",
    "state": "waiting-for-user",
    "started": "2026-05-25T20:02:29.035Z",
    "last": "2026-05-25T20:11:21.375Z"
  }
]
```

`started` and `last` are `null` when the transcript records no timestamps.

### Options

- `[SESSION_ID]`: The Claude session id to resume (optional with `--status`, which then lists every session for the current directory)
- `-f, --force`: Resume even if the session appears to be running in another process
- `--here`: Resume the session in the current directory (as a forked, new-id session) instead of its original one
- `--status`: Print the session's conversational state (`waiting-for-user`, `busy`, `awaiting-assistant`, or `empty`; or `<status> (live, pid <pid>)` when open elsewhere) and exit, without resuming. With no id, lists every session for the current directory with its state and start/last times
- `--json`: With `--status`, emit JSON instead of text (one object for an id, an array for the directory listing)
- `--shell-setup`: Add the `crap` shell function to your ~/.zshrc or ~/.bashrc

### Shell Integration

Because a program can't change its parent shell's working directory — and can't see shell aliases such as `clauded` — `crap` ships as a shell function. Install it once:

```bash
crap --shell-setup
```

Then run `source ~/.zshrc` (or `~/.bashrc`), or open a new terminal. After that, `crap <session-id>` will `cd` into the session's directory and resume it.

> **Note:** `--shell-setup` supports bash and zsh. The bare `crap` binary still works without the function — it just prints the session's directory to stdout instead of changing into it.

#### Manual Setup

If you prefer to add it manually, add this to your `~/.bashrc` or `~/.zshrc`:

```bash
function crap() {
    # --status only queries; it never changes the parent shell. Run it straight
    # through so its output (a token, or a multi-line listing) reaches the
    # terminal instead of being parsed as a "<session-id>\n<dir>" resume target.
    case " $* " in
        *" --status "*) command crap "$@"; return $? ;;
    esac
    local __crap_out
    __crap_out=$(command crap "$@") || return $?
    if [ "${__crap_out%%$'\n'*}" = "__CRAP_HERE__" ]; then
        local __crap_rest __crap_session __crap_link __crap_folder __crap_n0 __crap_watcher
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_link=${__crap_rest#*$'\n'}
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            __crap_folder=$(dirname -- "$__crap_link")
            __crap_n0=$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')
            (
                __crap_i=0
                while [ "$__crap_i" -lt 600 ]; do
                    if [ "$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')" -gt "$__crap_n0" ]; then
                        rm -f -- "$__crap_link"
                        exit 0
                    fi
                    __crap_i=$((__crap_i + 1))
                    sleep 0.1
                done
            ) &
            __crap_watcher=$!
            disown 2>/dev/null
        fi
        if command -v clauded >/dev/null 2>&1; then
            eval 'clauded --resume "$__crap_session" --fork-session'
        else
            claude --resume "$__crap_session" --fork-session
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            kill "$__crap_watcher" 2>/dev/null
            rm -f -- "$__crap_link"
        fi
        return
    fi
    local __crap_session __crap_dir
    __crap_session=${__crap_out%%$'\n'*}
    __crap_dir=${__crap_out#*$'\n'}
    cd -- "$__crap_dir" || return 1
    if command -v clauded >/dev/null 2>&1; then
        eval 'clauded --resume "$__crap_session"'
    else
        claude --resume "$__crap_session"
    fi
}
```

The binary speaks one of two output shapes. By default it prints the session id on the first line and the original directory on the rest; the function takes the first line as the session id and everything after it as the directory (so a path containing a newline stays intact), `cd`s there, and resumes. For `--here` it leads with a `__CRAP_HERE__` marker — having already symlinked the session into the current directory's project folder — so the function stays put and resumes with `--fork-session`. A backgrounded watcher counts the `.jsonl` files in that folder and removes the symlink as soon as a new (forked) one appears, so it doesn't linger for the whole session; a `kill` plus `rm` after Claude exits stops the watcher and serves as a safety net. If the link field is `__CRAP_NO_LINK__`, no symlink was needed and the watcher is skipped. Forwarding `"$@"` lets flags like `--force` and `--here` reach the binary. The `eval` is intentional: shell aliases aren't expanded inside function bodies, so it ensures a `clauded` alias is honored at call time. The `command crap` calls reach the binary past the function of the same name.

### Exit Codes

- `0`: Success
- `1`: No session found with that id
- `2`: Session has no recorded working directory
- `3`: The session's original directory no longer exists
- `4`: Invalid session id
- `5`: Shell setup failed
- `6`: Could not determine your home directory
- `7`: The session is already running in another process (use `--force` to override)
- `8`: `--here`: could not create the project folder or symlink
- `9`: `--here`: could not determine the current working directory

## bm (bulk move)

Recursively find and move files matching a pattern (suffix, prefix, or substring) to a destination directory. Named "bm" because moving lots of files is shitty.

Unlike a bare `mv`/`rename`, bm is collision-safe by default (it refuses to silently overwrite) and transparently handles **moves across volumes** — where `rename(2)` fails with a cross-device error — by copying then deleting, with a progress bar.

### Basic Usage

```bash
bm --suffix .jpg --destination ~/Pictures/photos
bm --prefix IMG_ --destination ~/Pictures/camera ~/Downloads
bm --substring 2024 --destination ~/archive/2024
```

### Options

- `-s`, `--suffix <SUFFIX>`: Match files ending with this string (e.g., `.jpg`, `.mkv`)
- `-p`, `--prefix <PREFIX>`: Match files starting with this string (e.g., `IMG_`, `video_`)
- `--substring <SUBSTRING>`: Match files containing this string anywhere in the name
- `-d`, `--destination <DIR>`: Directory to move matching files to (required; must already exist)
- `--on-collision <POLICY>`: What to do when a destination name already exists or repeats within the batch — `abort` (default), `skip`, `rename`, or `overwrite`
- `--dry-run`: Show what would be moved without moving anything
- `[DIR]...`: Directories to search (defaults to the current directory)

**Note:** Exactly one of `--suffix`, `--prefix`, or `--substring` must be specified.

### Collision handling

If two matched files would land on the same destination name — because the destination already contains that name, or because two source files share a basename — bm does not silently clobber. Choose the behavior with `--on-collision`:

- **`abort`** (default): report every collision and move nothing.
- **`skip`**: move the non-colliding files, leave the colliding ones in place.
- **`rename`**: move everything, disambiguating names (`foo.mkv` → `foo-1.mkv`), preserving extensions.
- **`overwrite`**: move everything, letting later files clobber earlier ones (lossy).

### Cross-volume moves

A plain `rename(2)` cannot move a file between filesystems (e.g. internal disk → an external `/Volumes/...` drive); it fails with a cross-device error. bm detects this and falls back to a chunked copy (with a progress bar) followed by deleting the source, so the same command works whether or not the destination is on the same volume:

```bash
bm --suffix .mkv --destination /Volumes/Backup/videos
```

### Why use bm instead of mv?

```bash
# Moving all .mkv files to a backup drive, with mv + find (verbose, error-prone,
# and find's mv fails file-by-file across volumes):
find . -name "*.mkv" -exec mv {} /Volumes/Backup/videos/ \;

# With bm (simple, collision-safe, cross-volume aware):
bm --suffix .mkv --destination /Volumes/Backup/videos
```

### Examples

Preview moving every file from 2024 without touching anything:
```bash
bm --substring 2024 --destination ~/archive/2024 --dry-run
```

Organize photos, keeping every file even on name clashes:
```bash
bm --prefix IMG_ --on-collision rename --destination ~/Pictures/iphone ~/Downloads
```

Move PDFs from several directories at once:
```bash
bm --suffix .pdf --destination ~/Documents/pdfs ~/Downloads ~/Desktop /tmp
```

### Output

On completion, bm prints a summary:
```
Move complete: 42 moved (40 renamed, 2 copied across volumes), 0 skipped in 1.23s (34 files/sec)
```
