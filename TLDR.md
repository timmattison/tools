# TL;DR

A one-line description of every program documented in the [README](./README.md), alphabetized. See the README for full options, examples, and install commands.

| Tool | What it does |
| --- | --- |
| `aa` | Quickly prints your AWS caller identity as JSON, no pager. |
| `aws2env` | Converts AWS credentials/config into env-var exports (use with `eval $(aws2env)`). |
| `beta` | Records and replays terminal sessions; exports to HTML players or MP4/GIF. |
| `bm` | Bulk Move — recursively find and move files matching a pattern to a destination. |
| `cdknuke` | Removes `cdk.out` directories from AWS CDK projects throughout a repo. |
| `cf` | Count Files — recursively counts files, with optional suffix/prefix/substring filters. |
| `claude-usage` | Parses an Anthropic API usage CSV and computes per-model costs. |
| `clipboard-random` | Generates random binary or Zalgo text data and copies it to the clipboard. |
| `crap` | Claude, Resume Anywhere Please — resume a Claude Code session from its original directory (refuses if it's already running, or if that directory is gone or unenterable — pointing you at `--here` to fork where you stand); if the id belongs to another account it's found automatically (self-first), or target one with `--user` (which errors and lists the real accounts if you name one that never ran Claude); owner-only project dirs are skipped, then named in the miss with copy-paste recovery commands — it never runs `sudo` itself; `--status <id>` reports where a session left off without resuming, with the same `--user`/self-first cross-user discovery but read-only (no copy, no fork). |
| `cwt` | Change Worktree — navigate, cycle, or jump between git worktrees. |
| `dirc` | Copies the current directory to the clipboard, or emits a `cd` from a clipboard path. |
| `dirhash` | SHA256 hash of a directory tree's contents to compare directories for equality. |
| `diskhog` | Live terminal UI of per-process disk I/O on macOS (IOPS with sudo). |
| `freeport` | Finds a free TCP port on localhost, cross-platform. |
| `gitdiggin` | Recursively searches git repos for commits containing a string (messages and diffs). |
| `gitrdun` | Shows your recent git commits across multiple repositories. |
| `glo` | Finds and displays large objects in git repositories. |
| `goup` | Updates Go dependencies across all `go.mod` files in a repo. |
| `gr8` | Displays GitHub API rate limit info, color-coded, via the GitHub CLI. |
| `gsw` | Git Status Watch — compact one-shot status dashboard meant to be wrapped by `viddy`/`watch`. |
| `hexfind` | Searches for a hex string in a binary file and shows a hex dump with offsets. |
| `htmlboard` | Pretty-prints HTML on the clipboard and puts it back. |
| `ic` | Fast terminal image/video display utility (an `imgcat` alternative; video needs ffmpeg). |
| `idear` | IDEA Reaper — cleans up orphaned `.idea` directories left by JetBrains IDEs. |
| `install-bin` | Installs a locally built binary onto a fresh inode so macOS's signature cache can't SIGKILL it, then verifies it execs. |
| `inscribe` | Generates git commit messages from staged changes using Claude AI. |
| `jsonboard` | Pretty-prints JSON on the clipboard and puts it back. |
| `kitchen-sync` | Installs every Rust binary from a git repo with one command. |
| `localnext` | Serves statically exported Next.js apps locally. |
| `ng` | Navel-Gaze — watches JS/TS files and re-runs `pnpm lint` (or `--typecheck`) on change. |
| `nodenuke` | Removes `node_modules` directories and lock files throughout a repo. |
| `nodeup` | Updates npm/pnpm/yarn packages across all `package.json` directories. |
| `nwt` | New Worktree — creates a git worktree with a random Docker-style name. |
| `op-cache` | 1Password credential cache wrapping `op read` to avoid repeated prompts/Touch ID. |
| `org-borg` | Bulk clone, update, and archive GitHub organization repositories via the GitHub CLI. |
| `pk` | Process Killer — kills processes (incl. ones `ps`/`pkill` can't see) with dry-run, regex, and signal options. |
| `polish` | Updates Rust dependencies across all `Cargo.toml` files in a repo. |
| `portplz` | Generates a consistent unprivileged port number from directory name and git branch. |
| `prcp` | Copies files with a Unicode progress bar; wildcards, multi-file, and verified move mode. |
| `prgz` | Like `prcp` but gzip-compresses the file, showing progress in the console. |
| `prhash` | Hashes files (MD5/SHA1/SHA256/SHA512/Blake3) with a progress bar, shasum-compatible output. |
| `procinfo` | Detailed info about running processes matching a name (cwd, open files, connections). |
| `r2-bucket-cleaner` | Lists and optionally clears all objects from a Cloudflare R2 bucket via wrangler. |
| `rcc` | Rust Cross Compiler — simplifies cross-compilation (target detection, `Cross.toml`, build). |
| `reposize` | Calculates the total size of a git repository, human-readable. |
| `repotidy` | Runs `go mod tidy` in all `go.mod` directories within a git repo. |
| `rr` | Rust Remover — runs `cargo clean` across all Rust projects to free disk space. |
| `runat` | TUI to run a command at a specified time with a live countdown. |
| `safeboard` | Monitors the clipboard for dangerous/invisible Unicode used in copy-paste attacks. |
| `sf` | Size of Files — total size of files in directories, with optional suffix/prefix/substring filters. |
| `sirn` | Serve It Right Now — a zero-config HTTP file server; serves files or the current directory on a git-derived port. |
| `spv` | Smart Process Viewer — find and view processes by PID/name/regex, with optional cwd and open files. |
| `subito` | Subscribes to AWS IoT Core topics and prints received messages. |
| `swt` | Subagent Worktree — isolated-worktree helper for parallel TDD (create/merge with green checks). |
| `symfix` | Recursively finds and optionally fixes broken symlinks. |
| `tc` | Token Count — counts estimated LLM tokens in files (multiple OpenAI tokenizers, stdin support). |
| `tsm` | Terminal Session Manager — records every shell command to JSONL logs you can search and replay. |
| `tubeboard` | Extracts the video ID from a YouTube URL on the clipboard. |
| `unescapeboard` | Unescapes one level of `\"`-style escaping in clipboard text. |
| `update-aws-credentials` | Writes AWS SSO clipboard credentials into your AWS config file. |
| `uuidplz` | Prints a random v4 UUID, or a repeatable v5 UUID seeded from a string, a file's contents, or stdin. |
| `vpn-tunnel` | Generates Docker-based gluetun + ProtonVPN + WireGuard tunnels with helper scripts. |
| `wifiqr` | Generates WiFi QR codes (with optional logo) for automatic device connection. |
| `wl` | Shows which process is listening on a given port. |
| `wolly` | Wake-on-LAN tool that sends magic packets with auto subnet broadcast detection. |
| `wu` | Cross-platform "who's using" a file/directory/device (process name, PID, user, mode). |
