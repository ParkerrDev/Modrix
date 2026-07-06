// SPDX-License-Identifier: GPL-2.0-only
//! Parsing and validation of `nxm://` "Download with Manager" links.
//!
//! Nexus fires:
//!
//! ```text
//! nxm://<game_domain>/mods/<mod_id>/files/<file_id>?key=<k>&expires=<ts>&user_id=<uid>
//! ```
//!
//! The OS hands this string to us untrusted, so every field is validated for
//! meaning (domain charset, positive integer ids, numeric query params) - not
//! just shape - before it can drive an API request. All parsing is bounded: the
//! input length is capped and the path/query have fixed, small structure.

use crate::error::{MAX_NXM_URI_LEN, NxmError};

/// A validated `nxm://` download link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NxmUri {
    /// The Nexus game domain (e.g. `skyrimspecialedition`).
    pub domain: String,
    /// The mod id.
    pub mod_id: u64,
    /// The file id.
    pub file_id: u64,
    /// Free-user download key (premium accounts may omit it).
    pub key: Option<String>,
    /// Expiry timestamp accompanying `key`.
    pub expires: Option<u64>,
    /// The Nexus user id the link was issued to.
    pub user_id: Option<u64>,
}

/// Cap on `key=value` pairs scanned in the query - the real link has three.
const MAX_QUERY_PAIRS: usize = 32;
/// Cap on the game-domain length.
const MAX_DOMAIN_LEN: usize = 64;

impl NxmUri {
    /// Parse and validate an `nxm://` URI.
    ///
    /// # Errors
    ///
    /// Returns an [`NxmError`] describing the first validation failure.
    pub fn parse(input: &str) -> Result<Self, NxmError> {
        if input.len() > MAX_NXM_URI_LEN {
            return Err(NxmError::TooLong);
        }
        let rest = strip_scheme(input).ok_or(NxmError::NotNxm)?;
        let (authority_path, query) = split_once(rest, '?');
        let (domain, path) = split_authority(authority_path);
        validate_domain(domain)?;
        let (mod_id, file_id) = parse_path(path)?;
        let query = parse_query(query)?;
        Ok(Self {
            domain: domain.to_owned(),
            mod_id,
            file_id,
            key: query.key,
            expires: query.expires,
            user_id: query.user_id,
        })
    }
}

/// Strip a case-insensitive `nxm://` prefix, returning the remainder.
fn strip_scheme(input: &str) -> Option<&str> {
    let prefix = input.get(..6)?;
    if prefix.eq_ignore_ascii_case("nxm://") {
        input.get(6..)
    } else {
        None
    }
}

/// Split on the first occurrence of `sep`; the right side is `None` if absent.
fn split_once(s: &str, sep: char) -> (&str, Option<&str>) {
    match s.split_once(sep) {
        Some((left, right)) => (left, Some(right)),
        None => (s, None),
    }
}

/// Split the authority (game domain) from the path at the first `/`.
fn split_authority(authority_path: &str) -> (&str, &str) {
    match authority_path.split_once('/') {
        Some((domain, path)) => (domain, path),
        None => (authority_path, ""),
    }
}

fn validate_domain(domain: &str) -> Result<(), NxmError> {
    let ok = !domain.is_empty()
        && domain.len() <= MAX_DOMAIN_LEN
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(NxmError::InvalidDomain)
    }
}

/// Parse `mods/<mod_id>/files/<file_id>` (leading slash already stripped).
fn parse_path(path: &str) -> Result<(u64, u64), NxmError> {
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let mods = segments.next().ok_or(NxmError::BadPath)?;
    let mod_id = segments.next().ok_or(NxmError::BadPath)?;
    let files = segments.next().ok_or(NxmError::BadPath)?;
    let file_id = segments.next().ok_or(NxmError::BadPath)?;
    if mods != "mods" || files != "files" || segments.next().is_some() {
        return Err(NxmError::BadPath);
    }
    Ok((parse_id(mod_id)?, parse_id(file_id)?))
}

/// Parse a positive integer id (rejecting `0`, signs, and overflow).
fn parse_id(text: &str) -> Result<u64, NxmError> {
    match text.parse::<u64>() {
        Ok(value) if value > 0 => Ok(value),
        _ => Err(NxmError::BadId),
    }
}

/// The optional query parameters of an `nxm://` link.
#[derive(Default)]
struct Query {
    key: Option<String>,
    expires: Option<u64>,
    user_id: Option<u64>,
}

/// Parse the optional query. Unknown keys are ignored; the scan is bounded.
fn parse_query(query: Option<&str>) -> Result<Query, NxmError> {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return Ok(Query::default());
    };
    let mut parsed = Query::default();
    for (index, pair) in query.split('&').enumerate() {
        if index >= MAX_QUERY_PAIRS {
            return Err(NxmError::BadQuery);
        }
        let (name, value) = pair.split_once('=').ok_or(NxmError::BadQuery)?;
        match name {
            "key" if !value.is_empty() => parsed.key = Some(value.to_owned()),
            "expires" => parsed.expires = Some(parse_u64(value)?),
            "user_id" => parsed.user_id = Some(parse_u64(value)?),
            _ => {}
        }
    }
    Ok(parsed)
}

fn parse_u64(text: &str) -> Result<u64, NxmError> {
    text.parse::<u64>().map_err(|_| NxmError::BadQuery)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: &str = "nxm://skyrimspecialedition/mods/12345/files/67890\
                        ?key=abcDEF123&expires=1700000000&user_id=42";

    #[test]
    fn parses_a_free_user_link() {
        let uri = NxmUri::parse(FREE).unwrap();
        assert_eq!(uri.domain, "skyrimspecialedition");
        assert_eq!(uri.mod_id, 12345);
        assert_eq!(uri.file_id, 67890);
        assert_eq!(uri.key.as_deref(), Some("abcDEF123"));
        assert_eq!(uri.expires, Some(1_700_000_000));
        assert_eq!(uri.user_id, Some(42));
    }

    #[test]
    fn parses_a_premium_link_without_query() {
        let uri = NxmUri::parse("nxm://fallout4/mods/1/files/2").unwrap();
        assert_eq!(uri.domain, "fallout4");
        assert_eq!(uri.mod_id, 1);
        assert_eq!(uri.file_id, 2);
        assert!(uri.key.is_none());
        assert!(uri.expires.is_none());
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert!(NxmUri::parse("NXM://fallout4/mods/1/files/2").is_ok());
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert_eq!(
            NxmUri::parse("https://example.com/mods/1/files/2"),
            Err(NxmError::NotNxm)
        );
    }

    #[test]
    fn rejects_bad_domain() {
        assert_eq!(
            NxmUri::parse("nxm://bad_domain!/mods/1/files/2"),
            Err(NxmError::InvalidDomain)
        );
    }

    #[test]
    fn rejects_malformed_paths() {
        for bad in [
            "nxm://game/mods/1/files",         // missing file id
            "nxm://game/plugins/1/files/2",    // wrong first segment
            "nxm://game/mods/1/textures/2",    // wrong third segment
            "nxm://game/mods/1/files/2/extra", // trailing segment
            "nxm://game/mods//files/2",        // empty id
        ] {
            assert_eq!(NxmUri::parse(bad), Err(NxmError::BadPath), "{bad}");
        }
    }

    #[test]
    fn rejects_non_numeric_and_zero_ids() {
        assert_eq!(
            NxmUri::parse("nxm://game/mods/abc/files/2"),
            Err(NxmError::BadId)
        );
        assert_eq!(
            NxmUri::parse("nxm://game/mods/0/files/2"),
            Err(NxmError::BadId)
        );
        assert_eq!(
            NxmUri::parse("nxm://game/mods/-1/files/2"),
            Err(NxmError::BadId)
        );
    }

    #[test]
    fn rejects_bad_query_numbers() {
        assert_eq!(
            NxmUri::parse("nxm://game/mods/1/files/2?expires=notanumber"),
            Err(NxmError::BadQuery)
        );
    }

    #[test]
    fn rejects_overlong_input() {
        let long = format!("nxm://game/mods/1/files/2?key={}", "a".repeat(9000));
        assert_eq!(NxmUri::parse(&long), Err(NxmError::TooLong));
    }
}
