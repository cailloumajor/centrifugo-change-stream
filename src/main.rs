use anyhow::Context as _;
use arcstr::ArcStr;
use axum::Server;
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, Verbosity};
use futures_util::StreamExt;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook::low_level::signal_name;
use signal_hook_tokio::Signals;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, info_span, instrument, Instrument};
use tracing_log::LogTracer;

use centrifugo_change_stream::CommonArgs;

mod centrifugo;
mod channel;
mod db;
mod http_api;
mod level_filter;
mod model;

use level_filter::VerbosityLevelFilter;

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
async fn wait_shutdown(signals: Signals, shutdown_token: CancellationToken) {
    info!(status = "started");
    let mut signals_stream = signals.map(|signal| signal_name(signal).unwrap_or("unknown"));
    tokio::select! {
        maybe_signal = signals_stream.next() => {
            if let Some(signal) = maybe_signal {
                info!(msg = "received signal", reaction = "shutting down", signal);
            } else {
                error!(err = "signal stream has been exhausted", reaction = "shutting down");
            }
        }
        _ = shutdown_token.cancelled() => {
            info!(msg = "received stop", reaction = "shutting down");
        }
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_max_level(VerbosityLevelFilter::from(&args.verbose))
        .init();

    LogTracer::init_with_filter(args.verbose.log_level_filter())?;

    let signals = Signals::new(TERM_SIGNALS).context("error registering termination signals")?;
    let signals_handle = signals.handle();

    let centrifugo_client = centrifugo::Client::new(&args.centrifugo);
    let (tags_update_channel, tags_update_task) =
        centrifugo_client.handle_tags_update(args.tags_update_buffer.into());
    let (health_channel, health_task) = centrifugo_client.handle_health();

    let shutdown_token = CancellationToken::new();

    let mongodb_collection = db::create_collection(&args.mongodb).await?;
    let change_stream_task = mongodb_collection
        .handle_change_stream(tags_update_channel, shutdown_token.clone())
        .await?;
    let (current_data_channel, current_data_task) = mongodb_collection.handle_current_data();

    let app = http_api::app(http_api::AppState {
        namespace_prefix: ArcStr::from(mongodb_collection.namespace() + ":"),
        health_channel,
        current_data_channel,
    });
    let cloned_token = shutdown_token.clone();
    async move {
        info!(addr = %args.common.listen_address, msg = "start listening");
        if let Err(err) = Server::bind(&args.common.listen_address)
            .serve(app.into_make_service())
            .with_graceful_shutdown(wait_shutdown(signals, cloned_token))
            .await
        {
            error!(kind = "HTTP server", %err);
        }
        info!(status = "terminating");
    }
    .instrument(info_span!("http_server_task"))
    .await;

    signals_handle.close();
    shutdown_token.cancel();

    let (change_stream_task_result, ..) = tokio::try_join!(
        change_stream_task,
        tags_update_task,
        health_task,
        current_data_task,
    )
    .context("error joining tasks")?;
    change_stream_task_result?;

    Ok(())
}
