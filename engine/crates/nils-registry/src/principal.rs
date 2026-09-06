// SPDX-License-Identifier: AGPL-3.0-only

//! The principal (Wave 4a §9.2, C30): who did a thing, as `user@node` from
//! the start, so that federation adds no column later. At the command line
//! it is the local user on this host, recorded as such; a door maps an
//! OIDC subject onto the same shape; `NILS_PRINCIPAL` names one outright,
//! which is how a door hands the principal to the verb it runs.

use std::fmt;

use crate::job::hostname;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user: String,
    pub node: String,
}

impl Principal {
    /// The one to record now: `NILS_PRINCIPAL` if set and well formed,
    /// else the operating system's user on this host.
    pub fn current() -> Principal {
        if let Some(p) = std::env::var("NILS_PRINCIPAL")
            .ok()
            .and_then(|v| Principal::parse(&v))
        {
            return p;
        }
        Principal::local()
    }

    /// The operating system's user on this host.
    pub fn local() -> Principal {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        Principal {
            user,
            node: hostname(),
        }
    }

    /// `user@node`. A name with no `@` is a user on this host, because
    /// that is what every record written before the shape existed meant.
    pub fn parse(text: &str) -> Option<Principal> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        match text.rsplit_once('@') {
            Some((user, node)) if !user.is_empty() && !node.is_empty() => Some(Principal {
                user: user.to_string(),
                node: node.to_string(),
            }),
            Some(_) => None,
            None => Some(Principal {
                user: text.to_string(),
                node: hostname(),
            }),
        }
    }
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.user, self.node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_principal_is_user_at_node() {
        let p = Principal::parse("anna@ward-3").unwrap();
        assert_eq!(p.user, "anna");
        assert_eq!(p.node, "ward-3");
        assert_eq!(p.to_string(), "anna@ward-3");
        // A bare user is this host's.
        let bare = Principal::parse("anna").unwrap();
        assert_eq!(bare.user, "anna");
        assert_eq!(bare.node, hostname());
        // An address-like thing with a user that has an @ keeps the last.
        let mail = Principal::parse("a@b@c").unwrap();
        assert_eq!((mail.user.as_str(), mail.node.as_str()), ("a@b", "c"));
        assert!(Principal::parse("@node").is_none());
        assert!(Principal::parse("user@").is_none());
        assert!(Principal::parse("  ").is_none());
    }
}
