use std::sync::Arc;

use deno_core::unsync::sync::AtomicFlag;

#[derive(Debug, Clone, Default)]
pub struct RuntimeState {
  pub init: Arc<AtomicFlag>,
  pub evaluating_mod: Arc<AtomicFlag>,
  pub event_loop_completed: Arc<AtomicFlag>,
  pub terminated: Arc<AtomicFlag>,
  pub found_inspector_session: Arc<AtomicFlag>,
  pub mem_reached_half: Arc<AtomicFlag>,
}

impl RuntimeState {
  pub fn is_init(&self) -> bool {
    self.init.is_raised()
  }

  pub fn is_evaluating_mod(&self) -> bool {
    self.evaluating_mod.is_raised()
  }

  pub fn is_event_loop_completed(&self) -> bool {
    self.event_loop_completed.is_raised()
  }

  pub fn is_terminated(&self) -> bool {
    self.terminated.is_raised()
  }

  pub fn is_found_inspector_session(&self) -> bool {
    self.found_inspector_session.is_raised()
  }
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn runtime_state_defaults_all_lowered() {
    let state = RuntimeState::default();
    assert!(!state.is_init());
    assert!(!state.is_evaluating_mod());
    assert!(!state.is_event_loop_completed());
    assert!(!state.is_terminated());
    assert!(!state.is_found_inspector_session());
  }

  #[test]
  fn runtime_state_raise_and_lower() {
    let state = RuntimeState::default();

    state.init.raise();
    assert!(state.is_init());

    state.init.lower();
    assert!(!state.is_init());
  }

  #[test]
  fn runtime_state_flags_are_independent() {
    let state = RuntimeState::default();

    state.init.raise();
    state.terminated.raise();

    assert!(state.is_init());
    assert!(state.is_terminated());
    assert!(!state.is_evaluating_mod());
    assert!(!state.is_event_loop_completed());
  }

  #[test]
  fn runtime_state_clone_shares_flags() {
    let state = RuntimeState::default();
    let cloned = state.clone();

    state.init.raise();
    assert!(cloned.is_init());

    cloned.terminated.raise();
    assert!(state.is_terminated());
  }

  #[test]
  fn runtime_state_mem_reached_half() {
    let state = RuntimeState::default();
    assert!(!state.mem_reached_half.is_raised());

    state.mem_reached_half.raise();
    assert!(state.mem_reached_half.is_raised());
  }
}
