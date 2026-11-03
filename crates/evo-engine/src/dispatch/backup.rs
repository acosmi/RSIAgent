//! One non-paying management step backed by the Store's owned backup stages.
use super::*;
use evo_storage::lifecycle::{BackupDurability, BackupName, BackupPhase, BackupPublication};
use std::time::{Duration, Instant};

const UNVERIFIED: &str = "backup_publication_unverified";

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TestPoint {
    Preparing,
    BeforePublication,
    AfterPublication,
}

#[cfg(test)]
pub(super) struct TestPause {
    point: TestPoint,
    reached: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    fail: bool,
}

#[cfg(test)]
async fn test_checkpoint(control: &WorkerControl, point: TestPoint) -> Result<()> {
    let pause = {
        let mut pause = control.backup_pause.lock().map_err(|_| Error::Internal)?;
        if pause.as_ref().is_some_and(|pause| pause.point == point) {
            pause.take()
        } else {
            None
        }
    };
    if let Some(pause) = pause {
        pause.reached.notify_one();
        pause.resume.notified().await;
        if pause.fail {
            return Err(Error::Internal);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackupCreateRequest {
    pub schema_version: String,
    pub request_key: String,
    pub relative_name: BackupName,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupStatus {
    pub phase: BackupPhase,
    pub durability: BackupDurability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
}

pub(super) fn status(record: &VersionedManagementJob) -> Option<BackupStatus> {
    let VersionedManagementJob::V2(job) = record else {
        return None;
    };
    job.backup.as_ref().map(|progress| BackupStatus {
        phase: progress.phase,
        durability: progress.durability,
        manifest_sha256: progress.manifest_sha256().map(str::to_owned),
    })
}

fn outcome(publication: BackupPublication) -> JobOutcome {
    let verified = publication.durability == BackupDurability::Verified;
    JobOutcome {
        state: if verified {
            ManagementJobState::Succeeded
        } else {
            ManagementJobState::Blocked
        },
        step: "backup_published".into(),
        result: Some(ManagementResultV2::Backup(
            ManagementBackupResultV2::BackupCreated {
                relative_name: publication.relative_name,
                manifest_sha256: publication.manifest_sha256,
                durability: publication.durability,
            },
        )),
        error_code: (!verified).then(|| UNVERIFIED.into()),
    }
}

fn before_publication(error: Error) -> ObservedWorkerExit {
    ObservedWorkerExit::BeforeCommitFailed(JobOutcome {
        state: ManagementJobState::Failed,
        step: "backup_preparation_failed".into(),
        result: None,
        error_code: Some(error_code(&error).into()),
    })
}

pub(super) async fn execute(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJobV2,
    request: BackupCreateRequest,
    control: &WorkerControl,
) -> Result<ObservedWorkerExit> {
    let mut session = store.session().await?;
    let current = match begin_management_step(ctx, &mut session, claimed, control).await {
        Ok(current) => current,
        Err(error) => return Ok(before_publication(error)),
    };
    session.commit().await?;
    let preparing = match store
        .backup(ctx, request.relative_name)?
        .begin(current.generation, current.lease_token.clone())
        .await
    {
        Ok(preparing) => preparing,
        Err(error) => return Ok(before_publication(error)),
    };
    let mut session = match store.session().await {
        Ok(session) => session,
        Err(error) => {
            preparing.discard().await?;
            return Ok(before_publication(error));
        }
    };
    let receipt = async {
        let mut current = begin_management_step(ctx, &mut session, claimed, control).await?;
        current.backup = Some(preparing.progress());
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.backup.preparing", &current.id)
            .await?;
        let remaining = u64::try_from(
            current
                .lease_until
                .checked_sub(now()?)
                .ok_or(Error::ClockRegressed)?,
        )
        .map_err(|_| Error::Conflict("management job lease fence changed".into()))?;
        Ok(Instant::now() + Duration::from_secs(remaining.min(30)))
    }
    .await;
    let deadline = match receipt {
        Ok(deadline) => deadline,
        Err(error) => {
            preparing.discard().await?;
            return Ok(before_publication(error));
        }
    };
    if let Err(error) = session.commit().await {
        preparing.discard().await?;
        return Ok(before_publication(error));
    }
    #[cfg(test)]
    test_checkpoint(control, TestPoint::Preparing).await?;
    let prepared = match preparing
        .prepare(deadline, control.stop_new_steps.clone())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => return Ok(before_publication(error)),
    };
    let progress = prepared.progress();
    let mut session = match store.session().await {
        Ok(session) => session,
        Err(error) => {
            prepared.discard().await?;
            return Ok(before_publication(error));
        }
    };
    let receipt = async {
        let mut current = begin_management_step(ctx, &mut session, claimed, control).await?;
        current.backup = Some(progress.clone());
        session
            .put(ctx, "job", &current.id, ctx.actor(), &current)
            .await?;
        session
            .audit(ctx, "management.backup.prepared", &current.id)
            .await
    }
    .await;
    if let Err(error) = receipt {
        prepared.discard().await?;
        return Ok(before_publication(error));
    }
    if let Err(error) = session.commit().await {
        prepared.discard().await?;
        return Ok(before_publication(error));
    }
    let publication = prepared.release();
    #[cfg(test)]
    test_checkpoint(control, TestPoint::BeforePublication).await?;
    // Failure to borrow this new Session leaves the persisted owned temp intact.
    let mut session = store.session().await?;
    let mut current = match begin_management_step(ctx, &mut session, claimed, control).await {
        Ok(current) => current,
        Err(error) => {
            publication.discard(&mut session).await?;
            return Ok(before_publication(error));
        }
    };
    if current.backup.as_ref().map(fingerprint).transpose()? != Some(fingerprint(&progress)?) {
        // A changed receipt cannot authorize deletion of the old owned temp.
        return Ok(ObservedWorkerExit::Failed(UNVERIFIED.into()));
    }
    let published = match publication.publish(&mut session).await {
        Ok(published) => published,
        Err(error) => return Ok(before_publication(error)),
    };
    #[cfg(test)]
    test_checkpoint(control, TestPoint::AfterPublication).await?;
    current.backup = Some(progress.published(published.durability));
    let completed = outcome(published);
    apply_completed_outcome(&mut current, &completed)?;
    // Any failure after rename leaves Prepared or Published provenance and the
    // real destination for the observed-exit reader; it never deletes output.
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.backup.published", &current.id)
        .await?;
    session.commit().await?;
    Ok(ObservedWorkerExit::Completed(completed))
}

async fn read_publication(
    ctx: &Context,
    store: &Store,
    record: &VersionedManagementJob,
) -> Result<Option<BackupPublication>> {
    let VersionedManagementJob::V2(job) = record else {
        return Ok(None);
    };
    let Some(progress) = &job.backup else {
        return Ok(None);
    };
    if progress.phase == BackupPhase::Preparing {
        return Ok(None);
    }
    let mut session = store.session().await?;
    let row = session
        .object_row(ctx, "artifact", &job.private_input_ref)
        .await?
        .ok_or(Error::NotFound)?;
    if row.owner != ctx.actor() {
        return Err(Error::Forbidden);
    }
    let body = evo_storage::lifecycle::parse_strict_json(&row.body)?;
    let ParsedRequest::BackupCreate(request) = decode_private_request(body, job)? else {
        return Err(Error::Conflict("backup private input differs".into()));
    };
    validate_request_authority(ctx, &ParsedRequest::BackupCreate(request.clone()))?;
    session.commit().await?;
    store
        .backup(ctx, request.relative_name)?
        .verify(progress, ctx, job, &row.body)
        .await
}

pub(super) async fn consumer_progress(
    ctx: &Context,
    store: &Store,
    record: &VersionedManagementJob,
) -> Result<ConsumerProgress> {
    match read_publication(ctx, store, record).await? {
        Some(publication) => Ok(ConsumerProgress::Committed(Box::new(outcome(publication)))),
        None => Ok(ConsumerProgress::Unknown),
    }
}

pub(super) async fn finish_observed(
    ctx: &Context,
    store: &Store,
    claimed: &ManagementJobV2,
    observed: &ObservedWorkerExit,
) -> Result<ManagementJobState> {
    let record = load_job_record(ctx, store, &claimed.id).await?;
    if is_terminal(record.identity().state) {
        return Ok(record.identity().state);
    }
    let publication = match read_publication(ctx, store, &record).await {
        Ok(publication) => publication,
        Err(error @ (Error::ClockRegressed | Error::Internal)) => return Err(error),
        Err(_) => None,
    };
    let mut session = store.session().await?;
    let actual = load_job_record_in_session(ctx, &mut session, &claimed.id).await?;
    if is_terminal(actual.identity().state) {
        let state = actual.identity().state;
        session.commit().await?;
        return Ok(state);
    }
    let mut current = actual.into_unfinished_v2()?;
    // Expiry is closable only here: the caller owns an actual exit/startup witness.
    if !same_claim_fields(&current, claimed) {
        return Err(Error::Conflict("management job lease fence changed".into()));
    }
    now()?;
    if let Some(publication) = publication {
        current.backup = current
            .backup
            .as_ref()
            .map(|saved| saved.published(publication.durability));
        apply_completed_outcome(&mut current, &outcome(publication))?;
    } else if let ObservedWorkerExit::BeforeConsumerFailed(outcome)
    | ObservedWorkerExit::BeforeCommitFailed(outcome) = observed
    {
        apply_completed_outcome(&mut current, outcome)?;
    } else if matches!(observed, ObservedWorkerExit::NeverStarted) {
        apply_completed_outcome(
            &mut current,
            &JobOutcome {
                state: ManagementJobState::Cancelled,
                step: "cancelled".into(),
                result: None,
                error_code: Some("cancelled".into()),
            },
        )?;
    } else {
        current.state = ManagementJobState::Blocked;
        current.error_code = Some(UNVERIFIED.into());
        current.pending_terminal_state = None;
        current.lease_token = None;
        current.lease_until = 0;
        if current.cancel_requested {
            current.cancel_arrival =
                Some(if stored_result_has_side_effect(current.result.as_ref()) {
                    ManagementCancelArrival::AfterSideEffect
                } else {
                    ManagementCancelArrival::BeforeSideEffect
                });
        }
    }
    let state = current.state;
    session
        .put(ctx, "job", &current.id, ctx.actor(), &current)
        .await?;
    session
        .audit(ctx, "management.backup.finish", &current.id)
        .await?;
    session.commit().await?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    #![cfg(unix)]
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::PathBuf;

    struct Fixture {
        _directory: tempfile::TempDir,
        root: PathBuf,
        store: Store,
        admin: Context,
        dispatcher: ManagementDispatcher,
    }

    impl Fixture {
        async fn queued() -> (Self, VersionedManagementJob) {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let database = root.join("rsia.sqlite3");
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&database)
                .unwrap();
            let store = Store::open(&database).await.unwrap();
            let admin = Context::new("n", "admin", Role::Admin).unwrap();
            let mut session = store.session().await.unwrap();
            session
                .bump_watermark(&admin, &hash(b"initial"))
                .await
                .unwrap();
            session.commit().await.unwrap();
            let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
            // The real submit writer and real claim are used; the unit seam
            // only pauses an owned worker at an actual production checkpoint.
            let queued = persist_management(&admin, &store, "backup.create", serde_json::json!({
                "schema_version":"rsia.management.backup_create.v1","request_key":"test","relative_name":"actual"
            })).await.unwrap();
            (
                Self {
                    _directory: directory,
                    root,
                    store,
                    admin,
                    dispatcher,
                },
                queued,
            )
        }
    }

    async fn paused(
        point: TestPoint,
        fail: bool,
    ) -> (
        Fixture,
        ManagementJobV2,
        WorkerControl,
        tokio::task::JoinHandle<Result<ObservedWorkerExit>>,
        Arc<tokio::sync::Notify>,
    ) {
        let (fixture, queued) = Fixture::queued().await;
        let (_, claimed) = fixture
            .dispatcher
            .claim(queued.identity().id)
            .await
            .unwrap();
        let claimed = claimed.unwrap();
        let control = WorkerControl::new();
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *control.backup_pause.lock().unwrap() = Some(TestPause {
            point,
            reached: reached.clone(),
            resume: resume.clone(),
            fail,
        });
        let worker_ctx = fixture.admin.clone();
        let worker_store = fixture.store.clone();
        let worker_claim = claimed.clone();
        let worker_control = control.clone();
        let handle = tokio::spawn(async move {
            let request = load_private_request(&worker_ctx, &worker_store, &worker_claim)
                .await
                .map_err(InputFault::into_error)?;
            execute_request(
                &worker_ctx,
                &worker_store,
                &worker_claim,
                request,
                &worker_control,
                None,
            )
            .await
        });
        reached.notified().await;
        (fixture, claimed, control, handle, resume)
    }

    #[tokio::test]
    async fn backup_cancel_before_prepare_has_no_business_side_effect() {
        let (fixture, queued) = Fixture::queued().await;
        let cancelled = fixture
            .dispatcher
            .cancel(&fixture.admin, queued.identity().id)
            .await
            .unwrap();
        assert_eq!(cancelled.identity().state, ManagementJobState::Cancelled);
        fixture
            .dispatcher
            .run_job(queued.identity().id)
            .await
            .unwrap();
        assert!(!fixture.root.join("backups").exists());
        let current = fixture
            .dispatcher
            .status(&fixture.admin, queued.identity().id)
            .await
            .unwrap();
        assert!(current.result().is_none());
        assert_eq!(current.generation(), 0);
        assert_eq!(
            current.cancel_arrival(),
            Some(ManagementCancelArrival::BeforeSideEffect)
        );
        fixture.store.close().await;
    }

    #[tokio::test]
    async fn backup_stale_generation_cannot_publish_prepared_directory() {
        let (fixture, claimed, _control, handle, resume) =
            paused(TestPoint::BeforePublication, false).await;
        let mut session = fixture.store.session().await.unwrap();
        let record = load_job_record_in_session(&fixture.admin, &mut session, &claimed.id)
            .await
            .unwrap();
        let mut current = record.into_unfinished_v2().unwrap();
        assert_eq!(
            current.backup.as_ref().unwrap().phase,
            BackupPhase::Prepared
        );
        current.generation += 1;
        current.lease_token = Some("new-real-persisted-generation".into());
        session
            .put(
                &fixture.admin,
                "job",
                &current.id,
                fixture.admin.actor(),
                &current,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        resume.notify_one();
        let observed = handle.await.unwrap().unwrap();
        assert!(matches!(
            observed,
            ObservedWorkerExit::BeforeCommitFailed(_)
        ));
        assert!(!fixture.root.join("backups/actual").exists());
        assert_eq!(
            std::fs::read_dir(fixture.root.join("backups"))
                .unwrap()
                .count(),
            0
        );
        assert!(
            matches!(finish_observed(&fixture.admin, &fixture.store, &claimed, &observed).await,
            Err(Error::Conflict(reason)) if reason == "management job lease fence changed")
        );
        let actual = load_job_record(&fixture.admin, &fixture.store, &claimed.id)
            .await
            .unwrap();
        assert_eq!(actual.generation(), current.generation);
        assert!(actual.result().is_none());
        fixture.store.close().await;
    }

    #[tokio::test]
    async fn backup_long_prepare_renews_or_blocks_without_publish() {
        let (fixture, claimed, control, handle, resume) = paused(TestPoint::Preparing, false).await;
        let mut session = fixture.store.session().await.unwrap();
        let record = load_job_record_in_session(&fixture.admin, &mut session, &claimed.id)
            .await
            .unwrap();
        let mut current = record.into_unfinished_v2().unwrap();
        current.lease_until = now().unwrap() - 1;
        session
            .put(
                &fixture.admin,
                "job",
                &current.id,
                fixture.admin.actor(),
                &current,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        control.stop();
        resume.notify_one();
        let observed = handle.await.unwrap().unwrap();
        assert!(matches!(
            observed,
            ObservedWorkerExit::BeforeCommitFailed(_)
        ));
        let terminal = finish_observed(&fixture.admin, &fixture.store, &claimed, &observed)
            .await
            .unwrap();
        assert_eq!(terminal, ManagementJobState::Failed);
        assert!(!fixture.root.join("backups/actual").exists());
        assert_eq!(
            std::fs::read_dir(fixture.root.join("backups"))
                .unwrap()
                .count(),
            0
        );
        let view = fixture
            .dispatcher
            .status_view(&fixture.admin, &claimed.id)
            .await
            .unwrap();
        assert!(matches!(
            view.fee_knowledge,
            ManagementFeeKnowledge::KnownNone
        ));
        assert!(view.pending_fees.unwrap().is_empty());
        fixture.store.close().await;
    }

    #[tokio::test]
    async fn backup_crash_before_publish_never_adopts_unowned_destination() {
        let (fixture, claimed, _control, handle, resume) =
            paused(TestPoint::BeforePublication, true).await;
        let destination = fixture.root.join("backups/actual");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&destination)
            .unwrap();
        std::fs::write(destination.join("unowned"), b"unowned output").unwrap();
        resume.notify_one();
        assert!(matches!(handle.await.unwrap(), Err(Error::Internal)));
        let state = finish_observed(
            &fixture.admin,
            &fixture.store,
            &claimed,
            &ObservedWorkerExit::Restarted,
        )
        .await
        .unwrap();
        assert_eq!(state, ManagementJobState::Blocked);
        let actual = load_job_record(&fixture.admin, &fixture.store, &claimed.id)
            .await
            .unwrap();
        assert_eq!(actual.error_code(), Some(UNVERIFIED));
        assert!(actual.result().is_none());
        assert_eq!(
            std::fs::read(destination.join("unowned")).unwrap(),
            b"unowned output"
        );
        assert_eq!(
            std::fs::read_dir(fixture.root.join("backups"))
                .unwrap()
                .count(),
            2
        );
        fixture.store.close().await;
    }

    #[tokio::test]
    async fn backup_crash_after_publish_reconciles_only_exact_owned_snapshot() {
        let (fixture, claimed, _control, handle, resume) =
            paused(TestPoint::AfterPublication, true).await;
        let path = fixture.root.join("backups/actual");
        let raw = std::fs::read(path.join("backup-manifest.json")).unwrap();
        resume.notify_one();
        assert!(matches!(handle.await.unwrap(), Err(Error::Internal)));
        let state = finish_observed(
            &fixture.admin,
            &fixture.store,
            &claimed,
            &ObservedWorkerExit::Restarted,
        )
        .await
        .unwrap();
        assert_eq!(state, ManagementJobState::Blocked);
        let actual = load_job_record(&fixture.admin, &fixture.store, &claimed.id)
            .await
            .unwrap();
        assert!(
            matches!(actual.result(), Some(ManagementResultV2::Backup(ManagementBackupResultV2::BackupCreated {
            relative_name, manifest_sha256, durability: BackupDurability::Unverified,
        })) if relative_name.as_str() == "actual" && manifest_sha256 == hash(&raw))
        );
        assert_eq!(actual.error_code(), Some(UNVERIFIED));
        assert_eq!(
            std::fs::read(path.join("backup-manifest.json")).unwrap(),
            raw
        );
        assert_eq!(
            std::fs::read_dir(fixture.root.join("backups"))
                .unwrap()
                .count(),
            1
        );
        let restarted =
            ManagementDispatcher::new(fixture.store.clone(), vec![fixture.admin.clone()]).unwrap();
        assert_eq!(restarted.recover_pending().await.unwrap(), 0);
        fixture.store.close().await;
    }

    #[tokio::test]
    async fn backup_unknown_publication_proof_is_blocked_and_retained() {
        let (fixture, claimed, _control, handle, resume) =
            paused(TestPoint::AfterPublication, true).await;
        let path = fixture.root.join("backups/actual");
        let raw = std::fs::read(path.join("backup-manifest.json")).unwrap();
        resume.notify_one();
        assert!(matches!(handle.await.unwrap(), Err(Error::Internal)));
        std::fs::write(path.join("rsia.sqlite3"), b"broken member proof").unwrap();
        let state = finish_observed(
            &fixture.admin,
            &fixture.store,
            &claimed,
            &ObservedWorkerExit::Restarted,
        )
        .await
        .unwrap();
        assert_eq!(state, ManagementJobState::Blocked);
        let actual = load_job_record(&fixture.admin, &fixture.store, &claimed.id)
            .await
            .unwrap();
        assert_eq!(actual.error_code(), Some(UNVERIFIED));
        assert!(
            matches!(actual.result(), Some(ManagementResultV2::Backup(ManagementBackupResultV2::BackupCreated {
            manifest_sha256, durability: BackupDurability::Unverified, ..
        })) if manifest_sha256 == hash(&raw))
        );
        assert_eq!(
            std::fs::read(path.join("rsia.sqlite3")).unwrap(),
            b"broken member proof"
        );
        assert_eq!(
            std::fs::read(path.join("backup-manifest.json")).unwrap(),
            raw
        );
        assert!(matches!(
            fixture
                .dispatcher
                .status_view(&fixture.admin, &claimed.id)
                .await
                .unwrap()
                .fee_knowledge,
            ManagementFeeKnowledge::KnownNone
        ));
        fixture.store.close().await;
    }
}
