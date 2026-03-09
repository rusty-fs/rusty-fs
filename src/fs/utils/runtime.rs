use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// Global Tokio runtime for async operations
/// Uses OnceLock to ensure only one runtime is created across all instances
static GLOBAL_RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Get or create the global Tokio runtime
pub fn runtime() -> &'static Runtime {
    GLOBAL_RUNTIME.get_or_init(|| {
        Runtime::new().expect("Failed to create global Tokio runtime")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_is_reused() {
        let rt1 = runtime();
        let rt2 = runtime();
        // Both should be the same instance
        assert_eq!(rt1 as *const _ as usize, rt2 as *const _ as usize);
    }
}
