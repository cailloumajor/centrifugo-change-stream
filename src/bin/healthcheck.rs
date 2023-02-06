use anyhow::{anyhow, Context, Result};
use clap::Parser;
use trillium_client::Conn;
use trillium_tokio::{block_on, TcpConnector};

use centrifugo_change_stream::CommonArgs;

type ClientConn = Conn<'static, TcpConnector>;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let url = format!(
        "http://127.0.0.1:{}/health",
        args.common.listen_address.port()
    );

    let mut resp = block_on(async { ClientConn::get(url.as_str()).await })?;

    let status = resp.status().context("missing status code")?;

    if status.is_success() {
        Ok(())
    } else {
        let body = block_on(resp.response_body().read_string())?;
        Err(anyhow!(body))
    }
}
