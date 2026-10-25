# The tool set is closed and each tool's risk is fixed

## Context

Every policy rule reads a tool's risk: its effect (control, read, memory, write, egress, external), whether its output is outside content, and whether the Lead may hold it. If that risk came from configuration, one wrong setting would quietly switch a rule off.

## Decision

The 13 tools are the variants of `ToolName`, and `effect()`, `taints()` and `lead()` are exhaustive matches on it. A new tool does not compile until all three are decided. Configuration chooses only, per agent, whether a tool is off, allow, ask or deny. `decide` takes the strictest of that setting and the rules, so configuration can tighten a call but never loosen it below what its risk requires. Validation rejects allow on an external tool, and `decide` still asks if validation is bypassed.

## Consequences

- Each tool's risk can be read in one file and checked in review.
- Taint, the allowlist and "external always asks" hold for every agent a user creates.
- Adding a tool is a code change: a variant, three match arms and a server implementation.
- Users cannot bring their own tools.

## Rejected alternatives

- **Plugin tools with configurable risk.** This is the flexible option, but each plugin would need its effect and taint declared correctly. A plugin marked "read" that sends data would bypass every rule, and nothing in the code could catch it. If plugins were added later, they would start as ask and count as outside content.
