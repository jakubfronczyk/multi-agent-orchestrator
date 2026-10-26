# multi-agent-orchestrator

A team of agents works on a task while a human approves every risky action, and each approval shows why it was asked.

## Work

**Task**:
One root run, any follow-ups after it, and every run they delegate. Each is one turn of the Lead. Taint, budget and refused calls belong to the task. The run limit (12) counts per turn, so each follow-up starts its own.
_Avoid_: job, session, conversation

**Budget**:
The most a task may spend on model calls. It is checked before each call, so every run that is running can end one call past it. A call that failed, timed out or was cancelled is not counted, since the provider never reported its cost.
_Avoid_: limit, quota

**Follow-up**:
The user's next message in a finished task: it starts a new Lead run in the same task, which keeps the task's taint, budget and refused calls.
_Avoid_: reply, new task, continuation

**Draft only**:
A turn that reads and drafts; nothing is saved, noted, sent or fetched. Each such call is refused with no card, and the refusal does not stop a later turn from making the same call.
_Avoid_: dry run, preview mode

**Task text**:
The instruction string given to a run, by the user for the root run or a follow-up, or by the parent for a delegated run.
_Avoid_: prompt, task (for the string)

**Run**:
One agent's loop of model steps inside a task.
_Avoid_: execution, thread

**Call**:
One function call the model makes. It gets exactly one output.
_Avoid_: tool use, invocation

**Delegation**:
The Lead handing a task text to a specialist, which starts a child run in the same task.
_Avoid_: handoff, subtask

**Plan**:
The Lead's ordered list of steps for a task, recorded each time it changes.

## Agents

**Team**:
A Lead and its specialists. Every task runs inside one team. A team changes what a task can reach, never how a call is decided.
_Avoid_: workspace, tenant, org

**Team folder**:
A team's own files. Its agents read and write it; another team's agents reach it only through a file grant.
_Avoid_: workspace, drive

**File grant**:
The user's permission for one team to open another team's folder, read or read and write. A grant never changes a file's trust.
_Avoid_: share, permission

**Shared reference**:
The `data/` folder every team reads and no agent writes.

**Agent**:
A configured team member with a slug, a prompt, a model and a decision per tool.
_Avoid_: bot, assistant

**Lead**:
The one agent in a team that plans and delegates. It cannot touch outside content itself.
_Avoid_: orchestrator, manager

**Specialist**:
Any agent that is not the Lead. It works only on task texts delegated to it.
_Avoid_: worker, sub-agent

**Slug**:
An agent's short, fixed identifier, the name the model sees; unique within its team.

**Note**:
One memory entry, private to an agent or shared by its team, kept with every past version.
_Avoid_: memory item, record

## Approval

**Decision**:
An agent's setting for one tool: allow, ask or deny. A tool with no decision is off and not offered.
_Avoid_: permission, mode, level

**Effect**:
The fixed kind of risk a tool carries: control, read, memory, write, egress or external.
_Avoid_: category, risk level

**Approval**:
The card for one call that needs a human answer.
_Avoid_: warrant

**Custody**:
What an approval shows about how the call came to be: who asked, through which agents, after reading what, under which rules.
_Avoid_: provenance, trace

**Rule**:
A named reason that shaped a call's decision, such as the agent's setting, an off-allowlist host or taint.
_Avoid_: policy, reason code

**Refused**:
A call stopped by the system without asking a human.
_Avoid_: blocked, rejected

**Denied**:
A call a human said no to on its approval.
_Avoid_: refused (for a human answer)

**Allowlist**:
The hosts a task may fetch from without asking.

## Trust

**Taint**:
The mark a task carries once any run in it has read outside content. Nothing clears it.
_Avoid_: contamination, flag

**Taint source**:
One piece of outside content that tainted a task: an inbox message, a web address, or a note or file stored as untrusted.

**Trust**:
Whether a result, note or file is trusted or untrusted. Anything written by a tainted task is untrusted.
_Avoid_: safety, confidence

**Outside content**:
Anything that is not the user's own message or the team's work: the inbox, the web and uploaded files. An upload counts even though the user attached it, since its text was written elsewhere.
_Avoid_: external data, input
