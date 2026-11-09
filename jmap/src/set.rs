//! The generic `/set` pieces (RFC 8620 section 5.3): the result reader, the
//! per-object refusal, and the PatchObject an update carries.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::error::{Error, OneLine};
use crate::mail::Fields;

/// A `SetError` (RFC 8620 section 5.3): why one object was not created or
/// updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetError {
    pub kind: String,
    pub description: Option<String>,
    /// The properties at fault, when the server names them.
    pub properties: Vec<String>,
}

impl SetError {
    fn read(value: &Value) -> Result<SetError, Error> {
        let f = Fields::of(value, "SetError")?;
        Ok(SetError {
            kind: f.string("type")?,
            description: f.opt_string("description")?,
            properties: f.strings("properties")?,
        })
    }
}

impl fmt::Display for SetError {
    /// `<type>[: <description>][ (<properties>)]`, every string from the
    /// server with its control characters escaped.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", OneLine(&self.kind))?;
        if let Some(description) = &self.description {
            write!(f, ": {}", OneLine(description))?;
        }
        if !self.properties.is_empty() {
            write!(f, " (")?;
            for (i, property) in self.properties.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", OneLine(property))?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

/// A `/set` result's creates and updates. `destroyed`, `notDestroyed`,
/// `oldState` and `newState` are not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetResult<C> {
    /// Creation id → what the server set on the new object.
    pub created: BTreeMap<String, C>,
    /// The ids updated; the per-id value the server may send is dropped.
    pub updated: Vec<String>,
    pub not_created: BTreeMap<String, SetError>,
    pub not_updated: BTreeMap<String, SetError>,
}

impl<C> SetResult<C> {
    /// Reads a `/set` result, each `created` value through `created`. An
    /// absent or null map is empty; a map of the wrong shape is
    /// `Error::Shape`.
    pub fn read(
        args: &Value,
        created: fn(&Value) -> Result<C, Error>,
    ) -> Result<SetResult<C>, Error> {
        let f = Fields::of(args, "set result")?;
        let updated = match f.get("updated") {
            None => Vec::new(),
            Some(value) => value
                .as_object()
                .ok_or_else(|| f.shape("updated", "is not an object"))?
                .iter()
                .map(|(id, value)| {
                    if value.is_null() || value.is_object() {
                        Ok(id.clone())
                    } else {
                        Err(f.shape(
                            "updated",
                            "holds a value that is neither null nor an object",
                        ))
                    }
                })
                .collect::<Result<Vec<String>, Error>>()?,
        };
        Ok(SetResult {
            created: f.map("created", created)?,
            updated,
            not_created: f.map("notCreated", SetError::read)?,
            not_updated: f.map("notUpdated", SetError::read)?,
        })
    }
}

/// A PatchObject (RFC 8620 section 5.3): JSON pointer paths, each set to a
/// value; `null` removes. Paths are escaped per RFC 6901.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Patch {
    paths: BTreeMap<String, Value>,
}

impl Patch {
    pub fn new() -> Patch {
        Patch::default()
    }

    /// Adds the object to the mailbox (`true`) or removes it (`null`).
    pub fn mailbox(self, id: &str, member: bool) -> Patch {
        self.flag("mailboxIds", id, member)
    }

    /// Sets the keyword (`true`) or clears it (`null`).
    pub fn keyword(self, keyword: &str, set: bool) -> Patch {
        self.flag("keywords", keyword, set)
    }

    /// Sets a whole property, e.g. `parentId` to `null`.
    pub fn property(mut self, name: &str, value: Value) -> Patch {
        self.paths.insert(escape(name), value);
        self
    }

    pub fn to_value(&self) -> Value {
        Value::Object(
            self.paths
                .iter()
                .map(|(path, value)| (path.clone(), value.clone()))
                .collect::<Map<String, Value>>(),
        )
    }

    fn flag(mut self, property: &str, key: &str, on: bool) -> Patch {
        let value = if on { Value::Bool(true) } else { Value::Null };
        self.paths
            .insert(format!("{property}/{}", escape(key)), value);
        self
    }
}

/// One reference token of a JSON pointer (RFC 6901 section 3).
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id_only(value: &Value) -> Result<String, Error> {
        Fields::of(value, "created")?.string("id")
    }

    #[test]
    fn a_set_result_reads_each_map_and_absent_ones_as_empty() {
        let result = SetResult::read(
            &json!({
                "accountId": "a1", "oldState": "s1", "newState": "s2",
                "created": {"k1": {"id": "e9"}},
                "updated": {"e1": null, "e2": {"size": 7}},
                "notCreated": {"k2": {"type": "overQuota"}},
                "notUpdated": {"e3": {
                    "type": "invalidProperties",
                    "description": "no mailbox",
                    "properties": ["mailboxIds"]
                }}
            }),
            id_only,
        )
        .unwrap();
        assert_eq!(
            result,
            SetResult {
                created: BTreeMap::from([("k1".to_owned(), "e9".to_owned())]),
                updated: vec!["e1".into(), "e2".into()],
                not_created: BTreeMap::from([(
                    "k2".to_owned(),
                    SetError {
                        kind: "overQuota".into(),
                        description: None,
                        properties: vec![],
                    }
                )]),
                not_updated: BTreeMap::from([(
                    "e3".to_owned(),
                    SetError {
                        kind: "invalidProperties".into(),
                        description: Some("no mailbox".into()),
                        properties: vec!["mailboxIds".into()],
                    }
                )]),
            }
        );
        let empty = SetResult::read(&json!({"accountId": "a1", "updated": null}), id_only).unwrap();
        assert!(empty.created.is_empty() && empty.updated.is_empty());
        assert!(empty.not_created.is_empty() && empty.not_updated.is_empty());
    }

    #[test]
    fn a_set_result_of_the_wrong_shape_is_malformed() {
        for args in [
            json!([]),
            json!({"created": []}),
            json!({"created": {"k1": {}}}),
            json!({"updated": ["e1"]}),
            json!({"updated": {"e1": true}}),
            json!({"notCreated": {"k1": {}}}),
            json!({"notUpdated": {"e1": {"type": 7}}}),
            json!({"notUpdated": {"e1": {"type": "x", "description": 7}}}),
            json!({"notUpdated": {"e1": {"type": "x", "properties": "mailboxIds"}}}),
            json!({"notUpdated": {"e1": {"type": "x", "properties": [7]}}}),
        ] {
            assert!(
                matches!(SetResult::read(&args, id_only), Err(Error::Shape(_))),
                "{args}"
            );
        }
    }

    #[test]
    fn a_set_error_renders_its_type_description_and_properties() {
        let bare = SetError {
            kind: "notFound".into(),
            description: None,
            properties: vec![],
        };
        assert_eq!(bare.to_string(), "notFound");
        let full = SetError {
            kind: "invalidProperties".into(),
            description: Some("bad value".into()),
            properties: vec!["mailboxIds".into(), "keywords".into()],
        };
        assert_eq!(
            full.to_string(),
            "invalidProperties: bad value (mailboxIds, keywords)"
        );
        let injected = SetError {
            kind: "bad\ntype".into(),
            description: Some("line\r\u{1b}[31m".into()),
            properties: vec!["a\u{0}b".into()],
        };
        let text = injected.to_string();
        assert!(!text.chars().any(char::is_control), "{text}");
        assert_eq!(text, r"bad\ntype: line\r\u{1b}[31m (a\u{0}b)");
    }

    #[test]
    fn a_patch_sets_and_removes_by_path() {
        let patch = Patch::new()
            .mailbox("m1", true)
            .mailbox("m2", false)
            .keyword("$seen", true)
            .keyword("$flagged", false);
        assert_eq!(
            patch.to_value(),
            json!({
                "mailboxIds/m1": true,
                "mailboxIds/m2": null,
                "keywords/$seen": true,
                "keywords/$flagged": null
            })
        );
        assert_eq!(
            Patch::new()
                .property("parentId", Value::Null)
                .property("name", json!("Receipts"))
                .to_value(),
            json!({"parentId": null, "name": "Receipts"})
        );
        assert_eq!(Patch::new().to_value(), json!({}));
    }

    #[test]
    fn a_patch_path_is_escaped_as_a_json_pointer() {
        assert_eq!(
            Patch::new()
                .keyword("a/b~c", true)
                .mailbox("~1/", false)
                .to_value(),
            json!({"keywords/a~1b~0c": true, "mailboxIds/~01~1": null})
        );
    }
}
