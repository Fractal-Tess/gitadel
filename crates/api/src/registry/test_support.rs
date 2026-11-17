//! Digest helper for HTTP registry authentication tests.

use sha2::{Digest as _, Sha256};

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
