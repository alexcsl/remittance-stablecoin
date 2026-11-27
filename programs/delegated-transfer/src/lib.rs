use solana_account_info::{next_account_info, AccountInfo};
use solana_clock::Clock;
use solana_program::program::{invoke, invoke_signed};
use solana_program_entrypoint::entrypoint;
use solana_program_error::{ProgramError, ProgramResult};
use solana_pubkey::Pubkey;
use solana_sysvar::Sysvar;
use spl_token_2022::{
    extension::{transfer_fee::TransferFeeConfig, BaseStateWithExtensions, StateWithExtensions},
    state::Mint,
};

entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    if instruction_data.len() != 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let amount = u64::from_le_bytes(instruction_data.try_into().unwrap());

    let account_info_iter = &mut accounts.iter();
    let source = next_account_info(account_info_iter)?;
    let mint = next_account_info(account_info_iter)?;
    let destination = next_account_info(account_info_iter)?;
    let authority = next_account_info(account_info_iter)?;
    let token_program = next_account_info(account_info_iter)?;

    let (decimals, fee) = {
        let mint_data = mint.try_borrow_data()?;
        let mint_state = StateWithExtensions::<Mint>::unpack(&mint_data)?;
        let fee_config = mint_state.get_extension::<TransferFeeConfig>()?;
        let epoch = Clock::get()?.epoch;
        let fee = fee_config
            .calculate_epoch_fee(epoch, amount)
            .ok_or(ProgramError::ArithmeticOverflow)?;
        (mint_state.base.decimals, fee)
    };

    let ix = spl_token_2022::extension::transfer_fee::instruction::transfer_checked_with_fee(
        token_program.key,
        source.key,
        mint.key,
        destination.key,
        authority.key,
        &[],
        amount,
        decimals,
        fee,
    )?;

    let (expected_pda, bump) = Pubkey::find_program_address(
        &[b"delegate", source.key.as_ref(), destination.key.as_ref()],
        program_id,
    );
    if authority.key == &expected_pda {
        let seeds = &[b"delegate".as_ref(), source.key.as_ref(), destination.key.as_ref(), &[bump]];
        invoke_signed(
            &ix,
            &[
                source.clone(),
                mint.clone(),
                destination.clone(),
                authority.clone(),
                token_program.clone(),
            ],
            &[seeds],
        )
    } else {
        invoke(
            &ix,
            &[
                source.clone(),
                mint.clone(),
                destination.clone(),
                authority.clone(),
                token_program.clone(),
            ],
        )
    }
}
