// Copyright 2018-2023 the Deno authors. All rights reserved. MIT license.

use deno_core::ModuleSpecifier;
use log::debug;
use log::error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthTokenData {
    Bearer(String),
    Basic { username: String, password: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthToken {
    host: String,
    token: AuthTokenData,
}

impl fmt::Display for AuthToken {
    #[allow(deprecated)]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.token {
            AuthTokenData::Bearer(token) => write!(f, "Bearer {token}"),
            AuthTokenData::Basic { username, password } => {
                let credentials = format!("{username}:{password}");
                write!(f, "Basic {}", base64::encode(credentials))
            }
        }
    }
}

/// A structure which contains bearer tokens that can be used when sending
/// requests to websites, intended to authorize access to private resources
/// such as remote modules.
#[derive(Debug, Clone)]
pub struct AuthTokens(Vec<AuthToken>);

impl AuthTokens {
    /// Create a new set of tokens based on the provided string. It is intended
    /// that the string be the value of an environment variable and the string is
    /// parsed for token values.  The string is expected to be a semi-colon
    /// separated string, where each value is `{token}@{hostname}`.
    pub fn new(maybe_tokens_str: Option<String>) -> Self {
        let mut tokens = Vec::new();
        if let Some(tokens_str) = maybe_tokens_str {
            for token_str in tokens_str.split(';') {
                if token_str.contains('@') {
                    let pair: Vec<&str> = token_str.rsplitn(2, '@').collect();
                    let token = pair[1];
                    let host = pair[0].to_lowercase();
                    if token.contains(':') {
                        let pair: Vec<&str> = token.rsplitn(2, ':').collect();
                        let username = pair[1].to_string();
                        let password = pair[0].to_string();
                        tokens.push(AuthToken {
                            host,
                            token: AuthTokenData::Basic { username, password },
                        })
                    } else {
                        tokens.push(AuthToken {
                            host,
                            token: AuthTokenData::Bearer(token.to_string()),
                        });
                    }
                } else {
                    error!("Badly formed auth token discarded.");
                }
            }
            debug!("Parsed {} auth token(s).", tokens.len());
        }

        Self(tokens)
    }

    /// Attempt to match the provided specifier to the tokens in the set.  The
    /// matching occurs from the right of the hostname plus port, irrespective of
    /// scheme.  For example `https://www.deno.land:8080/` would match a token
    /// with a host value of `deno.land:8080` but not match `www.deno.land`.  The
    /// matching is case insensitive.
    pub fn get(&self, specifier: &ModuleSpecifier) -> Option<AuthToken> {
        self.0.iter().find_map(|t| {
            let hostname = if let Some(port) = specifier.port() {
                format!("{}:{}", specifier.host_str()?, port)
            } else {
                specifier.host_str()?.to_string()
            };
            if hostname.to_lowercase().ends_with(&t.host) {
                Some(t.clone())
            } else {
                None
            }
        })
    }
}
