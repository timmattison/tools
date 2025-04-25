# Fun tools written by Tim Mattison

I started this repo forever ago (2014!) to hold some tools I needed at the time. Now I'm converting the tools to Golang
for fun.

## The tools

- dirhash
    - Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
      identical. This hash will only be the same if the directories have the same file names and the same file contents.
      However, we ignore the directory names and locations of files in the directories. See below for an example.
    - To install: `go install github.com/timmattison/tools/cmd/dirhash@latest`
- prcp
    - Copies a file and shows the progress in the console. Useful for when you're copying large files and you don't
      want to keep opening a new terminal window to run `du -sh` to see how much has been copied. You can also press the
      space bar to pause the copy and press it again to resume.
    - To install: `go install github.com/timmattison/tools/cmd/prcp@latest`
- prgz
    - Similar to `prcp` but instead of copying a file it gzip compresses it. It shows the progress in the console.
    - To install: `go install github.com/timmattison/tools/cmd/prgz@latest`
- update-aws-credentials
    - Takes AWS credentials from your clipboard in the format provided by AWS SSO and writes it to
      your AWS config file. This is useful if you're using AWS SSO and you want to use the AWS CLI locally.
    - To install: `go install github.com/timmattison/tools/cmd/update-aws-credentials@latest`
- sizeof
    - Shows you the size of files in the specified directories (and subdirectories) in a human-readable format. Supports
      searching for files with a specific suffix (e.g. `.mkv`), prefix (e.g. `IMG_`), or a substring (e.g. `G_00`). It
      doesn't support any other form of wildcards. It doesn't assume suffixes have a period in front of them so you need
      to include that if you want it.
    - To install: `go install github.com/timmattison/tools/cmd/sizeof@latest`
- numberof
    - Shows you the number of files in the specified directories (and subdirectories) in a human-readable format. Supports
      searching for files with a specific suffix (e.g. `.mkv`), prefix (e.g. `IMG_`), or a substring (e.g. `G_00`). It
      doesn't support any other form of wildcards. It doesn't assume suffixes have a period in front of them so you need
      to include that if you want it.
    - To install: `go install github.com/timmattison/tools/cmd/numberof@latest`
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
- cf
    - Recursively counts files in the current directory. The same as doing `find . | wc -l` but shorter and faster. In
      my testing with a directory with almost 300k files in it this program takes 5 seconds, `find . | wc -l` takes over
      one minute, `dust` takes 10 seconds (but arguably it is doing something different).
    - To install: `go install github.com/timmattison/tools/cmd/cf@latest`
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
  - To install: `go install github.com/timmattison/tools/cmd/portplz@latest`
- tubeboard
    - Waits for text that looks like a YouTube video URL to be put on the clipboard and then extracts the video ID from it.
      I use this for deep linking videos to my Roku TVs through their APIs.
  - To install: `go install github.com/timmattison/tools/cmd/tubeboard@latest`
- runat
    - Runs a command at a specified time. Shows a countdown timer and supports various time formats including UTC and local time.
      You can use full dates like "2024-01-01T12:00:00Z" or just times like "12:00" (which will run today or tomorrow at that time).
      Press Ctrl-C to cancel.
    - To install: `go install github.com/timmattison/tools/cmd/runat@latest`
- gitrdun
    - Shows your recent git commits across multiple repositories. Useful for finding what you've been working on recently
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
    - Searches for a hex string in a binary file and displays a hex dump with surrounding bytes. Shows the offset in both 
      hex and decimal formats. Useful for analyzing binary files and finding specific patterns or signatures.
    - To install: `go install github.com/timmattison/tools/cmd/hexfind@latest`
- wifiqr
    - Generates QR codes for WiFi networks that, when scanned by a mobile device, allow the device to automatically 
      connect to the WiFi network without manually entering credentials. Supports custom resolution, adding a logo 
      in the center of the QR code, and adjusting the logo size.
    - To install: `go install github.com/timmattison/tools/cmd/wifiqr@latest`

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

## update-aws-credentials

Just run `update-aws-credentials` and it will take the AWS credentials from your clipboard and write them to your AWS
config file. If something goes wrong it'll let you know.

## sizeof

Just run `sizeof -suffix .mkv` and you'll see the size of all of the `.mkv` files in the current directory and all
subdirectories. I use it to figure out how large my videos are in a certain directory before trying to move them around.

## runat

Run any command at a specified time. The program shows a countdown timer until execution and supports various time formats:

```
runat 2024-01-01T12:00:00Z echo hello world    # UTC time
runat 2024-01-01T12:00:00 echo hello world     # Local time
runat "2024-01-01 12:00" echo hello world      # Local time
runat 12:00 echo hello world                   # Today/tomorrow at 12:00 local time
```

If you specify just a time (like "12:00"), it will run today at that time, or if that time has already passed, it will run tomorrow at that time.

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

When scanned with a smartphone camera, these QR codes will prompt the device to join the specified WiFi network automatically.
