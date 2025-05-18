import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { WeightedPool } from "../target/types/weighted_pool";
import { Vault }        from "../target/types/vault";

describe("weighted-pool", () => {
  // ---------- Anchor provider ----------
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  // Typed program handles
  const vaultProgram     = anchor.workspace.Vault        as Program<Vault>;
  const weightedProgram  = anchor.workspace.WeightedPool as Program<WeightedPool>;

  // Canonical SPL-Token program id
  const TOKEN_PROGRAM_ID = new anchor.web3.PublicKey(
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
  );

  it("initializes", async () => {
    // ------------------------------------------------------------------
    // 1. Derive the PDAs that the pool initialize instruction requires
    // ------------------------------------------------------------------
    const [vaultStatePda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("vault-state"), provider.wallet.publicKey.toBuffer()],
      vaultProgram.programId,
    );

    const [poolStatePda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("pool-state"), vaultStatePda.toBuffer()],
      weightedProgram.programId,
    );

    const [lpMintPda] = anchor.web3.PublicKey.findProgramAddressSync(
      [Buffer.from("lp-mint"), poolStatePda.toBuffer()],
      weightedProgram.programId,
    );

    // ------------------------------------------------------------------
    // 2. (One-time) make sure the Vault is initialised, because
    //    the pool points to it.  If you already run the Vault test
    //    first you can comment this block out.
    // ------------------------------------------------------------------
    try {
      await vaultProgram.account.vaultState.fetch(vaultStatePda);
    } catch (_) {
      await vaultProgram.methods
        .initialize(provider.wallet.publicKey)               // owner
        .accounts({
          vaultState:    vaultStatePda,
          payer:         provider.wallet.publicKey,
          systemProgram: anchor.web3.SystemProgram.programId,
        })
        .rpc();
    }

    // ------------------------------------------------------------------
    // 3. Call weighted_pool::initialize
    // ------------------------------------------------------------------
    await weightedProgram.methods
      .initialize(vaultStatePda, new anchor.BN(1_000_000))   // vault pubkey, initial weight
      .accounts({
        vault:         vaultStatePda,
        poolState:     poolStatePda,
        lpMint:        lpMintPda,
        payer:         provider.wallet.publicKey,
        tokenProgram:  TOKEN_PROGRAM_ID,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    console.log("Weighted-pool initialised:");
    console.log("  vault_state =", vaultStatePda.toBase58());
    console.log("  pool_state  =", poolStatePda.toBase58());
    console.log("  lp_mint     =", lpMintPda.toBase58());
  });
});
