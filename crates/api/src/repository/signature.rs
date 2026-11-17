//! SSH signatures on Git commits.

use russh::keys::ssh_key::{HashAlg, PublicKey, SshSig};

pub(super) enum SshCommitSignature {
    Unsigned,
    Invalid,
    Valid(String),
}

pub(super) fn verify_ssh_commit_signature(body: &[u8]) -> SshCommitSignature {
    let Some((payload, pem)) = ssh_signed_payload(body) else {
        return SshCommitSignature::Unsigned;
    };
    if !pem.starts_with(b"-----BEGIN SSH SIGNATURE-----") {
        return SshCommitSignature::Unsigned;
    }
    let Ok(signature) = SshSig::from_pem(&pem) else {
        return SshCommitSignature::Invalid;
    };
    let public_key = PublicKey::new(signature.public_key().clone(), "");
    if public_key.verify("git", &payload, &signature).is_err() {
        return SshCommitSignature::Invalid;
    }
    SshCommitSignature::Valid(public_key.fingerprint(HashAlg::Sha256).to_string())
}

fn ssh_signed_payload(body: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let header_end = body.windows(2).position(|bytes| bytes == b"\n\n")?;
    let mut cursor = 0usize;
    while cursor <= header_end {
        let line_end = body[cursor..=header_end]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| cursor + offset)?;
        let line = &body[cursor..line_end];
        let value = line
            .strip_prefix(b"gpgsig ")
            .or_else(|| line.strip_prefix(b"gpgsig-sha256 "));
        let Some(first_line) = value else {
            cursor = line_end + 1;
            continue;
        };

        let mut pem = Vec::with_capacity(512);
        pem.extend_from_slice(first_line);
        pem.push(b'\n');
        let mut field_end = line_end + 1;
        while field_end <= header_end && body[field_end] == b' ' {
            let continuation_end = body[field_end..=header_end]
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|offset| field_end + offset)?;
            pem.extend_from_slice(&body[field_end + 1..continuation_end]);
            pem.push(b'\n');
            field_end = continuation_end + 1;
        }

        let mut payload = Vec::with_capacity(body.len() - (field_end - cursor));
        payload.extend_from_slice(&body[..cursor]);
        payload.extend_from_slice(&body[field_end..]);
        return Some((payload, pem));
    }
    None
}

#[cfg(test)]
mod tests {
    use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey, private::Ed25519Keypair};

    use super::{SshCommitSignature, verify_ssh_commit_signature};

    fn signed_commit() -> Vec<u8> {
        let payload = b"tree 0000000000000000000000000000000000000000\nauthor Alice <alice@example.com> 1 +0000\ncommitter Alice <alice@example.com> 1 +0000\n\nSigned commit\n";
        let private_key = PrivateKey::from(Ed25519Keypair::from_seed(&[7; 32]));
        let signature = private_key
            .sign("git", HashAlg::Sha512, payload)
            .expect("sign commit")
            .to_pem(LineEnding::LF)
            .expect("encode signature");
        let header_end = payload
            .windows(2)
            .position(|bytes| bytes == b"\n\n")
            .expect("commit headers");
        let mut body = Vec::with_capacity(payload.len() + signature.len() + 32);
        body.extend_from_slice(&payload[..=header_end]);
        for (index, line) in signature.lines().enumerate() {
            if index == 0 {
                body.extend_from_slice(b"gpgsig ");
            } else {
                body.push(b' ');
            }
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        body.extend_from_slice(&payload[header_end + 1..]);
        body
    }

    #[test]
    fn verifies_git_ssh_signature() {
        assert!(matches!(
            verify_ssh_commit_signature(&signed_commit()),
            SshCommitSignature::Valid(_)
        ));
    }

    #[test]
    fn rejects_signature_after_commit_changes() {
        let mut body = signed_commit();
        body.extend_from_slice(b"tampered");
        assert!(matches!(
            verify_ssh_commit_signature(&body),
            SshCommitSignature::Invalid
        ));
    }
}
