use evo_core::contract::RunProjection;
use evo_core::evidence::Purpose;
use evo_core::model_protocol::*;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Error, hash};
use serde_json::json;
fn context() -> ModelRequestContext {
    ModelRequestContext {
        request_id: "projection-request".into(),
        namespace: "n".into(),
        purpose: Purpose::Development,
        stage: ModelStage::ReflectFailure,
        episode_id: "episode".into(),
        step: 1,
        attempt: 1,
        parent_skill_digest: hash(b"parent"),
        bundle_digest: hash(b"bundle"),
        source_closure: vec![EvidenceRef {
            id: "run".into(),
            digest: hash(b"source"),
        }],
        model_digest: hash(b"model"),
        tools_digest: hash(b"tools"),
        rules_digest: hash(b"rules"),
        sampling_digest: sampling_digest_v1(
            ModelProtocolId::ChatCompletionsNonstreamV1,
            SamplingProfileId::TemperatureZeroV1,
        )
        .unwrap(),
        revoke_watermark: 0,
        max_suggestions: 2,
    }
}

fn provider() -> ChatProviderProjectionInput {
    ChatProviderProjectionInput {
        model_digest: hash(b"model"),
        provider_model: "registered-model".into(),
        max_output_tokens: 128,
        protocol_id: ModelProtocolId::ChatCompletionsNonstreamV1,
        sampling_profile: SamplingProfileId::TemperatureZeroV1,
    }
}
fn input() -> Vec<ModelInputPart> {
    [
        (ModelInputRole::System, "S"),
        (ModelInputRole::User, "U"),
        (ModelInputRole::Evidence, "E"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (role, content))| ModelInputPart {
        role,
        label: format!("part-{i}"),
        content: content.into(),
    })
    .collect()
}
#[test]
fn sampling_and_both_wire_projections_match_independent_literal_goldens() {
    let p = provider();
    assert_eq!(
        sampling_digest_v1(p.protocol_id, p.sampling_profile).unwrap(),
        hash(include_bytes!("golden/chat_sampling_v1.json"))
    );
    assert_eq!(
        hash(include_bytes!("golden/chat_sampling_v1.json")),
        "9589cea8cc1270c521a78981748294c017cd46b2459bc4446dbcaa4a431bb188"
    );
    let request = ModelRequest::build(context(), input()).unwrap();
    let bound = project_bound_chat_v1(&request, &p).unwrap();
    assert_eq!(
        bound.wire_utf8(),
        include_bytes!("golden/chat_bound_v1.json")
    );
    assert_eq!(
        bound.payload_digest(),
        "3952b35d781b385a18098b074ff8f23e3837cae7056b58179ed5abe608e526e5"
    );
    let projection = RunProjection {
        bundle_digest: hash(b"bundle"),
        instructions: vec!["I1".into(), "I2".into()],
        tools: vec!["not-an-http-field".into()],
    };
    let formal = project_formal_chat_v1("M", &projection, "T", &p).unwrap();
    assert_eq!(
        formal.wire_utf8(),
        include_bytes!("golden/chat_formal_v1.json")
    );
    assert_eq!(
        formal.payload_digest(),
        "b933b318da18efe5cf257a601254471287f5be975cb91b4bd2d2e65960cb3e54"
    );
    verify_projection_v1(
        p.protocol_id,
        formal.sampling_digest(),
        formal.payload_digest(),
        &formal,
    )
    .unwrap();
    assert_eq!(formal.sampling_digest(), bound.sampling_digest());
    let body: serde_json::Value = serde_json::from_slice(formal.wire_utf8()).unwrap();
    assert_eq!(body.as_object().unwrap().len(), 4);
    assert_eq!(body["temperature"].as_u64(), Some(0));
}
#[test]
fn arbitrary_unsupported_and_bare_untagged_digests_are_rejected_without_rewriting() {
    for sampling in [
        hash(b"opaque sampling"),
        hash(br#"{"temperature":1}"#),
        hash(br#"{"temperature":0}"#),
    ] {
        let mut context = context();
        context.sampling_digest = sampling;
        let request = ModelRequest::build(context, input()).unwrap();
        let original = serde_json::to_vec(&request).unwrap();
        assert!(
            matches!(project_bound_chat_v1(&request,&provider()),Err(Error::Conflict(s)) if s=="model_sampling_projection_mismatch")
        );
        assert_eq!(serde_json::to_vec(&request).unwrap(), original);
        request.validate().unwrap();
    }
}
#[test]
fn full_payload_changes_are_distinct_from_sampling_and_must_be_verified() {
    let p = provider();
    let request = ModelRequest::build(context(), input()).unwrap();
    let original = project_bound_chat_v1(&request, &p).unwrap();
    for change in 0..5 {
        let mut p = p.clone();
        let mut parts = input();
        match change {
            0 => p.provider_model = "other-model".into(),
            1 => p.max_output_tokens = 127,
            2 => parts.swap(0, 1),
            3 => parts[0].role = ModelInputRole::User,
            _ => parts[2].content = "引号\"与换行\n".into(),
        }
        let changed =
            project_bound_chat_v1(&ModelRequest::build(context(), parts).unwrap(), &p).unwrap();
        assert_ne!(changed.payload_digest(), original.payload_digest());
        assert_eq!(changed.sampling_digest(), original.sampling_digest());
        assert!(
            matches!(verify_projection_v1(p.protocol_id,original.sampling_digest(),original.payload_digest(),&changed),Err(Error::Conflict(s)) if s=="model_payload_projection_mismatch")
        );
    }
    assert!(
        matches!(verify_projection_v1(p.protocol_id,&hash(b"wrong"),original.payload_digest(),&original),Err(Error::Conflict(s)) if s=="model_sampling_projection_mismatch")
    );
}
#[test]
fn protocol_profile_and_projection_inputs_are_closed_and_model_bound() {
    assert!(serde_json::from_value::<ModelProtocolId>(json!("streaming")).is_err());
    assert!(serde_json::from_value::<SamplingProfileId>(json!("temperature_one_v1")).is_err());
    let mut v = serde_json::to_value(provider()).unwrap();
    v["top_p"] = json!(1);
    assert!(serde_json::from_value::<ChatProviderProjectionInput>(v).is_err());
    let request = ModelRequest::build(context(), input()).unwrap();
    let mut p = provider();
    p.model_digest = hash(b"other-model");
    assert!(matches!(
        project_bound_chat_v1(&request, &p),
        Err(Error::Forbidden)
    ));
    for n in [0, 32769] {
        let mut p = provider();
        p.max_output_tokens = n;
        assert!(project_bound_chat_v1(&request, &p).is_err());
    }
}
