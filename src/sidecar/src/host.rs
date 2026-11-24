//! The host repository: its worktree, its main checkout, and the paths that derive
//! from them.
//!
//! A host has one main checkout and any number of worktrees. All of them share one
//! common git directory, thus the main clones of the sidecars sit next to the main
//! checkout whichever worktree the user runs `sidecar` in.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail};

use crate::git::Git;
use crate::name::SidecarName;
use crate::registry::Registry;
use crate::remote;

/// The end of the name of the directory that holds the sidecar worktrees.
pub const WORKTREES_SUFFIX: &str = "-worktrees";

/// The name of the git directory of a main checkout.
const GIT_DIRECTORY: &str = ".git";

/// What `Host::discover` does when `sidecar.toml` is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryPolicy {
    /// A missing file is an error.
    Required,
    /// A missing file gives the default registry, which names no sidecar.
    DefaultWhenMissing,
}

/// The host repository, as seen from one worktree.
#[derive(Debug)]
pub struct Host {
    git: Git,
    worktree_root: PathBuf,
    main_checkout: PathBuf,
    parent: PathBuf,
    origin: String,
    name: String,
    registry: Registry,
}

impl Host {
    /// Finds the host from the directory `cwd`, which is inside a worktree of the host.
    ///
    /// # Errors
    ///
    /// Returns an error when `cwd` is not inside a git worktree, when the common git
    /// directory is not the `.git` directory of a main checkout, when the repository
    /// has no `origin`, when the URL of `origin` names no repository, or when the
    /// registry fails to load under `policy`.
    pub fn discover(cwd: &Path, policy: RegistryPolicy) -> anyhow::Result<Host> {
        let git = Git::without_token();
        let top = git.run(cwd, ["rev-parse", "--show-toplevel"]);
        if !top.ok {
            bail!(
                "sidecar runs inside a worktree of a git repository: {}",
                top.stderr
            );
        }
        let worktree_root = PathBuf::from(top.stdout);

        let common = git.run(
            &worktree_root,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        if !common.ok {
            bail!(
                "sidecar cannot find the git directory of {}: {}",
                worktree_root.display(),
                common.stderr
            );
        }
        let common = PathBuf::from(common.stdout);
        let main_checkout = match common.file_name() {
            Some(name) if name == GIT_DIRECTORY => common
                .parent()
                .ok_or_else(|| anyhow!("The git directory {} has no parent.", common.display()))?
                .to_path_buf(),
            _ => bail!(
                "The common git directory {} is not the .git directory of a main checkout, \
                 thus sidecar cannot find the place of the main clones.",
                common.display()
            ),
        };
        let parent = main_checkout
            .parent()
            .ok_or_else(|| {
                anyhow!(
                    "The main checkout {} has no parent directory.",
                    main_checkout.display()
                )
            })?
            .to_path_buf();

        let origin = git.run(&worktree_root, ["remote", "get-url", "origin"]);
        if !origin.ok {
            bail!(
                "sidecar cannot read the URL of the remote origin of {}: {}",
                worktree_root.display(),
                origin.stderr
            );
        }
        let origin = origin.stdout;
        let name = remote::repository_name(&origin)
            .ok_or_else(|| anyhow!("The URL of origin, \"{origin}\", names no repository."))?;

        let registry = match policy {
            RegistryPolicy::Required => Registry::load(&worktree_root)?,
            RegistryPolicy::DefaultWhenMissing => Registry::load_or_default(&worktree_root)?,
        };
        let git = Git::with_token_from(registry.token_env());
        Ok(Host {
            git,
            worktree_root,
            main_checkout,
            parent,
            origin,
            name,
            registry,
        })
    }

    /// The git runner, with the CI token when the registry names a variable that is set.
    #[must_use]
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// The root of the worktree that the user runs in.
    #[must_use]
    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    /// The main checkout of the host.
    #[must_use]
    pub fn main_checkout(&self) -> &Path {
        &self.main_checkout
    }

    /// The directory that holds the main checkout. The main clones sit here.
    #[must_use]
    pub fn parent(&self) -> &Path {
        &self.parent
    }

    /// The URL of the remote `origin` of the host.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The name of the host, for example `megarepo`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The registry of the host.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The place of the main clone of `name`: `<parent>/<host>-data-<name>`.
    #[must_use]
    pub fn main_clone(&self, name: &SidecarName) -> PathBuf {
        self.parent.join(self.sidecar_repository(name))
    }

    /// The directory of the worktrees of `name`: `<parent>/<host>-data-<name>-worktrees`.
    #[must_use]
    pub fn worktrees_dir(&self, name: &SidecarName) -> PathBuf {
        self.parent.join(format!(
            "{}{WORKTREES_SUFFIX}",
            self.sidecar_repository(name)
        ))
    }

    /// The place of the link of `name`: `<worktree root>/<link_dir>/<name>`.
    #[must_use]
    pub fn link(&self, name: &SidecarName) -> PathBuf {
        self.worktree_root
            .join(self.registry.link_dir())
            .join(name.as_str())
    }

    /// The link as a person reads it, for example `data/testdata`.
    #[must_use]
    pub fn link_shown(&self, name: &SidecarName) -> String {
        format!("{}/{name}", self.link_dir_shown())
    }

    /// The URL of the remote of the sidecar `name`.
    ///
    /// # Errors
    ///
    /// Returns an error when the URL of `origin` has no last segment to replace.
    pub fn sidecar_url(&self, name: &SidecarName) -> anyhow::Result<String> {
        remote::sidecar_url(&self.origin, name)
    }

    /// The name of the repository of the sidecar `name`: `<host>-data-<name>`.
    #[must_use]
    pub fn sidecar_repository(&self, name: &SidecarName) -> String {
        remote::sidecar_repository(&self.name, name)
    }

    /// True when git ignores the link directory in the worktree.
    #[must_use]
    pub fn ignores_link_dir(&self) -> bool {
        self.git
            .run(
                &self.worktree_root,
                ["check-ignore", "-q", &format!("{}/", self.link_dir_shown())],
            )
            .ok
    }

    /// The line that `.gitignore` needs, for example `/data/`.
    #[must_use]
    pub fn ignore_line(&self) -> String {
        format!("/{}/", self.link_dir_shown())
    }

    /// The link directory with `/` between its components.
    fn link_dir_shown(&self) -> String {
        self.registry
            .link_dir()
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}
