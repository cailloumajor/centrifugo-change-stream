use std::time::Duration;

use anyhow::{anyhow, Context as _};
use clap::Args;
use futures_util::stream::{AbortRegistration, Abortable};
use futures_util::StreamExt;
use mongodb::bson::{doc, Document};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection};
use tokio::sync::mpsc::{self, Sender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, info_span, instrument, Instrument};

use crate::model::{CentrifugoClientRequest, CurrentDataResponse, MongoDBData, UpdateEvent};

const APP_NAME: &str = concat!(env!("CARGO_PKG_NAME"), " (", env!("CARGO_PKG_VERSION"), ")");

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

pub(crate) struct MongoDBCollection(Collection<Document>);

impl MongoDBCollection {
    pub(crate) fn namespace(&self) -> String {
        self.0.namespace().to_string()
    }

    pub(crate) async fn handle_change_stream(
        &self,
        centrifugo_request: Sender<CentrifugoClientRequest>,
        abort_reg: AbortRegistration,
        stopper: oneshot::Sender<()>,
    ) -> anyhow::Result<JoinHandle<anyhow::Result<()>>> {
        let pipeline = [doc! { "$match": { "operationType": "update" } }];
        let change_stream = self
            .0
            .watch(pipeline, None)
            .await
            .context("error starting change stream")?
            .with_type::<UpdateEvent>();
        let mut abortable_stream = Abortable::new(change_stream, abort_reg);
        let handle = tokio::spawn(
            async move {
                info!(status = "started");

                while let Some(item) = abortable_stream.next().await {
                    let event = match item {
                        Ok(event) => event,
                        Err(err) => {
                            error!(kind = "stream item error", %err);
                            if stopper.send(()).is_err() {
                                error!(kind = "stop channel sending");
                            }
                            return Err(anyhow!("broken change stream"));
                        }
                    };
                    let request = CentrifugoClientRequest::TagsUpdate(event);
                    if let Err(err) = centrifugo_request.try_send(request) {
                        error!(kind = "request channel sending", %err);
                    }
                }

                info!(status = "terminating");
                Ok(())
            }
            .instrument(info_span!("change_stream_handler")),
        );

        Ok(handle)
    }

    pub(crate) fn handle_current_data(
        &self,
    ) -> (
        mpsc::Sender<(String, oneshot::Sender<CurrentDataResponse>)>,
        JoinHandle<()>,
    ) {
        let collection = self.0.clone_with_type::<MongoDBData>();
        let (tx, mut rx) = mpsc::channel::<(String, oneshot::Sender<CurrentDataResponse>)>(1);

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
