# TODO: Payment Authorization Pre-Approval (Two-Step Settlement) — Issue #127

## Steps

- [ ] 1. Update `PaymentStatus` enum in `lib.rs` (add `Authorized = 5`, shift `ScheduledPending = 6`)
- [ ] 2. Add `capture_deadline: u64` field to `Payment` struct in `lib.rs`
- [ ] 3. Update all `Payment { ... }` constructors in `lib.rs` to include `capture_deadline: 0`
- [ ] 4. Extract `finalize_payment` settlement helper from `complete_payment_internal` in `lib.rs`
- [ ] 5. Add `authorize_payment` function in `lib.rs`
- [ ] 6. Add `capture_payment` function in `lib.rs`
- [ ] 7. Update `expire_payment` to allow `Authorized` status in `lib.rs`
- [ ] 8. Update `dispute_payment` to allow `Authorized` status in `lib.rs`
- [ ] 9. Update `bulk_expire_payments` to allow `Authorized` status in `lib.rs`
- [ ] 10. Add `PaymentAuthorized` and `PaymentCaptured` events in `events.rs`
- [ ] 11. Add emitter functions for new events in `events.rs`
- [ ] 12. Add tests for authorization, capture within window, missed window, dispute during auth in `test.rs`
- [ ] 13. Run `cargo test -p ahjoor-payments` and fix any issues

