//! Focus step 2: finds the WezTerm pane of a terminal, and asks WezTerm to
//! focus it.

/// Makes the argv that lists the WezTerm panes as JSON.
#[must_use]
pub fn list_argv() -> Vec<String> {
    ["wezterm", "cli", "list", "--format", "json"]
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// One pane in the list of WezTerm.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct WeztermPane {
    /// The number of the pane.
    pub pane_id: u64,
    /// The terminal device of the pane, for example `/dev/ttys231`.
    #[serde(default)]
    pub tty_name: Option<String>,
}

/// Reads the panes from the output of `wezterm cli list --format json`.
///
/// The function ignores the other keys of each pane.
///
/// # Errors
///
/// Returns an error when the text is not a JSON list of panes.
pub fn parse_panes(_json: &str) -> Result<Vec<WeztermPane>, serde_json::Error> {
    Ok(Vec::new())
}

/// Finds the pane that shows the terminal device `tty`.
#[must_use]
pub fn pane_for_tty(_panes: &[WeztermPane], _tty: &str) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("../fixtures/wezterm-list.json");

    fn pane(id: u64, tty: Option<&str>) -> WeztermPane {
        WeztermPane {
            pane_id: id,
            tty_name: tty.map(ToString::to_string),
        }
    }

    #[test]
    fn the_list_argv_asks_for_json() {
        assert_eq!(list_argv(), vec!["wezterm", "cli", "list", "--format", "json"]);
    }

    #[test]
    fn the_fixture_has_three_panes() {
        assert_eq!(
            parse_panes(LIST).unwrap(),
            vec![
                pane(378, Some("/dev/ttys354")),
                pane(239, Some("/dev/ttys231")),
                pane(235, Some("/dev/ttys097")),
            ]
        );
    }

    #[test]
    fn a_pane_without_a_tty_name_parses_with_none() {
        assert_eq!(
            parse_panes(r#"[{"pane_id": 7, "title": "x"}]"#).unwrap(),
            vec![pane(7, None)]
        );
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(parse_panes("not json").is_err());
        assert!(parse_panes(r#"{"pane_id": 7}"#).is_err());
    }

    #[test]
    fn a_tty_finds_its_pane() {
        let panes = parse_panes(LIST).unwrap();
        assert_eq!(pane_for_tty(&panes, "/dev/ttys231"), Some(239));
        assert_eq!(pane_for_tty(&panes, "/dev/ttys354"), Some(378));
    }

    #[test]
    fn an_unknown_tty_has_no_pane() {
        let panes = parse_panes(LIST).unwrap();
        assert_eq!(pane_for_tty(&panes, "/dev/ttys999"), None);
        assert_eq!(pane_for_tty(&[pane(7, None)], "/dev/ttys231"), None);
    }
}
