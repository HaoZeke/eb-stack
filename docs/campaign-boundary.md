# Campaign Execution Boundary Documentation

*Generated from the current repository state (commit `1fd9fb568f60a5531527e82181ee18ce14b36c10`).*

## 1. Direct Executions performed by `src/campaign.rs`

| Action | Command / Function | Conditions | File:Line Reference |
|--------|-------------------|------------|---------------------|
| Read campaign bundle manifest | `read_json(&manifest_path)` | Always after acquiring lock | `src/campaign.rs:268-269` |
| Extract package name & version from manifest | `manifest.pointer("/package/name")` and `manifest.pointer("/package/version")` | Must exist in manifest | `src/campaign.rs:270-277` |
| Load recipes list | `load_campaign_recipes(&request.bundle)` | Must return at least one recipe | `src/campaign.rs:278-283` |
| Discover Resolvo profile locks | `discover_files(&request.bundle.join("locks"), "json")` | Must find at least one lock file | `src/campaign.rs:284-288` |
| Initialise or load campaign state JSON | `read_json(&request.state_path)` or construct new `CampaignState` | If state file exists, schema must match | `src/campaign.rs:291-319` |
| Record interrupted previous attempt (if any) | `record_interrupted_attempt(&mut state, &request.state_path)` | When previous state was `Running` | `src/campaign.rs:321`, function at `src/campaign.rs:800` |
| Write updated state to disk (atomic) | `write_state(&request.state_path, &state)` | After each state transition | `src/campaign.rs:1377-1390` |
| Pre-flight packaging gate for each recipe | `packaging_gate(&resolved, &[])` | Runs before any build; on error creates a `BuildFinding` with class `Checksum` or `Unknown` | `src/campaign.rs:356` |
| Stage bundle on target | `request.target.stage_bundle(&request.bundle)` | May fail with transport error | `src/campaign.rs:402` |
| Build each recipe on target | `request.target.build_command_with_overlay(...)` then `command.execute()` | Executes EasyBuild command; on failure creates a `BuildFinding` | `src/campaign.rs:532` |
| Verify binaries for each profile | `request.target.verification_command(&program, &args)` then `command.execute()` | Runs verification commands; on failure creates a `BuildFinding` | `src/campaign.rs:629` |
| Finalise campaign state as `Completed` | Set `state.status = CampaignStatus::Completed` and write state | After all builds and verification succeed | `src/campaign.rs:700` |

## 2. Files Written (but not executed) and Their Consumers

| File | Description | Who/What Consumes It |
|------|-------------|----------------------|
| `state_path` (e.g. `campaign.json`) | Persistent JSON representation of the campaign state, including history, findings, claims | Human operators, other campaign runs, debugging tools |
| `state_path.lock` and `state_path.lock.guard` | Exclusive lock metadata to prevent concurrent campaigns | `CampaignLock::acquire` ensures only one process owns the lock |
| Temporary `*.tmp` files during state writes | Atomic write-then-rename to avoid partial state files | The `write_state` helper (`src/campaign.rs:1377-1390`) |
| `campaign.lock` metadata JSON (`*.lock`) | Records host, PID, start ticks for the lock holder | Lock acquisition / release logic (`CampaignLock::acquire` and `Drop`) |
| `campaign.findings` entries inside the state JSON | Records failures, interruptions, and their evidences | Human reviewers, `eb-stack campaign finding` commands |
| `campaign.history` entries inside the state JSON | Chronological log of actions (e.g., "build evaluation on <target>") | Auditing, debugging |

## 3. Refusals (Error Paths): Where They Are Raised

### In `src/campaign.rs`

| Error Variant | Situation | Where Raised (File:Line) |
|---------------|-----------|--------------------------|
| `CampaignError::InvalidBundle` | Missing required fields in bundle (e.g., no recipes, no locks, missing manifest entries) | `src/campaign.rs:273-288` |
| `CampaignError::UnsupportedSchema` | State file schema version does not match `CAMPAIGN_SCHEMA_VERSION` | `src/campaign.rs:293-294` |
| `CampaignError::StateIdentity` | Existing state file refers to a different package/version than bundle | `src/campaign.rs:296-297` |
| `CampaignError::Busy` | Another process owns the campaign lock (`CampaignLock::acquire`) | `src/campaign.rs:1414-1416` (inside `CampaignLock::acquire`) |
| `CampaignError::Target` (wraps `TargetError`) | A target command that cannot be started is stored as a finding; `run_campaign` returns `Ok` with status `Failed` | Variant at `src/campaign.rs:1528`. Call sites `src/campaign.rs:402`, `444`, `537`, and `630` go through `record_target_command_failure` |
| `CampaignError::Io` / `Json` | File system or JSON (de)serialization failures | `read_json` at `src/campaign.rs:1371-1375` and `write_state` at `src/campaign.rs:1377-1390` |
| `CampaignError::FindingNotFound`, `FindingOwned`, `FindingState` | Operations on campaign findings that are invalid (e.g., claiming an already-owned finding) | `src/campaign.rs:843-876` (`claim_finding`) and `src/campaign.rs:881-910` (`resolve_finding`) |

### In `src/foreign.rs`

| Error Variant | Situation | Where Raised (File:Line) |
|---------------|-----------|--------------------------|
| `ForeignError::Parse` | Recipe could not be parsed as YAML or Python abstract syntax tree (AST) | `src/foreign.rs:575-598` (conda parsing) and `src/foreign.rs:1418` (Spack parsing) |
| `ForeignError::Unsupported` | File extension/name not recognised as a supported foreign format | `src/foreign.rs:354` (`parse_foreign_path`; detection is `detect_foreign_format` at `src/foreign.rs:289`) |
| `ForeignError::Io` | Underlying file read error | `src/foreign.rs:351` (`parse_foreign_path`) |

## 4. Operator Model (Human / Agent Requirements)

| Requirement | Explanation |
|-------------|-------------|
| **Easyconfigs tree** | The campaign needs a populated Easyconfigs directory (`EB_EASYCONFIGS`) that matches the bundle's `easyconfigs/` sub-directory. This is used by `resolve_easyconfig_file` and by the target's EasyBuild robot paths. |
| **Target configuration** | A `BuildTarget` (local, SSH, Slurm, Podman/Docker) must be provided in `CampaignRequest`. It defines how commands are executed, transport, executor, and runtime. |
| **Credentials / Access** | For remote targets (SSH, container registries) the operator must ensure appropriate authentication (SSH keys, token files) are available in the environment; the code does not manage credentials itself. |
| **State file location** | `request.state_path` must be writable by the operator. The system guarantees exclusive access via a lock file (`*.lock`) and atomic writes (`*.tmp` to final). |
| **Bundle integrity** | The operator must provide a valid bundle directory containing `package.plan.json`, `easyconfigs/`, and `locks/`. The system validates these before proceeding and aborts with `InvalidBundle` if missing. |
| **Guaranteed outcomes** | *Atomic state persistence*: each state transition is written atomically and recorded in the history. *Exclusive execution*: only one campaign can run against a given state file at a time. *Failure classification*: any command failure is classified into a `BuildFindingClass` with a deterministic disposition, enabling automated retry or human judgment. |

---

*This document is generated from the source code.*
