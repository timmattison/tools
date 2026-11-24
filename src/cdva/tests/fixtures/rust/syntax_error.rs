pub fn broken() -> i32 {
    let x = 1;
    x
}
}

#[cfg(test)]
mod tests {
    #[test]
    fn always() {
        assert!(1 + 1 == 2);
    }
}
