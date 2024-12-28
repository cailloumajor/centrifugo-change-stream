use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, Verbosity};
use futures_util::StreamExt;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook::low_level::signal_name;
use signal_hook_tokio::Signals;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, info_span, instrument, Instrument};
use tracing_log::LogTracer;

use centrifugo_change_stream::CommonArgs;

mod centrifugo;
mod channel;
mod db;
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

    /// Size of the tags update channel buffer
    #[arg(env, long, value_parser = clap::value_parser!(u8).range(1..), default_value = "10")]
    tags_update_buffer: u8,

    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,
}

#[instrument(skip_all)]
async fn handle_signals(signals: Signals, shutdown_token: CancellationToken) {
    info!(status = "started");
    let mut signals_stream = signals.map(|signal| signal_name(signal).unwrap_or("unknown"));
    while let Some(signal) = signals_stream.next().await {
        info!(msg = "received signal", reaction = "shutting down", signal);
        shutdown_token.cancel();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(args.verbose.tracing_level())
        .init();

    LogTracer::init_with_filter(args.verbose.log_level_filter())?;

    let shutdown_token = CancellationToken::new();

    let signals = Signals::new(TERM_SIGNALS).context("error registering termination signals")?;
    let signals_handle = signals.handle();
    let signals_task = tokio::spawn(handle_signals(signals, shutdown_token.clone()));

    let centrifugo_client = centrifugo::Client::new(&args.centrifugo);
    let (tags_update_channel, tags_update_task) =
        centrifugo_client.handle_tags_update(args.tags_update_buffer.into());
    let (health_channel, health_task) = centrifugo_client.handle_health();

    let mongodb_collection = db::create_collection(&args.mongodb).await?;
    let change_stream_task = mongodb_collection
        .handle_change_stream(tags_update_channel, shutdown_token.clone())
        .await?;
    let (current_data_channel, current_data_task) = mongodb_collection.handle_current_data();

    let app = http_api::app(http_api::AppState {
        namespace_prefix: Arc::from(mongodb_collection.namespace() + ":"),
        health_channel,
        current_data_channel,
    });
    async move {
        let listener = match TcpListener::bind(&args.common.listen_address).await {
            Ok(listener) => {
                info!(addr = %args.common.listen_address, msg = "listening");
                listener
            }
            Err(err) => {
                error!(kind="TCP listen", %err);
                return;
            }
        };
        if let Err(err) = axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(shutdown_token.cancelled_owned())
            .await
        {
            error!(kind = "HTTP server", %err);
        }
        info!(status = "terminating");
    }
    .instrument(info_span!("http_server_task"))
    .await;

    signals_handle.close();

    let (change_stream_task_result, ..) = tokio::try_join!(
        change_stream_task,
        signals_task,
        tags_update_task,
        health_task,
        current_data_task,
    )
    .context("error joining tasks")?;
    change_stream_task_result?;

    Ok(())
}
