pub fn describe(n: i32) -> String {
    format!("café {n}")
}

#[rstest]
#[case("café")]
fn describes(input: &str) {
    assert_eq!(describe(1), "café 1");
    let _ = input;
}
