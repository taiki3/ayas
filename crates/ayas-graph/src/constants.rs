/// Sentinel node name representing the graph entry point.
pub const START: &str = "__start__";

/// Sentinel node name representing the graph exit point.
pub const END: &str = "__end__";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinel_values() {
        assert_eq!(START, "__start__");
        assert_eq!(END, "__end__");
        assert_ne!(START, END);
    }
}
