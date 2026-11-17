use super::*;
use gitadel_db::{
    entity::{email_token, user_totp},
    test_support::{database, owner},
};
use gitadel_secrets::{SecretCipher, SecretKind};

#[tokio::test]
async fn bootstrap_creates_private_admin_namespace_and_rejects_a_second_bootstrap() {
    let db = database().await;
    let admin = bootstrap_admin(&db, " Alice ", "a secure password".into())
        .await
        .unwrap();
    assert_eq!(admin.username, "alice");
    assert!(admin.is_admin);
    assert_eq!(admin.default_repository_visibility, "private");
    assert!(
        verify_password("a secure password".into(), admin.password_hash)
            .await
            .unwrap()
    );
    assert_eq!(
        namespace::Entity::find_by_id("alice")
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .user_id,
        Some(admin.id)
    );
    assert_eq!(audit_event::Entity::find().count(&db).await.unwrap(), 1);
    assert!(matches!(
        bootstrap_admin(&db, "bob", "another password".into()).await,
        Err(IdentityError::Conflict(_))
    ));
}

#[tokio::test]
async fn sessions_reject_expiry_revocation_and_disabled_accounts() {
    let db = database().await;
    let account = owner(&db, "alice").await;
    let service = IdentityService::new(&db);
    let token = create_session_on(&db, account.id, ChronoDuration::hours(1))
        .await
        .unwrap();
    assert_eq!(
        service.authenticate_session(&token).await.unwrap().user.id,
        account.id
    );
    let expired = create_session_on(&db, account.id, ChronoDuration::seconds(-1))
        .await
        .unwrap();
    assert!(service.session_user(&expired).await.unwrap().is_none());
    assert!(matches!(
        service.authenticate_session(&expired).await,
        Err(IdentityError::Unauthorized)
    ));
    delete_session(&db, &token).await.unwrap();
    assert!(service.session_user(&token).await.unwrap().is_none());
    let token = create_session_on(&db, account.id, ChronoDuration::hours(1))
        .await
        .unwrap();
    let mut disabled: user::ActiveModel = account.into();
    disabled.disabled_at = Set(Some(Utc::now()));
    disabled.update(&db).await.unwrap();
    assert!(service.session_user(&token).await.unwrap().is_none());
    assert!(matches!(
        service.authenticate_session(&token).await,
        Err(IdentityError::Unauthorized)
    ));
}

#[tokio::test]
async fn api_tokens_translate_repository_read_scope_but_reject_other_scopes_and_revocation() {
    let db = database().await;
    let account = owner(&db, "alice").await;
    let token = api_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(account.id),
        name: Set("test".into()),
        token_hash: Set(hash_secret("api-token")),
        scopes: Set(SCOPE_READ),
        expires_at: Set(None),
        created_at: Set(Utc::now()),
        last_used_at: Set(None),
        revoked_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    let service = IdentityService::new(&db);
    assert!(
        service
            .authenticate_token("api-token", SCOPE_REPOSITORY_READ)
            .await
            .unwrap()
            .via_api_token
    );
    assert!(
        api_token::Entity::find_by_id(token.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .last_used_at
            .is_some()
    );
    assert!(matches!(
        service.authenticate_token("api-token", SCOPE_WRITE).await,
        Err(IdentityError::Forbidden(_))
    ));
    let mut revoked: api_token::ActiveModel = token.into();
    revoked.revoked_at = Set(Some(Utc::now()));
    revoked.update(&db).await.unwrap();
    assert!(matches!(
        service.authenticate_token("api-token", SCOPE_READ).await,
        Err(IdentityError::Unauthorized)
    ));
}

#[tokio::test]
async fn password_replacement_preserves_current_session_and_obeys_transaction_rollback() {
    let db = database().await;
    let account = owner(&db, "alice").await;
    let current = create_session_on(&db, account.id, ChronoDuration::hours(1))
        .await
        .unwrap();
    let other = create_session_on(&db, account.id, ChronoDuration::hours(1))
        .await
        .unwrap();
    let transaction = db.begin().await.unwrap();
    replace_password_on(
        &transaction,
        &account,
        "replacement".into(),
        &hash_secret(&current),
    )
    .await
    .unwrap();
    transaction.rollback().await.unwrap();
    assert!(
        IdentityService::new(&db)
            .session_user(&other)
            .await
            .unwrap()
            .is_some()
    );
    let transaction = db.begin().await.unwrap();
    replace_password_on(
        &transaction,
        &account,
        "replacement".into(),
        &hash_secret(&current),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert!(
        IdentityService::new(&db)
            .session_user(&current)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        IdentityService::new(&db)
            .session_user(&other)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        user::Entity::find_by_id(account.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "replacement"
    );
}

#[tokio::test]
async fn email_tokens_are_bound_to_purpose_expire_and_can_only_be_consumed_once() {
    let db = database().await;
    let account = owner(&db, "alice").await;
    let token = email::issue_token(
        &db,
        account.id,
        "verify",
        Some("alice@example.com"),
        ChronoDuration::hours(1),
    )
    .await
    .unwrap();
    let hash = hash_secret(&token);
    assert!(
        email::consume_token(&db, &hash, "reset")
            .await
            .unwrap()
            .is_none()
    );
    let consumed = email::consume_token(&db, &hash, "verify")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed.email.as_deref(), Some("alice@example.com"));
    assert!(
        email::consume_token(&db, &hash, "verify")
            .await
            .unwrap()
            .is_none()
    );
    let expired = email::issue_token(&db, account.id, "verify", None, ChronoDuration::seconds(-1))
        .await
        .unwrap();
    assert!(
        email::consume_token(&db, &hash_secret(&expired), "verify")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        email_token::Entity::find_by_id(&hash)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .used_at
            .is_some()
    );
}

#[tokio::test]
async fn confirmed_totp_and_recovery_codes_are_consumed_once_with_sealed_secrets() {
    let db = database().await;
    let account = owner(&db, "alice").await;
    let cipher = SecretCipher::from_material(b"identity test cipher").unwrap();
    let secret = b"12345678901234567890";
    let sealed = cipher
        .seal_text(
            SecretKind::TotpSecret,
            &account.id,
            &URL_SAFE_NO_PAD.encode(secret),
        )
        .unwrap();
    user_totp::ActiveModel {
        user_id: Set(account.id),
        secret: Set(sealed),
        confirmed_at: Set(Some(Utc::now())),
        last_used_step: Set(None),
        created_at: Set(Utc::now()),
    }
    .insert(&db)
    .await
    .unwrap();
    let code = format!(
        "{:06}",
        two_factor::hotp(secret, (Utc::now().timestamp() / 30) as u64).unwrap()
    );
    assert_eq!(
        two_factor::verify_second_factor(&db, &cipher, account.id, &code)
            .await
            .unwrap(),
        Some(two_factor::SecondFactor::Totp)
    );
    let wrong_cipher = SecretCipher::from_material(b"different instance key").unwrap();
    assert!(matches!(
        two_factor::verify_second_factor(&db, &wrong_cipher, account.id, &code).await,
        Err(IdentityError::SecretOpen(_))
    ));
    assert_eq!(
        two_factor::verify_second_factor(&db, &cipher, account.id, &code)
            .await
            .unwrap(),
        None
    );
    let codes = two_factor::replace_recovery_codes(&db, account.id)
        .await
        .unwrap();
    assert_eq!(codes.len(), 10);
    assert_eq!(
        two_factor::verify_second_factor(&db, &cipher, account.id, &codes[0].to_uppercase())
            .await
            .unwrap(),
        Some(two_factor::SecondFactor::RecoveryCode)
    );
    assert_eq!(
        two_factor::verify_second_factor(&db, &cipher, account.id, &codes[0])
            .await
            .unwrap(),
        None
    );
    assert!(two_factor::remove_on(&db, account.id).await.unwrap());
    assert!(!two_factor::is_enabled(&db, account.id).await.unwrap());
}

#[test]
fn totp_preserves_rfc_vectors_drift_and_replay_rules() {
    let secret = b"12345678901234567890";
    for (time, expected) in [
        (59, "287082"),
        (1_111_111_109, "081804"),
        (1_234_567_890, "005924"),
        (2_000_000_000, "279037"),
    ] {
        assert_eq!(
            format!(
                "{:06}",
                two_factor::hotp(secret, (time / 30) as u64).unwrap()
            ),
            expected
        );
    }
    let now = 1_111_111_109;
    let previous = format!(
        "{:06}",
        two_factor::hotp(secret, (now / 30 - 1) as u64).unwrap()
    );
    let step = two_factor::verify_totp(secret, &previous, now, None).unwrap();
    assert_eq!(step, Some(now / 30 - 1));
    assert_eq!(
        two_factor::verify_totp(secret, &previous, now, step).unwrap(),
        None
    );
    assert_eq!(
        two_factor::verify_totp(secret, "abcdef", now, None).unwrap(),
        None
    );
}

#[tokio::test]
async fn namespace_credentials_remain_private_to_the_owner() {
    let db = database().await;
    let alice = owner(&db, "alice").await;
    let bob = owner(&db, "bob").await;
    let actor = AuthenticatedUser {
        user: alice,
        via_api_token: false,
    };
    permissions::authorize_namespace(&db, &actor, "alice")
        .await
        .unwrap();
    assert!(matches!(
        permissions::authorize_namespace(&db, &actor, "bob").await,
        Err(IdentityError::NotFound)
    ));
    let actor = AuthenticatedUser {
        user: user::Model {
            is_admin: true,
            ..bob
        },
        via_api_token: false,
    };
    assert!(matches!(
        permissions::authorize_namespace(&db, &actor, "alice").await,
        Err(IdentityError::NotFound)
    ));
}

#[tokio::test]
async fn oauth_tokens_keep_distinct_scopes_and_reject_expiry() {
    use gitadel_db::entity::oauth_application;
    let db = database().await;
    let account = owner(&db, "alice").await;
    let app = oauth_application::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(account.id),
        name: Set("test".into()),
        client_id: Set("test-client".into()),
        client_secret_hash: Set(hash_secret("test-secret")),
        redirect_uri: Set("https://example.com/callback".into()),
        created_at: Set(Utc::now()),
    }
    .insert(&db)
    .await
    .unwrap();
    let token = oauth_access_token::ActiveModel {
        token_hash: Set(hash_secret("oauth-token")),
        application_id: Set(app.id),
        user_id: Set(account.id),
        scopes: Set(SCOPE_REPOSITORY_READ),
        scope: Set("read:repository".into()),
        created_at: Set(Utc::now()),
        expires_at: Set(Utc::now() + ChronoDuration::hours(1)),
        last_used_at: Set(None),
        revoked_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    let service = IdentityService::new(&db);
    assert!(
        service
            .authenticate_token("oauth-token", SCOPE_REPOSITORY_READ)
            .await
            .unwrap()
            .via_api_token
    );
    assert!(matches!(
        service.authenticate_token("oauth-token", SCOPE_READ).await,
        Err(IdentityError::Forbidden(_))
    ));
    let mut expired: oauth_access_token::ActiveModel = token.into();
    expired.expires_at = Set(Utc::now() - ChronoDuration::seconds(1));
    expired.update(&db).await.unwrap();
    assert!(matches!(
        service
            .authenticate_token("oauth-token", SCOPE_REPOSITORY_READ)
            .await,
        Err(IdentityError::Unauthorized)
    ));
}
