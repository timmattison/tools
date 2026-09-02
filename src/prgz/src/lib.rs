//! Core logic for `prgz` (Progress Gzip): gzip-compress one file and report
//! what the run cost.
//!
//! The binary owns the progress bar and the command line. This library owns the
//! two parts that a test can drive without a terminal: the compression itself,
//! and the closing report.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

use std::time::Duration;

use num_format::ToFormattedString;

pub use num_format::Locale;

/// The characters that end the locale name inside a `$LANG` value. A full
/// `$LANG` value has the shape `language_REGION.codeset@modifier`.
const CODESET_MARKS: [char; 2] = ['.', '@'];

/// The characters that separate the language from the region.
const REGION_MARKS: [char; 2] = ['_', '-'];

/// The count of fraction digits that a formatted number shows. The Go tool
/// shows the same count through `%.2f`.
const FRACTION_DIGITS: usize = 2;

/// The count of seconds in one second. A duration of this length or more reads
/// in seconds.
const SECONDS_PER_SECOND: f64 = 1.0;

/// The count of seconds in one millisecond. A shorter duration of this length
/// or more reads in milliseconds.
const SECONDS_PER_MILLISECOND: f64 = 0.001;

/// The count of seconds in one microsecond. A still shorter duration reads in
/// microseconds.
const SECONDS_PER_MICROSECOND: f64 = 0.000_001;

/// The suffix that marks a duration in seconds.
const SECOND_SUFFIX: &str = "s";

/// The suffix that marks a duration in milliseconds.
const MILLISECOND_SUFFIX: &str = "ms";

/// The suffix that marks a duration in microseconds.
const MICROSECOND_SUFFIX: &str = "\u{b5}s";

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

/// Format a number with two fraction digits, the thousands separator of the
/// locale, and the decimal separator of the locale.
///
/// The two fraction digits match the `%.2f` of the Go tool, and the rounding
/// matches it as well.
///
/// A value that is not finite has no digits to group. Such a value gets the
/// word of the locale instead: `nan` for a value that is not a number, and
/// `infinity` after the sign for an infinite value.
///
/// A finite value with an integer part of more than 39 digits is too large to
/// group. Such a value keeps its two fraction digits, but shows the digits of
/// the integer part without a separator.
pub fn format_float(value: f64, locale: &Locale) -> String {
    if !value.is_finite() {
        return format_not_finite(value, locale);
    }
    let sign = sign_of(value, locale);
    let digits = format!("{:.*}", FRACTION_DIGITS, value.abs());
    let (integer, fraction) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
    let grouped = match integer.parse::<u128>() {
        Ok(number) => number.to_formatted_string(locale),
        Err(_) => integer.to_string(),
    };
    let decimal = locale.decimal();
    format!("{sign}{grouped}{decimal}{fraction}")
}

/// Give the word of the locale for a value that is not finite.
fn format_not_finite(value: f64, locale: &Locale) -> String {
    if value.is_nan() {
        return locale.nan().to_string();
    }
    let sign = sign_of(value, locale);
    let infinity = locale.infinity();
    format!("{sign}{infinity}")
}

/// Give the minus sign of the locale for a negative value, and nothing for a
/// value that is not negative.
fn sign_of(value: f64, locale: &Locale) -> &'static str {
    if value.is_sign_negative() {
        locale.minus_sign()
    } else {
        ""
    }
}

/// Format a duration the way the closing report shows it.
///
/// A duration of one second or more gets seconds and the suffix `s`. A shorter
/// duration of one millisecond or more gets milliseconds and the suffix `ms`.
/// A still shorter duration gets microseconds and the suffix `µs`. Each of the
/// three goes through [`format_float`], thus each one shows two fraction
/// digits in the separators of the locale.
pub fn format_duration(duration: Duration, locale: &Locale) -> String {
    let seconds = duration.as_secs_f64();
    let (value, suffix) = if seconds >= SECONDS_PER_SECOND {
        (seconds, SECOND_SUFFIX)
    } else if seconds >= SECONDS_PER_MILLISECOND {
        (seconds / SECONDS_PER_MILLISECOND, MILLISECOND_SUFFIX)
    } else {
        (seconds / SECONDS_PER_MICROSECOND, MICROSECOND_SUFFIX)
    };
    let number = format_float(value, locale);
    format!("{number}{suffix}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{format_duration, format_float, format_int, locale_from_lang, Locale};

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

    #[test]
    fn format_float_uses_both_separators_of_the_locale() {
        assert_eq!(format_float(1_234_567.891, &Locale::en), "1,234,567.89");
        assert_eq!(format_float(1_234_567.891, &Locale::de), "1.234.567,89");
        assert_eq!(format_float(0.5, &Locale::en), "0.50");
        assert_eq!(format_float(0.5, &Locale::de), "0,50");
    }

    #[test]
    fn format_float_keeps_the_sign_of_a_negative_value() {
        assert_eq!(format_float(-1.789, &Locale::en), "-1.79");
        assert_eq!(format_float(-1.789, &Locale::de), "-1,79");
        assert_eq!(format_float(-1_234.5, &Locale::de), "-1.234,50");
    }

    #[test]
    fn format_float_of_a_value_that_is_not_finite_gives_the_word_of_the_locale() {
        assert_eq!(format_float(f64::NAN, &Locale::en), "NaN");
        assert_eq!(format_float(f64::INFINITY, &Locale::en), "\u{221e}");
        assert_eq!(format_float(f64::NEG_INFINITY, &Locale::en), "-\u{221e}");
        assert_eq!(format_float(f64::NAN, &Locale::de), "NaN");
        assert_eq!(format_float(f64::NEG_INFINITY, &Locale::de), "-\u{221e}");
    }

    #[test]
    fn format_duration_of_one_second_or_more_gives_seconds() {
        assert_eq!(
            format_duration(Duration::from_millis(1_500), &Locale::en),
            "1.50s"
        );
        assert_eq!(
            format_duration(Duration::from_millis(1_500), &Locale::de),
            "1,50s"
        );
        assert_eq!(
            format_duration(Duration::from_secs(1), &Locale::en),
            "1.00s"
        );
        assert_eq!(
            format_duration(Duration::from_secs(3_600), &Locale::en),
            "3,600.00s"
        );
    }

    #[test]
    fn format_duration_below_one_second_gives_milliseconds() {
        assert_eq!(
            format_duration(Duration::from_micros(1_500), &Locale::en),
            "1.50ms"
        );
        assert_eq!(
            format_duration(Duration::from_millis(1), &Locale::en),
            "1.00ms"
        );
        assert_eq!(
            format_duration(Duration::from_millis(999), &Locale::de),
            "999,00ms"
        );
    }

    #[test]
    fn format_duration_below_one_millisecond_gives_microseconds() {
        assert_eq!(
            format_duration(Duration::from_nanos(1_500), &Locale::en),
            "1.50\u{b5}s"
        );
        assert_eq!(
            format_duration(Duration::from_nanos(500), &Locale::en),
            "0.50\u{b5}s"
        );
        assert_eq!(format_duration(Duration::ZERO, &Locale::de), "0,00\u{b5}s");
    }
}
