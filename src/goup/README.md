# goup

- Updates Go dependencies in a git repository. Automatically finds all go.mod files and updates
  dependencies. Supports `--update` flag to use `go get -u all` for latest versions, otherwise
  uses `go mod tidy` for cleanup.
- To install: `cargo install --git https://github.com/timmattison/tools goup`
