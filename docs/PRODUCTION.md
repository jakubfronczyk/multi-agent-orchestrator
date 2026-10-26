# Production

How the system in `ARCHITECTURE.md` becomes a deployed, multi-node service. Each step names the event that triggers it.

## Steps

Until step 4, every deploy interrupts running tasks and expires pending approvals.

**Step 0. Freeze the schema.** When there is first real data.
- Freeze the first migration and only add new ones from then on.
- Run migrations and the seed once per deploy, not on every node start.
- Back up the SQLite file continuously with Litestream.
- Until step 3, run one ECS task with the SQLite file on an attached EBS volume, and deploy stop-first (minimum healthy 0%, maximum 100%). A rolling deploy would start the new task while the old one runs: boot recovery would close the old task's live runs, and both would write the same file.

**Step 1. First deploy.** When anyone else can reach it.
- Replace the loopback Host/Origin guard with the public host behind the ALB.
- Add OIDC login and record who started, approved or cancelled.
- Add authorization: each task, approval and inbox belongs to a user; only the owner (or a role allowed to) can see, approve or cancel it. Without this, any logged-in user can approve another user's send.
- Cap budgets, request bodies and text fields; put demo routes behind a flag.
- Read config from the environment and secrets from a secrets manager.
- Handle SIGTERM, check the database in `/health`, add a Dockerfile.
- Record latency, cost, retries, rate limits and pending approvals; add tracing.
- Add self-hosted Langfuse for LLM observability and evaluation:
  - a trace per task, with each model call and tool call as a span, tagged with cost and latency;
  - prompt versions stored and linked to the runs that used them, so a change can be compared with the previous version;
  - the `web/eval/` cases moved into Langfuse datasets, scored by the existing code checks and the judge, so each prompt version has a tracked score;
  - human feedback on summaries and approvals recorded as scores.
  - Self-hosted because traces contain prompts, emails and fetched pages, which stay inside our network. It is an observability tool, not an agent framework; the loop, policy and gateway stay our code.
- Support erasure by encrypting personal data with a per-user key and deleting the key. The key must cover every store that holds it: event payloads, `tool_calls` arguments, outbox bodies, memory, files, and Langfuse (or mask personal data before it reaches Langfuse and set a retention period).

**Step 2. Postgres.** When high availability is needed, or before step 3.
- Event ids stop arriving in commit order, so replay can skip events. Assign ids under an advisory lock or a per-task sequence.
- API reads lose their consistent snapshot. Use `REPEATABLE READ READ ONLY`.
- Concurrent agent edits can race, and a second Lead returns 500 instead of 422. Lock, and map unique violations to 422.
- A file and its trust label are kept consistent only by an in-process lock. Store them in one row, or in S3 with trust tied to the object version.
- `LIKE` becomes case-sensitive. Use `ILIKE` with a trigram index.

**Step 3. Second API node.** All of these land together.
- Order events per task, and wake other nodes with LISTEN/NOTIFY or Redis.
- Route approvals and cancels through the database.
- Give each task an owner and a lease; recovery only takes over expired leases.
- Share one rate limiter per API key.

**Step 4. Workers and resume.** When runs must survive a deploy.
- Run tasks in a separate worker service.
- Give each tool call an idempotency key.
- Store raw provider items and full tool results, which the log does not keep today; only then can a run resume from the log. Until then, resume means re-running from the last completed call.

**Step 5. Provider router.** When a provider has an outage or cost needs a target.
- Fall back between xAI and Bedrock when a run starts, never mid-run: the transcript carries xAI's encrypted reasoning, which another provider cannot read.
- Use a price table where a provider reports no cost.

**Step 6. Tenants.** With the first external customer.
- Add a tenant id to every row, make unique keys per tenant, store the allowlist in the database.

**Step 7. Data growth.** When data outgrows one database.
- Move the workspace to S3, add full-text and then vector search for memory, archive old events.

**Step 8. Real email.** With the first real recipient. Needs step 1 (per-user inbox) and step 2 (the relay reads the outbox from Postgres, not a SQLite file inside one task).
- Keep `send_message` writing to the outbox; add a status column (pending, sending, sent, failed).
- A relay reads pending rows and puts them on SQS; a sender (Lambda) delivers through SES. The outbox's unique tool call id stops a second row, but SQS delivers at least once and SES has no idempotency key. So the sender first claims the row with a conditional update (pending → sending) and skips it if the claim fails.
- One window remains: a crash after SES accepts the mail and before the row is marked sent leaves it in `sending`. Such rows are never resent automatically; they are shown for a person to check.
- SES bounce and complaint notifications (via SNS) update the row's status, and the run's events show it.
- Incoming mail: SES receiving stores messages in S3 and a handler writes them to the inbox table. They stay outside content, so reading one taints the task as today.
- Verify the sending domain (SPF, DKIM, DMARC) and leave the SES sandbox; until then only verified addresses receive mail.
- Approval before every send stays.

## AWS target

```
          CloudFront ── S3 (web)
              │
             ALB  (OIDC, WAF, idle timeout above SSE keep-alive)
              │
      ┌───────┴────────┐
  ECS: api         ECS: workers ──▶ xAI / Bedrock
      │   ▲              ├──▶ Langfuse on ECS  (traces, prompts, evals)
      │   └── ElastiCache Redis  (notify, rate limit)
      ▼                  ▼
  RDS PostgreSQL, Multi-AZ  (event log, projections, leases)
      │
      └── outbox ──▶ SQS ──▶ sender (Lambda) ──▶ SES / Slack
```

- **ECS Fargate** runs containers without managing hosts, and the API and workers scale separately.
- **ALB** holds long-lived SSE connections and handles OIDC login before requests reach the app.
- **RDS PostgreSQL, Multi-AZ** gives failover and point-in-time restore while keeping events and projections in one transaction.
- **ElastiCache Redis** wakes other nodes on new events and holds the shared rate limiter; the database stays the record.
- **SQS** delivers outbox messages with retries and a dead-letter queue. The outbox has one consumer; EventBridge is added when a second one appears.
- **SES** sends and receives email; bounce and complaint notifications arrive through SNS.
- **Secrets Manager** takes the API key out of `.env` and allows rotation.
- **CloudWatch with OpenTelemetry** collects the traces and metrics from step 1.
- **Langfuse on ECS** holds LLM traces, prompt versions and eval results. Self-hosting it adds its own stores to run (Postgres, ClickHouse, Redis, S3), which is the main cost of keeping traces in-house.

**Not chosen**
- **Aurora:** its read replicas and storage autoscaling are not needed here, and its minimum cost is higher.
- **Cassandra or DynamoDB:** an event and its projections must commit together.
- **Kafka:** there is one consumer.
- **Step Functions:** per-transition pricing and state-size limits do not fit long agent loops; Temporal would be the choice if durable execution is needed.
