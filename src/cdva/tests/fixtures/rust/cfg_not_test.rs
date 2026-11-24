pub fn shared() -> u32 {
    1
}

#[cfg(not(test))]
mod production_only {
    pub fn real_work() -> u32 {
        super::shared()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_is_one() {
        assert_eq!(super::shared(), 1);
    }
}
