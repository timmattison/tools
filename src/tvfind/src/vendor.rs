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

/// Whether `vendor` names a manufacturer that builds televisions.
#[must_use]
pub fn is_television_brand(vendor: &str) -> bool {
    let _ = vendor;
    todo!("no television brand list exists yet")
}

/// Whether a neighbour registered to `vendor` is worth reporting as a possible
/// television, given the user's `filter`.
#[must_use]
pub fn wanted_as_television(vendor: &str, filter: &str) -> bool {
    let _ = (vendor, filter);
    todo!("no television brand list exists yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_television_brand_that_opens_the_registered_name() {
        assert!(is_television_brand("TCL King Electrical Appliances(Huizhou)Co."));
        assert!(is_television_brand("Hisense Visual Technology"));
        assert!(is_television_brand("Vizio"));
    }

    #[test]
    fn recognises_a_television_brand_buried_inside_the_registered_name() {
        // The IEEE registry often leads with the city or the parent company.
        assert!(is_television_brand("Huizhou TCL Communication Electron"));
        assert!(is_television_brand("Shenzhen Skyworth Digital  Technology"));
        assert!(is_television_brand("Sichuan Changhong Electric"));
    }

    #[test]
    fn recognises_the_contract_factory_that_builds_a_brands_panels() {
        assert!(is_television_brand("Hui Zhou Gaoshengda Technology"));
    }

    #[test]
    fn rejects_a_vendor_that_builds_no_televisions() {
        assert!(!is_television_brand("Ubiquiti"));
        assert!(!is_television_brand("Espressif Inc."));
        assert!(!is_television_brand("Sonos"));
        assert!(!is_television_brand("Murata Manufacturing"));
        assert!(!is_television_brand("Apple"));
    }

    #[test]
    fn tells_a_television_maker_from_a_component_supplier_of_the_same_group() {
        // LG Innotek supplies camera and radio modules to other makers, so a
        // substring test for `lg` would report every one of them as a TV.
        assert!(is_television_brand("LG Electronics"));
        assert!(!is_television_brand("LG Innotek"));
    }

    #[test]
    fn matches_a_brand_as_a_whole_word_only() {
        // `Viziontech` merely opens with the letters of `Vizio`.
        assert!(!is_television_brand("Viziontech UK"));
    }

    #[test]
    fn reports_only_television_brands_when_no_filter_was_given() {
        assert!(wanted_as_television("TCL King Electrical Appliances", ""));
        assert!(!wanted_as_television("Ubiquiti", ""));
    }

    #[test]
    fn obeys_an_explicit_filter_even_for_a_vendor_that_builds_no_televisions() {
        // The filter is the user's own judgement, so it is used as given.
        assert!(wanted_as_television("Ubiquiti", "ubiquiti"));
        assert!(!wanted_as_television("Sonos", "tcl"));
    }

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
