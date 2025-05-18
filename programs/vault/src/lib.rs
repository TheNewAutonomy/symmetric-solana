use anchor_lang::prelude::*;

declare_id!("CsSfsxZcni7DTeLvxTvzbFsLa3PdvyQCKmakzmXeM2fz");

#[program]
pub mod vault {
    use super::*;

    /// Initialize the Vault state with an owner and zero pools
    pub fn initialize(ctx: Context<Initialize>, owner: Pubkey) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        vault_state.owner = owner;
        vault_state.pool_count = 0;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// Vault state account PDA
    #[account(
        init,
        payer = payer,
        space = 8 + VaultState::LEN,
        seeds = [b"vault-state", payer.key().as_ref()],
        bump
    )]
    pub vault_state: Account<'info, VaultState>,

    /// The signer paying for account creation
    #[account(mut)]
    pub payer: Signer<'info>,

    /// System program for account creation
    pub system_program: Program<'info, System>,
}

/// On-chain Vault state: owner and pool count
#[account]
pub struct VaultState {
    pub owner: Pubkey,
    pub pool_count: u64,
}

impl VaultState {
    pub const LEN: usize = 32 + 8;
}