# Remittance Stablecoin (SPL Token-2022)

Rust implementation and integration test suite for a Token-2022 remittance stablecoin covering stacked mint extensions, epoch-based transfer fees, per-account KYC thawing, confidential transfers with manual approval, permanent delegate seizure, and a minimal on-chain delegated transfer program verified under CPI Guard.

## Workspace Layout

- `stablecoin/src/mint.rs`:
  - `create_remittance_mint_instructions`: Creates a mint stacking `TransferFeeConfig`, `MetadataPointer` (pointing to the mint itself), `DefaultAccountState::Frozen`, and `MintCloseAuthority`. Sizes the account with `ExtensionType::try_calculate_account_len::<Mint>`, orders all extension initializations before `initialize_mint2`, and initializes on-chain `TokenMetadata` afterward.
  - `reissue_confidential_mint_instructions`: Re-issues the mint with the same extension set plus `PermanentDelegate`, `ConfidentialTransferMint` (`auto_approve_new_accounts = false`), and `ConfidentialTransferFeeConfig`.
- `stablecoin/src/transfer.rs`:
  - `transfer_with_epoch_fee`: Reads `TransferFeeConfig` via `StateWithExtensions::<Mint>::unpack`, computes the fee with `calculate_epoch_fee(current_epoch, amount)`, and builds `transfer_checked_with_fee`.
  - `thaw_after_kyc`: Thaws an individual token account using the freeze authority without modifying the mint's `DefaultAccountState`.
- `stablecoin/src/state.rs`: Reads mint and token account state exclusively through `StateWithExtensions`.
- `stablecoin/src/confidential.rs`: Full confidential transfer lifecycle (`ConfigureAccount` with owner-only `reallocate`, `ApproveAccount`, `Deposit`, `ApplyPendingBalance`, `TransferWithFee` using ZK proof context state accounts, and `Withdraw`).
- `programs/delegated-transfer/src/lib.rs` computes the epoch fee and calls Token-2022 via CPI. The delegate PDA is derived from the source and destination token accounts.
- `stablecoin/tests/integration.rs`: End-to-end `solana-program-test` suite exercising all tasks, the permanent delegate vs confidential balance gap, and the CPI Guard delegated-transfer extension challenge.

## Written Finding: Permanent Delegate vs. Confidential Balances

### What happens if a sanctioned user moves their balance into the confidential system before the permanent delegate acts?

The permanent delegate can seize any remaining public balance. Deposited funds sit in the encrypted pending balance and remain beyond the delegate's reach after the owner applies them to the available balance.

In Token-2022, a token account configured for confidential transfers maintains two separate balance domains:

1. **Public balance** (`Account.amount`): Plaintext `u64` balance.
2. **Confidential balance** (`ConfidentialTransferAccount.pending_balance` and `available_balance`): Twisted ElGamal ciphertexts encrypted under the token account owner's ElGamal public key, paired with an AES-encrypted `decryptable_available_balance`.

`PermanentDelegate` authorizes `Transfer`, `TransferChecked`, `TransferCheckedWithFee`, and `Burn` against the public `Account.amount` without requiring the account owner's signature. When a sanctioned user calls `DepositConfidentialTokens`, their public `Account.amount` is debited down to `0` and the equivalent value is credited into the encrypted `pending_balance` (and then moved to `available_balance` via `ApplyPendingBalance`).

Once funds are in the confidential balance:

- Any public `transfer_checked_with_fee` or `burn` invoked by the `PermanentDelegate` fails with insufficient funds because `Account.amount` is `0`.
- Confidential transfers and withdrawals require the token account owner's authorization and proofs about the encrypted balance. The permanent delegate extension grants neither confidential spending authority nor access to the owner's keys.
- Configuring an `auditor_elgamal_pubkey` on `ConfidentialTransferMint` does not close this gap. The auditor key only allows decrypting individual transfer ciphertexts for visibility; it cannot spend funds, withdraw them back to public balance, or authorize a seizure.

### Practical Implications and Mitigations

Adding both `PermanentDelegate` and `ConfidentialTransferMint` to the same mint creates an enforcement gap: seizure only works on public balances. Once a user enters the confidential balance, the issuer's remaining on-chain control is containment rather than recovery:

- **Freeze the account** (`FreezeAccount`): Blocks further confidential transfers, deposits, and withdrawals, locking the funds in place even though they cannot be moved to a seizure address.
- **Gate confidential entry** (`auto_approve_new_accounts = false`): Manual approval (`ApproveAccount`) ensures accounts cannot deposit into or receive confidential balances until KYC and sanctions screening complete.
- **Limit confidential eligibility** to accounts where freezing is an acceptable enforcement option. Keep settlement public where the issuer must be able to move the full balance under the permanent delegate authority.

This behavior is directly demonstrated in `test_reissued_confidential_lifecycle_and_seizure_gap` in `stablecoin/tests/integration.rs`, where the permanent delegate fails to seize Alice's confidential balance after deposit (`Account.amount == 0`) while succeeding against Bob's withdrawn public balance.

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
