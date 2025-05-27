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

    /// Register a new pool by bumping the pool_count
    pub fn register_pool(ctx: Context<RegisterPool>) -> Result<()> {
        let vault_state = &mut ctx.accounts.vault_state;
        vault_state.pool_count = vault_state
            .pool_count
            .checked_add(1)
            .ok_or(ErrorCode::Overflow)?;
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

#[derive(Accounts)]
pub struct RegisterPool<'info> {
    /// The vault state must be mutable, PDA'd by ["vault-state", owner]
    #[account(
        mut,
        seeds = [b"vault-state", vault_state.owner.as_ref()],
        bump,
        has_one = owner
    )]
    pub vault_state: Account<'info, VaultState>,

    /// Must match `vault_state.owner`
    pub owner: Signer<'info>,

    /// Required for CPI safety, though we don't invoke it here
    pub system_program: Program<'info, System>,
}

impl VaultState {
    pub const LEN: usize = 32 + 8;
}

#[error_code]
pub enum ErrorCode {
    #[msg("Overflow adding pool")]
    Overflow,
}
