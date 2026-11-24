pub fn feature_flagged() -> bool {
    cfg!(feature = "x")
}

#[cfg(all(test, feature = "x"))]
mod gated {
    #[test]
    fn flagged() {
        assert!(super::feature_flagged());
    }
}
