use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use dashmap::DashMap;
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRateLimitBudget {
  pub local: u32,
  pub global: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRateLimitRule {
  pub matches: String,
  pub ttl: u64,
  pub budget: TraceRateLimitBudget,
}

struct CompiledRule {
  matches: Regex,
  ttl: Duration,
  budget: TraceRateLimitBudget,
}

#[derive(Debug)]
struct Entry {
  count: u32,
  expires_at: Instant,
}

#[derive(Debug, Clone, Default)]
pub struct SharedRateLimitTable {
  table: Arc<DashMap<String, Entry>>,
}

impl SharedRateLimitTable {
  /// Spawns a background task that removes expired entries every `interval`.
  /// The task shuts down when `cancel` is cancelled.
  pub fn spawn_cleanup_task(
    &self,
    interval: Duration,
    cancel: CancellationToken,
  ) {
    let table = self.table.clone();
    tokio::spawn(async move {
      loop {
        tokio::select! {
          _ = cancel.cancelled() => break,
          _ = tokio::time::sleep(interval) => {
            let now = Instant::now();
            table.retain(|_, entry| entry.expires_at > now);
          }
        }
      }
    });
  }

  /// Returns `Ok(())` when the request is allowed, or `Err(retry_after_ms)`
  /// with the number of milliseconds until the window resets when denied.
  pub fn check_and_increment(
    &self,
    key: &str,
    budget: u32,
    ttl: Duration,
  ) -> Result<(), u64> {
    let now = Instant::now();

    let mut entry =
      self.table.entry(key.to_string()).or_insert_with(|| Entry {
        count: 0,
        expires_at: now + ttl,
      });

    if now >= entry.expires_at {
      tracing::debug!(
        key,
        count = entry.count,
        budget,
        "rate limit entry expired, resetting"
      );
      entry.count = 0;
      entry.expires_at = now + ttl;
    }

    let allowed = entry.count < budget;
    tracing::trace!(
      key,
      count = entry.count,
      budget,
      ?ttl,
      allowed,
      "rate limit check"
    );

    if !allowed {
      let retry_after_ms =
        entry.expires_at.saturating_duration_since(now).as_millis() as u64;
      return Err(retry_after_ms);
    }

    entry.count += 1;
    Ok(())
  }
}

/// Bundles the shared table and per-worker rules together.
/// `Some` only when both are present; passed through the worker creation chain.
#[derive(Debug, Clone)]
pub struct TraceRateLimiterConfig {
  pub table: SharedRateLimitTable,
  pub rules: Vec<TraceRateLimitRule>,
  /// Caller-supplied stable key shared across all instances of the same
  /// function.  Used as the rate-limit key for untraced requests so the global
  /// budget accumulates correctly regardless of how many worker instances exist.
  pub global_key: Option<String>,
}

/// Rate-limit configuration as it travels through the worker creation pipeline.
///
/// - `Rules`: rules provided by JS; the pool hasn't attached a shared table yet.
/// - `Configured`: the pool has assembled the full config; ready for compilation.
#[derive(Debug, Clone, Default)]
pub enum RateLimiterOpts {
  #[default]
  Disabled,
  Rules {
    rules: Vec<TraceRateLimitRule>,
    global_key: String,
  },
  Configured(TraceRateLimiterConfig),
}

#[derive(Clone)]
pub struct TraceRateLimiter {
  table: SharedRateLimitTable,
  rules: Arc<Vec<CompiledRule>>,
  global_key: Option<String>,
}

impl TraceRateLimiter {
  pub fn new(
    TraceRateLimiterConfig {
      table,
      rules,
      global_key,
    }: TraceRateLimiterConfig,
  ) -> Result<Self, regex::Error> {
    let compiled = rules
      .into_iter()
      .map(|r| {
        Ok(CompiledRule {
          matches: Regex::new(&r.matches)?,
          ttl: Duration::from_secs(r.ttl),
          budget: r.budget,
        })
      })
      .collect::<Result<Vec<_>, regex::Error>>()?;

    Ok(Self {
      table,
      rules: Arc::new(compiled),
      global_key,
    })
  }

  /// Returns `Ok(())` when the request is allowed, or `Err(retry_after_ms)`
  /// with the number of milliseconds until the window resets when denied.
  /// Returns `Ok(())` when the request is allowed, or `Err(retry_after_ms)`
  /// with the number of milliseconds until the window resets when denied.
  pub fn check_and_increment(
    &self,
    url: &str,
    key: &str,
    is_traced: bool,
  ) -> Result<(), u64> {
    let rule = self.rules.iter().find(|r| r.matches.is_match(url));

    let Some(rule) = rule else {
      return Ok(());
    };

    if is_traced {
      self
        .table
        .check_and_increment(key, rule.budget.local, rule.ttl)
    } else {
      // For untraced requests a stable global_key is required so the global
      // budget accumulates correctly across worker instances.  Deny the request
      // if the caller did not supply one.
      let Some(fid) = self.global_key.as_deref() else {
        return Err(0);
      };
      self
        .table
        .check_and_increment(fid, rule.budget.global, rule.ttl)
    }
  }
}

#[cfg(test)]
mod test {
  use super::*;

  fn make_table() -> SharedRateLimitTable {
    SharedRateLimitTable::default()
  }

  fn make_limiter(
    local_budget: u32,
    global_budget: u32,
    ttl_secs: u64,
    pattern: &str,
    global_key: Option<&str>,
  ) -> TraceRateLimiter {
    TraceRateLimiter::new(TraceRateLimiterConfig {
      table: make_table(),
      rules: vec![TraceRateLimitRule {
        matches: pattern.to_string(),
        ttl: ttl_secs,
        budget: TraceRateLimitBudget {
          local: local_budget,
          global: global_budget,
        },
      }],
      global_key: global_key.map(String::from),
    })
    .unwrap()
  }

  // -- SharedRateLimitTable --

  #[test]
  fn table_allows_requests_within_budget() {
    let table = make_table();
    let ttl = Duration::from_secs(60);

    assert!(table.check_and_increment("key1", 3, ttl).is_ok());
    assert!(table.check_and_increment("key1", 3, ttl).is_ok());
    assert!(table.check_and_increment("key1", 3, ttl).is_ok());
  }

  #[test]
  fn table_denies_requests_exceeding_budget() {
    let table = make_table();
    let ttl = Duration::from_secs(60);

    for _ in 0..3 {
      table.check_and_increment("key1", 3, ttl).unwrap();
    }
    let result = table.check_and_increment("key1", 3, ttl);
    assert!(result.is_err());
  }

  #[test]
  fn table_returns_retry_after_ms_on_denial() {
    let table = make_table();
    let ttl = Duration::from_secs(60);

    for _ in 0..2 {
      table.check_and_increment("key1", 2, ttl).unwrap();
    }
    let retry_after = table.check_and_increment("key1", 2, ttl).unwrap_err();
    // Should be roughly 60 seconds (in ms), give some margin
    assert!(retry_after > 0);
    assert!(retry_after <= 60_000);
  }

  #[test]
  fn table_resets_after_ttl_expires() {
    let table = make_table();
    let ttl = Duration::from_millis(1);

    // Exhaust the budget
    table.check_and_increment("key1", 1, ttl).unwrap();
    assert!(table.check_and_increment("key1", 1, ttl).is_err());

    // Wait for TTL to expire
    std::thread::sleep(Duration::from_millis(5));

    // Should be allowed again
    assert!(table.check_and_increment("key1", 1, ttl).is_ok());
  }

  #[test]
  fn table_tracks_keys_independently() {
    let table = make_table();
    let ttl = Duration::from_secs(60);

    table.check_and_increment("key1", 1, ttl).unwrap();
    assert!(table.check_and_increment("key1", 1, ttl).is_err());

    // Different key should still be allowed
    assert!(table.check_and_increment("key2", 1, ttl).is_ok());
  }

  // -- TraceRateLimiter --

  #[test]
  fn limiter_allows_unmatched_urls() {
    let limiter = make_limiter(1, 1, 60, r"^https://api\.example\.com", None);

    // URL doesn't match the rule pattern
    assert!(
      limiter
        .check_and_increment("https://other.com/path", "key1", true)
        .is_ok()
    );
  }

  #[test]
  fn limiter_uses_local_budget_for_traced_requests() {
    let limiter = make_limiter(2, 100, 60, r".*", Some("global"));

    // Traced requests use local budget (2)
    assert!(
      limiter
        .check_and_increment("https://api.example.com", "key1", true)
        .is_ok()
    );
    assert!(
      limiter
        .check_and_increment("https://api.example.com", "key1", true)
        .is_ok()
    );
    assert!(
      limiter
        .check_and_increment("https://api.example.com", "key1", true)
        .is_err()
    );
  }

  #[test]
  fn limiter_uses_global_budget_for_untraced_requests() {
    let limiter = make_limiter(100, 2, 60, r".*", Some("func-id"));

    // Untraced requests use global budget (2)
    assert!(
      limiter
        .check_and_increment("https://api.example.com", "key1", false)
        .is_ok()
    );
    assert!(
      limiter
        .check_and_increment("https://api.example.com", "key2", false)
        .is_ok()
    );
    assert!(
      limiter
        .check_and_increment("https://api.example.com", "key3", false)
        .is_err()
    );
  }

  #[test]
  fn limiter_denies_untraced_without_global_key() {
    let limiter = make_limiter(100, 100, 60, r".*", None);

    // No global key → always denied for untraced
    assert_eq!(
      limiter
        .check_and_increment("https://api.example.com", "key1", false)
        .unwrap_err(),
      0
    );
  }

  #[test]
  fn limiter_traced_and_untraced_budgets_are_independent() {
    let limiter = make_limiter(2, 2, 60, r".*", Some("func-id"));

    // Exhaust local budget for traced
    limiter
      .check_and_increment("https://a.com", "trace-key", true)
      .unwrap();
    limiter
      .check_and_increment("https://a.com", "trace-key", true)
      .unwrap();
    assert!(
      limiter
        .check_and_increment("https://a.com", "trace-key", true)
        .is_err()
    );

    // Global budget should still be available
    assert!(
      limiter
        .check_and_increment("https://a.com", "any", false)
        .is_ok()
    );
  }

  #[test]
  fn limiter_invalid_regex_returns_error() {
    let result = TraceRateLimiter::new(TraceRateLimiterConfig {
      table: make_table(),
      rules: vec![TraceRateLimitRule {
        matches: "[invalid".to_string(),
        ttl: 60,
        budget: TraceRateLimitBudget {
          local: 1,
          global: 1,
        },
      }],
      global_key: None,
    });
    assert!(result.is_err());
  }

  #[tokio::test]
  async fn table_cleanup_task_removes_expired_entries() {
    let table = make_table();
    let cancel = CancellationToken::new();
    let ttl = Duration::from_millis(10);

    // Add an entry
    table.check_and_increment("key1", 100, ttl).unwrap();

    // Start cleanup task with a fast interval
    table.spawn_cleanup_task(Duration::from_millis(5), cancel.clone());

    // Wait for entry to expire and cleanup to run
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Entry should have been cleaned up; new request gets a fresh window
    assert!(table.check_and_increment("key1", 1, ttl).is_ok());

    cancel.cancel();
  }
}
