# Remittance Stablecoin (SPL Token-2022)

A Rust workspace for issuing and transferring a Token-2022 remittance stablecoin.

## Mint and transfers

- [Mint](stablecoin/src/mint.rs) sizes the account with `ExtensionType::try_calculate_account_len`. It initializes transfer fees, a metadata pointer to the mint, frozen-by-default accounts, and a close authority before `InitializeMint2`. It writes token metadata after mint initialization.
- [Transfers](stablecoin/src/transfer.rs) use `transfer_checked_with_fee` and compute the expected fee with `calculate_epoch_fee(current_epoch, amount)`. After KYC, the freeze authority thaws the individual account. [State readers](stablecoin/src/state.rs) use `StateWithExtensions`.
- The reissued mint keeps the original extensions and adds `PermanentDelegate`, `ConfidentialTransferMint`, and `ConfidentialTransferFeeConfig`. The token account owner configures confidential transfers after ATA creation. The issuer approves the account manually.
- [Confidential transfers](stablecoin/src/confidential.rs) deposit public tokens into the pending balance, apply pending funds, transfer with encrypted fees, and withdraw back to the public balance. The recipient applies pending funds before withdrawing.

## Seizure after a confidential deposit

The permanent delegate can seize the public balance. A deposit moves the deposited amount out of that balance and into the encrypted pending balance. The funds stay beyond the delegate's reach after the owner applies them to the available balance.

Confidential transfers and withdrawals require the account owner's authorization and proofs about the encrypted balance. The auditor key can reveal transfer amounts and carries no spending authority. The issuer can freeze the token account to block further movement of the funds.

The [integration test](stablecoin/tests/integration.rs) shows the delegate's seizure failing after Alice deposits her full public balance and succeeding against Bob's withdrawn public balance.

## Extension Challenge: Delegated Transfer with CPI Guard

`approve_checked` gives the delegate PDA (`["delegate", source, destination]`) an allowance. Anyone can trigger settlement within that allowance to the approved destination. After the owner enables CPI Guard:

- An owner-signed CPI transfer through the program is rejected by Token-2022 (`CpiGuardTransferBlocked`).
- The delegate-signed CPI transfer path (`invoke_signed` with the approved PDA) continues to succeed unmodified because CPI Guard permits delegated transfers bounded by the user's explicit `Approve` allowance.

## Running Tests

```bash
cargo test --locked --workspace
```

The suite runs locally in `solana-program-test`, which bundles Token-2022 8.0.0. The confidential proof generator is pinned to 0.4.0 to match the proof format that runtime verifies.

To run the same suite with the compiled delegated program loaded:

```bash
cargo build-sbf --manifest-path programs/delegated-transfer/Cargo.toml --tools-version v1.56 --arch v0 -- --locked
SBF_OUT_DIR="$PWD/target/deploy" cargo test --locked --workspace
```
