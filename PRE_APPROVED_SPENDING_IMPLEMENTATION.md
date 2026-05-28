# Customer Pre-Approved Spending Allowance with On-Chain Consent Record

## Overview

This feature enables customers to grant merchants pre-approved spending allowances with comprehensive on-chain consent records. It provides granular control over spending limits, daily caps, and transaction limits while maintaining a complete audit trail of all consent and spending activities.

## Key Components

### Data Structures

#### SpendingAllowance

- **allowance_id**: Unique allowance identifier
- **customer**: Customer address
- **merchant**: Merchant address
- **token**: Token address
- **total_amount**: Total allowance amount
- **amount_spent**: Amount already spent
- **created_at**: Creation timestamp
- **expires_at**: Expiration timestamp
- **status**: Allowance status (Active, Paused, Revoked, Expired, Exhausted)
- **consent_hash**: Hash of consent terms
- **consent_timestamp**: When consent was given
- **consent_metadata**: Metadata about consent (IP, device, location)
- **per_transaction_limit**: Maximum per transaction
- **daily_limit**: Maximum per day
- **daily_spent**: Amount spent today
- **daily_reset_timestamp**: When daily limit resets

#### ConsentRecord

- **consent_id**: Unique consent identifier
- **customer**: Customer address
- **merchant**: Merchant address
- **consent_type**: Type of consent (SpendingAllowance, RecurringPayment, DataSharing, Marketing)
- **consent_hash**: Hash of consent terms
- **timestamp**: Consent timestamp
- **expires_at**: Consent expiration
- **ip_hash**: Hashed IP address
- **device_hash**: Hashed device fingerprint
- **location_hash**: Hashed geographic location
- **status**: Consent status (Active, Revoked, Expired)
- **metadata**: Additional metadata

#### AllowanceTransaction

- **tx_id**: Transaction ID
- **allowance_id**: Associated allowance ID
- **amount**: Amount spent
- **timestamp**: Transaction timestamp
- **status**: Transaction status (Pending, Approved, Declined, Completed, Reversed)
- **reference**: Transaction reference/description

#### AllowanceAuditLog

- **log_id**: Log entry ID
- **allowance_id**: Associated allowance ID
- **action**: Action type (Created, Modified, Paused, Resumed, Revoked, etc.)
- **actor**: Address that performed action
- **timestamp**: Action timestamp
- **details**: Action details

### Core Functions

#### create_allowance

Creates a new spending allowance with consent.

**Parameters:**

- customer: Customer address
- merchant: Merchant address
- token: Token address
- total_amount: Total allowance amount
- per_transaction_limit: Maximum per transaction
- daily_limit: Maximum per day
- expires_at: Expiration timestamp
- consent_hash: Hash of consent terms
- consent_metadata: Consent metadata

**Returns:** Allowance ID

**Validation:**

- All amounts must be positive
- Per-transaction limit ≤ total amount
- Daily limit ≤ total amount
- Expiration must be in future
- Only customer can create

#### record_consent

Records consent for an allowance with detailed metadata.

**Parameters:**

- customer: Customer address
- merchant: Merchant address
- consent_type: Type of consent
- consent_hash: Hash of consent terms
- ip_hash: Hashed IP address
- device_hash: Hashed device fingerprint
- location_hash: Hashed location
- expires_at: Expiration timestamp
- metadata: Additional metadata

**Returns:** Consent ID

**Validation:**

- Expiration must be in future
- Only customer can record consent

#### spend_from_allowance

Spends from an allowance with validation.

**Parameters:**

- allowance_id: Allowance ID
- amount: Amount to spend
- reference: Transaction reference

**Returns:** AllowanceTransaction record

**Validation:**

- Allowance must exist and be active
- Amount ≤ per-transaction limit
- Daily spending + amount ≤ daily limit
- Total spending + amount ≤ total amount
- Allowance must not be expired
- Automatic daily limit reset after 24 hours

#### pause_allowance

Pauses an allowance temporarily.

**Parameters:**

- allowance_id: Allowance ID

**Validation:**

- Only customer can pause
- Allowance must exist

#### resume_allowance

Resumes a paused allowance.

**Parameters:**

- allowance_id: Allowance ID

**Validation:**

- Only customer can resume
- Allowance must be paused

#### revoke_allowance

Permanently revokes an allowance.

**Parameters:**

- allowance_id: Allowance ID

**Validation:**

- Only customer can revoke
- Allowance must exist

#### revoke_consent

Revokes a consent record.

**Parameters:**

- consent_id: Consent ID

**Validation:**

- Only customer can revoke
- Consent must exist

#### get_allowance

Retrieves allowance details.

**Parameters:**

- allowance_id: Allowance ID

**Returns:** SpendingAllowance or None

#### get_consent

Retrieves consent record.

**Parameters:**

- consent_id: Consent ID

**Returns:** ConsentRecord or None

#### get_remaining_balance

Gets remaining balance for an allowance.

**Parameters:**

- allowance_id: Allowance ID

**Returns:** Remaining amount

#### get_daily_remaining

Gets remaining daily balance.

**Parameters:**

- allowance_id: Allowance ID

**Returns:** Remaining daily amount

#### get_allowance_transactions

Gets transaction history for an allowance.

**Parameters:**

- allowance_id: Allowance ID

**Returns:** Vector of AllowanceTransaction

#### get_audit_log

Gets audit log for an allowance.

**Parameters:**

- allowance_id: Allowance ID

**Returns:** Vector of AllowanceAuditLog

#### get_customer_allowances

Gets all allowances for a customer.

**Parameters:**

- customer: Customer address

**Returns:** Vector of SpendingAllowance

#### get_merchant_allowances

Gets all allowances for a merchant.

**Parameters:**

- merchant: Merchant address

**Returns:** Vector of SpendingAllowance

#### verify_consent

Verifies if consent is valid.

**Parameters:**

- consent_id: Consent ID

**Returns:** Boolean

#### update_allowance_limits

Updates spending limits for an allowance.

**Parameters:**

- allowance_id: Allowance ID
- per_transaction_limit: New per-transaction limit
- daily_limit: New daily limit

**Validation:**

- Only customer can update
- New limits must be positive

## Storage Schema

### Instance Storage

- `allowance_counter`: Current allowance ID counter
- `consent_counter`: Current consent ID counter
- `transaction_counter`: Current transaction ID counter
- `audit_log_counter`: Current audit log ID counter

### Persistent Storage

- `allowance_{id}`: Allowance data
- `consent_{id}`: Consent records
- `transaction_{id}`: Transaction records
- `audit_log_{id}`: Audit log entries
- `customer_allowances_{address}`: List of allowance IDs for customer
- `merchant_allowances_{address}`: List of allowance IDs for merchant

## Error Handling

### SpendingAllowanceError

- **AllowanceNotFound**: Allowance does not exist
- **AllowanceExpired**: Allowance has expired
- **AllowanceExhausted**: Total allowance exhausted
- **AllowanceRevoked**: Allowance has been revoked
- **TransactionExceedsLimit**: Transaction exceeds limits
- **DailyLimitExceeded**: Daily limit exceeded
- **PerTransactionLimitExceeded**: Per-transaction limit exceeded
- **UnauthorizedAccess**: Caller not authorized
- **ConsentNotFound**: Consent record not found
- **ConsentExpired**: Consent has expired
- **ConsentRevoked**: Consent has been revoked
- **InvalidConsentHash**: Consent hash invalid
- **AllowancePaused**: Allowance is paused
- **InvalidAllowanceAmount**: Amount validation failed

## Usage Example

```rust
// Create allowance with consent
let allowance_id = create_allowance(
    env,
    customer,
    merchant,
    usdc_token,
    10_000_000,      // 10 USDC total
    1_000_000,       // 1 USDC per transaction
    5_000_000,       // 5 USDC per day
    expires_at,
    consent_hash,
    consent_metadata,
);

// Record detailed consent
let consent_id = record_consent(
    env,
    customer,
    merchant,
    ConsentType::SpendingAllowance,
    consent_hash,
    ip_hash,
    device_hash,
    location_hash,
    expires_at,
    metadata,
);

// Spend from allowance
let tx = spend_from_allowance(
    env,
    allowance_id,
    500_000,  // 0.5 USDC
    "Purchase",
);

// Check remaining balance
let remaining = get_remaining_balance(env, allowance_id);

// Pause if needed
pause_allowance(env, allowance_id);

// Resume later
resume_allowance(env, allowance_id);

// Get audit trail
let audit_log = get_audit_log(env, allowance_id);
```

## Security Considerations

1. **Authorization**: Only customers can create and manage allowances
2. **Consent Recording**: Detailed metadata captures context of consent
3. **Limit Enforcement**: Multiple layers of limits (per-transaction, daily, total)
4. **Audit Trail**: Complete history of all actions
5. **Revocation**: Customers can revoke at any time
6. **Expiration**: Automatic expiration of allowances and consent
7. **Daily Reset**: Automatic daily limit reset

## Privacy Features

1. **Hashed Metadata**: IP, device, and location are hashed
2. **Consent Hash**: Terms are hashed, not stored in full
3. **Audit Trail**: Transparent record of all activities
4. **Revocation**: Complete control over consent

## Future Enhancements

1. Biometric verification for high-value transactions
2. Risk scoring for transaction approval
3. Machine learning for fraud detection
4. Integration with identity verification services
5. Multi-signature approval for allowance creation
6. Spending analytics and insights
7. Automatic allowance renewal
8. Tiered allowance levels based on merchant reputation
