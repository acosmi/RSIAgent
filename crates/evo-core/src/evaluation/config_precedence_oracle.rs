//! Trusted deterministic oracle. It shares syntax decoding, never host selection.
use super::ControlledTaskAnswerV1;
use super::config_precedence_generator::{
    ConfigIntent, GeneratedConfigCase, KeyType, decode_config_document,
};
use crate::{Error, Result};
use serde_json::{Value, json};

// An independent object-only pointer locator, including its own escape decoder.
// The host uses serde_json's pointer lookup; the oracle never calls that resolver.
fn oracle_pointer<'a>(document: &'a Value, pointer: &str) -> Option<&'a Value> {
    let mut node = document;
    for encoded in pointer.strip_prefix('/')?.split('/') {
        let mut decoded = String::with_capacity(encoded.len());
        let mut chars = encoded.chars();
        while let Some(ch) = chars.next() {
            if ch == '~' {
                match chars.next()? {
                    '0' => decoded.push('~'),
                    '1' => decoded.push('/'),
                    _ => return None,
                }
            } else {
                decoded.push(ch);
            }
        }
        node = node.as_object()?.get(&decoded)?;
    }
    Some(node)
}

pub fn oracle_answer(case: &GeneratedConfigCase) -> Result<ControlledTaskAnswerV1> {
    if case.intent() != ConfigIntent::Resolve {
        return Err(Error::Forbidden);
    }
    // Independently assemble RFC6901 escapes; no resolver result helper is shared.
    let mut pointer = String::new();
    for segment in case.query_segments() {
        pointer.push('/');
        for ch in segment.chars() {
            match ch {
                '~' => pointer.push_str("~0"),
                '/' => pointer.push_str("~1"),
                _ => pointer.push(ch),
            }
        }
    }
    let mut answer = ControlledTaskAnswerV1 {
        key: pointer.clone(),
        present: false,
        value: Value::Null,
        source_layer: Value::Null,
        source_path: Value::Null,
        json_pointer: Value::Null,
    };
    let mut ids = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    // Low to high: a higher present leaf replaces the accumulated lower answer.
    for file in case.files().iter().rev() {
        if !ids.insert(file.logical_id()) || !paths.insert(file.source_path()) {
            return Err(Error::Invalid("oracle duplicate layer or file".into()));
        }
        if file.format() != "json" {
            return Err(Error::Invalid(
                "oracle unsupported configuration format".into(),
            ));
        }
        let document = decode_config_document(case.template(), file.raw_utf8().as_bytes())?;
        let mut node = &document;
        let mut missing = false;
        for segment in case.query_segments() {
            match node.as_object().and_then(|object| object.get(*segment)) {
                Some(next) => node = next,
                None => {
                    missing = true;
                    break;
                }
            }
        }
        if missing || (case.key_type() == KeyType::H && node.is_null()) {
            continue;
        }
        // Validate the pointer against the actual same document, not an expected value.
        if oracle_pointer(&document, &pointer) != Some(node) {
            return Err(Error::Conflict(
                "oracle pointer does not locate the selected value".into(),
            ));
        }
        answer.present = true;
        answer.value = node.clone();
        answer.source_layer = json!(file.logical_id());
        answer.source_path = json!(file.source_path());
        answer.json_pointer = json!(pointer);
    }
    Ok(answer)
}

/// Only the complete six-field JSON object receives credit.
pub fn grade_exact(expected: &ControlledTaskAnswerV1, output: &Value) -> u8 {
    u8::from(serde_json::to_value(expected).is_ok_and(|canonical| canonical == *output))
}

#[cfg(test)]
mod tests {
    use super::super::config_precedence_generator::*;
    use super::*;
    #[test]
    fn null_policy_distinguishes_value_and_inheritance() {
        for template in TEMPLATES {
            for layer_count in LAYER_COUNTS {
                for (key_type, source, present) in [
                    (KeyType::N0, "override", true),
                    (
                        KeyType::H,
                        if layer_count == Layers::L3 {
                            "project"
                        } else {
                            "defaults"
                        },
                        true,
                    ),
                ] {
                    let case = generate_case(ClosedCaseSelector::Task {
                        template,
                        layer_count,
                        key_type,
                        placement: Placement::E,
                        seed: Seed::One,
                    })
                    .unwrap();
                    let answer = oracle_answer(&case).unwrap();
                    assert_eq!(answer.present, present);
                    assert_eq!(answer.source_layer, json!(source));
                    assert_eq!(answer.value.is_null(), key_type == KeyType::N0);
                }
            }
        }
    }
    #[test]
    fn pointer_escapes_and_locates_actual_node() {
        for key_type in KEY_TYPES {
            let case = generate_case(ClosedCaseSelector::Task {
                template: Template::X,
                layer_count: Layers::L3,
                key_type,
                placement: Placement::E,
                seed: Seed::Zero,
            })
            .unwrap();
            let answer = oracle_answer(&case).unwrap();
            let pointer = format!("/svc~0~1/key~1~0/{}", key_type.leaf());
            assert_eq!(answer.key, pointer);
            assert_eq!(answer.json_pointer, json!(pointer));
            let source = case
                .files()
                .iter()
                .find(|f| json!(f.source_path()) == answer.source_path)
                .unwrap();
            let raw: Value = serde_json::from_str(source.raw_utf8()).unwrap();
            assert_eq!(raw.pointer(&pointer), Some(&answer.value));
        }
    }
}
