/// Gluetun-supported ProtonVPN US cities.
///
/// Source: <https://github.com/qdm12/gluetun-wiki/blob/main/setup/providers/protonvpn.md>
/// Last updated: 2026-03-29
pub const PROTONVPN_US_CITIES: &[&str] = &[
    "Ashburn",
    "Atlanta",
    "Boston",
    "Charlotte",
    "Chicago",
    "Columbus",
    "Dallas",
    "Denver",
    "Detroit",
    "Honolulu",
    "Houston",
    "Jacksonville",
    "Kansas City",
    "Las Vegas",
    "Los Angeles",
    "Manassas",
    "Miami",
    "Minneapolis",
    "Nashville",
    "New York City",
    "Newark",
    "Oklahoma City",
    "Omaha",
    "Philadelphia",
    "Phoenix",
    "Portland",
    "Richmond",
    "Sacramento",
    "Salt Lake City",
    "San Diego",
    "San Francisco",
    "San Jose",
    "Seattle",
    "Secaucus",
    "St Louis",
    "Tampa",
    "Washington DC",
];

/// Check if a city name is valid (case-insensitive).
pub fn is_valid_city(city: &str) -> bool {
    let lower = city.to_lowercase();
    PROTONVPN_US_CITIES
        .iter()
        .any(|c| c.to_lowercase() == lower)
}

/// Get the canonical (properly cased) city name.
pub fn canonical_city(city: &str) -> Option<&'static str> {
    let lower = city.to_lowercase();
    PROTONVPN_US_CITIES
        .iter()
        .find(|c| c.to_lowercase() == lower)
        .copied()
}

/// Suggest similar city names for typo correction.
pub fn suggest_cities(input: &str) -> Vec<&'static str> {
    let lower = input.to_lowercase();
    PROTONVPN_US_CITIES
        .iter()
        .filter(|c| {
            let cl = c.to_lowercase();
            cl.contains(&lower) || lower.contains(&cl) || levenshtein(&lower, &cl) <= 3
        })
        .copied()
        .collect()
}

/// Simple Levenshtein distance for typo suggestions.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let (m, n) = (a_chars.len(), b_chars.len());

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_cities() {
        assert!(is_valid_city("Secaucus"));
        assert!(is_valid_city("secaucus"));
        assert!(is_valid_city("New York City"));
        assert!(is_valid_city("new york city"));
    }

    #[test]
    fn invalid_cities() {
        assert!(!is_valid_city("Atlantis"));
        assert!(!is_valid_city(""));
        assert!(!is_valid_city("Moon Base Alpha"));
    }

    #[test]
    fn canonical_returns_proper_case() {
        assert_eq!(canonical_city("secaucus"), Some("Secaucus"));
        assert_eq!(canonical_city("new york city"), Some("New York City"));
        assert_eq!(canonical_city("nope"), None);
    }

    #[test]
    fn suggestions_for_typos() {
        let suggestions = suggest_cities("seacaucus");
        assert!(suggestions.contains(&"Secaucus"));

        let suggestions = suggest_cities("san");
        assert!(suggestions.contains(&"San Diego"));
        assert!(suggestions.contains(&"San Francisco"));
        assert!(suggestions.contains(&"San Jose"));
    }
}
