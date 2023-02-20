use std::collections::HashMap;

use mongodb::bson::{Bson, DateTime};
use mongodb::Namespace;
use serde::ser::{self, SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{error, info_span};

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

/// Custom change stream event, specialized for updates.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateEvent {
    #[serde(with = "UpdateNamespace")]
    ns: Namespace,
    document_key: DocumentKey,
    update_description: UpdateDescription,
}

impl UpdateEvent {
    pub(crate) fn into_centrifugo(self) -> (String, MongoDBData) {
        let _entered = info_span!("update_event_into_centrifugo").entered();

        let mut data = MongoDBData::with_capacity(self.update_description.updated_fields.len());
        for (key, value) in self.update_description.updated_fields {
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

        let channel = self.ns.to_string() + ":" + self.document_key.id.as_str();

        (channel, data)
    }
}

pub(crate) enum CentrifugoClientRequest {
    TagsUpdate(UpdateEvent),
    Health(oneshot::Sender<bool>),
}

pub(crate) type CurrentDataResponse = Result<Option<MongoDBData>, ()>;

#[derive(Clone, Debug, Deserialize)]
struct Rfc3339Date(DateTime);

impl From<DateTime> for Rfc3339Date {
    fn from(value: DateTime) -> Self {
        Self(value)
    }
}

impl Serialize for Rfc3339Date {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let rfc3339 = self.0.try_to_rfc3339_string().map_err(ser::Error::custom)?;
        serializer.serialize_str(&rfc3339)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MongoDBData {
    val: HashMap<String, Bson>,
    ts: HashMap<String, Rfc3339Date>,
}

impl MongoDBData {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            val: HashMap::with_capacity(capacity),
            ts: HashMap::with_capacity(capacity),
        }
    }

    pub(crate) fn insert_value(&mut self, k: String, v: Bson) -> Option<Bson> {
        self.val.insert(k, v)
    }

    pub(crate) fn insert_timestamp(&mut self, k: String, v: DateTime) -> Option<DateTime> {
        self.ts.insert(k, v.into()).map(|d| d.0)
    }
}

pub(crate) struct EnsureObject<T>(pub Option<T>);

impl<T: Serialize> Serialize for EnsureObject<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.0 {
            Some(data) => data.serialize(serializer),
            None => {
                let map = serializer.serialize_map(Some(0))?;
                map.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod update_event {
        use super::*;

        #[test]
        fn into_centrifugo() {
            let ns = Namespace {
                db: "testdb".to_string(),
                coll: "testcoll".to_string(),
            };
            let document_key = DocumentKey {
                id: "testid".to_string(),
            };
            let updated_fields = HashMap::from([
                ("ignored".to_string(), Bson::Int32(42)),
                ("val.first".to_string(), Bson::Boolean(true)),
                ("val.second".to_string(), Bson::Int32(5646)),
                ("ts.invalid".to_string(), Bson::Boolean(false)),
                (
                    "ts.some".to_string(),
                    Bson::DateTime(DateTime::from_millis(0)),
                ),
                (
                    "ts.other".to_string(),
                    Bson::DateTime(DateTime::from_millis(45000)),
                ),
            ]);
            let update_description = UpdateDescription { updated_fields };
            let update_event = UpdateEvent {
                ns,
                document_key,
                update_description,
            };

            let (channel, data) = update_event.into_centrifugo();

            assert_eq!(channel, "testdb.testcoll:testid");

            assert_eq!(data.val.len(), 2);
            assert_eq!(data.val["first"].to_string(), "true");
            assert_eq!(data.val["second"].to_string(), "5646");

            assert_eq!(data.ts.len(), 2);
            assert_eq!(
                data.ts["some"].0.to_string(),
                "1970-01-01 0:00:00.0 +00:00:00"
            );
            assert_eq!(
                data.ts["other"].0.to_string(),
                "1970-01-01 0:00:45.0 +00:00:00"
            );
        }
    }
}
