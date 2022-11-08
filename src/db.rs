use std::collections::HashMap;
use std::time::Duration;

use actix::prelude::*;
use anyhow::Context as _;
use clap::Args;
use mongodb::bson::{self, doc, Bson, DateTime, DeserializerOptions, Document};
use mongodb::change_stream::event::ChangeStreamEvent;
use mongodb::options::ClientOptions;
use mongodb::Client;
use serde::Deserialize;
use tracing::{debug, error, info, info_span, instrument};

use crate::centrifugo::TagsUpdate;

type ChangeStream = mongodb::change_stream::ChangeStream<ChangeStreamEvent<Document>>;
type ChangeStreamItem = <ChangeStream as Stream>::Item;

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

#[derive(Debug, Deserialize)]
struct UpdatedFields {
    #[serde(rename = "updatedAt")]
    _updated_at: DateTime,

    #[serde(flatten)]
    data: HashMap<String, Bson>,
}

#[instrument(skip_all)]
pub(crate) async fn create_change_stream(config: &Config) -> anyhow::Result<ChangeStream> {
    let mut options = ClientOptions::parse(&config.mongodb_uri)
        .await
        .context("error parsing connection string URI")?;
    options.app_name = String::from(APP_NAME).into();
    options.server_selection_timeout = Duration::from_secs(2).into();
    let client = Client::with_options(options).context("error creating the client")?;
    let pipeline = [doc! { "$match": { "operationType": "update" } }];
    let change_stream = client
        .database(&config.mongodb_database)
        .collection::<Document>(&config.mongodb_collection)
        .watch(pipeline, None)
        .await
        .context("error starting change stream")?;

    info!(status = "success");
    Ok(change_stream)
}

pub(crate) struct DatabaseActor {
    pub(crate) tags_update_recipient: Recipient<TagsUpdate>,
}

impl Actor for DatabaseActor {
    type Context = Context<Self>;
}

impl StreamHandler<ChangeStreamItem> for DatabaseActor {
    fn handle(&mut self, item: ChangeStreamItem, _ctx: &mut Self::Context) {
        let _entered = info_span!("handle change stream item").entered();
        let event = match item {
            Ok(ev) => ev,
            Err(err) => {
                error!(kind = "change stream item", %err);
                return;
            }
        };
        debug!(?event);
        let ns = match event.ns {
            Some(ns) => ns,
            None => {
                error!(msg = "missing event `ns` member");
                return;
            }
        };
        let collection = match ns.coll {
            Some(coll) => coll,
            None => {
                error!(msg = "missing collection");
                return;
            }
        };
        let document_key = match event.document_key {
            Some(doc_key) => doc_key,
            None => {
                error!(msg = "missing document key");
                return;
            }
        };
        let updated_id = match document_key.get_str("_id") {
            Ok(id) => id.to_owned(),
            Err(err) => {
                error!(kind = "getting updated document id", %err);
                return;
            }
        };
        let update_description = match event.update_description {
            Some(desc) => desc,
            None => {
                error!(msg = "missing update description");
                return;
            }
        };
        let deserializer_options = DeserializerOptions::builder().human_readable(false).build();
        let updated_fields: UpdatedFields = match bson::from_document_with_options(
            update_description.updated_fields,
            deserializer_options,
        ) {
            Ok(updated) => updated,
            Err(err) => {
                error!(kind = "deserializing updated fields", %err);
                return;
            }
        };
        debug!(?updated_fields);
        let tags_update_data = updated_fields
            .data
            .into_iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("data.")
                    .map(|data_key| (data_key.to_owned(), v.into_relaxed_extjson()))
            })
            .collect();

        let tags_update = TagsUpdate {
            namespace: ns.db + "-" + collection.as_str(),
            channel_name: updated_id,
            data: tags_update_data,
        };

        if let Err(err) = self.tags_update_recipient.try_send(tags_update) {
            error!(kind = "sending tags update", %err);
        }
    }
}