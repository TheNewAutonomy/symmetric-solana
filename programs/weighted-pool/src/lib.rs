use anchor_lang::prelude::*;

declare_id!("DD3HQQHjNnAKqq9eX7RyNW84hnRyBMrzsoc8AxkuvNzZ");

#[program]
pub mod weighted_pool {
    use super::*;

    pub fn initialize(
        ctx: Context<Initialize>,
        vault: Pubkey,
        initial_weight: u128,
    ) -> Result<()> {
        let state = &mut ctx.accounts.pool_state;
        state.vault        = vault;
        state.lp_mint      = ctx.accounts.lp_mint.key();
        state.total_weight = initial_weight;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// CHECK: vault PDA, only used as a seed; its contents are not read
    pub vault: UncheckedAccount<'info>,

    #[account(
        init,
        payer  = payer,
        space  = 8 + PoolState::LEN,
        seeds  = [b"pool-state", vault.key().as_ref()],
        bump
    )]
    pub pool_state: Account<'info, PoolState>,

    /// CHECK: new SPL-Token mint; created here and owned by the program, so no prior data to validate
    #[account(
        init,
        payer  = payer,
        space  = 82,                // fixed size of an SPL Mint account
        seeds  = [b"lp-mint", pool_state.key().as_ref()],
        bump
    )]
    pub lp_mint: UncheckedAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: must equal the canonical SPL-Token program id
    pub token_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[account]
pub struct PoolState {
    pub vault: Pubkey,
    pub lp_mint: Pubkey,
    pub total_weight: u128,
}
impl PoolState {
    pub const LEN: usize = 32 + 32 + 16;
}
