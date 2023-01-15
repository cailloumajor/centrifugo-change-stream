use std::collections::HashMap;

use mongodb::bson::{Bson, DateTime};
use serde::ser::{self, SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

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
