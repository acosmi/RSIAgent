//! Pure stdin/stdout bridge for the original typed prepare operation. A hash
//! from this helper grants no source, role, snapshot or database authority.
use evo_core::prepare_contract::{HostPrepareConfig, PrepareOperation};
use evo_core::{Error, Prepare, Result, Validate, fingerprint};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    request: Prepare,
    config: HostPrepareConfig,
}

#[derive(Debug, Serialize)]
struct Output {
    /// Original typed serialization, retained as text instead of round-tripping
    /// through Value (whose object ordering is a different contract).
    operation_json: String,
    payload_hash: String,
}

fn evaluate(raw: &str) -> Result<Output> {
    let input: Input = serde_json::from_str(raw)
        .map_err(|_| Error::Invalid("invalid prepare preimage input".into()))?;
    input.request.validate()?;
    input.config.validate()?;
    let operation = PrepareOperation {
        request: &input.request,
        config: &input.config,
    };
    let operation_json = serde_json::to_string(&operation).map_err(|_| Error::Internal)?;
    let payload_hash = fingerprint(&operation)?;
    Ok(Output {
        operation_json,
        payload_hash,
    })
}

fn run() -> Result<()> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|_| Error::Invalid("cannot read prepare preimage input".into()))?;
    let result = evaluate(&raw)?;
    let mut output = serde_json::to_vec(&result).map_err(|_| Error::Internal)?;
    output.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|_| Error::Internal)
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => {
            // The input contains local configuration; never echo it in errors.
            eprintln!("prepare_preimage_invalid");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Synthetic pre-move typed vector, shared with prepare_contract_v43. This
    // is not a claim that a historical DB contains its original preimage.
    const REQUEST: &str = r#"{"request_key":"prepare-a","goal":"检查 café\r\n保留 é","capabilities":["tool-z","tool-a"]}"#;
    const CONFIG: &str = r#"{"profile_id":"profile-a","system_snapshot":{"schema_version":"rsia.system_snapshot.v2","profile_id":"profile-a","host_id":"host-a","host_version":"1.0 café","model_id":"model-a","tools":["tool-z","tool-a"],"mandatory_context_digest":"c15677734a3888998ee6488dc8a675b2d7025f1012d62e0fad9c48c9755380f6"},"host_surface_id":"surface-a","host_capabilities":{"available":["tool-a","tool-m","tool-z"],"granted":["tool-a","tool-z"]},"evolution_enabled":true,"capability_level":"tool_only"}"#;

    fn input() -> String {
        format!(r#"{{"request":{REQUEST},"config":{CONFIG}}}"#)
    }

    #[test]
    fn original_typed_bytes_and_hash_are_preserved() {
        let original = input();
        let output = evaluate(&original).unwrap();
        assert_eq!(output.operation_json.as_bytes(), original.as_bytes());
        assert_eq!(
            output.payload_hash,
            "f23d95bf3549269e929bf91b1e61811bd7b915867ef146155f96f1b3f489c7af"
        );
        assert_eq!(
            evo_core::hash(output.operation_json.as_bytes()),
            output.payload_hash
        );
    }

    #[test]
    fn duplicate_unknown_and_trailing_inputs_are_refused() {
        let original = input();
        for invalid in [
            original.replace(
                "\"request_key\":\"prepare-a\"",
                "\"request_key\":\"prepare-a\",\"request_key\":\"prepare-b\"",
            ),
            original.replacen("{", "{\"trusted\":true,", 1),
            format!("{original}{{}}"),
        ] {
            assert!(evaluate(&invalid).is_err());
        }
    }

    #[test]
    fn capability_and_original_tool_order_change_the_actual_hash() {
        let original = input();
        let baseline = evaluate(&original).unwrap().payload_hash;
        let reordered_tools = original.replace(
            "\"tools\":[\"tool-z\",\"tool-a\"]",
            "\"tools\":[\"tool-a\",\"tool-z\"]",
        );
        let reordered_request = original.replacen(
            "\"capabilities\":[\"tool-z\",\"tool-a\"]",
            "\"capabilities\":[\"tool-a\",\"tool-z\"]",
            1,
        );
        for changed in [
            reordered_tools,
            reordered_request,
            original.replace("\"evolution_enabled\":true", "\"evolution_enabled\":false"),
        ] {
            assert_ne!(evaluate(&changed).unwrap().payload_hash, baseline);
        }
    }
}
