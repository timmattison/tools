# Fun tools written by Tim Mattison

I started this repo forever ago (2014!) to hold some tools I needed at the time. Now I'm converting the tools to Golang
for fun.

## The tools

- dirhash - Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
  identical. This hash will only be the same if the directories have the same file names and the same file contents.
  However, we ignore the directory names and locations of files in the directories. See below for an example.
- prcp - Copies a file and shows the progress in the console. Useful for when you're copying large files and you don't
  want to keep opening a new terminal window to run `du -sh` to see how much has been copied. You can also press the
  space bar to pause the copy and press it again to resume.
- update-aws-credentials - Takes AWS credentials from your clipboard in the format provided by AWS SSO and writes it to
  your AWS config file. This is useful if you're using AWS SSO and you want to use the AWS CLI locally.

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
