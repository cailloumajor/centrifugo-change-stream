use actix::{Actor, Arbiter, StreamHandler, System};
use anyhow::{ensure, Context, Result};
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, LogLevel, Verbosity};
use futures_util::{stream, stream::AbortHandle, StreamExt};
use signal_hook::{consts::TERM_SIGNALS, low_level::signal_name};
use signal_hook_tokio::Signals;
use tokio::sync::mpsc::{self, Receiver};
use tracing::{info, info_span, instrument, Instrument};
use tracing_log::LogTracer;
use trillium_tokio::tokio_stream::wrappers::ReceiverStream;
use trillium_tokio::Stopper;

use centrifugo_change_stream::CommonArgs;

mod centrifugo;
mod db;
mod errors;
mod health;
mod http_api;
mod model;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    #[command(flatten)]
    centrifugo: centrifugo::Config,

    #[command(flatten)]
    mongodb: db::Config,

    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,
}

fn filter_from_verbosity<T>(verbosity: &Verbosity<T>) -> tracing::level_filters::LevelFilter
where
    T: LogLevel,
{
    use tracing_log::log::LevelFilter;
    match verbosity.log_level_filter() {
        LevelFilter::Off => tracing::level_filters::LevelFilter::OFF,
        LevelFilter::Error => tracing::level_filters::LevelFilter::ERROR,
        LevelFilter::Warn => tracing::level_filters::LevelFilter::WARN,
        LevelFilter::Info => tracing::level_filters::LevelFilter::INFO,
        LevelFilter::Debug => tracing::level_filters::LevelFilter::DEBUG,
        LevelFilter::Trace => tracing::level_filters::LevelFilter::TRACE,
    }
}

#[instrument(skip_all)]
async fn handle_terminate(
    signals: Signals,
    receiver: Receiver<&'static str>,
    abort_handle: AbortHandle,
    stopper: Stopper,
) {
    let signals_stream = signals.map(|signal| signal_name(signal).unwrap_or("unknown"));
    let receiver_stream = ReceiverStream::new(receiver);
    let mut events_stream = stream::select(signals_stream, receiver_stream);
    while let Some(event) = events_stream.next().await {
        info!(event, msg = "received termination event");
        abort_handle.abort();
        stopper.stop();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(filter_from_verbosity(&args.verbose))
        .init();

    LogTracer::init_with_filter(args.verbose.log_level_filter())?;

    let (term_sender, term_receiver) = mpsc::channel(1);

    let system = System::new();

    let centrifugo_addr = system.block_on(async {
        let centrifugo_client = centrifugo::Client::new(&args.centrifugo);
        centrifugo::CentrifugoActor {
            client: centrifugo_client,
        }
        .start()
    });

    let (abort_handle, abort_reg) = AbortHandle::new_pair();
    let database_addr = system.block_on(async {
        let collection = db::create_collection(&args.mongodb).await?;
        let change_stream =
            db::create_change_stream(&collection, abort_reg, term_sender.clone()).await?;
        Ok::<_, anyhow::Error>(db::DatabaseActor::create(|ctx| {
            db::DatabaseActor::add_stream(change_stream, ctx);
            db::DatabaseActor {
                collection,
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

    let trillium_stopper = Stopper::new();

    let signals = system
        .block_on(async { Signals::new(TERM_SIGNALS) })
        .context("error registering termination signals")?;
    let signals_handle = signals.handle();
    let sent = Arbiter::current().spawn(handle_terminate(
        signals,
        term_receiver,
        abort_handle,
        trillium_stopper.clone(),
    ));
    ensure!(sent, "error spawning signals handler");

    let handler = http_api::handler(health_addr.recipient(), database_addr.recipient());
    system.block_on(
        async move {
            info!(addr = %args.common.listen_address, msg="start listening");
            trillium_tokio::config()
                .with_host(&args.common.listen_address.ip().to_string())
                .with_port(args.common.listen_address.port())
                .without_signals()
                .with_stopper(trillium_stopper)
                .run_async(handler)
                .await;
        }
        .instrument(info_span!("http_server_task")),
    );

    signals_handle.close();

    Ok(())
}
