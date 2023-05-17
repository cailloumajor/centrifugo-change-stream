use anyhow::Context as _;
use axum::Server;
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, Verbosity};
use futures_util::stream::AbortHandle;
use futures_util::StreamExt;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook::low_level::signal_name;
use signal_hook_tokio::Signals;
use tokio::sync::oneshot;
use tracing::{error, info, info_span, instrument, Instrument};
use tracing_log::LogTracer;

use centrifugo_change_stream::CommonArgs;

mod centrifugo;
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

    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,
}

#[instrument(skip_all)]
async fn wait_shutdown(signals: Signals, stopper_rx: oneshot::Receiver<()>) {
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
        _ = stopper_rx => {
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

    let (api_stopper_tx, api_stopper_rx) = oneshot::channel();

    let signals = Signals::new(TERM_SIGNALS).context("error registering termination signals")?;
    let signals_handle = signals.handle();

    let centrifugo_client = centrifugo::Client::new(&args.centrifugo);
    let (centrifugo_requests_tx, centrifugo_client_task) = centrifugo_client.handle_requests();

    let (abort_handle, abort_reg) = AbortHandle::new_pair();

    let mongodb_collection = db::create_collection(&args.mongodb).await?;
    let change_stream_task = mongodb_collection
        .handle_change_stream(centrifugo_requests_tx.clone(), abort_reg, api_stopper_tx)
        .await?;
    let (current_data_tx, current_data_task) = mongodb_collection.handle_current_data();

    let app = http_api::app(
        &mongodb_collection.namespace(),
        centrifugo_requests_tx,
        current_data_tx,
    );
    async move {
        info!(addr = %args.common.listen_address, msg = "start listening");
        if let Err(err) = Server::bind(&args.common.listen_address)
            .serve(app.into_make_service())
            .with_graceful_shutdown(wait_shutdown(signals, api_stopper_rx))
            .await
        {
            error!(kind = "HTTP server", %err);
        }
        info!(status = "terminating");
    }
    .instrument(info_span!("http_server_task"))
    .await;

    abort_handle.abort();
    signals_handle.close();

    let (change_stream_task_result, ..) = tokio::try_join!(
        change_stream_task,
        centrifugo_client_task,
        current_data_task
    )
    .context("error joining tasks")?;
    change_stream_task_result?;

    Ok(())
}
