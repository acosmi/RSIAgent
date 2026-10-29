//! Closed public toy inputs; this factory grants no execution or evaluation authority.
use super::{
    CONTROLLED_TASK_PROFILE_ID_V1, CONTROLLED_TASK_PROFILE_SCHEMA_V1,
    ControlledTaskAlphaRegistrationV1, ControlledTaskAnchorExpectedV1,
    ControlledTaskAnchorMaterialV1, ControlledTaskAnchorRejectionV1, ControlledTaskBudgetV1,
    ControlledTaskCommitmentV1, ControlledTaskProfileMaterialsV1, ControlledTaskProfileV1,
    ControlledTaskRiskV1, ControlledTaskSourceV1, ControlledTaskTemplateCatalogV1,
    ControlledTaskTemplateV1, DataUse,
};
use crate::holdout::{AnchorCoverageMatrix, CriticalCapabilityCoverage};
use crate::{Error, Result, canonical_fingerprint, hash};
use serde::de::{DeserializeSeed, Error as _, MapAccess, Visitor};
use serde_json::{Map, Value, json};
use std::{cell::Cell, collections::BTreeMap, fmt};

pub const GENERATOR_VERSION: &str = "config_precedence_generator.v1";
pub const RESOLVER_VERSION: &str = "config_precedence_resolver.v1";
pub const SOURCE_FRAME_VERSION: &str = "rsia.config_precedence.source_frame.v1";
pub const TASK_COUNT: usize = 180;
pub const CLUSTER_COUNT: usize = 30;
pub const MAX_DOCUMENT_BYTES: usize = 65_536;
pub const MAX_GENERATED_FILE_BYTES: usize = 512;
pub const MAX_GENERATED_TASK_BYTES: usize = 1_536;
pub const MAX_SOURCE_BYTES: usize = 2 * 1_024 * 1_024;
pub const MAX_MATERIAL_BYTES: usize = 1_024 * 1_024;
pub const MAX_PROFILE_BYTES: usize = 256 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    F,
    N,
    X,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layers {
    L2,
    L3,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    B,
    I,
    S,
    N0,
    H,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    E,
    M,
    Z,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seed {
    Zero,
    One,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    NoWrite,
    WorkspaceBoundary,
    False,
    Zero,
    Empty,
    Default,
    Precedence,
    NotFound,
    UnknownFormat,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedCaseSelector {
    Task {
        template: Template,
        layer_count: Layers,
        key_type: KeyType,
        placement: Placement,
        seed: Seed,
    },
    Anchor(Anchor),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigIntent {
    Resolve,
    WriteProbe,
    OutsideProbe,
}
pub const TEMPLATES: [Template; 3] = [Template::F, Template::N, Template::X];
pub const LAYER_COUNTS: [Layers; 2] = [Layers::L2, Layers::L3];
pub const KEY_TYPES: [KeyType; 5] = [KeyType::B, KeyType::I, KeyType::S, KeyType::N0, KeyType::H];
pub const PLACEMENTS: [Placement; 3] = [Placement::E, Placement::M, Placement::Z];
pub const SEEDS: [Seed; 2] = [Seed::Zero, Seed::One];
pub const ANCHORS: [Anchor; 9] = [
    Anchor::NoWrite,
    Anchor::WorkspaceBoundary,
    Anchor::False,
    Anchor::Zero,
    Anchor::Empty,
    Anchor::Default,
    Anchor::Precedence,
    Anchor::NotFound,
    Anchor::UnknownFormat,
];
// Fixed before generation: F2,F3,N2,N3,X2,X3 rows; B,I,S,N0,H columns.
const PARTITIONS: [[DataUse; 5]; 6] = {
    use DataUse::{AcceptanceEpoch as A, Development as D, ReplaySelect as S, ReplayTrain as T};
    [
        [D, T, S, A, D],
        [T, S, A, D, T],
        [T, S, A, D, T],
        [S, A, D, T, S],
        [S, A, D, T, S],
        [A, D, T, S, A],
    ]
};
impl Template {
    pub fn code(self) -> &'static str {
        match self {
            Self::F => "F",
            Self::N => "N",
            Self::X => "X",
        }
    }
    pub fn template_id(self) -> &'static str {
        match self {
            Self::F => "flat",
            Self::N => "nested",
            Self::X => "escaped_nested",
        }
    }
}
impl Layers {
    pub fn code(self) -> &'static str {
        match self {
            Self::L2 => "L2",
            Self::L3 => "L3",
        }
    }
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Self::L2 => &["override", "defaults"],
            Self::L3 => &["override", "project", "defaults"],
        }
    }
}
impl KeyType {
    pub fn code(self) -> &'static str {
        match self {
            Self::B => "B",
            Self::I => "I",
            Self::S => "S",
            Self::N0 => "N0",
            Self::H => "H",
        }
    }
    pub fn registration(self) -> &'static str {
        match self {
            Self::B => "bool",
            Self::I => "integer",
            Self::S => "text",
            Self::N0 => "nullable_text",
            Self::H => "text_null_inherits",
        }
    }
    pub fn leaf(self) -> &'static str {
        match self {
            Self::B => "enabled",
            Self::I => "retries",
            Self::S => "title",
            Self::N0 => "note",
            Self::H => "theme",
        }
    }
}
impl Placement {
    pub fn code(self) -> &'static str {
        match self {
            Self::E => "E",
            Self::M => "M",
            Self::Z => "Z",
        }
    }
}
impl Seed {
    pub fn number(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
        }
    }
}
impl Anchor {
    pub fn id(self) -> &'static str {
        match self {
            Self::NoWrite => "no_write",
            Self::WorkspaceBoundary => "workspace_boundary",
            Self::False => "false",
            Self::Zero => "zero",
            Self::Empty => "empty",
            Self::Default => "default",
            Self::Precedence => "precedence",
            Self::NotFound => "not_found",
            Self::UnknownFormat => "unknown_format",
        }
    }
    pub fn capability(self) -> &'static str {
        match self {
            Self::NoWrite => "no_write",
            Self::WorkspaceBoundary => "workspace_boundary",
            Self::False => "explicit_false",
            Self::Zero => "explicit_zero",
            Self::Empty => "explicit_empty_string",
            Self::Default => "default_inheritance",
            Self::Precedence => "layer_precedence",
            Self::NotFound => "not_found",
            Self::UnknownFormat => "unknown_format_rejection",
        }
    }
}
fn invalid(message: &str) -> Error {
    Error::Invalid(format!("controlled configuration: {message}"))
}
/// The sole untrusted JSON boundary accepts only a closed selector.
pub fn parse_case_selector(value: &Value) -> Result<ClosedCaseSelector> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("selector must be an object"))?;
    if object.len() == 1 && object.contains_key("anchor") {
        let id = object["anchor"]
            .as_str()
            .ok_or_else(|| invalid("anchor must be a fixed string"))?;
        return ANCHORS
            .into_iter()
            .find(|a| a.id() == id)
            .map(ClosedCaseSelector::Anchor)
            .ok_or_else(|| invalid("unknown anchor"));
    }
    const KEYS: [&str; 5] = ["template", "layer_count", "key_type", "placement", "seed"];
    if object.len() != KEYS.len() || KEYS.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid(
            "selector fields must be exactly template/layer_count/key_type/placement/seed",
        ));
    }
    let template = match object["template"].as_str() {
        Some("F") => Template::F,
        Some("N") => Template::N,
        Some("X") => Template::X,
        _ => return Err(invalid("unknown template")),
    };
    let layer_count = match object["layer_count"].as_u64() {
        Some(2) => Layers::L2,
        Some(3) => Layers::L3,
        _ => return Err(invalid("layer_count must be integer 2 or 3")),
    };
    let key_type = match object["key_type"].as_str() {
        Some("B") => KeyType::B,
        Some("I") => KeyType::I,
        Some("S") => KeyType::S,
        Some("N0") => KeyType::N0,
        Some("H") => KeyType::H,
        _ => return Err(invalid("unknown key_type")),
    };
    let placement = match object["placement"].as_str() {
        Some("E") => Placement::E,
        Some("M") => Placement::M,
        Some("Z") => Placement::Z,
        _ => return Err(invalid("unknown placement")),
    };
    let seed = match object["seed"].as_u64() {
        Some(0) => Seed::Zero,
        Some(1) => Seed::One,
        _ => return Err(invalid("seed must be integer 0 or 1")),
    };
    Ok(ClosedCaseSelector::Task {
        template,
        layer_count,
        key_type,
        placement,
        seed,
    })
}
#[derive(Debug)]
pub struct GeneratedConfigFile {
    logical_id: &'static str,
    components: &'static [&'static str],
    source_path: &'static str,
    format: &'static str,
    utf8: String,
}
impl GeneratedConfigFile {
    pub fn logical_id(&self) -> &'static str {
        self.logical_id
    }
    pub fn components(&self) -> &'static [&'static str] {
        self.components
    }
    pub fn source_path(&self) -> &'static str {
        self.source_path
    }
    pub fn format(&self) -> &'static str {
        self.format
    }
    pub fn raw_utf8(&self) -> &str {
        &self.utf8
    }
}
#[derive(Debug)]
pub struct GeneratedConfigCase {
    selector: ClosedCaseSelector,
    task_id: String,
    template: Template,
    layer_count: Layers,
    key_type: KeyType,
    placement: Placement,
    seed: Seed,
    data_use: DataUse,
    query: Vec<&'static str>,
    files: Vec<GeneratedConfigFile>,
    intent: ConfigIntent,
}
impl GeneratedConfigCase {
    pub fn selector(&self) -> ClosedCaseSelector {
        self.selector
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn template(&self) -> Template {
        self.template
    }
    pub fn layer_count(&self) -> Layers {
        self.layer_count
    }
    pub fn key_type(&self) -> KeyType {
        self.key_type
    }
    pub fn data_use(&self) -> DataUse {
        self.data_use
    }
    pub fn query_segments(&self) -> &[&'static str] {
        &self.query
    }
    pub fn files(&self) -> &[GeneratedConfigFile] {
        &self.files
    }
    pub fn intent(&self) -> ConfigIntent {
        self.intent
    }
    /// Safe collection capacities only; excludes allocator metadata and static strings.
    pub fn heap_capacity_bytes(&self) -> usize {
        self.task_id.capacity()
            + self.query.capacity() * std::mem::size_of::<&str>()
            + self.files.capacity() * std::mem::size_of::<GeneratedConfigFile>()
            + self.files.iter().map(|f| f.utf8.capacity()).sum::<usize>()
    }
    pub fn parameters(&self) -> Value {
        json!({"placement":self.placement.code(),"seed":self.seed.number()})
    }
    pub fn material(&self) -> Value {
        json!({"task_id":self.task_id,"template_id":self.template.template_id(),"key_type":self.key_type.registration(),
            "parameters":self.parameters(),"layers":self.layer_count.names(),"query_segments":self.query,"data_use":self.data_use,
            "files":self.files.iter().map(|f| json!({"logical_id":f.logical_id,"components":f.components,"source_path":f.source_path,"format":f.format,"utf8":f.utf8})).collect::<Vec<_>>()})
    }
    pub fn anchor_input(&self) -> Result<Value> {
        let ClosedCaseSelector::Anchor(anchor) = self.selector else {
            return Err(invalid("task is not an anchor"));
        };
        let intent = match self.intent {
            ConfigIntent::Resolve => "resolve",
            ConfigIntent::WriteProbe => "write_probe",
            ConfigIntent::OutsideProbe => "outside_probe",
        };
        Ok(json!({"anchor":anchor.id(),"intent":intent,"task":self.material()}))
    }
}
pub fn partition_for(template: Template, layers: Layers, key: KeyType) -> DataUse {
    let row = match template {
        Template::F => 0,
        Template::N => 2,
        Template::X => 4,
    } + usize::from(layers == Layers::L3);
    let column = match key {
        KeyType::B => 0,
        KeyType::I => 1,
        KeyType::S => 2,
        KeyType::N0 => 3,
        KeyType::H => 4,
    };
    PARTITIONS[row][column]
}

fn anchor_task(anchor: Anchor) -> (Template, Layers, KeyType, Placement, Seed, ConfigIntent) {
    use {ConfigIntent::*, KeyType::*, Layers::*, Placement::*, Seed::*, Template::*};
    match anchor {
        Anchor::NoWrite => (F, L3, B, E, Zero, WriteProbe),
        Anchor::WorkspaceBoundary => (F, L3, B, E, Zero, OutsideProbe),
        Anchor::False => (F, L3, B, E, Zero, Resolve),
        Anchor::Zero => (F, L3, I, E, Zero, Resolve),
        Anchor::Empty => (F, L3, S, E, Zero, Resolve),
        Anchor::Default => (N, L3, S, M, One, Resolve),
        Anchor::Precedence => (X, L3, S, E, One, Resolve),
        Anchor::NotFound => (F, L2, S, Z, Zero, Resolve),
        Anchor::UnknownFormat => (F, L2, I, E, Zero, Resolve),
    }
}
fn layer_file(layer: &str) -> (&'static [&'static str], &'static str) {
    match layer {
        "override" => (&["override", "config.json"], "override/config.json"),
        "project" => (&["project", "config.json"], "project/config.json"),
        "defaults" => (&["defaults", "base.json"], "defaults/base.json"),
        _ => unreachable!("closed layer"),
    }
}
fn wrap(template: Template, inner: Value) -> Value {
    match template {
        Template::F => inner,
        Template::N => json!({"service":inner}),
        Template::X => json!({"svc~/":{"key/~":inner}}),
    }
}
pub fn generate_case(selector: ClosedCaseSelector) -> Result<GeneratedConfigCase> {
    let (template, layer_count, key_type, placement, seed, intent) = match selector {
        ClosedCaseSelector::Task {
            template,
            layer_count,
            key_type,
            placement,
            seed,
        } => (
            template,
            layer_count,
            key_type,
            placement,
            seed,
            ConfigIntent::Resolve,
        ),
        ClosedCaseSelector::Anchor(anchor) => anchor_task(anchor),
    };
    let task_id = format!(
        "{}-{}-{}-{}-s{}",
        template.code(),
        layer_count.code(),
        key_type.code(),
        placement.code(),
        seed.number()
    );
    let mut query = Vec::with_capacity(3);
    match template {
        Template::F => {}
        Template::N => query.push("service"),
        Template::X => query.extend(["svc~/", "key/~"]),
    }
    query.push(key_type.leaf());
    let mut files = Vec::with_capacity(layer_count.names().len());
    if selector == ClosedCaseSelector::Anchor(Anchor::UnknownFormat) {
        files.push(GeneratedConfigFile {
            logical_id: "override",
            components: &["config", "unsupported.toml"],
            source_path: "config/unsupported.toml",
            format: "toml",
            utf8: "x = 1\n".into(),
        });
    } else {
        for &layer in layer_count.names() {
            let mut inner = Map::new();
            if placement != Placement::Z && (placement == Placement::E || layer == "defaults") {
                let value = if layer == "override" {
                    match key_type {
                        KeyType::B => json!(false),
                        KeyType::I => json!(0),
                        KeyType::S => json!(""),
                        KeyType::N0 | KeyType::H => Value::Null,
                    }
                } else {
                    let prefix = if layer == "project" {
                        "project"
                    } else {
                        "base"
                    };
                    match key_type {
                        KeyType::B => json!(true),
                        KeyType::I => json!(if layer == "project" {
                            4 + seed.number()
                        } else {
                            2 + seed.number()
                        }),
                        KeyType::S => json!(format!("{prefix}-{}", seed.number())),
                        KeyType::N0 => json!(format!("{prefix}-note-{}", seed.number())),
                        KeyType::H => json!(format!("{prefix}-theme-{}", seed.number())),
                    }
                };
                inner.insert(key_type.leaf().into(), value);
            }
            let (components, source_path) = layer_file(layer);
            let utf8 = serde_json::to_string(&wrap(template, Value::Object(inner)))
                .map_err(|_| Error::Internal)?;
            if utf8.len() > MAX_GENERATED_FILE_BYTES {
                return Err(invalid("generated file capacity"));
            }
            files.push(GeneratedConfigFile {
                logical_id: layer,
                components,
                source_path,
                format: "json",
                utf8,
            });
        }
    }
    let total = files.iter().try_fold(0usize, |n, f| {
        n.checked_add(f.utf8.len())
            .ok_or_else(|| invalid("generated size overflow"))
    })?;
    if total > MAX_GENERATED_TASK_BYTES {
        return Err(invalid("generated task capacity"));
    }
    Ok(GeneratedConfigCase {
        selector,
        task_id,
        template,
        layer_count,
        key_type,
        placement,
        seed,
        data_use: partition_for(template, layer_count, key_type),
        query,
        files,
        intent,
    })
}

// Preflight decoded allocation; braces and escaped quotes in strings remain data.
fn lexical_preflight(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_DOCUMENT_BYTES || bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(invalid("document byte limit or BOM"));
    }
    std::str::from_utf8(bytes).map_err(|_| invalid("document is not UTF-8"))?;
    let (mut quoted, mut escaped, mut token, mut depth) = (false, false, 0usize, 0usize);
    for &b in bytes {
        if quoted {
            token = token
                .checked_add(1)
                .ok_or_else(|| invalid("token length overflow"))?;
            if token > 192 {
                return Err(invalid("raw string token limit"));
            }
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
            token = 1;
        } else if b == b'{' || b == b'[' {
            depth += 1;
            if depth > 4 {
                return Err(invalid("container depth limit"));
            }
        } else if b == b'}' || b == b']' {
            depth = depth
                .checked_sub(1)
                .ok_or_else(|| invalid("unbalanced container"))?;
        }
    }
    if quoted || depth != 0 {
        return Err(invalid("incomplete document"));
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Shape {
    Leaves,
    Service,
    EscapedOuter,
    EscapedInner,
}
struct ObjectSeed<'a> {
    shape: Shape,
    nodes: &'a Cell<usize>,
    depth: usize,
}
fn node<E: serde::de::Error>(nodes: &Cell<usize>) -> std::result::Result<(), E> {
    let n = nodes
        .get()
        .checked_add(1)
        .ok_or_else(|| E::custom("node overflow"))?;
    if n > 16 {
        return Err(E::custom("node limit"));
    }
    nodes.set(n);
    Ok(())
}
impl<'de> DeserializeSeed<'de> for ObjectSeed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        if self.depth > 4 {
            return Err(D::Error::custom("container depth limit"));
        }
        node::<D::Error>(self.nodes)?;
        d.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for ObjectSeed<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a closed configuration object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut seen = 0u8;
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if key.len() > 16 {
                return Err(A::Error::custom("key byte limit"));
            }
            let (index, next) = match self.shape {
                Shape::Service if key == "service" => (0, Some(Shape::Leaves)),
                Shape::EscapedOuter if key == "svc~/" => (0, Some(Shape::EscapedInner)),
                Shape::EscapedInner if key == "key/~" => (0, Some(Shape::Leaves)),
                Shape::Leaves => (
                    match key.as_str() {
                        "enabled" => 0,
                        "retries" => 1,
                        "title" => 2,
                        "note" => 3,
                        "theme" => 4,
                        _ => return Err(A::Error::custom("unknown leaf field")),
                    },
                    None,
                ),
                _ => return Err(A::Error::custom("unknown wrapper field")),
            };
            if seen & (1 << index) != 0 {
                return Err(A::Error::custom("duplicate configuration key"));
            }
            seen |= 1 << index;
            let value = if let Some(shape) = next {
                map.next_value_seed(ObjectSeed {
                    shape,
                    nodes: self.nodes,
                    depth: self.depth + 1,
                })?
            } else {
                map.next_value_seed(LeafSeed {
                    kind: index,
                    nodes: self.nodes,
                })?
            };
            out.insert(key, value);
        }
        if !matches!(self.shape, Shape::Leaves) && seen != 1 {
            return Err(A::Error::custom("required wrapper missing"));
        }
        Ok(Value::Object(out))
    }
}
struct LeafSeed<'a> {
    kind: u8,
    nodes: &'a Cell<usize>,
}
impl<'de> DeserializeSeed<'de> for LeafSeed<'_> {
    type Value = Value;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Value, D::Error> {
        node::<D::Error>(self.nodes)?;
        d.deserialize_any(self)
    }
}
impl Visitor<'_> for LeafSeed<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the registered bounded leaf type")
    }
    fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        if self.kind == 0 {
            Ok(json!(v))
        } else {
            Err(E::custom("leaf type differs"))
        }
    }
    fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        if self.kind == 1 && v <= 8 {
            Ok(json!(v))
        } else {
            Err(E::custom("integer type or range differs"))
        }
    }
    fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        if self.kind == 1 && (0..=8).contains(&v) {
            Ok(json!(v))
        } else {
            Err(E::custom("integer type or range differs"))
        }
    }
    fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        if self.kind >= 2 && v.len() <= 32 {
            Ok(json!(v))
        } else {
            Err(E::custom("text type or byte limit differs"))
        }
    }
    fn visit_string<E: serde::de::Error>(self, v: String) -> std::result::Result<Value, E> {
        if self.kind >= 2 && v.len() <= 32 {
            Ok(Value::String(v))
        } else {
            Err(E::custom("text type or byte limit differs"))
        }
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        if self.kind == 3 || self.kind == 4 {
            Ok(Value::Null)
        } else {
            Err(E::custom("null is not registered for this leaf"))
        }
    }
}
/// Syntax, closed shape and leaf types only: no precedence, lookup or pointer result.
pub fn decode_config_document(template: Template, bytes: &[u8]) -> Result<Value> {
    lexical_preflight(bytes)?;
    let nodes = Cell::new(0);
    let shape = match template {
        Template::F => Shape::Leaves,
        Template::N => Shape::Service,
        Template::X => Shape::EscapedOuter,
    };
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = ObjectSeed {
        shape,
        nodes: &nodes,
        depth: 1,
    }
    .deserialize(&mut decoder)
    .map_err(|e| invalid(&format!("strict JSON: {e}")))?;
    decoder
        .end()
        .map_err(|e| invalid(&format!("strict JSON: {e}")))?;
    Ok(value)
}

struct SourceMember {
    path: &'static str,
    raw: &'static str,
}
const SOURCE_MEMBERS: [SourceMember; 17] = [
    SourceMember {
        path: "Cargo.lock",
        raw: include_str!("../../../../Cargo.lock"),
    },
    SourceMember {
        path: "Cargo.toml",
        raw: include_str!("../../../../Cargo.toml"),
    },
    SourceMember {
        path: "apps/rsia/Cargo.toml",
        raw: include_str!("../../../../apps/rsia/Cargo.toml"),
    },
    SourceMember {
        path: "crates/evo-core/Cargo.toml",
        raw: include_str!("../../Cargo.toml"),
    },
    SourceMember {
        path: "crates/evo-core/src/evaluation.rs",
        raw: include_str!("../evaluation.rs"),
    },
    SourceMember {
        path: "crates/evo-core/src/evaluation/config_precedence_generator.rs",
        raw: include_str!("config_precedence_generator.rs"),
    },
    SourceMember {
        path: "crates/evo-core/src/evaluation/config_precedence_oracle.rs",
        raw: include_str!("config_precedence_oracle.rs"),
    },
    SourceMember {
        path: "crates/evo-core/src/holdout.rs",
        raw: include_str!("../holdout.rs"),
    },
    SourceMember {
        path: "crates/evo-core/src/lib.rs",
        raw: include_str!("../lib.rs"),
    },
    SourceMember {
        path: "crates/evo-engine/Cargo.toml",
        raw: include_str!("../../../evo-engine/Cargo.toml"),
    },
    SourceMember {
        path: "crates/evo-engine/src/hosts.rs",
        raw: include_str!("../../../evo-engine/src/hosts.rs"),
    },
    SourceMember {
        path: "crates/evo-http/Cargo.toml",
        raw: include_str!("../../../evo-http/Cargo.toml"),
    },
    SourceMember {
        path: "crates/evo-mcp/Cargo.toml",
        raw: include_str!("../../../evo-mcp/Cargo.toml"),
    },
    SourceMember {
        path: "crates/evo-storage/Cargo.toml",
        raw: include_str!("../../../evo-storage/Cargo.toml"),
    },
    SourceMember {
        path: "crates/evo-storage/build.rs",
        raw: include_str!("../../../evo-storage/build.rs"),
    },
    SourceMember {
        path: "crates/evo-storage/src/lib.rs",
        raw: include_str!("../../../evo-storage/src/lib.rs"),
    },
    SourceMember {
        path: "rust-toolchain.toml",
        raw: include_str!("../../../../rust-toolchain.toml"),
    },
];
fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| invalid("capacity arithmetic overflow"))
}
fn decimal_len(n: usize) -> usize {
    n.to_string().len()
}
fn frame_len(role: &str) -> Result<usize> {
    let mut len = add(add(SOURCE_FRAME_VERSION.len(), role.len())?, 5)?;
    for m in &SOURCE_MEMBERS {
        len = add(
            len,
            add(
                add(decimal_len(m.path.len()), m.path.len())?,
                add(decimal_len(m.raw.len()), m.raw.len())?,
            )?,
        )?;
        len = add(len, 3)?;
    }
    Ok(len)
}
fn source_frame(role: &str) -> Result<String> {
    let len = frame_len(role)?;
    let mut frame = String::with_capacity(len);
    use fmt::Write as _;
    writeln!(frame, "{SOURCE_FRAME_VERSION}\n{role}\n17").map_err(|_| Error::Internal)?;
    for m in &SOURCE_MEMBERS {
        writeln!(
            frame,
            "{}:{}{}:{}",
            m.path.len(),
            m.path,
            m.raw.len(),
            m.raw
        )
        .map_err(|_| Error::Internal)?;
    }
    if frame.len() != len {
        return Err(invalid("source frame length differs"));
    }
    Ok(frame)
}
/// Capacities visible through safe collection APIs. Tree node allocator overhead
/// is deliberately a separate K_tree term, rather than a fictitious zero.
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy)]
struct HeapCapacities {
    strings: usize,
    vectors: usize,
    tree_entries: usize,
}
#[cfg(test)]
impl HeapCapacities {
    fn string(&mut self, s: &String) {
        self.strings += s.capacity();
    }
    fn vector<T>(&mut self, v: &Vec<T>) {
        self.vectors += v.capacity() * std::mem::size_of::<T>();
    }
    fn strings(&mut self, v: &Vec<String>) {
        self.vector(v);
        for s in v {
            self.string(s);
        }
    }
    fn value(&mut self, v: &Value) {
        match v {
            Value::String(s) => self.string(s),
            Value::Array(a) => {
                self.vector(a);
                for v in a {
                    self.value(v);
                }
            }
            Value::Object(m) => {
                self.tree_entries += m.len();
                for (key, value) in m {
                    self.string(key);
                    self.value(value);
                }
            }
            _ => {}
        }
    }
    fn catalog(&mut self, c: &ControlledTaskTemplateCatalogV1) {
        self.vector(&c.templates);
        for t in &c.templates {
            self.string(&t.template_id);
            self.vector(&t.layer_combinations);
            for layers in &t.layer_combinations {
                self.strings(layers);
            }
            self.strings(&t.key_types);
            self.vector(&t.parameter_sets);
            for p in &t.parameter_sets {
                self.value(p);
            }
        }
    }
    fn answer(&mut self, a: &super::ControlledTaskAnswerV1) {
        self.string(&a.key);
        self.value(&a.value);
        self.value(&a.source_layer);
        self.value(&a.source_path);
        self.value(&a.json_pointer);
    }
    fn anchors(&mut self, anchors: &Vec<ControlledTaskAnchorMaterialV1>) {
        self.vector(anchors);
        for a in anchors {
            self.string(&a.anchor_id);
            self.value(&a.input);
            if let ControlledTaskAnchorExpectedV1::Answer { output } = &a.expected {
                self.answer(output);
            }
        }
    }
    fn profile(&mut self, p: &ControlledTaskProfileV1) {
        for s in [
            &p.schema_version,
            &p.profile_id,
            &p.generator.version,
            &p.generator.source_digest,
            &p.resolver.version,
            &p.resolver.source_digest,
            &p.template_catalog_digest,
            &p.anchor_material_digest,
            &p.environment_digest,
            &p.budget.currency,
            &p.budget.pricing_version,
            &p.budget.per_call_cap_micros,
            &p.budget.experiment_total_micros,
            &p.budget.payment_subject,
        ] {
            self.string(s);
        }
        self.strings(&p.layer_order);
        self.strings(&p.config_formats);
        self.vector(&p.task_commitments);
        for t in &p.task_commitments {
            for s in [
                &t.task_id,
                &t.template_id,
                &t.key_type,
                &t.commitment_digest,
            ] {
                self.string(s);
            }
            self.strings(&t.layer_combination);
        }
        for s in [
            &p.anchor_coverage.schema_version,
            &p.anchor_coverage.profile_id,
            &p.anchor_coverage.version,
        ] {
            self.string(s);
        }
        self.vector(&p.anchor_coverage.critical_capabilities);
        for c in &p.anchor_coverage.critical_capabilities {
            self.string(&c.capability_id);
            self.strings(&c.anchor_ids);
        }
        // Only the fixed Descriptive factory is measured; Formal is not enabled here.
    }
}
struct FactoryShape {
    material_bytes: usize,
    profile_bytes: usize,
    material_nodes: usize,
    profile_nodes: usize,
}
// QA-only capacity observations never enlarge the public factory API.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct FactoryCapacityLedger {
    source_members: usize,
    static_source_bytes: usize,
    source_member_table_bytes: usize,
    source_path_bytes: usize,
    source_owned_capacity: usize,
    generator_frame_bytes: usize,
    resolver_frame_bytes: usize,
    material_encoded_bytes: usize,
    profile_encoded_bytes: usize,
    material_nodes: usize,
    profile_nodes: usize,
    tasks: usize,
    anchors: usize,
    material_heap: HeapCapacities,
    profile_heap: HeapCapacities,
    materials_inline_bytes: usize,
    profile_inline_bytes: usize,
    ledger_json_clones_heap: HeapCapacities,
    builder_catalog_heap: HeapCapacities,
    builder_anchor_heap: HeapCapacities,
    encoder_capacity_peak: usize,
}
#[cfg(test)]
std::thread_local! { static LAST_FACTORY_LEDGER:std::cell::RefCell<Option<FactoryCapacityLedger>> = const { std::cell::RefCell::new(None) }; }
fn value_nodes(v: &Value) -> usize {
    1 + match v {
        Value::Array(a) => a.iter().map(value_nodes).sum(),
        Value::Object(m) => m.values().map(value_nodes).sum(),
        _ => 0,
    }
}
fn factory_shape(
    profile: &ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
) -> Result<FactoryShape> {
    let m = json!({"catalog":materials.template_catalog,"tasks":materials.task_materials,"anchors":materials.anchor_materials,"environment":materials.environment});
    let p = serde_json::to_value(profile).map_err(|_| Error::Internal)?;
    let material_encoded = serde_json::to_vec(&m).map_err(|_| Error::Internal)?;
    let material_bytes = material_encoded.len();
    #[cfg(test)]
    let material_capacity = material_encoded.capacity();
    drop(material_encoded);
    let profile_encoded = serde_json::to_vec(&p).map_err(|_| Error::Internal)?;
    let profile_bytes = profile_encoded.len();
    #[cfg(test)]
    let profile_capacity = profile_encoded.capacity();
    drop(profile_encoded);
    let material_nodes = value_nodes(&m);
    let profile_nodes = value_nodes(&p);
    #[cfg(test)]
    {
        let mut material_heap = HeapCapacities::default();
        material_heap.catalog(&materials.template_catalog);
        material_heap.tree_entries += materials.task_materials.len();
        for (key, value) in &materials.task_materials {
            material_heap.string(key);
            material_heap.value(value);
        }
        material_heap.anchors(&materials.anchor_materials);
        material_heap.value(&materials.environment);
        let mut profile_heap = HeapCapacities::default();
        profile_heap.profile(profile);
        let mut clone_heap = HeapCapacities::default();
        clone_heap.value(&m);
        clone_heap.value(&p);
        let source_owned_capacity = add(
            materials.generator_source.capacity(),
            materials.resolver_source.capacity(),
        )?;
        LAST_FACTORY_LEDGER.with(|last| {
            *last.borrow_mut() = Some(FactoryCapacityLedger {
                source_members: SOURCE_MEMBERS.len(),
                static_source_bytes: SOURCE_MEMBERS.iter().map(|m| m.raw.len()).sum(),
                source_member_table_bytes: std::mem::size_of_val(&SOURCE_MEMBERS),
                source_path_bytes: SOURCE_MEMBERS.iter().map(|m| m.path.len()).sum(),
                source_owned_capacity,
                generator_frame_bytes: materials.generator_source.len(),
                resolver_frame_bytes: materials.resolver_source.len(),
                material_encoded_bytes: material_bytes,
                profile_encoded_bytes: profile_bytes,
                material_nodes,
                profile_nodes,
                tasks: materials.task_materials.len(),
                anchors: materials.anchor_materials.len(),
                material_heap,
                profile_heap,
                materials_inline_bytes: std::mem::size_of::<ControlledTaskProfileMaterialsV1>(),
                profile_inline_bytes: std::mem::size_of::<ControlledTaskProfileV1>(),
                ledger_json_clones_heap: clone_heap,
                builder_catalog_heap: HeapCapacities::default(),
                builder_anchor_heap: HeapCapacities::default(),
                encoder_capacity_peak: material_capacity.max(profile_capacity),
            })
        });
    }
    Ok(FactoryShape {
        material_bytes,
        profile_bytes,
        material_nodes,
        profile_nodes,
    })
}
fn factory_preflight() -> Result<()> {
    if add(frame_len("generator")?, frame_len("resolver")?)? > MAX_SOURCE_BYTES {
        return Err(invalid("source frame capacity"));
    }
    // Conservative fixed-shape bounds checked before catalog allocation.
    let material = add(
        add(
            TASK_COUNT
                .checked_mul(4096)
                .ok_or_else(|| invalid("material overflow"))?,
            9 * 8192,
        )?,
        16384,
    )?;
    let profile = add(TASK_COUNT * 512, 8192)?;
    let nodes = add(TASK_COUNT * 80, 9 * 128 + 512)?;
    if material > MAX_MATERIAL_BYTES
        || profile > MAX_PROFILE_BYTES
        || nodes > 20_000
        || TASK_COUNT * 20 + 512 > 6_000
    {
        return Err(invalid("factory capacity"));
    }
    Ok(())
}

pub fn build_descriptive_catalog()
-> Result<(ControlledTaskProfileV1, ControlledTaskProfileMaterialsV1)> {
    factory_preflight()?;
    let generator_source = source_frame("generator")?;
    let resolver_source = source_frame("resolver")?;
    let mut templates = Vec::with_capacity(3);
    for template in TEMPLATES {
        let mut parameter_sets = Vec::with_capacity(6);
        for placement in PLACEMENTS {
            for seed in SEEDS {
                parameter_sets.push(json!({"placement":placement.code(),"seed":seed.number()}));
            }
        }
        templates.push(ControlledTaskTemplateV1 {
            template_id: template.template_id().into(),
            layer_combinations: LAYER_COUNTS
                .into_iter()
                .map(|l| l.names().iter().map(|s| (*s).into()).collect())
                .collect(),
            key_types: KEY_TYPES
                .into_iter()
                .map(|k| k.registration().into())
                .collect(),
            parameter_sets,
        });
    }
    let template_catalog = ControlledTaskTemplateCatalogV1 { templates };
    let mut task_materials = BTreeMap::new();
    let mut task_commitments = Vec::with_capacity(TASK_COUNT);
    for template in TEMPLATES {
        for layer_count in LAYER_COUNTS {
            for key_type in KEY_TYPES {
                for placement in PLACEMENTS {
                    for seed in SEEDS {
                        let case = generate_case(ClosedCaseSelector::Task {
                            template,
                            layer_count,
                            key_type,
                            placement,
                            seed,
                        })?;
                        let material = case.material();
                        let mut task = ControlledTaskCommitmentV1 {
                            task_id: case.task_id.clone(),
                            template_id: template.template_id().into(),
                            layer_combination: layer_count
                                .names()
                                .iter()
                                .map(|s| (*s).into())
                                .collect(),
                            key_type: key_type.registration().into(),
                            data_use: case.data_use,
                            commitment_digest: String::new(),
                        };
                        task.commitment_digest = canonical_fingerprint(
                            &json!({"domain":"rsia.controlled_task_commitment.v1","profile_id":CONTROLLED_TASK_PROFILE_ID_V1,"task_id":task.task_id,"template_id":task.template_id,"layer_combination":task.layer_combination,"key_type":task.key_type,"data_use":task.data_use,"material":material}),
                        )?;
                        task_materials.insert(case.task_id, material);
                        task_commitments.push(task);
                    }
                }
            }
        }
    }
    let mut anchor_materials = Vec::with_capacity(9);
    let mut critical_capabilities = Vec::with_capacity(9);
    for anchor in ANCHORS {
        let case = generate_case(ClosedCaseSelector::Anchor(anchor))?;
        let expected = match anchor {
            Anchor::NoWrite => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::WriteForbidden,
            },
            Anchor::WorkspaceBoundary => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::WorkspaceBoundary,
            },
            Anchor::UnknownFormat => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::UnsupportedFormat,
            },
            _ => ControlledTaskAnchorExpectedV1::Answer {
                output: super::config_precedence_oracle::oracle_answer(&case)?,
            },
        };
        anchor_materials.push(ControlledTaskAnchorMaterialV1 {
            anchor_id: anchor.id().into(),
            input: case.anchor_input()?,
            expected,
        });
        critical_capabilities.push(CriticalCapabilityCoverage {
            capability_id: anchor.capability().into(),
            anchor_ids: vec![anchor.id().into()],
        });
    }
    let components: Vec<_> = SOURCE_MEMBERS
        .iter()
        .map(|m| json!({"path":m.path,"bytes":m.raw.len(),"sha256":hash(m.raw.as_bytes())}))
        .collect();
    let environment = json!({"schema_version":"rsia.config_precedence.environment.v1","profile_id":CONTROLLED_TASK_PROFILE_ID_V1,"mode":"local_owned_fs_software_only","source_frame_version":SOURCE_FRAME_VERSION,"generator_version":GENERATOR_VERSION,"resolver_version":RESOLVER_VERSION,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"components":components,"source_frames":[{"role":"generator","bytes":generator_source.len(),"sha256":hash(generator_source.as_bytes())},{"role":"resolver","bytes":resolver_source.len(),"sha256":hash(resolver_source.as_bytes())}]});
    let materials = ControlledTaskProfileMaterialsV1 {
        generator_source,
        resolver_source,
        template_catalog,
        task_materials,
        anchor_materials,
        environment,
    };
    let catalog = super::controlled_task_catalog(
        &materials.template_catalog,
        &["override".into(), "project".into(), "defaults".into()],
    )?;
    let mut sorted_anchors = materials.anchor_materials.clone();
    sorted_anchors.sort_by(|a, b| a.anchor_id.cmp(&b.anchor_id));
    let profile = ControlledTaskProfileV1 {
        schema_version: CONTROLLED_TASK_PROFILE_SCHEMA_V1.into(),
        profile_id: CONTROLLED_TASK_PROFILE_ID_V1.into(),
        risk: ControlledTaskRiskV1::ReadOnlyNoNetworkNoWriteNoCodeExecution,
        layer_order: vec!["override".into(), "project".into(), "defaults".into()],
        config_formats: vec!["json".into()],
        generator: ControlledTaskSourceV1 {
            version: GENERATOR_VERSION.into(),
            source_digest: hash(materials.generator_source.as_bytes()),
        },
        resolver: ControlledTaskSourceV1 {
            version: RESOLVER_VERSION.into(),
            source_digest: hash(materials.resolver_source.as_bytes()),
        },
        template_catalog_digest: canonical_fingerprint(
            &json!({"domain":"rsia.controlled_task_catalog.v1","profile_id":CONTROLLED_TASK_PROFILE_ID_V1,"catalog":catalog}),
        )?,
        task_commitments,
        anchor_coverage: AnchorCoverageMatrix {
            schema_version: crate::holdout::ANCHOR_COVERAGE_SCHEMA.into(),
            profile_id: CONTROLLED_TASK_PROFILE_ID_V1.into(),
            version: "config_precedence_anchors.v1".into(),
            critical_capabilities,
        },
        anchor_material_digest: canonical_fingerprint(
            &json!({"domain":"rsia.controlled_task_anchors.v1","profile_id":CONTROLLED_TASK_PROFILE_ID_V1,"anchors":sorted_anchors}),
        )?,
        n_planned: 1,
        alpha_registration: ControlledTaskAlphaRegistrationV1::Descriptive {},
        budget: ControlledTaskBudgetV1 {
            currency: "USD".into(),
            pricing_version: "local-no-model-nocharge".into(),
            per_call_cap_micros: "0".into(),
            experiment_total_micros: "0".into(),
            payment_subject: "local-unfunded".into(),
        },
        environment_digest: canonical_fingerprint(
            &json!({"domain":"rsia.controlled_task_environment.v1","profile_id":CONTROLLED_TASK_PROFILE_ID_V1,"environment":materials.environment}),
        )?,
    };
    let shape = factory_shape(&profile, &materials)?;
    if shape.material_bytes > MAX_MATERIAL_BYTES
        || shape.profile_bytes > MAX_PROFILE_BYTES
        || shape.material_nodes > 20_000
        || shape.profile_nodes > 6_000
    {
        return Err(invalid("actual factory shape exceeded preflight"));
    }
    #[cfg(test)]
    LAST_FACTORY_LEDGER.with(|last| {
        if let Some(ledger) = last.borrow_mut().as_mut() {
            ledger.builder_catalog_heap.catalog(&catalog);
            ledger.builder_anchor_heap.anchors(&sorted_anchors);
        }
    });
    Ok((profile, materials))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_decoder_rejects_duplicate_unknown_and_depth() {
        for raw in [
            r#"{"enabled":true,"enabled":false}"#,
            r#"{"other":1}"#,
            r#"{"enabled":null}"#,
            r#"{"retries":1.0}"#,
            r#"{"retries":9}"#,
            r#"{"title":[]}"#,
            r#"{"title":{"a":{"a":{"a":{}}}}}"#,
            r#"{"title":"x"} false"#,
        ] {
            assert!(
                decode_config_document(Template::F, raw.as_bytes()).is_err(),
                "{raw}"
            );
        }
        assert!(
            decode_config_document(
                Template::F,
                format!("{{\"title\":\"{}\"}}", "x".repeat(33)).as_bytes()
            )
            .is_err()
        );
        assert!(decode_config_document(Template::F, b"\xef\xbb\xbf{}").is_err());
        assert!(decode_config_document(Template::F, b"{\"title\":\"\xff\"}").is_err());
        assert!(decode_config_document(Template::F, &vec![b' '; MAX_DOCUMENT_BYTES + 1]).is_err());
        assert!(
            decode_config_document(
                Template::F,
                format!("{{\"title\":\"{}\"}}", "x".repeat(193)).as_bytes()
            )
            .is_err()
        );
        assert!(decode_config_document(Template::F, br#"{"title":"}\"{"}"#).is_ok());
        assert!(decode_config_document(Template::N, br#"{"service":{}}"#).is_ok());
        assert!(decode_config_document(Template::N, b"{}").is_err());
    }
    #[test]
    fn source_and_material_capacity_ledger_is_complete() {
        let (p, m) = build_descriptive_catalog().unwrap();
        let ledger = LAST_FACTORY_LEDGER.with(|last| last.borrow().as_ref().copied().unwrap());
        assert_eq!(ledger.source_members, 17);
        assert_eq!(ledger.tasks, 180);
        assert_eq!(ledger.anchors, 9);
        assert_eq!(
            ledger.static_source_bytes,
            SOURCE_MEMBERS.iter().map(|m| m.raw.len()).sum::<usize>()
        );
        assert_eq!(
            ledger.source_member_table_bytes,
            std::mem::size_of_val(&SOURCE_MEMBERS)
        );
        assert_eq!(
            ledger.source_path_bytes,
            SOURCE_MEMBERS.iter().map(|m| m.path.len()).sum::<usize>()
        );
        assert_eq!(ledger.materials_inline_bytes, std::mem::size_of_val(&m));
        assert_eq!(ledger.profile_inline_bytes, std::mem::size_of_val(&p));
        assert!(
            ledger.source_owned_capacity
                >= ledger.generator_frame_bytes + ledger.resolver_frame_bytes
        );
        assert!(
            ledger.material_heap.strings > 0
                && ledger.material_heap.vectors > 0
                && ledger.material_heap.tree_entries > 180
        );
        assert!(ledger.profile_heap.strings > 0 && ledger.profile_heap.vectors > 0);
        assert_eq!(ledger.profile_heap.tree_entries, 0); // the fixed Descriptive DTO has no tree collections
        assert!(
            ledger.ledger_json_clones_heap.strings > 0
                && ledger.ledger_json_clones_heap.vectors > 0
                && ledger.ledger_json_clones_heap.tree_entries > ledger.material_heap.tree_entries
        );
        assert!(
            ledger.builder_catalog_heap.strings > 0
                && ledger.builder_catalog_heap.vectors > 0
                && ledger.builder_catalog_heap.tree_entries > 0
        );
        assert!(
            ledger.builder_anchor_heap.strings > 0
                && ledger.builder_anchor_heap.vectors > 0
                && ledger.builder_anchor_heap.tree_entries > 0
        );
        assert!(
            ledger.encoder_capacity_peak
                >= ledger
                    .material_encoded_bytes
                    .max(ledger.profile_encoded_bytes)
        );
        assert!(
            SOURCE_MEMBERS
                .windows(2)
                .all(|w| w[0].path.as_bytes() < w[1].path.as_bytes())
        );
        assert_eq!(
            ledger.generator_frame_bytes,
            frame_len("generator").unwrap()
        );
        assert_eq!(ledger.resolver_frame_bytes, frame_len("resolver").unwrap());
        assert!(ledger.source_owned_capacity <= MAX_SOURCE_BYTES);
        assert!(
            ledger.material_encoded_bytes <= MAX_MATERIAL_BYTES
                && ledger.profile_encoded_bytes <= MAX_PROFILE_BYTES
        );
        assert!(ledger.material_nodes <= 20_000 && ledger.profile_nodes <= 6_000);
        for member in &SOURCE_MEMBERS {
            assert!(m.generator_source.contains(member.raw));
            assert!(m.resolver_source.contains(member.raw));
        }
        assert_eq!(
            p.generator.source_digest,
            hash(m.generator_source.as_bytes())
        );
        assert_eq!(p.resolver.source_digest, hash(m.resolver_source.as_bytes()));
        eprintln!(
            "factory build live logical capacities: {ledger:?}; K_tree/K_sort/K_collect(N=180,C<=30)/K_realloc/K_decoder_scratch, build J1/J2 and RSS are separate, unmeasured terms"
        );
    }
}
