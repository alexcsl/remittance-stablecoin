use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_system_interface::instruction as system_instruction;
use solana_zk_sdk::{
    encryption::{
        auth_encryption::AeKey,
        elgamal::{ElGamalKeypair, ElGamalPubkey},
        pod::elgamal::PodElGamalPubkey,
    },
    zk_elgamal_proof_program::{
        self,
        instruction::{close_context_state, ContextStateInfo, ProofInstruction},
        proof_data::{
            BatchedGroupedCiphertext2HandlesValidityProofData,
            BatchedGroupedCiphertext3HandlesValidityProofData, BatchedRangeProofU256Data,
            BatchedRangeProofU64Data, CiphertextCommitmentEqualityProofData,
            PercentageWithCapProofData, PubkeyValidityProofData, ZkProofData,
        },
        state::ProofContextState,
    },
};
use spl_token_2022::{
    extension::{
        confidential_transfer::{
            self,
            account_info::{
                ApplyPendingBalanceAccountInfo, TransferAccountInfo, WithdrawAccountInfo,
            },
            ConfidentialTransferAccount, ConfidentialTransferMint,
        },
        confidential_transfer_fee::ConfidentialTransferFeeConfig,
        transfer_fee::TransferFeeConfig,
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction as token_instruction,
    state::{Account, Mint},
};
use spl_token_confidential_transfer_proof_extraction::instruction::{ProofData, ProofLocation};

pub fn configure_confidential_account_instructions(
    token_account: &Pubkey,
    mint: &Pubkey,
    payer: &Pubkey,
    owner: &Pubkey,
    elgamal_keypair: &ElGamalKeypair,
    aes_key: &AeKey,
    maximum_pending_balance_credit_counter: u64,
) -> Result<Vec<Instruction>, ProgramError> {
    let token_program_id = spl_token_2022::id();
    let decryptable_zero_balance = aes_key.encrypt(0).into();
    let proof_data = PubkeyValidityProofData::new(elgamal_keypair)
        .map_err(|_| ProgramError::InvalidArgument)?;

    let mut ixs = vec![token_instruction::reallocate(
        &token_program_id,
        token_account,
        payer,
        owner,
        &[],
        &[
            ExtensionType::ConfidentialTransferAccount,
            ExtensionType::ConfidentialTransferFeeAmount,
        ],
    )?];

    ixs.extend(confidential_transfer::instruction::configure_account(
        &token_program_id,
        token_account,
        mint,
        &decryptable_zero_balance,
        maximum_pending_balance_credit_counter,
        owner,
        &[],
        ProofLocation::InstructionOffset(
            1.try_into().unwrap(),
            ProofData::InstructionData(&proof_data),
        ),
    )?);

    Ok(ixs)
}

pub fn approve_confidential_account_instruction(
    token_account: &Pubkey,
    mint: &Pubkey,
    authority: &Pubkey,
) -> Result<Instruction, ProgramError> {
    confidential_transfer::instruction::approve_account(
        &spl_token_2022::id(),
        token_account,
        mint,
        authority,
        &[],
    )
}

pub fn deposit_confidential_instruction(
    token_account: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Result<Instruction, ProgramError> {
    confidential_transfer::instruction::deposit(
        &spl_token_2022::id(),
        token_account,
        mint,
        amount,
        decimals,
        owner,
        &[],
    )
}

pub fn apply_pending_balance_instruction(
    token_account: &Pubkey,
    account_data: &[u8],
    owner: &Pubkey,
    elgamal_keypair: &ElGamalKeypair,
    aes_key: &AeKey,
) -> Result<Instruction, ProgramError> {
    let account_state = StateWithExtensions::<Account>::unpack(account_data)?;
    let ct_account = account_state.get_extension::<ConfidentialTransferAccount>()?;
    let info = ApplyPendingBalanceAccountInfo::new(ct_account);
    let new_decryptable = info
        .new_decryptable_available_balance(elgamal_keypair.secret(), aes_key)
        .map_err(|e| ProgramError::Custom(e as u32))?;

    confidential_transfer::instruction::apply_pending_balance(
        &spl_token_2022::id(),
        token_account,
        info.pending_balance_credit_counter(),
        &new_decryptable.into(),
        owner,
        &[],
    )
}

pub fn create_proof_context_instructions<T: bytemuck::Pod + ZkProofData<U>, U: bytemuck::Pod>(
    payer: &Pubkey,
    context_state_account: &Pubkey,
    context_state_authority: &Pubkey,
    proof_instruction: ProofInstruction,
    proof_data: &T,
    rent: &Rent,
) -> Vec<Instruction> {
    let space = std::mem::size_of::<ProofContextState<U>>();
    let lamports = rent.minimum_balance(space);
    let context_info = ContextStateInfo {
        context_state_account,
        context_state_authority,
    };
    vec![
        ComputeBudgetInstruction::set_compute_unit_limit(400_000),
        system_instruction::create_account(
            payer,
            context_state_account,
            lamports,
            space as u64,
            &zk_elgamal_proof_program::id(),
        ),
        proof_instruction.encode_verify_proof(Some(context_info), proof_data),
    ]
}

pub fn close_proof_context_instruction(
    context_state_account: &Pubkey,
    destination: &Pubkey,
    context_state_authority: &Pubkey,
) -> Instruction {
    close_context_state(
        ContextStateInfo {
            context_state_account,
            context_state_authority,
        },
        destination,
    )
}

pub struct ConfidentialTransferContextAccounts {
    pub equality: Pubkey,
    pub ciphertext_validity: Pubkey,
    pub fee_sigma: Pubkey,
    pub fee_ciphertext_validity: Pubkey,
    pub range: Pubkey,
}

pub struct PreparedConfidentialTransfer {
    pub create_proof_batches: Vec<Vec<Instruction>>,
    pub transfer_ixs: Vec<Instruction>,
    pub close_proof_ixs: Vec<Instruction>,
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_confidential_transfer_with_fee(
    payer: &Pubkey,
    source_account: &Pubkey,
    source_data: &[u8],
    destination_account: &Pubkey,
    destination_data: &[u8],
    mint: &Pubkey,
    mint_data: &[u8],
    current_epoch: u64,
    owner: &Pubkey,
    source_elgamal: &ElGamalKeypair,
    source_aes: &AeKey,
    amount: u64,
    ctx_accounts: &ConfidentialTransferContextAccounts,
    rent: &Rent,
) -> Result<PreparedConfidentialTransfer, ProgramError> {
    let mint_state = StateWithExtensions::<Mint>::unpack(mint_data)?;
    let ct_mint = mint_state.get_extension::<ConfidentialTransferMint>()?;
    let ct_fee = mint_state.get_extension::<ConfidentialTransferFeeConfig>()?;
    let fee_config = mint_state.get_extension::<TransferFeeConfig>()?;
    let epoch_fee = fee_config.get_epoch_fee(current_epoch);

    let auditor_elgamal: Option<ElGamalPubkey> =
        Option::<PodElGamalPubkey>::from(ct_mint.auditor_elgamal_pubkey)
            .map(|pod| pod.try_into().map_err(|_| ProgramError::InvalidAccountData))
            .transpose()?;
    let withdraw_withheld_elgamal: ElGamalPubkey = ct_fee
        .withdraw_withheld_authority_elgamal_pubkey
        .try_into()
        .map_err(|_| ProgramError::InvalidAccountData)?;

    let source_state = StateWithExtensions::<Account>::unpack(source_data)?;
    let source_ct = source_state.get_extension::<ConfidentialTransferAccount>()?;
    let dest_state = StateWithExtensions::<Account>::unpack(destination_data)?;
    let dest_ct = dest_state.get_extension::<ConfidentialTransferAccount>()?;
    let dest_elgamal: ElGamalPubkey = dest_ct
        .elgamal_pubkey
        .try_into()
        .map_err(|_| ProgramError::InvalidAccountData)?;

    let transfer_info = TransferAccountInfo::new(source_ct);
    let proof_data = transfer_info
        .generate_split_transfer_with_fee_proof_data(
            amount,
            source_elgamal,
            source_aes,
            &dest_elgamal,
            auditor_elgamal.as_ref(),
            &withdraw_withheld_elgamal,
            u16::from(epoch_fee.transfer_fee_basis_points),
            u64::from(epoch_fee.maximum_fee),
        )
        .map_err(|e| ProgramError::Custom(e as u32))?;

    let new_source_decryptable = transfer_info
        .new_decryptable_available_balance(amount, source_aes)
        .map_err(|e| ProgramError::Custom(e as u32))?;

    let create_proof_batches = vec![
        create_proof_context_instructions::<CiphertextCommitmentEqualityProofData, _>(
            payer,
            &ctx_accounts.equality,
            owner,
            ProofInstruction::VerifyCiphertextCommitmentEquality,
            &proof_data.equality_proof_data,
            rent,
        ),
        create_proof_context_instructions::<BatchedGroupedCiphertext3HandlesValidityProofData, _>(
            payer,
            &ctx_accounts.ciphertext_validity,
            owner,
            ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity,
            &proof_data
                .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
                .proof_data,
            rent,
        ),
        create_proof_context_instructions::<PercentageWithCapProofData, _>(
            payer,
            &ctx_accounts.fee_sigma,
            owner,
            ProofInstruction::VerifyPercentageWithCap,
            &proof_data.percentage_with_cap_proof_data,
            rent,
        ),
        create_proof_context_instructions::<BatchedGroupedCiphertext2HandlesValidityProofData, _>(
            payer,
            &ctx_accounts.fee_ciphertext_validity,
            owner,
            ProofInstruction::VerifyBatchedGroupedCiphertext2HandlesValidity,
            &proof_data.fee_ciphertext_validity_proof_data,
            rent,
        ),
        create_proof_context_instructions::<BatchedRangeProofU256Data, _>(
            payer,
            &ctx_accounts.range,
            owner,
            ProofInstruction::VerifyBatchedRangeProofU256,
            &proof_data.range_proof_data,
            rent,
        ),
    ];

    let transfer_ixs = confidential_transfer::instruction::transfer_with_fee(
        &spl_token_2022::id(),
        source_account,
        mint,
        destination_account,
        &new_source_decryptable.into(),
        &proof_data
            .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
            .ciphertext_lo,
        &proof_data
            .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
            .ciphertext_hi,
        owner,
        &[],
        ProofLocation::ContextStateAccount(&ctx_accounts.equality),
        ProofLocation::ContextStateAccount(&ctx_accounts.ciphertext_validity),
        ProofLocation::ContextStateAccount(&ctx_accounts.fee_sigma),
        ProofLocation::ContextStateAccount(&ctx_accounts.fee_ciphertext_validity),
        ProofLocation::ContextStateAccount(&ctx_accounts.range),
    )?;

    let close_proof_ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(20_000),
        close_proof_context_instruction(&ctx_accounts.equality, payer, owner),
        close_proof_context_instruction(&ctx_accounts.ciphertext_validity, payer, owner),
        close_proof_context_instruction(&ctx_accounts.fee_sigma, payer, owner),
        close_proof_context_instruction(&ctx_accounts.fee_ciphertext_validity, payer, owner),
        close_proof_context_instruction(&ctx_accounts.range, payer, owner),
    ];

    Ok(PreparedConfidentialTransfer {
        create_proof_batches,
        transfer_ixs,
        close_proof_ixs,
    })
}

pub struct PreparedConfidentialWithdraw {
    pub create_proof_batches: Vec<Vec<Instruction>>,
    pub withdraw_ixs: Vec<Instruction>,
    pub close_proof_ixs: Vec<Instruction>,
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_confidential_withdraw(
    payer: &Pubkey,
    token_account: &Pubkey,
    account_data: &[u8],
    mint: &Pubkey,
    decimals: u8,
    owner: &Pubkey,
    elgamal_keypair: &ElGamalKeypair,
    aes_key: &AeKey,
    amount: u64,
    equality_ctx: &Pubkey,
    range_ctx: &Pubkey,
    rent: &Rent,
) -> Result<PreparedConfidentialWithdraw, ProgramError> {
    let account_state = StateWithExtensions::<Account>::unpack(account_data)?;
    let ct_account = account_state.get_extension::<ConfidentialTransferAccount>()?;
    let withdraw_info = WithdrawAccountInfo::new(ct_account);

    let proof_data = withdraw_info
        .generate_proof_data(amount, elgamal_keypair, aes_key)
        .map_err(|e| ProgramError::Custom(e as u32))?;
    let new_decryptable = withdraw_info
        .new_decryptable_available_balance(amount, aes_key)
        .map_err(|e| ProgramError::Custom(e as u32))?;

    let create_proof_batches = vec![
        create_proof_context_instructions::<CiphertextCommitmentEqualityProofData, _>(
            payer,
            equality_ctx,
            owner,
            ProofInstruction::VerifyCiphertextCommitmentEquality,
            &proof_data.equality_proof_data,
            rent,
        ),
        create_proof_context_instructions::<BatchedRangeProofU64Data, _>(
            payer,
            range_ctx,
            owner,
            ProofInstruction::VerifyBatchedRangeProofU64,
            &proof_data.range_proof_data,
            rent,
        ),
    ];

    let withdraw_ixs = confidential_transfer::instruction::withdraw(
        &spl_token_2022::id(),
        token_account,
        mint,
        amount,
        decimals,
        &new_decryptable.into(),
        owner,
        &[],
        ProofLocation::ContextStateAccount(equality_ctx),
        ProofLocation::ContextStateAccount(range_ctx),
    )?;

    let close_proof_ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(10_000),
        close_proof_context_instruction(equality_ctx, payer, owner),
        close_proof_context_instruction(range_ctx, payer, owner),
    ];

    Ok(PreparedConfidentialWithdraw {
        create_proof_batches,
        withdraw_ixs,
        close_proof_ixs,
    })
}
