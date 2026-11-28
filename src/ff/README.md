# ff (find files)

`ff` finds files by name across a whole machine. It answers the question that
`find / -iname` answers, and it does not lose the scan.

BSD `find` ends the complete walk when a system call under `fts_read` returns
`EINTR`:

```
find / -iname "120.mp4" | tee other-locations.txt
find: fts_read: Interrupted system call
```

Each path that `find` did not print yet is lost, and the interruption arrives
at a time that nobody can predict. `ff` reads each directory with its own
calls. When a call returns `EINTR`, `ff` does the same call again at the same
position, and the walk continues.

## Usage

```bash
ff 120.mp4                 # each mounted volume that holds user files
ff '*.mp4' ~/Movies /Volumes/External
ff -e '^IMG_\d{4}\.HEIC$' ~/Pictures
ff -0 '*.log' /var/log | xargs -0 ls -l
```

- `<pattern>` is a glob that must match the whole file name. It ignores case.
  A glob that holds a `/` is refused, because a file name holds no `/`.
- `[paths...]` are the directories to search. With no path, `ff` searches the
  whole machine (macOS only). On a different platform, name each path.
- `-s`, `--case-sensitive` matches the case of each letter.
- `-e`, `--regex` takes a regular expression in place of a glob. It matches
  anywhere in the name. Use `^` and `$` to match the whole name.
- `-x`, `--one-file-system` stops at each mount point.
- `--include-cloud` also searches the File Provider directories.
- `--show-skipped` names each subtree that `ff` could not read, and each
  directory and volume that it skipped.
- `-0`, `--print0` ends each match with a NUL, for `xargs -0`.

`ff` prints each match when it finds it. It does not hold the results to the
end. It does not follow a symbolic link below a path, but it follows a path
that you name. It matches the name of each path that you name too, as `find`
does.

## The summary and the exit status

At the end, `ff` writes a summary to standard error:

```
ff: matches: 2
ff: entries read: 7561234 in 81.4 s
ff: volumes searched: 5, skipped: 13
ff: incomplete subtrees: 0
ff: denied directories: 412 (System Integrity Protection and the privacy settings refuse some directories)
ff: cloud directories skipped: 4 (--include-cloud searches them)
ff: --show-skipped names each one
```

A search that returns fewer results than the truth, and says nothing, is worse
than a search that fails. Thus `ff` counts each subtree that it could not read
completely, and the summary always gives that count.

- **Exit status 0**: `ff` read each subtree to its end.
- **Exit status 1**: `ff` could not read one or more subtrees. `--show-skipped`
  names each one, with its error.
- **Exit status 2**: `ff` could not start, for example because of a bad
  pattern, or it could not write a match.

A directory below a path that the system refuses (`EACCES` or `EPERM`) is not
incomplete. System Integrity Protection and the privacy settings of macOS
refuse some directories on each Mac, thus `ff` counts them and does not change
the exit status. But a path that you name and that the system refuses is
incomplete, because `ff` read nothing there. A directory that goes away during
the walk is not a failure either.

A reader that stops early, for example `head`, stops the walk. `ff` then exits
0 and writes no summary.

## A search of the whole machine (macOS)

With no path, `ff` gets the volumes from the mount table, and it walks each
volume that holds user files one time. Each walk stays on the device of its
volume, and it does not enter another mount point.

- **The data volume, one time.** `/System/Volumes/Data` has the same device id
  as `/`, thus a device check does not stop a walk of `/` there. A plain walk
  of `/` reads the home directories two times: through `/Users`, and through
  `/System/Volumes/Data/Users`. `ff` reads `/usr/share/firmlinks` at each run.
  It walks each firmlinked directory through its firmlink (`/Users`), and the
  rest of the data volume directly. The directories of the data volume that
  have no firmlink, for example `/System/Volumes/Data/mnt`, stay in the walk.
- **Skipped volumes.** `devfs`, each `autofs` map (for example
  `/System/Volumes/Data/home`), the read-only simulator images under
  `/Library/Developer/CoreSimulator`, the system volumes
  `/System/Volumes/{VM,Preboot,Update,xarts,iSCPreboot,Hardware}`, and each
  volume inside one of these. `--show-skipped` names each one with its reason.
- **The File Provider directories.** Each directory in
  `~/Library/CloudStorage` (Google Drive, Proton Drive, Dropbox, and others)
  and `~/Library/Mobile Documents` (iCloud Drive) answers `readdir` over the
  network. `ff` skips them unless you give `--include-cloud`, and the summary
  gives their count. A path that you name is searched also when it is such a
  directory. This rule applies to a search of named paths too. `ff` decides
  this rule on the real path, thus `ff x .` in `~/Library` skips them too.

`ff` does not use Spotlight. `mdfind` returns nothing when indexing is off on
a volume, and it says nothing about that.

## How the retry works

`std::fs::read_dir` cannot do this job: after a read error, its stream ends,
thus a caller cannot read the same position again. `walkdir` and `ignore` read
through it. `ff` reads with `open`, `fdopendir`, `readdir`, `fstat`, and
`fstatat` through `libc`. An interrupted call did nothing, thus the same call
reads the same position again. The walk counts each retry, and the summary
gives the count. One call that is interrupted 1000 times in a row makes its
subtree incomplete, thus a stream of signals cannot make `ff` hang.

To install: `cargo install --git https://github.com/timmattison/tools ff`
