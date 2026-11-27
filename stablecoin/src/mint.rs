use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_system_interface::instruction as system_instruction;
use solana_zk_sdk::encryption::pod::elgamal::PodElGamalPubkey;
use spl_pod::optional_keys::OptionalNonZeroPubkey;
use spl_token_2022::{
    extension::{
        confidential_transfer, confidential_transfer_fee, default_account_state, metadata_pointer,
        transfer_fee, ExtensionType,
    },
    instruction as token_instruction,
    state::{AccountState, Mint},
};
use spl_token_metadata_interface::state::TokenMetadata;
use spl_type_length_value::variable_len_pack::VariableLenPack;

pub struct RemittanceMintParams<'a> {
    pub payer: &'a Pubkey,
    pub mint: &'a Pubkey,
    pub authority: &'a Pubkey,
    pub decimals: u8,
    pub transfer_fee_bps: u16,
    pub max_fee: u64,
    pub name: String,
    pub symbol: String,
    pub uri: String,
}

pub struct ConfidentialMintParams<'a> {
    pub base: RemittanceMintParams<'a>,
    pub permanent_delegate: &'a Pubkey,
    pub auditor_elgamal_pubkey: Option<PodElGamalPubkey>,
    pub withdraw_withheld_authority_elgamal_pubkey: PodElGamalPubkey,
}

fn metadata_rent_len(mint: &Pubkey, authority: &Pubkey, name: &str, symbol: &str, uri: &str) -> Result<usize, ProgramError> {
    let metadata = TokenMetadata {
        update_authority: OptionalNonZeroPubkey::try_from(Some(*authority))?,
        mint: *mint,
        name: name.to_string(),
        symbol: symbol.to_string(),
        uri: uri.to_string(),
        additional_metadata: vec![],
    };
    let packed_len = metadata.get_packed_len()?;
    Ok(packed_len + 4)
}

pub fn create_remittance_mint_instructions(
    params: &RemittanceMintParams,
    rent: &Rent,
) -> Result<Vec<Instruction>, ProgramError> {
    let token_program_id = spl_token_2022::id();
    let extension_types = [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
    ];
    let mint_len = ExtensionType::try_calculate_account_len::<Mint>(&extension_types)?;
    let extra_meta_len = metadata_rent_len(
        params.mint,
        params.authority,
        &params.name,
        &params.symbol,
        &params.uri,
    )?;
    let lamports = rent.minimum_balance(mint_len + extra_meta_len);

    Ok(vec![
        system_instruction::create_account(
            params.payer,
            params.mint,
            lamports,
            mint_len as u64,
            &token_program_id,
        ),
        transfer_fee::instruction::initialize_transfer_fee_config(
            &token_program_id,
            params.mint,
            Some(params.authority),
            Some(params.authority),
            params.transfer_fee_bps,
            params.max_fee,
        )?,
        metadata_pointer::instruction::initialize(
            &token_program_id,
            params.mint,
            Some(*params.authority),
            Some(*params.mint),
        )?,
        default_account_state::instruction::initialize_default_account_state(
            &token_program_id,
            params.mint,
            &AccountState::Frozen,
        )?,
        token_instruction::initialize_mint_close_authority(
            &token_program_id,
            params.mint,
            Some(params.authority),
        )?,
        token_instruction::initialize_mint2(
            &token_program_id,
            params.mint,
            params.authority,
            Some(params.authority),
            params.decimals,
        )?,
        spl_token_metadata_interface::instruction::initialize(
            &token_program_id,
            params.mint,
            params.authority,
            params.mint,
            params.authority,
            params.name.clone(),
            params.symbol.clone(),
            params.uri.clone(),
        ),
    ])
}

pub fn reissue_confidential_mint_instructions(
    params: &ConfidentialMintParams,
    rent: &Rent,
) -> Result<Vec<Instruction>, ProgramError> {
    let token_program_id = spl_token_2022::id();
    let extension_types = [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
        ExtensionType::PermanentDelegate,
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::ConfidentialTransferFeeConfig,
    ];
    let mint_len = ExtensionType::try_calculate_account_len::<Mint>(&extension_types)?;
    let extra_meta_len = metadata_rent_len(
        params.base.mint,
        params.base.authority,
        &params.base.name,
        &params.base.symbol,
        &params.base.uri,
    )?;
    let lamports = rent.minimum_balance(mint_len + extra_meta_len);

    Ok(vec![
        system_instruction::create_account(
            params.base.payer,
            params.base.mint,
            lamports,
            mint_len as u64,
            &token_program_id,
        ),
        transfer_fee::instruction::initialize_transfer_fee_config(
            &token_program_id,
            params.base.mint,
            Some(params.base.authority),
            Some(params.base.authority),
            params.base.transfer_fee_bps,
            params.base.max_fee,
        )?,
        metadata_pointer::instruction::initialize(
            &token_program_id,
            params.base.mint,
            Some(*params.base.authority),
            Some(*params.base.mint),
        )?,
        default_account_state::instruction::initialize_default_account_state(
            &token_program_id,
            params.base.mint,
            &AccountState::Frozen,
        )?,
        token_instruction::initialize_mint_close_authority(
            &token_program_id,
            params.base.mint,
            Some(params.base.authority),
        )?,
        token_instruction::initialize_permanent_delegate(
            &token_program_id,
            params.base.mint,
            params.permanent_delegate,
        )?,
        confidential_transfer::instruction::initialize_mint(
            &token_program_id,
            params.base.mint,
            Some(*params.base.authority),
            false,
            params.auditor_elgamal_pubkey,
        )?,
        confidential_transfer_fee::instruction::initialize_confidential_transfer_fee_config(
            &token_program_id,
            params.base.mint,
            Some(*params.base.authority),
            &params.withdraw_withheld_authority_elgamal_pubkey,
        )?,
        token_instruction::initialize_mint2(
            &token_program_id,
            params.base.mint,
            params.base.authority,
            Some(params.base.authority),
            params.base.decimals,
        )?,
        spl_token_metadata_interface::instruction::initialize(
            &token_program_id,
            params.base.mint,
            params.base.authority,
            params.base.mint,
            params.base.authority,
            params.base.name.clone(),
            params.base.symbol.clone(),
            params.base.uri.clone(),
        ),
    ])
}
