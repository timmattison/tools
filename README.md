# Fun tools written by Tim Mattison

I started this repo forever ago (2014!) to hold some tools I needed at the time. Now I'm converting the tools to Golang
for fun.

## The tools

- dirhash - Gets a SHA256 hash of a directory tree. This is useful for comparing two directories to see if they are
  identical. This hash will only be the same if the directories have the same file names and the same file contents.
  However, we ignore the directory names and locations of files in the directories. See below for an example.

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
