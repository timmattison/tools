# cf (count files)

- Recursively counts files in the specified directories. Without filters, counts all files. Supports optional
  filtering by suffix (e.g. `--suffix .mkv`), prefix (e.g. `--prefix IMG_`), or substring (e.g. `--substring G_00`).
  The same as doing `find . | wc -l` but shorter and faster.
- To install: `cargo install --git https://github.com/timmattison/tools cf`
