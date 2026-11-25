use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use std::collections::BTreeMap;
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

/// The directory the walk starts at, as `--path` gave it.
///
/// The run reads one tree and writes one manifest. The two name the same
/// directory only on a default run. Both are paths, so the compiler accepted
/// every mix-up of the two, and this code made three of them: the manifest went
/// to the directory the run started in rather than beside the tree, the member
/// paths started at the tree rather than at the manifest, and the rule that
/// keeps the manifest out of its own member list named the wrong file. A
/// `SearchRoot` is not an `OutputManifest`, so the next such swap is a compile
/// error rather than a defect a reader must find.
struct SearchRoot(PathBuf);

impl SearchRoot {
    /// Names `path` as the directory the walk starts at.
    fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// The directory itself, for the calls that take a plain path.
    fn as_path(&self) -> &Path {
        &self.0
    }
}

/// The manifest file the run writes, as `--output` named it or as the default
/// resolved it.
///
/// Cargo resolves a member against the manifest that lists it, so this file
/// decides what a member path means, and it decides which file the walk must
/// leave out of that list. The search root decides neither. `SearchRoot` holds
/// the other half of the same rule, and the two types keep the compiler between
/// them.
///
/// The directory that holds the file is resolved once, when the file is named.
/// Both of those decisions need it, and neither one can use the spelling the
/// run typed: `--path` and `--output` arrive one relative and one absolute as
/// readily as not, and either one carries `.` and `..` segments.
struct OutputManifest {
    /// The file as the run named it. Every message about the file, and the
    /// write itself, uses this spelling, because it is the one the reader
    /// typed.
    named: PathBuf,
    /// The directory that holds the file, resolved. Every member path is
    /// measured from it.
    directory: PathBuf,
    /// The file itself, named from that resolved directory. The walk compares
    /// every manifest it meets against this one.
    resolved: PathBuf,
}

impl OutputManifest {
    /// Names `manifest` as the file the run writes, and resolves the directory
    /// that holds it.
    ///
    /// # Errors
    ///
    /// Answers the failure to resolve that directory. The failure is now met
    /// before the walk rather than after it, which is the better order: a run
    /// that names a directory nothing holds cannot write its manifest whatever
    /// the walk finds, so a tree of thousands is read for nothing.
    fn new(manifest: PathBuf) -> Result<Self> {
        let directory = manifest_directory(&manifest)?;

        // `Path::file_name` answers `None` for a path that ends in `..` or in
        // a separator. Neither one names a file to write, and the write states
        // that failure itself, so the file keeps the spelling it was given.
        let resolved = match manifest.file_name() {
            Some(name) => directory.join(name),
            None => manifest.clone(),
        };

        Ok(Self {
            named: manifest,
            directory,
            resolved,
        })
    }

    /// The file itself, for the calls that take a plain path.
    fn as_path(&self) -> &Path {
        &self.named
    }

    /// The directory every member path is measured from.
    fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether `candidate` is the file this run writes.
    ///
    /// The walk asks this of every manifest it meets, because a workspace that
    /// listed the manifest it is written into would name the directory that
    /// manifest lives in. The question is about one file, so the answer reads
    /// both paths through the file system rather than as text: the walk builds
    /// a candidate onto the search root, and the search root and `--output`
    /// name the same file in two spellings as often as in one.
    ///
    /// The rule used to name `<search path>/Cargo.toml` instead, which is this
    /// file only on a run that left `--output` at its default. Every other run
    /// dropped the package at the search path without a word, and listed the
    /// manifest it was about to write as a member of itself.
    fn writes(&self, candidate: &Path) -> bool {
        // The walk just met the candidate, so it exists and resolves. A
        // candidate that does not resolve all the same is compared as it was
        // spelled: a file the run is about to overwrite must not reach the
        // members list because a resolve failed.
        match candidate.canonicalize() {
            Ok(candidate) => candidate == self.resolved,
            Err(_) => candidate == self.resolved || candidate == self.named,
        }
    }
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
/// The walk stops at the first checkout that decides, and goes on past a
/// submodule (see [`Checkout`]). A repository of its own decides that its
/// package is listed: what lies above a repository says nothing about what is
/// inside it. A linked worktree decides that its package is left out. A
/// submodule belongs to the checkout of its superproject, so the checkout
/// above it decides: a package in a submodule of the main checkout is listed,
/// and a package in a submodule of a linked worktree is left out with that
/// worktree.
fn is_in_worktree(manifest: &Path, root: &SearchRoot) -> bool {
    let Some(package) = manifest.parent() else {
        return false;
    };

    for directory in package.ancestors() {
        // The search path is where the walk ends. Comparing paths rather than
        // subtracting them keeps a relative `--path` working: `WalkDir` hands
        // back every path built onto the root exactly as it was given, and
        // `Path` compares by component, so `.` and `./` both stop here.
        if directory == root.as_path() {
            return false;
        }

        // A directory the root does not hold cannot be reached by walking up
        // from a path under the root, so this is unreachable in practice. It is
        // the second half of the stop condition all the same: an equality that
        // never matches would otherwise walk to the root of the file system,
        // which is the behaviour being removed.
        if directory.strip_prefix(root.as_path()).is_err() {
            return false;
        }

        match Checkout::at(directory) {
            Some(Checkout::Worktree) => return true,
            Some(Checkout::Repository) => return false,
            Some(Checkout::Submodule) | None => {}
        }
    }

    false
}

/// The kind of git checkout whose top is one directory, as the worktree filter
/// sees it.
///
/// A `.git` directory makes a repository of its own. A `.git` file names a git
/// directory kept elsewhere, and where that git directory lies tells three
/// kinds apart:
///
/// - A linked worktree's lies under `.git/worktrees/<name>` and holds a
///   `commondir` file that names the repository it borrows.
/// - A submodule's lies in the `modules` directory of the git directory of its
///   superproject: `.git/modules/<name>`, or `.git/worktrees/<wt>/modules/<name>`
///   in a linked worktree. A submodule name can hold a `/`, and a submodule of
///   a submodule nests one `modules` directory in the next.
/// - Any other git directory is a repository of its own. `git init
///   --separate-git-dir` and `git clone --separate-git-dir` make one, with a
///   `.git` file and a git directory outside the checkout.
///
/// A `.git` file that cannot be read, that does not start with `gitdir:`, or
/// that names no directory makes a repository of its own too. Git itself stops
/// at such a file ("invalid gitfile format"), and does not put the directory
/// in the checkout above it.
#[derive(Debug, Clone, Copy)]
enum Checkout {
    /// A repository of its own: the packages in it are nobody's second copy.
    Repository,
    /// A linked worktree: a second checkout of a repository kept elsewhere.
    Worktree,
    /// A submodule: part of the checkout of its superproject.
    Submodule,
}

impl Checkout {
    /// The checkout whose top is `directory`, or `None` when `directory` holds
    /// no `.git`.
    ///
    /// A relative git directory is resolved against `directory`, which holds
    /// the `.git` file.
    fn at(directory: &Path) -> Option<Self> {
        let git_path = directory.join(".git");
        if git_path.is_dir() {
            return Some(Self::Repository);
        }
        if !git_path.is_file() {
            return None;
        }

        let Ok(content) = fs::read_to_string(&git_path) else {
            return Some(Self::Repository);
        };
        let Some(named) = content.trim().strip_prefix("gitdir:") else {
            return Some(Self::Repository);
        };
        // Resolved through the file system, so that the walk up the git
        // directory below meets real directories and not `..` components.
        let Ok(gitdir) = directory.join(named.trim()).canonicalize() else {
            return Some(Self::Repository);
        };

        if gitdir.join("commondir").is_file() {
            Some(Self::Worktree)
        } else if lies_in_a_modules_directory(&gitdir) {
            Some(Self::Submodule)
        } else {
            Some(Self::Repository)
        }
    }
}

/// Whether `gitdir` lies in the `modules` directory of another git directory,
/// which is where git keeps the git directory of a submodule.
///
/// A git directory always holds a `HEAD` file, so a `modules` directory beside
/// a `HEAD` file is the one git made. A directory named `modules` anywhere else
/// on the path, such as `/srv/modules/project.git`, is no reason to call a
/// repository a submodule.
fn lies_in_a_modules_directory(gitdir: &Path) -> bool {
    gitdir.ancestors().skip(1).any(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "modules")
            && ancestor
                .parent()
                .is_some_and(|holder| holder.join("HEAD").is_file())
    })
}

/// What one walk of the tree found.
///
/// The two counts answer different questions and an empty result needs both:
/// packages the scan found and a count of the ones the worktree filter took
/// back out. A run that found nothing because everything it found was filtered
/// must not read like a run over a tree that holds no package - the second one
/// is an answer, and the first is a flag the user has not been told about.
///
/// Entries the scan gave up on are counted separately again, by `Skipped`,
/// because those decide the exit status and these do not: a package left out on
/// purpose is not a package the scan could not use.
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
/// A tree of any size holds things a run cannot use: a directory whose mode
/// keeps it out, a `Cargo.toml` that nobody can parse, a package that states no
/// name. Each of those is a reason to skip that entry, not a reason to abandon
/// the tree around it. The first two used to end the walk, so a single
/// unreadable entry threw away every package the scan had already found and
/// said nothing about them — which a scan of a home directory or of a shared
/// tree meets routinely. Each is now named on stderr as it is met and counted
/// here, and the total is stated once at the end.
///
/// A package with no name is read without trouble and is still no member: cargo
/// refuses a whole workspace that lists one. So it belongs here beside the
/// entries nothing could read. It used to reach the members list all the same,
/// and the run then reported a workspace cargo loads no part of.
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

    /// States the total once, and answers nothing.
    ///
    /// The summary goes to stderr, because standard output carries the manifest
    /// under `--dry-run`. A scan that skipped nothing states nothing.
    ///
    /// This is the half of the report that a run with packages to list needs.
    /// That call comes after the manifest is written and after the success
    /// message is printed, where the run has nothing left to fail at, so it
    /// reaches a method that cannot fail. It used to call `report`, whose `?`
    /// read as though the run could still fail there.
    fn state_total(&self) {
        if self.count == 0 {
            return;
        }

        eprintln!(
            "Skipped {} {} that this run could not use.",
            self.count,
            self.entries()
        );
    }

    /// States the total, and says whether the scan answered the question.
    ///
    /// `found` is how many packages reach the members list, so a package the
    /// run left out counts here and not there.
    ///
    /// A scan that skipped something and still has packages to list succeeded:
    /// those packages are the answer, and what it skipped is named above them.
    /// A scan that skipped something and has no package left to list never
    /// answered the question it was asked, so it fails instead of reporting an
    /// empty tree — the two are different answers, and only one is success.
    ///
    /// # Errors
    ///
    /// Answers that refusal.
    fn report(&self, found: usize) -> Result<()> {
        self.state_total();

        if self.count > 0 && found == 0 {
            return Err(anyhow::anyhow!(
                "Found no packages, and this run skipped {} {}",
                self.count,
                self.entries()
            ));
        }

        Ok(())
    }

    /// The word for the number of entries the scan gave up on. Both halves of
    /// the report state that number, and they have to agree on the word for it.
    fn entries(&self) -> &'static str {
        if self.count == 1 {
            "entry"
        } else {
            "entries"
        }
    }
}

/// The packages below `root` that the manifest `output` has to list.
///
/// The walk reads the tree under `root` and leaves out three things: a
/// directory `excludes` names, a manifest that describes a workspace, and the
/// one file `output` names, which no workspace can hold as a member of itself.
/// The two paths are separate arguments because they answer separate questions,
/// and they name the same file only on a run that left `--output` at its
/// default.
fn find_cargo_tomls(
    root: &SearchRoot,
    output: &OutputManifest,
    excludes: &[String],
    include_worktrees: bool,
    skipped: &mut Skipped,
) -> Scan {
    let mut cargo_files = Vec::new();
    let mut worktrees = 0;

    for entry in WalkDir::new(root.as_path())
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let path = e.path();

            // Skip hidden directories
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with('.') && path != root.as_path() {
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
            let below_root = path
                .strip_prefix(root.as_path())
                .unwrap_or_else(|_| Path::new(""));
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

        // The manifest the run writes is the one file the walk leaves out. It
        // is not the manifest at the search path, except on a run that writes
        // there.
        if path.file_name() == Some("Cargo.toml".as_ref()) && !output.writes(path) {
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

/// The name the package in `path` states.
///
/// # Errors
///
/// Answers the failure to read the file, the failure to parse it, and a
/// `[package]` table that states no name. Every one of those names the file,
/// because the caller reports them beside the other entries it skipped: a
/// warning that names no file leaves the reader to find it in a tree of
/// thousands.
fn get_package_name(path: &Path) -> Result<String> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let doc = content
        .parse::<DocumentMut>()
        .with_context(|| format!("Failed to parse {}", path.display()))?;

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
/// the manifest `output` names.
///
/// Cargo resolves a member against the manifest that lists it, so the manifest
/// decides what a member means — not the tree the walk happened to start at.
/// The two are the same directory on a default run and part company as soon as
/// `--output` names a file somewhere else.
///
/// `OutputManifest::new` is the one caller. It hands over a plain path, because
/// this is the step that makes an `OutputManifest`, and it keeps the answer, so
/// no run resolves this directory twice.
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

/// The member paths for the packages `manifests` names, as the manifest in
/// `manifest_directory` lists them.
///
/// Both sides of the subtraction are canonical, so it is exact whether the run
/// named its tree relatively or absolutely — a relative `--path` used to leave
/// a `./` on the front of every member, because an absolute root cannot be
/// taken off a relative path.
///
/// A package in the manifest directory itself is the member cargo calls `.`,
/// and cargo loads a workspace that lists it. The subtraction leaves nothing
/// there, so the member used to read as an empty string: the manifest held
/// `members = [""]` and the run printed a bullet with nothing behind it.
///
/// Cargo holds a member only where the manifest that lists it sits above that
/// member on the file system. It refuses every other member with `workspace
/// member ... is not hierarchically below the workspace root ...`, and it says
/// that of an absolute path and of a `../` path alike. So a package the
/// manifest directory does not hold has no spelling at all, and this run
/// refuses it. It used to write such a package as an absolute path, which no
/// checkout could load, not even the one that wrote it. An explicit `--output`
/// below the tree that was scanned is what reaches this.
///
/// The refusal names every such package rather than the first one, because
/// `--output` in the wrong place is one mistake in the invocation, and the
/// user who repairs it needs the whole list. It refuses the run rather than
/// leaves those packages out, for the same reason: the mistake is in the
/// invocation and not in the tree, so no part of the answer is safe to drop.
/// A defect in one package of the tree is the opposite case, and it takes the
/// opposite treatment: that one package goes and the rest are still listed.
///
/// # Errors
///
/// Answers the failure to resolve a package directory, and the refusal above.
fn member_paths(manifests: &[PathBuf], manifest_directory: &Path) -> Result<Vec<String>> {
    let mut members = Vec::new();
    let mut outside = Vec::new();

    for manifest in manifests {
        let package = manifest
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", manifest.display()))?;
        let package = package
            .canonicalize()
            .with_context(|| format!("Failed to resolve {}", package.display()))?;

        match package.strip_prefix(manifest_directory) {
            Ok(relative) => {
                let member = relative.to_string_lossy().replace('\\', "/");
                members.push(if member.is_empty() {
                    // The manifest directory holds a package of its own, so the
                    // subtraction leaves nothing. Cargo names that directory
                    // `.`.
                    ".".to_string()
                } else {
                    member
                });
            }
            Err(_) => outside.push(package),
        }
    }

    if outside.is_empty() {
        return Ok(members);
    }

    // The scan hands its packages over sorted, so this report reads the same on
    // every run over one tree, like the duplicate name report above it.
    let (packages, verb) = if outside.len() == 1 {
        ("package", "is")
    } else {
        ("packages", "are")
    };
    eprintln!(
        "\nError: {} {packages} {verb} not below {}, the directory that holds the workspace manifest:",
        outside.len(),
        manifest_directory.display()
    );
    for package in &outside {
        eprintln!("  - {}", package.display());
    }
    eprintln!("\nWorkspace creation failed: a member has to sit below the manifest that lists it.");
    eprintln!(
        "Cargo resolves a member against that manifest, and refuses a member outside it. An absolute path and a `../` path are both refused."
    );
    eprintln!("\nSuggestions:");
    eprintln!("1. Give --output a path at or above the tree that was scanned, such as `workit --path packages --output Cargo.toml`");
    eprintln!("2. Leave --output out, so the manifest lands beside the tree that was scanned");
    eprintln!("3. Use a --path that the manifest directory holds");

    Err(anyhow::anyhow!(
        "{} {packages} outside the manifest directory",
        outside.len()
    ))
}

fn create_or_update_workspace(
    output: &OutputManifest,
    members: &[String],
    dry_run: bool,
) -> Result<()> {
    let output = output.as_path();

    let mut doc = if output.exists() {
        let content = fs::read_to_string(output)
            .with_context(|| format!("Failed to read existing {}", output.display()))?;
        content
            .parse::<DocumentMut>()
            .with_context(|| format!("Failed to parse existing {}", output.display()))?
    } else {
        DocumentMut::new()
    };

    if doc.get("workspace").is_none() {
        doc["workspace"] = Item::Table(Table::new());
    }

    // Cargo loads a workspace written as an inline table,
    // `workspace = { members = [...] }`, as readily as a `[workspace]` table,
    // so the run edits either one where it stands.
    let workspace = doc["workspace"].as_table_like_mut().ok_or_else(|| {
        anyhow::anyhow!("The workspace key in {} is not a table", output.display())
    })?;

    if workspace.get("members").is_none() {
        workspace.insert(
            "members",
            Item::Value(toml_edit::Value::Array(Array::new())),
        );
    }

    let members_array = workspace
        .get_mut("members")
        .and_then(Item::as_array_mut)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "The workspace.members key in {} is not an array",
                output.display()
            )
        })?;

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
        workspace.insert("resolver", toml_edit::value("2"));
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
    let output = OutputManifest::new(args.output.unwrap_or_else(|| args.path.join("Cargo.toml")))?;

    // The tree the walk reads. It is named apart from the manifest above, so
    // neither one reaches a parameter that means the other.
    let root = SearchRoot::new(args.path);

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
        &root,
        &output,
        &all_excludes,
        args.include_worktrees,
        &mut skipped,
    );

    // Read the name of every package the walk found, and keep the ones that
    // have a name. A `[package]` table with no name passes the package test,
    // because that test asks only whether the table is there, so the walk hands
    // such a manifest over like any other. Cargo refuses a whole workspace that
    // lists a package it cannot name, so this run skips that one package and
    // lists the rest — the defect is in one package of the tree, not in the
    // invocation, which is the same reason an unreadable manifest is skipped.
    // An `--output` below the packages takes the opposite treatment, because
    // that mistake is in the invocation and no part of the answer is then safe
    // to drop.
    //
    // The map is ordered by package name, and the scan handed its packages over
    // sorted, so the duplicate report below reads the same on every run: the
    // groups in order by name, and the paths inside one group in order by path.
    // A `HashMap` seeds its iteration order afresh in every process, which
    // printed one tree's groups in a different order on every run and on every
    // machine — and that report is a list of directories to rename, so it is
    // read beside the previous run's.
    let mut package_names: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut named: Vec<PathBuf> = Vec::new();
    for cargo_file in &scan.packages {
        match get_package_name(cargo_file) {
            Ok(name) => {
                package_names
                    .entry(name)
                    .or_default()
                    .push(cargo_file.clone());
                named.push(cargo_file.clone());
            }
            Err(error) => skipped.skip(&error),
        }
    }

    if named.is_empty() {
        // A tree the scan read whole and found nothing in is an answer. A tree
        // it could not read is not, and neither is a tree whose every package
        // refused to name itself, so `report` refuses both of those: it takes
        // the number of packages that reach the members list, which is none
        // here. The line below therefore prints on the one path where it is
        // true.
        skipped.report(named.len())?;
        println!("No Cargo.toml files found in subdirectories");
        scan.explain_empty_result(root.as_path());
        return Ok(());
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

    // Name each package the way the manifest that lists it has to read it. The
    // directory it is measured from was resolved when the manifest was named,
    // before the walk. A tree this manifest cannot hold is refused here, so the
    // members reach the reader only when every one of them is a member cargo
    // accepts.
    let members = member_paths(&named, output.directory())?;

    // The count states the members, and the bullets below name them. It used to
    // state what the walk found, above the duplicate report, and it counted
    // every manifest the walk met. So a run that left a package out said it had
    // found two packages and then listed one, and a run the duplicate report
    // refused printed a count with nothing under it.
    let packages = if members.len() == 1 {
        "package"
    } else {
        "packages"
    };
    println!("Found {} {packages}:", members.len());
    for member in &members {
        println!("  - {member}");
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
    // afterwards like a run that used the whole tree. Only the total is stated
    // here. The run has a member for every package it named, and it wrote the
    // manifest and printed the success message above, so there is nothing left
    // for it to refuse.
    skipped.state_total();

    Ok(())
}
