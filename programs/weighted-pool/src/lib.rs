use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_spl::token::{self, Burn, MintTo, Token, Transfer};
use math::{fixed, weighted_math, U256};
use spl_token::state::Account as SplAccount;

// Import the Vault CPI interfaces
// bring in your Vault CPI…
use vault::cpi::{register_pool as vault_register_pool, accounts::RegisterPool as VaultRegisterPool};
// …and the program struct itself
use vault::program::Vault as VaultProgram;
use vault::VaultState;

// ---------------------------------------------------------------------
// Program ID
// ---------------------------------------------------------------------
declare_id!("WPoo1QeY5T2r8j6YfGLwRoTSesFiNUFDXL9uBebzh1e");

#[program]
pub mod weighted_pool {
    use super::*;

    /* ---------------------------------------------------------------
       Initialise a pool
    ---------------------------------------------------------------- */
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        weights: Vec<u128>,
        swap_fee: u64,
    ) -> Result<()> {
        // ensure one vault per weight
        require!(weights.len() == ctx.remaining_accounts.len(), ErrorCode::LengthMismatch);

        // initialize our pool state
        let pool = &mut ctx.accounts.pool;
        pool.vault     = ctx.accounts.vault_state.key();
        pool.lp_mint   = ctx.accounts.lp_mint.key();
        pool.weights   = weights;
        pool.swap_fee  = swap_fee;
        pool.total_bpt = 0;

        // Now register this pool in the Vault program via CPI
        let cpi_program = ctx.accounts.vault_program.to_account_info();
        let cpi_accounts = VaultRegisterPool {
            vault_state:    ctx.accounts.vault_state.to_account_info(),
            owner:          ctx.accounts.payer.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
        };
        vault_register_pool(CpiContext::new(cpi_program, cpi_accounts))?;

        Ok(())
    }

    /* ---------------------------------------------------------------
       Join – deposit all tokens, mint BPT
       remaining_accounts: [user_tok0, vault_tok0, user_tok1, vault_tok1, …]
    ---------------------------------------------------------------- */
    pub fn join_exact_tokens_in_for_bpt_out<'info>(
        ctx: Context<'_, '_, '_, 'info, PoolContext<'info>>,
        amounts_in: Vec<u64>,
    ) -> Result<()> {
        let pool = &ctx.accounts.pool;
        let n    = pool.weights.len();

        require!(ctx.remaining_accounts.len() == n * 2, ErrorCode::LengthMismatch);
        require!(amounts_in.len() == n, ErrorCode::LengthMismatch);

        // 1. read vault balances
        let mut balances_fp = Vec::with_capacity(n);
        for i in 0..n {
            let vault_ai = &ctx.remaining_accounts[i * 2 + 1];
            let data     = vault_ai.try_borrow_data()?;
            let acct     = SplAccount::unpack_from_slice(&data)?;
            balances_fp.push(U256::from(acct.amount) * fixed::ONE);
        }

        // 2. maths
        let weights_fp: Vec<U256> = pool.weights.iter().map(|w| U256::from(*w)).collect();
        let amounts_fp: Vec<U256> = amounts_in.iter().map(|a| U256::from(*a) * fixed::ONE).collect();
        let bpt_out_fp = weighted_math::calc_bpt_out_given_exact_tokens_in(
            &balances_fp,
            &weights_fp,
            &amounts_fp,
            U256::from(pool.total_bpt) * fixed::ONE,
            U256::from(pool.swap_fee),
        );
        require!(bpt_out_fp > U256::zero(), ErrorCode::MathUnderflow);
        let bpt_out = (bpt_out_fp / fixed::ONE).as_u64();

        // 3. CPI transfers (user → vault)
        let token_prog = ctx.accounts.token_program.to_account_info();
        let user_auth  = ctx.accounts.user.to_account_info();
        for i in 0..n {
            let cpi_accounts = Transfer {
                from:      ctx.remaining_accounts[i * 2].clone(),
                to:        ctx.remaining_accounts[i * 2 + 1].clone(),
                authority: user_auth.clone(),
            };
            token::transfer(
                CpiContext::new(token_prog.clone(), cpi_accounts),
                amounts_in[i],
            )?;
        }

        // 4. mint BPT
        let bump         = ctx.bumps.lp_mint_authority;
        let pool_key     = ctx.accounts.pool.key();
        let bump_arr     = [bump];
        let seed_slice: &[&[u8]] = &[
            b"lp-mint-authority",
            pool_key.as_ref(),
            &bump_arr,
        ];
        let signer_seeds = &[seed_slice];
        let mint_ctx = CpiContext::new_with_signer(
            token_prog.clone(),
            MintTo {
                mint:      ctx.accounts.lp_mint.clone(),
                to:        ctx.accounts.user_lp_account.clone(),
                authority: ctx.accounts.lp_mint_authority.clone(),
            },
            signer_seeds,
        );
        token::mint_to(mint_ctx, bpt_out)?;

        // 5. bookkeeping
        ctx.accounts.pool.total_bpt = ctx.accounts.pool
            .total_bpt
            .checked_add(bpt_out)
            .ok_or(ErrorCode::MathUnderflow)?;
        Ok(())
    }

    /* ---------------------------------------------------------------
       Exit – burn BPT, withdraw proportional tokens
    ---------------------------------------------------------------- */
    pub fn exit_exact_bpt_in_for_tokens_out<'info>(
        ctx: Context<'_, '_, '_, 'info, PoolContext<'info>>,
        bpt_in: u64,
    ) -> Result<()> {
        let pool = &ctx.accounts.pool;
        let n    = pool.weights.len();

        require!(ctx.remaining_accounts.len() == n * 2, ErrorCode::LengthMismatch);
        require!(bpt_in > 0 && bpt_in <= pool.total_bpt, ErrorCode::MathUnderflow);

        // 1. balances
        let mut balances_fp = Vec::with_capacity(n);
        for i in 0..n {
            let vault_ai = &ctx.remaining_accounts[i * 2 + 1];
            let data     = vault_ai.try_borrow_data()?;
            let acct     = SplAccount::unpack_from_slice(&data)?;
            balances_fp.push(U256::from(acct.amount) * fixed::ONE);
        }

        // 2. maths
        let mut tokens_out = Vec::with_capacity(n);
        let bpt_in_fp      = U256::from(bpt_in) * fixed::ONE;
        let total_bpt_fp   = U256::from(pool.total_bpt) * fixed::ONE;
        let fee_fp         = U256::from(pool.swap_fee);
        for i in 0..n {
            let out_fp = weighted_math::calc_token_out_given_exact_bpt_in(
                balances_fp[i],
                U256::from(pool.weights[i]),
                bpt_in_fp,
                total_bpt_fp,
                fee_fp,
            );
            tokens_out.push((out_fp / fixed::ONE).as_u64());
        }

        // 3. burn BPT
        let token_prog = ctx.accounts.token_program.to_account_info();
        let burn_ctx = CpiContext::new(
            token_prog.clone(),
            Burn {
                mint:      ctx.accounts.lp_mint.clone(),
                from:      ctx.accounts.user_lp_account.clone(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::burn(burn_ctx, bpt_in)?;

        // 4. vault → user transfers
        let bump         = ctx.bumps.lp_mint_authority;
        let pool_key     = ctx.accounts.pool.key();
        let bump_arr     = [bump];
        let seed_slice: &[&[u8]] = &[
            b"lp-mint-authority",
            pool_key.as_ref(),
            &bump_arr,
        ];
        let signer_seeds = &[seed_slice];
        for i in 0..n {
            let cpi_accounts = Transfer {
                from:      ctx.remaining_accounts[i * 2 + 1].clone(),
                to:        ctx.remaining_accounts[i * 2].clone(),
                authority: ctx.accounts.lp_mint_authority.clone(),
            };
            token::transfer(
                CpiContext::new_with_signer(token_prog.clone(), cpi_accounts, signer_seeds),
                tokens_out[i],
            )?;
        }

        // 5. bookkeeping
        ctx.accounts.pool.total_bpt = ctx.accounts.pool
            .total_bpt
            .checked_sub(bpt_in)
            .ok_or(ErrorCode::MathUnderflow)?;
        Ok(())
    }

    /* ---------------------------------------------------------------
       Swap – exact in → out across two tokens
    ---------------------------------------------------------------- */
    pub fn swap_exact_token_in_for_token_out<'info>(
        ctx: Context<'_, '_, '_, 'info, SwapContext<'info>>,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<()> {
        // 1. read vault balances
        let balance_in_fp = {
            let data = ctx.accounts.vault_in.try_borrow_data()?;
            U256::from(SplAccount::unpack_from_slice(&data)?.amount) * fixed::ONE
        };
        let balance_out_fp = {
            let data = ctx.accounts.vault_out.try_borrow_data()?;
            U256::from(SplAccount::unpack_from_slice(&data)?.amount) * fixed::ONE
        };

        // 2. maths: how much out?
        let fee_fp      = U256::from(ctx.accounts.pool.swap_fee);
        let weights     = &ctx.accounts.pool.weights;
        let weight_in_fp  = U256::from(weights[0]);
        let weight_out_fp = U256::from(weights[1]);
        let amount_in_fp  = U256::from(amount_in) * fixed::ONE;
        let out_fp = weighted_math::calc_out_given_in(
            balance_in_fp,
            balance_out_fp,
            weight_in_fp,
            weight_out_fp,
            amount_in_fp,
            fee_fp,
        );
        let amount_out = (out_fp / fixed::ONE).as_u64();
        require!(amount_out >= minimum_amount_out, ErrorCode::MathUnderflow);

        // 3. transfer in (user → vault)
        let token_prog = ctx.accounts.token_program.to_account_info();
        let cpi_in = Transfer {
            from:      ctx.accounts.user_token_account_in.clone(),
            to:        ctx.accounts.vault_in.clone(),
            authority: ctx.accounts.user_authority.to_account_info(),
        };
        token::transfer(CpiContext::new(token_prog.clone(), cpi_in), amount_in)?;

        // 4. transfer out (vault → user)
        let bump      = ctx.bumps.lp_mint_authority;
        let pool_key  = ctx.accounts.pool.key();
        let bump_arr  = [bump];
        let seed_slice: &[&[u8]] = &[
            b"lp-mint-authority",
            pool_key.as_ref(),
            &bump_arr,
        ];
        let signer_seeds = &[seed_slice];
        let cpi_out = Transfer {
            from:      ctx.accounts.vault_out.clone(),
            to:        ctx.accounts.user_token_account_out.clone(),
            authority: ctx.accounts.lp_mint_authority.clone(),
        };
        token::transfer(
            CpiContext::new_with_signer(token_prog, cpi_out, signer_seeds),
            amount_out,
        )?;

        Ok(())
    }
}

/* ------------------------------------------------------------------
   Accounts: initialize & pool contexts
------------------------------------------------------------------ */
#[derive(Accounts)]
#[instruction(weights: Vec<u128>, swap_fee: u64)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: an already‐initialized VaultState account
    #[account(mut)]
    pub vault_state: Account<'info, VaultState>,

    pub vault_program: Program<'info, VaultProgram>,

    /// CHECK: The LP‐token mint for this pool (must match the one in `pool.lp_mint`)
    #[account(mut)]
    pub lp_mint: AccountInfo<'info>,

    /// CHECK: PDA mint authority for `lp_mint`; derived from `["lp-mint-authority", pool.key().as_ref()]`
    #[account(
        seeds = [b"lp-mint-authority", pool.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: AccountInfo<'info>,

    /// The Pool state PDA itself
    #[account(
        init,
        seeds = [b"pool-state", vault_state.key().as_ref()],
        bump,
        payer = payer,
        space = 8 + Pool::INIT_SPACE
    )]
    pub pool: Account<'info, Pool>,

    /// CHECK: Token program, used for minting; standard program
    pub token_program: Program<'info, Token>,

    /// CHECK: System program, used for account init; standard program
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PoolContext<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,

    /// CHECK: Same LP mint account as in InitializePool
    #[account(mut)]
    pub lp_mint: AccountInfo<'info>,

    /// CHECK: PDA mint authority; seed ensures the correct authority
    #[account(
        seeds = [b"lp-mint-authority", pool.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: AccountInfo<'info>,

    #[account(mut)]
    pub user: Signer<'info>,

    /// CHECK: User's LP token account for receiving minted BPT
    #[account(mut)]
    pub user_lp_account: AccountInfo<'info>,

    /// CHECK: Token program, used for transfers and minting
    pub token_program: Program<'info, Token>,
}

/* ------------------------------------------------------------------
   Accounts: swap context
------------------------------------------------------------------ */
#[derive(Accounts)]
pub struct SwapContext<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,

    /// CHECK: Vault account for the 'in' token; validated by seed off-chain
    #[account(mut)]
    pub vault_in: AccountInfo<'info>,

    /// CHECK: Vault account for the 'out' token; validated by seed off-chain
    #[account(mut)]
    pub vault_out: AccountInfo<'info>,

    #[account(mut)]
    pub user_authority: Signer<'info>,

    /// CHECK: User's token account for the 'in' mint; must be owned by user
    #[account(mut)]
    pub user_token_account_in: AccountInfo<'info>,

    /// CHECK: User's token account for the 'out' mint; must be owned by user
    #[account(mut)]
    pub user_token_account_out: AccountInfo<'info>,

    /// CHECK: PDA for LP mint authority; seed ensures correct authority
    #[account(
        seeds = [b"lp-mint-authority", pool.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: AccountInfo<'info>,

    /// CHECK: Token program, used for transfers
    pub token_program: Program<'info, Token>,
}

/* ------------------------------------------------------------------
   State & Errors
------------------------------------------------------------------ */
#[account]
pub struct Pool {
    pub vault: Pubkey,
    pub lp_mint: Pubkey,
    pub weights: Vec<u128>,
    pub swap_fee: u64,
    pub total_bpt: u64,
}
impl Pool {
    pub const INIT_SPACE: usize = 252;
}

#[error_code]
pub enum ErrorCode {
    #[msg("Vector length mismatch")]
    LengthMismatch,
    #[msg("Math underflow or overflow")]
    MathUnderflow,
}
