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
    - Shows you the size of files in the current directory (and subdirectories) in a human-readable format. Supports
      searching for files with a specific suffix (e.g. `.mkv`), prefix (e.g. `IMG_`), or a substring (e.g. `G_00`). It
      doesn't support any other form of wildcards. It doesn't assume suffixes have a period in front of them so you need
      to include that if you want it.
    - To install: `go install github.com/timmattison/tools/cmd/sizeof@latest`
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
