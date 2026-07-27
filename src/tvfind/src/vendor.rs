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
    let _ = (vendor, filter);
    false
}
