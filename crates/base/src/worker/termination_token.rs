use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct TerminationToken {
  pub inbound: CancellationToken,
  pub outbound: CancellationToken,
}

impl std::fmt::Debug for TerminationToken {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("TerminationToken").finish()
  }
}

impl Default for TerminationToken {
  fn default() -> Self {
    Self::new()
  }
}

impl TerminationToken {
  pub fn new() -> Self {
    Self {
      inbound: CancellationToken::default(),
      outbound: CancellationToken::default(),
    }
  }

  pub fn child_token(&self) -> Self {
    Self {
      inbound: self.inbound.child_token(),
      outbound: self.outbound.child_token(),
    }
  }

  pub fn cancel(&self) {
    self.inbound.cancel();
  }

  pub async fn cancel_and_wait(&self) {
    if self.outbound.is_cancelled() {
      return;
    }

    self.cancel();
    self.outbound.cancelled().await;
  }
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn new_token_is_not_cancelled() {
    let token = TerminationToken::new();
    assert!(!token.inbound.is_cancelled());
    assert!(!token.outbound.is_cancelled());
  }

  #[test]
  fn cancel_only_cancels_inbound() {
    let token = TerminationToken::new();
    token.cancel();
    assert!(token.inbound.is_cancelled());
    assert!(!token.outbound.is_cancelled());
  }

  #[test]
  fn child_token_inherits_cancellation() {
    let parent = TerminationToken::new();
    let child = parent.child_token();

    assert!(!child.inbound.is_cancelled());
    parent.cancel();
    assert!(child.inbound.is_cancelled());
  }

  #[test]
  fn child_token_does_not_cancel_parent() {
    let parent = TerminationToken::new();
    let child = parent.child_token();

    child.cancel();
    assert!(!parent.inbound.is_cancelled());
  }

  #[test]
  fn debug_format() {
    let token = TerminationToken::new();
    let debug = format!("{:?}", token);
    assert!(debug.contains("TerminationToken"));
  }

  #[test]
  fn default_creates_new_token() {
    let token = TerminationToken::default();
    assert!(!token.inbound.is_cancelled());
    assert!(!token.outbound.is_cancelled());
  }

  #[tokio::test]
  async fn cancel_and_wait_returns_immediately_when_outbound_cancelled() {
    let token = TerminationToken::new();
    token.outbound.cancel();
    // Should return immediately without hanging
    token.cancel_and_wait().await;
    assert!(token.inbound.is_cancelled());
  }

  #[tokio::test]
  async fn cancel_and_wait_waits_for_outbound() {
    let token = TerminationToken::new();
    let outbound = token.outbound.clone();

    // Spawn a task that cancels outbound after a short delay
    tokio::spawn(async move {
      tokio::time::sleep(Duration::from_millis(10)).await;
      outbound.cancel();
    });

    let result = tokio::time::timeout(
      Duration::from_secs(1),
      token.cancel_and_wait(),
    )
    .await;

    assert!(result.is_ok());
    assert!(token.inbound.is_cancelled());
    assert!(token.outbound.is_cancelled());
  }
}
