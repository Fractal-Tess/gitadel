//! Secrets at rest.
//!
//! Stored credentials (Actions secrets, Dokploy API keys, mirror identity
//! tokens, webhook secrets, OIDC client secrets, TOTP seeds) are sealed with
//! AES-256-GCM under a key derived from the SSH host key, which is already a
//! persistent, private, backed-up instance secret. Every ciphertext is bound
//! to the kind of secret and the row it belongs to through the associated
//! data, so a value cannot be moved to another row and opened there.
//!
//! Text columns hold `v1:` followed by the URL-safe base64 of nonce and
//! ciphertext. Anything without that prefix is a value written before
//! sealing existed and is read as plain text; [`seal_stored_secrets`] rewrites
//! such rows once at startup. Replacing the host key makes sealed values
//! unreadable: they must then be entered again, and reads fail with an error
//! that names the setting instead of a panic.

use std::fmt;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use russh::keys::{PrivateKey, ssh_encoding::Encode};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
};
use sha2::Sha256;

use crate::entity::{
    namespace_integration, namespace_mirror_identity, oidc_provider, repository_webhook, user_totp,
};

/// Kept verbatim from the Actions sealer so existing Actions secrets open
/// without re-encryption.
const KEY_CONTEXT: &[u8] = b"gitadel actions secrets v1";
const NONCE_BYTES: usize = 12;
const TEXT_PREFIX: &str = "v1:";

/// Seals and opens stored secret values.
#[derive(Clone)]
pub(crate) struct SecretCipher {
    cipher: Aes256Gcm,
}

/// What a text-column secret is, which fixes the associated data it is
/// sealed with and the name an unreadable value is reported under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SecretKind {
    DokployApiKey,
    MirrorIdentitySecret,
    WebhookSecret,
    OidcClientSecret,
    TotpSecret,
}

impl SecretKind {
    fn label(self) -> &'static str {
        match self {
            Self::DokployApiKey => "dokploy-api-key",
            Self::MirrorIdentitySecret => "mirror-identity-secret",
            Self::WebhookSecret => "webhook-secret",
            Self::OidcClientSecret => "oidc-client-secret",
            Self::TotpSecret => "totp-secret",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::DokployApiKey => "Dokploy API key of integration",
            Self::MirrorIdentitySecret => "access token of mirror identity",
            Self::WebhookSecret => "secret of webhook",
            Self::OidcClientSecret => "client secret of identity provider",
            Self::TotpSecret => "authenticator secret of user",
        }
    }
}

/// A stored value that cannot be opened, almost always because the SSH host
/// key changed since it was sealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenError {
    kind: SecretKind,
    id: String,
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "The {} {} cannot be read because it was sealed under a different SSH host key. Enter it again.",
            self.kind.description(),
            self.id
        )
    }
}

impl std::error::Error for OpenError {}

/// Sealing cannot fail for well-formed input; this only reports an AEAD
/// failure, which would mean a broken cipher implementation.
#[derive(Debug)]
pub(crate) struct SealError;

impl fmt::Display for SealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("could not seal the secret")
    }
}

impl std::error::Error for SealError {}

impl SecretCipher {
    /// Derives the sealing key from the SSH host key's private key material.
    pub(crate) fn from_host_key(key: &PrivateKey) -> anyhow::Result<Self> {
        let material = key
            .key_data()
            .encode_vec()
            .map_err(|error| anyhow::anyhow!("could not encode the SSH host key: {error}"))?;
        Self::from_material(&material)
    }

    pub(crate) fn from_material(material: &[u8]) -> anyhow::Result<Self> {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(KEY_CONTEXT)
            .map_err(|_| anyhow::anyhow!("invalid secret key context"))?;
        mac.update(material);
        let key = mac.finalize().into_bytes();
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("invalid derived secret key length"))?;
        Ok(Self { cipher })
    }

    /// A cipher for tests, where no host key exists.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::from_material(b"gitadel test secret cipher").expect("test cipher")
    }

    /// Seals `value` bound to `associated_data`; the result is the random
    /// nonce followed by the ciphertext and tag.
    pub(crate) fn seal_bytes(
        &self,
        associated_data: &[u8],
        value: &[u8],
    ) -> Result<Vec<u8>, SealError> {
        let nonce_bytes: [u8; NONCE_BYTES] = rand::random();
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value,
                    aad: associated_data,
                },
            )
            .map_err(|_| SealError)?;
        let mut sealed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    /// Opens a value from [`Self::seal_bytes`]; `None` when the key, the
    /// associated data, or the ciphertext does not match.
    pub(crate) fn open_bytes(&self, associated_data: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let (nonce, ciphertext) = sealed.split_at_checked(NONCE_BYTES)?;
        let nonce = Nonce::try_from(nonce).ok()?;
        self.cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: associated_data,
                },
            )
            .ok()
    }

    fn text_associated_data(kind: SecretKind, id: &dyn fmt::Display) -> Vec<u8> {
        format!("gitadel-secret:v1:{}:{id}", kind.label()).into_bytes()
    }

    /// Seals a text-column secret for the row `id` of `kind`.
    pub(crate) fn seal_text(
        &self,
        kind: SecretKind,
        id: &dyn fmt::Display,
        value: &str,
    ) -> Result<String, SealError> {
        let sealed = self.seal_bytes(&Self::text_associated_data(kind, id), value.as_bytes())?;
        Ok(format!("{TEXT_PREFIX}{}", URL_SAFE_NO_PAD.encode(sealed)))
    }

    /// Opens a text-column secret. A value without the sealed prefix predates
    /// sealing and is returned as it is.
    pub(crate) fn open_text(
        &self,
        kind: SecretKind,
        id: &dyn fmt::Display,
        stored: &str,
    ) -> Result<String, OpenError> {
        let Some(encoded) = stored.strip_prefix(TEXT_PREFIX) else {
            return Ok(stored.to_owned());
        };
        let failed = || OpenError {
            kind,
            id: id.to_string(),
        };
        let sealed = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| failed())?;
        let plaintext = self
            .open_bytes(&Self::text_associated_data(kind, id), &sealed)
            .ok_or_else(failed)?;
        String::from_utf8(plaintext).map_err(|_| failed())
    }
}

/// Whether a stored text value is already sealed.
pub(crate) fn is_sealed(stored: &str) -> bool {
    stored.starts_with(TEXT_PREFIX)
}

/// How many rows the startup pass sealed, per kind.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SealReport {
    pub(crate) dokploy_api_keys: u64,
    pub(crate) mirror_identity_secrets: u64,
    pub(crate) webhook_secrets: u64,
    pub(crate) oidc_client_secrets: u64,
    pub(crate) totp_secrets: u64,
}

impl SealReport {
    pub(crate) fn total(&self) -> u64 {
        self.dokploy_api_keys
            + self.mirror_identity_secrets
            + self.webhook_secrets
            + self.oidc_client_secrets
            + self.totp_secrets
    }
}

/// Seals every secret that is still stored as plain text. Safe to run on
/// every start: sealed rows are left alone, and readers accept both forms,
/// so the pass can run while requests are served.
pub(crate) async fn seal_stored_secrets<C: ConnectionTrait>(
    database: &C,
    cipher: &SecretCipher,
) -> Result<SealReport, anyhow::Error> {
    let mut report = SealReport::default();

    for row in namespace_integration::Entity::find().all(database).await? {
        if is_sealed(&row.api_key) || row.api_key.is_empty() {
            continue;
        }
        let sealed = cipher.seal_text(SecretKind::DokployApiKey, &row.id, &row.api_key)?;
        let mut active = row.into_active_model();
        active.api_key = Set(sealed);
        active.update(database).await?;
        report.dokploy_api_keys += 1;
    }

    for row in namespace_mirror_identity::Entity::find()
        .all(database)
        .await?
    {
        if is_sealed(&row.secret) || row.secret.is_empty() {
            continue;
        }
        let sealed = cipher.seal_text(SecretKind::MirrorIdentitySecret, &row.id, &row.secret)?;
        let mut active = row.into_active_model();
        active.secret = Set(sealed);
        active.update(database).await?;
        report.mirror_identity_secrets += 1;
    }

    for row in repository_webhook::Entity::find()
        .filter(repository_webhook::Column::Secret.is_not_null())
        .all(database)
        .await?
    {
        let Some(secret) = row.secret.as_deref() else {
            continue;
        };
        if is_sealed(secret) || secret.is_empty() {
            continue;
        }
        let sealed = cipher.seal_text(SecretKind::WebhookSecret, &row.id, secret)?;
        let mut active = row.into_active_model();
        active.secret = Set(Some(sealed));
        active.update(database).await?;
        report.webhook_secrets += 1;
    }

    for row in oidc_provider::Entity::find().all(database).await? {
        if is_sealed(&row.client_secret) || row.client_secret.is_empty() {
            continue;
        }
        let sealed = cipher.seal_text(SecretKind::OidcClientSecret, &row.id, &row.client_secret)?;
        let mut active = row.into_active_model();
        active.client_secret = Set(sealed);
        active.update(database).await?;
        report.oidc_client_secrets += 1;
    }

    for row in user_totp::Entity::find().all(database).await? {
        if is_sealed(&row.secret) || row.secret.is_empty() {
            continue;
        }
        let sealed = cipher.seal_text(SecretKind::TotpSecret, &row.user_id, &row.secret)?;
        let mut active = row.into_active_model();
        active.secret = Set(sealed);
        active.update(database).await?;
        report.totp_secrets += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_secrets_round_trip_and_bind_to_kind_and_row() {
        let cipher = SecretCipher::for_tests();
        let id = uuid::Uuid::new_v4();
        let sealed = cipher
            .seal_text(SecretKind::WebhookSecret, &id, "hunter2")
            .unwrap();
        assert!(sealed.starts_with("v1:"));
        assert!(!sealed.contains("hunter2"));
        assert!(is_sealed(&sealed));
        assert_eq!(
            cipher
                .open_text(SecretKind::WebhookSecret, &id, &sealed)
                .unwrap(),
            "hunter2"
        );
        assert!(
            cipher
                .open_text(SecretKind::DokployApiKey, &id, &sealed)
                .is_err(),
            "another kind cannot open it"
        );
        assert!(
            cipher
                .open_text(SecretKind::WebhookSecret, &uuid::Uuid::new_v4(), &sealed)
                .is_err(),
            "another row cannot open it"
        );
        let error = SecretCipher::from_material(b"replaced host key")
            .unwrap()
            .open_text(SecretKind::WebhookSecret, &id, &sealed)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "The secret of webhook {id} cannot be read because it was sealed under a different SSH host key. Enter it again."
            )
        );
        assert!(
            cipher
                .open_text(SecretKind::WebhookSecret, &id, "v1:not-base64!")
                .is_err()
        );
    }

    #[test]
    fn plain_text_values_are_read_as_they_are() {
        let cipher = SecretCipher::for_tests();
        assert_eq!(
            cipher
                .open_text(SecretKind::TotpSecret, &"user", "legacy-plain")
                .unwrap(),
            "legacy-plain"
        );
        assert!(!is_sealed("legacy-plain"));
    }

    /// Ciphertexts written by the Actions sealer before it moved here, for
    /// `from_material(b"host key material")`. They must keep opening.
    #[test]
    fn actions_ciphertexts_from_before_the_move_still_open() {
        let cipher = SecretCipher::from_material(b"host key material").unwrap();
        let repository =
            "gitadel-actions-secret:v1:repository:8b1c2f3e-4d5a-4b6c-9d7e-1f2a3b4c5d6e:TOKEN";
        let sealed = hex("3a813eb55b4a67b4d1f04faa6885d9f11f1f7b3ce7c2d0345c5a5c4a02b9eb8a271ad2");
        assert_eq!(
            cipher.open_bytes(repository.as_bytes(), &sealed).as_deref(),
            Some(b"hunter2".as_slice())
        );
        let namespace = "gitadel-actions-secret:v1:namespace:A";
        let sealed = hex("cafa1fbf23e72667594d8afd3960ad72261c2caa509305e47299669914725622");
        assert_eq!(
            cipher.open_bytes(namespace.as_bytes(), &sealed).as_deref(),
            Some(b"same".as_slice())
        );
    }

    fn hex(value: &str) -> Vec<u8> {
        (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn startup_pass_seals_plain_rows_once_and_leaves_the_rest() {
        use chrono::Utc;
        use sea_orm::{ActiveModelTrait, Set};
        use uuid::Uuid;

        let database = crate::actions::test_support::database().await;
        let owner = crate::actions::test_support::owner(&database, "alice").await;
        let project = crate::actions::test_support::repository(&database, &owner, "project").await;
        let cipher = SecretCipher::for_tests();
        let now = Utc::now();
        let integration = namespace_integration::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("alice".to_owned()),
            provider: Set("dokploy".to_owned()),
            name: Set("Dokploy".to_owned()),
            enabled: Set(true),
            url: Set("https://dokploy.example".to_owned()),
            api_key: Set("plain-key".to_owned()),
            dokploy_internal_url: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let identity = namespace_mirror_identity::ActiveModel {
            id: Set(Uuid::new_v4()),
            namespace: Set("alice".to_owned()),
            name: Set("GitHub".to_owned()),
            kind: Set("token".to_owned()),
            username: Set(None),
            secret: Set("ghp_plain".to_owned()),
            provider: Set(Some("github".to_owned())),
            instance_url: Set(Some("https://github.com".to_owned())),
            public_key: Set(None),
            fingerprint: Set(None),
            last_used_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let hook = |secret: Option<&str>| repository_webhook::ActiveModel {
            id: Set(Uuid::new_v4()),
            repository_id: Set(project.id),
            url: Set("https://example.com/hook".to_owned()),
            secret: Set(secret.map(str::to_owned)),
            active: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
            last_delivery_at: Set(None),
            last_response_status: Set(None),
            last_response_message: Set(None),
            events: Set("push".to_owned()),
        };
        let signed = hook(Some("hook-secret")).insert(&database).await.unwrap();
        let unsigned = hook(None).insert(&database).await.unwrap();
        let provider = oidc_provider::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("Example".to_owned()),
            issuer_url: Set("https://id.example".to_owned()),
            client_id: Set("gitadel".to_owned()),
            client_secret: Set("oidc-secret".to_owned()),
            enabled: Set(true),
            auto_provision: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let public_client = oidc_provider::ActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set("Public".to_owned()),
            issuer_url: Set("https://public.example".to_owned()),
            client_id: Set("gitadel".to_owned()),
            client_secret: Set(String::new()),
            enabled: Set(true),
            auto_provision: Set(false),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();
        let totp = user_totp::ActiveModel {
            user_id: Set(owner.id),
            secret: Set("totp-seed".to_owned()),
            confirmed_at: Set(Some(now)),
            last_used_step: Set(None),
            created_at: Set(now),
        }
        .insert(&database)
        .await
        .unwrap();

        let report = seal_stored_secrets(&database, &cipher).await.unwrap();
        assert_eq!(
            report,
            SealReport {
                dokploy_api_keys: 1,
                mirror_identity_secrets: 1,
                webhook_secrets: 1,
                oidc_client_secrets: 1,
                totp_secrets: 1,
            }
        );
        assert_eq!(report.total(), 5);

        let stored_integration = namespace_integration::Entity::find_by_id(integration.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert!(is_sealed(&stored_integration.api_key));
        assert_eq!(
            cipher
                .open_text(
                    SecretKind::DokployApiKey,
                    &integration.id,
                    &stored_integration.api_key
                )
                .unwrap(),
            "plain-key"
        );
        let stored_identity = namespace_mirror_identity::Entity::find_by_id(identity.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cipher
                .open_text(
                    SecretKind::MirrorIdentitySecret,
                    &identity.id,
                    &stored_identity.secret
                )
                .unwrap(),
            "ghp_plain"
        );
        let stored_hook = repository_webhook::Entity::find_by_id(signed.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cipher
                .open_text(
                    SecretKind::WebhookSecret,
                    &signed.id,
                    stored_hook.secret.as_deref().unwrap()
                )
                .unwrap(),
            "hook-secret"
        );
        assert_eq!(
            repository_webhook::Entity::find_by_id(unsigned.id)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .secret,
            None
        );
        let stored_provider = oidc_provider::Entity::find_by_id(provider.id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cipher
                .open_text(
                    SecretKind::OidcClientSecret,
                    &provider.id,
                    &stored_provider.client_secret
                )
                .unwrap(),
            "oidc-secret"
        );
        assert_eq!(
            oidc_provider::Entity::find_by_id(public_client.id)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .client_secret,
            "",
            "an absent client secret stays absent"
        );
        let stored_totp = user_totp::Entity::find_by_id(totp.user_id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            cipher
                .open_text(SecretKind::TotpSecret, &totp.user_id, &stored_totp.secret)
                .unwrap(),
            "totp-seed"
        );

        // Running again changes nothing.
        let again = seal_stored_secrets(&database, &cipher).await.unwrap();
        assert_eq!(again.total(), 0);
        assert_eq!(
            user_totp::Entity::find_by_id(totp.user_id)
                .one(&database)
                .await
                .unwrap()
                .unwrap()
                .secret,
            stored_totp.secret
        );
    }
}
