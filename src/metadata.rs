use std::marker::PhantomData;

use serde::de::value::MapAccessDeserializer;
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// The parts of yt-dlp's info JSON that veo reads. Unknown fields are skipped
/// while parsing, which keeps memory low for multi-megabyte playlist dumps.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Metadata {
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub upload_date: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub playlist_title: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub playlist_index: Option<f64>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub extractor_key: Option<String>,
    #[serde(rename = "_type", deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(deserialize_with = "entries", skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<Option<Metadata>>>,
    #[serde(deserialize_with = "formats", skip_serializing_if = "Option::is_none")]
    pub formats: Option<Vec<Format>>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_live: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_drm: Option<Value>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub filesize: Option<f64>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub filesize_approx: Option<f64>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub tbr: Option<f64>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub vcodec: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub acodec: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(deserialize_with = "object", skip_serializing_if = "Option::is_none")]
    pub subtitles: Option<Map<String, Value>>,
    #[serde(deserialize_with = "object", skip_serializing_if = "Option::is_none")]
    pub automatic_captions: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Format {
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub vcodec: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub acodec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_drm: Option<Value>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub filesize: Option<f64>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub filesize_approx: Option<f64>,
    #[serde(deserialize_with = "number", skip_serializing_if = "Option::is_none")]
    pub tbr: Option<f64>,
    #[serde(deserialize_with = "text", skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// JavaScript truthiness of a JSON value.
pub fn truthy(value: &Option<Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// `a || b` for optional strings: empty strings are falsy.
pub fn first<'a>(values: &[&'a Option<String>]) -> Option<&'a str> {
    values.iter().find_map(|value| value.as_deref().filter(|text| !text.is_empty()))
}

pub fn positive(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

impl Format {
    pub fn drm(&self) -> bool {
        truthy(&self.has_drm)
    }

    pub fn is_video(&self) -> bool {
        self.vcodec.as_deref() != Some("none")
    }
}

impl Metadata {
    pub fn drm(&self) -> bool {
        truthy(&self.has_drm)
    }

    pub fn live(&self) -> bool {
        truthy(&self.is_live)
    }

    pub fn is_collection(&self) -> bool {
        self.kind.as_deref() == Some("playlist") || self.entries.is_some()
    }

    /// The info dict itself used as a single format (direct media URLs).
    pub fn as_format(&self) -> Format {
        Format {
            height: self.height,
            vcodec: self.vcodec.clone(),
            acodec: self.acodec.clone(),
            has_drm: self.has_drm.clone(),
            ext: self.ext.clone(),
            protocol: self.protocol.clone(),
            filesize: self.filesize,
            filesize_approx: self.filesize_approx,
            tbr: self.tbr,
            url: self.url.clone(),
        }
    }

    pub fn formats(&self) -> &[Format] {
        self.formats.as_deref().unwrap_or(&[])
    }

    pub fn has_entries(&self) -> bool {
        self.entries.as_ref().is_some_and(|entries| !entries.is_empty())
    }
}

pub(crate) fn text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    })
}

pub(crate) fn number<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Number(number) => number.as_f64(),
        _ => None,
    })
}

fn object<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Map<String, Value>>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Object(map) => Some(map),
        _ => None,
    })
}

/// An object element parsed in place; any other JSON value becomes `None`.
/// Avoids buffering huge format lists (with fragment tables) as `Value`s.
struct Lenient<T>(Option<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Visit<T> {
            type Value = Lenient<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(|value| Lenient(Some(value)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Lenient(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(Lenient(None))
            }
        }
        deserializer.deserialize_any(Visit(PhantomData))
    }
}

/// An array of lenient elements; a non-array value yields `None`.
struct LenientList<T>(Option<Vec<Option<T>>>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for LenientList<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Visit<T> {
            type Value = LenientList<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("any JSON value")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(Lenient(item)) = seq.next_element::<Lenient<T>>()? {
                    items.push(item);
                }
                Ok(LenientList(Some(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(LenientList(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(LenientList(None))
            }
        }
        deserializer.deserialize_any(Visit(PhantomData))
    }
}

fn formats<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<Format>>, D::Error> {
    let LenientList(items) = LenientList::<Format>::deserialize(deserializer)?;
    Ok(items.map(|items| items.into_iter().flatten().collect()))
}

fn entries<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Vec<Option<Metadata>>>, D::Error> {
    Ok(LenientList::<Metadata>::deserialize(deserializer)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerates_unusual_field_types() {
        let metadata: Metadata = serde_json::from_str(
            r#"{"id":123,"title":null,"has_drm":"maybe","formats":[{"height":"x","vcodec":null},5],"entries":[null,{"title":"a"}],"other":{"x":1}}"#,
        )
        .unwrap();
        assert_eq!(metadata.id.as_deref(), Some("123"));
        assert!(metadata.drm());
        assert_eq!(metadata.formats().len(), 1);
        assert_eq!(metadata.formats()[0].height, None);
        assert!(metadata.formats()[0].is_video());
        assert_eq!(metadata.entries.as_ref().unwrap().len(), 2);
        assert!(metadata.entries.as_ref().unwrap()[0].is_none());
    }
}
