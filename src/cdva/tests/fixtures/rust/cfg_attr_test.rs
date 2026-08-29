#[cfg_attr(test, allow(dead_code))]
fn production_helper() -> u32 {
    2
}

#[test]
fn production_helper_is_two() {
    assert_eq!(production_helper(), 2);
}
