//! AG-166c-reset recovery controls for `seed.reset_stage` (plan v4.5
//! §6.1.2 L828, §11.1 L1468, capacity L381/L387, E16.3).
//!
//! The reset kernel shares one staging tail for the public API and the
//! management handler. This file pins the capacity gate of that tail and the
//! job-recovery behaviour of the handler: staging and the terminal job state
//! in one transaction, cancellation before and after the effect, and the way
//! out of Draining.
//!
//! Two-tree probe. It uses only API that predates the wiring, so it compiles
//! on the parent tree, where the public reset has no capacity admission and
//! the probe fails with `PARENT-EXPECTED-FAILURE`:
//! - CPP-C (refusal with zero writes): at 20/20 staged packages the 21st
//!   public reset is refused with the typed staging conflict, while a replay
//!   of an existing key still returns its original image.
#[path = "support/seed_reset.rs"]
mod seed_reset;

use evo_core::Error;
use evo_engine::seeds::PersistentSeedStore;
use seed_reset::{COMPILER_VERSION, Fixture, staged_id};

#[tokio::test]
async fn cpp_c_public_seed_reset_refuses_the_21st_staging_package() {
    let fixture = Fixture::new().await;
    let environment = fixture.run_environment_digest.clone();
    // Twenty explicit safe-B stagings fill the instance-wide 20-package
    // limit; the install itself stages nothing.
    for index in 0..20 {
        PersistentSeedStore::stage_reset_to_baseline(
            &fixture.admin,
            &fixture.store,
            &fixture.install_id,
            &format!("fill-{index}"),
            &environment,
            COMPILER_VERSION,
        )
        .await
        .unwrap();
    }
    assert_eq!(fixture.staged_packages().await, 20);
    let before = fixture.business_shape().await;

    // At the limit, the replay of an existing key is not a new derivation: it
    // returns the original image and writes nothing.
    let replayed = PersistentSeedStore::stage_reset_to_baseline(
        &fixture.admin,
        &fixture.store,
        &fixture.install_id,
        "fill-0",
        &environment,
        COMPILER_VERSION,
    )
    .await
    .unwrap();
    assert_eq!(replayed.id, staged_id("fill-0"));
    assert_eq!(fixture.business_shape().await, before);

    // The 21st is a new staging package: refused with the typed conflict
    // that names the item, the used count and the limit.
    match PersistentSeedStore::stage_reset_to_baseline(
        &fixture.admin,
        &fixture.store,
        &fixture.install_id,
        "fill-20",
        &environment,
        COMPILER_VERSION,
    )
    .await
    {
        Err(Error::Capacity(detail)) => {
            assert_eq!(
                detail.item,
                evo_core::capacity::CapacityItem::StagingPackage
            );
            assert_eq!((detail.used, detail.limit, detail.delta), (20, 20, 1));
        }
        Err(other) => panic!("unexpected refusal of the 21st staging: {other:?}"),
        Ok(staged) => panic!(
            "PARENT-EXPECTED-FAILURE: the public seed reset admitted a 21st staging package ({})",
            staged.id
        ),
    }
    assert_eq!(fixture.staged_packages().await, 20);
    assert_eq!(
        fixture.business_shape().await,
        before,
        "the refused 21st staging wrote nothing"
    );
    fixture.store.close().await;
}
