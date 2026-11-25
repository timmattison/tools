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

## msgs (iMessage browser)

The design of `msgs` described these features, and the first version does not have them.

- **Inline display of image attachments.** The message view shows each attachment as a line
  with its name and size. A view that shows the image needs the Tauri asset protocol with a
  scope limited to `~/Library/Messages/Attachments`, and a command that gives the frontend
  the URL of one attachment after a path check.
- **Progress events for a large export.** The export runs with no progress report, and the
  export bar shows no count. An `export-progress` event from the backend, as `cache-status`
  does for the search index, lets the bar show the files copied so far.
- **A manual rebuild of the search index.** The index builds once at each start, thus a
  message that arrives later is not in the index until the next start. There is no way to
  force a rebuild during a session. A command and a button make a rebuild possible, with
  the same `cache-status` events.
- **Search in one chat.** The `search_messages` command takes a chat id, and the UI always
  sends none. A scope control in the chat header and the `Cmd+Shift+F` shortcut make a
  search inside the open conversation possible.
- **Search results grouped by conversation.** The results are one flat list of messages.
  A view that groups them under the conversation name, with the surrounding messages,
  helps a reader who looks for one thread.
- **The service (iMessage or SMS) in the chat header.** The header shows the name and the
  identifier of the chat. The `chat` table holds the service, and a command has to pass it
  to the frontend.
- **An About window.** The version shows only for `msgs --version`. A native menu with an
  About window shows the version string with the git hash and the dirty status.
- **A list of the skipped attachments in the export summary.** The export skips an attachment
  whose file is missing or outside the attachments root, and the summary counts only the
  copied files. A list of the skipped names in the export bar tells the user what is missing.
- **A placeholder for a message whose text cannot be read.** The parser returns no text for
  an unknown blob format, and the bubble then shows no text. A placeholder such as
  "[unable to read message]" shows that the message has content that msgs cannot read.
