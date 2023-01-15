use std::collections::HashMap;
use std::time::Duration;

use actix::prelude::*;
use anyhow::Context as _;
use clap::Args;
use futures_util::stream::{AbortRegistration, Abortable};
use futures_util::{future, FutureExt, StreamExt, TryStreamExt};
use mongodb::bson::{doc, Bson};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection, Namespace};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, info_span, instrument, Instrument};

use crate::centrifugo::TagsUpdate;
use crate::health::{HealthPing, HealthResult};
use crate::model::MongoDBData;

type DataCollection = Collection<MongoDBData>;

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

/// Custom change stream event, specialized for updates.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateEvent {
    #[serde(with = "UpdateNamespace")]
    ns: Namespace,
    document_key: DocumentKey,
    update_description: UpdateDescription,
}

#[derive(Deserialize)]
#[serde(remote = "Namespace")]
struct UpdateNamespace {
    db: String,
    coll: String,
}

#[derive(Debug, Deserialize)]
struct DocumentKey {
    #[serde(rename = "_id")]
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDescription {
    updated_fields: HashMap<String, Bson>,
}

#[instrument(skip_all)]
pub(crate) async fn create_collection<T>(config: &Config) -> anyhow::Result<Collection<T>> {
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
    Ok(collection)
}

#[instrument(skip_all)]
pub(crate) async fn create_change_stream<C>(
    collection: &Collection<C>,
    abort_reg: AbortRegistration,
    term_sender: Sender<&'static str>,
) -> anyhow::Result<impl Stream<Item = UpdateEvent>>
where
    C: DeserializeOwned + Send + Sync + Unpin,
{
    let pipeline = [doc! { "$match": { "operationType": "update" } }];
    let change_stream = collection
        .watch(pipeline, None)
        .await
        .context("error starting change stream")?
        .with_type::<UpdateEvent>();
    let returned_stream = Abortable::new(change_stream, abort_reg)
        .inspect_err(move |err| {
            let _entered = info_span!("change_stream_error_inspect").entered();
            error!(?err);
            let term_sender = term_sender.clone();
            Arbiter::current().spawn(async move {
                term_sender
                    .send("change stream error")
                    .await
                    .expect("termination channel sending error");
            });
        })
        .take_while(|item| future::ready(item.is_ok()))
        .map(|item| item.unwrap());

    info!(status = "success");
    Ok(returned_stream)
}

pub(crate) struct DatabaseActor {
    pub(crate) collection: DataCollection,
    pub(crate) tags_update_recipient: Recipient<TagsUpdate>,
}

impl Actor for DatabaseActor {
    type Context = Context<Self>;
}

impl StreamHandler<UpdateEvent> for DatabaseActor {
    fn handle(&mut self, item: UpdateEvent, _ctx: &mut Self::Context) {
        let _entered = info_span!("handle change stream item").entered();
        debug!(?item);

        let namespace = item.ns.to_string();
        let channel_name = item.document_key.id;

        let mut data = MongoDBData::with_capacity(item.update_description.updated_fields.len());
        for (key, value) in item.update_description.updated_fields {
            if let Some(data_key) = key.strip_prefix("val.") {
                data.insert_value(data_key.into(), value);
            } else if let Some(ts_key) = key.strip_prefix("ts.") {
                let Bson::DateTime(date_time) = value else {
                    error!(kind = "not a BSON DateTime", field=key, ?value);
                    continue;
                };
                data.insert_timestamp(ts_key.into(), date_time);
            }
        }

        let tags_update = TagsUpdate {
            namespace,
            channel_name,
            data,
        };

        if let Err(err) = self.tags_update_recipient.try_send(tags_update) {
            error!(kind = "sending tags update", %err);
        }
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        let _entered = info_span!("DatabaseActor::finished").entered();
        info!(msg = "change stream finished");
    }
}

#[derive(Clone)]
pub(crate) enum CurrentDataError {
    BadNamespace,
    MongoDB,
}

pub(crate) type CurrentDataResponse = Result<Option<MongoDBData>, CurrentDataError>;

#[derive(Debug, Message)]
#[rtype(result = "CurrentDataResponse")]
pub(crate) struct CurrentDataRequest {
    namespace: String,
    id: String,
}

impl CurrentDataRequest {
    pub(crate) fn new(namespace: String, id: String) -> Self {
        Self { namespace, id }
    }
}

impl Handler<CurrentDataRequest> for DatabaseActor {
    type Result = ResponseFuture<CurrentDataResponse>;

    fn handle(&mut self, msg: CurrentDataRequest, _ctx: &mut Self::Context) -> Self::Result {
        let collection = self.collection.clone();

        async move {
            debug!(?msg);

            if msg.namespace != collection.namespace().to_string() {
                error!(kind = "bad namespace", got = msg.namespace);
                return Err(CurrentDataError::BadNamespace);
            }

            let filter = doc! { "_id": msg.id };
            let current_data = collection.find_one(filter, None).await.map_err(|err| {
                error!(kind = "finding document", %err);
                CurrentDataError::MongoDB
            })?;
            debug!(event = "data from MongoDB", ?current_data);

            Ok(current_data)
        }
        .instrument(info_span!("current_data_handler"))
        .boxed()
    }
}

impl Handler<HealthPing> for DatabaseActor {
    type Result = HealthResult;

    fn handle(&mut self, _msg: HealthPing, ctx: &mut Self::Context) -> Self::Result {
        let state = ctx.state();
        if state == ActorState::Running {
            Ok(())
        } else {
            Err(format!("actor is in `{:?}` state", state))
        }
    }
}
