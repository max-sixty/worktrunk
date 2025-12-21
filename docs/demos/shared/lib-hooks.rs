pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

pub fn subtract(a: i32, b: i32) -> i32 {
    a - b
}

pub fn multiply(a: i32, b: i32) -> i32 {
    a * b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operations() {
        assert_eq!(add(2, 3), 5);
        assert_eq!(subtract(5, 3), 2);
        assert_eq!(multiply(3, 4), 12);
    }
}
