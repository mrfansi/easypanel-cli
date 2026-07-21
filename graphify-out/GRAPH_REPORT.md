# Graph Report - .  (2026-07-18)

## Corpus Check
- 27 files · ~105,864 words
- Verdict: corpus is large enough that graph structure adds value.

## Summary
- 710 nodes · 1879 edges · 30 communities (27 shown, 3 thin omitted)
- Extraction: 90% EXTRACTED · 10% INFERRED · 0% AMBIGUOUS · INFERRED: 180 edges (avg confidence: 0.81)
- Token cost: 91,498 input · 0 output

## Community Hubs (Navigation)
- TUI App State
- API Client & CLI Commands
- TUI Integration Tests
- Rendering & Widgets
- Keyboard & Mouse Input
- Server Config Store
- Output Formatting
- Async Worker Requests
- Form Building & Submission
- CLI Command Definitions
- Form Navigation & Steps
- Container Terminal & DB Shell
- Event Loop & External Editor
- Form Field Builders
- Agent Brief & Done Criteria
- Form Field Constructors
- Log Parsing
- HTTP RPC Client
- Panel-Gap Killer Features
- Release Workflow & Changelog
- EasyPanel Platform Model
- Project Overview & API Protocol
- TUI Screens & Log Tail
- Live Verification Discipline
- Chooser List Widget
- API Limits & Feature Requests
- Bug Reports & Credentials
- Install Script
- Env Editing Releases
- Prometheus Metrics Group

## God Nodes (most connected - your core abstractions)
1. `App` - 89 edges
2. `EasypanelClient` - 66 edges
3. `Req` - 44 edges
4. `Form` - 39 edges
5. `field()` - 33 edges
6. `Field` - 29 edges
7. `ServerConfig` - 25 edges
8. `App` - 24 edges
9. `ui()` - 22 edges
10. `form()` - 22 edges

## Surprising Connections (you probably didn't know these)
- `tRPC-style API and easypanel-api.json (374 endpoints)` --semantically_similar_to--> `tRPC-style API protocol (POST /api/rpc/{group}/{op})`  [INFERRED] [semantically similar]
  CONTRIBUTING.md → README.md
- `Middlewares deferred (sprawling Traefik anyOf)` --references--> `Known limits section`  [INFERRED]
  .github/AGENT_BRIEF.md → README.md
- `v0.42.0 — codebase and UI translated to English` --conceptually_related_to--> `North star: complement the EasyPanel web dashboard`  [INFERRED]
  CHANGELOG.md → .github/AGENT_BRIEF.md
- `A test that encodes a wrong assumption is worse than no test` --semantically_similar_to--> `A test that encodes a wrong assumption is worse than no test`  [INFERRED] [semantically similar]
  .github/AGENT_BRIEF.md → CONTRIBUTING.md
- `Recurring bug classes to hunt` --semantically_similar_to--> `Recurring bug classes`  [INFERRED] [semantically similar]
  .github/AGENT_BRIEF.md → CONTRIBUTING.md

## Import Cycles
- 1-file cycle: `src/tui/form.rs -> src/tui/form.rs`

## Hyperedges (group relationships)
- **Live-verification discipline (mocks lie, harnesses lie, drive the real thing)** — _github_agent_brief_no_live_server_constraint, _github_agent_brief_wrong_assumption_test, contributing_live_verification, contributing_dont_trust_measurement, _github_agent_brief_drive_the_tui, _github_pull_request_template_verification_checklist [INFERRED 0.85]
- **Definition of done gate (fmt + clippy + test + changelog + release)** — _github_agent_brief_definition_of_done, contributing_definition_of_done, _github_workflows_ci_check_job, _github_agent_brief_every_change_ships_a_release, _github_workflows_release_release_workflow, changelog_keep_a_changelog [EXTRACTED 1.00]
- **Panel-gap killer features (what the EasyPanel dashboard cannot do)** — readme_clone_service, readme_cross_service_log_search, readme_container_terminal, changelog_db_shell_feature, readme_hosts_screen, _github_agent_brief_north_star [EXTRACTED 1.00]

## Communities (30 total, 3 thin omitted)

### Community 0 - "TUI App State"
Cohesion: 0.06
Nodes (32): Box, Instant, MenuRun, Parser, field(), App, Confirm, HostRow (+24 more)

### Community 1 - "API Client & CLI Commands"
Cohesion: 0.07
Nodes (76): Client, EasypanelClient, String, action_kill(), action_list(), action_row(), action_row_shows_target_duration_and_trims_description(), actions_input() (+68 more)

### Community 2 - "TUI Integration Tests"
Cohesion: 0.04
Nodes (16): create_source(), SourceCall, a_new_app_carries_its_source_in_the_same_request(), a_new_service_without_a_source_sends_none_at_all(), auto_deploy_error_names_the_cause_and_never_swallows_the_rest(), clone_body_drops_identity_and_source_but_keeps_config(), create_form(), dockerfile_source_is_not_mislabelled_as_an_image() (+8 more)

### Community 3 - "Rendering & Widgets"
Cohesion: 0.13
Nodes (48): Cell, Color, Constraint, Frame, Vec, series_last(), series_spark(), auto_cell() (+40 more)

### Community 4 - "Keyboard & Mouse Input"
Cohesion: 0.14
Nodes (17): MouseEvent, App, KeyCode, Sender, auto_deploy_cell(), Line2, metric_cols(), move_table() (+9 more)

### Community 5 - "Server Config Store"
Cohesion: 0.17
Nodes (22): PathBuf, adding_more_keeps_first_default(), corrupt_file_never_wipes_the_server_list(), corrupt_file_reads_as_empty_but_does_not_throw(), empty_when_missing(), first_added_becomes_default_and_persists(), missing_file_is_empty_not_an_error(), path_of() (+14 more)

### Community 6 - "Output Formatting"
Cohesion: 0.10
Nodes (18): NaiveDateTime, age_of(), duration_between(), first_line(), format_bytes(), format_rate(), human_duration(), json_string() (+10 more)

### Community 7 - "Async Worker Requests"
Cohesion: 0.19
Nodes (28): Fn, FnOnce, apply_clone_source(), auto_deploy_error(), clone_body(), clone_service(), env_body(), fetch_view() (+20 more)

### Community 8 - "Form Building & Submission"
Cohesion: 0.26
Nodes (23): basic_auth_body(), build_body(), build_fields(), create_build(), create_domains(), create_env(), domain_body(), domain_fields() (+15 more)

### Community 9 - "CLI Command Definitions"
Cohesion: 0.20
Nodes (19): Shell, ActionCmd, BackupCmd, CertificateCmd, Cli, Command, DomainCmd, main() (+11 more)

### Community 10 - "Form Navigation & Steps"
Cohesion: 0.15
Nodes (9): basic_auth_fields(), Form, FormKind, resource_fields(), Into, Option, Vec, basic_auth_body_sets_clears_and_rejects_half() (+1 more)

### Community 11 - "Container Terminal & DB Shell"
Cohesion: 0.14
Nodes (21): KeyEvent, MaybeTlsStream, Message, base64(), db_command(), encode_key(), resize_msg(), Duration (+13 more)

### Community 12 - "Event Loop & External Editor"
Cohesion: 0.26
Nodes (20): DefaultTerminal, Path, apply_server_action(), disable_mouse(), edit_config_in_editor(), edit_env_in_editor(), edit_text_in_editor(), editor_candidates() (+12 more)

### Community 13 - "Form Field Builders"
Cohesion: 0.12
Nodes (21): mount_fields(), port_fields(), redirect_fields(), source_fields(), branch_falls_back_to_text_when_its_list_cannot_load(), changing_the_builder_version_actually_reaches_the_body(), dockerfile_source_sends_its_contents_inline(), f_val() (+13 more)

### Community 14 - "Agent Brief & Done Criteria"
Cohesion: 0.11
Nodes (20): Code health — audit and refactor to stop bloat, Definition of done (fmt, clippy, test, changelog, honest commit), Do not trust your own harness (echo, tac, diff-render), Drive the TUI and look at it before calling anything done, Feature consistency & gaps — hunt asymmetries, tui/ module boundaries (worker, app, keys, form, table, render), Named colours break contrast; use Color::Indexed, UX, workflow, and architecture are part of "done" (+12 more)

### Community 15 - "Form Field Constructors"
Cohesion: 0.24
Nodes (3): Field, FieldKind, Self

### Community 16 - "Log Parsing"
Cohesion: 0.20
Nodes (9): after(), flattens_loki_entries_oldest_first_with_time_prefix(), format(), format_time(), newest_ts(), Option, String, Value (+1 more)

### Community 17 - "HTTP RPC Client"
Cohesion: 0.25
Nodes (10): maps_401_to_friendly_message(), posts_json_envelope_with_bearer_and_unwraps_json(), Duration, Option, Result, Self, Value, sends_given_input_wrapped_in_json() (+2 more)

### Community 18 - "Panel-Gap Killer Features"
Cohesion: 0.20
Nodes (11): Killer feature: clone a service, ws /ws/containerShell protocol (token query param, JSON framing), Killer feature: remote container terminal (WebSocket), Killer feature: cross-service log search, Killer feature: one-key DB shell (`y`), North star: complement the EasyPanel web dashboard, One-key database shell with stored credentials, v0.38.0 — edit a database service's Config File (+3 more)

### Community 19 - "Release Workflow & Changelog"
Cohesion: 0.22
Nodes (11): Every change ships a release, Release build matrix (linux-x86_64, darwin-arm64, darwin-x86_64), Man page packaging with -s emptiness guard, Single publish job (removes the release-creation race), Release workflow, Pre-publish check that every target artifact exists, CHANGELOG, Keep a Changelog + SemVer convention (+3 more)

### Community 20 - "EasyPanel Platform Model"
Cohesion: 0.22
Nodes (10): EasyPanel runs on Docker Swarm, `enabled` is a config flag, not a running state, Health & crash visibility via getDockerTaskStats, Recurring bug classes to hunt, v0.36.0 — deploy visibility and debounce hint, Recurring bug classes, Crash visibility (`turun` status), Creating a service does not deploy it (+2 more)

### Community 21 - "Project Overview & API Protocol"
Cohesion: 0.31
Nodes (9): Agent brief — hourly self-improvement, Upload source type is unimplementable (server-side archivePath), CONTRIBUTING guide, Contribution scope (take vs ask first), tRPC-style API and easypanel-api.json (374 endpoints), easypanel-cli, Known limits section, Shell completions command (+1 more)

### Community 22 - "TUI Screens & Log Tail"
Cohesion: 0.25
Nodes (8): Logs backed by Grafana Loki + Promtail, Middlewares deferred (sprawling Traefik anyOf), Scalability at hundreds of services / 700+ domains, Redirect rules from the TUI (read-modify-write), Hosts screen (all servers at once), Live log tail via cursor-based polling, Full-screen ratatui TUI, Viewer pane (logs, env, ports, mounts)

### Community 23 - "Live Verification Discipline"
Cohesion: 0.25
Nodes (8): Read-only SSH policy (never mutate over SSH), A test that encodes a wrong assumption is worse than no test, Ask for the verbatim server error, Live verification against a zzz-* throwaway, A test that encodes a wrong assumption is worse than no test, Auto column (✓ / ✗ / -), Errors nest their message under `json`, updateSourceGithub resets autoDeploy server-side

### Community 24 - "Chooser List Widget"
Cohesion: 0.48
Nodes (3): Chooser, ListState, Rect

### Community 25 - "API Limits & Feature Requests"
Cohesion: 0.33
Nodes (6): Alerting is not buildable through the API, Cloudflare Tunnel not verifiable without a Cloudflare token, Hard constraint: no live server, never invent shapes, Templates catalogue is not exposed by the API, Does EasyPanel's own API support it?, Feature request issue template

### Community 26 - "Bug Reports & Credentials"
Cohesion: 0.33
Nodes (6): Bug report issue template, Never paste an API token or servers.json, Usage questions routed to Discussions, easypanel CLI commands, --json raw-response output, Server credentials store (~/.config/easypanel/servers.json)

## Knowledge Gaps
- **11 isolated node(s):** `Shell completions command`, `Cross-service log search (`g`)`, `ws /ws/containerShell protocol (token query param, JSON framing)`, `tui/ module boundaries (worker, app, keys, form, table, render)`, `Keep a Changelog + SemVer convention` (+6 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **3 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `App` connect `TUI App State` to `Form Building & Submission`, `Chooser List Widget`, `Form Navigation & Steps`, `Container Terminal & DB Shell`?**
  _High betweenness centrality (0.129) - this node is a cross-community bridge._
- **Why does `ServerConfig` connect `Server Config Store` to `API Client & CLI Commands`, `Event Loop & External Editor`, `CLI Command Definitions`?**
  _High betweenness centrality (0.124) - this node is a cross-community bridge._
- **Why does `EasypanelClient` connect `API Client & CLI Commands` to `HTTP RPC Client`, `Container Terminal & DB Shell`, `Event Loop & External Editor`, `Async Worker Requests`?**
  _High betweenness centrality (0.105) - this node is a cross-community bridge._
- **What connects `Shell completions command`, `Cross-service log search (`g`)`, `ws /ws/containerShell protocol (token query param, JSON framing)` to the rest of the system?**
  _11 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `TUI App State` be split into smaller, more focused modules?**
  _Cohesion score 0.056043956043956046 - nodes in this community are weakly interconnected._
- **Should `API Client & CLI Commands` be split into smaller, more focused modules?**
  _Cohesion score 0.07052600646488393 - nodes in this community are weakly interconnected._
- **Should `TUI Integration Tests` be split into smaller, more focused modules?**
  _Cohesion score 0.043740573152337855 - nodes in this community are weakly interconnected._