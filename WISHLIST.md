# Wishlist

This file holds the feature ideas that no tool in this repository gives yet. A line here is
a thing somebody wanted and nobody built. Nothing here is a promise.

## krt (Knights of the Round Trip)

The design of `krt` names these seven as non-goals. Each one is a real feature, and the
design left each one out to keep the first version small.

- **Several destinations in one run.** One run traces one destination today. Several
  destinations need one tracer for each of them, one recorded file for each of them, and one
  table that holds them all.
- **Autonomous System (AS) number lookup.** A hop shows its address and its name. The number
  of the network that owns that address says which operator the hop belongs to, and a path
  that crosses three operators reads differently from a path that stays inside one.
- **File rotation and compression.** A run at the default interval writes about 85 MB per day
  on a 20-hop path, and it writes that file forever. A run that must last for weeks needs the
  file to roll over and the old parts to compress.
- **An alert when a metric crosses a threshold.** A run records a path and draws it. It says
  nothing when the loss of a hop climbs, so somebody must watch the table to see it happen.
- **Path MTU discovery.** A trace reports the hops of the path. It does not report the largest
  packet that the path carries, which is the other thing that a broken path hides.
- **Replay through the live table with playback controls.** `krt replay` prints one table of
  the whole run. A replay that draws the live table can move through the rounds again, with a
  pause, a step, and a seek, so a reader watches the path change.
- **A web view or a remote view.** The table draws under the terminal of the machine that runs
  the trace. A recorded file travels, and a live run does not.
