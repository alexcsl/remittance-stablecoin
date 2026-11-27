use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spl_token_2022::{
    extension::{transfer_fee, BaseStateWithExtensions, StateWithExtensions},
    instruction as token_instruction,
    state::Mint,
};

pub fn transfer_with_epoch_fee(
    mint_data: &[u8],
    current_epoch: u64,
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Result<(Instruction, u64), ProgramError> {
    let mint_state = StateWithExtensions::<Mint>::unpack(mint_data)?;
    let fee_config = mint_state.get_extension::<transfer_fee::TransferFeeConfig>()?;
    let fee = fee_config
        .calculate_epoch_fee(current_epoch, amount)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    let ix = transfer_fee::instruction::transfer_checked_with_fee(
        &spl_token_2022::id(),
        source,
        mint,
        destination,
        authority,
        &[],
        amount,
        mint_state.base.decimals,
        fee,
    )?;

    Ok((ix, fee))
}

pub fn thaw_after_kyc(
    token_account: &Pubkey,
    mint: &Pubkey,
    freeze_authority: &Pubkey,
) -> Result<Instruction, ProgramError> {
    token_instruction::thaw_account(
        &spl_token_2022::id(),
        token_account,
        mint,
        freeze_authority,
        &[],
    )
}
