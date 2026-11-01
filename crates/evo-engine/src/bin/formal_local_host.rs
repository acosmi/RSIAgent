//! Finite trusted host/grader entry point. A deterministic probe remains a
//! ProgramFixture. The external mode obtains an opaque capability only from
//! the registered transport and the observed OS principal/workspace.
use evo_core::{Context, Error, Result, Role};
use evo_engine::broker::{BrokerConfig, PersistentModelBroker};
use evo_engine::data_directory_lock::mount_gate;
use evo_engine::formal_authority::{
    FormalAuthorityIssuer, RegisteredExternalTransport, TypedContentRef, prepare_formal_task,
};
use evo_engine::streaming_evaluator::{
    BrokerTaskView, GraderReceiptRequest, IndependentEvaluationControl,
};
use evo_storage::Store;
use serde::Deserialize;
use std::io::Read;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Fixture {
        input: String,
    },
    /// Run as the registered grader OS user in its private registered workspace.
    /// The finite socket ingress authenticates Admin/Evaluator/Worker peers.
    Authority {
        database: String,
        host: Box<evo_engine::formal_authority::LoadedRegisteredFormalHost>,
        principals: evo_engine::formal_authority::RegisteredPrincipalSet,
        socket: String,
        connections: u16,
    },
    /// Run as the actual operator/worker OS user. Check the authority's kernel
    /// UID before sending the credential from this subject's private environment.
    Operator {
        socket: String,
        expected_authority_uid: u32,
        credential_environment_key: String,
        command: evo_engine::formal_authority::FormalOperatorCommand,
    },
    Execute {
        database: String,
        namespace: String,
        actor: String,
        provider_registration_id: String,
        task: BrokerTaskView,
        mandatory_context: String,
        task_input_utf8: String,
        billing_scope: String,
        currency: String,
        pricing_version: String,
        max_cost_micros: i64,
        lease_seconds: i64,
    },
    Grade {
        database: String,
        namespace: String,
        actor: String,
        receipt_id: String,
        ticket_id: String,
        target_id: String,
        candidate_execution_receipt_id: String,
        baseline_execution_receipt_id: String,
        issued_at_unix_seconds: i64,
    },
    Cost {
        database: String,
        namespace: String,
        actor: String,
        ticket_id: String,
        profile_registration_id: String,
        measurements: evo_core::evaluation::CompleteEpisodeCostMeasurements,
        sources: Vec<TypedContentRef>,
    },
    CaptureCost {
        database: String,
        namespace: String,
        actor: String,
        ticket_id: String,
        profile_registration_id: String,
    },
    Finalize {
        database: String,
        namespace: String,
        actor: String,
        ticket_id: String,
    },
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(value) => {
            println!("{value}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            // Error variants carry no credentials, protected inputs or oracle.
            let code = match error {
                Error::Forbidden => "forbidden",
                Error::NotFound => "missing",
                Error::Budget => "budget",
                Error::Invalid(_) => "invalid",
                Error::Conflict(_) => "conflict",
                Error::Cancelled => "cancelled",
                Error::ClockRegressed => "clock_regressed",
                Error::Internal => "internal",
            };
            eprintln!("{{\"status\":\"refused\",\"reason\":\"{code}\"}}");
            std::process::ExitCode::from(2)
        }
    }
}
async fn run() -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Internal)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Invalid("host command exceeds bound".into()));
    }
    let command: Command = serde_json::from_slice(&bytes)
        .map_err(|_| Error::Invalid("invalid host command".into()))?;
    if let Command::Fixture { input } = command {
        evo_core::text(&input, "fixture input", 128 * 1024)?;
        return Ok(
            serde_json::json!({"schema_version":"rsia.local_host_fixture.v1","output":input,
            "evidence_scope":"program_fixture","ordinary_p_eligible":false,"promotion_eligible":false}),
        );
    }
    if let Command::Authority {
        database,
        host,
        principals,
        socket,
        connections,
    } = command
    {
        return authority(database, *host, principals, socket, connections).await;
    }
    if let Command::Operator {
        socket,
        expected_authority_uid,
        credential_environment_key,
        command,
    } = command
    {
        return operator(
            socket,
            expected_authority_uid,
            credential_environment_key,
            command,
        )
        .await;
    }
    let (database, namespace, actor, role) = match &command {
        Command::Execute {
            database,
            namespace,
            actor,
            ..
        }
        | Command::Cost {
            database,
            namespace,
            actor,
            ..
        }
        | Command::CaptureCost {
            database,
            namespace,
            actor,
            ..
        } => (database, namespace, actor, Role::Host),
        Command::Grade {
            database,
            namespace,
            actor,
            ..
        }
        | Command::Finalize {
            database,
            namespace,
            actor,
            ..
        } => (database, namespace, actor, Role::Evaluator),
        Command::Fixture { .. } | Command::Authority { .. } | Command::Operator { .. } => {
            return Err(Error::Internal);
        }
    };
    // These launch fields are trusted supervisor input. Skill and task text
    // enter only the fixed request projection and never choose a role or path.
    let ctx = Context::new(namespace, actor, role)?;
    let (_data_lock, gate) = mount_gate(std::path::Path::new(database), None)
        .await
        .map_err(|error| Error::Conflict(error.to_string()))?;
    let store = Store::open(std::path::Path::new(database)).await?;
    let result = async {
        gate.admit()
            .map_err(|error| Error::Conflict(error.to_string()))?;
        execute(&ctx, &store, command).await
    }
    .await;
    store.close().await;
    result
}
async fn execute(ctx: &Context, store: &Store, command: Command) -> Result<serde_json::Value> {
    match command {
        Command::Execute {
            provider_registration_id,
            task,
            mandatory_context,
            task_input_utf8,
            billing_scope,
            currency,
            pricing_version,
            max_cost_micros,
            lease_seconds,
            ..
        } => {
            let transport =
                RegisteredExternalTransport::connect(ctx, store, &provider_registration_id).await?;
            let request =
                prepare_formal_task(ctx, store, task, mandatory_context, task_input_utf8).await?;
            let broker = PersistentModelBroker::new(
                store.clone(),
                transport,
                BrokerConfig {
                    billing_scope,
                    actor: ctx.actor().into(),
                    currency,
                    pricing_version,
                    max_cost_micros,
                    lease_seconds,
                },
            )?;
            serde_json::to_value(
                broker
                    .dispatch_formal_task(ctx.namespace(), request)
                    .await?,
            )
            .map_err(|_| Error::Internal)
        }
        Command::Grade {
            receipt_id,
            ticket_id,
            target_id,
            candidate_execution_receipt_id,
            baseline_execution_receipt_id,
            issued_at_unix_seconds,
            ..
        } => serde_json::to_value(
            IndependentEvaluationControl::record_grader_receipt(
                ctx,
                store,
                GraderReceiptRequest {
                    receipt_id,
                    ticket_id,
                    target_id,
                    candidate_execution_receipt_id,
                    baseline_execution_receipt_id,
                    issued_at_unix_seconds,
                },
            )
            .await?,
        )
        .map_err(|_| Error::Internal),
        Command::Cost {
            ticket_id,
            profile_registration_id,
            measurements,
            sources,
            ..
        } => {
            FormalAuthorityIssuer::record_episode_cost(
                ctx,
                store,
                &ticket_id,
                &profile_registration_id,
                measurements,
                sources,
            )
            .await?;
            Ok(serde_json::json!({"status":"actual_cost_sources_recorded","ticket_id":ticket_id}))
        }
        Command::CaptureCost {
            ticket_id,
            profile_registration_id,
            ..
        } => {
            FormalAuthorityIssuer::capture_episode_cost(
                ctx,
                store,
                &ticket_id,
                &profile_registration_id,
            )
            .await?;
            Ok(
                serde_json::json!({"status":"actual_episode_measurements_captured","ticket_id":ticket_id}),
            )
        }
        Command::Finalize { ticket_id, .. } => serde_json::to_value(
            IndependentEvaluationControl::finalize_complete(
                ctx,
                store,
                &ticket_id,
                evo_core::now()?,
            )
            .await?,
        )
        .map_err(|_| Error::Internal),
        Command::Fixture { .. } | Command::Authority { .. } | Command::Operator { .. } => {
            Err(Error::Internal)
        }
    }
}

async fn authority(
    database: String,
    host: evo_engine::formal_authority::LoadedRegisteredFormalHost,
    principals: evo_engine::formal_authority::RegisteredPrincipalSet,
    socket: String,
    connections: u16,
) -> Result<serde_json::Value> {
    evo_engine::formal_authority::PrincipalAuthorityService::serve_owned(
        std::path::PathBuf::from(database),
        host,
        principals,
        std::path::PathBuf::from(socket),
        connections,
    )
    .await
}
async fn operator(
    socket: String,
    expected_authority_uid: u32,
    credential_environment_key: String,
    command: evo_engine::formal_authority::FormalOperatorCommand,
) -> Result<serde_json::Value> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    evo_core::identifier(&credential_environment_key)?;
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|_| Error::Forbidden)?;
    let peer = stream.peer_cred().map_err(|_| Error::Forbidden)?;
    if peer.uid() != expected_authority_uid {
        return Err(Error::Forbidden);
    }
    let credential = std::env::var(credential_environment_key).map_err(|_| Error::Forbidden)?;
    if credential.len() < 32 || credential.len() > 4096 {
        return Err(Error::Forbidden);
    }
    let mut bytes =
        serde_json::to_vec(&serde_json::json!({"credential":credential,"command":command}))
            .map_err(|_| Error::Internal)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Forbidden);
    }
    stream
        .write_u32(u32::try_from(bytes.len()).map_err(|_| Error::Internal)?)
        .await
        .map_err(|_| Error::Internal)?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| Error::Internal)?;
    bytes.fill(0);
    let size = tokio::time::timeout(std::time::Duration::from_secs(300), stream.read_u32())
        .await
        .map_err(|_| Error::Cancelled)?
        .map_err(|_| Error::Internal)? as usize;
    if size == 0 || size > 1024 * 1024 {
        return Err(Error::Forbidden);
    }
    let mut response = vec![0; size];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|_| Error::Internal)?;
    evo_engine::formal_authority::decode_formal_operator_response(&response)
}
