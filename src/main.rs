fn main() -> anyhow::Result<()> {
    install_panic_hook();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(lanweave::app::run())
}

fn install_panic_hook() {
    let _ = std::panic::take_hook();
}
