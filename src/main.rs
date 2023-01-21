// Since fly.io is a one core machine, we only need the current thread
#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    async_main().await
}

async fn async_main() -> color_eyre::Result<()> {
    Ok(())
}
