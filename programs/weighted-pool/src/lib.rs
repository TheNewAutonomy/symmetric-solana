use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, MintTo, Token, Transfer};
use math::{fixed, weighted_math, U256};
use spl_token::state::Account as SplAccount;
use anchor_lang::solana_program::program_pack::Pack;

// ---------------------------------------------------------------------
// Program ID (change when you deploy a new one)
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
        // One weight per vault token PDA handed in via remaining accounts
        require!(
            weights.len() == ctx.remaining_accounts.len(),
            ErrorCode::LengthMismatch
        );

        let pool              = &mut ctx.accounts.pool;
        pool.vault            = ctx.accounts.vault.key();
        pool.lp_mint          = ctx.accounts.lp_mint.key();
        pool.weights          = weights;
        pool.swap_fee         = swap_fee;
        pool.total_bpt        = 0;
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

        require!(
            ctx.remaining_accounts.len() == n * 2,
            ErrorCode::LengthMismatch
        );
        require!(amounts_in.len() == n, ErrorCode::LengthMismatch);

        /* -------- 1. read vault balances -------- */
        let mut balances_fp = Vec::with_capacity(n);
        for i in 0..n {
            let vault_ai = &ctx.remaining_accounts[i * 2 + 1];
            let data     = vault_ai.try_borrow_data()?;
            let acct     = SplAccount::unpack_from_slice(&data)?;
            balances_fp.push(U256::from(acct.amount) * fixed::ONE);
        }

        /* -------- 2. maths -------- */
        let weights_fp: Vec<U256> =
            pool.weights.iter().map(|w| U256::from(*w)).collect();
        let amounts_fp: Vec<U256> =
            amounts_in.iter().map(|a| U256::from(*a) * fixed::ONE).collect();

        let bpt_out_fp = weighted_math::calc_bpt_out_given_exact_tokens_in(
            &balances_fp,
            &weights_fp,
            &amounts_fp,
            U256::from(pool.total_bpt) * fixed::ONE,
            U256::from(pool.swap_fee),
        );
        require!(bpt_out_fp > U256::zero(), ErrorCode::MathUnderflow);
        let bpt_out = (bpt_out_fp / fixed::ONE).as_u64();

        /* -------- 3. CPI transfers (user → vault) -------- */
        let token_prog = ctx.accounts.token_program.to_account_info();
        let user_auth  = ctx.accounts.user.to_account_info();
        for i in 0..n {
            let from_ai = ctx.remaining_accounts[i * 2].clone();
            let to_ai   = ctx.remaining_accounts[i * 2 + 1].clone();
            let cpi = Transfer {
                from: from_ai,
                to:   to_ai,
                authority: user_auth.clone(),
            };
            token::transfer(
                CpiContext::new(token_prog.clone(), cpi),
                amounts_in[i],
            )?;
        }

        /* -------- 4. mint BPT -------- */
        let bump         = ctx.bumps.lp_mint_authority;
        let pool_key     = ctx.accounts.pool.key();
        let bump_arr     = [bump];                                         // lives long enough
        let seed_slice: &[&[u8]] = &[
            b"lp-mint-authority",
            pool_key.as_ref(),
            &bump_arr,
        ];
        // slice-of-slices (&[&[&[u8]]])
        let signer_seeds_arr: [&[&[u8]]; 1] = [seed_slice];
        let signer_seeds                    = &signer_seeds_arr[..];

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

        /* -------- 5. bookkeeping -------- */
        ctx.accounts.pool.total_bpt = ctx
            .accounts
            .pool
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

        require!(
            ctx.remaining_accounts.len() == n * 2,
            ErrorCode::LengthMismatch
        );
        require!(
            bpt_in > 0 && bpt_in <= pool.total_bpt,
            ErrorCode::MathUnderflow
        );

        /* -------- 1. balances -------- */
        let mut balances_fp = Vec::with_capacity(n);
        for i in 0..n {
            let vault_ai = &ctx.remaining_accounts[i * 2 + 1];
            let data     = vault_ai.try_borrow_data()?;
            let acct     = SplAccount::unpack_from_slice(&data)?;
            balances_fp.push(U256::from(acct.amount) * fixed::ONE);
        }

        /* -------- 2. maths -------- */
        let mut tokens_out = Vec::with_capacity(n);
        let bpt_in_fp    = U256::from(bpt_in) * fixed::ONE;
        let total_bpt_fp = U256::from(pool.total_bpt) * fixed::ONE;
        let fee_fp       = U256::from(pool.swap_fee);
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

        /* -------- 3. burn BPT -------- */
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

        /* -------- 4. vault → user transfers -------- */
        let bump         = ctx.bumps.lp_mint_authority;
        let pool_key     = ctx.accounts.pool.key();
        let bump_arr     = [bump];
        let seed_slice: &[&[u8]] = &[
            b"lp-mint-authority",
            pool_key.as_ref(),
            &bump_arr,
        ];
        let signer_seeds_arr: [&[&[u8]]; 1] = [seed_slice];
        let signer_seeds                    = &signer_seeds_arr[..];

        for i in 0..n {
            let vault_ai = ctx.remaining_accounts[i * 2 + 1].clone();
            let user_ai  = ctx.remaining_accounts[i * 2].clone();
            let cpi = Transfer {
                from:      vault_ai,
                to:        user_ai,
                authority: ctx.accounts.lp_mint_authority.clone(),
            };
            token::transfer(
                CpiContext::new_with_signer(
                    token_prog.clone(),
                    cpi,
                    signer_seeds,
                ),
                tokens_out[i],
            )?;
        }

        /* -------- 5. bookkeeping -------- */
        ctx.accounts.pool.total_bpt = ctx
            .accounts
            .pool
            .total_bpt
            .checked_sub(bpt_in)
            .ok_or(ErrorCode::MathUnderflow)?;
        Ok(())
    }
}

/* ------------------------------------------------------------------
   Accounts
------------------------------------------------------------------ */
#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: Verified in the Vault program.
    pub vault: AccountInfo<'info>,

    /// CHECK: SPL-Token mint that represents the pool’s LP/BPT.
    #[account(mut)]
    pub lp_mint: AccountInfo<'info>,

    /// CHECK: PDA (seeds = ["lp-mint-authority", pool.key()], bump)
    ///        used as the mint authority for `lp_mint`.
    #[account(
        seeds = [b"lp-mint-authority", pool.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: AccountInfo<'info>,

    #[account(init, payer = payer, space = 8 + Pool::INIT_SPACE)]
    pub pool: Account<'info, Pool>,

    pub system_program: Program<'info, System>,
    pub token_program:  Program<'info, Token>,
}

#[derive(Accounts)]
pub struct PoolContext<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,

    /// CHECK: Same LP mint as above.
    #[account(mut)]
    pub lp_mint: AccountInfo<'info>,

    /// CHECK: PDA mint authority (same seeds as above).
    #[account(
        seeds = [b"lp-mint-authority", pool.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: AccountInfo<'info>,

    #[account(mut)]
    pub user: Signer<'info>,

    /// CHECK: User’s LP token account.
    #[account(mut)]
    pub user_lp_account: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,

    // remaining accounts: [user_tok0, vault_tok0, …]
}

/* ------------------------------------------------------------------
   State
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
    // 32 + 32 + 4 + 16*10 + 8 + 8 = 252
    pub const INIT_SPACE: usize = 252;
}

/* ------------------------------------------------------------------
   Errors
------------------------------------------------------------------ */
#[error_code]
pub enum ErrorCode {
    #[msg("Vector length mismatch")]
    LengthMismatch,
    #[msg("Math underflow or overflow")]
    MathUnderflow,
}
