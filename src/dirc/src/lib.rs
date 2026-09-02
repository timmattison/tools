//! Directory Clipboard.
//!
//! `dirc` moves a directory from one shell to another through the clipboard.
//! One shell copies the directory it is in. The other reads the clipboard and
//! gets the `cd` line that goes there.
//!
//! A tool cannot change the directory of the shell that started it. The shell
//! is the parent of the tool, and a child process cannot write the state of its
//! parent. So `dirc` writes the `cd` line to standard output, and the shell
//! runs that line: `eval $(dirc --paste)`.
//!
//! That `eval` is the reason [`mode::cd_command`] quotes the path. The shell
//! reads the output of `dirc` as a command, so a directory whose name holds a
//! space, a dollar sign, or a quote is a directory whose name holds shell
//! syntax.

pub mod mode;
