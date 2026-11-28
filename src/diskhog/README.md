# diskhog

- Shows per-process disk I/O usage on macOS in a continuously updating terminal UI. Displays disk bandwidth
  (read/write bytes per second) for all processes. When run with sudo, also shows IOPS (operations per second)
  using fs_usage. Features include configurable refresh rate, process count limits, and keyboard controls (q/Esc to quit).
- To install: `cargo install --git https://github.com/timmattison/tools diskhog`
