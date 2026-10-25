use evo_core::contract::{HostSurfaceManifest, SURFACE_SCHEMA};
use std::path::PathBuf;

#[test]
fn reference_host_fixture_covers_extracted_fields() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/reference-host/host_surface.v1.json");
    let raw = std::fs::read_to_string(path).unwrap();
    let manifest: HostSurfaceManifest = serde_json::from_str(&raw).unwrap();
    assert_eq!(manifest.schema_version, SURFACE_SCHEMA);
    manifest
        .validate_against_extraction(&["instruction".into(), "model".into(), "theme".into()])
        .unwrap();
}

mod reviewed_source_v2_contract {
    use evo_core::contract::{
        HostSurfaceDefaultV2, ReviewedDefaultV1, ReviewedHostSourceEnvelopeV1,
        ReviewedHostSourceProjectionV1, ReviewedTypedDefaultV1, extract_reviewed_host_source,
        reviewed_defaults_same_value,
    };

    fn source_with_declarations(declarations: &str) -> ReviewedHostSourceEnvelopeV1 {
        ReviewedHostSourceEnvelopeV1 {
            schema_version: "rsia.host_source_envelope.v1".into(),
            source_kind: "reviewed_fixture".into(),
            source_schema: "rsia.host_reviewed_declarations.v1".into(),
            extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
            original_utf8: format!(
                r#"{{"schema_version":"rsia.host_reviewed_declarations.v1","host":"fixture-host","host_version":"1","declarations":[{declarations}]}}"#
            ),
        }
    }

    fn single(value_type: &str, choices: &str, default: &str) -> ReviewedHostSourceEnvelopeV1 {
        source_with_declarations(&format!(
            r#"{{"name":"setting","value_type":"{value_type}","enum_values":{choices},"default":{default}}}"#
        ))
    }

    fn with_value(value_type: &str, value: &str) -> ReviewedHostSourceEnvelopeV1 {
        single(
            value_type,
            "[]",
            &format!(r#"{{"kind":"value","value":{value}}}"#),
        )
    }

    fn projected_value(projection: &ReviewedHostSourceProjectionV1) -> &ReviewedTypedDefaultV1 {
        match &projection.declarations[0].default {
            ReviewedDefaultV1::Value(value) => value,
            other => panic!("expected a validated value default, got {other:?}"),
        }
    }

    fn nested_object(depth: usize, leaf: &str) -> String {
        format!("{}{}{}", r#"{"x":"#.repeat(depth), leaf, "}".repeat(depth))
    }

    #[test]
    fn actual_utf8_is_preserved_while_value_comparison_is_semantic() {
        let original = with_value("string", r#""é""#);
        let mut escaped = original.clone();
        escaped.original_utf8 = escaped.original_utf8.replace('é', r#"\u00e9"#);
        let mut crlf = original.clone();
        crlf.original_utf8 = crlf.original_utf8.replace(',', ",\r\n");
        let a = extract_reviewed_host_source(&original).unwrap();
        let b = extract_reviewed_host_source(&escaped).unwrap();
        let c = extract_reviewed_host_source(&crlf).unwrap();
        for (input, output) in [(&original, &a), (&escaped, &b), (&crlf, &c)] {
            assert_eq!(output.source.original_utf8, input.original_utf8);
            assert_eq!(
                output.source_digest,
                evo_core::hash(input.original_utf8.as_bytes())
            );
        }
        assert_ne!(a.source_digest, b.source_digest);
        assert_ne!(a.source_digest, c.source_digest);
        assert!(reviewed_defaults_same_value(
            &a.declarations[0].default,
            &b.declarations[0].default
        ));
        assert!(reviewed_defaults_same_value(
            &a.declarations[0].default,
            &c.declarations[0].default
        ));
        let different =
            extract_reviewed_host_source(&with_value("string", r#""e\u0301""#)).unwrap();
        assert!(!reviewed_defaults_same_value(
            &a.declarations[0].default,
            &different.declarations[0].default
        ));
    }

    #[test]
    fn typed_json_keeps_real_numbers_and_integer_domain() {
        for (value_type, raw) in [
            ("integer", "-9223372036854775808"),
            ("integer", "18446744073709551615"),
            ("object", r#"{"n":0.1,"fraction":1.5,"inner_null":null}"#),
        ] {
            let projection = extract_reviewed_host_source(&with_value(value_type, raw)).unwrap();
            let typed = projected_value(&projection);
            let decoded: serde_json::Value = serde_json::from_str(typed.typed_json_utf8()).unwrap();
            if value_type == "integer" {
                assert!(decoded.is_i64() || decoded.is_u64());
                assert!(!decoded.is_f64());
                assert_eq!(typed.typed_json_utf8(), raw);
            } else {
                assert!(decoded.is_object());
                assert!(decoded["n"].is_number());
                assert!(decoded["fraction"].is_number());
                assert_eq!(decoded["n"], serde_json::json!(0.1));
                assert_eq!(decoded["fraction"], serde_json::json!(1.5));
                assert!(decoded["inner_null"].is_null());
            }
        }
        let a = extract_reviewed_host_source(&with_value("object", r#"{"n":0.1}"#)).unwrap();
        let b = extract_reviewed_host_source(&with_value("object", r#"{"n":1e-1}"#)).unwrap();
        assert!(reviewed_defaults_same_value(
            &a.declarations[0].default,
            &b.declarations[0].default
        ));
        let healthy = with_value("integer", "1");
        extract_reviewed_host_source(&healthy).unwrap();
        assert!(extract_reviewed_host_source(&with_value("integer", "1.0")).is_err());
    }

    #[test]
    fn precision_loss_and_non_json_numbers_are_rejected_after_healthy_control() {
        extract_reviewed_host_source(&with_value("object", r#"{"n":0.1}"#)).unwrap();
        for raw in [
            r#"{"n":0.100000000000000001}"#,
            r#"{"n":18446744073709551616}"#,
            r#"{"n":NaN}"#,
            r#"{"n":Infinity}"#,
            r#"{"n":01}"#,
            r#"{"n":1e}"#,
        ] {
            assert!(
                extract_reviewed_host_source(&with_value("object", raw)).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn normalized_default_boundary_is_exactly_4096_bytes() {
        for (allowed_json, refused_json) in [
            (
                format!("\"{}\"", "a".repeat(4094)),
                format!("\"{}\"", "a".repeat(4095)),
            ),
            (
                format!("\"{}\"", "é".repeat(2047)),
                format!("\"{}a\"", "é".repeat(2047)),
            ),
        ] {
            assert_eq!(allowed_json.len(), 4096);
            assert_eq!(refused_json.len(), 4097);
            let healthy =
                extract_reviewed_host_source(&with_value("string", &allowed_json)).unwrap();
            assert_eq!(
                projected_value(&healthy).comparison_projection().len(),
                4096
            );
            assert!(extract_reviewed_host_source(&with_value("string", &refused_json)).is_err());
        }
    }

    #[test]
    fn legal_depth_200_is_not_replaced_by_a_serde_string_depth_gate() {
        let raw = nested_object(200, "0");
        let projection = extract_reviewed_host_source(&with_value("object", &raw)).unwrap();
        assert!(projected_value(&projection).comparison_projection().len() <= 4096);
        assert_eq!(projected_value(&projection).typed_json_utf8(), raw);
        let inner_null = nested_object(200, "null");
        extract_reviewed_host_source(&with_value("object", &inner_null)).unwrap();
        assert!(extract_reviewed_host_source(&with_value("object", "null")).is_err());
    }

    #[test]
    fn decoded_duplicate_keys_are_rejected_even_inside_legal_deep_objects() {
        let healthy = nested_object(198, r#"{"x":1,"y":2}"#);
        extract_reviewed_host_source(&with_value("object", &healthy)).unwrap();
        for leaf in [r#"{"x":1,"x":2}"#, r#"{"x":1,"\u0078":2}"#] {
            let duplicate = nested_object(198, leaf);
            assert!(extract_reviewed_host_source(&with_value("object", &duplicate)).is_err());
        }
        let mut top_duplicate = with_value("bool", "true");
        extract_reviewed_host_source(&top_duplicate).unwrap();
        top_duplicate.original_utf8 = top_duplicate.original_utf8.replacen(
            r#""host":"fixture-host""#,
            r#""host":"fixture-host","\u0068ost":"fixture-host""#,
            1,
        );
        assert!(extract_reviewed_host_source(&top_duplicate).is_err());
    }

    #[test]
    fn source_tuple_and_body_shape_have_no_generic_fallback() {
        let healthy = with_value("bool", "true");
        extract_reviewed_host_source(&healthy).unwrap();
        for kind in ["machine_schema", "type_declaration", "handshake", "unknown"] {
            let mut changed = healthy.clone();
            changed.source_kind = kind.into();
            assert!(extract_reviewed_host_source(&changed).is_err(), "{kind}");
        }
        let mut schema = healthy.clone();
        schema.source_schema = "rsia.host_reviewed_declarations.v2".into();
        assert!(extract_reviewed_host_source(&schema).is_err());
        let mut extractor = healthy.clone();
        extractor.extractor_version = "rsia.host_reviewed_declarations.extractor.v2".into();
        assert!(extract_reviewed_host_source(&extractor).is_err());
        let mut envelope = healthy.clone();
        envelope.schema_version = "rsia.host_source_envelope.v2".into();
        assert!(extract_reviewed_host_source(&envelope).is_err());
        let mut extra = healthy;
        extra.original_utf8.insert_str(1, r#""unregistered":true,"#);
        assert!(extract_reviewed_host_source(&extra).is_err());
    }

    #[test]
    fn default_three_states_and_opaque_do_not_accept_unregistered_content() {
        for state in ["none", "unknown"] {
            let healthy = single("opaque", "[]", &format!(r#"{{"kind":"{state}"}}"#));
            extract_reviewed_host_source(&healthy).unwrap();
            for extra in [r#""value":1"#, r#""other":true"#] {
                let bad = single("opaque", "[]", &format!(r#"{{"kind":"{state}",{extra}}}"#));
                assert!(extract_reviewed_host_source(&bad).is_err());
            }
        }
        assert!(extract_reviewed_host_source(&with_value("opaque", "true")).is_err());
        for (healthy, invalid) in [
            (r#"{"kind":"none"}"#, r#"{"kind":"none","value":1}"#),
            (
                r#"{"kind":"unknown"}"#,
                r#"{"kind":"unknown","other":true}"#,
            ),
        ] {
            let valid: HostSurfaceDefaultV2 = serde_json::from_str(healthy).unwrap();
            assert_eq!(serde_json::to_string(&valid).unwrap(), healthy);
            assert!(serde_json::from_str::<HostSurfaceDefaultV2>(invalid).is_err());
        }
    }

    #[test]
    fn default_dto_preserves_three_wires_and_rejects_duplicate_fields() {
        for wire in [
            r#"{"kind":"none"}"#,
            r#"{"kind":"unknown"}"#,
            r#"{"kind":"value","value":null}"#,
            r#"{"kind":"value","value":1.5}"#,
            r#"{"kind":"value","value":{"inner":null}}"#,
        ] {
            let value: HostSurfaceDefaultV2 = serde_json::from_str(wire).unwrap();
            assert_eq!(serde_json::to_string(&value).unwrap(), wire);
        }
        let value: HostSurfaceDefaultV2 =
            serde_json::from_str(r#"{"value":null,"kind":"value"}"#).unwrap();
        assert!(matches!(value, HostSurfaceDefaultV2::Value { value } if value.is_null()));

        for invalid in [
            r#"{"kind":"none","value":null}"#,
            r#"{"kind":"unknown","value":null}"#,
            r#"{"kind":"none","other":null}"#,
            r#"{"kind":"unknown","other":null}"#,
            r#"{"kind":"none","kind":"none"}"#,
            r#"{"kind":"unknown","kind":"unknown"}"#,
            r#"{"kind":"none","ki\u006ed":"none"}"#,
            r#"{"kind":"value","value":null,"value":null}"#,
            r#"{"kind":"value","value":1,"va\u006cue":2}"#,
            r#"{"kind":"value","value":null,"other":null}"#,
            r#"{"kind":"value"}"#,
            r#"{"value":null}"#,
        ] {
            assert!(
                serde_json::from_str::<HostSurfaceDefaultV2>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn declaration_names_and_enum_values_are_exact_and_unique() {
        let declaration = r#"{"name":"setting","value_type":"enum","enum_values":["a","b"],"default":{"kind":"value","value":"a"}}"#;
        extract_reviewed_host_source(&source_with_declarations(declaration)).unwrap();
        assert!(
            extract_reviewed_host_source(&source_with_declarations(&format!(
                "{declaration},{declaration}"
            )))
            .is_err()
        );
        for choices in [r#"[]"#, r#"["b","a"]"#, r#"["a","a"]"#] {
            let bad = single("enum", choices, r#"{"kind":"value","value":"a"}"#);
            assert!(extract_reviewed_host_source(&bad).is_err());
        }
        extract_reviewed_host_source(&single("enum", r#"["a"]"#, r#"{"kind":"none"}"#)).unwrap();
        assert!(extract_reviewed_host_source(&single("enum", "[]", r#"{"kind":"none"}"#)).is_err());
        assert!(
            extract_reviewed_host_source(&single(
                "enum",
                r#"["a","b"]"#,
                r#"{"kind":"value","value":"c"}"#
            ))
            .is_err()
        );
        assert!(
            extract_reviewed_host_source(&single(
                "bool",
                r#"["a"]"#,
                r#"{"kind":"value","value":true}"#
            ))
            .is_err()
        );
    }

    #[test]
    fn string_list_order_is_semantic_and_object_key_order_is_not() {
        let a = extract_reviewed_host_source(&with_value("string_list", r#"["a","b"]"#)).unwrap();
        let b = extract_reviewed_host_source(&with_value("string_list", r#"["b","a"]"#)).unwrap();
        assert!(!reviewed_defaults_same_value(
            &a.declarations[0].default,
            &b.declarations[0].default
        ));
        let c = extract_reviewed_host_source(&with_value("object", r#"{"b":2,"a":1}"#)).unwrap();
        let d = extract_reviewed_host_source(&with_value("object", r#"{"a":1,"b":2}"#)).unwrap();
        assert!(reviewed_defaults_same_value(
            &c.declarations[0].default,
            &d.declarations[0].default
        ));
    }
    #[test]
    fn source_record_and_default_dto_reject_the_same_decoded_duplicate_keys() {
        use evo_core::contract::{
            decode_host_surface_default_v2_json, decode_host_surface_record_v2_json,
            parse_reviewed_host_source_json,
        };
        for value in [r#"{"x":1,"y":2}"#, r#"{"x":1,"\u0078":2}"#] {
            let valid = value.contains('y');
            let source = with_value("object", value);
            let default = format!(r#"{{"kind":"value","value":{value}}}"#);
            let record = format!(
                r#"{{"schema_version":"rsia.host_surface_record.v2","default":{default}}}"#
            );
            assert_eq!(extract_reviewed_host_source(&source).is_ok(), valid);
            assert_eq!(
                parse_reviewed_host_source_json(&serde_json::to_string(&source).unwrap()).is_ok(),
                valid
            );
            assert_eq!(decode_host_surface_record_v2_json(&record).is_ok(), valid);
            assert_eq!(decode_host_surface_default_v2_json(&default).is_ok(), valid);
            assert_eq!(
                serde_json::from_str::<HostSurfaceDefaultV2>(&default).is_ok(),
                valid
            );
        }
        let source = with_value("bool", "true");
        let envelope = serde_json::to_string(&source).unwrap();
        let duplicate = envelope.replacen(
            r#""source_kind":"reviewed_fixture""#,
            r#""source_kind":"reviewed_fixture","source_ki\u006ed":"reviewed_fixture""#,
            1,
        );
        assert!(parse_reviewed_host_source_json(&duplicate).is_err());
        // Record parsing checks untrusted JSON and its schema label only. It
        // intentionally does not grant a complete record identity or source.
        let minimal = r#"{"schema_version":"rsia.host_surface_record.v2"}"#;
        assert!(decode_host_surface_record_v2_json(minimal).is_ok());
        assert!(decode_host_surface_record_v2_json(&format!("{minimal} true")).is_err());
        assert!(
            decode_host_surface_record_v2_json(
                r#"{"schema_version":"rsia.host_surface_record.v1"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_escapes_and_unicode_surrogates_fail_across_new_raw_json_entries() {
        use evo_core::contract::{
            decode_host_surface_default_v2_json, decode_host_surface_record_v2_json,
        };
        for value in [
            r#""\u00e9""#,
            r#""\ud83d\ude00""#,
            r#""\x00""#,
            r#""\uZZZZ""#,
            r#""\ud800""#,
            r#""\udc00""#,
            r#""\ud800x""#,
            "\"line\nfeed\"",
        ] {
            let valid = matches!(value, r#""\u00e9""# | r#""\ud83d\ude00""#);
            let default = format!(r#"{{"kind":"value","value":{value}}}"#);
            let record = format!(
                r#"{{"schema_version":"rsia.host_surface_record.v2","default":{default}}}"#
            );
            assert_eq!(
                extract_reviewed_host_source(&with_value("string", value)).is_ok(),
                valid,
                "{value}"
            );
            assert_eq!(
                decode_host_surface_record_v2_json(&record).is_ok(),
                valid,
                "{value}"
            );
            assert_eq!(
                decode_host_surface_default_v2_json(&default).is_ok(),
                valid,
                "{value}"
            );
            assert_eq!(
                serde_json::from_str::<HostSurfaceDefaultV2>(&default).is_ok(),
                valid,
                "{value}"
            );
        }
    }

    #[test]
    fn new_json_resource_limits_have_healthy_byte_node_and_depth_boundaries() {
        use evo_core::Error;
        use evo_core::contract::{
            REVIEWED_HOST_JSON_MAX_DEPTH, REVIEWED_HOST_JSON_MAX_NODES,
            REVIEWED_HOST_JSON_MAX_UTF8_BYTES, decode_host_surface_default_v2_json,
            decode_host_surface_record_v2_json,
        };
        let base = r#"{"schema_version":"rsia.host_surface_record.v2"}"#;
        let at_byte_limit = format!(
            "{base}{}",
            " ".repeat(REVIEWED_HOST_JSON_MAX_UTF8_BYTES - base.len())
        );
        decode_host_surface_record_v2_json(&at_byte_limit).unwrap();
        assert!(
            matches!(decode_host_surface_record_v2_json(&(at_byte_limit + " ")),
            Err(Error::Invalid(reason)) if reason == "reviewed host JSON resource limit: UTF-8 bytes")
        );
        let node_record = |items: usize| {
            format!(
                r#"{{"schema_version":"rsia.host_surface_record.v2","payload":[{}]}}"#,
                std::iter::repeat_n("null", items)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        decode_host_surface_record_v2_json(&node_record(REVIEWED_HOST_JSON_MAX_NODES - 3)).unwrap();
        assert!(
            matches!(decode_host_surface_record_v2_json(&node_record(REVIEWED_HOST_JSON_MAX_NODES - 2)),
            Err(Error::Invalid(reason)) if reason == "reviewed host JSON resource limit: nodes")
        );
        let depth_record = |depth: usize| {
            format!(
                r#"{{"schema_version":"rsia.host_surface_record.v2","payload":{}}}"#,
                nested_object(depth, "0")
            )
        };
        decode_host_surface_record_v2_json(&depth_record(REVIEWED_HOST_JSON_MAX_DEPTH - 1))
            .unwrap();
        assert!(
            matches!(decode_host_surface_record_v2_json(&depth_record(REVIEWED_HOST_JSON_MAX_DEPTH)),
            Err(Error::Invalid(reason)) if reason == "reviewed host JSON resource limit: depth")
        );
        let deep_default = format!(r#"{{"kind":"value","value":{}}}"#, nested_object(200, "0"));
        let parsed = decode_host_surface_default_v2_json(&deep_default).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), deep_default);
        let too_wide = format!(
            r#"{{"kind":"value","value":[{}]}}"#,
            vec!["0"; 4096].join(",")
        );
        assert!(matches!(decode_host_surface_default_v2_json(&too_wide),
            Err(Error::Invalid(reason)) if reason == "reviewed host JSON resource limit: normalized default bytes"));
    }

    #[test]
    fn number_dense_defaults_pin_one_normalized_4096_byte_budget_across_entries() {
        use evo_core::contract::{
            decode_host_surface_default_v2_json, decode_host_surface_record_v2_json,
        };
        for number in ["1", "1.0", "1e0"] {
            let normalized_value = |items| format!(r#"{{"":[{}]}}"#, vec![number; items].join(","));
            let value = normalized_value(2045);
            let projection = extract_reviewed_host_source(&with_value("object", &value)).unwrap();
            assert_eq!(
                projected_value(&projection).comparison_projection().len(),
                4096
            );
            assert_eq!(
                projected_value(&projection).comparison_projection(),
                format!(r#"{{"":[{}]}}"#, vec!["1"; 2045].join(","))
            );
            let default = format!(r#"{{"kind":"value","value":{value}}}"#);
            decode_host_surface_default_v2_json(&default).unwrap();
            serde_json::from_str::<HostSurfaceDefaultV2>(&default).unwrap();
            let record = format!(
                r#"{{"schema_version":"rsia.host_surface_record.v2","default":{default}}}"#
            );
            decode_host_surface_record_v2_json(&record).unwrap();
            let over = normalized_value(2046);
            assert!(extract_reviewed_host_source(&with_value("object", &over)).is_err());
            let default = format!(r#"{{"kind":"value","value":{over}}}"#);
            assert!(decode_host_surface_default_v2_json(&default).is_err());
            assert!(serde_json::from_str::<HostSurfaceDefaultV2>(&default).is_err());
            let record = format!(
                r#"{{"schema_version":"rsia.host_surface_record.v2","default":{default}}}"#
            );
            assert!(decode_host_surface_record_v2_json(&record).is_err());
        }
    }
}

mod typed_upgrade_v2_data_contract {
    use evo_core::contract::{
        HostCapabilities, HostSurfaceClassificationV2, HostSurfaceDeclarationV2,
        HostSurfaceDefaultV2, HostSurfaceManifest, HostSurfaceManifestV2, HostSurfacePredecessorV2,
        HostSurfaceRegistrationV2, HostSurfaceRenameV2, HostSurfaceUpgradeDiffV2,
        HostSurfaceValueTypeV2, RetiredHostSurfaceV2, ReviewedHostSourceEnvelopeV1, SURFACE_SCHEMA,
        SURFACE_SCHEMA_V2, SurfaceCoverage, extract_reviewed_host_source, field_contract,
        host_surface_upgrade_diff_v2, host_surface_v2_allows_field, host_surface_v2_from_source,
        validate_host_surface_v2_source,
    };
    use evo_core::{Error, Result, fingerprint, hash};
    use serde_json::json;

    const INVALID: &str = "invalid reviewed host source/default";

    fn declaration(name: &str, value_type: &str, choices: &str, default: &str) -> String {
        format!(
            r#"{{"name":"{name}","value_type":"{value_type}","enum_values":{choices},"default":{default}}}"#
        )
    }

    fn source(declarations: &str) -> ReviewedHostSourceEnvelopeV1 {
        ReviewedHostSourceEnvelopeV1 {
            schema_version: "rsia.host_source_envelope.v1".into(),
            source_kind: "reviewed_fixture".into(),
            source_schema: "rsia.host_reviewed_declarations.v1".into(),
            extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
            original_utf8: format!(
                "\r\n{{\"schema_version\":\"rsia.host_reviewed_declarations.v1\",\"host\":\"reference\",\"host_version\":\"0.1.0\",\"declarations\":[{declarations}]}}\r\n"
            ),
        }
    }

    fn unsupported(name: &str) -> HostSurfaceClassificationV2 {
        HostSurfaceClassificationV2 {
            name: name.into(),
            coverage: SurfaceCoverage::Unsupported,
            mapped_field: None,
            consumer: None,
            reason: "Data-only fixture; no runtime support claim".into(),
            renamed_from: None,
            test_ref: None,
            capability: None,
        }
    }

    fn supported(name: &str, field: &str, test_ref: &str) -> HostSurfaceClassificationV2 {
        let contract = field_contract(field).unwrap();
        HostSurfaceClassificationV2 {
            name: name.into(),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some(field.into()),
            consumer: Some(contract.consumer.into()),
            reason: "Data-only metadata; no passed behavior evidence or authority".into(),
            renamed_from: None,
            test_ref: Some(test_ref.into()),
            capability: Some("fixture-cap".into()),
        }
    }

    fn registration(
        classifications: Vec<HostSurfaceClassificationV2>,
        retired: Vec<RetiredHostSurfaceV2>,
    ) -> HostSurfaceRegistrationV2 {
        HostSurfaceRegistrationV2 {
            adapter_version: "0.1.0".into(),
            classifications,
            retired,
        }
    }

    fn retired(name: &str) -> RetiredHostSurfaceV2 {
        RetiredHostSurfaceV2 {
            name: name.into(),
            reason: "Removed by this data-contract version".into(),
        }
    }

    fn build(
        source: &ReviewedHostSourceEnvelopeV1,
        registration: &HostSurfaceRegistrationV2,
    ) -> (HostSurfaceManifestV2, Vec<HostSurfaceDeclarationV2>) {
        let (manifest, typed) = host_surface_v2_from_source(source, registration).unwrap();
        let names = typed
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<_>>();
        validate_host_surface_v2_source(&manifest, source, &names, &typed).unwrap();
        (manifest, typed)
    }

    fn reject<T: std::fmt::Debug>(result: Result<T>, expected: &str) {
        assert!(
            matches!(result, Err(Error::Invalid(ref message)) if message == expected),
            "expected Invalid({expected:?}), got {result:?}"
        );
    }

    #[test]
    fn real_source_producer_and_validator_bind_raw_source_and_typed_fields() {
        let raw = declaration(
            "setting",
            "object",
            "[]",
            r#"{"kind":"value","value":{"b":2,"a":1}}"#,
        );
        let original = source(&raw);
        let reg = registration(vec![unsupported("setting")], vec![]);
        let (manifest, typed) = build(&original, &reg);
        assert_eq!(manifest.schema_version, SURFACE_SCHEMA_V2);
        assert_eq!(manifest.source_kind, "reviewed_fixture");
        assert_eq!(
            manifest.source_digest,
            hash(original.original_utf8.as_bytes())
        );
        assert_eq!(
            manifest.source_envelope_digest,
            fingerprint(&original).unwrap()
        );
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].name, "setting");
        match &typed[0].default {
            HostSurfaceDefaultV2::Value { value } => assert_eq!(value, &json!({"a":1,"b":2})),
            other => panic!("expected real object default, got {other:?}"),
        }
        for fault in [
            "source",
            "source_digest",
            "envelope_digest",
            "typed_missing",
            "typed_type",
            "typed_enum",
            "typed_default",
            "manifest_default",
        ] {
            let mut changed_source = original.clone();
            let mut changed_manifest = manifest.clone();
            let mut changed_typed = typed.clone();
            match fault {
                "source" => {
                    changed_source = source(&declaration(
                        "setting",
                        "object",
                        "[]",
                        r#"{"kind":"value","value":{"b":2,"a":2}}"#,
                    ));
                    build(&changed_source, &reg);
                    assert_ne!(
                        hash(changed_source.original_utf8.as_bytes()),
                        manifest.source_digest
                    );
                }
                "source_digest" => changed_manifest.source_digest = hash(b"wrong stored source"),
                "envelope_digest" => {
                    changed_manifest.source_envelope_digest = hash(b"wrong envelope")
                }
                "typed_missing" => {
                    changed_typed.pop();
                }
                "typed_type" => changed_typed[0].value_type = HostSurfaceValueTypeV2::String,
                "typed_enum" => changed_typed[0].enum_values = vec!["extra".into()],
                "typed_default" => {
                    changed_typed[0].default = HostSurfaceDefaultV2::Value {
                        value: json!({"a":2,"b":2}),
                    }
                }
                "manifest_default" => {
                    changed_manifest.items[0].default = HostSurfaceDefaultV2::Value {
                        value: json!({"a":2,"b":2}),
                    }
                }
                _ => unreachable!(),
            }
            let names = typed
                .iter()
                .map(|item| item.name.clone())
                .collect::<Vec<_>>();
            reject(
                validate_host_surface_v2_source(
                    &changed_manifest,
                    &changed_source,
                    &names,
                    &changed_typed,
                ),
                INVALID,
            );
        }
    }

    #[test]
    fn classification_has_exact_consumers_owners_unique_mappings_and_supported_requirements() {
        let declarations = [
            declaration("alpha", "bool", "[]", r#"{"kind":"none"}"#),
            declaration("beta", "string", "[]", r#"{"kind":"none"}"#),
        ]
        .join(",");
        let original = source(&declarations);
        let reg = registration(
            vec![
                supported("alpha", "skill.content", "metadata-alpha"),
                supported("beta", "host.model", "metadata-beta"),
            ],
            vec![],
        );
        let (manifest, typed) = build(&original, &reg);
        assert_eq!(manifest.items[0].consumer.as_deref(), Some("compiler"));
        assert_eq!(manifest.items[1].consumer.as_deref(), Some("runner"));
        for fault in [
            "consumer",
            "protected",
            "mapped_duplicate",
            "missing_test_ref",
            "blank_test_ref",
            "missing_capability",
            "classification_duplicate",
            "unregistered_field",
        ] {
            let mut changed = reg.clone();
            match fault {
                "consumer" => changed.classifications[0].consumer = Some("runner".into()),
                "protected" => {
                    let field = field_contract("namespace").unwrap();
                    changed.classifications[0].mapped_field = Some(field.path.into());
                    changed.classifications[0].consumer = Some(field.consumer.into());
                }
                "mapped_duplicate" => {
                    changed.classifications[1].mapped_field = Some("skill.content".into());
                    changed.classifications[1].consumer = Some("compiler".into());
                }
                "missing_test_ref" => changed.classifications[0].test_ref = None,
                "blank_test_ref" => changed.classifications[0].test_ref = Some(" \t".into()),
                "missing_capability" => changed.classifications[0].capability = None,
                "classification_duplicate" => {
                    changed.classifications.push(reg.classifications[0].clone())
                }
                "unregistered_field" => {
                    changed.classifications[0].mapped_field = Some("unregistered.field".into())
                }
                _ => unreachable!(),
            }
            let expected = if fault == "unregistered_field" {
                "no consumer for field unregistered.field"
            } else {
                INVALID
            };
            reject(host_surface_v2_from_source(&original, &changed), expected);
        }
        let names = vec!["alpha".into(), "beta".into()];
        reject(
            validate_host_surface_v2_source(
                &manifest,
                &original,
                &["alpha".into(), "alpha".into()],
                &typed,
            ),
            INVALID,
        );
        let mut duplicate_typed = typed.clone();
        duplicate_typed[1] = typed[0].clone();
        reject(
            validate_host_surface_v2_source(&manifest, &original, &names, &duplicate_typed),
            INVALID,
        );
        let mut duplicate_items = manifest.clone();
        duplicate_items.items[1] = manifest.items[0].clone();
        reject(
            validate_host_surface_v2_source(&duplicate_items, &original, &names, &typed),
            INVALID,
        );
        for (value_type, default) in [
            ("bool", r#"{"kind":"unknown"}"#),
            ("opaque", r#"{"kind":"none"}"#),
        ] {
            let input = source(&declaration("setting", value_type, "[]", default));
            extract_reviewed_host_source(&input).unwrap();
            build(&input, &registration(vec![unsupported("setting")], vec![]));
            reject(
                host_surface_v2_from_source(
                    &input,
                    &registration(
                        vec![supported("setting", "skill.content", "metadata-only")],
                        vec![],
                    ),
                ),
                INVALID,
            );
        }
        // No registered FieldContract currently has Diagnostics ownership.
        // Unknown-field rejection above is not coverage of that owner branch.
    }

    #[test]
    fn validated_data_projection_requires_capability_in_both_sets_for_the_exact_field() {
        let input = source(&declaration("setting", "bool", "[]", r#"{"kind":"none"}"#));
        let (manifest, _) = build(
            &input,
            &registration(
                vec![supported("setting", "skill.content", "metadata-only")],
                vec![],
            ),
        );
        for available in [false, true] {
            for granted in [false, true] {
                let mut capabilities = HostCapabilities {
                    available: ["other-cap".into()].into_iter().collect(),
                    granted: ["other-cap".into()].into_iter().collect(),
                };
                if available {
                    capabilities.available.insert("fixture-cap".into());
                }
                if granted {
                    capabilities.granted.insert("fixture-cap".into());
                }
                assert_eq!(
                    capabilities.effective().contains("fixture-cap"),
                    available && granted
                );
                assert_eq!(
                    host_surface_v2_allows_field(&manifest, &capabilities, "skill.content"),
                    available && granted
                );
                assert!(!host_surface_v2_allows_field(
                    &manifest,
                    &capabilities,
                    "host.model"
                ));
                assert!(!host_surface_v2_allows_field(
                    &manifest,
                    &capabilities,
                    "unregistered.field"
                ));
            }
        }
        // This boolean data predicate is not supported-scope evidence, attached,
        // actual behavior/smoke, Store authorization or a Host capability grant.
    }

    #[test]
    fn actual_core_upgrade_computes_all_vectors_and_uses_default_value_semantics() {
        let before_source = source(
            &[
                declaration("drop", "bool", "[]", r#"{"kind":"none"}"#),
                declaration("enum", "enum", r#"["a","b"]"#, r#"{"kind":"none"}"#),
                declaration("mapping", "bool", "[]", r#"{"kind":"none"}"#),
                declaration("move", "bool", "[]", r#"{"kind":"none"}"#),
                declaration(
                    "object",
                    "object",
                    "[]",
                    r#"{"kind":"value","value":{"a":1,"b":2}}"#,
                ),
                declaration(
                    "same",
                    "object",
                    "[]",
                    r#"{"kind":"value","value":{"b":2,"a":1}}"#,
                ),
                declaration("type", "bool", "[]", r#"{"kind":"none"}"#),
            ]
            .join(","),
        );
        let before_reg = registration(
            vec![
                unsupported("drop"),
                unsupported("enum"),
                supported("mapping", "skill.content", "metadata-before"),
                unsupported("move"),
                unsupported("object"),
                unsupported("same"),
                unsupported("type"),
            ],
            vec![],
        );
        let (previous, _) = build(&before_source, &before_reg);
        let after_source = source(
            &[
                declaration("add", "bool", "[]", r#"{"kind":"none"}"#),
                declaration("enum", "enum", r#"["a","c"]"#, r#"{"kind":"none"}"#),
                declaration("mapping", "bool", "[]", r#"{"kind":"none"}"#),
                declaration("moved", "bool", "[]", r#"{"kind":"none"}"#),
                declaration(
                    "object",
                    "object",
                    "[]",
                    r#"{"kind":"value","value":{"a":2,"b":2}}"#,
                ),
                declaration(
                    "same",
                    "object",
                    "[]",
                    r#"{"kind":"value","value":{"a":1,"b":2}}"#,
                ),
                declaration("type", "integer", "[]", r#"{"kind":"none"}"#),
            ]
            .join(","),
        );
        let mut moved = unsupported("moved");
        moved.renamed_from = Some("move".into());
        let after_reg = registration(
            vec![
                unsupported("add"),
                unsupported("enum"),
                supported("mapping", "host.model", "metadata-after"),
                moved,
                unsupported("object"),
                unsupported("same"),
                unsupported("type"),
            ],
            vec![retired("drop")],
        );
        let (current, _) = build(&after_source, &after_reg);
        let actual = host_surface_upgrade_diff_v2(
            &current,
            Some(HostSurfacePredecessorV2::Current(&previous)),
        )
        .unwrap();
        assert_eq!(
            actual,
            HostSurfaceUpgradeDiffV2 {
                added: vec!["add".into()],
                removed: vec!["drop".into()],
                renamed: vec![HostSurfaceRenameV2 {
                    from: "move".into(),
                    to: "moved".into()
                }],
                type_changed: vec!["type".into()],
                enum_changed: vec!["enum".into()],
                default_changed: vec!["object".into()],
                mapping_changed: vec!["mapping".into()],
                legacy_untyped: vec![],
            }
        );
        let first = host_surface_upgrade_diff_v2(&current, None);
        // retired needs a real predecessor; first-version registration cannot
        // silently treat a claimed historical retirement as a current addition.
        reject(first, INVALID);
        let mut external = serde_json::to_value(&after_reg).unwrap();
        external["upgrade_diff"] = json!({"added":["invented"]});
        let error = serde_json::from_value::<HostSurfaceRegistrationV2>(external).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("unknown field \u{60}upgrade_diff\u{60}")
        );
    }

    #[test]
    fn changed_supported_data_requires_renewed_test_metadata_without_minting_behavior_evidence() {
        for fault in ["type", "enum", "default", "rename", "mapping"] {
            let (old_type, old_choices, old_default, new_type, new_choices, new_default) =
                match fault {
                    "type" => (
                        "bool",
                        "[]",
                        r#"{"kind":"none"}"#,
                        "integer",
                        "[]",
                        r#"{"kind":"none"}"#,
                    ),
                    "enum" => (
                        "enum",
                        r#"["a","b"]"#,
                        r#"{"kind":"none"}"#,
                        "enum",
                        r#"["a","c"]"#,
                        r#"{"kind":"none"}"#,
                    ),
                    "default" => (
                        "bool",
                        "[]",
                        r#"{"kind":"value","value":false}"#,
                        "bool",
                        "[]",
                        r#"{"kind":"value","value":true}"#,
                    ),
                    _ => (
                        "bool",
                        "[]",
                        r#"{"kind":"none"}"#,
                        "bool",
                        "[]",
                        r#"{"kind":"none"}"#,
                    ),
                };
            let old_source = source(&declaration("setting", old_type, old_choices, old_default));
            let (previous, _) = build(
                &old_source,
                &registration(
                    vec![supported("setting", "skill.content", "metadata-before")],
                    vec![],
                ),
            );
            assert_eq!(
                host_surface_upgrade_diff_v2(
                    &previous,
                    Some(HostSurfacePredecessorV2::Current(&previous))
                )
                .unwrap(),
                HostSurfaceUpgradeDiffV2::default()
            );
            let name = if fault == "rename" {
                "renamed"
            } else {
                "setting"
            };
            let new_source = source(&declaration(name, new_type, new_choices, new_default));
            let field = if fault == "mapping" {
                "host.model"
            } else {
                "skill.content"
            };
            let mut class = supported(name, field, "metadata-before");
            if fault == "rename" {
                class.renamed_from = Some("setting".into());
            }
            let mut reg = registration(vec![class], vec![]);
            let (unrenewed, _) = build(&new_source, &reg);
            reject(
                host_surface_upgrade_diff_v2(
                    &unrenewed,
                    Some(HostSurfacePredecessorV2::Current(&previous)),
                ),
                "changed supported surface needs a renewed test_ref",
            );
            reg.classifications[0].test_ref = Some("metadata-after".into());
            let (renewed, _) = build(&new_source, &reg);
            let actual = host_surface_upgrade_diff_v2(
                &renewed,
                Some(HostSurfacePredecessorV2::Current(&previous)),
            )
            .unwrap();
            let mut expected = HostSurfaceUpgradeDiffV2::default();
            match fault {
                "type" => expected.type_changed = vec![name.into()],
                "enum" => expected.enum_changed = vec![name.into()],
                "default" => expected.default_changed = vec![name.into()],
                "rename" => {
                    expected.renamed = vec![HostSurfaceRenameV2 {
                        from: "setting".into(),
                        to: name.into(),
                    }]
                }
                "mapping" => expected.mapping_changed = vec![name.into()],
                _ => unreachable!(),
            }
            assert_eq!(actual, expected, "{fault}");
        }
        // Metadata renewal alone never registers passed behavior tests or
        // authorizes a supported runtime projection, attached or native host.
    }

    #[test]
    fn retirement_rename_and_legacy_untyped_boundaries_keep_original_v1_bytes() {
        let keep = declaration("keep", "bool", "[]", r#"{"kind":"none"}"#);
        let old = declaration("old", "bool", "[]", r#"{"kind":"none"}"#);
        let (previous, _) = build(
            &source(&format!("{keep},{old}")),
            &registration(vec![unsupported("keep"), unsupported("old")], vec![]),
        );
        let current_source = source(&keep);
        let valid_reg = registration(vec![unsupported("keep")], vec![retired("old")]);
        let (removed, _) = build(&current_source, &valid_reg);
        let expected_removed = HostSurfaceUpgradeDiffV2 {
            removed: vec!["old".into()],
            ..HostSurfaceUpgradeDiffV2::default()
        };
        assert_eq!(
            host_surface_upgrade_diff_v2(
                &removed,
                Some(HostSurfacePredecessorV2::Current(&previous))
            )
            .unwrap(),
            expected_removed
        );
        for fault in ["missing", "foreign", "duplicate", "current"] {
            let mut reg = valid_reg.clone();
            match fault {
                "missing" => reg.retired.clear(),
                "foreign" => reg.retired.push(retired("absent")),
                "duplicate" => reg.retired.push(retired("old")),
                "current" => reg.retired.push(retired("keep")),
                _ => unreachable!(),
            }
            if matches!(fault, "duplicate" | "current") {
                reject(host_surface_v2_from_source(&current_source, &reg), INVALID);
            } else {
                let (candidate, _) = build(&current_source, &reg);
                reject(
                    host_surface_upgrade_diff_v2(
                        &candidate,
                        Some(HostSurfacePredecessorV2::Current(&previous)),
                    ),
                    INVALID,
                );
            }
        }
        let new = declaration("new", "bool", "[]", r#"{"kind":"none"}"#);
        let other = declaration("other", "bool", "[]", r#"{"kind":"none"}"#);
        let rename_source = source(&format!("{keep},{new},{other}"));
        let mut renamed = unsupported("new");
        renamed.renamed_from = Some("old".into());
        let rename_reg = registration(
            vec![unsupported("keep"), renamed, unsupported("other")],
            vec![],
        );
        let (healthy, _) = build(&rename_source, &rename_reg);
        let expected_rename = HostSurfaceUpgradeDiffV2 {
            added: vec!["other".into()],
            renamed: vec![HostSurfaceRenameV2 {
                from: "old".into(),
                to: "new".into(),
            }],
            ..HostSurfaceUpgradeDiffV2::default()
        };
        assert_eq!(
            host_surface_upgrade_diff_v2(
                &healthy,
                Some(HostSurfacePredecessorV2::Current(&previous))
            )
            .unwrap(),
            expected_rename
        );
        for fault in ["foreign", "duplicate", "retired"] {
            let mut reg = rename_reg.clone();
            match fault {
                "foreign" => reg.classifications[1].renamed_from = Some("absent".into()),
                "duplicate" => reg.classifications[2].renamed_from = Some("old".into()),
                "retired" => reg.retired.push(retired("old")),
                _ => unreachable!(),
            }
            let (candidate, _) = build(&rename_source, &reg);
            reject(
                host_surface_upgrade_diff_v2(
                    &candidate,
                    Some(HostSurfacePredecessorV2::Current(&previous)),
                ),
                INVALID,
            );
        }
        let present_source = source(&format!("{keep},{new},{old}"));
        let present_reg = registration(
            vec![unsupported("keep"), unsupported("new"), unsupported("old")],
            vec![],
        );
        let (present, _) = build(&present_source, &present_reg);
        assert_eq!(
            host_surface_upgrade_diff_v2(
                &present,
                Some(HostSurfacePredecessorV2::Current(&previous))
            )
            .unwrap()
            .added,
            vec!["new".to_string()]
        );
        let mut contradiction = present_reg;
        contradiction.classifications[1].renamed_from = Some("old".into());
        let (candidate, _) = build(&present_source, &contradiction);
        reject(
            host_surface_upgrade_diff_v2(
                &candidate,
                Some(HostSurfacePredecessorV2::Current(&previous)),
            ),
            INVALID,
        );

        let raw = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/reference-host/host_surface.v1.json"),
        )
        .unwrap();
        assert_eq!(raw.len(), 722);
        assert_eq!(
            hash(&raw),
            "bf827beee4191e8da289d73d16582fcd9c2c15fab71fb3febbdae514ab539da1"
        );
        let legacy: HostSurfaceManifest = serde_json::from_slice(&raw).unwrap();
        assert_eq!(SURFACE_SCHEMA, "rsia.host_surface.v1");
        assert_eq!(legacy.schema_version, SURFACE_SCHEMA);
        let old_wire = serde_json::to_vec(&legacy).unwrap();
        let legacy_names = ["instruction", "model", "theme"];
        legacy
            .validate_against_extraction(&legacy_names.map(String::from))
            .unwrap();
        let declarations = legacy_names
            .iter()
            .map(|name| declaration(name, "string", "[]", r#"{"kind":"none"}"#))
            .collect::<Vec<_>>()
            .join(",");
        let mut runtime_owned = unsupported("model");
        runtime_owned.coverage = SurfaceCoverage::RuntimeOwned;
        let (typed, _) = build(
            &source(&declarations),
            &registration(
                vec![
                    supported("instruction", "skill.content", "metadata-v2"),
                    runtime_owned,
                    unsupported("theme"),
                ],
                vec![],
            ),
        );
        let legacy_diff =
            host_surface_upgrade_diff_v2(&typed, Some(HostSurfacePredecessorV2::Legacy(&legacy)))
                .unwrap();
        assert_eq!(
            legacy_diff,
            HostSurfaceUpgradeDiffV2 {
                legacy_untyped: legacy_names.map(String::from).to_vec(),
                ..HostSurfaceUpgradeDiffV2::default()
            }
        );
        assert_eq!(serde_json::to_vec(&legacy).unwrap(), old_wire);
        assert_eq!(
            hash(&raw),
            "bf827beee4191e8da289d73d16582fcd9c2c15fab71fb3febbdae514ab539da1"
        );
        let mut extended = serde_json::from_slice::<serde_json::Value>(&raw).unwrap();
        extended["retired"] = json!([]);
        let error = serde_json::from_value::<HostSurfaceManifest>(extended).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("unknown field \u{60}retired\u{60}")
        );
        // Legacy comparison explicitly records lack of typed history; it
        // neither upgrades old bytes nor grants legacy new-prepare eligibility.
    }
}
