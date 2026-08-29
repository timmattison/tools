/// Greets whoever asks, in the language of the greeting.
pub fn greet() -> &'static str {
    "こんにちは"
}

pub fn shout() -> String {
    format!("{}!", greet())
}

#[cfg(test)]
mod tests {
    use super::{greet, shout};

    #[test]
    fn greet_is_polite() {
        assert_eq!(greet(), "こんにちは");
    }

    #[test]
    fn shout_is_loud() {
        assert!(shout().ends_with('!'));
    }
}
