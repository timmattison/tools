use anyhow::Context;
use buildinfo::version_string;
use clap::Parser;
use std::io::{self, IsTerminal, Read};
use uuid::Uuid;

/// Default RFC 4122 namespace used for v5 generation when `--namespace` is not
/// given. The URL namespace is a well-known constant, so output is reproducible
/// and interoperable with other UUIDv5 implementations.
const DEFAULT_NAMESPACE: Uuid = Uuid::nil();

#[derive(Parser)]
#[command(name = "uuidplz")]
#[command(version = version_string!())]
#[command(
    about = "Generate a random UUID, or a repeatable name-based UUIDv5 from a string or file",
    long_about = None
)]
struct Cli {
    /// Input to derive a v5 UUID from. If it names an existing file, the file's
    /// contents are hashed; otherwise the text itself is hashed. Omit entirely
    /// to generate a random v4 UUID (or pipe data on stdin for v5).
    input: Option<String>,

    /// Treat INPUT as a literal string, even if a file by that name exists.
    #[arg(long)]
    string: bool,

    /// Treat INPUT as a file path and hash its contents.
    #[arg(long)]
    file: bool,

    /// Namespace UUID for v5 generation (defaults to the RFC 4122 URL namespace).
    #[arg(long, value_name = "UUID")]
    namespace: Option<String>,

    /// Print how the UUID was derived to stderr (stdout stays just the UUID).
    #[arg(short, long)]
    verbose: bool,
}

/// How the program will produce a UUID, after resolving arguments and environment.
#[derive(Debug, PartialEq, Eq)]
enum Resolution {
    /// No input: emit a random v4 UUID.
    Random,
    /// Hash the given string's UTF-8 bytes as a v5 name.
    FromString(String),
    /// Read the file at this path and hash its bytes as a v5 name.
    FromFile(String),
    /// Read all of stdin and hash its bytes as a v5 name.
    FromStdin,
}

/// Why argument resolution failed.
#[derive(Debug, PartialEq, Eq)]
enum ResolveError {
    /// `--string`/`--file` were given without any INPUT argument.
    FlagWithoutInput,
}

/// Decide what to generate from parsed arguments and environment.
///
/// `file_exists` reports whether `input` (when present) names an existing file;
/// it is injected so the decision stays pure and unit-testable. `stdin_is_tty`
/// distinguishes an interactive terminal (random) from piped data (hash stdin).
///
/// # Errors
///
/// Returns [`ResolveError::FlagWithoutInput`] if `--string`/`--file` are set but
/// no `input` was provided.
fn resolve(
    input: Option<&str>,
    force_string: bool,
    force_file: bool,
    file_exists: bool,
    stdin_is_tty: bool,
) -> Result<Resolution, ResolveError> {
    let _ = (input, force_string, force_file, file_exists, stdin_is_tty);
    todo!()
}

/// Parse the optional `--namespace` value, falling back to [`DEFAULT_NAMESPACE`].
///
/// # Errors
///
/// Returns an error if `arg` is `Some` but not a valid UUID.
fn parse_namespace(arg: Option<&str>) -> Result<Uuid, uuid::Error> {
    let _ = arg;
    todo!()
}

/// Generate a random version-4 UUID.
fn generate_v4() -> Uuid {
    todo!()
}

/// Generate a name-based version-5 (SHA-1) UUID from a namespace and name bytes.
fn generate_v5(namespace: &Uuid, name: &[u8]) -> Uuid {
    let _ = (namespace, name);
    todo!()
}

/// Read a file's raw bytes for hashing.
///
/// # Errors
///
/// Returns an error (with the path attached) if the file cannot be read.
fn read_file_bytes(path: &str) -> anyhow::Result<Vec<u8>> {
    let _ = path;
    todo!()
}

/// Render a human-readable description of how the UUID was derived, for `--verbose`.
fn describe(resolution: &Resolution, namespace: &Uuid) -> String {
    let _ = (resolution, namespace);
    todo!()
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let namespace = parse_namespace(cli.namespace.as_deref())
        .map_err(|e| anyhow::anyhow!("invalid --namespace value: {e}"))?;

    let stdin = io::stdin();
    let input_ref = cli.input.as_deref();
    let file_exists = input_ref
        .map(|p| std::path::Path::new(p).is_file())
        .unwrap_or(false);

    let resolution = resolve(
        input_ref,
        cli.string,
        cli.file,
        file_exists,
        stdin.is_terminal(),
    )
    .map_err(|e| match e {
        ResolveError::FlagWithoutInput => {
            anyhow::anyhow!("--string/--file require an INPUT argument")
        }
    })?;

    let uuid = match &resolution {
        Resolution::Random => generate_v4(),
        Resolution::FromString(s) => generate_v5(&namespace, s.as_bytes()),
        Resolution::FromFile(path) => generate_v5(&namespace, &read_file_bytes(path)?),
        Resolution::FromStdin => {
            let mut buf = Vec::new();
            stdin
                .lock()
                .read_to_end(&mut buf)
                .context("failed to read stdin")?;
            generate_v5(&namespace, &buf)
        }
    };

    if cli.verbose {
        eprintln!("uuidplz: {}", describe(&resolution, &namespace));
    }
    println!("{uuid}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Version;

    // --- v5 generation ---

    #[test]
    fn v5_matches_known_vector() {
        // Canonical example from Python's uuid docs:
        // uuid5(NAMESPACE_DNS, 'python.org') == 886313e1-3b8a-5372-9b90-0c9aee199e5d
        let expected = Uuid::parse_str("886313e1-3b8a-5372-9b90-0c9aee199e5d").unwrap();
        assert_eq!(generate_v5(&Uuid::NAMESPACE_DNS, b"python.org"), expected);
    }

    #[test]
    fn v5_is_deterministic() {
        let a = generate_v5(&DEFAULT_NAMESPACE, b"hello");
        let b = generate_v5(&DEFAULT_NAMESPACE, b"hello");
        assert_eq!(a, b);
    }

    #[test]
    fn v5_differs_for_different_input() {
        let a = generate_v5(&DEFAULT_NAMESPACE, b"alpha");
        let b = generate_v5(&DEFAULT_NAMESPACE, b"beta");
        assert_ne!(a, b);
    }

    #[test]
    fn v5_reports_version_5() {
        let u = generate_v5(&DEFAULT_NAMESPACE, b"hello");
        assert_eq!(u.get_version(), Some(Version::Sha1));
    }

    #[test]
    fn default_namespace_is_rfc_url() {
        assert_eq!(DEFAULT_NAMESPACE, Uuid::NAMESPACE_URL);
    }

    #[test]
    fn string_and_file_bytes_produce_same_uuid() {
        // Identical bytes via the string path or the file path must yield the
        // same UUID, since v5 hashes raw bytes.
        let via_string = generate_v5(&DEFAULT_NAMESPACE, "config.json".as_bytes());
        let via_file = generate_v5(&DEFAULT_NAMESPACE, b"config.json");
        assert_eq!(via_string, via_file);
    }

    #[test]
    fn v5_handles_multibyte_utf8() {
        // Must not panic on multi-byte input and must stay deterministic.
        for s in ["日本語", "café", "🎉🎊"] {
            let a = generate_v5(&DEFAULT_NAMESPACE, s.as_bytes());
            let b = generate_v5(&DEFAULT_NAMESPACE, s.as_bytes());
            assert_eq!(a, b);
            assert_eq!(a.get_version(), Some(Version::Sha1));
        }
    }

    // --- v4 generation ---

    #[test]
    fn v4_reports_version_4() {
        assert_eq!(generate_v4().get_version(), Some(Version::Random));
    }

    #[test]
    fn v4_is_random_across_calls() {
        assert_ne!(generate_v4(), generate_v4());
    }

    // --- namespace parsing ---

    #[test]
    fn parse_namespace_defaults_to_url() {
        assert_eq!(parse_namespace(None).unwrap(), Uuid::NAMESPACE_URL);
    }

    #[test]
    fn parse_namespace_accepts_valid_uuid() {
        // NAMESPACE_DNS, supplied as a string.
        let ns = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        assert_eq!(parse_namespace(Some(ns)).unwrap(), Uuid::NAMESPACE_DNS);
    }

    #[test]
    fn parse_namespace_rejects_invalid_uuid() {
        assert!(parse_namespace(Some("not-a-uuid")).is_err());
    }

    // --- resolution matrix ---

    #[test]
    fn resolve_no_input_tty_is_random() {
        assert_eq!(
            resolve(None, false, false, false, true).unwrap(),
            Resolution::Random
        );
    }

    #[test]
    fn resolve_no_input_piped_is_stdin() {
        assert_eq!(
            resolve(None, false, false, false, false).unwrap(),
            Resolution::FromStdin
        );
    }

    #[test]
    fn resolve_autodetects_existing_file() {
        assert_eq!(
            resolve(Some("config.json"), false, false, true, true).unwrap(),
            Resolution::FromFile("config.json".into())
        );
    }

    #[test]
    fn resolve_autodetects_missing_file_as_string() {
        assert_eq!(
            resolve(Some("config.json"), false, false, false, true).unwrap(),
            Resolution::FromString("config.json".into())
        );
    }

    #[test]
    fn resolve_force_string_overrides_existing_file() {
        assert_eq!(
            resolve(Some("config.json"), true, false, true, true).unwrap(),
            Resolution::FromString("config.json".into())
        );
    }

    #[test]
    fn resolve_force_file_even_when_not_detected() {
        assert_eq!(
            resolve(Some("data"), false, true, false, true).unwrap(),
            Resolution::FromFile("data".into())
        );
    }

    #[test]
    fn resolve_flags_without_input_error() {
        assert_eq!(
            resolve(None, true, false, false, true),
            Err(ResolveError::FlagWithoutInput)
        );
        assert_eq!(
            resolve(None, false, true, false, true),
            Err(ResolveError::FlagWithoutInput)
        );
    }

    // --- file reading ---

    #[test]
    fn read_file_bytes_returns_contents() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello bytes").unwrap();
        let path = f.path().to_str().unwrap().to_string();
        assert_eq!(read_file_bytes(&path).unwrap(), b"hello bytes");
    }

    #[test]
    fn read_file_bytes_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.bin");
        assert!(read_file_bytes(missing.to_str().unwrap()).is_err());
    }

    // --- verbose description ---

    #[test]
    fn describe_random_mentions_v4() {
        assert_eq!(
            describe(&Resolution::Random, &DEFAULT_NAMESPACE),
            "generated a random v4 UUID"
        );
    }

    #[test]
    fn describe_file_includes_path_and_namespace() {
        let ns = Uuid::NAMESPACE_DNS;
        let d = describe(&Resolution::FromFile("x.txt".into()), &ns);
        assert!(d.contains("x.txt"), "should mention the file path: {d}");
        assert!(
            d.contains(&ns.to_string()),
            "should mention the namespace: {d}"
        );
    }

    // --- CLI wiring ---

    #[test]
    fn cli_rejects_string_and_file_together() {
        assert!(Cli::try_parse_from(["uuidplz", "--string", "--file", "x"]).is_err());
    }
}
