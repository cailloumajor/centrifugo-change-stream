use anyhow::{anyhow, Context as _};
use clap::Parser;
use trillium_client::Conn;
use trillium_tokio::TcpConnector;

use centrifugo_change_stream::CommonArgs;

type ClientConn = Conn<'static, TcpConnector>;

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

    let mut resp = ClientConn::get(url.as_str()).await?;

    let status = resp.status().context("missing status code")?;

    if status.is_success() {
        Ok(())
    } else {
        let body = resp.response_body().read_string().await?;
        Err(anyhow!(body))
    }
}
