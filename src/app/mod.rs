//! Application event loop. Owns startup, shutdown, and dispatch. Other modules
//! produce events into the loop; the loop updates state and renders the TUI.
//!
//! The reducer owns application state and the terminal lifecycle guard is
//! managed here.

pub mod error;

pub async fn run() -> anyhow::Result<()> {
    println!("lanweave dev build: no TUI yet");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run;

    #[tokio::test]
    async fn run_returns_ok() {
        assert!(run().await.is_ok());
    }
}
