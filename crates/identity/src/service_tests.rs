use std::sync::Arc;

use chrono::{Duration, Utc};
use gitadel_core::audit::AuditContext;
use gitadel_db::{
    entity::{invitation, organization_member},
    test_support::{database, owner},
};
use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};

use crate::{
    AccountService, CredentialSource, IdentityError, IdentityService, Principal, SCOPE_READ,
    SCOPE_REPOSITORY_READ, SCOPE_WRITE, auth, hash_secret, resources, runtime::AuthRuntime,
};

async fn browser(database: &sea_orm::DatabaseConnection, id: uuid::Uuid) -> Principal {
    let raw = crate::create_session_on(database, id, Duration::hours(1))
        .await
        .unwrap();
    IdentityService::new(database)
        .authenticate_session_principal(&raw)
        .await
        .unwrap()
}

fn runtime(database: sea_orm::DatabaseConnection) -> AuthRuntime {
    AuthRuntime::new(
        database,
        gitadel_config::AuthSettings {
            session_lifetime_hours: 24,
            invitation_lifetime_hours: 24,
        },
        "http://localhost:3000".parse().unwrap(),
        gitadel_mail::Mailer::disabled(),
        Arc::new(gitadel_secrets::SecretCipher::from_material(b"service test").unwrap()),
    )
    .unwrap()
}

#[tokio::test]
async fn direct_mutations_recheck_scope_source_and_revocation() {
    let database = database().await;
    let user = owner(&database, "alice").await;
    let actor = browser(&database, user.id).await;
    let service = AccountService::new(&database, AuditContext::default());
    let created = resources::create_token(
        &service,
        &actor,
        resources::CreateTokenRequest {
            name: "reader".into(),
            scopes: vec!["read".into()],
            expires_in_days: None,
        },
    )
    .await
    .unwrap();
    let principal = IdentityService::new(&database)
        .authenticate_token_principal(&created.token, SCOPE_REPOSITORY_READ)
        .await
        .unwrap();
    assert_eq!(principal.source(), CredentialSource::PersonalToken);
    assert_eq!(principal.granted_scopes(), SCOPE_READ);
    principal
        .authorize(&database, SCOPE_REPOSITORY_READ)
        .await
        .unwrap();
    assert!(matches!(
        resources::create_organization(
            &service,
            &principal,
            resources::CreateOrganizationRequest {
                slug: "team".into(),
                display_name: "Team".into()
            }
        )
        .await,
        Err(IdentityError::Forbidden(_))
    ));
    assert!(matches!(
        resources::list_tokens(&service, &principal).await,
        Err(IdentityError::Forbidden(_))
    ));
    resources::revoke_token(&service, &actor, created.details.id)
        .await
        .unwrap();
    assert!(matches!(
        resources::list_organizations(&service, &principal).await,
        Err(IdentityError::Unauthorized)
    ));
}

#[tokio::test]
async fn direct_admin_operation_does_not_trust_a_stale_or_forged_admin_snapshot() {
    let database = database().await;
    let user = owner(&database, "alice").await;
    let mut actor = browser(&database, user.id).await;
    actor.actor.user.is_admin = true;
    let service = AccountService::new(&database, AuditContext::default());
    assert!(matches!(
        crate::users::list_invitations(&service, &actor).await,
        Err(IdentityError::Forbidden(_))
    ));
}

#[tokio::test]
async fn removing_the_last_owner_rolls_back_the_membership_and_audit() {
    let database = database().await;
    let user = owner(&database, "alice").await;
    let actor = browser(&database, user.id).await;
    let service = AccountService::new(&database, AuditContext::default());
    let organization = resources::create_organization(
        &service,
        &actor,
        resources::CreateOrganizationRequest {
            slug: "team".into(),
            display_name: "Team".into(),
        },
    )
    .await
    .unwrap();
    let before = gitadel_db::entity::audit_event::Entity::find()
        .count(&database)
        .await
        .unwrap();
    assert!(matches!(
        resources::remove_member(&service, &actor, "team".into(), "alice".into()).await,
        Err(IdentityError::Conflict(_))
    ));
    assert!(
        organization_member::Entity::find_by_id((organization.id, user.id))
            .one(&database)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        gitadel_db::entity::audit_event::Entity::find()
            .count(&database)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn failed_registration_releases_the_invitation_claim() {
    let database = database().await;
    let existing = owner(&database, "alice").await;
    let hash = hash_secret("invitation");
    invitation::ActiveModel {
        token_hash: Set(hash.clone()),
        created_by: Set(existing.id),
        expires_at: Set(Utc::now() + Duration::hours(1)),
        used_at: Set(None),
        created_at: Set(Utc::now()),
    }
    .insert(&database)
    .await
    .unwrap();
    let runtime = runtime(database.clone());
    let service = runtime.service(AuditContext::default());
    assert!(matches!(
        auth::register(
            &service,
            auth::InvitationRegistrationRequest {
                token: "invitation".into(),
                username: "alice".into(),
                password: "long enough password".into()
            }
        )
        .await,
        Err(IdentityError::Conflict(_))
    ));
    assert!(
        invitation::Entity::find_by_id(hash)
            .one(&database)
            .await
            .unwrap()
            .unwrap()
            .used_at
            .is_none()
    );
}

#[tokio::test]
async fn oauth_repository_scope_cannot_authorize_account_mutations() {
    let database = database().await;
    let user = owner(&database, "alice").await;
    let application_id = uuid::Uuid::new_v4();
    gitadel_db::entity::oauth_application::ActiveModel {
        id: Set(application_id),
        user_id: Set(user.id),
        name: Set("app".into()),
        client_id: Set("client".into()),
        client_secret_hash: Set(hash_secret("secret")),
        redirect_uri: Set("http://localhost/callback".into()),
        created_at: Set(Utc::now()),
    }
    .insert(&database)
    .await
    .unwrap();
    let pair = crate::oauth::TokenPair::generate();
    pair.store(&database, application_id, user.id, "read:repository")
        .await
        .unwrap();
    let principal = IdentityService::new(&database)
        .authenticate_token_principal(&pair.access_token, SCOPE_REPOSITORY_READ)
        .await
        .unwrap();
    assert_eq!(principal.source(), CredentialSource::OAuthToken);
    assert!(matches!(
        principal.authorize(&database, SCOPE_READ).await,
        Err(IdentityError::Forbidden(_))
    ));
    assert!(matches!(
        AccountService::new(&database, AuditContext::default())
            .require_actor(&principal, false, SCOPE_WRITE)
            .await,
        Err(IdentityError::Forbidden(_))
    ));
}

#[tokio::test]
async fn invalid_redirect_consumes_an_authorization_code_even_without_http() {
    let database = database().await;
    let user = owner(&database, "alice").await;
    let application_id = uuid::Uuid::new_v4();
    gitadel_db::entity::oauth_application::ActiveModel {
        id: Set(application_id),
        user_id: Set(user.id),
        name: Set("app".into()),
        client_id: Set("client".into()),
        client_secret_hash: Set(hash_secret("secret")),
        redirect_uri: Set("http://localhost/callback".into()),
        created_at: Set(Utc::now()),
    }
    .insert(&database)
    .await
    .unwrap();
    gitadel_db::entity::oauth_authorization_code::ActiveModel {
        code_hash: Set(hash_secret("code")),
        application_id: Set(application_id),
        user_id: Set(user.id),
        redirect_uri: Set("http://localhost/callback".into()),
        scope: Set("read:repository".into()),
        expires_at: Set(Utc::now() + Duration::minutes(1)),
        created_at: Set(Utc::now()),
    }
    .insert(&database)
    .await
    .unwrap();
    let runtime = runtime(database.clone());
    let outcome = crate::oauth::access_token(
        &runtime.service(AuditContext::default()),
        crate::oauth::TokenRequest {
            grant_type: "authorization_code".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            code: Some("code".into()),
            redirect_uri: Some("http://localhost/wrong".into()),
            refresh_token: None,
        },
    )
    .await;
    assert!(
        matches!(outcome,crate::oauth::OAuthOutcome::Rejected{ref error,..} if error=="invalid_grant")
    );
    assert!(
        gitadel_db::entity::oauth_authorization_code::Entity::find_by_id(hash_secret("code"))
            .one(&database)
            .await
            .unwrap()
            .is_none()
    );
}
