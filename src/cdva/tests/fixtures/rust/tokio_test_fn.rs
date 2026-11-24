pub async fn fetch() -> u32 {
    7
}

#[tokio::test]
async fn fetch_returns_seven() {
    assert_eq!(fetch().await, 7);
}
