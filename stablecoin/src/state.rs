use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spl_token_2022::{
    extension::{
        confidential_transfer::{ConfidentialTransferAccount, ConfidentialTransferMint},
        confidential_transfer_fee::ConfidentialTransferFeeConfig,
        cpi_guard::CpiGuard,
        default_account_state::DefaultAccountState,
        metadata_pointer::MetadataPointer,
        mint_close_authority::MintCloseAuthority,
        permanent_delegate::PermanentDelegate,
        transfer_fee::{TransferFeeAmount, TransferFeeConfig},
        BaseStateWithExtensions, StateWithExtensions,
    },
    state::{Account, AccountState, Mint},
};
use spl_token_metadata_interface::state::TokenMetadata;

pub struct MintSnapshot {
    pub decimals: u8,
    pub supply: u64,
    pub mint_authority: Option<Pubkey>,
    pub freeze_authority: Option<Pubkey>,
    pub transfer_fee_config: TransferFeeConfig,
    pub metadata_pointer: MetadataPointer,
    pub default_account_state: AccountState,
    pub close_authority: Option<Pubkey>,
    pub metadata: TokenMetadata,
    pub permanent_delegate: Option<Pubkey>,
    pub confidential_transfer_mint: Option<ConfidentialTransferMint>,
    pub confidential_transfer_fee_config: Option<ConfidentialTransferFeeConfig>,
}

pub struct AccountSnapshot {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    pub state: AccountState,
    pub withheld_fee: u64,
    pub cpi_guard_enabled: bool,
    pub confidential: Option<ConfidentialTransferAccount>,
}

pub fn read_mint(data: &[u8]) -> Result<MintSnapshot, ProgramError> {
    let state = StateWithExtensions::<Mint>::unpack(data)?;
    let transfer_fee_config = *state.get_extension::<TransferFeeConfig>()?;
    let metadata_pointer = *state.get_extension::<MetadataPointer>()?;
    let default_state_ext = state.get_extension::<DefaultAccountState>()?;
    let default_account_state =
        AccountState::try_from(default_state_ext.state).map_err(|_| ProgramError::InvalidAccountData)?;
    let close_ext = state.get_extension::<MintCloseAuthority>()?;
    let close_authority: Option<Pubkey> = close_ext.close_authority.into();
    let metadata = state.get_variable_len_extension::<TokenMetadata>()?;
    let permanent_delegate = state
        .get_extension::<PermanentDelegate>()
        .ok()
        .and_then(|ext| Option::<Pubkey>::from(ext.delegate));
    let confidential_transfer_mint = state
        .get_extension::<ConfidentialTransferMint>()
        .ok()
        .copied();
    let confidential_transfer_fee_config = state
        .get_extension::<ConfidentialTransferFeeConfig>()
        .ok()
        .copied();

    Ok(MintSnapshot {
        decimals: state.base.decimals,
        supply: state.base.supply,
        mint_authority: state.base.mint_authority.into(),
        freeze_authority: state.base.freeze_authority.into(),
        transfer_fee_config,
        metadata_pointer,
        default_account_state,
        close_authority,
        metadata,
        permanent_delegate,
        confidential_transfer_mint,
        confidential_transfer_fee_config,
    })
}

pub fn read_account(data: &[u8]) -> Result<AccountSnapshot, ProgramError> {
    let state = StateWithExtensions::<Account>::unpack(data)?;
    let withheld_fee = state
        .get_extension::<TransferFeeAmount>()
        .map(|ext| u64::from(ext.withheld_amount))
        .unwrap_or(0);
    let cpi_guard_enabled = state
        .get_extension::<CpiGuard>()
        .map(|ext| bool::from(ext.lock_cpi))
        .unwrap_or(false);
    let confidential = state
        .get_extension::<ConfidentialTransferAccount>()
        .ok()
        .copied();

    Ok(AccountSnapshot {
        mint: state.base.mint,
        owner: state.base.owner,
        amount: state.base.amount,
        state: state.base.state,
        withheld_fee,
        cpi_guard_enabled,
        confidential,
    })
}
