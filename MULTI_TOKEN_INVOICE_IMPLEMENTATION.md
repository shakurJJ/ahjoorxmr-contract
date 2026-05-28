# Multi-Token Invoice Acceptance with Preferred Settlement Conversion

## Overview

This feature enables merchants to create invoices that accept payments in multiple tokens while maintaining conversion rates and settling in a preferred token. It provides a flexible payment system for international commerce and multi-currency transactions.

## Key Components

### Data Structures

#### MultiTokenInvoice

- **invoice_id**: Unique identifier for the invoice
- **merchant**: Merchant address
- **customer**: Customer address
- **created_at**: Invoice creation timestamp
- **due_date**: Invoice due date
- **total_amount**: Total amount in base currency
- **base_currency**: Base currency token address
- **accepted_tokens**: List of tokens accepted for payment
- **preferred_settlement_token**: Token for settlement
- **line_items**: Invoice line items with descriptions, quantities, and prices
- **status**: Invoice status (Draft, Issued, PartiallyPaid, FullyPaid, Overdue, Cancelled)
- **payments_received**: Map of token to amount paid
- **conversion_rates**: Conversion rates for each accepted token to base currency
- **settlement_conversion_rate**: Rate from base to settlement currency
- **metadata**: Additional invoice metadata

#### InvoiceLineItem

- **description**: Item description
- **quantity**: Quantity ordered
- **unit_price**: Price per unit
- **amount**: Total amount for line item
- **tax_rate_bps**: Tax rate in basis points

#### InvoicePayment

- **payment_id**: Unique payment identifier
- **invoice_id**: Associated invoice ID
- **token**: Token used for payment
- **amount**: Amount paid in the token
- **amount_in_base**: Amount converted to base currency
- **amount_in_settlement**: Amount converted to settlement currency
- **paid_at**: Payment timestamp
- **payer**: Payer address
- **tx_hash**: Transaction hash reference

#### SettlementBatch

- **batch_id**: Unique batch identifier
- **invoice_ids**: List of invoices in batch
- **total_settlement_amount**: Total amount in settlement currency
- **settled_at**: Settlement timestamp
- **status**: Settlement status (Pending, Processing, Completed, Failed)
- **merchant**: Merchant receiving settlement

### Core Functions

#### create_invoice

Creates a new multi-token invoice with specified parameters.

**Parameters:**

- merchant: Merchant address
- customer: Customer address
- total_amount: Total invoice amount
- base_currency: Base currency token
- accepted_tokens: List of accepted payment tokens
- preferred_settlement_token: Settlement token
- line_items: Invoice line items
- due_date: Invoice due date
- metadata: Additional metadata

**Returns:** Invoice ID

**Validation:**

- Total amount must be positive
- Line items must be valid (positive quantity and price)
- Maximum 20 line items per invoice
- Due date must be in the future

#### accept_payment

Accepts payment for an invoice in any accepted token.

**Parameters:**

- invoice_id: Invoice ID
- payer: Payer address
- token: Payment token
- amount: Payment amount

**Returns:** InvoicePayment record

**Validation:**

- Invoice must exist and not be cancelled
- Token must be in accepted tokens list
- Conversion rate must be set for the token
- Payment cannot exceed remaining balance
- Invoice status updates based on payment

#### set_conversion_rate

Sets the conversion rate for a token to base currency.

**Parameters:**

- merchant: Merchant address
- token: Token address
- rate_to_base: Conversion rate (scaled by 1,000,000)

**Validation:**

- Rate must be positive
- Only merchant can set rates

#### set_settlement_conversion_rate

Sets the conversion rate from base to settlement currency.

**Parameters:**

- merchant: Merchant address
- invoice_id: Invoice ID
- rate: Conversion rate (scaled by 1,000,000)

**Validation:**

- Rate must be positive
- Only merchant can set rates

#### settle_invoices

Settles multiple invoices in batch with preferred token conversion.

**Parameters:**

- merchant: Merchant address
- invoice_ids: List of invoice IDs to settle

**Returns:** SettlementBatch record

**Validation:**

- All invoices must be fully paid
- All invoices must belong to merchant
- Maximum 50 invoices per batch

#### get_invoice

Retrieves invoice details.

**Parameters:**

- invoice_id: Invoice ID

**Returns:** MultiTokenInvoice or None

#### get_invoice_status

Gets the current status of an invoice.

**Parameters:**

- invoice_id: Invoice ID

**Returns:** InvoiceStatus

#### get_invoice_balance

Gets the remaining balance for an invoice.

**Parameters:**

- invoice_id: Invoice ID

**Returns:** Remaining amount in base currency

#### cancel_invoice

Cancels an invoice.

**Parameters:**

- invoice_id: Invoice ID

**Validation:**

- Only merchant can cancel
- Invoice must not be fully paid

#### get_settlement_batch

Retrieves settlement batch details.

**Parameters:**

- batch_id: Batch ID

**Returns:** SettlementBatch or None

## Storage Schema

### Instance Storage

- `invoice_counter`: Current invoice ID counter
- `payment_counter`: Current payment ID counter
- `settlement_batch_counter`: Current batch ID counter

### Persistent Storage

- `invoice_{id}`: Invoice data
- `invoice_payment_{id}`: Payment records
- `settlement_batch_{id}`: Settlement batch data
- `merchant_rates_{address}`: Merchant conversion rates

## Error Handling

### MultiTokenInvoiceError

- **InvoiceNotFound**: Invoice does not exist
- **InvalidInvoiceStatus**: Invoice status does not allow operation
- **PaymentExceedsInvoiceAmount**: Payment exceeds remaining balance
- **TokenNotAccepted**: Token not in accepted tokens list
- **ConversionRateNotSet**: Conversion rate not configured
- **InvoiceExpired**: Invoice due date has passed
- **UnauthorizedAccess**: Caller not authorized
- **InvalidLineItem**: Line item validation failed
- **SettlementFailed**: Settlement operation failed
- **InvalidConversionRate**: Conversion rate is invalid

## Usage Example

```rust
// Create invoice
let invoice_id = create_invoice(
    env,
    merchant,
    customer,
    1_000_000,  // 1 USDC
    usdc_token,
    vec![usdc_token, usdt_token, eur_token],
    usdc_token,
    line_items,
    due_date,
    metadata,
);

// Set conversion rates
set_conversion_rate(env, merchant, usdt_token, 1_000_000);  // 1:1
set_conversion_rate(env, merchant, eur_token, 1_100_000);   // 1.1:1

// Accept payment in USDT
let payment = accept_payment(
    env,
    invoice_id,
    payer,
    usdt_token,
    1_000_000,
);

// Settle invoices
let batch = settle_invoices(
    env,
    merchant,
    vec![invoice_id],
);
```

## Security Considerations

1. **Authorization**: Only merchants can create and manage their invoices
2. **Conversion Rates**: Rates must be set before accepting payments
3. **Payment Validation**: Payments are validated against invoice state
4. **Batch Settlement**: Only fully paid invoices can be settled
5. **Storage TTL**: Persistent storage for long-term record keeping

## Future Enhancements

1. Partial payment tracking with payment schedules
2. Automatic conversion rate updates from oracle
3. Invoice templates for recurring invoices
4. Payment reminders and notifications
5. Dispute resolution mechanism
6. Multi-signature approval for large invoices
7. Invoice factoring support
