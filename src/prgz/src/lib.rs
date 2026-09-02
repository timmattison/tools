//! Core logic for `prgz` (Progress Gzip): gzip-compress one file and report
//! what the run cost.
//!
//! The binary owns the progress bar and the command line. This library owns the
//! two parts that a test can drive without a terminal: the compression itself,
//! and the closing report.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

use num_format::ToFormattedString;

pub use num_format::Locale;

/// The characters that end the locale name inside a `$LANG` value. A full
/// `$LANG` value has the shape `language_REGION.codeset@modifier`.
const CODESET_MARKS: [char; 2] = ['.', '@'];

/// The characters that separate the language from the region.
const REGION_MARKS: [char; 2] = ['_', '-'];

/// Resolve the number formatting locale from a `$LANG` value.
///
/// The function first removes the codeset and the modifier from the value, so
/// `de_DE.UTF-8@euro` becomes `de_DE`. It then looks for the remainder in the
/// locale table. If the table does not hold the remainder, the function looks
/// for the language part alone, so `de_DE` becomes `de`. If the table holds
/// neither name, the function answers `Locale::en`. The values `C`, `POSIX`,
/// and an empty value name no locale, thus they all answer `Locale::en`.
pub fn locale_from_lang(lang: &str) -> Locale {
    let name = lang
        .split_once(CODESET_MARKS)
        .map_or(lang, |(head, _)| head);
    if let Ok(locale) = Locale::from_name(name) {
        return locale;
    }
    let language = name.split_once(REGION_MARKS).map_or(name, |(head, _)| head);
    Locale::from_name(language).unwrap_or(Locale::en)
}

/// Format an integer with the thousands separator of the locale.
///
/// The locale also sets the size of each group of digits. Most locales put the
/// digits in groups of three. Some locales, such as the locales of India, use
/// other group sizes.
pub fn format_int(value: u64, locale: &Locale) -> String {
    value.to_formatted_string(locale)
}

#[cfg(test)]
mod tests {
    use super::{format_int, locale_from_lang, Locale};

    #[test]
    fn a_lang_with_a_codeset_gives_the_language_of_the_lang() {
        assert_eq!(locale_from_lang("de_DE.UTF-8").name(), "de");
        assert_eq!(locale_from_lang("en_US.UTF-8").name(), "en");
    }

    #[test]
    fn a_lang_with_a_modifier_gives_the_language_of_the_lang() {
        assert_eq!(locale_from_lang("de_DE.UTF-8@euro").name(), "de");
        assert_eq!(locale_from_lang("de_DE@euro").name(), "de");
    }

    #[test]
    fn a_lang_that_the_table_holds_gives_that_entry() {
        assert_eq!(locale_from_lang("de_CH").name(), "de-CH");
        assert_eq!(locale_from_lang("de_CH.UTF-8").name(), "de-CH");
    }

    #[test]
    fn a_lang_that_names_no_locale_gives_english() {
        assert_eq!(locale_from_lang("C").name(), "en");
        assert_eq!(locale_from_lang("POSIX").name(), "en");
        assert_eq!(locale_from_lang("").name(), "en");
        assert_eq!(locale_from_lang("zz_ZZ.UTF-8").name(), "en");
    }

    #[test]
    fn format_int_uses_the_separator_of_the_locale() {
        assert_eq!(format_int(1_234_567, &Locale::en), "1,234,567");
        assert_eq!(format_int(1_234_567, &Locale::de), "1.234.567");
        assert_eq!(format_int(0, &Locale::de), "0");
    }
}
