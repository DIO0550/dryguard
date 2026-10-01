pub fn total(values: &[u32]) -> u32 {
    values.iter().sum()
}

pub fn count(values: &[u32]) -> usize {
    values.iter().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_total_of_two_values_is_their_sum() {
        assert_eq!(total(&[1, 2]), 3);
    }

    #[test]
    fn test_count_of_two_values_is_two() {
        assert_eq!(count(&[1, 2]), 2);
    }
}
