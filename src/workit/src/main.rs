use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(version = version_string!(), about = "Find all Cargo.toml files and add them to a workspace", long_about = None)]
struct Args {
    #[arg(
        short,
        long,
        help = "Path to search for Cargo.toml files",
        default_value = "."
    )]
    path: PathBuf,

    #[arg(
        short,
        long,
        help = "Output file for workspace Cargo.toml [default: <path>/Cargo.toml]"
    )]
    output: Option<PathBuf>,

    #[arg(
        short,
        long,
        help = "Dry run - show what would be done without making changes"
    )]
    dry_run: bool,

    #[arg(
        short,
        long,
        help = "Exclude directories with these exact names, at any depth below the search path (in addition to defaults: target, node_modules)"
    )]
    exclude: Vec<String>,

    #[arg(
        short = 'P',
        long,
        help = "Prefix to add to member paths (e.g., 'src/'); a package the manifest directory does not hold is named in full and takes no prefix"
    )]
    prefix: Option<String>,

    #[arg(
        long,
        help = "Include packages in a git worktree below the search path (excluded by default; where the search path itself sits is never matched)"
    )]
    include_worktrees: bool,

    #[arg(long, help = "Disable default exclusions (target, node_modules)")]
    no_default_excludes: bool,
}

fn get_default_excludes() -> Vec<String> {
    vec!["target".to_string(), "node_modules".to_string()]
}

/// Whether the package whose manifest is `manifest` sits in a git worktree
/// that lies below `root`.
///
/// `--include-worktrees` answers one question: this tree holds a second
/// checkout of packages the repository already has, so do not list them twice.
/// That is a statement about the directories *below* the search path, so the
/// walk up stops at `root` and never asks where `root` itself sits.
///
/// It used to walk on to the root of the file system, which asked the opposite
/// question — is the search path inside a worktree — and answered it for every
/// package at once. A search path inside a worktree therefore lost all of them,
/// and the run reported an empty tree. That is the same defect the exclusion
/// matching in `find_cargo_tomls` carried, in the same place: a filter meant to
/// judge what is under the root ended up judging the root.
///
/// A `.git` *directory* is a repository of its own rather than a worktree. A
/// package under one is a package nothing else holds, so it is listed, and the
/// walk stops there: what lies above a repository says nothing about what is
/// inside it.
fn is_in_worktree(manifest: &Path, root: &Path) -> bool {
    let Some(package) = manifest.parent() else {
        return false;
    };

    for directory in package.ancestors() {
        // The search path is where the walk ends. Comparing paths rather than
        // subtracting them keeps a relative `--path` working: `WalkDir` hands
        // back every path built onto the root exactly as it was given, and
        // `Path` compares by component, so `.` and `./` both stop here.
        if directory == root {
            return false;
        }

        // A directory the root does not hold cannot be reached by walking up
        // from a path under the root, so this is unreachable in practice. It is
        // the second half of the stop condition all the same: an equality that
        // never matches would otherwise walk to the root of the file system,
        // which is the behaviour being removed.
        if directory.strip_prefix(root).is_err() {
            return false;
        }

        let git_path = directory.join(".git");
        if git_path.is_file() {
            // A linked worktree keeps a file naming the directory it borrows.
            if let Ok(content) = fs::read_to_string(&git_path) {
                return content.trim().starts_with("gitdir:");
            }
            return false;
        }
        if git_path.is_dir() {
            return false; // A repository of its own, not a worktree.
        }
    }

    false
}

/// What one walk of the tree found.
///
/// The two counts answer different questions and an empty result needs both:
/// packages the scan found and a count of the ones the worktree filter took
/// back out. A run that found nothing because everything it found was filtered
/// must not read like a run over a tree that holds no package - the second one
/// is an answer, and the first is a flag the user has not been told about.
///
/// Entries the scan could not read are counted separately again, by `Skipped`,
/// because those decide the exit status and these do not: a package left out on
/// purpose is not a package the scan failed to read.
struct Scan {
    /// The package manifests to write into the workspace, sorted.
    packages: Vec<PathBuf>,
    /// How many manifests the worktree filter left out.
    worktrees: usize,
}

impl Scan {
    /// Says why the scan found nothing, when the reason is the worktree filter.
    ///
    /// Prints nothing when the filter left nothing out: a tree that holds no
    /// package at all is not a filtering problem, and naming a flag that would
    /// change nothing sends the reader the wrong way.
    fn explain_empty_result(&self, root: &Path) {
        if self.worktrees == 0 {
            return;
        }

        let files = if self.worktrees == 1 {
            "Cargo.toml file"
        } else {
            "Cargo.toml files"
        };
        println!(
            "{} {files} below {} {} left out for being in a git worktree. \
             Pass --include-worktrees to list them.",
            self.worktrees,
            root.display(),
            if self.worktrees == 1 { "was" } else { "were" },
        );
    }
}

/// The entries a scan gave up on.
///
/// A tree of any size holds things a run cannot read: a directory whose mode
/// keeps it out, a `Cargo.toml` that nobody can parse. Each of those is a
/// reason to skip that entry, not a reason to abandon the tree around it. One
/// of them used to end the walk, so a single unreadable entry threw away every
/// package the scan had already found and said nothing about them — which a
/// scan of a home directory or of a shared tree meets routinely. Each is now
/// named on stderr as it is met and counted here, and the total is stated once
/// at the end.
///
/// Every reason names the path it is about: `walkdir` says so in its own
/// message, and every failure to read a manifest carries the path in its
/// context. So the warning states the path once, rather than repeating it.
#[derive(Default)]
struct Skipped {
    count: usize,
}

impl Skipped {
    /// Names one entry the scan gave up on, and counts it.
    fn skip(&mut self, reason: &dyn fmt::Display) {
        eprintln!("Warning: {reason:#}");
        self.count += 1;
    }

    /// States the total once, and says whether the scan answered the question.
    ///
    /// The summary goes to stderr, because standard output carries the manifest
    /// under `--dry-run`.
    ///
    /// A scan that skipped something and still found packages succeeded: those
    /// packages are the answer, and what it skipped is named above them. A scan
    /// that skipped something and found nothing never read the tree it was
    /// pointed at, so it fails instead of reporting an empty tree — the two are
    /// different answers, and only one of them is success.
    fn report(&self, found: usize) -> Result<()> {
        if self.count == 0 {
            return Ok(());
        }

        let entries = if self.count == 1 { "entry" } else { "entries" };
        eprintln!("Skipped {} {entries} that could not be read.", self.count);

        if found == 0 {
            return Err(anyhow::anyhow!(
                "Found no packages, and {} {entries} could not be read",
                self.count
            ));
        }

        Ok(())
    }
}

fn find_cargo_tomls(
    root: &Path,
    excludes: &[String],
    include_worktrees: bool,
    skipped: &mut Skipped,
) -> Scan {
    let mut cargo_files = Vec::new();
    let mut worktrees = 0;

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let path = e.path();

            // Skip hidden directories
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with('.') && path != root {
                    return false;
                }
            }

            // Skip excluded directories. An exclusion names a directory, so it
            // is matched against whole components rather than against the path
            // as text: `targets` and `node_modules_backup` are directories of
            // their own and are not `target` or `node_modules`.
            //
            // Only the components below the search root are matched. WalkDir
            // hands the root itself to this filter first, and the user asked
            // for that root by name, so a word in its own path says nothing
            // about what is under it — a root named `targeting`, or an
            // explicit `--path /home/tim/target/myproj`, is searched normally.
            let below_root = path.strip_prefix(root).unwrap_or_else(|_| Path::new(""));
            for component in below_root.components() {
                let name = component.as_os_str();
                if excludes
                    .iter()
                    .any(|exclude| name == std::ffi::OsStr::new(exclude))
                {
                    return false;
                }
            }

            true
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                skipped.skip(&error);
                continue;
            }
        };
        let path = entry.path();

        if path.file_name() == Some("Cargo.toml".as_ref()) && path != root.join("Cargo.toml") {
            // Skip a second checkout of packages the repository already holds,
            // unless the run asked for it. Counted, so a scan the filter
            // emptied can say so rather than reporting a tree with nothing in
            // it.
            if !include_worktrees && is_in_worktree(path, root) {
                worktrees += 1;
                continue;
            }

            // Check if it's a package (not already a workspace)
            match is_package_toml(path) {
                Ok(true) => cargo_files.push(path.to_path_buf()),
                Ok(false) => {}
                Err(error) => skipped.skip(&error),
            }
        }
    }

    cargo_files.sort();
    Scan {
        packages: cargo_files,
        worktrees,
    }
}

/// Whether `path` holds a package rather than a workspace.
///
/// # Errors
///
/// Answers the failure to read the file or to parse it, naming the file in
/// both. The caller reports that failure beside every other entry it skipped,
/// and a message that names no file leaves the reader to find it in a tree of
/// thousands: an unparsable manifest used to end the whole scan with nothing
/// but `TOML parse error at line 1, column 6` to say which file it had read.
fn is_package_toml(path: &Path) -> Result<bool> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let doc = content
        .parse::<DocumentMut>()
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    // It's a package if it has [package] but not [workspace]
    Ok(doc.get("package").is_some() && doc.get("workspace").is_none())
}

fn get_package_name(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    let doc = content.parse::<DocumentMut>()?;

    if let Some(package) = doc.get("package") {
        if let Some(name) = package.get("name") {
            if let Some(name_str) = name.as_str() {
                return Ok(name_str.to_string());
            }
        }
    }

    Err(anyhow::anyhow!(
        "No package name found in {}",
        path.display()
    ))
}

/// The directory every member path is measured from: the directory that holds
/// the manifest being written.
///
/// Cargo resolves a member against the manifest that lists it, so the manifest
/// decides what a member means — not the tree the walk happened to start at.
/// The two are the same directory on a default run and part company as soon as
/// `--output` names a file somewhere else.
fn manifest_directory(output: &Path) -> Result<PathBuf> {
    // `Path::parent` answers `Some("")` for a bare file name, which names no
    // directory. The file lands in the directory the run was started from, so
    // that is the directory its members are measured from.
    let directory = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    directory.canonicalize().with_context(|| {
        format!(
            "Failed to resolve {}, the directory that would hold {}",
            directory.display(),
            output.display()
        )
    })
}

/// The member path for the package directory `package`, as the manifest in
/// `manifest_directory` lists it.
///
/// Both sides of the subtraction are canonical, so it is exact whether the run
/// named its tree relatively or absolutely — a relative `--path` used to leave
/// a `./` on the front of every member, because an absolute root cannot be
/// taken off a relative path.
///
/// A package the manifest directory does not hold cannot be named relative to
/// it, which an explicit `--output` makes possible. Such a package is named in
/// full, and the prefix does not apply to it: a prefix names a directory under
/// the manifest, and an absolute path is under nothing.
fn member_path(package: &Path, manifest_directory: &Path, prefix: Option<&str>) -> Result<String> {
    let package = package
        .canonicalize()
        .with_context(|| format!("Failed to resolve {}", package.display()))?;

    match package.strip_prefix(manifest_directory) {
        Ok(relative) => {
            let member = relative.to_string_lossy().replace('\\', "/");
            Ok(match prefix {
                Some(prefix) => format!("{prefix}{member}"),
                None => member,
            })
        }
        Err(_) => Ok(package.to_string_lossy().replace('\\', "/")),
    }
}

fn create_or_update_workspace(output: &Path, members: &[String], dry_run: bool) -> Result<()> {
    let mut doc = if output.exists() {
        let content = fs::read_to_string(output)
            .with_context(|| format!("Failed to read existing {}", output.display()))?;
        content
            .parse::<DocumentMut>()
            .with_context(|| format!("Failed to parse existing {}", output.display()))?
    } else {
        DocumentMut::new()
    };

    // Ensure [workspace] section exists
    if doc.get("workspace").is_none() {
        doc["workspace"] = Item::Table(Table::new());
    }

    // Get or create members array
    let workspace = doc["workspace"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("Failed to create workspace table"))?;

    if workspace.get("members").is_none() {
        workspace["members"] = Item::Value(toml_edit::Value::Array(Array::new()));
    }

    let members_array = workspace["members"]
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("Failed to create members array"))?;

    // The members list is rewritten whole, so every member the scan did not
    // find goes. Name them first: a manifest that quietly loses a member reads
    // afterwards like a manifest nobody touched.
    let mut dropped = Vec::new();
    for existing in members_array.iter().filter_map(toml_edit::Value::as_str) {
        if !members.iter().any(|member| member.as_str() == existing) {
            dropped.push(existing.to_string());
        }
    }

    if !dropped.is_empty() {
        eprintln!(
            "Dropping {} member(s) of {} that the scan did not find:",
            dropped.len(),
            output.display()
        );
        for member in &dropped {
            eprintln!("  - {member}");
        }
    }

    // Clear existing members and add new ones
    members_array.clear();
    for member in members {
        members_array.push(member);
    }

    // Add common workspace configuration if not present
    if workspace.get("resolver").is_none() {
        workspace["resolver"] = toml_edit::value("2");
    }

    if dry_run {
        println!("Would write to {}:", output.display());
        println!("{}", doc);
    } else {
        fs::write(output, doc.to_string())
            .with_context(|| format!("Failed to write {}", output.display()))?;
        println!("Created/updated workspace at {}", output.display());
    }

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // The manifest lands beside the tree that was scanned unless the run named
    // a file of its own. It used to land in the directory the run was started
    // from whatever tree it read, so `workit --path <elsewhere>` rewrote the
    // members of a manifest it had not looked at, naming packages that
    // directory does not hold. Clap cannot spell a default that reads another
    // argument, so the default is resolved here.
    let output = args.output.unwrap_or_else(|| args.path.join("Cargo.toml"));

    // Merge default excludes with user excludes
    let all_excludes = if args.no_default_excludes {
        args.exclude
    } else {
        let mut excludes = get_default_excludes();
        excludes.extend(args.exclude);
        excludes
    };

    // Find all Cargo.toml files. An entry the walk cannot read is skipped and
    // counted rather than propagated, so the packages it did find still reach
    // the manifest.
    let mut skipped = Skipped::default();
    let scan = find_cargo_tomls(
        &args.path,
        &all_excludes,
        args.include_worktrees,
        &mut skipped,
    );
    let cargo_files = &scan.packages;

    if cargo_files.is_empty() {
        // A tree the scan read whole and found nothing in is an answer. A tree
        // it could not read is not, so `report` refuses that one.
        skipped.report(cargo_files.len())?;
        println!("No Cargo.toml files found in subdirectories");
        scan.explain_empty_result(&args.path);
        return Ok(());
    }

    println!("Found {} Cargo.toml files:", cargo_files.len());

    // Check for duplicate package names
    let mut package_names: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for cargo_file in cargo_files {
        match get_package_name(cargo_file) {
            Ok(name) => {
                package_names
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .push(cargo_file.clone());
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to get package name from {}: {}",
                    cargo_file.display(),
                    e
                );
            }
        }
    }

    // Report duplicate package names
    let mut has_duplicates = false;
    for (name, paths) in &package_names {
        if paths.len() > 1 {
            has_duplicates = true;
            eprintln!(
                "\nError: Package name '{}' appears in multiple locations:",
                name
            );
            for path in paths {
                eprintln!("  - {}", path.display());
            }
        }
    }

    if has_duplicates {
        eprintln!("\nWorkspace creation failed: duplicate package names found.");
        eprintln!("Each package in a workspace must have a unique name in its Cargo.toml [package] section.");
        eprintln!("\nSuggestions:");
        eprintln!("1. Rename one of the duplicate packages");
        eprintln!("2. Exclude some paths using --exclude");
        eprintln!("3. Use a more specific --path to avoid including duplicates");
        return Err(anyhow::anyhow!("Duplicate package names found"));
    }

    // Name each package the way the manifest that lists it has to read it.
    let mut members = Vec::new();
    let manifest_directory = manifest_directory(&output)?;

    for cargo_file in cargo_files {
        let parent = cargo_file
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Cargo.toml has no parent directory"))?;

        let member = member_path(parent, &manifest_directory, args.prefix.as_deref())?;

        println!("  - {member}");
        members.push(member);
    }

    // Create or update workspace Cargo.toml
    create_or_update_workspace(&output, &members, args.dry_run)?;

    if !args.dry_run {
        println!("\nWorkspace created with {} members", members.len());
        println!("You can now use commands like:");
        println!("  cargo build --workspace");
        println!("  cargo test --workspace");
        println!("  cargo build -p <package-name>");
    }

    // The last word on the scan: a run that skipped something must not read
    // afterwards like a run that read the whole tree.
    skipped.report(cargo_files.len())?;

    Ok(())
}
