# Monitor - BDD Scenarios

## Scenario: Missing files
Given `.forge/status.json` is missing
When `forge monitor` starts
Then dashboard shows default placeholders
And process remains stable
