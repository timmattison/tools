use std::path::Path;

/// The directory whose `portplz` derivation supplies the default port.
///
/// It is the export root's PARENT when the root has one, and the root itself
/// otherwise.
///
/// `find_root` resolves to the same absolute `out` path whether the user ran from
/// the project directory or from inside `out`, so both invocations already agree
/// on a port. The parent matters for a different reason: outside a git repository
/// `portplz` hashes the directory BASENAME, and every static-export root on the
/// machine is named `out` — so deriving from the root itself would hand every
/// project the same port, which is the collision this port of the tool exists to
/// remove. The parent is the project directory, whose name is distinct. Inside a
/// git repository the choice makes no difference, because the repository name and
/// the current branch decide the hash.
fn port_basis(root: &Path) -> &Path {
    root
}

fn main() {
    // The CLI arrives in a later slice; this binary exists so the crate builds
    // with both a library and a binary target from the start.
}

#[cfg(test)]
mod tests {
    use super::port_basis;
    use std::path::Path;

    #[test]
    fn the_basis_is_the_project_directory_holding_the_export_root() {
        assert_eq!(
            port_basis(Path::new("/projects/site/out")),
            Path::new("/projects/site")
        );
    }

    #[test]
    fn a_root_with_no_parent_is_its_own_basis() {
        assert_eq!(port_basis(Path::new("/")), Path::new("/"));
    }
}
