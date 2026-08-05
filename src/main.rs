fn main() -> anyhow::Result<()> {
    lanweave::tui::install_panic_hook();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(lanweave::app::run())
}
