# Client-side compaction in Olo Coder

Research and implementation review: **10 November 2026**. This document describes
custom OpenAI-compatible endpoints. Codex continues to use Token Pool's native
Responses compaction and replays its encrypted reasoning and compaction items.

## Evidence behind the design

There is no single demonstrated best compactor across coding models and workloads.
The following primary sources informed this implementation:

- [Compact-Bench (22 October, revised 28 October 2026)](https://arxiv.org/abs/2610.31710)
  measures continuation damage and recurring-detail retention. Mechanical
  extraction retains details that model summaries often discard; retention alone
  does not predict task success. Coder combines exact excerpts and retrievable
  originals with structured summaries.
- [Prune the Summary, Not the History (14 October 2026)](https://arxiv.org/abs/2610.20927)
  identifies unsupported actions introduced by summaries and evaluates source
  judging. Coder checks exact quotations mechanically, then asks a separate
  inference pass whether the complete claim follows from its sources. This is
  an adaptation, **not** a reproduction of the paper's calibrated Jev judge or
  its reported results.
- [Benchmarks Are Surprisingly Compaction-Friendly (22 October 2026)](https://arxiv.org/abs/2610.31778)
  finds that instructions and constraints can disappear despite apparently good
  benchmark performance. Coder preserves original user excerpts, independently
  supplies current goals/specifications/policy, and keeps prior checkpoint entries
  instead of repeatedly rewriting their text.
- [SUMONE (4 November 2026)](https://arxiv.org/abs/2611.06445)
  trains a standalone summarizer using reconstruction of original trajectories.
  It motivates separately configurable summarization models and evaluating
  recoverability. Coder does not train or bundle SUMONE weights, or claim its
  benchmark performance.
- [Letta's Context Constitution (2 April 2026)](https://www.letta.com/blog/context-constitution/)
  describes versioned memory and background memory work. Coder's implementation
  uses an immutable transcript and a transactional checkpoint chain; it does not
  launch autonomous filesystem-editing memory agents.
- [OpenAI's session-memory example](https://developers.openai.com/cookbook/examples/agents_sdk/session_memory)
  provides a practical foundation for preserving recent turns, explicit state,
  chronology and unresolved work in summaries.

These findings support the architecture below. They do not establish that this
harness outperforms another agent on multi-week tasks. Model-dependent summary
quality still needs evaluation on representative real workloads.

## Durable sources, replaceable working context

Every newly committed native conversation item gets an immutable SQLite record.
Its full JSON—including reasoning, unknown provider fields, tool calls and tool
results—is retained. `Session.context_refs` aligns active messages with those
records. Compaction retires a prefix from active context; it never deletes source
records. A `run_context` audit event retains the system instructions, discovered
tool definitions and model configuration used by a run. Each `model_request`
manifest records its source IDs and tracked-state inputs, allowing reconstruction
without duplicating the entire context window on every request. These events are
available through the paged audit history and `history_search`. Raw process output that already lives in an artifact stays there, with
its artifact reference in the tool result.

The transcript has its own persistent full-text index. Image encodings and opaque
encrypted payloads are retained in original records but excluded from text search.
Retrieval returns exact serialized JSON in UTF-8-safe byte pages. Follow
`next_offset` to reconstruct an entire record, including records larger than the
normal tool-result limit. Incremental SQLite reads fetch only the requested page,
so inspecting a small excerpt does not load a multi-megabyte record. Search is
literal substring search, not embedding-based
semantic retrieval. There is no automatic disk-retention expiry.

Agent retrieval pages account for JSON escaping and the configured tool-output
budget. Oversized checkpoint details, search results and other tool responses
return valid JSON with an `artifact` ID and `read_with: "read_artifact"`; their
complete serialized result is durably saved before that reference is exposed.
Follow artifact `next_offset` values until null to recover the exact result.
UTF-8 pages return `text`; binary pages return `encoding: "base64"` and `data`.
This also works at the minimum 1 KB tool-output budget.

Upgrades backfill the active native context available in an older database.
They cannot reconstruct native payloads discarded by earlier versions; the older
audit history remains separately available. Removed Claude sessions are paused,
their original snapshot and context are archived, and opaque Claude items are
never sent to Codex.

## Preparation and application

1. At a configurable soft threshold, one background job snapshots a completed
   tool boundary. Foreground inference and tool work can continue. No idle timer
   scans sessions or starts requests.
2. The job retains recent complete exchanges, splits the expired source prefix
   into bounded chunks, and extracts constraints, decisions, progress, failures,
   open work and uncertainty. Every claim must cite a real record and an exact
   source quotation. Exact identifiers, errors and proposed-versus-completed work
   are part of the summarization instructions.
3. By default, a separate verification request filters unsupported claims. A
   failed or malformed summarization/verification uses labeled source excerpts;
   warnings and the full source-ID inventory remain in the checkpoint. A verifier
   can still make mistakes: “source checked” is not proof or action approval.
4. Previous claims and their evidence are carried forward without recursive
   rewriting. The bounded active index favors constraints, current-goal relevance
   and recency. Older entries remain in parent checkpoints. Original user excerpts
   provide an additional anchor; they are explicitly excerpts, not a complete
   replacement for the user's messages.
5. A `ready` checkpoint commits before application. At a safe boundary, application
   compares the exact source prefix, model configuration and parent checkpoint.
   It preserves every newly appended message and requires an actual context
   reduction. Pending tool batches, unknown effects and approvals prevent applying
   the checkpoint. Context and checkpoint state commit in one transaction.
6. At the hard threshold the foreground waits cancellably for preparation. At the
   end of a foreground answer, an existing job may finish before the run becomes
   idle. Stop cancels its requests; unfinished checkpoints become `interrupted`.
   Each completed chunk is durably checkpointed. Interrupted preparation resumes
   from those chunks when the source prefix, model, policy and tracked inputs still
   match. A matching ready checkpoint is reused without another model request;
   it is never installed over a changed prefix.

Summarization usage counts toward session usage and the current run's budget.
The context meter continues to describe foreground context. The optional model
for compaction can use another endpoint, model, reasoning effort and output limit.
A foreground request checks the latest billed usage again after compaction;
exhaustion stops the run while retaining its completed checkpoint. Explicit
Resume starts a fresh run budget. Already admitted requests can exceed a budget
when their actual usage arrives, including concurrent foreground/background work;
the budget is an admission limit, not a provider-side billing cap.
A background failure backs off until context grows; it does not retry on an idle
polling loop. A hard-boundary failure stops explicitly with the original context
intact instead of quietly dropping messages.

Checkpoint publication and application check cancellation while holding the
journal lock. A worker delayed on that lock cannot revive a checkpoint after
Stop marks it interrupted. Completion commits also check cancellation so a late
completion cannot replace the paused state.

## Defaults and controls

| Setting | Default | Purpose |
| --- | --- | --- |
| `model.context_window` | 128,000 | Configure the actual deployed model's window |
| `model.compact_at` | 90,000 | Hard boundary, further limited by output/headroom |
| `compaction.background` | `true` | Prepare while foreground work continues |
| `compaction.prepare_percent` | 70 | Percentage of hard boundary to start preparing |
| `compaction.keep_recent_exchanges` | 6 | Preferred recent complete exchanges |
| `compaction.keep_recent_tokens` | 12,000 | Preferred verbatim recent context |
| `compaction.chunk_tokens` | 16,000 | Maximum source chunk estimate |
| `compaction.summary_tokens` | 4,000 | Summary output/working-index budget |
| `compaction.verify` | `true` | Source verification pass |
| `compaction.model` | `null` | Use session model; otherwise a full ModelConfig |
| `compaction.instructions` | empty | Domain-specific extraction guidance |

Recent-context and summary budgets adapt downward for smaller windows. Token
counts are conservative byte estimates, not the endpoint's exact tokenizer;
reported provider usage also participates in triggering compaction. Complete
request preflight includes tools and output allowance. Configure context/output
limits for the actual deployment. A single oversized message or tool exchange
may require increasing the window or beginning another session.

Both GUI and TUI accept:

- `/compact`: prepare and apply a checkpoint now.
- `/transcript [RECORD_ID]`: list original records or start reading one.
- `/transcript-search TEXT`: search original records, including retired context.
- `/compactions [CHECKPOINT_ID]`: list checkpoints or inspect sources/evidence.
- `/restore-context CHECKPOINT_ID`: restore the current checkpoint's original
  prefix while idle, preserving newer messages. Repeat in reverse order to
  unwind earlier checkpoints. This changes context only, never filesystem effects.
- `/set compaction.FIELD JSON`: tune a setting while idle.

The JSON API exposes `transcript` (`after`, `limit`, `record`, `offset`),
`transcript_search`, `compactions` (`before`, `checkpoint`), and
`restore_compaction`. Agent tools `read_transcript`, `search_transcript` and
`compaction_history` expose the same original sources and evidence. API schemas
are discoverable using `{"op":"schema"}`. Session ▸ Original Transcript and
Session ▸ Compaction Checkpoints are GUI shortcuts; background progress appears
in both clients' status line.

Forking keeps the bounded working context. It snapshots checkpoint metadata and
inherits read-only transcript ranges, including earlier fork ancestry. The same
retrieval tools resolve inherited record IDs; later parent messages are outside
the captured ranges. Restoring a fork updates its own checkpoint state, never the
parent's. This avoids loading or copying weeks of raw history merely to fork.
Restore requires all ambiguous effects to be resolved first.

## Verification scope

`crates/coder/tests/endpoints.rs` exercises repeated compaction, original-record
retrieval after restart, checkpoint restoration with new messages, forks,
source-quote validation, malformed summaries, judge rejection, cancellation,
provider migration and custom HTTP/tool/reasoning replay. Internal tests cover
stale-prefix/model rejection, pending-tool rejection and transactional application.
These deterministic fixtures test data integrity and control flow; they are not
an evaluation of an actual summarizer's reasoning quality or weeks-long task
success. The ignored archive benchmark measures durable write/search costs in a
release build. Real-provider tests remain opt-in because they consume quota.
