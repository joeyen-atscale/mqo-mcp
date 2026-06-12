const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Renders a sparkline of block characters for the given values.
/// Returns at least 1 char if `values` is non-empty.
/// If all values are equal (or only 1 value), returns flat (▄).
pub fn sparkline(values: &[f64]) -> String {
    if values.is_empty() {
        return String::new();
    }
    if values.len() == 1 {
        return BLOCKS[3].to_string();
    }

    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;

    values
        .iter()
        .map(|&v| {
            if range == 0.0 {
                BLOCKS[3]
            } else {
                let normalized = (v - min) / range;
                let idx = (normalized * 7.0).round() as usize;
                BLOCKS[idx.min(7)]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_empty() {
        assert_eq!(sparkline(&[]), "");
    }

    #[test]
    fn single_returns_mid_block() {
        assert_eq!(sparkline(&[42.0]), "▄");
    }

    #[test]
    fn two_values_min_max() {
        let s = sparkline(&[0.0, 1.0]);
        assert_eq!(s.chars().count(), 2);
    }

    #[test]
    fn all_equal_returns_flat() {
        let s = sparkline(&[5.0, 5.0, 5.0]);
        assert!(s.chars().all(|c| c == '▄'));
    }
}
