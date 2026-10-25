# Taint is tracked per task, not per value

Once any run in a task reads outside content, the whole task is tainted, and every write or egress call set to allow asks a human instead. CaMeL and FIDES track untrusted data per value, which is more precise but needs a data-flow layer between the model and every tool. We chose one per-task flag because a person can read it on an approval card and audit it, and because the code that sets it is small enough to explain line by line.

## Consequences

- Over-asking is expected: a tainted task asks before writes that had nothing to do with what was read.
- Taint never clears within a task. The label follows the data across tasks: notes and files a tainted task writes are stored untrusted, and a later task that reads them is tainted too, so outside content cannot be laundered through storage.
- A manipulated Lead can still delegate a bad step, but every risky action it triggers asks.
