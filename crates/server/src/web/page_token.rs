//! Tokens of the pages being shown. A page shown with `ssp web` finds one in `window.ssp` and
//! may read metrics, displays and system figures through the API with it (see `api::auth`).
//! A token is valid while its page is shown: it is never written to disk or the log.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, MutexGuard};

use base64::Engine;

/// The tokens of this process's pages.
static VALID: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// A random token, valid until this value is dropped.
#[derive(Debug)]
pub struct PageToken(String);

impl PageToken {
    /// A new token.
    pub fn issue() -> Self {
        let mut bytes = [0u8; 24];
        if let Err(err) = getrandom::fill(&mut bytes) {
            // Without randomness there is no secret to hand out; a page then reads nothing.
            tracing::error!("cannot make a page token: {err}");
            return Self(String::new());
        }
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        valid().insert(token.clone());
        Self(token)
    }

    /// The token, as the page sends it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for PageToken {
    fn drop(&mut self) {
        valid().remove(&self.0);
    }
}

/// Whether `token` is the token of a page being shown.
pub fn is_valid(token: &str) -> bool {
    !token.is_empty() && valid().contains(token)
}

fn valid() -> MutexGuard<'static, HashSet<String>> {
    VALID
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_valid_while_it_lives() {
        let token = PageToken::issue();
        let value = token.as_str().to_owned();
        assert_eq!(value.len(), 32);
        assert!(is_valid(&value));
        assert_ne!(PageToken::issue().as_str(), value);
        drop(token);
        assert!(!is_valid(&value));
        assert!(!is_valid(""));
    }
}
