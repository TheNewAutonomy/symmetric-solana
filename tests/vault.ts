import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { Vault }   from "../target/types/vault";

describe("vault", () => {
  // Use the local validator & default wallet
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Vault as Program<Vault>;

  it("initializes", async () => {
    // ---- 1.  Derive the vault-state PDA (seed = "vault-state" + payer pubkey) ----
    const [vaultStatePda] = anchor.web3.PublicKey.findProgramAddressSync(
      [
        Buffer.from("vault-state"),
        provider.wallet.publicKey.toBuffer(),
      ],
      program.programId
    );

    // ---- 2.  Call initialize(owner) and pass the PDA + required accounts ----
    await program.methods
      .initialize(provider.wallet.publicKey)              // owner argument
      .accounts({
        vaultState:    vaultStatePda,
        payer:         provider.wallet.publicKey,
        systemProgram: anchor.web3.SystemProgram.programId,
      })
      .rpc();

    console.log("Vault initialized at", vaultStatePda.toBase58());
  });
});
