# Fun tools written by Tim Mattison

I started this repo forever ago (2014!) to hold some tools I needed at the time. Now I'm converting the tools to Golang
for fun. Several tools have also been ported to Rust for improved performance and cross-platform compatibility.

## The tools

- dirhash
    - Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
      identical. This hash will only be the same if the directories have the same file names and the same file contents.
      However, we ignore the directory names and locations of files in the directories. See below for an example.
    - To install: `cargo install --git https://github.com/timmattison/tools dirhash`
- prcp
    - Copies a file and shows the progress in the console. Useful for when you're copying large files and you don't
      want to keep opening a new terminal window to run `du -sh` to see how much has been copied. You can also press the
      space bar to pause the copy and press it again to resume.
    - To install Go version: `go install github.com/timmattison/tools/cmd/prcp@latest`
    - To install Rust version: `cargo install --git https://github.com/timmattison/tools prcp`
- prgz
    - Similar to `prcp` but instead of copying a file it gzip compresses it. It shows the progress in the console.
    - To install: `go install github.com/timmattison/tools/cmd/prgz@latest`
- update-aws-credentials
    - Takes AWS credentials from your clipboard in the format provided by AWS SSO and writes it to
      your AWS config file. This is useful if you're using AWS SSO and you want to use the AWS CLI locally.
    - To install Go version: `go install github.com/timmattison/tools/cmd/update-aws-credentials@latest`
    - To install Rust version: `cargo install --git https://github.com/timmattison/tools update-aws-credentials`
- sf (size of files)
    - Shows you the total size of files in the specified directories (and subdirectories) in a human-readable format. 
      Supports optional filtering by suffix (e.g. `--suffix .mkv`), prefix (e.g. `--prefix IMG_`), or substring 
      (e.g. `--substring G_00`). Without filters, it shows the total size of all files. Doesn't assume suffixes have 
      a period in front of them so you need to include that if you want it.
    - To install: `cargo install --git https://github.com/timmattison/tools sf`
- cf (count files)
    - Recursively counts files in the specified directories. Without filters, counts all files. Supports optional 
      filtering by suffix (e.g. `--suffix .mkv`), prefix (e.g. `--prefix IMG_`), or substring (e.g. `--substring G_00`). 
      The same as doing `find . | wc -l` but shorter and faster. In my testing with a directory with almost 300k files 
      in it this program takes 5 seconds, `find . | wc -l` takes over one minute.
    - To install: `cargo install --git https://github.com/timmattison/tools cf`
- htmlboard
    - Waits for HTML to be put on the clipboard and then pretty prints it and puts it back in the clipboard.
    - To install: `go install github.com/timmattison/tools/cmd/htmlboard@latest`
- jsonboard
    - Waits for JSON to be put on the clipboard and then pretty prints it and puts it back in the clipboard.
    - To install: `go install github.com/timmattison/tools/cmd/jsonboard@latest`
- reposize
    - Shows you the size of the git repository you're currently in. Useful for investigating performance issues with
      large git repos.
    - To install: `go install github.com/timmattison/tools/cmd/reposize@latest`
- bm
    - Bulk Move. Named "bm" because moving lots of files is shitty.
    - To install: `go install github.com/timmattison/tools/cmd/bm@latest`
- repotidy
    - Runs `go mod tidy` on every directory in the current git repo that has a `go.mod`. I wrote this while working on a
      CDK project with multiple Golang functions since I kept having to track down which one needed to be updated.
    - To install: `go install github.com/timmattison/tools/cmd/repotidy@latest`
- localnext
    - Runs statically compiled NextJS applications locally. You'll need to build your code and get the magic `out`
      directory by adding `output: 'export'` to your `next.config.mjs` file. This was written to work
      with [the templates I was testing at the time](https://github.com/timmattison/material-ui-react-templates)
    - To install: `go install github.com/timmattison/tools/cmd/localnext@latest`
- repoup
    - Runs `go get -u all` on every directory in the current git repo that has a `go.mod`. I wrote this while working on
      a
      CDK project with multiple Golang functions since I kept having to track down which one needed to be updated.
    - To install: `go install github.com/timmattison/tools/cmd/repoup@latest`
- unescapeboard
    - Waits for text with `\\"` in it to be put on the clipboard and then unescapes one level of it.
    - To install: `go install github.com/timmattison/tools/cmd/unescapeboard@latest`
- prhash
    - Hashes a file with the requested hashing algorithm and shows the progress in the console. Good for hashing very
      large files and my experiments show that it runs a little bit faster than the standard system tools.
    - To install: `go install github.com/timmattison/tools/cmd/prhash@latest`
- subito
    - Subscribes to a list of topics on AWS IoT Core and prints out the messages it receives. This is useful for
      debugging and testing. I was going to call it `subiot` but `subito` actually means "immediately" in Italian and
      I thought that was cooler. Just run `subito topic1 topic2 topic3 ...` and you'll see the messages.
    - To install: `go install github.com/timmattison/tools/cmd/subito@latest`
- portplz
    - Generates an unprivileged port number based on the name of the current directory. Nice for picking a port number
      for a service that needs to live behind a reverse proxy that also needs to be consistent across deployments and
      separate instances/VMs.
    - To install: `cargo install --git https://github.com/timmattison/tools portplz`
- tubeboard
    - Waits for text that looks like a YouTube video URL to be put on the clipboard and then extracts the video ID from
      it.
      I use this for deep linking videos to my Roku TVs through their APIs.
    - To install: `go install github.com/timmattison/tools/cmd/tubeboard@latest`
- runat
    - Runs a command at a specified time. Shows a countdown timer and supports various time formats including UTC and
      local time.
      You can use full dates like "2024-01-01T12:00:00Z" or just times like "12:00" (which will run today or tomorrow at
      that time).
      Press Ctrl-C to cancel.
    - To install: `go install github.com/timmattison/tools/cmd/runat@latest`
- gitrdun
    - Shows your recent git commits across multiple repositories. Useful for finding what you've been working on
      recently
      across different projects.
    - To install: `go install github.com/timmattison/tools/cmd/gitrdun@latest`
- nodenuke
    - Nukes your node_modules and .next directories and npm and pnpm lock files
    - To install: `go install github.com/timmattison/tools/cmd/nodenuke@latest`
- nodeup
    - Updates your package.json dependencies recursively
    - To install: `go install github.com/timmattison/tools/cmd/nodeup@latest`
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
- wifiqr
    - Generates QR codes for WiFi networks that, when scanned by a mobile device, allow the device to automatically
      connect to the WiFi network without manually entering credentials. Supports custom resolution, adding a logo
      in the center of the QR code, and adjusting the logo size.
    - To install: `cargo install --git https://github.com/timmattison/tools wifiqr`
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
    - To install Go version: `go install github.com/timmattison/tools/cmd/gitdiggin@latest`
    - To install Rust version: `cargo install --git https://github.com/timmattison/tools gitdiggin`
- glo
    - Finds and displays large objects in Git repositories. Useful for identifying files that are bloating your
      repository
      and could be candidates for Git LFS or removal.
    - To install Go version: `go install github.com/timmattison/tools/cmd/glo@latest`
    - To install Rust version: `cargo install --git https://github.com/timmattison/tools glo`
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

## dirhash

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

## prcp

Simply run `prcp <source> <destination>` and you'll see the progress of the copy in the console.

**Available versions:**
- **Go version** (original): Uses Bubble Tea for TUI
- **Rust version** (new): Uses Ratatui for TUI, with automatic fallback to simple progress when no TTY is available

**Features:**
- Progress bar with percentage complete
- Real-time throughput display (MB/s, GB/s, etc.)
- Pause/resume with spacebar
- Ctrl+C to cancel
- Works in both interactive terminals and CI/headless environments

## update-aws-credentials

Just run `update-aws-credentials` (Go version) or `update-aws-credentials` (Rust version) and it will take the AWS
credentials from your clipboard and write them to your AWS config file. If something goes wrong it'll let you know.

## sizeof

Just run `sizeof -suffix .mkv` and you'll see the size of all of the `.mkv` files in the current directory and all
subdirectories. I use it to figure out how large my videos are in a certain directory before trying to move them around.

## runat

Run any command at a specified time. The program shows a countdown timer until execution and supports various time
formats:

```
runat 2024-01-01T12:00:00Z echo hello world    # UTC time
runat 2024-01-01T12:00:00 echo hello world     # Local time
runat "2024-01-01 12:00" echo hello world      # Local time
runat 12:00 echo hello world                   # Today/tomorrow at 12:00 local time
```

If you specify just a time (like "12:00"), it will run today at that time, or if that time has already passed, it will
run tomorrow at that time.

The program shows:

- Current time
- Target time
- Time remaining (hours:minutes:seconds)
- Command to be executed

You can press Ctrl-C at any time to cancel the scheduled execution.

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
