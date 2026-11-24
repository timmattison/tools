pub fn double(n: i32) -> i32 {
    n * 2
}

#[test]
fn double_doubles() {
    assert_eq!(double(2), 4);
}
