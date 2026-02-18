use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    HalfOpen,
    Open,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CircuitBreakerState {
    pub state: CircuitState,
    pub consecutive_no_progress: u32,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_no_progress: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RunStatus {
    pub state: String,
    pub current_loop: u64,
    pub total_loops_executed: u64,
    pub last_error: Option<String>,
    pub completion_indicators: u32,
    pub exit_signal_seen: bool,
    pub session_id: Option<String>,
    pub circuit_state: CircuitState,
    pub current_loop_started_at_epoch: u64,
    pub last_heartbeat_at_epoch: u64,
    pub updated_at_epoch: u64,
}

impl Default for RunStatus {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            current_loop: 0,
            total_loops_executed: 0,
            last_error: None,
            completion_indicators: 0,
            exit_signal_seen: false,
            session_id: None,
            circuit_state: CircuitState::Closed,
            current_loop_started_at_epoch: 0,
            last_heartbeat_at_epoch: 0,
            updated_at_epoch: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProgressSnapshot {
    pub loops_with_progress: u64,
    pub loops_without_progress: u64,
    pub last_summary: String,
    pub updated_at_epoch: u64,
}
