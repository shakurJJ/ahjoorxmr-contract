# FEATURE: Refund Customer-Facing Refund Status Tracker with On-Chain State History (ahjoor-refund)

## Overview

Add an immutable, customer-facing on-chain state history ledger to the `ahjoor-refund` contract.

For each refund request, every status transition (submitted/requested, reviewed/approved, rejected, escalated, processed, cancelled, etc.) is appended to a per-refund history Vec stored in persistent contract storage. This allows customers and off-chain indexers to reconstruct the full refund lifecycle without relying only on event logs.

## Motivation

- Stellar event logs are pruned over time and are not directly queryable from within contracts.
- The contract itself must provide a durable, inspectable record of:
  - who took each action (actor)
  - when it happened (ledger sequence)
  - the resulting refund status

## Proposed Behaviour

### Data model

Define:

```rust
RefundHistoryEntry {
    status: RefundStatus,
    actor: Address,
    ledger: u32,
    note_hash: Option<BytesN<32>>,
}
```

### History storage

Each refund record gains:

- `history: Vec<RefundHistoryEntry>`

Rules:

- Append-only: no entry may be modified or deleted.
- Each status change appends exactly one new entry (until the cap).
- The history is stored in contract persistent storage and bumped on each access.

### note_hash semantics

- Optional off-chain note commitment.
- Represents `SHA-256(message)` stored off-chain by the actor.
- If not provided by the actor during a transition call, store `None`.

### Public API

Add:

- `get_refund_history(refund_id) -> Vec<RefundHistoryEntry>`

### History cap

- `MAX_HISTORY_ENTRIES = 20`
- Once the history reaches 20 entries:
  - further transitions do **not** append to storage
  - transitions beyond the cap emit events only

## Status Transition Coverage

Every public method that transitions `Refund.status` must also append a history entry with:

- the new `RefundStatus` reached
- the transition `actor` (the address that authorized / initiated the call)
- `ledger` = `env.ledger().sequence()` cast to `u32`
- optional `note_hash`

## Acceptance Criteria

1. `RefundHistoryEntry` struct defined.
2. Refund records store a `history: Vec<RefundHistoryEntry>`.
3. Every status transition appends an entry to history.
4. `get_refund_history(refund_id)` returns the full history Vec.
5. Append-only integrity: no modifications/deletions are possible.
6. Cap of 20 enforced; beyond-cap transitions do not modify stored history.
7. Tests cover:
   - full lifecycle history population
   - cap enforcement
   - correctness of `get_refund_history`
   - append-only integrity

## Implementation Notes

- Use Soroban `#[contracttype]` for `RefundHistoryEntry` and any new enums/types.
- Ensure TTL bumping is performed for the refund record (and optionally for history-related storage keys if history is stored separately).
- Cap enforcement should be enforced at the moment of append.

## Test Plan

Add tests under `contracts/ahjoor-refund/src/`:

- `test_refund_history_full_lifecycle.rs`
- `test_refund_history_cap.rs`
- `test_refund_history_read_fn.rs`
- `test_refund_history_append_only.rs`

Core test scenarios:

1. Drive a refund through multiple status transitions and assert history length and content.
2. Trigger >20 transitions and assert that stored history length remains exactly 20.
3. Call `get_refund_history` and assert equality to stored history.
4. Verify that after the history is full, later transitions do not change existing entries.
