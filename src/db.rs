use std::collections::HashMap;
use std::time::Duration;

use actix::prelude::*;
use anyhow::Context as _;
use clap::Args;
use futures_util::{future, StreamExt, TryStreamExt};
use mongodb::bson::{self, doc, Bson, DateTime, DeserializerOptions, Document};
use mongodb::options::ClientOptions;
use mongodb::Client;
use serde::Deserialize;
use tracing::{debug, error, info, info_span, instrument};

use crate::centrifugo::TagsUpdate;
use crate::errors::{TracedError, TracedErrorContext};
use crate::health::{HealthPing, HealthResult};

type ChangeStreamEvent = mongodb::change_stream::event::ChangeStreamEvent<Document>;

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
pub(crate) async fn create_change_stream(
    config: &Config,
) -> anyhow::Result<impl Stream<Item = ChangeStreamEvent>> {
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
        .context("error starting change stream")?
        .inspect_err(|err| {
            let _entered = info_span!("change stream error inspect").entered();
            error!(kind="change stream error", %err);
        })
        .take_while(|item| future::ready(item.is_ok()))
        .map(|item| item.unwrap());

    info!(status = "success");
    Ok(change_stream)
}

impl TryFrom<ChangeStreamEvent> for TagsUpdate {
    type Error = TracedError;

    fn try_from(value: ChangeStreamEvent) -> Result<Self, Self::Error> {
        let _entered = info_span!("TagsUpdate::try_from::<ChangeStreamEvent>").entered();
        debug!(?value);

        let ns = value
            .ns
            .ok_or_else(|| TracedError::from_msg("missing event `ns` member"))?;
        let collection = ns
            .coll
            .ok_or_else(|| TracedError::from_msg("missing collection"))?;
        let document_key = value
            .document_key
            .ok_or_else(|| TracedError::from_msg("missing document key"))?;
        let updated_id = document_key
            .get_str("_id")
            .map(String::from)
            .context_during("getting updated document id")?;
        let update_description = value
            .update_description
            .ok_or_else(|| TracedError::from_msg("missing update description"))?;
        let deserializer_options = DeserializerOptions::builder().human_readable(false).build();
        let updated_fields: UpdatedFields = bson::from_document_with_options(
            update_description.updated_fields,
            deserializer_options,
        )
        .context_during("deserializing updated fields")?;
        debug!(?updated_fields);
        let tags_update_data = updated_fields
            .data
            .into_iter()
            .filter_map(|(k, v)| {
                k.strip_prefix("data.")
                    .map(|data_key| (data_key.to_owned(), v.into_relaxed_extjson()))
            })
            .collect();

        Ok(Self {
            namespace: ns.db + "-" + collection.as_str(),
            channel_name: updated_id,
            data: tags_update_data,
        })
    }
}

pub(crate) struct DatabaseActor {
    pub(crate) tags_update_recipient: Recipient<TagsUpdate>,
}

impl Actor for DatabaseActor {
    type Context = Context<Self>;
}

impl StreamHandler<ChangeStreamEvent> for DatabaseActor {
    fn handle(&mut self, item: ChangeStreamEvent, _ctx: &mut Self::Context) {
        let _entered = info_span!("handle change stream item").entered();

        let tags_update = match TagsUpdate::try_from(item) {
            Ok(tags_update) => tags_update,
            Err(err) => {
                err.trace_error();
                return;
            }
        };

        if let Err(err) = self.tags_update_recipient.try_send(tags_update) {
            error!(kind = "sending tags update", %err);
        }
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        let _entered = info_span!("DatabaseActor::finished").entered();
        error!(kind = "fatal", err = "change stream finished");
        System::current().stop();
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

#[cfg(test)]
mod tests {
    use super::*;

    mod tags_update {
        use super::*;

        mod from_change_stream_event {
            use super::*;

            #[test]
            fn missing_ns() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "documentKey": {
                        "_id": "anid"
                    },
                    "updateDescription": {
                        "updatedFields": {
                            "updatedAt": DateTime::from_millis(0),
                            "data.first": 9,
                            "data.second": "other"
                        },
                        "removedFields": []
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event);

                assert!(update.is_err());
            }

            #[test]
            fn missing_coll() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "ns": {
                        "db": "testdb"
                    },
                    "documentKey": {
                        "_id": "anid"
                    },
                    "updateDescription": {
                        "updatedFields": {
                            "updatedAt": DateTime::from_millis(0),
                            "data.first": 9,
                            "data.second": "other"
                        },
                        "removedFields": []
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event);

                assert!(update.is_err());
            }

            #[test]
            fn missing_document_key() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "ns": {
                        "db": "testdb",
                        "coll": "testcoll"
                    },
                    "updateDescription": {
                        "updatedFields": {
                            "updatedAt": DateTime::from_millis(0),
                            "data.first": 9,
                            "data.second": "other"
                        },
                        "removedFields": []
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event);

                assert!(update.is_err());
            }

            #[test]
            fn wrong_updated_document_id_type() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "ns": {
                        "db": "testdb",
                        "coll": "testcoll"
                    },
                    "documentKey": {
                        "_id": 42
                    },
                    "updateDescription": {
                        "updatedFields": {
                            "updatedAt": DateTime::from_millis(0),
                            "data.first": 9,
                            "data.second": "other"
                        },
                        "removedFields": []
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event);

                assert!(update.is_err());
            }

            #[test]
            fn missing_update_description() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "ns": {
                        "db": "testdb",
                        "coll": "testcoll"
                    },
                    "documentKey": {
                        "_id": "anid"
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event);

                assert!(update.is_err());
            }

            #[test]
            fn error_deserializing_updated_fields() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "ns": {
                        "db": "testdb",
                        "coll": "testcoll"
                    },
                    "documentKey": {
                        "_id": "anid"
                    },
                    "updateDescription": {
                        "updatedFields": {},
                        "removedFields": []
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event);

                assert!(update.is_err());
            }

            #[test]
            fn success() {
                let document = doc! {
                    "_id": Bson::Null,
                    "operationType": "update",
                    "ns": {
                        "db": "testdb",
                        "coll": "testcoll"
                    },
                    "documentKey": {
                        "_id": "anid"
                    },
                    "updateDescription": {
                        "updatedFields": {
                            "updatedAt": DateTime::from_millis(0),
                            "data.first": 9,
                            "data.second": "other"
                        },
                        "removedFields": []
                    }
                };
                let event: ChangeStreamEvent = bson::from_document(document).unwrap();
                let update = TagsUpdate::try_from(event).unwrap();

                assert_eq!(update.namespace, "testdb-testcoll");
                assert_eq!(update.channel_name, "anid");
                assert_eq!(update.data["first"], 9);
                assert_eq!(update.data["second"], "other");
            }
        }
    }
}
