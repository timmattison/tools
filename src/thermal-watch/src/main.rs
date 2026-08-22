//! Command line entrance for `thermal-watch`.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use thermal_watch::dvfs::DvfsTable;
use thermal_watch::load::{performance_core_count, Load};
use thermal_watch::powermetrics::{SampleStream, SAMPLERS};
use thermal_watch::render::sample_line;
use thermal_watch::report::{judge, Outcome, Verdict, BUSY_THRESHOLD_PCT, HOLD_RATIO};

/// How often `powermetrics` reports a sample.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// How long the load runs past the end of the watch, so the last sample is
/// taken under load rather than during the wind-down.
const LOAD_MARGIN: Duration = Duration::from_secs(5);

/// The width of the rule that separates the display from the report.
const RULE_WIDTH: usize = 72;

/// Show whether this Mac decreases its clock under sustained load.
///
/// macOS reports two different signals. The thermal pressure level tells
/// applications to do less work, and stays `Nominal` through most real
/// throttling. The measured P-cluster frequency, against the DVFS table of the
/// chip, is the ground truth. This tool samples both and judges on the second.
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// Make a full P-core load instead of watching a load you started
    #[clap(long)]
    load: bool,

    /// How long to watch, in seconds
    #[clap(long, default_value_t = 300)]
    duration: u64,

    /// Print one JSON object for each sample instead of a live display
    #[clap(long)]
    json: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.duration == 0 {
        anyhow::bail!("--duration needs a positive number of seconds");
    }

    let table = DvfsTable::read().context("cannot read the DVFS table of this machine")?;
    let watch_for = Duration::from_secs(args.duration);
    let sample_count = u32::try_from(args.duration).unwrap_or(u32::MAX);

    if !args.json {
        announce(&table, args.load, args.duration);
    }

    // The load starts first, so the first sample is already taken under load.
    let load = args.load.then(|| {
        Load::start(
            performance_core_count(),
            Instant::now() + watch_for + LOAD_MARGIN,
        )
    });

    let stream = SampleStream::spawn(SAMPLE_INTERVAL, sample_count)
        .context("cannot start powermetrics; it needs root, so run this tool with sudo")?;

    let mut samples = Vec::with_capacity(usize::try_from(args.duration).unwrap_or_default());
    let mut out = io::stdout().lock();
    for sample in stream {
        if args.json {
            writeln!(out, "{}", serde_json::to_string(&sample)?)?;
        } else {
            writeln!(out, "{}", sample_line(&sample, table.p_max()))?;
        }
        out.flush()?;
        samples.push(sample);
    }

    if let Some(load) = load {
        load.stop();
    }

    if !args.json {
        report(&judge(&samples, &table), args.load, &mut out)?;
    }
    Ok(())
}

/// Print what the run is about to do.
fn announce(table: &DvfsTable, generates_load: bool, duration: u64) {
    println!(
        "P-cores: max {} over {} steps   E-cores: max {}",
        table.p_max(),
        table.p_steps().len(),
        table.e_max(),
    );
    println!("Sampling powermetrics ({SAMPLERS}) once a second.");
    if generates_load {
        println!("Making a full P-core load for {duration}s. Press Ctrl-C to stop early.\n");
    } else {
        println!("Watching for {duration}s. Start your load now. Press Ctrl-C to stop early.\n");
    }
}

/// Print the verdict of the run.
fn report(verdict: &Verdict, generated_load: bool, out: &mut impl Write) -> Result<()> {
    let rule = "─".repeat(RULE_WIDTH);
    writeln!(out, "\n{rule}")?;

    if let Outcome::NotEnoughData { busy_samples } = verdict.outcome {
        writeln!(
            out,
            "The P-cluster was busy for {busy_samples} samples, which cannot support a verdict.",
        )?;
        writeln!(
            out,
            "A cluster counts as busy above {BUSY_THRESHOLD_PCT:.0}% active residency."
        )?;
        if generated_load {
            writeln!(out, "\nTwo causes are possible:")?;
            writeln!(
                out,
                "  1. Apple changed the output of powermetrics, and the parser no longer reads it."
            )?;
            writeln!(
                out,
                "     Compare `sudo powermetrics --samplers {SAMPLERS} -n 1` against the parser."
            )?;
            writeln!(out, "  2. The load did not reach the performance cores.")?;
        } else {
            writeln!(
                out,
                "\nStart a real load (a build, a render, a benchmark), then run this tool again."
            )?;
        }
        return Ok(());
    }

    writeln!(out, "Peak clock under load : {}", verdict.peak)?;
    writeln!(out, "Early mean            : {}", verdict.early_mean)?;
    writeln!(
        out,
        "Late mean             : {}  ({:.0}% of the maximum of the chip)",
        verdict.late_mean,
        verdict.late_ratio_of_max * 100.0,
    )?;
    writeln!(
        out,
        "Peak CPU power        : {:.1} W",
        f64::from(verdict.peak_power_mw) / 1_000.0,
    )?;
    writeln!(out, "Worst pressure level  : {:?}", verdict.worst_pressure)?;
    writeln!(out, "{rule}")?;

    match verdict.outcome {
        Outcome::HeldClock => {
            writeln!(
                out,
                "{} no thermal throttling. The clock held near the maximum.",
                "VERDICT:".green().bold(),
            )?;
        }
        Outcome::Throttled { decay } => {
            writeln!(
                out,
                "{} the clock decreased {:.0}% from its early mean, and ends at {:.0}% of the maximum.",
                "VERDICT:".red().bold(),
                decay * 100.0,
                verdict.late_ratio_of_max * 100.0,
            )?;
            writeln!(
                out,
                "This is thermal throttling, or a power limit. Both decrease the clock."
            )?;
            if verdict.worst_pressure == thermal_watch::PressureLevel::Nominal {
                writeln!(
                    out,
                    "{}",
                    "\nThe pressure level stayed Nominal through all of it. That is normal.\n\
                     macOS raises that level to tell applications to do less work, not to\n\
                     report each decrease of the clock."
                        .dimmed(),
                )?;
            }
        }
        Outcome::NeverReachedPeak => {
            writeln!(
                out,
                "{} the clock sat at {:.0}% of the maximum from the first sample on.",
                "VERDICT:".yellow().bold(),
                verdict.late_ratio_of_max * 100.0,
            )?;
            writeln!(
                out,
                "It never decayed, so this is not heat. A clock below {:.0}% of the maximum\n\
                 for a whole run points at another load on the machine.",
                HOLD_RATIO * 100.0,
            )?;
        }
        Outcome::NotEnoughData { .. } => unreachable!("handled above"),
    }
    Ok(())
}
