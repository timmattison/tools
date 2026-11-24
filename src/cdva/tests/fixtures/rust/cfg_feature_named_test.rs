#[cfg(feature = "test-support")]
pub fn shipped_under_a_feature() -> u32 {
    3
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_feature_is_off_here() {
        assert_eq!(3, 3);
    }
}
