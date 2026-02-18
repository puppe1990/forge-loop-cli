# Status - BDD Scenarios

## Scenario: Existing status file
Given `.forge/status.json` exists
When user runs `forge status`
Then command prints core fields and exits success
