use anyhow::{Context as _, anyhow};
use clap::Parser;

use centrifugo_change_stream::CommonArgs;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let url = format!(
        "http://127.0.0.1:{}/health",
        args.common.listen_address.port()
    );

    let resp = reqwest::get(url).await.context("request error")?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.context("error getting response body")?;
        Err(anyhow!(body))
    }
}
