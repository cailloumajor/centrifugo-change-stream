use actix::{Actor, Arbiter, StreamHandler, System};
use anyhow::{ensure, Context, Result};
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, LogLevel, Verbosity};
use tracing::{info, info_span, Instrument};

use centrifugo_change_stream::CommonArgs;

mod centrifugo;
mod db;
mod errors;
mod health;
mod http_api;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,

    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    centrifugo: centrifugo::Config,

    #[command(flatten)]
    mongodb: db::Config,
}

fn filter_from_verbosity<T>(verbosity: &Verbosity<T>) -> tracing::level_filters::LevelFilter
where
    T: LogLevel,
{
    use log::LevelFilter;
    match verbosity.log_level_filter() {
        LevelFilter::Off => tracing::level_filters::LevelFilter::OFF,
        LevelFilter::Error => tracing::level_filters::LevelFilter::ERROR,
        LevelFilter::Warn => tracing::level_filters::LevelFilter::WARN,
        LevelFilter::Info => tracing::level_filters::LevelFilter::INFO,
        LevelFilter::Debug => tracing::level_filters::LevelFilter::DEBUG,
        LevelFilter::Trace => tracing::level_filters::LevelFilter::TRACE,
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(filter_from_verbosity(&args.verbose))
        .init();

    let system = System::new();

    let centrifugo_addr = system.block_on(async {
        let centrifugo_client = centrifugo::Client::new(&args.centrifugo);
        centrifugo::CentrifugoActor {
            client: centrifugo_client,
        }
        .start()
    });

    let database_addr = system.block_on(async {
        let change_stream = db::create_change_stream(&args.mongodb).await?;
        Ok::<_, anyhow::Error>(db::DatabaseActor::create(|ctx| {
            db::DatabaseActor::add_stream(change_stream, ctx);
            db::DatabaseActor {
                tags_update_recipient: centrifugo_addr.clone().recipient(),
            }
        }))
    })?;

    let health_addr = system.block_on(async {
        let addr = health::HealthService::start_default();
        addr.send(health::Subscribe::new(
            "Centrifugo",
            centrifugo_addr.clone().recipient(),
        ))
        .await
        .context("failed to subscribe Centrifugo actor for health")?;
        addr.send(health::Subscribe::new(
            "database",
            database_addr.clone().recipient(),
        ))
        .await
        .context("failed to subscribe database actor for health")?;
        Ok::<_, anyhow::Error>(addr)
    })?;

    let handler = http_api::handler(health_addr.recipient());

    let sent = Arbiter::current().spawn(
        async move {
            info!(addr = %args.common.listen_address, msg="start listening");
            trillium_tokio::config()
                .with_host(&args.common.listen_address.ip().to_string())
                .with_port(args.common.listen_address.port())
                .without_signals()
                .run_async(handler)
                .await;
        }
        .instrument(info_span!("http_server_task")),
    );
    ensure!(sent, "http server spawning error");

    system.run().context("error running system")
}
