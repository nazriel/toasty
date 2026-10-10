use crate::prelude::*;

#[driver_test(scenario(crate::scenarios::two_models))]
pub async fn not_found(t: &mut Test) -> Result<()> {
    let mut db = setup(t).await;

    let err = User::all().get(&mut db).await.unwrap_err();
    assert!(
        err.is_record_not_found() | err.is_unsupported_feature(),
        "expected RecordNotFound or UnsupportedFeature, got: {err}"
    );

    // Create and immediately delete to get a known-missing ID
    let user = toasty::create!(User { name: "hello" })
        .exec(&mut db)
        .await?;
    let gone_id = user.id;
    user.delete().exec(&mut db).await?;

    let err = User::filter_by_id(gone_id).get(&mut db).await.unwrap_err();
    assert!(
        err.is_record_not_found(),
        "expected RecordNotFound, got: {err}"
    );

    let err = User::get_by_id(&mut db, gone_id).await.unwrap_err();
    assert!(
        err.is_record_not_found(),
        "expected RecordNotFound, got: {err}"
    );
    Ok(())
}

/// The unversioned twin of `field_version::update_after_delete_fails`: an
/// instance update whose row has been deleted fails with `record_not_found`
/// and leaves the instance as it was.
///
/// SQL only: with no condition to check, DynamoDB's `UpdateItem` writes the
/// key back instead of failing.
#[driver_test(requires(sql), scenario(crate::scenarios::two_models))]
pub async fn update_after_delete_fails(t: &mut Test) -> Result<()> {
    let mut db = setup(t).await;

    let mut user = toasty::create!(User { name: "hello" })
        .exec(&mut db)
        .await?;
    let id = user.id;

    // Delete the row out from under the instance.
    User::filter_by_id(id).delete().exec(&mut db).await?;

    let result: Result<()> = user.update().name("stale").exec(&mut db).await;
    let err = assert_err!(result);
    assert!(
        err.is_record_not_found(),
        "expected record_not_found, got {err:?}"
    );

    assert_eq!(user.id, id);
    assert_eq!(user.name, "hello");
    Ok(())
}

/// `update_after_delete_fails` for an update that sets more than one field.
#[driver_test(requires(sql), scenario(crate::scenarios::user_name_email))]
pub async fn update_many_fields_after_delete_fails(t: &mut Test) -> Result<()> {
    let mut db = setup(t).await;

    let mut user = toasty::create!(User {
        name: "hello",
        email: "hello@example.com",
    })
    .exec(&mut db)
    .await?;

    User::filter_by_id(user.id).delete().exec(&mut db).await?;

    let result: Result<()> = user
        .update()
        .name("stale")
        .email("stale@example.com")
        .exec(&mut db)
        .await;
    let err = assert_err!(result);
    assert!(
        err.is_record_not_found(),
        "expected record_not_found, got {err:?}"
    );

    assert_eq!(user.name, "hello");
    assert_eq!(user.email, "hello@example.com");
    Ok(())
}

/// The unversioned twin of `field_version::relative_update_after_delete_fails`:
/// a relative update (`increment`) reads its result back from the row; with
/// the row deleted there is nothing to read, and the update fails rather than
/// reloading the instance from a phantom row.
#[driver_test(requires(sql), scenario(crate::scenarios::counter_value))]
pub async fn relative_update_after_delete_fails(t: &mut Test) -> Result<()> {
    let mut db = setup(t).await;

    let mut counter = toasty::create!(Counter {
        id: uuid::Uuid::new_v4(),
        value: 10
    })
    .exec(&mut db)
    .await?;

    Counter::filter_by_id(counter.id)
        .delete()
        .exec(&mut db)
        .await?;

    let result: Result<()> = counter
        .update()
        .value(toasty::stmt::increment())
        .exec(&mut db)
        .await;
    let err = assert_err!(result);
    assert!(
        err.is_record_not_found(),
        "expected record_not_found, got {err:?}"
    );

    assert_eq!(counter.value, 10);
    Ok(())
}
