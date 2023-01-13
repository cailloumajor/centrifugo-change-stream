use std::collections::HashMap;

use mongodb::bson::{Bson, DateTime};
use serde::{ser, Deserialize, Serialize, Serializer};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MongoDBData {
    data: HashMap<String, Bson>,
    source_timestamps: HashMap<String, DateTime>,
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct TagsUpdateData {
    #[serde(flatten)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    values: HashMap<String, Bson>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    ts: HashMap<String, Rfc3339Date>,
}

impl TagsUpdateData {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            values: HashMap::with_capacity(capacity),
            ts: HashMap::with_capacity(capacity),
        }
    }

    pub(crate) fn insert_value(&mut self, k: String, v: Bson) -> Option<Bson> {
        self.values.insert(k, v)
    }

    pub(crate) fn insert_ts(&mut self, k: String, v: DateTime) -> Option<DateTime> {
        self.ts.insert(k, v.into()).map(|d| d.0)
    }
}

impl From<MongoDBData> for TagsUpdateData {
    fn from(value: MongoDBData) -> Self {
        Self {
            values: value.data,
            ts: value
                .source_timestamps
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
        }
    }
}
