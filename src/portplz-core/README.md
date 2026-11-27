# portplz-core

A shared Rust library that derives a deterministic, unprivileged TCP port for a
project and a user.

`portplz` prints the port, and `sirn` serves on it. Both tools call this crate,
thus they agree on the port for one project and one user. `sirn` does not need
`portplz` on the machine.

## Usage

```rust
let path = std::env::current_dir()?;
let user = portplz_core::UserSalt::current()?;
let derivation = portplz_core::derive(&path, false, &user, None)?;

println!("{}", derivation.port.get()); // 51877
// Port 51877 for repo 'foo' on branch 'main' (uid 501)
println!("{}", derivation.describe());
```

`derive()` is the one entry point. It hides the SHA-256 hash, the discovery of
the repository through `gix`, and the detection of the user. It gives back a
`Derivation`: the port, the source of the hash input, the user, and the name.

A `DerivedPort` is never less than 1024. Its value is private, and it has no
`Display`, thus a caller reads the number only through `DerivedPort::get`.

## What goes into the port

The hash input comes from the path that the caller gives:

- **A git repository on a branch:** the root name of the repository, the
  current branch, and the user. The root name comes from the shared git
  directory, thus each worktree of one repository gives the same name.
- **A git repository with a detached HEAD:** the root name of the repository
  and the user.
- **No git:** the name of the directory and the user. This source applies when
  the caller sets `no_git`, or when the path is not in a repository.

## The user

The user goes into the hash. Thus two users on one machine get different ports
for one repository and one branch, and they can run the same project side by
side with no collision. On Unix, the user is the numeric user id. On a platform
with no user id, the user is the login name.

`PORTPLZ_UID` overrides the detected user. Set it to a non-negative integer to:

- Get the port of a teammate, for example to reproduce a problem on that port.
- Get a fixed port in a container or in CI, where the user id is not the user
  id of your workstation.
- Get the same port on each deployment, instance, or VM of one service, for
  example behind a reverse proxy.

An empty value, or a value of only whitespace, is the same as no value, and the
detected user applies. Each other value that is not a non-negative integer is an
error (`UserSaltError::InvalidUidOverride`), and not a different port.

## The name

`derive()` also takes an optional name. The name tells one application of a
project apart from another, thus a repository that holds a site and an API gives
each of them its own port. The name is a third component beside the repository
and the branch, and it replaces neither.

The name sits between two NUL bytes at the head of the hash input. No unnamed
input can start with a NUL:

- A user id is decimal digits.
- The crate removes each NUL from a login name, also from a `UserSalt::Name`
  that a caller builds by hand.
- An empty login name puts the `\n` separator first.

Thus a named derivation never hashes the same input as an unnamed one. The
crate also removes each NUL from the name, thus a name cannot close its frame
early.

The name goes in front of the unnamed input, and the unnamed input does not
change. Thus each port that a caller derives with no name keeps the value that
it had before the name came into the crate.

## Used by

- [`portplz`](../../README.md) — prints the port
- [`sirn`](../../README.md) — serves on the port
