//! What every data type's standard methods share (RFC 8620 section 5): the
//! object reader, the `/get` and `/query` results, the ids a get asks for and
//! a query's sort comparator.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::error::Error;

/// One `/query` sort comparator (RFC 8620 section 5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub property: &'static str,
    pub is_ascending: bool,
}

impl Sort {
    /// A `/query`'s `sort` argument: the comparators in order.
    pub(crate) fn values(sort: &[Sort]) -> Value {
        sort.iter()
            .map(|sort| json!({"property": sort.property, "isAscending": sort.is_ascending}))
            .collect()
    }
}

/// A `/query` result (RFC 8620 section 5.5): the ids in sort order
/// from `position`, and the total when it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    pub ids: Vec<String>,
    pub position: u64,
    pub total: Option<u64>,
}

impl QueryResult {
    pub fn read(args: &Value) -> Result<QueryResult, Error> {
        let f = Fields::of(args, "query result")?;
        Ok(QueryResult {
            ids: f.required_strings("ids")?,
            position: f.u64("position")?,
            total: f.opt_u64("total")?,
        })
    }
}

/// The ids a get asks for: listed, or taken from an earlier call's result.
#[derive(Debug, Clone, PartialEq)]
pub enum Ids {
    /// Sent as `ids`.
    Listed(Vec<String>),
    /// Sent as `#ids`; a `result_ref` value.
    Ref(Value),
}

impl Ids {
    /// Writes the ids into a `/get`'s arguments, under `ids` or `#ids`.
    pub(crate) fn insert_into(&self, args: &mut Map<String, Value>) {
        match self {
            Ids::Listed(ids) => args.insert("ids".into(), json!(ids)),
            Ids::Ref(reference) => args.insert("#ids".into(), reference.clone()),
        };
    }
}

/// A get result: the objects found, and the ids asked for that do not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetResult<T> {
    pub list: Vec<T>,
    pub not_found: Vec<String>,
}

/// Reads a `/get` result (RFC 8620 section 5.1): `list` through `read`, and
/// `notFound`.
pub(crate) fn read_get<T>(
    args: &Value,
    read: fn(&Value) -> Result<T, Error>,
) -> Result<GetResult<T>, Error> {
    let f = Fields::of(args, "get result")?;
    let list = f
        .get("list")
        .ok_or_else(|| f.shape("list", "is missing"))?
        .as_array()
        .ok_or_else(|| f.shape("list", "is not an array"))?
        .iter()
        .map(read)
        .collect::<Result<Vec<T>, Error>>()?;
    Ok(GetResult {
        list,
        not_found: f.required_strings("notFound")?,
    })
}

/// An object's properties, read with the type the data type's RFC gives
/// each. A property of the wrong type is `Error::Shape` naming the object and
/// the property; "absent" below means absent or null.
pub(crate) struct Fields<'a> {
    object: &'a Map<String, Value>,
    what: &'static str,
}

impl<'a> Fields<'a> {
    pub(crate) fn of(value: &'a Value, what: &'static str) -> Result<Fields<'a>, Error> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::Shape(format!("{what} is not an object")))?;
        Ok(Fields { object, what })
    }

    pub(crate) fn shape(&self, key: &str, problem: &str) -> Error {
        Error::Shape(format!("{}.{key} {problem}", self.what))
    }

    /// The property, `None` when absent.
    pub(crate) fn get(&self, key: &str) -> Option<&'a Value> {
        self.object.get(key).filter(|value| !value.is_null())
    }

    pub(crate) fn string(&self, key: &str) -> Result<String, Error> {
        self.opt_string(key)?
            .ok_or_else(|| self.shape(key, "is missing"))
    }

    pub(crate) fn opt_string(&self, key: &str) -> Result<Option<String>, Error> {
        self.get(key)
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| self.shape(key, "is not a string"))
            })
            .transpose()
    }

    pub(crate) fn u64(&self, key: &str) -> Result<u64, Error> {
        self.opt_u64(key)?
            .ok_or_else(|| self.shape(key, "is missing"))
    }

    pub(crate) fn opt_u64(&self, key: &str) -> Result<Option<u64>, Error> {
        self.get(key)
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| self.shape(key, "is not an unsigned integer"))
            })
            .transpose()
    }

    pub(crate) fn u64_or_zero(&self, key: &str) -> Result<u64, Error> {
        Ok(self.opt_u64(key)?.unwrap_or(0))
    }

    pub(crate) fn bool(&self, key: &str) -> Result<bool, Error> {
        self.opt_bool(key)?
            .ok_or_else(|| self.shape(key, "is missing"))
    }

    pub(crate) fn opt_bool(&self, key: &str) -> Result<Option<bool>, Error> {
        self.get(key)
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| self.shape(key, "is not a boolean"))
            })
            .transpose()
    }

    /// A list through `read`; empty when absent.
    pub(crate) fn list<T>(
        &self,
        key: &str,
        read: fn(&Value) -> Result<T, Error>,
    ) -> Result<Vec<T>, Error> {
        let Some(value) = self.get(key) else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .ok_or_else(|| self.shape(key, "is not an array"))?
            .iter()
            .map(read)
            .collect()
    }

    /// A string list; empty when absent.
    pub(crate) fn strings(&self, key: &str) -> Result<Vec<String>, Error> {
        if self.get(key).is_none() {
            return Ok(Vec::new());
        }
        self.required_strings(key)
    }

    /// A string list that must be present.
    pub(crate) fn required_strings(&self, key: &str) -> Result<Vec<String>, Error> {
        self.get(key)
            .ok_or_else(|| self.shape(key, "is missing"))?
            .as_array()
            .ok_or_else(|| self.shape(key, "is not an array"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| self.shape(key, "holds a non-string"))
            })
            .collect()
    }

    /// An object of values through `read`, by key; empty when absent.
    pub(crate) fn map<T>(
        &self,
        key: &str,
        read: fn(&Value) -> Result<T, Error>,
    ) -> Result<BTreeMap<String, T>, Error> {
        let Some(value) = self.get(key) else {
            return Ok(BTreeMap::new());
        };
        value
            .as_object()
            .ok_or_else(|| self.shape(key, "is not an object"))?
            .iter()
            .map(|(name, value)| Ok((name.clone(), read(value)?)))
            .collect()
    }

    /// An `Id[Boolean]` or `String[Boolean]` set; empty when absent.
    pub(crate) fn flags(&self, key: &str) -> Result<BTreeMap<String, bool>, Error> {
        let Some(value) = self.get(key) else {
            return Ok(BTreeMap::new());
        };
        value
            .as_object()
            .ok_or_else(|| self.shape(key, "is not an object"))?
            .iter()
            .map(|(name, flag)| {
                flag.as_bool()
                    .map(|flag| (name.clone(), flag))
                    .ok_or_else(|| self.shape(key, "holds a non-boolean"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_query_result_is_read_with_and_without_its_total() {
        assert_eq!(
            QueryResult::read(&json!({
                "accountId": "a1", "queryState": "q1", "canCalculateChanges": false,
                "position": 0, "ids": ["e1", "e2"], "total": 41
            }))
            .unwrap(),
            QueryResult {
                ids: vec!["e1".into(), "e2".into()],
                position: 0,
                total: Some(41),
            }
        );
        assert_eq!(
            QueryResult::read(&json!({"position": 5, "ids": []})).unwrap(),
            QueryResult {
                ids: vec![],
                position: 5,
                total: None,
            }
        );
        for args in [
            json!({"position": 0}),
            json!({"ids": []}),
            json!({"position": 0, "ids": [1]}),
            json!({"position": "0", "ids": []}),
        ] {
            assert!(
                matches!(QueryResult::read(&args), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }
}
