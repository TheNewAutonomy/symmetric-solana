use anchor_lang::prelude::*;
use anchor_spl::token::{self, MintTo, Token, Transfer};
use math::{fixed, weighted_math, U256};
use spl_token::state::Account as SplAccount;
use anchor_lang::solana_program::program_pack::Pack;

declare_id!("WPoo1QeY5T2r8j6YfGLwRoTSesFiNUFDXL9uBebzh1e");

#[program]
pub mod weighted_pool {
    use super::*;

    // ------------------------------------------------------------------
    // Initialise a pool
    // ------------------------------------------------------------------
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        weights: Vec<u128>,
        swap_fee: u64,
    ) -> Result<()> {
        require!(
            weights.len() == ctx.remaining_accounts.len(),
            ErrorCode::LengthMismatch
        );
        let pool = &mut ctx.accounts.pool;
        pool.vault   = ctx.accounts.vault.key();
        pool.lp_mint = ctx.accounts.lp_mint.key();
        pool.weights = weights;
        pool.swap_fee = swap_fee;
        pool.total_bpt = 0;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Join – user deposits *all* tokens, receives LP/BPT
    // remaining_accounts: [user_token0, vault_token0, user_token1, vault_token1, …]
    // ------------------------------------------------------------------
    pub fn join_exact_tokens_in_for_bpt_out<'info>(
        ctx: Context<'_, '_, '_, 'info, PoolContext<'info>>, // explicit lifetimes
        amounts_in: Vec<u64>,
    ) -> Result<()> {
        // --------------------
        // 0. Snapshot immutable pool fields first – avoids long mutable borrow.
        // --------------------
        let (weights, total_bpt, swap_fee) = {
            let p = &ctx.accounts.pool;
            (p.weights.clone(), p.total_bpt, p.swap_fee)
        };
        let n = weights.len();

        require!(ctx.remaining_accounts.len() == n * 2, ErrorCode::LengthMismatch);
        require!(amounts_in.len()            == n    , ErrorCode::LengthMismatch);

        // --------------------
        // 1.  Copy (user,vault) AccountInfos locally.
        // --------------------
        let pairs: Vec<(AccountInfo<'info>, AccountInfo<'info>)> = ctx
            .remaining_accounts
            .chunks_exact(2)
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect();

        // --------------------
        // 2.  Read current vault balances.
        // --------------------
        let mut balances_fp = Vec::<U256>::with_capacity(n);
        for (_, vault_ai) in pairs.iter() {
            let data  = vault_ai.try_borrow_data()?;
            let acct  = SplAccount::unpack_from_slice(&data)?;
            balances_fp.push(U256::from(acct.amount) * fixed::ONE);
        }

        // --------------------
        // 3.  Maths – how many BPT to mint?
        // --------------------
        let weights_fp : Vec<U256> = weights   .iter().map(|w| U256::from(*w)).collect();
        let amounts_fp : Vec<U256> = amounts_in.iter().map(|a| U256::from(*a) * fixed::ONE).collect();

        let bpt_out_fp = weighted_math::calc_bpt_out_given_exact_tokens_in(
            &balances_fp,
            &weights_fp,
            &amounts_fp,
            U256::from(total_bpt) * fixed::ONE,
            U256::from(swap_fee),
        );
        require!(bpt_out_fp > U256::zero(), ErrorCode::MathUnderflow);
        let bpt_out = (bpt_out_fp / fixed::ONE).as_u64();

        // --------------------
        // 4.  CPI transfers – user → vault.
        // --------------------
        let token_prog = ctx.accounts.token_program.to_account_info();

        for (i, (from_ai, to_ai)) in pairs.into_iter().enumerate() {
            let signer_ai = ctx.accounts.user.to_account_info();
            let cpi_accounts = Transfer {
                from:      from_ai,
                to:        to_ai,
                authority: signer_ai,
            };
            token::transfer(CpiContext::new(token_prog.clone(), cpi_accounts), amounts_in[i])?;
        }

        // --------------------
        // 5.  Mint BPT to the user.
        // --------------------
        let mint_cpi = CpiContext::new(
            token_prog,
            MintTo {
                mint:      ctx.accounts.lp_mint.clone(),
                to:        ctx.accounts.user_lp_account.clone(),
                authority: ctx.accounts.lp_mint_authority.clone(),
            },
        );
        token::mint_to(mint_cpi, bpt_out)?;

        // --------------------
        // 6.  Update pool state.
        // --------------------
        let pool_mut = &mut ctx.accounts.pool;
        pool_mut.total_bpt = pool_mut
            .total_bpt
            .checked_add(bpt_out)
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
    /// CHECK: vault PDA checked externally
    pub vault: AccountInfo<'info>,
    /// CHECK: SPL Token mint – verified via CPI before use
    #[account(mut)]
    pub lp_mint: AccountInfo<'info>,
    /// CHECK: PDA mint authority
    #[account(mut)]
    pub lp_mint_authority: AccountInfo<'info>,
    #[account(init, payer = payer, space = 8 + Pool::INIT_SPACE)]
    pub pool: Account<'info, Pool>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct PoolContext<'info> {
    #[account(mut)]
    pub pool: Account<'info, Pool>,
    /// CHECK: SPL Token mint – verified via CPI before use
    #[account(mut)]
    pub lp_mint: AccountInfo<'info>,
    /// CHECK: PDA mint authority
    pub lp_mint_authority: AccountInfo<'info>,
    #[account(mut)]
    pub user: Signer<'info>,
    /// CHECK: SPL Token account owned by `user` – verified in frontend/tests
    #[account(mut)]
    pub user_lp_account: AccountInfo<'info>,
    pub token_program: Program<'info, Token>,
    // remaining_accounts: user/vault token pairs
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
    pub const INIT_SPACE: usize = 32 + 32 + 4 + 16 * 4 + 8 + 8;
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
