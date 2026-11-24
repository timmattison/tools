/// Returns the two party emoji, each of which takes four bytes.
pub fn party() -> &'static str {
    "🎉🎊"
}

#[cfg(test)]
mod tests {
    #[test]
    fn party_is_festive() {
        // 日本語のコメント — three bytes a character, and an em dash besides.
        assert_eq!(super::party(), "🎉🎊");
    }
}
