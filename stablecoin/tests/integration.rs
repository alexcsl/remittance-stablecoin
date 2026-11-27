use remittance_stablecoin::{
    confidential::{
        apply_pending_balance_instruction, approve_confidential_account_instruction,
        configure_confidential_account_instructions, deposit_confidential_instruction,
        prepare_confidential_transfer_with_fee, prepare_confidential_withdraw,
        ConfidentialTransferContextAccounts,
    },
    mint::{
        create_remittance_mint_instructions, reissue_confidential_mint_instructions,
        ConfidentialMintParams, RemittanceMintParams,
    },
    state::{read_account, read_mint},
    transfer::{thaw_after_kyc, transfer_with_epoch_fee},
};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_program_test::{processor, BanksClient, ProgramTest};
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_zk_sdk::encryption::{auth_encryption::AeKey, elgamal::ElGamalKeypair};
use spl_associated_token_account::{
    get_associated_token_address_with_program_id,
    instruction::create_associated_token_account,
};
use spl_token_2022::{
    error::TokenError,
    extension::{cpi_guard, ExtensionType},
    instruction as token_instruction,
    state::AccountState,
};

async fn send_tx(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    instructions: &[Instruction],
    signers: &[&Keypair],
) -> Result<(), solana_program_test::BanksClientError> {
    let blockhash = banks_client.get_latest_blockhash().await?;
    let mut all_signers = vec![payer];
    for s in signers {
        if s.pubkey() != payer.pubkey() {
            all_signers.push(*s);
        }
    }
    let tx = Transaction::new_signed_with_payer(
        instructions,
        Some(&payer.pubkey()),
        &all_signers,
        blockhash,
    );
    banks_client.process_transaction(tx).await
}

fn new_program_test(delegated_program_id: Option<Pubkey>) -> ProgramTest {
    let mut pt = ProgramTest::default();
    if let Some(pid) = delegated_program_id {
        pt.add_program(
            "delegated_transfer",
            pid,
            processor!(delegated_transfer::process_instruction),
        );
    }
    pt
}

#[tokio::test]
async fn test_remittance_mint_kyc_transfer_and_close() {
    let mut ctx = new_program_test(None).start_with_context().await;
    let rent: Rent = ctx.banks_client.get_sysvar().await.unwrap();
    let authority = Keypair::new();
    let mint = Keypair::new();
    let alice = Keypair::new();
    let bob = Keypair::new();
    let charlie = Keypair::new();

    let params = RemittanceMintParams {
        payer: &ctx.payer.pubkey(),
        mint: &mint.pubkey(),
        authority: &authority.pubkey(),
        decimals: 6,
        transfer_fee_bps: 50,
        max_fee: 5_000_000,
        name: "RemitUSD".to_string(),
        symbol: "RUSD".to_string(),
        uri: "https://remit.example/metadata.json".to_string(),
    };
    let ixs = create_remittance_mint_instructions(&params, &rent).unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &ixs, &[&mint, &authority])
        .await
        .unwrap();

    let mint_account = ctx.banks_client.get_account(mint.pubkey()).await.unwrap().unwrap();
    let snapshot = read_mint(&mint_account.data).unwrap();
    assert_eq!(snapshot.decimals, 6);
    assert_eq!(snapshot.default_account_state, AccountState::Frozen);
    assert_eq!(snapshot.close_authority, Some(authority.pubkey()));
    assert_eq!(
        Option::<Pubkey>::from(snapshot.metadata_pointer.metadata_address),
        Some(mint.pubkey())
    );
    assert_eq!(snapshot.metadata.name, "RemitUSD");
    assert_eq!(snapshot.metadata.symbol, "RUSD");

    let alice_ata = get_associated_token_address_with_program_id(&alice.pubkey(), &mint.pubkey(), &spl_token_2022::id());
    let bob_ata = get_associated_token_address_with_program_id(&bob.pubkey(), &mint.pubkey(), &spl_token_2022::id());

    send_tx(
        &mut ctx.banks_client,
        &ctx.payer,
        &[
            create_associated_token_account(&ctx.payer.pubkey(), &alice.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
            create_associated_token_account(&ctx.payer.pubkey(), &bob.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
        ],
        &[],
    )
    .await
    .unwrap();

    let alice_data = ctx.banks_client.get_account(alice_ata).await.unwrap().unwrap().data;
    assert_eq!(read_account(&alice_data).unwrap().state, AccountState::Frozen);

    send_tx(
        &mut ctx.banks_client,
        &ctx.payer,
        &[
            thaw_after_kyc(&alice_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
            thaw_after_kyc(&bob_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
        ],
        &[&authority],
    )
    .await
    .unwrap();

    let charlie_ata = get_associated_token_address_with_program_id(&charlie.pubkey(), &mint.pubkey(), &spl_token_2022::id());
    send_tx(
        &mut ctx.banks_client,
        &ctx.payer,
        &[create_associated_token_account(
            &ctx.payer.pubkey(),
            &charlie.pubkey(),
            &mint.pubkey(),
            &spl_token_2022::id(),
        )],
        &[],
    )
    .await
    .unwrap();
    let charlie_data = ctx.banks_client.get_account(charlie_ata).await.unwrap().unwrap().data;
    assert_eq!(read_account(&charlie_data).unwrap().state, AccountState::Frozen);

    let mint_to_ix = token_instruction::mint_to_checked(
        &spl_token_2022::id(),
        &mint.pubkey(),
        &alice_ata,
        &authority.pubkey(),
        &[],
        10_000_000,
        6,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[mint_to_ix], &[&authority])
        .await
        .unwrap();

    let clock: solana_sdk::clock::Clock = ctx.banks_client.get_sysvar().await.unwrap();
    let fresh_mint = ctx.banks_client.get_account(mint.pubkey()).await.unwrap().unwrap();
    let (transfer_ix, expected_fee) = transfer_with_epoch_fee(
        &fresh_mint.data,
        clock.epoch,
        &alice_ata,
        &mint.pubkey(),
        &bob_ata,
        &alice.pubkey(),
        2_000_000,
    )
    .unwrap();
    assert_eq!(expected_fee, 10_000);

    send_tx(&mut ctx.banks_client, &ctx.payer, &[transfer_ix], &[&alice])
        .await
        .unwrap();

    let bob_acc = read_account(&ctx.banks_client.get_account(bob_ata).await.unwrap().unwrap().data).unwrap();
    assert_eq!(bob_acc.amount, 2_000_000 - expected_fee);
    assert_eq!(bob_acc.withheld_fee, expected_fee);

    let close_mint = Keypair::new();
    let close_params = RemittanceMintParams {
        payer: &ctx.payer.pubkey(),
        mint: &close_mint.pubkey(),
        authority: &authority.pubkey(),
        decimals: 6,
        transfer_fee_bps: 50,
        max_fee: 5_000_000,
        name: "TempUSD".to_string(),
        symbol: "TUSD".to_string(),
        uri: "https://remit.example/temp.json".to_string(),
    };
    let close_setup_ixs = create_remittance_mint_instructions(&close_params, &rent).unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &close_setup_ixs, &[&close_mint, &authority])
        .await
        .unwrap();

    let close_ix = token_instruction::close_account(
        &spl_token_2022::id(),
        &close_mint.pubkey(),
        &ctx.payer.pubkey(),
        &authority.pubkey(),
        &[],
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[close_ix], &[&authority])
        .await
        .unwrap();
    assert!(ctx.banks_client.get_account(close_mint.pubkey()).await.unwrap().is_none());
}

#[tokio::test]
async fn test_reissued_confidential_lifecycle_and_seizure_gap() {
    let mut ctx = new_program_test(None).start_with_context().await;
    let rent: Rent = ctx.banks_client.get_sysvar().await.unwrap();
    let authority = Keypair::new();
    let permanent_delegate = Keypair::new();
    let mint = Keypair::new();
    let alice = Keypair::new();
    let bob = Keypair::new();
    let seizure_dest_owner = Keypair::new();

    let auditor_elgamal = ElGamalKeypair::new_rand();
    let fee_authority_elgamal = ElGamalKeypair::new_rand();

    let params = ConfidentialMintParams {
        base: RemittanceMintParams {
            payer: &ctx.payer.pubkey(),
            mint: &mint.pubkey(),
            authority: &authority.pubkey(),
            decimals: 6,
            transfer_fee_bps: 50,
            max_fee: 5_000_000,
            name: "RemitUSD v2".to_string(),
            symbol: "RUSD2".to_string(),
            uri: "https://remit.example/v2.json".to_string(),
        },
        permanent_delegate: &permanent_delegate.pubkey(),
        auditor_elgamal_pubkey: Some((*auditor_elgamal.pubkey()).into()),
        withdraw_withheld_authority_elgamal_pubkey: (*fee_authority_elgamal.pubkey()).into(),
    };

    let init_ixs = reissue_confidential_mint_instructions(&params, &rent).unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &init_ixs, &[&mint, &authority])
        .await
        .unwrap();

    let mint_data = ctx.banks_client.get_account(mint.pubkey()).await.unwrap().unwrap().data;
    let mint_snap = read_mint(&mint_data).unwrap();
    assert_eq!(mint_snap.permanent_delegate, Some(permanent_delegate.pubkey()));
    assert!(!bool::from(mint_snap.confidential_transfer_mint.unwrap().auto_approve_new_accounts));

    let alice_ata = get_associated_token_address_with_program_id(&alice.pubkey(), &mint.pubkey(), &spl_token_2022::id());
    let bob_ata = get_associated_token_address_with_program_id(&bob.pubkey(), &mint.pubkey(), &spl_token_2022::id());
    let seizure_ata = get_associated_token_address_with_program_id(
        &seizure_dest_owner.pubkey(),
        &mint.pubkey(),
        &spl_token_2022::id(),
    );

    send_tx(
        &mut ctx.banks_client,
        &ctx.payer,
        &[
            create_associated_token_account(&ctx.payer.pubkey(), &alice.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
            create_associated_token_account(&ctx.payer.pubkey(), &bob.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
            create_associated_token_account(&ctx.payer.pubkey(), &seizure_dest_owner.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
            thaw_after_kyc(&alice_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
            thaw_after_kyc(&bob_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
            thaw_after_kyc(&seizure_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
        ],
        &[&authority],
    )
    .await
    .unwrap();

    let alice_elgamal = ElGamalKeypair::new_rand();
    let alice_aes = AeKey::new_rand();
    let bob_elgamal = ElGamalKeypair::new_rand();
    let bob_aes = AeKey::new_rand();

    let unauthorized_cfg = configure_confidential_account_instructions(
        &alice_ata,
        &mint.pubkey(),
        &ctx.payer.pubkey(),
        &bob.pubkey(),
        &alice_elgamal,
        &alice_aes,
        65_536,
    )
    .unwrap();
    assert!(send_tx(&mut ctx.banks_client, &ctx.payer, &unauthorized_cfg, &[&bob]).await.is_err());

    let alice_cfg = configure_confidential_account_instructions(
        &alice_ata,
        &mint.pubkey(),
        &ctx.payer.pubkey(),
        &alice.pubkey(),
        &alice_elgamal,
        &alice_aes,
        65_536,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &alice_cfg, &[&alice])
        .await
        .unwrap();

    let bob_cfg = configure_confidential_account_instructions(
        &bob_ata,
        &mint.pubkey(),
        &ctx.payer.pubkey(),
        &bob.pubkey(),
        &bob_elgamal,
        &bob_aes,
        65_536,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &bob_cfg, &[&bob])
        .await
        .unwrap();

    let mint_to_ix = token_instruction::mint_to_checked(
        &spl_token_2022::id(),
        &mint.pubkey(),
        &alice_ata,
        &authority.pubkey(),
        &[],
        1_000_000,
        6,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[mint_to_ix], &[&authority])
        .await
        .unwrap();

    let unapproved_deposit = deposit_confidential_instruction(&alice_ata, &mint.pubkey(), &alice.pubkey(), 1_000_000, 6).unwrap();
    assert!(send_tx(&mut ctx.banks_client, &ctx.payer, &[unapproved_deposit], &[&alice]).await.is_err());

    send_tx(
        &mut ctx.banks_client,
        &ctx.payer,
        &[
            approve_confidential_account_instruction(&alice_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
            approve_confidential_account_instruction(&bob_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
        ],
        &[&authority],
    )
    .await
    .unwrap();

    let deposit_ix = deposit_confidential_instruction(&alice_ata, &mint.pubkey(), &alice.pubkey(), 1_000_000, 6).unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[deposit_ix], &[&alice])
        .await
        .unwrap();

    let alice_after_deposit = ctx.banks_client.get_account(alice_ata).await.unwrap().unwrap().data;
    assert_eq!(read_account(&alice_after_deposit).unwrap().amount, 0);

    let apply_alice_ix = apply_pending_balance_instruction(
        &alice_ata,
        &alice_after_deposit,
        &alice.pubkey(),
        &alice_elgamal,
        &alice_aes,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[apply_alice_ix], &[&alice])
        .await
        .unwrap();

    let eq_kp = Keypair::new();
    let cv_kp = Keypair::new();
    let fs_kp = Keypair::new();
    let fcv_kp = Keypair::new();
    let rng_kp = Keypair::new();
    let ctx_accounts = ConfidentialTransferContextAccounts {
        equality: eq_kp.pubkey(),
        ciphertext_validity: cv_kp.pubkey(),
        fee_sigma: fs_kp.pubkey(),
        fee_ciphertext_validity: fcv_kp.pubkey(),
        range: rng_kp.pubkey(),
    };

    let clock: solana_sdk::clock::Clock = ctx.banks_client.get_sysvar().await.unwrap();
    let alice_data = ctx.banks_client.get_account(alice_ata).await.unwrap().unwrap().data;
    let bob_data = ctx.banks_client.get_account(bob_ata).await.unwrap().unwrap().data;
    let fresh_mint = ctx.banks_client.get_account(mint.pubkey()).await.unwrap().unwrap().data;

    let prepared_transfer = prepare_confidential_transfer_with_fee(
        &ctx.payer.pubkey(),
        &alice_ata,
        &alice_data,
        &bob_ata,
        &bob_data,
        &mint.pubkey(),
        &fresh_mint,
        clock.epoch,
        &alice.pubkey(),
        &alice_elgamal,
        &alice_aes,
        400_000,
        &ctx_accounts,
        &rent,
    )
    .unwrap();

    let proof_kps = [&eq_kp, &cv_kp, &fs_kp, &fcv_kp, &rng_kp];
    for (batch, kp) in prepared_transfer.create_proof_batches.iter().zip(proof_kps.iter()) {
        send_tx(&mut ctx.banks_client, &ctx.payer, batch, &[kp])
            .await
            .unwrap();
    }
    send_tx(&mut ctx.banks_client, &ctx.payer, &prepared_transfer.transfer_ixs, &[&alice])
        .await
        .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &prepared_transfer.close_proof_ixs, &[&alice])
        .await
        .unwrap();

    let bob_pending_data = ctx.banks_client.get_account(bob_ata).await.unwrap().unwrap().data;
    let apply_bob_ix = apply_pending_balance_instruction(
        &bob_ata,
        &bob_pending_data,
        &bob.pubkey(),
        &bob_elgamal,
        &bob_aes,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[apply_bob_ix], &[&bob])
        .await
        .unwrap();

    let bob_avail_data = ctx.banks_client.get_account(bob_ata).await.unwrap().unwrap().data;
    let w_eq_kp = Keypair::new();
    let w_rng_kp = Keypair::new();
    let prepared_withdraw = prepare_confidential_withdraw(
        &ctx.payer.pubkey(),
        &bob_ata,
        &bob_avail_data,
        &mint.pubkey(),
        6,
        &bob.pubkey(),
        &bob_elgamal,
        &bob_aes,
        300_000,
        &w_eq_kp.pubkey(),
        &w_rng_kp.pubkey(),
        &rent,
    )
    .unwrap();

    for (batch, kp) in prepared_withdraw.create_proof_batches.iter().zip([&w_eq_kp, &w_rng_kp].iter()) {
        send_tx(&mut ctx.banks_client, &ctx.payer, batch, &[kp])
            .await
            .unwrap();
    }
    send_tx(&mut ctx.banks_client, &ctx.payer, &prepared_withdraw.withdraw_ixs, &[&bob])
        .await
        .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &prepared_withdraw.close_proof_ixs, &[&bob])
        .await
        .unwrap();

    let bob_after_withdraw = read_account(&ctx.banks_client.get_account(bob_ata).await.unwrap().unwrap().data).unwrap();
    assert_eq!(bob_after_withdraw.amount, 300_000);

    let (seize_confidential_ix, _) = transfer_with_epoch_fee(
        &fresh_mint,
        clock.epoch,
        &alice_ata,
        &mint.pubkey(),
        &seizure_ata,
        &permanent_delegate.pubkey(),
        100_000,
    )
    .unwrap();
    assert!(
        send_tx(&mut ctx.banks_client, &ctx.payer, &[seize_confidential_ix], &[&permanent_delegate])
            .await
            .is_err()
    );

    let (seize_public_ix, seize_fee) = transfer_with_epoch_fee(
        &fresh_mint,
        clock.epoch,
        &bob_ata,
        &mint.pubkey(),
        &seizure_ata,
        &permanent_delegate.pubkey(),
        100_000,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[seize_public_ix], &[&permanent_delegate])
        .await
        .unwrap();
    let seized = read_account(&ctx.banks_client.get_account(seizure_ata).await.unwrap().unwrap().data).unwrap();
    assert_eq!(seized.amount, 100_000 - seize_fee);
}

#[tokio::test]
async fn test_delegated_transfer_program_with_cpi_guard() {
    let delegated_program_id = Pubkey::new_unique();
    let mut ctx = new_program_test(Some(delegated_program_id)).start_with_context().await;
    let rent: Rent = ctx.banks_client.get_sysvar().await.unwrap();
    let authority = Keypair::new();
    let mint = Keypair::new();
    let alice = Keypair::new();
    let bob = Keypair::new();

    let params = RemittanceMintParams {
        payer: &ctx.payer.pubkey(),
        mint: &mint.pubkey(),
        authority: &authority.pubkey(),
        decimals: 6,
        transfer_fee_bps: 50,
        max_fee: 5_000_000,
        name: "RemitUSD".to_string(),
        symbol: "RUSD".to_string(),
        uri: "https://remit.example/metadata.json".to_string(),
    };
    let init_ixs = create_remittance_mint_instructions(&params, &rent).unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &init_ixs, &[&mint, &authority])
        .await
        .unwrap();

    let alice_ata = get_associated_token_address_with_program_id(&alice.pubkey(), &mint.pubkey(), &spl_token_2022::id());
    let bob_ata = get_associated_token_address_with_program_id(&bob.pubkey(), &mint.pubkey(), &spl_token_2022::id());
    send_tx(
        &mut ctx.banks_client,
        &ctx.payer,
        &[
            create_associated_token_account(&ctx.payer.pubkey(), &alice.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
            create_associated_token_account(&ctx.payer.pubkey(), &bob.pubkey(), &mint.pubkey(), &spl_token_2022::id()),
            thaw_after_kyc(&alice_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
            thaw_after_kyc(&bob_ata, &mint.pubkey(), &authority.pubkey()).unwrap(),
            token_instruction::mint_to_checked(
                &spl_token_2022::id(),
                &mint.pubkey(),
                &alice_ata,
                &authority.pubkey(),
                &[],
                5_000_000,
                6,
            )
            .unwrap(),
        ],
        &[&authority],
    )
    .await
    .unwrap();

    let (delegate_pda, _) = Pubkey::find_program_address(
        &[b"delegate", alice_ata.as_ref(), bob_ata.as_ref()],
        &delegated_program_id,
    );
    let approve_ix = token_instruction::approve_checked(
        &spl_token_2022::id(),
        &alice_ata,
        &mint.pubkey(),
        &delegate_pda,
        &alice.pubkey(),
        &[],
        2_000_000,
        6,
    )
    .unwrap();
    send_tx(&mut ctx.banks_client, &ctx.payer, &[approve_ix], &[&alice])
        .await
        .unwrap();

    let enable_guard_ixs = vec![
        token_instruction::reallocate(
            &spl_token_2022::id(),
            &alice_ata,
            &ctx.payer.pubkey(),
            &alice.pubkey(),
            &[],
            &[ExtensionType::CpiGuard],
        )
        .unwrap(),
        cpi_guard::instruction::enable_cpi_guard(
            &spl_token_2022::id(),
            &alice_ata,
            &alice.pubkey(),
            &[],
        )
        .unwrap(),
    ];
    send_tx(&mut ctx.banks_client, &ctx.payer, &enable_guard_ixs, &[&alice])
        .await
        .unwrap();

    let alice_snap = read_account(&ctx.banks_client.get_account(alice_ata).await.unwrap().unwrap().data).unwrap();
    assert!(alice_snap.cpi_guard_enabled);

    let owner_cpi_ix = Instruction {
        program_id: delegated_program_id,
        accounts: vec![
            AccountMeta::new(alice_ata, false),
            AccountMeta::new_readonly(mint.pubkey(), false),
            AccountMeta::new(bob_ata, false),
            AccountMeta::new_readonly(alice.pubkey(), true),
            AccountMeta::new_readonly(spl_token_2022::id(), false),
        ],
        data: 1_000_000u64.to_le_bytes().to_vec(),
    };
    let error = send_tx(&mut ctx.banks_client, &ctx.payer, &[owner_cpi_ix], &[&alice])
        .await
        .unwrap_err();
    assert_eq!(
        error.unwrap(),
        solana_sdk::transaction::TransactionError::InstructionError(
            0,
            solana_instruction::error::InstructionError::Custom(
                TokenError::CpiGuardTransferBlocked as u32,
            ),
        ),
    );

    let delegated_cpi_ix = Instruction {
        program_id: delegated_program_id,
        accounts: vec![
            AccountMeta::new(alice_ata, false),
            AccountMeta::new_readonly(mint.pubkey(), false),
            AccountMeta::new(bob_ata, false),
            AccountMeta::new_readonly(delegate_pda, false),
            AccountMeta::new_readonly(spl_token_2022::id(), false),
        ],
        data: 1_000_000u64.to_le_bytes().to_vec(),
    };
    send_tx(&mut ctx.banks_client, &ctx.payer, &[delegated_cpi_ix], &[])
        .await
        .unwrap();

    let bob_snap = read_account(&ctx.banks_client.get_account(bob_ata).await.unwrap().unwrap().data).unwrap();
    assert_eq!(bob_snap.amount, 1_000_000 - 5_000);
    assert_eq!(bob_snap.withheld_fee, 5_000);
}
