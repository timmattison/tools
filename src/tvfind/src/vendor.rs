//! Matching user-supplied vendor filters against reported manufacturer names.

/// Contract-manufacturer names that ship panels under a better-known brand.
///
/// A TV reports its brand over HTTP, but the MAC it registers belongs to
/// whichever factory built it. Without these aliases an OUI lookup for a
/// powered-off `TCL` set would miss, because the address block is registered
/// to the Huizhou ODM rather than to TCL itself.
const ODM_ALIASES: &[(&str, &[&str])] = &[("tcl", &["gaoshengda"])];

/// Whether `vendor` satisfies `filter`, case-insensitively.
///
/// An empty filter matches everything. Matching is by substring so that
/// `tcl` matches `TCL King Electrical Appliances(Huizhou)Co.`, and known ODM
/// aliases for the filter are matched too.
#[must_use]
pub fn matches(vendor: &str, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }

    let vendor = vendor.to_lowercase();
    if vendor.contains(&filter) {
        return true;
    }

    ODM_ALIASES
        .iter()
        .filter(|(brand, _)| *brand == filter)
        .flat_map(|(_, factories)| factories.iter())
        .any(|factory| vendor.contains(factory))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_filter_accepts_every_vendor() {
        assert!(matches("TCL", ""));
        assert!(matches("Hisense", ""));
        assert!(matches("", ""));
    }

    #[test]
    fn matches_a_brand_regardless_of_case() {
        assert!(matches("TCL", "tcl"));
        assert!(matches("tcl", "TCL"));
    }

    #[test]
    fn matches_a_brand_buried_in_a_registered_company_name() {
        assert!(matches("TCL King Electrical Appliances(Huizhou)Co.", "tcl"));
    }

    #[test]
    fn rejects_an_unrelated_vendor() {
        assert!(!matches("Sonos", "tcl"));
        assert!(!matches("Hui Zhou Gaoshengda Technology", "hisense"));
    }

    #[test]
    fn matches_the_contract_manufacturer_that_builds_the_brands_panels() {
        // TCL sets register MACs to their Huizhou ODM, not to TCL itself.
        assert!(matches("Hui Zhou Gaoshengda Technology", "tcl"));
    }

    #[test]
    fn ignores_surrounding_whitespace_in_the_filter() {
        assert!(matches("TCL", "  tcl  "));
    }
}
