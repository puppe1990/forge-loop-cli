use forge_types::{CircuitBreakerState, CircuitState};

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    pub state: CircuitBreakerState,
    pub no_progress_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitBreakerAction {
    Continue,
    OpenCircuit,
}

impl CircuitBreaker {
    pub fn new(no_progress_limit: u32) -> Self {
        Self {
            state: CircuitBreakerState::default(),
            no_progress_limit,
        }
    }

    pub fn record_progress(&mut self) -> CircuitBreakerAction {
        self.state.consecutive_no_progress = 0;
        self.state.state = CircuitState::Closed;
        CircuitBreakerAction::Continue
    }

    pub fn record_no_progress(&mut self) -> CircuitBreakerAction {
        self.state.consecutive_no_progress += 1;

        if self.state.consecutive_no_progress >= self.no_progress_limit {
            self.state.state = CircuitState::Open;
            CircuitBreakerAction::OpenCircuit
        } else {
            self.state.state = CircuitState::HalfOpen;
            CircuitBreakerAction::Continue
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state.state, CircuitState::Open)
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.state.state, CircuitState::Closed)
    }

    pub fn is_half_open(&self) -> bool {
        matches!(self.state.state, CircuitState::HalfOpen)
    }

    pub fn consecutive_no_progress(&self) -> u32 {
        self.state.consecutive_no_progress
    }

    pub fn reset(&mut self) {
        self.state = CircuitBreakerState::default();
    }

    pub fn from_state(state: CircuitBreakerState, no_progress_limit: u32) -> Self {
        Self {
            state,
            no_progress_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let cb = CircuitBreaker::new(3);
        assert!(cb.is_closed());
        assert!(!cb.is_open());
        assert!(!cb.is_half_open());
    }

    #[test]
    fn record_progress_keeps_closed() {
        let mut cb = CircuitBreaker::new(3);

        let action = cb.record_progress();

        assert_eq!(action, CircuitBreakerAction::Continue);
        assert!(cb.is_closed());
        assert_eq!(cb.consecutive_no_progress(), 0);
    }

    #[test]
    fn first_no_progress_moves_to_half_open() {
        let mut cb = CircuitBreaker::new(3);

        let action = cb.record_no_progress();

        assert_eq!(action, CircuitBreakerAction::Continue);
        assert!(cb.is_half_open());
        assert_eq!(cb.consecutive_no_progress(), 1);
    }

    #[test]
    fn opens_after_limit_reached() {
        let mut cb = CircuitBreaker::new(3);

        cb.record_no_progress();
        cb.record_no_progress();
        let action = cb.record_no_progress();

        assert_eq!(action, CircuitBreakerAction::OpenCircuit);
        assert!(cb.is_open());
        assert_eq!(cb.consecutive_no_progress(), 3);
    }

    #[test]
    fn progress_resets_counter_and_closes() {
        let mut cb = CircuitBreaker::new(3);

        cb.record_no_progress();
        cb.record_no_progress();

        cb.record_progress();

        assert!(cb.is_closed());
        assert_eq!(cb.consecutive_no_progress(), 0);
    }

    #[test]
    fn reset_clears_all_state() {
        let mut cb = CircuitBreaker::new(3);

        cb.record_no_progress();
        cb.record_no_progress();
        cb.record_no_progress();

        cb.reset();

        assert!(cb.is_closed());
        assert_eq!(cb.consecutive_no_progress(), 0);
    }

    #[test]
    fn from_state_restores_previous_state() {
        let state = CircuitBreakerState {
            state: CircuitState::HalfOpen,
            consecutive_no_progress: 2,
        };

        let cb = CircuitBreaker::from_state(state, 5);

        assert!(cb.is_half_open());
        assert_eq!(cb.consecutive_no_progress(), 2);
        assert_eq!(cb.no_progress_limit, 5);
    }

    #[test]
    fn limit_one_opens_immediately() {
        let mut cb = CircuitBreaker::new(1);

        let action = cb.record_no_progress();

        assert_eq!(action, CircuitBreakerAction::OpenCircuit);
        assert!(cb.is_open());
    }

    #[test]
    fn alternating_progress_resets_counter() {
        let mut cb = CircuitBreaker::new(2);

        cb.record_no_progress();
        cb.record_progress();
        cb.record_no_progress();

        assert!(cb.is_half_open());
        assert_eq!(cb.consecutive_no_progress(), 1);
    }
}
