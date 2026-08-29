pub fn ok() -> bool {
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn ok_is_true() {
        assert!(super::ok());
    }
}
