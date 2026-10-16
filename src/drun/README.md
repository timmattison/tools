# drun

`drun` runs a Docker container on the least busy of your Docker hosts, and
finds the container again later. Use it when this computer is short of memory
and other computers on the network can run the containers.

A host is a Docker context. `drun` works with each host that
`docker --context` can connect to, usually a computer that it connects to
through SSH.

```console
$ drun run -d -p 8080:80 nginx
drun: quirky-otter-4821 runs on box-b (22% busy: 12.1 of 15.6 GiB memory free, load 0.31 on 4 CPUs)
box-b quirky-otter-4821

$ drun status quirky-otter-4821
box-b running

$ drun ps
HOST   NAME               IMAGE  STATUS        PORTS
box-b  quirky-otter-4821  nginx  Up 2 minutes  0.0.0.0:8080->80/tcp
```

## Set up the hosts

Do these steps one time for each host. The examples use a host named `box-a`
and a user named `you`.

1. Install Docker Engine on the host, and add your user to the `docker` group,
   so that `docker` runs without `sudo`. Docker Desktop also works.
2. Make SSH log in with a key, without a password:

   ```sh
   ssh-copy-id you@box-a
   ssh you@box-a true
   ```

   Do the second command one time by hand. `drun` closes stdin, so SSH cannot
   ask you to accept the host key during a run.
3. Make a Docker context for the host, and do a test of it:

   ```sh
   docker context create box-a --docker "host=ssh://you@box-a"
   docker --context box-a info
   ```

4. Write the hosts in `~/.config/drun/config.toml` (or in
   `$XDG_CONFIG_HOME/drun/config.toml`):

   ```toml
   contexts = ["box-a", "box-b"]

   # Optional. How many seconds to wait for each host to answer. The default is 10.
   host_timeout_secs = 10
   ```

   `DRUN_CONTEXTS=box-a,box-b` overrides the `contexts` list for one shell or
   one command. `drun` refuses a key that it does not know, so a typing error
   in the file does not go unseen.
5. Make sure that `drun` can see the hosts:

   ```console
   $ drun hosts
   HOST   MEMORY FREE       LOAD           BUSY
   box-a  3.1 of 15.6 GiB   1.10 / 4 CPUs  80%
   box-b  12.1 of 15.6 GiB  0.31 / 4 CPUs  22%   chosen
   ```

When `drun` chooses a host, it uses only the contexts that you name. It never
puts a container or a builder on a context that you did not name, such as
`default` or a production server. A command that `drun` does not know goes to
the docker of this computer without a change, as
[Other docker commands](#other-docker-commands) tells.

## Reuse SSH connections

Each `docker --context` command opens a new SSH connection. `drun run` makes
two calls to each host before it starts a container. `drun ps`,
`drun buildx create`, and each command that finds a container make one call
to each host. To use one SSH
connection again for ten minutes, add this to `~/.ssh/config`:

```sshconfig
Host box-a box-b
    ControlMaster auto
    ControlPath ~/.ssh/cm-%r@%h:%p
    ControlPersist 10m
```

The Docker CLI runs the `ssh` of the system, so it reads this file.

## Run a container

`drun run` takes the arguments of `docker run` and gives them to `docker run`
on the chosen host without a change.

- **Without `-d`**, `drun` gives the terminal to the container. `-it` and
  Ctrl-C work, and `drun` exits with the exit status of the container, the
  same as a local `docker run`. `drun` writes the host and the name to
  stderr, so the output of the container stays clean.
- **With `-d`**, `drun` prints `<host> <name>` on stdout when the container
  starts, and nothing else. A script can read the two words, wait for the
  result, and get it:

  ```sh
  read -r host name < <(drun run -d -v results:/out alpine sh -c 'sleep 30; echo done > /out/result.txt')
  drun wait "$name"
  drun cp "$name":/out/result.txt .
  drun rm "$name"
  ```

`drun` gives each container a name in the form `adjective-noun-1234` that no
host uses yet. Give your own name with `--name`. Docker makes a name unique on
one host only, so `drun` refuses a `--name` that any host uses. `drun run` also
refuses an empty `--name`, as `--name ""` or `--name /`, because docker then
gives the container a random name that `drun` cannot report. The refusal gives
exit status 2. Each container gets the label `drun.managed=true`, which is how
`drun ps` finds it.

## Find a container again

`drun` keeps no record of which host runs which container. A record on this
computer goes out of date when somebody removes a container directly or a
host restarts. The Docker engines keep the only correct record, so `drun`
asks every host each time.

| Command | What it does |
| --- | --- |
| `drun ps` | Lists the containers that `drun` started, on every host, with a `HOST` column. It includes the containers that stopped. |
| `drun status NAME` | Prints the host and the state, and the exit status when the container stopped: `box-b running`, `box-b exited 0`. |
| `drun logs [options] NAME` | Runs `docker logs` on the host of the container. `-f` works. |
| `drun wait NAME` | Waits until the container stops, prints its exit status, and exits with it. |
| `drun exec [options] NAME COMMAND` | Runs `docker exec` on the host of the container. `-it` works. |
| `drun cp [options] NAME:PATH LOCAL_PATH` | Copies files out of the container. `drun cp [options] LOCAL_PATH NAME:PATH` copies files into it. |
| `drun rm [options] NAME...` | Removes each container on its own host. It removes nothing when one of the names matches no container or more than one container. |

`NAME` is the name of the container, its ID, or the start of its ID. A name or
a full ID that matches wins over the start of an ID, also when they are on
different hosts. As in docker, a `NAME` that starts with `/` is a name and
never an ID, and the one `/` is not part of the name. `NAME` must match exactly one container on all hosts
together:

- When no container matches, `drun` stops with exit status 125 and names each
  host that did not answer.
- When more than one container matches, on one host or on more than one,
  `drun` stops with exit status 125 and names each container and its host, as
  in `one (abc111222333) on box-a, two (abc999888777) on box-b`. Give more of
  the ID, or use `docker --context HOST` directly.

These commands find any container on the hosts, not only the containers that
`drun` started. Only `drun ps` limits the list to the label.

## How drun chooses a host

`drun` starts one small `busybox:1.37` container on each host, all at the
same time. The container prints `/proc/meminfo`, `/proc/loadavg`, and the
count of CPUs, and Docker removes it when it stops. Docker gets the image one
time for each host.

A host is as busy as the larger of two fractions:

- the memory in use: `1 - MemAvailable / MemTotal`
- the one-minute load average for each CPU

`drun` chooses the host with the lowest value. A host with free memory but
CPUs that work at their limit is busy, and so is a host with idle CPUs but no
free memory. When two hosts have the same value, the host with more free
memory wins, and after that the host that is first in the configuration.

On a Linux host, the figures are those of the host. On a host that runs
Docker Desktop, they are those of the virtual machine that runs the
containers, which is the memory that a container can use.

When a host does not answer in `host_timeout_secs`, or answers with an error,
`drun run` writes a warning and chooses between the other hosts. When no host
answers, `drun` stops with exit status 125 and gives the reason for each
host.

## What changes when a container runs on another computer

- **Bind mounts.** `-v ./src:/app` mounts a path on the host, not on this
  computer. `drun run` writes a warning when the source of a bind mount is a
  path on this computer. Write the results to a named volume, and get them
  with `drun cp`.
- **Ports.** `-p 8080:80` opens port 8080 on the host. Connect to
  `box-b:8080`, not to `localhost:8080`.
- **Images.** Each host gets the image itself. Push an image that you build to
  a registry that the hosts can get to. When this computer has Apple silicon
  and a host is x86, build for both:
  `docker buildx build --platform linux/amd64,linux/arm64`. To build on a host
  and not on this computer, see [Build images on a host](#build-images-on-a-host).
- **`--rm` with `-d`.** Docker removes the container when it stops, with its
  exit status and its logs, so `drun status`, `drun logs`, and `drun wait`
  find nothing after that. `drun run` writes a warning. Remove the container
  with `drun rm` after you have the result.
- **Two fast runs.** A new container uses its memory a short time after it
  starts. Two `drun run` commands one after the other can choose the same host,
  because the first container does not show in the figures yet.

## Build images on a host

A buildx builder runs BuildKit on its endpoint, and each build on the builder
runs there. `docker buildx create` uses the current context when you name no
endpoint, so the builds run on this computer. `drun buildx create` puts the
new builder on the least busy host:

```console
$ drun buildx create --name multiarch --driver docker-container --use
drun: the builder multiarch runs on box-b (22% busy: 12.1 of 15.6 GiB memory free, load 0.31 on 4 CPUs)
multiarch

$ docker buildx build --platform linux/amd64,linux/arm64 --tag registry.example/app:1 --push .
```

`drun` adds the chosen host as the endpoint, and runs
`docker buildx create --name multiarch --driver docker-container --use box-b`
on this computer. The docker of this computer keeps the builders. Thus
`--use` selects the builder for the current context of this computer, and
`docker buildx build` on this computer finds it. `drun builder create` does
the same, because docker reads `builder` as `buildx`.

`drun` changes nothing, and asks no host, when one of these is true:

- The arguments name an endpoint, as in `drun buildx create --name x box-a`.
- The driver is not `docker-container`, as in `--driver remote`. The endpoint
  of such a driver is not a Docker host. `docker-container` is the default.
- The arguments remove a node with `--leave`, or ask for the help.
- The first argument is not `buildx` or `builder`. In `drun -D buildx create`,
  the first argument is a docker flag, so the command goes to docker without a
  change.

`drun` chooses the host one time, when it creates the builder. The builder
stays on that host. To choose again, remove the builder and create it again.

CAUTION: `docker buildx rm NAME` DELETES THE BUILD CACHE OF THE BUILDER.

When a build runs on a host, these things change:

- **The build context.** buildx sends the files of the build context to the
  host through SSH for each build. Keep the context small with a
  `.dockerignore` file.
- **Push.** With `--push`, the host sends the image directly to the registry,
  so the host must be able to get to the registry. buildx gives BuildKit the
  credentials of `docker login` on this computer, so log in on this computer.
- **Load.** With `--load`, the image goes into the Docker of the host, not of
  this computer.
- **Other platforms.** The host builds a platform that its CPU cannot run
  through an emulator in its kernel. On a Debian or Ubuntu host, install one
  with `sudo apt install qemu-user-static binfmt-support`. It stays after a
  reboot. `docker buildx inspect --bootstrap NAME` lists the platforms that
  the builder can build.

## Other docker commands

`drun` gives a command that it does not know to the docker of this computer
without a change. A docker flag before the command, as in
`drun --context box-a ps`, goes to docker too. Thus a script can run `drun` in
place of `docker`, and `drun buildx build`, `drun login --password-stdin`, and
`drun --context box-a ps` work as they do in docker. `drun` replaces itself
with docker, so the terminal, stdin, the signals, and the exit status are
those of docker.

- `drun` asks no host for these commands, and adds no label and no name. A
  container that `drun container run` or `drun compose up` starts runs on the
  current context of this computer, and `drun ps` does not show it.
- `run`, `ps`, `logs`, `wait`, `exec`, `cp`, and `rm` are commands of `drun`
  and of docker, and `drun` runs its own command. The two are different. For
  example, `drun ps` takes no flags. For the command of docker, run `docker`.
- `-h`, `--help`, `help`, `-V`, and `--version` give the help and the version
  of `drun`.

## Flags that drun does not know

To find the image in `docker run -h web alpine`, `drun` must know that `-h`
takes a value. It keeps a table of the flags of `docker run`, `logs`, `exec`,
`cp`, and `rm`. It also keeps a table of the flags of `docker buildx` and
`docker buildx create`, to find the command of buildx and the endpoint. When
a new version of Docker adds a flag, `drun` refuses it, because it cannot know
if the next argument is the value of the flag or the image. Write the flag
with its value after `=`, as in `--new-flag=value` or `--new-switch=true`, and
`drun` accepts it. On each buildx command line, `drun` reads the flags of
`docker buildx` before its command, to find the command, and for `create` it
reads the flags of `docker buildx create`. There it refuses an unknown flag,
a flag without its value, and a switch whose value is not `true` or `false`.
It refuses them also when it then gives the arguments to docker without a
change, as in `drun buildx --new-flag x ls`. `drun` reads the arguments of no
other command that it gives to docker without a change, so it refuses no flag
of such a command.

### Find a table that is out of date

Run `drun check-flags` after each update of Docker on this computer. It runs
`docker run --help`, `docker logs --help`, `docker exec --help`,
`docker cp --help`, `docker rm --help`, `docker buildx --help`, and
`docker buildx create --help`, reads the flags from each, and compares them
with the tables. It asks docker for the help without an engine,
so the result does not change with the current context:

```console
$ drun check-flags
The flag tables of drun agree with the docker CLI on this computer.

$ drun check-flags
docker run: docker lists --new-flag, a flag that takes a value, but the table of drun does not have it
drun: the flag tables and docker disagree in 1 places. Update the tables in src/drun/src/docker_args.rs, and copy the new help into src/drun/tests/fixtures/help/
```

It reports five kinds of difference: a flag that docker lists and the table
does not have, a flag that the table lists and docker does not, a flag that
takes a value on one side only, a flag whose one-letter form differs, and a
flag that the table marks as hidden but docker lists. It exits with status 0
when the tables agree and 1 when they do not. When docker gives no help, or
when `drun` cannot read a line of the help, it exits with status 125. It never
reports agreement from help that it did not read.

To update a table after a difference:

1. Change the table in `src/drun/src/docker_args.rs`. Wrap a flag that docker
   accepts but does not list in `hidden(...)`, as for `--net`.
2. Copy the new help into the fixtures, and record the version:

   ```sh
   help=src/drun/tests/fixtures/help
   no_engine() {
       env -i PATH="$PATH" HOME="$HOME" DOCKER_HOST=unix:///dev/null/drun-no-engine.sock \
           "$@" < /dev/null
   }
   for command in run logs exec cp rm buildx; do
       no_engine docker "$command" --help > "$help/$command.txt"
   done
   no_engine docker buildx create --help > "$help/buildx-create.txt"
   { docker --version; docker buildx version; } > "$help/VERSION"
   ```

   The docker CLI hides the flags that the engine behind the current context
   does not support, and the flags that `DOCKER_API_VERSION` excludes. `env -i`
   removes those variables, `DOCKER_HOST` points docker at no engine, and
   `< /dev/null` keeps the wrap width at 80 columns, the same as
   `drun check-flags`.

3. Run `cargo test -p drun`. The test
   `every_table_agrees_with_the_committed_help_of_docker` holds each table to
   the committed help, without docker.

## Exit status

| Status | Meaning |
| --- | --- |
| the status of docker | The docker command ran. For `drun run` without `-d` and for `drun wait`, it is the exit status of the container. For a command that `drun` gives to docker without a change, it is the exit status of docker. The one exception is a buildx command line whose flags `drun` refuses (an unknown flag, a flag without its value, or a switch whose value is not `true` or `false`), which gives status 2. |
| 1 | `drun check-flags` found a difference between the flag tables and docker. |
| 2 | The arguments are wrong. |
| 125 | `drun` stopped before the docker command could run: no hosts are set, no host answered, a NAME matches no container or more than one, a host did not answer `drun ps`, `drun check-flags` could not get or read the help of docker, or the `docker` program did not start. `docker run` gives 125 for the same kind of failure. |

## Install

```sh
cargo install --git https://github.com/timmattison/tools drun
```
