# Run - BDD Scenarios

## Scenario: Completion gate succeeds
Given an iteration output with `STATUS: COMPLETE` and `EXIT_SIGNAL: true`
When `forge run` analyzes the output
Then it exits with completion reason
And writes final status as completed

## Scenario: Completion gate fails
Given an iteration output with `STATUS: COMPLETE` and `EXIT_SIGNAL: false`
When `forge run` analyzes the output
Then it continues looping
And does not mark status as completed
