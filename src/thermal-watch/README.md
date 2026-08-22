# thermal-watch

Show whether an Apple Silicon Mac decreases its clock under sustained load.

## The question this answers

Every process-level monitor on macOS reports a **thermal pressure level** —
`Nominal`, `Fair`, `Serious`, or `Critical`. It is easy to read that as "the
chip is at full speed", and it is not what the level means. macOS raises the
level to tell *applications* to do less work, and Apple Silicon decreases its
clock long before the level ever leaves `Nominal`.

So a machine can report `Nominal` while its P-cores run 20% below their peak
clock, and no pressure-based tool says a word about it.

`thermal-watch` measures the other signal: the **achieved P-cluster frequency**,
against the DVFS table of the chip. That is the ground truth, and it is what the
verdict is built on.

## What it does

1. Reads the DVFS table of the running chip from the IO Registry, which needs no
   special privilege. The largest P-cluster step is the peak clock of the chip.
2. Runs `powermetrics` once a second and reads the achieved clock, the active
   residency, the CPU power, and the thermal pressure level from each sample.
3. Optionally makes its own full P-core load, so no separate build is necessary.
4. Compares the mean clock at the start of the load against the mean clock at
   the end of it. That decay is the throttling. Each window is one third of the
   run or less, so the two windows never share a sample. The early window is
   the full 20 seconds on a run of 60 seconds or more. The late window is the
   full 60 seconds on a run of 180 seconds or more.

## Usage

`powermetrics` needs root, so this tool does too. It never runs `sudo` itself.

```bash
sudo thermal-watch --load                 # make a 5-minute load and watch it
sudo thermal-watch --load --duration 900  # 15 minutes, the interesting case
sudo thermal-watch                        # watch a build you started
sudo thermal-watch --json                 # one object for each sample, then the verdict
```

| Option | What it does |
| --- | --- |
| `--load` | Make a full P-core load instead of watching one you started. |
| `--duration <SECONDS>` | How long to watch. The default is 300. The maximum is 86400, which is one day. |
| `--json` | Print one JSON object for each sample, and then one final object that carries the verdict, instead of the live display. |

## Reading the output

Both modes end with the verdict. The live display writes it as a report, and
the `--json` mode writes it as one final object.

Each line of the live display carries the time from the start, a bar of the
clock against the peak of the chip, the clock itself, how busy the P-cluster
was, the CPU power, and the thermal pressure level.

```text
P-cores: max 4.51 GHz over 19 steps   E-cores: max 2.59 GHz
Sampling powermetrics (cpu_power,thermal) once a second.
Making a full P-core load for 900s. Press Ctrl-C to stop early.

00:07  ████████████████████████ 4.51 GHz (100% of max)  busy  99.9%  cpu  38.2W  Nominal
04:31  ██████████████████▎      3.44 GHz ( 76% of max)  busy  99.9%  cpu  27.1W  Nominal
```

The run ends with one of four verdicts. Each mode names them in its own way:
the report prints the name in the first column, and the JSON `outcome` key
holds the name in the second.

| Verdict | In JSON | What it means |
| --- | --- | --- |
| `HeldClock` | `held_clock` | The clock held near the peak for the whole run. No throttling. |
| `Throttled` | `throttled` | The clock started near the peak and then decreased. This is thermal throttling, or a power limit. |
| `NeverReachedPeak` | `never_reached_peak` | The clock was low from the first sample on. It never decayed, so this is not heat — look for another load on the machine. |
| `NotEnoughData` | `not_enough_data` | The P-cluster was never busy for long enough to judge. |

A `Throttled` verdict beside a `Nominal` pressure level is the normal case, not
a contradiction. The report says so.

### The JSON mode

`--json` prints the same run as line-delimited JSON. Each sample is one object.
The last line is one more object, and it carries the verdict of the run.

```text
{"at_seconds":7.0,"p_freq":4500,"p_active_pct":99.9,"e_freq":1020,"cpu_power_mw":38200,"gpu_power_mw":0,"pressure":"nominal"}
{"verdict":{"outcome":"throttled","decay":0.244,"peak":4500,"early_mean":4500,"late_mean":3400,"late_ratio_of_max":0.754,"peak_power_mw":48500,"worst_pressure":"nominal"}}
```

A reader tells the two kinds of line apart by one key: a sample carries
`at_seconds` and no `verdict`, and the verdict carries `verdict` and no
`at_seconds`.

The data of the outcome sits beside `outcome`, not under it. `throttled` adds
`decay`, which is the share of the early mean that was lost. `not_enough_data`
adds `busy_samples`, which is the count of busy samples the run collected. The
other two outcomes add nothing.

The outcome also decides which measurements the line carries. A
`not_enough_data` run measured no clock at all, so its line leaves out `peak`,
`early_mean`, `late_mean` and `late_ratio_of_max`. A zero in those keys would
read as a measurement, and an absent key cannot. The line keeps
`peak_power_mw` and `worst_pressure`, because the run measured both of them
even though it could not support a verdict.

```text
{"verdict":{"outcome":"not_enough_data","busy_samples":3,"peak_power_mw":48500,"worst_pressure":"nominal"}}
```

The other three outcomes carry every key in the table below.

| Key | On which outcomes | What it holds |
| --- | --- | --- |
| `outcome` | all four | The name of the verdict, in lower snake case. |
| `decay` | `throttled` | The share of the early mean that was lost. |
| `busy_samples` | `not_enough_data` | The count of busy samples the run collected. |
| `peak` | the three judged ones | The peak clock under load, in megahertz. |
| `early_mean` | the three judged ones | The mean clock over the early window, in megahertz. |
| `late_mean` | the three judged ones | The mean clock over the late window, in megahertz. |
| `late_ratio_of_max` | the three judged ones | The late mean as a share of the peak of the chip. |
| `peak_power_mw` | all four | The highest CPU package power, in milliwatts. |
| `worst_pressure` | all four | The worst thermal pressure level of the run. |

## Why the IO Registry, and not `ioreg`

The command line tool `ioreg` renders the DVFS tables as hexadecimal inside a
wall of text, so reading them that way means a regular expression over the
output of another program. "What does this property of this registry node hold"
names a structure rather than a piece of text, so this crate asks IOKit itself.

The `powermetrics` output is a different case. It is line-oriented, prints no
machine-readable form that carries the same fields, and every field wanted here
is a labelled line — so that half is a line scan, which is the correct
instrument for a genuinely lexical format.

## Why the load generator cannot outlive the process

A load generator whose only stop is a call after the loop never stops when its
caller dies. Each worker here carries its own deadline and checks it, so the
load ends on time even when nothing calls `stop`. The workers are threads rather
than processes, so they also end when the process ends. Neither guarantee
depends on the other, and `load_bounds.rs` holds both.

## Installation

```bash
cargo install --git https://github.com/timmattison/tools thermal-watch
```
