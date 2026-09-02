//! Strict JSON object reading shared by control message decoding.
//!
//! One serde visitor turns a JSON object into borrowed key/value entries.
//! Duplicate detection happens at field access so parser errors never embed
//! peer-controlled text, and values stay unparsed until a message schema asks
//! for them. Entry storage stays bounded by the body itself because every
//! key and value is a borrowed slice of it.

use std::borrow::Cow;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

use super::message::MessageError;

/// A decoded JSON object with borrowed entries and strict field access.
pub struct StrictObject<'a> {
    entries: Vec<(Cow<'a, str>, &'a RawValue)>,
}

impl<'a> StrictObject<'a> {
    /// Parses `body` as exactly one JSON object with no trailing value.
    pub fn parse(body: &'a [u8]) -> Result<Self, MessageError> {
        serde_json::from_slice(body).map_err(|error| match error.classify() {
            serde_json::error::Category::Data => MessageError::NotAnObject,
            _ => MessageError::Malformed,
        })
    }

    /// Returns whether `field` appears at least once.
    pub fn contains(&self, field: &str) -> bool {
        self.entries.iter().any(|(key, _)| key == field)
    }

    /// Reads a required string field.
    pub fn required_str(&self, field: &'static str) -> Result<Cow<'a, str>, MessageError> {
        self.optional_str(field)?
            .ok_or(MessageError::MissingField(field))
    }

    /// Reads an optional string field.
    pub fn optional_str(&self, field: &'static str) -> Result<Option<Cow<'a, str>>, MessageError> {
        self.scalar(field, "a string")
    }

    /// Reads a required boolean field.
    pub fn required_bool(&self, field: &'static str) -> Result<bool, MessageError> {
        self.scalar(field, "true or false")?
            .ok_or(MessageError::MissingField(field))
    }

    /// Reads a required unsigned integer bounded by `max`.
    ///
    /// The JSON number grammar plus `serde_json` reject floats, exponents,
    /// signs, and values beyond `u64`; the fixed limit is checked afterwards.
    pub fn required_uint(&self, field: &'static str, max: u64) -> Result<u64, MessageError> {
        let value = self
            .scalar::<u64>(field, "an unsigned integer")?
            .ok_or(MessageError::MissingField(field))?;
        if value > max {
            return Err(MessageError::InvalidValue { field });
        }
        Ok(value)
    }

    /// Reads a required field holding an array of JSON objects.
    pub fn required_objects(
        &self,
        field: &'static str,
    ) -> Result<Vec<StrictObject<'a>>, MessageError> {
        let value = self.one(field)?.ok_or(MessageError::MissingField(field))?;
        serde_json::from_str(value.get()).map_err(|_| MessageError::WrongType {
            field,
            expected: "an array of objects",
        })
    }

    /// Rejects every field outside `allowed` once a schema consumed its fields.
    pub fn finish(self, allowed: &[&str]) -> Result<(), MessageError> {
        for (key, _) in &self.entries {
            if !allowed.contains(&key.as_ref()) {
                return Err(MessageError::UnknownField);
            }
        }
        Ok(())
    }

    /// Returns the single value of `field`, rejecting repeats.
    fn one(&self, field: &'static str) -> Result<Option<&'a RawValue>, MessageError> {
        let mut values = self.values(field).into_iter();
        match values.next() {
            None => Ok(None),
            Some(value) => {
                if values.next().is_some() {
                    Err(MessageError::DuplicateField(field))
                } else {
                    Ok(Some(value))
                }
            }
        }
    }

    fn values(&self, field: &str) -> Vec<&'a RawValue> {
        self.entries
            .iter()
            .filter(|(key, _)| key == field)
            .map(|(_, value)| *value)
            .collect()
    }

    /// Decodes one scalar of JSON type `T` so wrong shapes fail per field.
    fn scalar<T: Deserialize<'a>>(
        &self,
        field: &'static str,
        expected: &'static str,
    ) -> Result<Option<T>, MessageError> {
        match self.one(field)? {
            None => Ok(None),
            Some(value) => serde_json::from_str(value.get())
                .map(Some)
                .map_err(|_| MessageError::WrongType { field, expected }),
        }
    }
}

impl<'de> Deserialize<'de> for StrictObject<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = StrictObject<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                // Duplicate and unknown keys are kept and judged later by the
                // message schema, so no peer text ever enters an error here.
                let mut entries = Vec::new();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    let value = map.next_value::<&'de RawValue>()?;
                    entries.push((key, value));
                }
                Ok(StrictObject { entries })
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}
