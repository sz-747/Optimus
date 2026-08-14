---
title: "Tauri minimal session orchestrator"
type: ideation
status: complete
date: 2026-06-07
---

# Prompt

What would `cmux` look like if it were a Tauri desktop app that **orchestrated
sessions** rather than **tracking** them, while staying **minimal** and adapting
to the hardware it is running on?

# Grounding

- The current Windows plan is terminal-first: native panes, splits, tabs,
  per-surface routing, notifications, and hook-driven metadata.
- Phase 4 already shifts the product toward orchestration with
  `CMUX_SURFACE_ID`, per-surface environment injection, agent hooks, and a
  socket/command router.
- If moved to Tauri, the product should stop pretending it is primarily a
  terminal renderer. Tauri is a better fit for a control plane than for a
  high-performance embedded terminal multiplexer.
- The new constraint matters: on a 16 GB laptop, "spawn freely and let the user
  manage the fallout" is a product bug. Capacity control must be part of the
  core design, not a settings afterthought.

# Best Direction

## 1. Session control plane, not terminal emulator

This is the strongest version.

The Tauri app becomes a **mission-control layer for AI work sessions**. A
session is not a row of metadata to watch. It is a runtime object with:

- desired task
- repo/worktree
- agent kind (`codex`, `claude`, later others)
- execution policy
- notification rules
- current stage (`queued`, `starting`, `running`, `blocked`, `review`,
  `done`, `failed`)
- next allowed actions

The core move is that the app owns the lifecycle:

- create session
- prepare branch/worktree/env
- launch the agent
- inject routing env vars
- receive hook events
- escalate when stuck
- request review
- hand off or terminate

The terminal becomes secondary. Users may still inspect logs or attach to a
session console, but the product is no longer "a nicer place to host shells."
It is "the desktop conductor for parallel agent work."

## Why it survives critique

- It matches the repo's strongest existing idea: per-surface env injection and
  hook routing already want a control plane.
- It avoids Tauri's weakest fit: rebuilding a native-class terminal renderer in
  a webview stack.
- It turns notifications into actions. A stop hook should create a decision
  point, not just unread state.

# Product Shape

## 2. Persistent task board as the source of truth

The board should not be decorative UI. It should be the product's primary state
model.

A task has:

- title
- description
- owner
- status
- project or repo
- execution mode
- handoff summary history

The board is the single source of truth:

- a task survives crashes and reboots
- moving a card changes the orchestration state
- ownership is explicit
- completion is explicit
- the next agent reads the last handoff summary before starting

The better shape is a **single workbench around that board**:

- Left column: task columns `Triage`, `Todo`, `Ready`, `In Progress`,
  `Blocked`, `Done`
- Main panel: selected task detail
- Thin top bar: machine capacity, active budget, queue pressure
- Bottom drawer: logs / terminal attach only when needed

Each task card shows:

- owner
- repo + branch/worktree
- task title
- current status
- execution mode
- resource class
- last meaningful event
- quick controls: `assign`, `start`, `pause`, `resume`, `review`, `close`

This is better than a kanban-like board because it keeps the center of gravity
on **what can run now**, **what is waiting**, and **why**.

## 3. Board column semantics

The kanban columns should be fixed and meaningful:

- `Triage` - rough ideas land here before there is a full spec; example: "I
  want auth rate limiting"
- `Todo` - task exists but is waiting on a dependency; it should not move until
  its parent finishes
- `Ready` - dependencies are satisfied and the task is waiting for an agent to
  pick it up
- `In Progress` - an agent is actively running on the task
- `Blocked` - the agent hit a wall and flagged the task for a human; nothing
  runs until it is unblocked
- `Done` - finished, with full run history, summary, and metadata preserved

These columns are not just labels. They define orchestration behavior:

- only `Ready` tasks are eligible for admission
- `Todo` tasks are dependency-blocked by design
- `Blocked` tasks are human-blocked by design
- `Done` tasks are immutable except for metadata and audit views

## 4. Task detail as a state machine plus handoff log

Opening a task should feel like inspecting a job controller:

- spec: repo, branch, owner, task, policies
- runtime: pid, start time, cwd, env tags, socket path
- progress: current phase and checkpoints
- artifacts: diff, notes, PR, generated files
- handoff summaries: what changed, what was built, what the next agent needs
  to know
- interventions: send instruction, approve, retry from checkpoint, escalate

The detail view should answer:

- What is this task trying to do?
- What state is it in?
- Why is it allowed or blocked from running?
- What can I do right now?
- What did the last agent leave behind?

## 5. Handoff summaries are mandatory

When an agent finishes a run, it must write a structured handoff summary before
the task can move forward.

Minimum summary shape:

- objective attempted
- status reached
- files changed
- commands run
- artifacts produced
- blockers or open questions
- recommended next step for the next agent

The next agent reads that summary before it starts. This is the continuity
mechanism that makes crash recovery and sequential work viable.

# Technical Shape

## 6. Tauri shell + Rust supervisor

If this were Tauri, the architecture should be:

- Tauri frontend for the workbench UI
- Rust backend as the durable session supervisor
- spawned agent processes managed by Rust
- event/channel stream from Rust to the frontend
- notifications and OS integration through Tauri plugins

Concretely:

- Keep the Rust-heavy parts: process spawning, env injection, IPC, hook
  routing, state machine, persistence.
- Drop the GPU terminal-rendering ambition from the app core.
- Use an optional web terminal/log viewer only for attach/inspection.

## 7. Hardware-aware scheduler is a first-class feature

This is the most important refinement.

The app should not "spawn sessions" directly. It should run every launch
request through an **admission controller**:

- inspect machine profile at startup
- maintain a live capacity budget
- classify sessions by expected cost
- queue when budget is exhausted
- pause or defer low-priority work under pressure

The minimal product behavior on a 16 GB laptop should be conservative by
default:

- max 1-2 active coding sessions
- additional sessions stay queued
- heavy sessions block parallel heavy launches
- review/lightweight sessions may fit alongside one coding session

The user should see this explicitly:

- `Running 2/2`
- `1 queued: waiting for memory budget`
- `Balanced mode`

Not hidden diagnostics. A visible contract.

## 8. Capacity model

Do not try to predict exact RAM usage perfectly. Use coarse classes:

- `light` - hook listener, review worker, metadata task
- `medium` - normal coding agent
- `heavy` - coding agent plus indexing/test/build activity

Each class consumes abstract capacity units rather than raw MB. The device
profile maps hardware into available units.

Example starting policy:

- 16 GB laptop on battery: 2 units
- 16 GB laptop plugged in: 3 units
- 32 GB desktop: 5+ units

Session costs:

- light = 0.5-1 unit
- medium = 1 unit
- heavy = 2 units

So on a 16 GB laptop:

- 1 heavy session can run alone
- or 2 medium sessions
- or 1 medium + 1 light

That is a much safer model than pretending every session is equal.

## 9. Adaptation inputs

The scheduler should adapt to:

- total RAM
- current memory pressure
- CPU core count
- battery vs plugged-in
- thermal / sustained CPU pressure if observable
- whether the user marked a session as high priority

The product does not need perfect OS-level forecasting. It needs sensible
backpressure.

Useful modes:

- `Conservative` - protect interactivity; queue aggressively
- `Balanced` - default
- `Performance` - allow more concurrency when plugged in

## 10. Baseline scheduler spec for the current laptop

Use the observed machine behavior as the initial calibration point:

- with other apps closed, the machine can tolerate about 6-7 terminal sessions
- that number is the theoretical ceiling, not the default orchestration target

Initial scheduler constants:

- `hard_max_sessions = 6`
- `safe_active_sessions = 3`
- `headroom_target = 30%`
- `heavy_session_limit = 1` in `Balanced`
- `heavy_session_limit = 2` in `Performance`

Initial admission policy:

- never admit more than 6 total sessions
- never admit more than 3 active sessions by default
- never admit more than 1 heavy session in balanced mode
- queue additional sessions rather than spawning optimistically
- preserve enough free capacity that the machine remains interactive

Initial resource classes:

- `light` - review, logs, hook handling, metadata refresh
- `medium` - normal coding session
- `heavy` - coding session likely to run tests, builds, indexing, or large diffs

Allowed starting combinations on this laptop:

- `1 heavy`
- `1 heavy + 1 light`
- `1 heavy + 1 medium`
- `1 medium + 1 medium`
- `1 medium + 1 medium + 1 light`
- `1 medium + 1 medium + 1 medium`

Disallowed starting combinations:

- `2 heavy + 1 medium` in balanced mode
- `3 heavy`
- `5 medium`
- anything above 6 total sessions

Required UI signals:

- `Theoretical max: 6`
- `Safe active now: 3`
- `Running: N`
- `Queued: M`
- `Why queued: memory budget / heavy-session cap / manual pause`

This baseline should be configurable later, but the product should ship with
conservative defaults rather than asking the user to discover the crash limit by
trial and error.

## 11. Two execution modes

The orchestrator should support two distinct modes, not one generic scheduler.

### Top-level switch: project mode vs task mode

This switch is fundamental.

Optimus should support two different ways of using the same pool of agent
sessions:

- `Project mode` - each agent session points at its own project or folder and
  drives sequential work for that project, similar to having split terminal
  windows aimed at different repos
- `Task mode` - multiple agent sessions point at the same task, and
  orchestrator/worker/validator roles are injected

This must be an explicit switch in the product, not an emergent side effect of
assignment.

The reason it matters is that these are different user intents:

- "Work on three unrelated things in parallel"
- "Keep one project moving sequentially without losing the ability to fan out
  checks"
- "Coordinate multiple agents on one bounded task"

They need different UX, different admission behavior, and different audit
semantics.

### Mode 0: project mode

Use this when you want the product to behave like a higher-level replacement for
split terminal windows.

Rules:

- each session targets a separate repo or folder
- each session can be owned by a different profile
- there is no shared task card unless the user explicitly creates one
- the main value is launch, visibility, budgeting, persistence, and sequential
  execution for that project
- this is the closest behavior to "three terminals open on three projects"

Project mode still benefits from the scheduler:

- folder-specific sessions survive restarts
- machine capacity limits still apply
- logs, summaries, and completion state are still durable

The important clarification: project mode is **not** strictly limited to one
agent. It means one primary project lane at a time, with optional subordinate
fan-out.

Allowed subordinate behavior in project mode:

- readonly subagents for inspection, analysis, and verification
- temporary parallel checks
- delegated research or audit runs

The primary constraint is that the main project lane remains sequential and
coherent. Parallelism is allowed as support work, not as peer implementation
lanes that create competing write surfaces.

So the mental model is:

- one main owner moving the project forward
- optional subordinate helpers
- low merge pressure
- low context fragmentation

### Mode 1: task mode

Use this when multiple sessions should collaborate on one bounded task.

Rules:

- all participating sessions attach to the same task card
- role injection is explicit
- the orchestrator coordinates workers and validators
- handoff summaries accumulate on the task
- completion is judged against the validation contract
- work should be isolated enough that parallel execution is safe

This is the full Optimus orchestration path.

Task mode exists for tasks that have:

- clear boundaries
- clear goals
- isolated worktrees or isolated change surfaces
- explicit roles and ownership
- a real reason to execute in parallel

Its main operational benefits are:

- reduced context bloat
- reduced git merge conflicts
- clearer ownership of write surfaces
- better validator independence

Task mode therefore needs two hard product features:

- `autocompact` or equivalent context-compaction support between runs
- isolation rules that prefer separate worktrees or otherwise separated write
  scopes when multiple workers are active

### Shared rule: project-manager coordinator

Every run starts with a coordinator role.

On a 16 GB machine with a safe zone of 3 agents, the default live topology is:

- `1 orchestrator`
- `2 workers`

The orchestrator does not mainly write product code. It coordinates:

- checks available profiles before task creation
- scopes the work
- writes the validation contract
- assigns workers
- monitors progress
- decides whether validation is required
- closes or requeues the task

The orchestrator is the control plane agent. The workers are the execution
agents.

In project mode, the orchestrator may stay mostly dormant and act as a scheduler
plus launcher. In task mode, it becomes active and writes scope, validation, and
handoff policy.

### Shared rule: step zero is `kanban_list`

Before creating or assigning any task, the system must discover which real
profiles exist.

Operational rule:

- step zero is always `kanban_list`
- profile discovery happens before task creation
- a task may only be assigned to a real discovered profile

You noted that the system silently skips tasks whose assignee does not match a
real profile. That behavior is acceptable as an execution guard, but the board
should still record why the task did not start. Silent at runtime is fine;
silent in audit is not.

### Mode A: collaborative multi-agent task

Use this when 2-3 agents are working together on the same task.

Rules:

- one board card represents the shared task
- one orchestrator coordinates the card
- up to 2 workers execute in parallel on the card by default on this laptop
- each agent run writes its own handoff entry
- the task stays one unit of truth even if ownership changes during execution
- scheduler still respects machine limits before admitting parallel runs

This mode is for:

- implementation + review pairing
- investigation by one agent, fix by another
- fan-out exploration followed by convergence
- bounded tasks where isolated parallelism is worth the coordination cost

### Mode B: single-agent sequential tasks

Use this when you want one agent at a time across one or more independent
projects.

Rules:

- each task has one active owner at a time
- one orchestrator still coordinates sequencing
- the next task does not start until the current active task completes, pauses,
  or yields
- handoff summaries provide continuity between agents and between sessions
- this mode should be the safest default on constrained hardware

This mode is for:

- one agent per project
- deliberate sequential execution
- maximum stability on a 16 GB laptop
- using subordinate readonly parallelism without turning the project into a
  merge-conflict factory

The UI should make the mode explicit on each task card and in task creation.

The UI should also expose the top-level execution switch when launching work:

- `Launch as project session`
- `Launch as coordinated task`

### Validator phase

After workers finish, the orchestrator may convert the remaining capacity into a
validation phase.

Validation rules:

- validation is optional but policy-driven
- validators must approach the code with fresh context
- validators should be adversarial, not cooperative
- validators verify against the written validation contract, not against the
  worker's self-reported success

The clean model is:

- workers finish execution
- at least one follow-up run is launched in validator mode
- validator mode reads the task, diff, artifacts, and handoff summary
- validator mode has not participated in the implementation run

This matters because "worker becomes validator" is only valid if the validation
run starts from fresh context and is treated as a distinct role, not as the same
continuing conversational state.

## 12. Data model shift

Current direction:

- surface
- pane
- tab
- unread notification
- sidebar metadata

Orchestrator direction:

- task
- session spec
- session runtime
- session stage
- session resource class
- session admission status
- session artifact set
- session attention state
- session dependency edges
- machine capacity profile
- handoff summary
- validation contract
- profile catalog

This is the key semantic change. In the Tauri version, a pane is a view, a
session is a runtime, and a **task on the board** is the durable product
primitive.

# Strong Supporting Ideas

## 13. Recipes instead of raw spawning

Launching a task should usually come from a recipe:

- agent type
- repo selection rule
- branch naming rule
- worktree strategy
- startup prompt template
- hook policy
- completion policy
- expected resource class
- default execution mode
- required profile or profile class

That gives you one-click actions like:

- "Investigate failing test on a fresh worktree"
- "Review PR in parallel with Codex and Claude"
- "Refactor this module with checkpoints every 15 minutes"

Recipes should also define whether a task is allowed to start immediately or
must enter the queue.

## 14. Dependency-aware orchestration

Sessions should be able to depend on other sessions:

- blocked until another session finishes
- fan-out review sessions from one finished coding session
- retry only the failed branch of a multi-session run

This is more valuable than historical tracking because it coordinates live work.

## 15. Attention routing over passive notifications

Notifications should become typed interrupts:

- needs approval
- produced diff
- blocked on tool failure
- waiting for clarification
- completed successfully

The app should route each interrupt into a queue with explicit actions, not just
badge counts.

## 16. Subscription-driven terminals, not API-driven sessions

This is an important architectural rule.

The terminal sessions should be subscription-driven:

- the orchestrator subscribes to session state changes
- the board updates from streamed events
- workers and validators publish lifecycle events, summaries, and status changes
- the UI reflects durable task state derived from those subscriptions

The product should not be modeled as a request/response API that polls a session
for truth. The source of truth is the board plus the subscribed event stream.

This fits the handoff model better:

- session starts
- progress events stream
- completion summary arrives
- validator events stream
- final state lands on the task

## 17. Validation contract defines done before code exists

During scoping, before any worker starts writing code, the orchestrator writes a
validation contract for the task.

That contract defines what "done" means:

- expected behavior
- non-goals
- required checks
- validation steps
- evidence required to close the task

Workers implement toward that contract. Validators test against that contract.
`Done` means the contract was satisfied, not merely that the agent stopped.

## 18. Embedded rules plus user-defined rules

The product should ship with a strong built-in rule set, but it must not be a
closed system.

There are two rule layers:

- `system rules` - embedded defaults that define the safe operating model
- `user rules` - additional policies supplied by the user

System rules are the baseline and include:

- board column semantics
- profile discovery before assignment
- scheduler admission limits
- project mode vs task mode behavior
- handoff summary requirements
- validation contract requirements
- validator freshness and independence
- isolation preferences for parallel work

User rules may extend or tighten the defaults. Examples:

- never run more than 2 sessions after 10 PM
- always require validation for changes under `payments/`
- force project mode for specific repos
- prefer Claude as validator and Codex as worker
- disallow parallel writes on certain branches

The important constraint is that user rules should not silently remove the
system's safety guarantees. The default posture should be:

- user rules can add constraints freely
- user rules can relax some defaults only when the relaxation is explicit
- hard safety rails stay embedded unless the user knowingly opts out

## 19. Rule engine behavior

Rules should be applied as policy, not scattered conditionals.

Each task or session launch should be evaluated against:

1. system rules
2. repo/project rules
3. user/global rules
4. task-specific overrides

When rules conflict:

- the more restrictive rule wins by default
- overrides should be visible in audit history
- the board should record which rule caused blocking, queuing, or validation

This matters because the product is an orchestrator. Users need to know not just
what happened, but which policy decided it.

### 19.1 Precedence order

When more than one matching rule affects the same decision, Optimus should
resolve them in this order:

1. hard safety gates beat soft gates
2. more restrictive effects beat less restrictive effects
3. narrower scope beats broader scope
4. later source layers may tighten earlier ones, but should not silently remove
   hard system safety rails
5. `priority` only breaks ties between otherwise similar rules in the same
   source layer

This keeps the system conservative by default without making precedence feel
random.

### 19.2 Restrictiveness order

For v1, treat effects in roughly this order from most restrictive to least
restrictive:

1. `deny`
2. `mark_blocked`
3. `require_profile`
4. `require_isolation`
5. `force_mode`
6. `force_role`
7. `limit_parallelism`
8. `queue`
9. `require_validation`
10. `warn`
11. `allow`

This is not about severity in the abstract. It is about how strongly the effect
constrains execution.

### 19.3 Scope specificity order

When two matching rules have different scopes, prefer the narrower one:

1. `task_ids`
2. `project_ids`
3. `repo_ids + paths`
4. `repo_ids + branches`
5. `repo_ids`
6. `profiles`
7. global empty scope

This is a tie-break rule, not a license for a narrow rule to bypass hard system
safety.

### 19.4 Source layer behavior

Source layer order remains:

1. system
2. repo/project
3. user/global
4. task override

But layer order does not mean "last one always wins."

The intended behavior is:

- later layers may add restrictions freely
- later layers may make a rule narrower for a specific task or repo
- later layers may relax defaults only when the relaxed behavior is not a hard
  system safety rail
- any relaxation should be visible in audit history

### 19.5 Priority use

`priority` should stay narrow in purpose.

Use it only when:

- both rules are in the same source layer
- both rules have the same gate strength
- both rules have similar scope specificity
- both rules produce the same kind of effect or effects with similar
  restrictiveness

Higher `priority` wins in that tie.

If a conflict needs `priority` to routinely override obvious safety or scope
logic, the rule set is poorly shaped and should be rewritten.

### 19.6 Resolution examples

Example 1:

- system rule says `queue` heavy work under high memory pressure
- user rule says `allow` this task

Result:

- `queue` wins because hard safety and higher restrictiveness beat `allow`

Example 2:

- repo rule says `force_mode=project` on `main`
- task override says `force_mode=task` for `task_123`
- neither is a hard safety rail

Result:

- the task override may win because it is narrower and later, but the audit log
  should show both rules and the reason the override was allowed

Example 3:

- user rule says `require_validation` for `payments/**`
- task-specific rule says `warn` only

Result:

- `require_validation` wins because it is more restrictive

### 19.7 Audit requirement for precedence

When one matching rule beats another, the audit trail should record:

- winning rule id
- losing rule id
- winning reason, such as `hard-gate`, `more-restrictive`, `narrower-scope`, or
  `higher-priority`

That is how the system stays explainable instead of feeling arbitrary.

## 20. Rule schema

The rule schema should be deliberately small. It only needs to express policy
that affects orchestration decisions.

Recommended shape:

- `id` - stable identifier
- `name` - short human-readable label
- `enabled` - whether the rule currently applies
- `source` - `system`, `repo`, `user`, or `task_override`
- `scope` - where the rule applies
- `priority` - only used when two non-safety rules conflict at the same layer
- `mode` - optional restriction to `project`, `task`, or `any`
- `when` - list of predicates that must all match
- `effect` - what the rule does when it matches
- `reason` - short audit message shown on the board
- `created_at`
- `updated_at`

Minimal JSON shape:

```json
{
  "id": "limit-heavy-balanced",
  "name": "Limit heavy sessions in balanced mode",
  "enabled": true,
  "source": "system",
  "scope": {
    "repo_ids": [],
    "task_ids": [],
    "profiles": [],
    "branches": [],
    "paths": []
  },
  "priority": 100,
  "mode": "any",
  "when": [
    { "field": "scheduler.mode", "op": "eq", "value": "Balanced" },
    { "field": "session.resource_class", "op": "eq", "value": "heavy" },
    { "field": "scheduler.active_heavy_count", "op": "gte", "value": 1 }
  ],
  "effect": {
    "type": "queue",
    "queue_reason": "heavy-session-cap"
  },
  "reason": "Queued because the balanced heavy-session cap is already reached.",
  "created_at": "2026-06-07T00:00:00Z",
  "updated_at": "2026-06-07T00:00:00Z"
}
```

## 21. Scope model

`scope` should be explicit and narrow. Avoid hidden targeting logic.

### 21.1 Core scope fields

- `repo_ids`
- `task_ids`
- `project_ids`
- `profiles`
- `branches`
- `paths`

Rules with an empty scope are global within their source layer.

### 21.2 Field meanings

| Scope field | What it matches | Example | Notes |
| --- | --- | --- | --- |
| `repo_ids` | repository ids | `["optimus"]` | Match against canonical repo ids, not display names. |
| `task_ids` | exact task ids | `["task_123"]` | Most specific scope; useful for one-off overrides. |
| `project_ids` | project or workspace ids | `["client-portal"]` | Useful when several repos belong to one larger effort. |
| `profiles` | discovered profile ids | `["claude-code"]` | Use only real profile ids from `kanban_list`. |
| `branches` | git branch names | `["main", "release/*"]` | Prefer exact names in v1; allow simple glob support only if needed. |
| `paths` | task target paths or repo areas | `["payments/**"]` | Best for policy by subsystem or folder area. |

### 21.3 Matching rules

Scope matching should stay simple:

- within one scope field, values are `OR`
- across different scope fields, matching is `AND`
- an omitted scope field means "do not filter on this dimension"
- an empty scope object means the rule is global within its source layer
- if a scoped field cannot be resolved, the rule should not silently match

Examples:

- a repo rule scoped to `optimus`
- a branch rule scoped to `main`
- a task override scoped to one task id

Example meaning:

- `repo_ids=["optimus","cmux"]` means match either repo
- `repo_ids=["optimus"]` plus `branches=["main"]` means match only `optimus`
  on `main`
- `task_ids=["task_123"]` plus any broader scope should still only match
  `task_123`

### 21.4 Specificity guidance

More specific scopes should generally be easier to reason about:

- `task_ids` is more specific than `project_ids`
- `project_ids` is more specific than `repo_ids`
- `repo_ids` plus `paths` is more specific than `repo_ids` alone
- `branches` should narrow repo-level policy, not replace it blindly

This is not a hidden precedence system. It is a guideline for writing clean
rules and for explaining why a narrow override exists.

### 21.5 Deliberate v1 limits

Do not support these scope forms in v1:

- negative scope like "all repos except X"
- regex scope values
- nested scope groups
- scope based on raw prompt text
- scope based on arbitrary environment variables

If those are ever needed, add them as explicit features rather than letting
scope grow into an unstructured filter language.

## 22. Predicate model

The `when` clause should be a flat list of predicates in v1. Treat it as
logical `AND`.

Predicate shape:

- `field`
- `op`
- `value`

Recommended v1 operators:

- `eq`
- `neq`
- `gt`
- `gte`
- `lt`
- `lte`
- `in`
- `not_in`
- `contains`
- `starts_with`
- `ends_with`
- `matches_glob`
- `is_true`
- `is_false`

Do not start with nested boolean expression trees unless forced. A flat `AND`
model plus multiple rules is easier to debug and audit.

Useful fields to support early:

- `scheduler.mode`
- `scheduler.active_count`
- `scheduler.active_heavy_count`
- `machine.on_battery`
- `machine.memory_pressure`
- `task.mode`
- `task.status`
- `task.has_validation_contract`
- `task.repo`
- `task.branch`
- `task.path`
- `task.priority`
- `task.requires_validation`
- `run.profile`
- `run.role`
- `run.resource_class`

## 23. Predicate field catalog

The field catalog is the official vocabulary for `when.field`.

It answers:

- which facts rules are allowed to inspect
- what each fact means
- what type it has
- which operators are valid for it
- where the value comes from

Without this catalog, the rule system will drift into ad hoc names like
`ramHigh`, `repoDirty`, or `currentLoad`, which makes validation and audit
explanations unreliable.

### 23.1 Catalog rules

- field names should use dotted lowercase paths
- fields should describe facts, not decisions
- fields should be normalized before rule evaluation
- missing fields should evaluate as `unknown`, not silently coerce to false
- a rule may only use operators that are valid for that field

### 23.2 Core v1 fields

| Field | Type | Example | Allowed operators | Source | Notes |
| --- | --- | --- | --- | --- | --- |
| `task.status` | enum | `Ready` | `eq`, `neq`, `in`, `not_in` | board state | One of the fixed kanban columns. |
| `task.mode` | enum | `project` | `eq`, `neq`, `in`, `not_in` | task spec | Top-level execution mode. |
| `task.priority` | enum | `high` | `eq`, `neq`, `in`, `not_in` | task spec | Keep v1 coarse: `low`, `normal`, `high`, `urgent`. |
| `task.repo` | string | `optimus` | `eq`, `neq`, `in`, `not_in`, `starts_with`, `ends_with` | task spec | Canonical repo id, not display label. |
| `task.branch` | string | `main` | `eq`, `neq`, `in`, `not_in`, `starts_with`, `ends_with` | git snapshot | Branch at admission time. |
| `task.path` | string | `payments/api` | `eq`, `neq`, `starts_with`, `ends_with`, `matches_glob` | task spec | Primary target path or area. |
| `task.requires_validation` | boolean | `true` | `is_true`, `is_false`, `eq`, `neq` | task spec | Explicit task-level requirement. |
| `task.has_validation_contract` | boolean | `true` | `is_true`, `is_false`, `eq`, `neq` | task artifact store | Indicates scoping wrote the contract. |
| `task.dependency_count` | integer | `2` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | board graph | Number of unresolved parent tasks. |
| `run.role` | enum | `worker` | `eq`, `neq`, `in`, `not_in` | run spec | `orchestrator`, `worker`, `validator`, `subagent`. |
| `run.profile` | string | `claude-code` | `eq`, `neq`, `in`, `not_in` | profile catalog | Must match a real discovered profile. |
| `run.resource_class` | enum | `heavy` | `eq`, `neq`, `in`, `not_in` | run spec | `light`, `medium`, `heavy`. |
| `run.is_fresh_context` | boolean | `true` | `is_true`, `is_false`, `eq`, `neq` | runtime metadata | Important for validator independence. |
| `scheduler.mode` | enum | `Balanced` | `eq`, `neq`, `in`, `not_in` | scheduler config | Eg `Balanced`, `Conservative`, `Performance`. |
| `scheduler.active_count` | integer | `3` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | scheduler state | Total admitted active runs. |
| `scheduler.active_heavy_count` | integer | `1` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | scheduler state | Count of admitted heavy runs. |
| `scheduler.queued_count` | integer | `4` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | scheduler state | Visible backlog size. |
| `scheduler.safe_active_limit` | integer | `3` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | machine profile | Current safe limit after adaptation. |
| `machine.memory_pressure` | number | `0.78` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | machine monitor | Normalized `0.0` to `1.0`. |
| `machine.available_memory_gb` | number | `4.5` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | machine monitor | Rounded, human-auditable metric. |
| `machine.on_battery` | boolean | `false` | `is_true`, `is_false`, `eq`, `neq` | OS power state | Useful for tightening limits on laptops. |
| `machine.cpu_pressure` | number | `0.64` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte` | machine monitor | Also normalized `0.0` to `1.0`. |
| `git.branch` | string | `main` | `eq`, `neq`, `in`, `not_in`, `starts_with`, `ends_with` | git snapshot | Use when run-level git state matters more than task spec. |
| `git.worktree_isolated` | boolean | `true` | `is_true`, `is_false`, `eq`, `neq` | git snapshot | Whether the run has its own isolated write surface. |
| `git.has_uncommitted_changes` | boolean | `true` | `is_true`, `is_false`, `eq`, `neq` | git snapshot | Useful before validator or reassignment. |
| `clock.local_hour` | integer | `22` | `eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `in`, `not_in` | local clock | `0` to `23` in machine local time. |
| `clock.day_of_week` | enum | `Sunday` | `eq`, `neq`, `in`, `not_in` | local clock | Prefer names over integers in audit logs. |

### 23.3 Normalization rules

To keep rules portable and predictable:

- enums should use canonical values, not UI labels or aliases
- percentages should be normalized to decimals like `0.80`, not `80`
- booleans should come from actual runtime state, not string values like
  `"yes"` or `"no"`
- repo, profile, and branch names should be compared against canonical ids
- if a field is unavailable, the audit trail should say `field_unavailable`
  rather than pretending the predicate did not match

### 23.4 Deliberate v1 exclusions

Do not support these as free-form predicate fields in v1:

- full shell command text
- raw prompt contents
- token counts from a provider
- arbitrary environment variables
- unbounded diff contents

If Optimus needs those later, add them as explicit normalized fields with clear
privacy and audit behavior.

## 24. Canonical values

After defining which fields exist, v1 also needs to define the exact allowed
words for fields that use named values.

This matters because the system should not accept near-duplicates like:

- `InProgress`
- `in_progress`
- `in progress`
- `In Progress`

The board, rules engine, scheduler, and audit trail should all store and
compare one canonical version.

### 24.1 Canonical value rules

- every enum-like field should have one official set of allowed values
- internal storage should use the canonical value only
- UI labels may differ, but they should map back to the canonical value
- old aliases may be accepted at import time, but they should be normalized
  immediately
- audits should display the canonical value, not the raw alias that was typed

### 24.2 Core v1 canonical values

| Field | Allowed values | Notes |
| --- | --- | --- |
| `task.status` | `Triage`, `Todo`, `Ready`, `In Progress`, `Blocked`, `Done` | Must match the fixed board columns exactly. |
| `task.mode` | `project`, `task` | `project` = sequential by default, `task` = bounded parallel work. |
| `task.priority` | `low`, `normal`, `high`, `urgent` | Keep v1 small and readable. |
| `run.role` | `orchestrator`, `worker`, `validator`, `subagent` | `subagent` is for helper runs under a main session. |
| `run.resource_class` | `light`, `medium`, `heavy` | Used by scheduler admission rules. |
| `scheduler.mode` | `Conservative`, `Balanced`, `Performance` | Device policy preset, not task priority. |
| `clock.day_of_week` | `Monday`, `Tuesday`, `Wednesday`, `Thursday`, `Friday`, `Saturday`, `Sunday` | Prefer names over numbers for readability. |

### 24.3 Alias normalization

The system may receive older or shorthand forms from imports, user rules, or
manual edits. Those should be rewritten to canonical values before storage or
evaluation.

Examples:

| Field | Acceptable input alias | Stored as |
| --- | --- | --- |
| `task.status` | `InProgress` | `In Progress` |
| `task.status` | `in_progress` | `In Progress` |
| `task.status` | `todo` | `Todo` |
| `task.mode` | `Project` | `project` |
| `run.role` | `pm` | `orchestrator` |
| `run.resource_class` | `HIGH` | `heavy` |
| `scheduler.mode` | `balanced` | `Balanced` |

V1 should keep this alias list intentionally short. If too many aliases are
accepted, the system becomes fuzzy again.

### 24.4 Values that should not be free-form

Do not allow arbitrary user-defined values for these fields in v1:

- `task.status`
- `task.mode`
- `run.role`
- `run.resource_class`
- `scheduler.mode`

If a team wants custom labels later, add a separate display-label layer rather
than weakening the core stored values.

## 25. Effect model

Effects should describe orchestration outcomes, not arbitrary scripting.

### 25.1 Core effect types

- `allow`
- `deny`
- `queue`
- `require_validation`
- `force_mode`
- `force_role`
- `limit_parallelism`
- `require_isolation`
- `require_profile`
- `mark_blocked`
- `warn`

### 25.2 Core effect contract

Each effect type should have a fixed meaning:

- `allow` - explicitly permits a run or transition
- `deny` - blocks a run or transition
- `queue` - defers admission until conditions improve
- `require_validation` - marks validation as required before the task can be
  considered done
- `force_mode` - rewrites the task execution mode
- `force_role` - rewrites the role for the target run
- `limit_parallelism` - caps how many related runs may execute at once
- `require_isolation` - requires a protected write surface such as a worktree
- `require_profile` - requires a real discovered profile before proceeding
- `mark_blocked` - moves or keeps the task in `Blocked`
- `warn` - records a warning without stopping progress

### 25.3 Allowed extra fields by effect type

Each effect type should only allow the extra fields it actually needs.

| `effect.type` | Allowed extra fields | Notes |
| --- | --- | --- |
| `allow` | `gate` | Rare in v1, mainly for explicit allow rules. |
| `deny` | `gate`, `deny_reason` | Use for hard stops that should not become queue states. |
| `queue` | `gate`, `queue_reason` | Preferred for temporary pressure or capacity limits. |
| `require_validation` | `gate`, `validation_reason`, `validator_role` | Can be hard or soft depending on policy. |
| `force_mode` | `gate`, `required_mode` | `required_mode` must be one of the canonical task modes. |
| `force_role` | `gate`, `required_role` | `required_role` must be one of the canonical run roles. |
| `limit_parallelism` | `gate`, `max_parallel`, `parallel_scope` | `parallel_scope` could be `task`, `repo`, or `project`. |
| `require_isolation` | `gate`, `isolation_strategy` | Example strategy: `worktree`. |
| `require_profile` | `gate`, `required_profile` | Used when a named profile must exist. |
| `mark_blocked` | `gate`, `blocked_reason` | Should leave a clear human-readable blocker. |
| `warn` | `gate`, `warning_message` | Purely advisory. |

V1 should reject unknown extra fields. That keeps effects simple to validate and
easy to explain in the UI.

### 25.4 Field meanings

These extra fields should stay narrow and plain:

- `deny_reason` - why the action is forbidden
- `queue_reason` - why the task is waiting instead of running
- `validation_reason` - why validation is required
- `validator_role` - which role should perform validation, usually
  `validator`
- `required_mode` - the exact mode the task must use
- `required_role` - the exact role the run must use
- `max_parallel` - maximum allowed concurrent related runs
- `parallel_scope` - where that limit applies
- `isolation_strategy` - the protection method required for safe writes
- `required_profile` - exact profile id that must exist
- `blocked_reason` - human-facing reason the task is blocked
- `warning_message` - advisory message shown in the UI

### 25.5 Minimal JSON shapes

Queue:

```json
{
  "type": "queue",
  "gate": "hard",
  "queue_reason": "memory-pressure"
}
```

Force task mode:

```json
{
  "type": "force_mode",
  "gate": "hard",
  "required_mode": "project"
}
```

Require validation:

```json
{
  "type": "require_validation",
  "gate": "hard",
  "validation_reason": "payments-change",
  "validator_role": "validator"
}
```

Warn only:

```json
{
  "type": "warn",
  "gate": "soft",
  "warning_message": "Running close to the safe session limit."
}
```

### 25.6 Design rules for effects

- effects should describe one outcome clearly
- effects should not embed scripts or shell commands
- effects should not carry unused fields
- if a rule needs two outcomes, prefer two rules unless the outcomes are
  inseparable
- audits should show both the effect type and its key extra field, such as
  `queue: memory-pressure`

Examples:

- `queue` when memory pressure is high
- `force_mode=project` for a fragile repo
- `require_validation` for changes under `payments/`
- `require_isolation=worktree` when parallel writes are allowed

## 26. Safety model

Some effects are advisory. Some are hard gates.

Suggested gate types:

- `hard` - cannot proceed unless the rule is satisfied or explicitly overridden
- `soft` - can proceed, but the UI records the warning

Add this under `effect`:

```json
{
  "type": "require_validation",
  "gate": "hard"
}
```

Examples:

- missing profile match -> `hard`
- validation recommended for docs-only changes -> `soft`
- high memory pressure queue -> `hard`

## 27. Audit contract

Every matched rule should produce an audit event.

Minimum audit shape:

- `timestamp`
- `rule_id`
- `rule_name`
- `source`
- `target_type` - `task`, `run`, or `session`
- `target_id`
- `decision`
- `reason`

This is how the board can answer:

- Why was this task queued?
- Why was validation required?
- Why was this forced into project mode?

## 28. Example rules

Require validation for payments:

```json
{
  "id": "payments-require-validation",
  "name": "Require validation for payments changes",
  "enabled": true,
  "source": "user",
  "scope": {
    "paths": ["payments/**"]
  },
  "priority": 200,
  "mode": "any",
  "when": [
    { "field": "task.path", "op": "matches_glob", "value": "payments/**" }
  ],
  "effect": {
    "type": "require_validation",
    "gate": "hard"
  },
  "reason": "Payments work must always go through validation."
}
```

Force project mode on main:

```json
{
  "id": "main-branch-project-mode",
  "name": "Force project mode on main",
  "enabled": true,
  "source": "repo",
  "scope": {
    "branches": ["main"]
  },
  "priority": 150,
  "mode": "any",
  "when": [
    { "field": "task.branch", "op": "eq", "value": "main" }
  ],
  "effect": {
    "type": "force_mode",
    "required_mode": "project",
    "gate": "hard"
  },
  "reason": "Main branch work must stay in project mode."
}
```

Limit active sessions late at night:

```json
{
  "id": "late-night-limit",
  "name": "Limit sessions after 10 PM",
  "enabled": true,
  "source": "user",
  "scope": {},
  "priority": 120,
  "mode": "any",
  "when": [
    { "field": "clock.local_hour", "op": "gte", "value": 22 },
    { "field": "scheduler.active_count", "op": "gte", "value": 2 }
  ],
  "effect": {
    "type": "queue",
    "queue_reason": "late-night-limit",
    "gate": "hard"
  },
  "reason": "Late-night rule limits active sessions to 2."
}
```

## 27. Non-goals for v1

Do not add these yet:

- arbitrary user code inside rules
- nested boolean logic trees
- cross-rule mutation
- hidden precedence rules
- side-effectful scripts as rule actions

v1 should stay declarative. If a policy cannot be expressed declaratively, that
is evidence the core orchestrator needs a new built-in capability rather than a
more dangerous rule engine.

## 28. Usage-window forecast

The UI should include a rough forecast of remaining useful session budget in the
current usage window.

Not an exact billing meter. A heuristic forecast:

- recent session start rate
- average session duration
- current active sessions
- projected additional runs for validation

Example signals:

- `Estimated sessions left this window: 5-7`
- `Current plan likely needs 2 more runs`
- `Validation phase will consume 1 additional slot`

This is valuable even if approximate because it helps the user choose between
parallel execution and conservative sequencing.

## 29. Safe degradation under pressure

When the machine comes under pressure, the app should degrade gracefully:

- stop launching queued sessions
- suggest pausing the least important active session
- suspend background watchers first
- reduce polling / event churn
- keep the UI responsive even if agents are busy

This is the practical difference between "adaptive" and "reads system info."

# Rejected Directions

## 30. Full terminal multiplexer in Tauri

Reject.

That rebuilds the hardest part of the current native app inside a weaker shell.
You would spend time on PTY streaming, rendering, selection, scrollback, focus,
copy/paste, and DPI behavior instead of on orchestration.

## 31. "Tracker, but prettier"

Reject.

A task tracker records status only. This product must own launch, control,
recovery, handoff, and routing. If the app is not changing what happens next,
it is still just tracking.

## 32. Unlimited parallel spawning

Reject.

On constrained hardware, unlimited spawning is not a power-user feature. It is
an admission that the app has no operational model.

# Recommendation

Build the Tauri version as a **minimal desktop orchestrator with an optional
terminal attachment**, not as a terminal app with extra metadata.

The first serious version would have:

1. persistent task board
2. Rust supervisor
3. hardware-aware admission controller
4. coordinator-driven orchestration with profile discovery
5. two execution modes: collaborative and sequential
6. mandatory handoff summaries
7. validation contract written during scoping
8. subscription-driven session updates
9. optional attach-to-console view

If you want the product to feel minimal, the rule is simple: **fewer views,
stronger scheduling, stronger handoff discipline**. The sophistication should
live in orchestration policy, not in the chrome.
