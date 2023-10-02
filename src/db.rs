use std::time::Duration;

use anyhow::{anyhow, Context as _};
use clap::Args;
use futures_util::StreamExt;
use mongodb::bson::{doc, Document};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, info_span, instrument, Instrument};

use crate::centrifugo::TagsUpdateChannel;
use crate::channel::{roundtrip_channel, RoundtripSender};
use crate::model::{MongoDBData, UpdateEvent};

const APP_NAME: &str = concat!(env!("CARGO_PKG_NAME"), " (", env!("CARGO_PKG_VERSION"), ")");
const SEND_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Args)]
#[group(skip)]
pub(crate) struct Config {
    /// URI of MongoDB server
    #[arg(env, long, default_value = "mongodb://mongo")]
    mongodb_uri: String,

    /// MongoDB database
    #[arg(env, long)]
    mongodb_database: String,

    /// MongoDB collection
    #[arg(env, long)]
    mongodb_collection: String,
}

pub(crate) type CurrentDataChannel = RoundtripSender<String, Result<Option<MongoDBData>, ()>>;

pub(crate) struct MongoDBCollection(Collection<Document>);

impl MongoDBCollection {
    pub(crate) fn namespace(&self) -> String {
        self.0.namespace().to_string()
    }

    pub(crate) async fn handle_change_stream(
        &self,
        tags_update_channel: TagsUpdateChannel,
        shutdown_token: CancellationToken,
    ) -> anyhow::Result<JoinHandle<anyhow::Result<()>>> {
        let pipeline = [doc! { "$match": { "operationType": "update" } }];
        let mut change_stream = self
            .0
            .watch(pipeline, None)
            .await
            .context("error starting change stream")?
            .with_type::<UpdateEvent>()
            .take_until(shutdown_token.clone().cancelled_owned())
            .boxed();
        let handle = tokio::spawn(
            async move {
                info!(status = "started");

                while let Some(item) = change_stream.next().await {
                    let event = match item {
                        Ok(event) => event,
                        Err(err) => {
                            error!(kind = "stream item error", %err);
                            shutdown_token.cancel();
                            return Err(anyhow!("broken change stream"));
                        }
                    };
                    if let Err(err) = tags_update_channel.send_timeout(event, SEND_TIMEOUT).await {
                        error!(kind = "tags update channel sending", %err);
                    }
                }

                info!(status = "terminating");
                Ok(())
            }
            .instrument(info_span!("change_stream_handler")),
        );

        Ok(handle)
    }

    pub(crate) fn handle_current_data(&self) -> (CurrentDataChannel, JoinHandle<()>) {
        let collection = self.0.clone_with_type::<MongoDBData>();
        let (tx, mut rx) = roundtrip_channel(1);

        let task = tokio::spawn(
            async move {
                info!(status = "started");

                while let Some((document_id, response_tx)) = rx.recv().await {
                    debug!(%document_id);

                    let filter = doc! { "_id": document_id };
                    let found = collection.find_one(filter, None).await.map_err(|err| {
                        error!(kind = "finding document", %err);
                    });
                    if response_tx.send(found).is_err() {
                        error!(kind = "response channel sending");
                    }
                }

                info!(status = "terminating");
            }
            .instrument(info_span!("current_data_handler")),
        );

        (tx, task)
    }
}

#[instrument(skip_all)]
pub(crate) async fn create_collection(config: &Config) -> anyhow::Result<MongoDBCollection> {
    let mut options = ClientOptions::parse(&config.mongodb_uri)
        .await
        .context("error parsing connection string URI")?;
    options.app_name = String::from(APP_NAME).into();
    options.server_selection_timeout = Duration::from_secs(2).into();
    let client = Client::with_options(options).context("error creating the client")?;
    let collection = client
        .database(&config.mongodb_database)
        .collection(&config.mongodb_collection);

    info!(status = "success");
    Ok(MongoDBCollection(collection))
}
