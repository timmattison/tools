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
A shared Rust library that derives a deterministic, unprivileged TCP port from a git repository's root name,
current branch, and the current user (or, with no git, a directory name plus the user). Mixing in the user means
two people on the same machine get different ports for the same repo and branch, so they can run the same project
side by side without colliding. It hides SHA-256 hashing, `gix` repository discovery, and user detection behind a
single `derive()` entry point. Used by `portplz` (which prints the port) and `sirn` (which serves on it), so both
agree on the same port for a given project and user without `portplz` needing to be installed. Set `PORTPLZ_UID`
to a fixed integer to override the detected user (handy for reproducing a teammate's port or pinning one in
containers/CI).

### gitscratch
A shared Rust library that owns the hardened "dry-run a git operation without touching anything real" harness.
Answering "would this rebase conflict, and how badly?" means actually performing it against the developer's own
repository, which is only safe because of a set of pinned settings — `rebase.updateRefs=false` so the replay
doesn't rewrite the very branches being simulated, `rerere.enabled=false` so a simulated resolution never poisons
the shared `rr-cache`, hooks redirected at an empty directory, `gc.auto=0`, `commit.gpgsign=false`, and an editor
environment a halted rebase can't hang on. A scratch worktree can only be built through `Scratch`, and a `Scratch`
only hands out a git runner that already carries that configuration, so no tool can drift onto a weaker version of
it. Teardown removes the scratch worktree by path and deliberately never runs the repo-wide `git worktree prune`,
which would delete the administrative state of any worktree whose directory is merely missing right now. Used by
`grist`.

See [src/gitscratch/README.md](src/gitscratch/README.md) for the full list of guarantees.

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
    - Generates an unprivileged port number based on the name of the current directory, the git branch, and the current
      user. Mixing in the user lets two people run the same branch of the same repo at the same time without colliding.
      Because the user is mixed in, the derived port is *not* the same across machines by default — different
      deployments, instances, or VMs run the service under different uids and so land on different ports. To get a port
      that stays consistent across deployments and separate instances/VMs — say, for a service living behind a reverse
      proxy — set `PORTPLZ_UID` to the same fixed integer on each, which overrides the detected user and pins the port.
    - To install: `cargo install --git https://github.com/timmattison/tools portplz`
- sirn
    - Serve It Right Now — a tiny, zero-config HTTP file server. Run `sirn <file>...` to serve each file at
      `/<basename>`, or `sirn` with no arguments to serve the current directory as a browsable tree. The listening
      port is derived automatically from the git repo root, branch, and current user (the same algorithm as `portplz`),
      so a given project always serves on a stable port and two users on one machine don't collide; override it with
      `-p/--port`. Binds `127.0.0.1` by default — use `--bind 0.0.0.0` to expose it on the LAN.
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
  - Change Worktree - Navigate between the git worktrees of a repository and of the
    repositories inside it. Shows a list of all worktrees with the current one highlighted,
    or cycle through them with `-f` (forward) and `-p` (previous). Can also jump directly to
    a worktree by directory name or branch name. Use `--no-family` to stay inside one
    repository, and `--shell-setup` to automatically add shell integration to your config.
  - To install: `cargo install --git https://github.com/timmattison/tools cwt`
- gitnuke
  - Removes a git worktree and deletes the branch it had checked out, resolved as one
    operation so the branch is never deleted while the worktree is left standing. Takes a
    path, a directory name, or a branch name (exact matches only). A worktree with
    submodules checked out - which plain `git worktree remove` refuses outright - or with
    uncommitted changes needs `--force`. `--dry-run` reports the plan as a preflight, and
    `--safe` keeps the branch unless it is fully merged.
  - To install: `cargo install --git https://github.com/timmattison/tools gitnuke`
- crap (Claude, Resume Anywhere Please)
  - Resume a Claude Code session from wherever you are. Given a session id, `crap` looks the
    session up under `~/.claude/projects`, recovers the directory it originally ran in, `cd`s
    there, and re-launches Claude with `--resume <id>` — preferring your `clauded` alias if you
    have one, otherwise plain `claude`. If the original directory no longer exists — or exists but
    can't be entered from your account — it tells you and stops, pointing you at `crap --here <id>`
    to fork it where you stand instead; and it refuses to resume a session that's already open in
    another running process (pass `--force` to override) so two processes can't corrupt the same
    session log. With `--here` it brings the session into the *current* directory instead, resuming
    it as a forked (new-id) session so you can carry its context into a different working tree. If
    the id belongs to another account on the machine, `crap` finds it automatically — searching your
    own sessions first, then other users' as a self-first fallback — and resumes a private fork of
    it (or target a specific account with `--user <name>`, which fails up front and lists the real
    accounts if you name one that never ran Claude). A project directory it isn't allowed to read is
    skipped rather than fatal, and if the session was hiding in one the miss names the account and
    prints the commands to copy the transcript over — `crap` never runs `sudo` itself.
    `--status <id>`
    reports where a session left off (`waiting-for-user`, `busy`, `awaiting-assistant`, …) without
    resuming — and finds a session under another account the same way a resume does, honoring
    `--user <name>` or falling back self-first, but only ever *reading* it (no copy, no fork); a
    cross-user miss that stepped over an owner-only directory prints the same copy-it-first
    guidance. `--status` with no id lists every session for the current directory (as a table, or
    JSON with `--json`) showing each one's state and start/last times; that per-directory listing is
    inherently your own, so it stays current-user-only. Run `crap --shell-setup` once to install the
    shell function.
  - To install: `cargo install --git https://github.com/timmattison/tools crap`
- ng (navel-gaze)
  - Watches JS/TS source files in the current directory and re-runs `pnpm lint` on change. Pass
    `-t` / `--typecheck` to run `pnpm typecheck` instead. Events are debounced (300ms), and common
    build/dependency directories (`node_modules`, `dist`, `.output`, `.git`, `.next`, `target`,
    `build`, `.turbo`, `.cache`) and `*.test.ts(x)` files are ignored. The screen is cleared and
    pass/fail status is printed in color before each run.
  - To install: `cargo install --git https://github.com/timmattison/tools ng`
- gsw (git status watch)
  - Compact pretty output of branch state: a self-refreshing live watch on a TTY, and a single
    render when its output is piped or `--one-shot` is given — so it needs no `viddy`/`watch`
    wrapper, but still works under one. Shows branch, ahead/behind, working-tree changes, and
    a `git log --oneline` tail. Ages use two units and get coarser as they grow — `5m23s`,
    `2h14m`, `3d12h`, `5y6mo` — so a repo untouched for years stays readable. Every age sits on
    the row of the thing it ages, a file or a commit, and each is shown exactly once: the newest
    commit's age is on the first log row, not also in the header (so `--no-log` takes the commit
    age off the frame too). An age gsw cannot compute — a commit timestamp ahead of the local
    clock, say — renders as `?` rather than `0s`. Respects
    `COLUMNS` and preserves colors under watch wrappers. Nothing it prints ever wraps: the age
    column is fixed-width by contract, and the header shrinks to fit by dropping the tracking
    ref's name and shortening the branch from the middle.
  - A merge or rebase in progress adds one row between the header and the separator: `⚠ merge`,
    or `⚠ rebase 1/2` where `1/2` is git's own step progress. Either label picks up
    `· 1 conflict to resolve` (`· 2 conflicts to resolve`) while the working tree still has
    unmerged paths, and shows alone when it has none — a rebase stopped for `edit` or `reword`,
    say. When git's step-counter files are missing or unreadable the counts are dropped and the
    row reads just `⚠ rebase`, so the rebase is still surfaced. Interactive rebases and
    apply-backend rebases (`git rebase --apply`) all report as a rebase; cherry-pick, revert,
    bisect, and plain `git am` deliberately get no indicator. The row is cut to the terminal
    width rather than wrapped, and the label outranks the conflict clause for the columns
    available.
  - Under the live watch, the separator under the header carries a refresh
    clock — `──── last refresh: 3m2s ago, next refresh: 15s ─────` — so you can tell at a glance
    whether the screen is still live. Filesystem changes refresh it immediately; with nothing
    happening on disk it re-walks the repository every `--refresh-interval` seconds (default 60),
    which is what the countdown counts down to. The two halves add up to the wait the countdown is
    measuring: one interval while nothing else is pending, less when a filesystem change deferred
    through the cooldown pulls the next walk in, more when the duty-cycle budget pushes it out.
    `--refresh-interval 0` turns the timed refresh off, which removes the clock with it and leaves
    gsw purely event-driven. On a repository where a status walk is expensive, the 1% duty-cycle
    budget pushes the timed refresh out past the interval, and the countdown shows the longer wait
    rather than promising one it will not keep.
  - Watch-mode keys: `q` or Ctrl-C quits, `r` forces an immediate refresh, and `p` pushes the
    current branch. Ctrl-C quits from anywhere, including while a push is in flight.
  - `p` always asks first, and the question names the branch, the remote, and how much is going —
    so what you confirm is what runs. If the checkout moves in another pane between the question
    and your answer, the push is refused rather than redirected at the branch that is there now:
    gsw says the branch changed and you press `p` again for a question about the new one.
    A branch that is **not on the remote yet** gets a different,
    yellow confirmation (`Create new remote branch origin/my-branch?`) rather than the routine
    `Push 3 commits to origin/my-branch?`, because creating a branch on a shared remote is not the
    same act as moving one that is already there. Answer with `y`/Enter or `n`/Esc. A branch that
    is already fully pushed says so instead of asking. In a pane too short to draw the question in
    — one row, all of which the frame keeps — `p` asks nothing at all rather than asking invisibly,
    and the keys go on meaning what they mean everywhere else, so Enter can never confirm something
    you were not shown; make the pane taller and press `p` again. A pane resized down to that size
    with a question already up drops the question for the same reason.
    - An untracked branch is published with `git push -u`, so the upstream is recorded and the
      header's tracking segment appears; a tracked one runs a bare `git push` and lets git supply
      the remote and refspec. The remote for a new branch is `remote.pushDefault` if set, else the
      only remote whatever it is called, else `origin` — and gsw refuses rather than guess when a
      repository has several remotes and none of them settles it. `p` never force-pushes.
    - The push runs off the render thread, so the countdown, the ages, and resizes keep working
      while it is in flight, and a second `p` cannot start an overlapping push. git's own error
      text is shown under the frame in red (up to three rows, `hint:` advice dropped first, and
      fewer rows still on a pane too short to spare three — the frame always keeps a row) and
      stays there until you press a key. A push that succeeds re-walks the repository immediately,
      so the ahead/behind arrows match what just happened. Nothing the push runs can prompt at the
      terminal — it would be reading the same keystrokes gsw is. The push is started detached from
      the terminal — its own session on Unix, no inherited console on Windows — so nothing in its
      process tree can reach the keyboard gsw is reading, not git, not ssh, and not anything below
      them: an HTTPS remote that wants a password, a passphrase-protected key with no agent, and an
      unknown host key all fail fast and say so under the frame instead of hanging behind a
      question gsw never drew. Credential helpers and a GUI askpass still work — neither needs the
      terminal.
    - Everything gsw says itself goes away on its own: `Pushed 3 commits to origin/my-branch
      (12s ago)` counts up in place, fades toward black as it goes, and takes itself off the
      screen after a minute — the frame gets the row back with no key pressed. Refusals
      (`origin/my-branch is already up to date (7s ago)`) age and expire the same way, because
      they describe the repository as it stood when `p` was pressed. **git's error text is the
      exception and does not expire**: it is something to read and act on, so it waits for a key
      the way it always has. The fade is the 24-bit gradient the commit log uses, under the same
      `--truecolor`/`--no-truecolor` control; without truecolor the message simply dims halfway
      through its life instead.
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
  - Subagent worktree helper for parallel TDD. `swt create <name>` spins up an isolated worktree on a
    new branch and runs the green check *inside* it — a clean checkout of HEAD, so uncommitted parent
    state can't fake a pass — tearing the worktree and branch back down if it fails; `swt merge <path>`
    refuses unless both worktrees are clean and green, rebases if the parent advanced,
    fast-forward-merges, and cleans up. Concurrent merges are serialized via a `swt.lock` in the git
    directory *shared* by every worktree of the repo (`git rev-parse --git-common-dir`), so two merges
    launched from two different worktrees of one repo contend for the same lock. Drop an executable
    `.swt-check` at the parent repo root to override the default green check.
  - To install: `cargo install --git https://github.com/timmattison/tools swt`
  - Upgrading from the old TypeScript version: it was installed by symlinking `swt/swt.ts` into your
    `PATH`. That file is gone, so the symlink now dangles — and depending on `PATH` order it can keep
    shadowing the installed binary. Remove it (`rm ~/.local/bin/swt`) and run the `cargo install` above.
- install-bin
  - Installs a locally built binary into `~/.local/bin` (or `--dest <dir>`) without tripping macOS's
    per-vnode code-signature cache: `cp` over an existing binary keeps the destination inode, so the
    kernel SIGKILLs every exec of the new bytes ("Killed", exit 137) even though `codesign -vv` passes.
    `install-bin` unlinks the destination first so the copy always lands on a fresh inode, then execs
    the installed binary once (`--verify-arg`, default `--version`) to prove the kernel accepts it,
    re-signing ad-hoc and retrying once on a SIGKILL. Usage: `install-bin target/release/mytool`.
  - To install: `cargo install --git https://github.com/timmattison/tools install-bin`. Because it's
    a single binary with no runtime dependency, it can also install itself:
    `cargo build --release -p install-bin && ./target/release/install-bin ./target/release/install-bin`.
- grist
  - Ranks the orders you could squash-merge a set of branches in, cheapest conflicts first. Squash
    merging destroys commit identity, so whichever branch lands second replays work that already
    landed and collides — and the bill is not symmetric. `grist` replays every ordering against a
    throwaway detached worktree, counts the conflict hunks, stops, and files each one would cost, and
    ranks them. Up to six branches; `--onto <REF>` sets what they land on, and `-q` prints just the
    winning order for piping.
  - To install: `cargo install --git https://github.com/timmattison/tools grist`
- zth (zero the hero)
  - Recursively finds files that are larger than zero bytes and contain nothing but zero bytes, then
    prints their absolute paths - the wreckage a failed copy, a truncated restore, or a dying disk
    leaves behind. Each file is read only until its first non-zero byte, so a directory of ordinary
    files costs one read apiece no matter how large they are. The directory walk runs alongside the
    reads, so the progress bar's "discovered" count keeps climbing while files are already being
    scanned, and the estimate follows it. Errors are skipped in silence: unreadable files and
    directories, a path that does not exist, a file that vanishes mid-scan. Nothing but results ever
    reaches stdout, and nothing at all reaches stderr, so `zth /data > suspects.txt` just works.
  - Usage: `zth <PATH>`, `zth -j 32 /mnt/backups` (more readers for a network or spinning-rust
    volume; defaults to the machine's core count).
  - To install: `cargo install --git https://github.com/timmattison/tools zth`

- krt (Knights of the Round Trip)
  - Records the network path to a destination, hop by hop. `krt` accepts one destination and the
    flags of the probe. The flags set the round period, the range of the TTL, the protocol, the
    multipath mode, and the address family.
  - The `replay` command reads a file that an earlier run wrote, and it takes no destination and no
    flag of a probe. `--run` picks which run in that file to read, and the last run of the file is
    the default. A recorded file holds one JSON record on each line.
  - This build reads that file and prints one summary line for a recorded run. It writes no file,
    because it does not probe yet. The writer is in place, and it opens the file in append mode.
    One source and one destination will keep one file across many runs, once the tracer arrives. A
    later build adds the tracer and the aggregate table.
  - Usage: `krt example.com`, `krt example.com --interval 500ms --protocol udp --multipath paris`,
    `krt replay trace.jsonl`, `krt replay trace.jsonl --run 2026-08-19T12:00:00.000Z`
  - To install: `cargo install --git https://github.com/timmattison/tools krt`

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

### Env File Copying

After creating the worktree, nwt copies untracked `.env` files from the main worktree into the new one, preserving their relative paths, so development settings that aren't committed to git are there immediately. Two patterns are copied: `.env` exactly, and anything starting with `.env.` (`.env.local`, `.env.development`, and so on). Nothing else is — `.envrc` (direnv) and `.environment` don't match the pattern, and any file tracked by git is skipped, since git already puts it in the new worktree.

A destination that already exists is never overwritten. nwt leaves it exactly as it found it and prints a line naming it:

```text
Kept existing: .env.local (already in the new worktree; not overwritten from main worktree)
```

This exists because a repo's `post-checkout` hook lives in the shared git directory, so it runs during `git worktree add` — before nwt's copy. A hook that generates a worktree-specific `.env` (a unique port, a unique database name, a freshly minted secret) would otherwise have its work replaced by the main worktree's version. The worktree's own hook knows more about that worktree than the main repo does, so nwt skips and says so.

The trade-off: nwt does not parse or merge `.env` files, so when the hook writes a `.env.local`, the main worktree's `.env.local` does not reach the new worktree at all. Keys that live only in the main worktree's copy — `DISABLE_AUTH`, a shared API key, whatever else you keep there — will be missing, and you have to copy them over by hand. The per-file `Kept existing:` line is there so that divergence is discoverable. A trailing summary reports both counts (`Copied N untracked .env files to new worktree` and `Kept N existing .env files already in the new worktree`); `-q`/`--quiet` suppresses the per-file lines and the summary alike.

On Unix, copied `.env` files are created at mode `0600` — owner read/write only — no matter what the source file's mode is. A `0644` `.env` in the main worktree therefore no longer propagates a world-readable secrets file into every worktree. The mode is applied when the file is created, so the copy is never briefly readable by anyone else. Windows has no equivalent mode; everything else behaves the same there.

Disable copying for a single invocation with `--no-copy-env`, or set `copy_env = false` in `~/.nwt.toml` to disable it by default.

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

Navigate between the git worktrees of a repository and of the repositories inside it. Lists them all, cycles through them, or jumps to a specific one by name.

### Basic Usage

```bash
cwt                           # Show list of worktrees with current highlighted
cwt -f                        # Go to next worktree (wraps around)
cwt -p                        # Go to previous worktree (wraps around)
cwt -m                        # Go to the main worktree, or up a level when you are at its root
cwt main                      # Go to worktree by branch name
cwt absurd-rock               # Go to worktree by directory name
cwt vial-qmk:vial             # Go to one repository's worktree by name
```

### Options

- `-f, --forward`: Go to the next worktree in the sorted list (wraps around)
- `-p, --prev`: Go to the previous worktree (wraps around)
- `-m, --main`: Go to the main worktree (see [The main worktree](#the-main-worktree))
- `[TARGET]`: Worktree to switch to (directory name, branch name, or `REPO:NAME`)
- `--no-family`: List only the repository you are standing in
- `--shell-setup`: Automatically add shell integration to your ~/.zshrc or ~/.bashrc
- `-q, --quiet`: Suppress error messages

### The main worktree

`cwt -m` (and the `wtm` alias) goes to the worktree on branch `main`. If no worktree is on
`main`, it goes to the worktree on branch `master`, so a repository that never renamed its
first branch gets the same shortcut.

In a family, `cwt -m` stays in the repository you are standing in until you are already at
the top of its main worktree. Every repository of a family has a main worktree of its own,
and the shortcut takes you to yours.

The branch name must match exactly. This is what separates `cwt -m` from `cwt main`, which
also substring-matches: in a `master` repository, `cwt main` lands on a branch such as
`wt-main-master`, and `cwt -m` does not. A detached worktree has no branch, so it is never
the main worktree.

If your repository has neither branch checked out, `cwt -m` lists the worktrees and exits
with code 3.

#### Pressing it again climbs out

When the directory you are in **is** the main worktree, you have asked to go up. So the
next `wtm` takes you to the main worktree of the repository that **holds** yours, and the
one after that goes up again, for as deep as the repositories are nested:

```bash
cd keyboards/zmk-config-corne-worktrees/guard-the-commit-hashes-in-prose
wtm   # -> keyboards/zmk-config-corne        the repository's own main worktree
wtm   # -> keyboards                         the repository that holds it
wtm   # Error: No repository above /code/keyboards has a main worktree
```

It is the directory you stand in that decides, not the worktree that holds you. From a
subdirectory of the main worktree — `zmk-config-corne/config`, say — `wtm` takes you to the
top of it, exactly as it does from a subdirectory of any other worktree. Only the top of
the main worktree has anywhere left to go, so that is the only place the climb starts:

```bash
cd keyboards/zmk-config-corne/config
wtm   # -> keyboards/zmk-config-corne        the top of the worktree you are in
wtm   # -> keyboards                         now the climb starts
```

The climb is measured from where your repository **sits on disk** — its own main worktree —
never from the worktree you happen to stand in. That is what keeps the first `wtm` above
from skipping `zmk-config-corne` and landing on `keyboards`.

Only the directory directly above counts, the same one level the family scan looks down. A
repository on the way with neither `main` nor `master` cannot be a destination, so the
climb steps over it and asks the repository above it. When nothing above has a main
worktree, `cwt -m` says so and exits with code 3. `--no-family` does not change any of
this: it says which repositories the **listing** shows, and the climb is not a listing.

### Families of Repositories

Some repositories are containers. They track the map of a workspace, and the real
repositories sit one level below them, kept out of the parent's history by
`.gitignore`:

```text
keyboards/                    # the parent repository
  qmk_firmware/               # a repository of its own
  vial/                       # a repository of its own
  vial-qmk/                   # a repository of its own
  zmk-config-corne/           # a repository of its own
```

`cwt` treats the parent and its children as one **family**. One listing shows every
worktree of every repository, grouped by repository, and one name reaches any of them:

```bash
cd keyboards && cwt
# keyboards
# >   /code/keyboards                                          [main]
#     /code/keyboards-worktrees/keymap-parity-tool             [keymap-parity-tool]
#
# qmk_firmware
#     /code/keyboards/qmk_firmware                             [master]
#
# vial-qmk
#     /code/keyboards/vial-qmk                                 [vial]
#     /code/keyboards/vial-qmk-worktrees/split-handedness-build [split-handedness-build]
```

The family is the same from anywhere inside it, so `cwt` gets you out of a child
repository as easily as it gets you in. Only one level is scanned: a child of a child
is that child's business.

A directory that looks like a repository but has no worktree to offer — a bare
repository, which git lists without a HEAD — cannot join the family, because there is
nothing there to change into. `cwt` leaves it out of the listing and says so on
standard error, so a directory you can see is never missing without a reason:

```text
Warning: skipped /code/keyboards/archive: no worktrees to list
```

#### Which repository answers a name

A name is offered to each repository in turn, nearest first:

1. The repository you are standing in
2. The parent repository the family is anchored at
3. Every other repository in the family

So `wtm` still means "my repository's main branch" wherever you are standing — and,
once you are standing in it, the repository above that. Within
each repository the order is the same as before: exact directory name, then exact
branch name, then a case-insensitive substring of a branch name. An exact name
anywhere in the family beats a substring anywhere.

#### When two repositories share a name

`cwt` refuses to guess, and names the candidates the way you have to type them:

```bash
cwt master
# Error: Multiple worktrees match 'master'. Be more specific:
#   qmk_firmware:qmk_firmware [master]
#   zmk-config-corne:zmk-config-corne [master]
```

A `REPO:NAME` target searches one repository of the family. The repository can be
named in full or by any part of its name, and `REPO:` on its own selects that
repository's main worktree:

```bash
cwt zmk-config-corne:master    # that repository's master
cwt zmk:                       # that repository's main worktree
```

Every name `cwt` prints can be typed straight back, so two repositories of one family
cannot be left sharing a name — not even the parent and a child named after it. The
child is listed, and typed, as the path that leads to it:

```bash
cd keyboards && cwt
# keyboards
# >   /code/keyboards                              [main]
#
# keyboards/keyboards
#     /code/keyboards/keyboards                    [main]
#     /code/keyboards/keyboards-worktrees/inner    [inner]

cwt keyboards:                 # the parent
cwt keyboards/keyboards:       # the child checked out inside it
cwt keyboards/keyboards:inner  # a worktree of that child
```

#### Staying in one repository

`--no-family` confines the listing, the cycling, and the name search to the repository
you are standing in. Set `CWT_NO_FAMILY` to any value other than `0` to make that the
default.

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
alias wtm='wt --main'  # Main worktree, or a level up when you are at its root
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
alias wtm 'wt --main'  # Main worktree, or a level up when you are at its root
```

### Examples

Show all worktrees with current highlighted:
```bash
cwt
#   /path/to/repo                        [main]
# > /path/to/repo-worktrees/absurd-rock  [feature-branch]
#   /path/to/repo-worktrees/zen          [fix-bug]
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
wt vial-qmk:vial  # By repository and branch name
wtm               # The main worktree, or a level up when you are at its root
```

### Exit Codes

- `0`: Success
- `1`: Not in a git repository
- `2`: Git command error
- `3`: Worktree not found
- `4`: Could not determine current worktree (for -f/-p)
- `5`: Shell setup failed
- `6`: Multiple worktrees matched the name (be more specific)

## gitnuke (nuke a worktree)

Removes a git worktree and deletes the branch it had checked out. The two halves are one
operation: the target is resolved against `git worktree list`, so the branch gitnuke deletes
is whatever that worktree actually had checked out, and it is only deleted once the removal
has succeeded. A refused removal never leaves the branch destroyed and the worktree standing.

### Basic Usage

```bash
gitnuke ../issue-42-wt        # by path
gitnuke issue-42-wt           # by directory name
gitnuke issue-42              # by branch name
gitnuke --force issue-42      # worktree has submodules or uncommitted changes
gitnuke --dry-run issue-42    # report the plan, change nothing
gitnuke --safe issue-42       # keep the branch unless it is fully merged
gitnuke wt-a wt-b wt-c        # nuke several; each is reported independently
```

### Worktrees with submodules

Plain `git worktree remove` refuses a worktree that has submodules checked out:

```text
fatal: working trees containing submodules cannot be moved or removed
```

gitnuke refuses too, but tells you what is in the way and how to override it:

```text
gitnuke: /code/repo-worktrees/issue-42 contains submodules (sub) — git refuses to remove a
worktree with submodules checked out.
  Nuking it deletes those checkouts along with any uncommitted or unpushed work inside them.
  Re-run with --force to nuke it anyway.
```

`--force` is required because that warning is literal: a submodule checkout can hold commits
and untracked files that exist nowhere else, and removing the worktree deletes them silently.
One `--force` covers both of the refusals gitnuke overrides — submodules checked out and
uncommitted changes in the worktree itself.

### Worktrees with uncommitted changes

git's other refusal, and gitnuke raises it the same way it raises the submodule one — on its
own terms, before `git worktree remove` is ever reached:

```text
gitnuke: /code/repo-worktrees/issue-42 contains modified or untracked files — nuking it
discards work that exists nowhere else.
  Re-run with --force to nuke it anyway.
```

Untracked files count, ignored files do not — the same question `git worktree remove` asks.
This is exit code `9`, and `--dry-run` reports it with the identical message: one gate serves
both runs, so they cannot drift. A worktree that has *both* submodules and uncommitted
changes reports the submodule refusal (`5`), because a submodule checkout is the more
consequential thing `--force` would take with it.

### Locked worktrees

git has a third refusal, and `--force` is not the answer to it. A worktree you locked with
`git worktree lock` is declined even by `git worktree remove --force`, which asks for
`remove -f -f` instead. A lock is a deliberate "leave this alone" marker you set by hand, so
gitnuke honours it rather than escalating, and says so before it touches anything:

```text
gitnuke: /code/repo-worktrees/issue-42 is locked (mid-bisect, do not touch) — git refuses to
remove a locked worktree even with --force.
  Unlock it first: git worktree unlock /code/repo-worktrees/issue-42
```

The reason in parentheses is whatever you passed to `git worktree lock --reason`; it is left
out when the lock has none. This is exit code `8`, and `--dry-run` reports it too.

### Safety rails

- **Exact matches only.** Unlike `cwt`, gitnuke never substring-matches: asking for
  `issue-42` will not resolve `issue-421`. A target that matches both one worktree's
  directory name and another's branch name is reported as ambiguous rather than guessed at.
- **Never your own directory.** gitnuke refuses to remove the worktree your shell is
  standing in (or any parent of it) and names somewhere else to `cd` to first. It is a
  binary, not a shell function, so it cannot move your shell out of a directory it deletes.
- **Never the main worktree.** It is the worktree the repository itself lives in, and git
  declines to remove it with or without `--force`. gitnuke refuses it up front and says what
  to do instead, rather than letting git's `fatal: '…' is a main working tree` be the answer —
  a `--dry-run` never runs that command, so leaving the rule to git meant the dry run cleared
  the one worktree nothing can ever remove. Both runs report exit `2`, the code git's own
  refusal produces.
- **Never a branch whose removal git refused.** The branch is deleted only once the worktree
  is actually gone.
- **Uncommitted work is gitnuke's own refusal.** Submodules and uncommitted changes are both
  diagnosed by gitnuke before `git worktree remove` is invoked, so each gets a message that
  names what is in the way and an exit code of its own (`5` and `9`) rather than git's
  locale-dependent `fatal:` under the generic `2`.
- **A lock is honoured, not overridden.** A worktree locked with `git worktree lock` is
  refused even with `--force`, quoting the lock reason if git recorded one and handing back
  the `git worktree unlock` command to run.
- **`--dry-run` is a real preflight.** It runs every check a real run runs — submodules,
  uncommitted changes, and under `--safe` whether the branch is merged — and exits with the
  same status that run would, so `gitnuke -n x` failing means `gitnuke x` would fail too.

### Options

- `-f, --force`: Nuke the worktree despite the two refusals it overrides — submodules checked
  out, or uncommitted changes. It does not override a locked worktree; that is refused
  separately, with instructions to unlock it
- `-s, --safe`: Keep the branch unless it is fully merged (`git branch -d` semantics instead
  of the default force-delete). Only affects the branch; the worktree is still removed
- `-n, --dry-run`: Report what would happen without removing or deleting anything
- `[TARGETS]...`: One or more worktrees, each given as a path, directory name, or branch name

### Exit Codes

- `0`: Success
- `1`: Not in a git repository
- `2`: A git command failed
- `3`: No worktree matched the target
- `4`: The target matched more than one worktree
- `5`: The worktree contains submodules and `--force` was not given
- `6`: The shell is standing inside the target worktree
- `7`: The worktree was removed but its branch could not be deleted
- `8`: The worktree is locked, which `--force` does not override
- `9`: The worktree contains modified or untracked files and `--force` was not given

### Replacing the shell functions

gitnuke supersedes the hand-rolled `gitnuke`/`gitclean` shell functions this pattern usually
starts as:

```bash
# Before: deletes the branch even when the worktree removal failed, and only
# works when the worktree's directory name happens to equal its branch name.
gitnuke() { git worktree remove "$@"; git branch -D "$@"; }
```

If you keep a `gitclean` alias for the safe variant, point it at the binary so it inherits
the same submodule handling and ordering guarantees:

```bash
gitclean() { gitnuke --safe "$@"; }
```

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

`--here` also accepts a cross-user source. Combine it with [`--user`](#resume-another-users-session---user) — `crap --here <id> --user alice` — to fork another account's session right here in your current directory. Because a transcript in someone else's home can never be found by a `claude --resume` you run yourself, `crap` **copies** it into your own tree rather than symlinking (nothing is ever linked into another user's home), then forks and cleans the copy up the same way it removes the symlink. A same-user `--here` still symlinks exactly as before. This is the escape hatch when the session's original directory is gone or you can't enter it: `--here` ignores that directory entirely.

#### Choosing the forked session's id

By default the fork gets a random new id, which you only learn after Claude starts. Pass a second argument to choose it yourself:

```bash
crap --here 57570685-2d64-4431-8ab6-c021a12fa1af 9f8e7d6c-5b4a-3210-fedc-ba9876543210
```

The new id must be a valid UUID, and `crap` refuses it if it already names a session (so the fork can never overwrite an unrelated transcript). This is handy when a script needs to know the resumed session's id in advance — generate a UUID, hand it to `crap --here`, and you already know where the new transcript will live. Omit it to keep the random-id behavior.

### Resume another user's session: automatic, or `--user`

A session started under a different account is found automatically: when the id isn't in your own tree, plain `crap <id>` falls back to the other accounts on the machine (see below). `--user <name>` is the explicit form — point `crap` at one account by name and it searches only that user's `~/.claude/projects` tree instead:

```bash
crap 57570685-2d64-4431-8ab6-c021a12fa1af --user alice
```

The name resolves as a sibling of your home (`<home>/../<name>` — `/Users/alice` on macOS, `/home/alice` on Linux). Because a transcript that belongs to another user can never be found by a `claude --resume` you run yourself, `crap` copies it into your own tree and resumes it as a `--fork-session` (a fresh id) at its original recorded directory — the foreign transcript is only ever read, and every write lands under your home. The transient copy is removed once Claude writes the forked transcript, the same way `--here` cleans up its import (a symlink for a same-user source, a copy for a cross-user one). Because the fork only reads the original, `--user` is safe even while that session is still live in the other user's account. A `--user` naming your own account is a same-user hit and simply resumes in place.

Most of the time you don't need the flag at all. With no `--user`, `crap <id>` searches **your own** tree first — byte-for-byte the same fast path as always — and only if the id isn't there does it automatically fall back to scanning every sibling home that has run Claude, resuming the first readable match exactly as `--user` would (copy into your own tree, fork at the original directory). The fallback is **self-first**, so an id that happens to exist in two accounts always resolves to *your* copy, never the foreign one. Reach for `--user <name>` only when you want to force a specific account — it skips your own tree entirely, which is also how you disambiguate on purpose.

### When a project directory is owner-only

Claude Code creates its project folders with whatever the owner's umask gives them, and on a shared machine that is often `0o700` — the owner may read, list, and enter; nobody else may do anything. Such a directory is *opaque*, not merely unreadable: `crap` can't list it, so it can't even tell whether the session you asked for is inside. Seeing inside would take `sudo`, and `crap` scans other homes with exactly the privileges you invoked it with.

So a directory it is refused is **skipped and remembered** — along with the account that owns it — instead of ending the search. (The same goes one level up, when an entire account's `~/.claude/projects` can't be listed.) Treating a permission error as fatal would let one locked folder in someone else's home hide a session sitting perfectly readable two folders later; this way an unreadable neighbour never makes a readable session unfindable.

If the id then turns up nowhere readable **and** at least one directory was skipped, `crap` doesn't flatly claim the session doesn't exist — a claim it isn't in a position to make. The headline hedges to "no *readable* Claude session", every account that refused it gets a count of what was stepped over, and you get the remedy as commands you can paste:

```text
Error: no readable Claude session found with id '99999999-8888-7777-6666-555555555555'
       looked under /Users/me/.claude/projects
       …and 2 other accounts on this machine
       1 project dir under user 'alice' is owner-only and was skipped.
       1 project dir under user 'scyloswork' is owner-only and was skipped.
       if the session is in one of those, crap cannot read it — and crap never
       runs sudo itself. copy it into your own tree first — for user 'alice',
       for example:
         SRC=$(sudo -u alice find /Users/alice/.claude/projects -name '99999999-8888-7777-6666-555555555555.jsonl')
         mkdir -p ~/.claude/projects/-Users-me-code-foo
         sudo -u alice cat "$SRC" > ~/.claude/projects/-Users-me-code-foo/99999999-8888-7777-6666-555555555555.jsonl
         crap --here 99999999-8888-7777-6666-555555555555
```

Those four lines do exactly what `crap` would have done for you if it could see: find the transcript anywhere in the owning account's tree, copy it into your own, and resume it. The copy lands in the project folder for **the directory you're standing in**, and the last line is therefore [`crap --here`](#resume-in-the-current-directory---here) rather than a plain resume — with the transcript unreadable, the directory recorded inside it is precisely the thing that can't be known, and your current directory is both knowable and almost certainly where you meant to work, since it's where you typed the id. Counts are printed for every account, but the worked example is keyed to the first one named; if the session turns out to belong to a different account, swap the name in.

That is where `crap` stops. It prints the escalation for you to read, weigh, and run yourself — it never runs one, not even to satisfy the request you just made. This isn't a convention a later change could quietly relax: a test scans the compiled source for every program the binary spawns and fails on anything outside a two-entry allowlist (`ps`, for the is-it-still-running check, and `bash`, which the shell-integration tests use), and it holds the shell function `--shell-setup` installs to the same rule. Escalating stays your decision, and the audit trail points at you rather than at a tool that reached for `sudo` on your behalf.

A miss with nothing skipped is unchanged: `crap` looked everywhere it can look, so it still says plainly that no session has that id.

### When the original directory is gone or sealed, or the account is wrong

A cross-user resume lands you at the session's **original** recorded directory (`--here` is the exception — it deliberately ignores that directory and forks where you stand). When the original directory can't be used, `crap` refuses rather than silently dropping you in the current directory — the same contract a same-user resume has always had — and points you straight at the escape hatch that works precisely then:

```text
Error: the directory for session '57570685-…' no longer exists:
       /Volumes/code/old-worktree
       use 'crap --here 57570685-…' to fork it in the current directory instead.
```

There are two distinct versions of this, because "gone" and "sealed" are different facts. If the directory simply isn't there any more, that's the `no longer exists` message above (exit code `3`). If it *is* there but your account can't enter it — a parent folder you lack permission on, or the directory missing its search bit — you get `cannot be entered from this account:` instead (exit code `11`), never a silent fall-back to wherever you happened to be. Either way, `crap --here <id>` is the fix: it never touches the original directory, so it succeeds exactly when the original directory is the problem.

Separately, if `--user <name>` names an account that isn't there — a typo, or a real account that has simply never run Claude — `crap` says so up front instead of searching a phantom tree and reporting a misleading "no session found". It lists the accounts you *can* resume from (exit code `12`):

```text
Error: --user 'alcie' does not name an account with a Claude projects tree.
       accounts you can resume from with --user:
         me
         alice
```

An account whose projects tree merely *exists but is unreadable* to you is **not** treated as invalid — it's a real account, so `crap` resolves it and the [owner-only guidance](#when-a-project-directory-is-owner-only) above takes over (which, unlike this message, does show you the `sudo` remedy). `--user` reports a wrong *name*; owner-only reports sealed *data*.

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

- `[SESSION_ID]`: The Claude session id to resume (optional with `--status`, which then lists every session for the current directory). The lookup is **self-first** — your own tree first, then, only on a miss, every sibling home that has run Claude — so an id belonging to another account is found and forked automatically, with no flag
- `-f, --force`: Resume even if the session appears to be running in another process
- `--here`: Resume the session in the current directory (as a forked, new-id session) instead of its original one; also accepts a cross-user source (combined with `--user`), which is copied into your own tree rather than symlinked
- `--user <name>`: Resume another user's session from a specific account — **not required** for a cross-user resume, since the no-flag path already falls back on its own; reach for it when you want to force one account in particular. `<name>` is resolved as a sibling of your home (`<home>/../<name>` — `/Users/<name>` on macOS, `/home/<name>` on Linux), and only that user's `~/.claude/projects` tree is searched (your own is skipped, so `--user` also disambiguates an id on purpose). The foreign transcript is copied into your own tree and resumed as a fork (fresh id) at its original directory — or in the current directory instead when combined with `--here` — the original is only ever read. Naming your own account is a same-user hit and resumes in place
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

The binary speaks one of three output shapes. By default it prints the session id on the first line and the original directory on the rest; the function takes the first line as the session id and everything after it as the directory (so a path containing a newline stays intact), `cd`s there, and resumes. For `--here` it leads with a `__CRAP_HERE__` marker — having already imported the session into the current directory's project folder (a symlink for a same-user source, or a copy for a cross-user source) — so the function stays put and resumes with `--fork-session`. A backgrounded watcher counts the `.jsonl` files in that folder and removes that import as soon as a new (forked) one appears, so it doesn't linger for the whole session; a `kill` plus `rm` after Claude exits stops the watcher and serves as a safety net. If the link field is `__CRAP_NO_LINK__`, no symlink was needed and the watcher is skipped. For a cross-user resume — whether `--user` asked for one or the automatic fallback found it — it leads with a `__CRAP_FORK_AT__` marker instead — the binary has already *copied* the foreign transcript into your own tree, and the wire layout appends the session's original directory as a trailing field, so the function `cd`s into that original directory and then runs the same `--fork-session` plus background-watcher cleanup sequence as `--here`. Forwarding `"$@"` lets flags like `--force` and `--here` reach the binary. The `eval` is intentional: shell aliases aren't expanded inside function bodies, so it ensures a `clauded` alias is honored at call time. The `command crap` calls reach the binary past the function of the same name.

### Exit Codes

- `0`: Success
- `1`: No session found with that id
- `2`: Session has no recorded working directory
- `3`: The session's original directory no longer exists
- `4`: Invalid session id
- `5`: Shell setup failed
- `6`: Could not determine your home directory
- `7`: The session is already running in another process (use `--force` to override)
- `8`: `--here` or a cross-user resume (whether `--user` asked for one or the automatic fallback found it): could not import the session into the project folder (create the folder, or make the symlink for a same-user source / copy the transcript for a cross-user source)
- `9`: `--here`: could not determine the current working directory
- `10`: `--here`: the requested new session id already names an existing session (choose a fresh id so the fork can't overwrite it)
- `11`: The session's original directory exists but can't be entered from this account (a sealed parent, or a missing search bit) — use `crap --here <id>` to fork it in the current directory instead
- `12`: `--user <name>` named no account with a `.claude/projects` tree (a typo, or an account that never ran Claude); the message lists the accounts you can resume from

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

## zth (zero the hero)

Recursively hunt down files that are larger than zero bytes and contain nothing but zero bytes, and print their absolute paths. Named for the relaxing Cannibal Corpse cover of Black Sabbath's "Zero the Hero".

Files full of nothing are the residue of something going wrong: an interrupted `dd`, a restore that allocated the file but never filled it, a network copy that dropped, a drive quietly returning zeroes on the way out. They look fine in `ls` — right name, right size — and only give themselves up when you read them.

### Basic Usage

```bash
zth /Volumes/Backup
zth /Volumes/Backup > suspects.txt
zth -j 32 /mnt/nas
```

### Options

- `-j`, `--jobs <N>`: How many files to read at once. Defaults to the machine's core count. Scanning waits on the storage device far more than on the CPU, so a network share or a spinning disk often does better well above the core count.
- `<PATH>`: The directory to scan recursively. A single file works too, in which case only that file is checked.

### How it reads

Each file is read only until its first non-zero byte, so an ordinary file costs a single read no matter how large it is — a 4 GB video is dismissed by its first few bytes. Only files that really are all zeroes get read to the end. Blocks are compared against a zero block with `memcmp`, which the CPU vectorizes, so the all-zero case runs at memory speed rather than byte-at-a-time.

That first read asks for 16 KiB, not a full buffer. Almost every file is disqualified by its very first byte, so the first read exists to reject rather than to consume, and pulling 256 KiB off the platter to look at one of them is waste in two directions — the transfer itself, and the page cache it evicts. Three hundred thousand files at a quarter-megabyte apiece flush the directory metadata the walk is still working through, which buys extra seeks in exchange for bytes nothing ever reads. Once a file survives the probe it is likely to be all zeroes, so every read after the first one goes full width and runs sequentially.

For the same reason, `zth` asks the kernel not to cache what it reads at all (`F_NOCACHE` on macOS, `posix_fadvise` on Linux). It reads every byte exactly once and never comes back, so a scan that filled the cache would only be competing with itself.

Sparse files are settled without being read. A file made entirely of a hole — a range the filesystem never allocated, which reads back as zeroes without any of it existing on disk — is exactly what an interrupted restore leaves behind, and it is routinely enormous. One `lseek` against metadata already in memory answers it, so a 64 GB sparse file costs the same as a 64-byte one. Filesystems that cannot answer the question are simply read the ordinary way.

Empty files are never reported, however they were made. A file with no bytes has no zero bytes either, and a directory full of `touch`ed placeholders is not what you are looking for.

### Spinning disks

Reading many small files at once helps on a hard drive, which surprises people who have been told that disks hate concurrency. That advice is about independent sequential streams thrashing the head against each other; this is the opposite workload. Command queuing lets the drive hold up to 32 outstanding reads and service them in whatever order costs the least head travel, so a single 7200 RPM spindle that manages roughly 75 random reads per second one-at-a-time will manage two to three times that with a full queue.

`-j 24` or so is the sweet spot for one spinning disk. Past about twice the drive's queue depth the extra workers just wait in line. Two things to know: a USB enclosure speaking the older mass-storage protocol instead of UASP has no queue at all and gains nothing from any of this, and a repeat run measures the page cache rather than the disk unless you clear it first.

### The progress bar

The directory walk and the reading run at the same time, which is what makes the estimate honest: discovery keeps turning up files while workers are already reading, so the bar shows the count discovered so far, the count still waiting, and a time estimate that re-derives itself as the denominator grows.

```
⠲ [███████████▍                    ] discovered 105,564 · remaining 71,628 · ETA 23s
```

It draws on stderr, so it stays out of the way of the results on stdout and disappears entirely when stderr is not a terminal. That is the split you want: `zth /Volumes/Backup > suspects.txt` still shows you the bar while the list fills up, and a run whose stderr goes to a pipe or a log file — a script, a cron job, CI — draws nothing at all.

### Errors

Every I/O error during the scan is skipped without a word: unreadable files, unreadable directories, a path that does not exist, a file that vanishes mid-scan. `zth` never writes a diagnostic to stderr — the progress bar has it to itself — and none of those failures touch the exit status, so a scan of a large tree you do not fully own produces a clean list rather than a screenful of permission complaints. The one thing that does fail the run is a failure to write the results themselves — a full disk swallowing half of `> suspects.txt`, say — because a truncated list must never be able to pass for a complete one. A reader that simply stops early is not that: `zth /data | head` still exits 0.

Symlinks are reported by neither name nor target — they are not followed, so a scan cannot escape the tree it was pointed at or report the same file twice through two paths.

### Examples

Find the damage on a backup drive and count it:
```bash
zth /Volumes/Backup | wc -l
```

Check a single suspicious file:
```bash
zth ~/Downloads/ubuntu.iso
```

Delete what turns up, after reading the list first:
```bash
zth /Volumes/Backup > suspects.txt
less suspects.txt
tr '\n' '\0' < suspects.txt | xargs -0 rm --
```

## occ (old Claude Code)

List the Claude Code sessions running on this machine, oldest release first, with the process id, the release, how long the session has been open, the session id, and the working directory.

A Claude Code session keeps running the release it started on. Upgrades land in the background and change nothing for a session already open, so a machine that upgrades often accumulates sessions spread across many releases. Left alone they are easy to miss: a session opened six weeks ago looks exactly like one opened this morning, and a terminal tab or a detached multiplexer pane can hold one for months. `occ` puts the oldest ones at the top, which is where the ones worth closing are.

### Basic Usage

```bash
occ
```

### Options

- `-V`, `--version`: Print the version, the git hash, and whether the build was clean.
- `-h`, `--help`: Print the usage.

### How it reads the release

Each Claude Code release installs as a single executable named for its version, such as `~/.local/share/claude/versions/2.1.232`. macOS records the basename of the executed file as the process accounting name, so a running session reports its own release through the kernel, and `occ` reads it there.

Nothing else on the machine answers the question correctly. The executable path of a running session resolves through the `claude` launcher, which is a link to whichever release is installed *now* — read the release from that path and a session running a four-month-old release is reported as running today's. The accounting name is recorded when the process starts and never changes afterwards, so it also survives an upgrade that deletes the old release file. A session running a release that no longer exists on disk is exactly the session this tool exists to find, and it is still named correctly.

### How it identifies the session

A live session writes `~/.claude/sessions/<pid>.json` and keeps it current. The record names the session, so `occ` reads the answer rather than working it out. `claude agents --json` prints these same records. `occ` reads the files instead, because that command costs a subprocess on every run and drops the `version` field, which is the one fact this tool exists to report.

A record is believed only when it is about the process asking. The file is named for a process identifier, and an identifier is reused once the process holding it dies, so a file can outlive the session it describes. Two checks reject such a file: the identifier recorded inside it must be the one asked about, and the recorded start must agree with the process's own start. Measured across 119 live sessions, a session registered itself between one and nine seconds after its process started, and `occ` allows two minutes.

A session that wrote no record is left blank and counted in the footer. Seven of 126 sessions on the machine this was built against had no record. A blank is the whole answer there, because the alternative was measured: reconstructing the link from the transcripts under `~/.claude/projects` — by working directory, release, and creation time — named the wrong session for 25 of those 126 and refused to name 70 more. Printing one session's id against another session's process is the worst failure available here, because nothing in the output would say it had happened.

### What it leaves out, and says so

- **Support processes.** The background daemon, pty hosts, and spares run the same executable but are not sessions. They are counted in the footer, not listed.
- **Spawned tools.** A tool a session starts, such as a search, can still be holding the Claude Code executable at the moment the process table is sampled. It reports its own name in `argv[0]`, which is how `occ` tells it apart from a session.
- **Other accounts' sessions.** They appear in the process table but give up no arguments, no working directory, and no start time without privileges `occ` does not ask for. They are counted in the footer. Reporting a clean machine while sixty unreadable sessions run on it would be worse than saying they cannot be read.
