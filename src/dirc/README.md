# dirc

- A versatile directory path tool that can both:
    - Copy the current working directory to the clipboard
    - Read a directory path from the clipboard and output a command to change to that directory (`--paste` mode)
- Works best with an alias like `dirp='eval "$(dirc --paste)"'` in your shell configuration. Keep the double
  quotes. Bash and Zsh split the output of an unquoted `$(...)` at every character the field separator holds, so a
  directory name that holds a tab or a newline reaches `cd` with that character turned into a space. In fish the
  alias runs `eval (dirc --paste | string collect)` instead, because fish splits a substitution at every newline.
- The flag is `--paste`, with two dashes. The Go version of this tool also took the single-dash `-paste`, and this
  one does not, so an alias written for the Go version needs the second dash.
- Set `DIRC_CLIPBOARD_FILE` to the path of a file to read and write that file in place of the clipboard of the
  machine. This is what lets the tests of `dirc` run without touching the clipboard of the person at the keyboard.
- In a mosh-rs session, `dirc` copies to the clipboard of your client computer, and `dirc --paste` reads it. Both
  go through the `pbcopy` and `pbpaste` shims that the mosh-rs server puts on `PATH`. A read needs
  `MOSH_ALLOW_PASTE=1` on the client. When paste is off, the shim runs the next `pbpaste` on `PATH`, which is the
  `pbpaste` of the server. Outside mosh-rs, `dirc` uses the clipboard of the machine, and it runs `pbcopy` or
  `pbpaste` from `PATH` when that clipboard does not open.
- To install: `cargo install --git https://github.com/timmattison/tools dirc`
