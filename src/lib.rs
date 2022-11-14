use std::net::SocketAddr;

use clap::Args;

#[derive(Args)]
pub struct CommonArgs {
    /// Address to listen on
    #[arg(env, long, default_value = "0.0.0.0:8080")]
    pub listen_address: SocketAddr,
}
