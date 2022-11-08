use actix::{Actor, StreamHandler, System};
use anyhow::{Context, Result};
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, LogLevel, Verbosity};

mod centrifugo;
mod db;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    verbose: Verbosity<InfoLevel>,

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

    let centrifugo_client = centrifugo::Client::new(&args.centrifugo);

    let change_stream = system.block_on(db::create_change_stream(&args.mongodb))?;

    system.block_on(async {
        let centrifugo_actor = centrifugo::CentrifugoActor {
            client: centrifugo_client,
        };
        let centrifugo_recipent = centrifugo_actor.start().recipient();
        db::DatabaseActor::create(|ctx| {
            db::DatabaseActor::add_stream(change_stream, ctx);
            db::DatabaseActor {
                tags_update_recipient: centrifugo_recipent,
            }
        })
    });

    system.run().context("error running system")
}
