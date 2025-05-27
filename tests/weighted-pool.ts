import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import {
  createMint,
  getOrCreateAssociatedTokenAccount,
} from "@solana/spl-token";

import { Vault }        from "../target/types/vault";
import { WeightedPool } from "../target/types/weighted_pool";

const provider = anchor.AnchorProvider.env();
anchor.setProvider(provider);

const vaultProgram    = anchor.workspace.Vault        as Program<Vault>;
const weightedProgram = anchor.workspace.WeightedPool as Program<WeightedPool>;

const TOKEN_PROGRAM_ID = new anchor.web3.PublicKey(
  "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
);

/**
 * Derive all the PDAs your Rust code expects:
 *  - vaultState  (from the Vault program)
 *  - poolState   (for weighted‐pool)
 *  - lpMintAuth  (the “lp‐mint‐authority” PDA)
 */
function derivePdas(owner: anchor.web3.PublicKey) {
  const [vaultState] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("vault-state"), owner.toBuffer()],
    vaultProgram.programId
  );
  const [poolState] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("pool-state"), vaultState.toBuffer()],
    weightedProgram.programId
  );
  const [lpMintAuth] = anchor.web3.PublicKey.findProgramAddressSync(
    [Buffer.from("lp-mint-authority"), poolState.toBuffer()],
    weightedProgram.programId
  );
  return { vaultState, poolState, lpMintAuth };
}

describe("weighted-pool", () => {
  it("initialises the weighted pool", async () => {
    const { vaultState, poolState, lpMintAuth } =
      derivePdas(provider.wallet.publicKey);

    // 1. Make sure the Vault is already initialized
    try {
      await vaultProgram.account.vaultState.fetch(vaultState);
    } catch {
      await vaultProgram.methods
        .initialize(provider.wallet.publicKey)
        .accounts({
          vaultState,
          payer:         provider.wallet.publicKey,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .rpc();
    }

    // 2. Create the LP mint, with the PDA as its mint authority
    const lpMintKp = anchor.web3.Keypair.generate();
    await createMint(
      provider.connection,
      provider.wallet.payer,  // fee-payer
      lpMintAuth,             // mint-authority = PDA
      null,                   // freeze-authority (none)
      6,                      // decimals
      lpMintKp                // new mint keypair
    );

    // 3. User needs an ATA for the LP tokens
    const userLpAta = await getOrCreateAssociatedTokenAccount(
      provider.connection,
      provider.wallet.payer,
      lpMintKp.publicKey,
      provider.wallet.publicKey
    );

    // 4. Call our initialize_pool instruction
    await weightedProgram.methods
    .initializePool(
      [new anchor.BN(1_000_000)], // weights
      new anchor.BN(0)            // swap_fee
    )
    .accounts({
      vaultState:    vaultState,                      // ← rename from “vault”
      vaultProgram:  vaultProgram.programId,          // ← must pass the CPI‐target program
      pool:          poolState,
      lpMint:        lpMintKp.publicKey,
      lpMintAuthority: lpMintAuth,
      payer:         provider.wallet.publicKey,
      tokenProgram:  TOKEN_PROGRAM_ID,
      systemProgram: anchor.web3.SystemProgram.programId,
    })
    .remainingAccounts([
      {
        pubkey:     provider.wallet.publicKey,
        isWritable: false,
        isSigner:   false,
      },
    ])
    .rpc();


    console.log("✅ weighted-pool initialised");
    console.log("   vault_state  :", vaultState.toBase58());
    console.log("   pool_state   :", poolState.toBase58());
    console.log("   lp_mint      :", lpMintKp.publicKey.toBase58());
    console.log("   lp_mint_auth :", lpMintAuth.toBase58());
    console.log("   user_lp_ata  :", userLpAta.address.toBase58());
  });
});
