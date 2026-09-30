//! Restricted Improver v2 candidate content (plan §9, §9.1; E14 increment 1).
//!
//! The first round opens exactly one mechanism class: the exploration policy.
//! The content is a closed, strictly typed document. Unknown fields, unknown or
//! protected mechanism classes and duplicate keys are rejected; a class that is
//! not open yet, has no consumer, or belongs to the control plane is refused by
//! name before the typed parse runs.
//!
//! This content is deliberately not a `ResolvedBundle`, `Strategy` or
//! `compile_bundle` input and never reaches a world context signature (plan
//! §7.4.1, V094.c): it only influences a run through the policy a world freezes
//! at registration, which `decide_elastic` then consumes.
use crate::strategy::ElasticPolicyV1;
use crate::{Error, Result, fingerprint};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

pub const IMPROVER_CONTENT_V2: &str = "rsia.improver_content.v2";

/// Upper bound on the serialized content. The open class is a few hundred
/// bytes; the bound only keeps untrusted candidate bytes from being parsed at
/// arbitrary size.
pub const IMPROVER_CONTENT_MAX_BYTES: usize = 8 * 1024;

/// The mechanism a candidate changes, tagged by `class`. Only the exploration
/// policy is open; every other class is refused before it can be parsed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImproverMechanismV2 {
    ExplorationPolicy { policy: ElasticPolicyV1 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImproverContentV2 {
    pub schema_version: String,
    pub mechanism: ImproverMechanismV2,
}

impl ImproverContentV2 {
    /// The built-in fixed mechanism (I0): the default policy in a content
    /// wrapper, so that a candidate can be compared with it by digest.
    pub fn builtin_default() -> Self {
        Self {
            schema_version: IMPROVER_CONTENT_V2.into(),
            mechanism: ImproverMechanismV2::ExplorationPolicy {
                policy: ElasticPolicyV1::default(),
            },
        }
    }

    /// The exploration policy this content carries.
    pub fn exploration_policy(&self) -> &ElasticPolicyV1 {
        let ImproverMechanismV2::ExplorationPolicy { policy } = &self.mechanism;
        policy
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != IMPROVER_CONTENT_V2 {
            return Err(Error::Invalid("unsupported improver content schema".into()));
        }
        self.exploration_policy().validate()
    }

    /// Digest of the typed value. It is computed from the serialized struct
    /// and so does not depend on the key order of the bytes it was parsed from.
    pub fn content_digest(&self) -> Result<String> {
        fingerprint(self)
    }

    /// Strictly parses candidate bytes. The mechanism class is read first so a
    /// class that is not open, has no consumer or is protected control plane
    /// gets its own refusal; only then is the document deserialized with all
    /// unknown fields denied, and the policy bounds are checked.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > IMPROVER_CONTENT_MAX_BYTES {
            return Err(Error::Invalid(format!(
                "improver content exceeds {IMPROVER_CONTENT_MAX_BYTES} bytes"
            )));
        }
        let StrictValue(value) = serde_json::from_slice(bytes).map_err(|error| {
            Error::Invalid(format!("improver content is not strict JSON: {error}"))
        })?;
        classify_mechanism_class(&value)?;
        let content: Self = serde_json::from_value(value)
            .map_err(|error| Error::Invalid(format!("improver content rejected: {error}")))?;
        content.validate()?;
        Ok(content)
    }
}

fn classify_mechanism_class(value: &serde_json::Value) -> Result<()> {
    let class = value
        .get("mechanism")
        .and_then(|mechanism| mechanism.get("class"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Invalid("improver content needs a mechanism class".into()))?;
    match class {
        "exploration_policy" => Ok(()),
        "generation_strategy" | "optimizer_guidance" => Err(Error::Invalid(
            "mechanism class not open in this round (plan §9.1)".into(),
        )),
        "acquisition_policy" => Err(Error::Invalid("acquisition_policy: no consumer".into())),
        "caps" | "exploration_caps" | "simulation_profile" | "holdout" | "oracle" | "grader"
        | "budget" | "approval" | "goal" => Err(Error::Forbidden),
        _ => Err(Error::Invalid("unknown improver mechanism class".into())),
    }
}

/// A JSON value that refuses duplicate object keys at every depth.
/// `serde_json::Value` silently keeps the last duplicate, which would let one
/// document show the classifier a different `class` than the typed parser.
struct StrictValue(serde_json::Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON value without duplicate object keys")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(value.into()))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(value.into()))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(value.into()))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<StrictValue, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| StrictValue(number.into()))
                    .ok_or_else(|| E::custom("non-finite number"))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(value.to_owned().into()))
            }

            fn visit_string<E: de::Error>(
                self,
                value: String,
            ) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(value.into()))
            }

            fn visit_unit<E: de::Error>(self) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(serde_json::Value::Null))
            }

            fn visit_none<E: de::Error>(self) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(serde_json::Value::Null))
            }

            fn visit_some<D2>(self, deserializer: D2) -> std::result::Result<StrictValue, D2::Error>
            where
                D2: Deserializer<'de>,
            {
                StrictValue::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<StrictValue, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(StrictValue(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(StrictValue(serde_json::Value::Array(items)))
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<StrictValue, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut object = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if object.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate key {key:?}")));
                    }
                    let StrictValue(value) = map.next_value()?;
                    object.insert(key, value);
                }
                Ok(StrictValue(serde_json::Value::Object(object)))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}
